//! 段 5：計画と見積もり（architecture.md §3・§3.1・§4.5・§6.1・§8・ADR-37・ADR-38）。
//!
//! - 計画（`Tree::plan`）：倍率から、節ごとに平坦化した点・破線・線の外形を作り、固定小数の辺（y の順）と、画素の外接矩形・
//!   色・塗りの規則を持つ「描く項目」にする（塗りと線で 1 つずつ。`paint-order` の順）。`Plan` は範囲に依らない
//! - 見積もり（`Tree::estimate`）：点を作らずに、節ごとの数え（分ける数の和・部分パスの数・描く座標系の制御多角形の長さ・
//!   線を作る座標系の制御多角形の長さと曲がる角の和）から閉じた式で上限を求める。曲がる角の和は、平坦化した折れ線の
//!   曲がる角の和以上（内接する折れ線の曲がる角は曲線の全曲率以下、Bézier の全曲率は制御多角形の曲がる角以下）なので、
//!   round の継ぎ目の円弧の点の和の上限になる
//! - 辺の置き場は見積もりの上限の数で最初に取り（伸ばさない）、最後に使った長さに詰めて写す（持ち続ける量は実際の量。
//!   項目の置き場は見積もりの容量のまま）。一時の置き場は要るものだけ取る。キャンバスと交わらない節は見積もりも計画も飛ばす
//! - 範囲の見積もり（`Plan::estimate_region`）は、計画で持った辺ごとの上限（辺を項目の矩形の行で切った部分の、セル ≤
//!   (|Δx| + |Δy|) / 1 画素 + 4 と 行の数 × (幅 + 3) の小さい方）から求める。
//!   同じ形の式なので、全体に含まれる範囲の見積もりは `RenderEstimate.render` を超えない（FR-OUT-06）

use crate::budget::{Budget, Estimate};
use crate::cancel::Cancel;
use crate::cost;
use crate::doc::PixelSize;
use crate::error::{Error, Invalid, InvalidReason, Resource, Stage};
use crate::geom::flatten::{BOX, flatten_seg, pieces};
use crate::geom::stroke::{Cap, Dashes, Join, LoopSink, Pen, Poly, PolyBuf, dash_polyline, stroke_polyline};
use crate::geom::{PathRef, PathSeg, Point, Transform};
use crate::limits::RenderLimits;
use crate::math;
use crate::mem::{Frozen, MVec, Plain, lower_bound_try, stable_sort};
use crate::raster::{Edge, FillRule, ONE, PixelBox, push_segment};
use crate::tree::{
    CAP_ROUND, CAP_SQUARE, CRISP, EVEN_ODD, HAS_FILL, HAS_STROKE, JOIN_BEVEL, JOIN_ROUND, NON_SCALING,
    PATH_LENGTH_ZERO, PathNode, STROKE_FIRST, Tree,
};

/// 平坦化の許容の誤差（画素。仮。ADR-89 (4) で差の例を見てから決める）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Quality {
    pub(crate) tol: f64,
}

impl Quality {
    pub(crate) const DEFAULT: Quality = Quality { tol: 0.1 };
}

/// 描く設定。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    /// 倍率（正の有限の数。描く座標系 = ユーザー単位 × 倍率。FR-OUT-03）
    pub scale: f64,
    pub limits: RenderLimits,
}

impl RenderOptions {
    /// 倍率 1・既定の上限。
    pub const DEFAULT: RenderOptions = RenderOptions { scale: 1.0, limits: RenderLimits::DEFAULT };
}

/// 計画と描く前の見積もり。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderEstimate {
    /// 計画（`Plan` が持ち続ける量を含む）
    pub plan: Estimate,
    /// 全体を 1 度描く分
    pub render: Estimate,
}

/// 描く項目（塗りか線の 1 つ）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DrawItem {
    /// 辺の範囲（始めと数）
    pub(crate) edges: [u32; 2],
    /// 画素の外接矩形（キャンバスの中。x0・y0・x1・y1）
    pub(crate) bbox: [i64; 4],
    /// セルの数と、辺が行にかかる数の上限（どの範囲でも）
    pub(crate) cells: u64,
    pub(crate) edge_rows: u64,
    /// 乗算済みの色
    pub(crate) color: [u8; 4],
    /// 塗りの規則（0 非ゼロ・1 偶奇）と反エイリアス（1 あり）
    pub(crate) rule: u8,
    pub(crate) aa: u8,
    pub(crate) pad: [u8; 2],
    /// 辺の高さ（固定小数）の最大（範囲にかかりうる辺の始めを 2 分の探索で求める。§6.1）
    pub(crate) max_dy: u32,
    pub(crate) pad2: [u8; 4],
}

impl Plain for DrawItem {}

impl DrawItem {
    pub(crate) fn pixel_box(&self) -> PixelBox {
        PixelBox { x0: self.bbox[0], y0: self.bbox[1], x1: self.bbox[2], y1: self.bbox[3] }
    }

    pub(crate) fn fill_rule(&self) -> FillRule {
        if self.rule == 1 { FillRule::EvenOdd } else { FillRule::NonZero }
    }
}

/// 計画。何も借りない（`'static`・`Send + Sync`）。作ったときの上限を持つ。
#[derive(Debug)]
pub struct Plan {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) items: Frozen<DrawItem>,
    pub(crate) edges: Frozen<Edge>,
    /// 1 つの項目の辺の数の最大（描く段の、行にかかる辺の置き場の大きさ）
    pub(crate) max_item_edges: u64,
    pub(crate) limits: RenderLimits,
    /// 背景（乗算済み。透明なら塗らない。ADR-97 (4)）
    pub(crate) background: [u8; 4],
}

/// 背景を塗る手間（`pixels` 画素。透明なら 0。描く段と範囲の見積もりの式）。
pub(crate) const fn background_work(background: [u8; 4], pixels: u64) -> u64 {
    if background[3] == 0 { 0 } else { pixels.saturating_mul(cost::PAINT_PIXEL) }
}

impl Plan {
    /// 計画が持ち続ける量。
    pub fn held_bytes(&self) -> u64 {
        self.items.bytes().saturating_add(self.edges.bytes())
    }

    /// 画素の大きさ。
    pub fn pixel_size(&self) -> PixelSize {
        PixelSize { width: self.width, height: self.height }
    }

    pub(crate) fn edges_of(&self, it: &DrawItem) -> &[Edge] {
        let start = it.edges[0] as usize;
        self.edges.as_slice().get(start..start.saturating_add(it.edges[1] as usize)).unwrap_or(&[])
    }

    /// 項目の辺のうち、画素の行の範囲 `b` にかかりうるもの（y0 の順に並んだ辺の、y0 ≥ 範囲の上 − 高さの最大 から y0 < 範囲の
    /// 下 まで。前の辺は範囲の上で終わり、後の辺は範囲の下で始まる）。2 分の探索を 2 度払う（範囲の描画の手間を範囲の外の
    /// 辺に比例させない。architecture.md §6.1・単位 2 の 3 回目のレビュー）。
    pub(crate) fn region_edges(&self, it: &DrawItem, b: &PixelBox, budget: &Budget<'_>) -> Result<&[Edge], Error> {
        let all = self.edges_of(it);
        let top = b.y0.saturating_mul(ONE).saturating_sub(i64::from(it.max_dy));
        let bottom = b.y1.saturating_mul(ONE);
        let start = lower_bound_try(budget, all, |e| Ok(i64::from(e.y0) < top))?;
        let end = lower_bound_try(budget, all, |e| Ok(i64::from(e.y0) < bottom))?;
        Ok(all.get(start..end.max(start)).unwrap_or(&[]))
    }
}

/// 項目の範囲の描画の 2 分の探索の手間（[`Plan::region_edges`]。辺 `e` 本）。
pub(crate) fn region_search(e: u64) -> u64 {
    cost::search(e).saturating_mul(2)
}

/// 呼び手の誤り。
fn argument() -> Error {
    Error::Invalid(Invalid { reason: InvalidReason::Argument, position: None })
}

/// 節の数え（閉じた式の見積もりと、計画の置き場の大きさに使う）。
#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    /// 平坦化の点（部分パスの始めの点を含む）・曲線（2 次・3 次・弧）の区間の数・部分パスの数・1 つの部分パスの点の最大
    points: u64,
    curves: u64,
    /// 曲線から出る平坦化の点の数と、曲線の中の曲がる角の和（滑らかな点の外側の円弧の上限）
    curve_points: u64,
    curve_turning: f64,
    /// 区間の数（移動と閉じを含む。計画が部分パスを集める歩きで区間ごとに払う分）
    segs: u64,
    subpaths: u64,
    max_sub: u64,
    /// 描く座標系の制御多角形の長さ（線分・閉じ・曲線の制御多角形・弧は |Δθ| × 最大の半径）
    len_dev: f64,
    /// 要素の座標系（ユーザー単位）の制御多角形の長さ（pathLength の倍）
    len_user: f64,
    /// 線を作る座標系の制御多角形の長さと、曲がる角の和（部分パスごとに π を足す）
    len_stroke: f64,
    turning: f64,
    /// 見る箱（キャンバスを線の届く幅だけ広げたもの）にかかる区間の、線を作る座標系の長さ（線分は箱の中の分だけ）と、
    /// かかる区間の数（破線の部分の数の上限。ADR-30 (4)）
    len_stroke_in: f64,
    segs_in: u64,
    /// 行の帯（キャンバスの行を線の届く幅だけ広げた横の帯）にかかる区間の、描く座標系と線を作る座標系の長さと平坦化の点の
    /// 数（帯の外の辺はキャンバスの行に入らない。描く段の行とセルの数の上限）。塗りの暗黙の閉じの線分も足す
    len_dev_band: f64,
    len_stroke_band: f64,
    pts_band: u64,
}

/// 見る箱（描く座標系。キャンバスを広げたもの）。破線の部分はこの箱にかかるものだけを作る（範囲に依らない）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct View {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl View {
    /// キャンバス `canvas` を `reach`（描く座標系の線の届く幅）と 2 画素の余白で広げた箱。
    fn new(canvas: PixelBox, reach: f64) -> View {
        let m = reach + 2.0;
        View { x0: canvas.x0 as f64 - m, y0: canvas.y0 as f64 - m, x1: canvas.x1 as f64 + m, y1: canvas.y1 as f64 + m }
    }

    /// 行の帯（横は限りなし）。
    fn band(canvas: PixelBox, reach: f64) -> View {
        let v = View::new(canvas, reach);
        View { x0: f64::NEG_INFINITY, x1: f64::INFINITY, ..v }
    }

    /// 矩形を含むか。
    fn contains(&self, r: &crate::geom::Rect) -> bool {
        r.x0 >= self.x0 && r.x1 <= self.x1 && r.y0 >= self.y0 && r.y1 <= self.y1
    }

    /// 矩形と交わるか（NaN は交わるとする：落とさない側）。
    fn meets(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
        !(x1 < self.x0 || x0 > self.x1 || y1 < self.y0 || y0 > self.y1)
    }

    /// 線分を箱で切った端（箱にかからなければ None）。外の端は箱の境の上の点にし、境に近い方の端から内挿して求める
    /// （計画の辺の切り方 `raster::push_segment` と同じ。巨大な線分の箱の中の部分を、t の丸めで失わない。ファズと単位 2 の
    /// 3 回目のレビュー）。
    fn clip(&self, a: Point, b: Point) -> Option<(Point, Point)> {
        let (p, q) = crate::raster::clip_y(a, b, self.y0, self.y1)?;
        crate::raster::clip_x(p, q, self.x0, self.x1)
    }

    /// 線分のうち箱の中の割合（0〜1。有限でなければ 1）。箱で切った部分の長さ ÷ 全体の長さ（長い方の軸の差で比べる）。
    fn inside_fraction(&self, a: Point, b: Point) -> f64 {
        if !a.finite() || !b.finite() {
            return 1.0;
        }
        if a == b {
            return if self.meets(a.x, a.y, a.x, a.y) { 1.0 } else { 0.0 };
        }
        let Some((p, q)) = self.clip(a, b) else { return 0.0 };
        let (dx, dy) = ((b.x - a.x).abs(), (b.y - a.y).abs());
        if !(dx.is_finite() && dy.is_finite()) {
            return 1.0;
        }
        let f = if dx >= dy { (q.x - p.x).abs() / dx } else { (q.y - p.y).abs() / dy };
        if f.is_finite() { math::min(f, 1.0) } else { 1.0 }
    }
}

/// 線分（描く座標系の `p` → `r`）のうち、描く段の行とセルにかかる分の割合（0〜1。行の帯 `band` の中の割合以下）：見る箱
/// `view` の中の部分は長さで、帯の中で箱の横の外の部分は y の差だけで数える（描く段は範囲の左の外を 1 列にまとめ、右の外を
/// 落とすので、横の外の部分は行ごとに 1 セル。巨大な線分の、キャンバスの横の外の長さを数えない）。有限でなければ帯の割合。
fn raster_fraction(view: &View, band: &View, p: Point, r: Point) -> f64 {
    let g = band.inside_fraction(p, r);
    if g <= 0.0 {
        return 0.0;
    }
    let l = math::hypot(r.x - p.x, r.y - p.y);
    if !(l.is_finite() && l > 0.0) {
        return g;
    }
    let Some((a, b)) = band.clip(p, r) else { return g };
    let dy_band = (b.y - a.y).abs();
    let f = view.inside_fraction(p, r);
    let dy_view = view.clip(p, r).map_or(0.0, |(c, d)| (d.y - c.y).abs());
    let out = dy_band - dy_view;
    let h = (f * l + if out > 0.0 { out } else { 0.0 }) / l;
    if h.is_finite() { math::min(h, g) } else { g }
}

/// 曲線の区間の、描く座標系の y の変化の上限（制御多角形の y の差の和。曲線は制御多角形より y の変化が小さい）。弧と
/// ほかの区間は None。
fn curve_dy(seg: &PathSeg, t: &Transform) -> Option<f64> {
    let dy = |a: Point, b: Point| (t.apply(b).y - t.apply(a).y).abs();
    match *seg {
        PathSeg::Quad(a, b, e) => Some(dy(a, b) + dy(b, e)),
        PathSeg::Cubic(a, b, d, e) => Some(dy(a, b) + dy(b, d) + dy(d, e)),
        _ => None,
    }
}

/// 2 つの向きの間の角の大きさ（0〜π。どちらかが長さ 0 なら 0）。
fn turn_between(a: Point, b: Point) -> f64 {
    let cross = a.x * b.y - a.y * b.x;
    let dot = a.x * b.x + a.y * b.y;
    if (a.x == 0.0 && a.y == 0.0) || (b.x == 0.0 && b.y == 0.0) {
        return 0.0;
    }
    math::atan2(cross, dot).abs()
}

/// 制御多角形の曲がる角の和（線を作る座標系。部分パスごとに π と、部分パスの終わりから始めへの角も足す。弧は接線で）。
/// 見積もりの `count` と、計画が round の継ぎ目の点を抑える（[`round_join_extra`]）ときに、同じ区間を同じ順で足すので同じ値に
/// なる（丸めで乱れた折れ線でも、計画の継ぎ目の点が見積もりを超えない。単位 1 の 4 回目のレビュー）。
#[derive(Debug, Clone, Copy, Default)]
struct Turning {
    sum: f64,
    /// 曲線（2 次・3 次・弧）の中の曲がる角の和（区間のつなぎ目の角を含まない）と、曲線の区間の数
    curve: f64,
    curves: u64,
    first: Option<Point>,
    last: Option<Point>,
}

impl Turning {
    /// 部分パスを閉じる（終わりから始めへの角）。
    fn close(&mut self) {
        if let (Some(f), Some(l)) = (self.first, self.last) {
            self.sum += turn_between(l, f);
        }
        self.first = None;
        self.last = None;
    }

    /// 向き `d` の区間を足す（長さ 0 は飛ばす）。
    fn dir(&mut self, d: Point) {
        if d.x == 0.0 && d.y == 0.0 {
            return;
        }
        if let Some(l) = self.last {
            self.sum += turn_between(l, d);
        }
        if self.first.is_none() {
            self.first = Some(d);
        }
        self.last = Some(d);
    }

    /// 区間 1 つを足す（`s` は線を作る座標系への変形）。
    fn feed(&mut self, seg: &PathSeg, s: &Transform) {
        let v = |a: Point, b: Point| s.apply_vec(Point::new(b.x - a.x, b.y - a.y));
        match *seg {
            PathSeg::Move(_) => {
                self.close();
                self.sum += core::f64::consts::PI;
            }
            PathSeg::Close(a, b) => {
                let d = v(a, b);
                if let Some(l) = self.last {
                    self.sum += turn_between(l, d);
                }
                if d.x != 0.0 || d.y != 0.0 {
                    self.last = Some(d);
                }
                self.close();
            }
            PathSeg::Line(a, b) => self.dir(v(a, b)),
            PathSeg::Quad(a, b, e) => {
                self.curves = self.curves.saturating_add(1);
                self.dir(v(a, b));
                let start = self.sum;
                self.dir(v(b, e));
                self.curve += self.sum - start;
            }
            PathSeg::Cubic(a, b, d, e) => {
                self.curves = self.curves.saturating_add(1);
                self.dir(v(a, b));
                let start = self.sum;
                self.dir(v(b, d));
                self.dir(v(d, e));
                self.curve += self.sum - start;
            }
            PathSeg::ArcTo(_, arc) => {
                // 始めと終わりの接線
                let tangent = |th: f64| {
                    let [m0, m1, m2, m3] = arc.m;
                    let (sn, cs) = (math::sin(th), math::cos(th));
                    let sign = if arc.dtheta < 0.0 { -1.0 } else { 1.0 };
                    s.apply_vec(Point::new(sign * (-m0 * sn + m1 * cs), sign * (-m2 * sn + m3 * cs)))
                };
                // 弧の中の曲がる角：線を作る座標系の楕円の接線を、始め・真ん中・終わりで求めて隣どうしの角を足す（半分ずつは
                // 掃く角が π 未満なので、楕円の接線は単調に回り、これで正確。|Δθ| は円でない楕円で曲がる角より小さい。単位 1・2 の
                // レビュー）。接線が長さ 0（つぶれた楕円）なら π ⌈|Δθ| / π⌉
                let (t0, tm, t1) =
                    (tangent(arc.theta1), tangent(arc.theta1 + arc.dtheta / 2.0), tangent(arc.theta1 + arc.dtheta));
                let flat = |d: Point| !(d.x.is_finite() && d.y.is_finite()) || (d.x == 0.0 && d.y == 0.0);
                let inner = if flat(t0) || flat(tm) || flat(t1) {
                    core::f64::consts::PI * (arc.dtheta.abs() / core::f64::consts::PI).ceil()
                } else {
                    turn_between(t0, tm) + turn_between(tm, t1)
                };
                self.sum += inner;
                self.curve += inner;
                self.curves = self.curves.saturating_add(1);
                // 弧の中の曲がりは上で足したので、2 つの接線の間は数えない
                if let Some(l) = self.last {
                    self.sum += turn_between(l, t0);
                }
                if self.first.is_none() && (t0.x != 0.0 || t0.y != 0.0) {
                    self.first = Some(t0);
                }
                if t1.x != 0.0 || t1.y != 0.0 {
                    self.last = Some(t1);
                }
            }
        }
    }
}

/// round の継ぎ目の円弧で、頂点ごとの 1 つを超えて使ってよい点の数（`turning` は [`Turning`] の和。見積もりの式と同じ）。
fn round_join_extra(pen: &Pen, turning: f64) -> u64 {
    let kk = (pen.half * pen.scale / (8.0 * pen.tol)).sqrt();
    up(kk * turning)
}

/// 滑らかな点のつなぎ目の曲がる角の上限（`same_direction` の 1 − cos θ ≤ 2⁻¹² の θ ≈ 0.0221 を丸めて上へ）。
const SMOOTH_JOIN_TURN: f64 = 0.0222;

/// 継ぎ目の円弧に使う曲がる角：round の継ぎ目はすべての頂点、ほかは滑らかな点の外側だけ（曲線の中の角と、曲線の前後の
/// つなぎ目の角の上限。ADR-101）。
fn arc_turning(pen: &Pen, t: &Turning) -> f64 {
    if pen.join == Join::Round { t.sum } else { t.curve + (t.curves as f64) * 2.0 * SMOOTH_JOIN_TURN }
}

/// 節を数える。`t` は描く座標系への変形、`stroke_space` は線を作る座標系への変形（ユーザー単位なら単位の行列）。
#[allow(clippy::too_many_arguments, reason = "数えの箱を分けて渡す")]
fn count(
    path: PathRef<'_>,
    t: &Transform,
    stroke_space: &Transform,
    q: Quality,
    reach: f64,
    view: &View,
    band: &View,
    stroke: bool,
    budget: &Budget<'_>,
) -> Result<Counts, Error> {
    let mut c = Counts::default();
    let mut sub = 0u64;
    let mut turn = Turning::default();
    // 区間ごとの手間（種類ごと。曲がる角と線を作る座標系の長さは線のときだけ求める。単位 2 の 4 回目のレビュー）
    let seg_cost = |curve: bool| {
        cost::COUNT_SEGMENT.saturating_add(if stroke { cost::COUNT_STROKE } else { 0 }).saturating_add(if curve {
            cost::COUNT_CURVE
        } else {
            0
        })
    };
    let len = |a: Point, b: Point, m: &Transform| {
        let (p, r) = (m.apply(a), m.apply(b));
        math::hypot(r.x - p.x, r.y - p.y)
    };
    let close_sub = |c: &mut Counts, sub: &mut u64| {
        c.max_sub = c.max_sub.max(*sub);
        *sub = 0;
    };
    // 塗りの暗黙の閉じ（部分パスの終わりから始めへ）の帯の中の長さ
    let mut sub_start: Option<Point> = None;
    let mut sub_end: Option<Point> = None;
    let implicit_close = |c: &mut Counts, a: Option<Point>, b: Option<Point>| {
        if let (Some(a), Some(b)) = (a, b) {
            let (p, r) = (t.apply(a), t.apply(b));
            let f = raster_fraction(view, band, p, r);
            c.len_dev_band += f * math::hypot(r.x - p.x, r.y - p.y);
        }
    };
    for seg in path.segments() {
        let curve = matches!(seg, PathSeg::Quad(..) | PathSeg::Cubic(..) | PathSeg::ArcTo(..));
        budget.spend(seg_cost(curve))?;
        c.segs = c.segs.saturating_add(1);
        if stroke {
            turn.feed(&seg, stroke_space);
        }
        sub_end = match seg {
            PathSeg::Move(_) => sub_end,
            PathSeg::Close(_, b) => Some(b),
            ref other => Some(crate::geom::flatten::end_point(other)),
        };
        let n = match seg {
            PathSeg::Move(p) => {
                implicit_close(&mut c, sub_end, sub_start);
                sub_start = Some(p);
                sub_end = Some(p);
                close_sub(&mut c, &mut sub);
                c.subpaths = c.subpaths.saturating_add(1);
                sub = 1;
                c.points = c.points.saturating_add(1);
                continue;
            }
            PathSeg::Close(a, b) => {
                c.len_dev += len(a, b, t);
                c.len_user += len(a, b, &Transform::IDENTITY);
                c.len_stroke += len(a, b, stroke_space);
                in_view(&mut c, &seg, 1, t, stroke_space, view, band, q.tol, reach, budget)?;
                close_sub(&mut c, &mut sub);
                continue;
            }
            PathSeg::Line(..) => 1,
            _ => {
                c.curves = c.curves.saturating_add(1);
                let n = pieces(&seg, t, q.tol, reach);
                c.curve_points = c.curve_points.saturating_add(n);
                n
            }
        };
        c.points = c.points.saturating_add(n);
        sub = sub.saturating_add(n);
        // 見る箱にかかる長さ（線分は箱の中の割合、曲線は厳密な外接矩形が箱にかかれば全体）
        in_view(&mut c, &seg, n, t, stroke_space, view, band, q.tol, reach, budget)?;
        // 制御多角形の長さ（要素の座標系と線を作る座標系の長さは線のときだけ：pathLength の倍と破線の数に使う）
        let s = stroke_space;
        let id = Transform::IDENTITY;
        match seg {
            PathSeg::Line(a, b) => {
                c.len_dev += len(a, b, t);
                if stroke {
                    c.len_user += len(a, b, &id);
                    c.len_stroke += len(a, b, s);
                }
            }
            PathSeg::Quad(a, b, e) => {
                c.len_dev += len(a, b, t) + len(b, e, t);
                if stroke {
                    c.len_user += len(a, b, &id) + len(b, e, &id);
                    c.len_stroke += len(a, b, s) + len(b, e, s);
                }
            }
            PathSeg::Cubic(a, b, d, e) => {
                c.len_dev += len(a, b, t) + len(b, d, t) + len(d, e, t);
                if stroke {
                    c.len_user += len(a, b, &id) + len(b, d, &id) + len(d, e, &id);
                    c.len_stroke += len(a, b, s) + len(b, d, s) + len(d, e, s);
                }
            }
            PathSeg::ArcTo(_, arc) => {
                c.len_dev += arc.dtheta.abs() * arc.transformed(t).max_radius();
                if stroke {
                    c.len_user += arc.dtheta.abs() * arc.max_radius();
                    c.len_stroke += arc.dtheta.abs() * arc.transformed(s).max_radius();
                }
            }
            _ => {}
        }
    }
    c.turning = turn.sum;
    c.curve_turning = turn.curve;
    c.max_sub = c.max_sub.max(sub);
    implicit_close(&mut c, sub_end, sub_start);
    Ok(c)
}

/// 区間が見る箱と行の帯にかかる分を数えに足す（`n` は区間の平坦化の点の数）。線分は箱の中の割合で、曲線は厳密な外接矩形が
/// 箱の中に収まれば全体、外なら 0、またがれば平坦化した弦ごとに箱の中の割合で数える（計画は同じ弦で外形を作る。巨大な
/// 曲線の、キャンバスの外の長さを数えない。単位 2 のレビュー）。
#[allow(clippy::too_many_arguments, reason = "数えの箱を分けて渡す")]
fn in_view(
    c: &mut Counts,
    seg: &PathSeg,
    n: u64,
    t: &Transform,
    s: &Transform,
    view: &View,
    band: &View,
    tol: f64,
    reach: f64,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let len_in = |a: Point, b: Point, m: &Transform| {
        let (p, r) = (m.apply(a), m.apply(b));
        math::hypot(r.x - p.x, r.y - p.y)
    };
    // 線分（か弦）1 本の分
    let chord = |c: &mut Counts, a: Point, b: Point, pts: u64| {
        let (p, r) = (t.apply(a), t.apply(b));
        let f = view.inside_fraction(p, r);
        if f > 0.0 {
            c.len_stroke_in += f * len_in(a, b, s);
            c.segs_in = c.segs_in.saturating_add(1);
        }
        // 帯の中で見る箱の横の外の部分は y の差だけ（描く段の行とセル。[`raster_fraction`]）
        let g = raster_fraction(view, band, p, r);
        if band.inside_fraction(p, r) > 0.0 {
            c.len_dev_band += g * len_in(a, b, t);
            c.len_stroke_band += g * len_in(a, b, s);
            c.pts_band = c.pts_band.saturating_add(pts);
        }
    };
    match *seg {
        PathSeg::Line(a, b) | PathSeg::Close(a, b) => chord(c, a, b, n),
        PathSeg::Move(_) => {}
        _ => {
            let mut r = crate::geom::Rect::EMPTY;
            crate::geom::add_bounds(&mut r, &crate::geom::transform_seg(seg, t));
            let lens = |m: &Transform| match *seg {
                PathSeg::Quad(a, b, e) => len_in(a, b, m) + len_in(b, e, m),
                PathSeg::Cubic(a, b, d, e) => len_in(a, b, m) + len_in(b, d, m) + len_in(d, e, m),
                PathSeg::ArcTo(_, arc) => arc.dtheta.abs() * arc.transformed(m).max_radius(),
                _ => 0.0,
            };
            let (mv, mb) = (view.meets(r.x0, r.y0, r.x1, r.y1), band.meets(r.x0, r.y0, r.x1, r.y1));
            let (cv, cb) = (view.contains(&r), band.contains(&r));
            if (mv && !cv) || (mb && !cb) {
                // またがる：平坦化した弦ごと（計画と同じ点）
                let mut prev = match *seg {
                    PathSeg::Quad(a, ..) | PathSeg::Cubic(a, ..) | PathSeg::ArcTo(a, _) => a,
                    _ => return Ok(()),
                };
                flatten_seg(seg, t, tol, reach, budget, &mut |p| {
                    chord(c, prev, p, 1);
                    prev = p;
                    Ok(())
                })?;
                return Ok(());
            }
            if mv {
                c.len_stroke_in += lens(s);
                c.segs_in = c.segs_in.saturating_add(1);
            }
            if mb {
                // 見る箱の外の曲線は、描く段の行とセルに y の変化だけかかる（2 次・3 次は制御多角形の y の差の和で抑え、
                // 線を作る座標系の長さも同じ割合にする。[`raster_fraction`]）
                let (ld, ls) = (lens(t), lens(s));
                let k = match curve_dy(seg, t) {
                    Some(dy) if !mv && ld > 0.0 && (dy / ld).is_finite() => math::min(dy / ld, 1.0),
                    _ => 1.0,
                };
                c.len_dev_band += k * ld;
                c.len_stroke_band += k * ls;
                c.pts_band = c.pts_band.saturating_add(n);
            }
        }
    }
    Ok(())
}

/// 節の見積もりの上限。
#[derive(Debug, Clone, Copy, Default)]
struct Bounds {
    /// 計画の手間・辺の数・項目の数・置き場の点の最大
    plan_work: u64,
    edges: u64,
    items: u64,
    scratch_points: u64,
    /// 描く手間（全体を 1 度）
    render_work: u64,
    /// 項目ごとの辺の数の最大
    max_item_edges: u64,
    /// 計画が要る一時の置き場（[`BUF_STROKE`] など）
    buffers: u8,
}

/// 計画の一時の置き場の印：線（まとめた点）・描く座標系で作る線（写した点）・破線（部分と最初の部分）。
const BUF_STROKE: u8 = 1;
const BUF_DEVICE: u8 = 2;
const BUF_DASH: u8 = 4;

/// 置き場の印から、点の置き場の数（部分パスの点は いつも 1 つ）。
fn buffer_count(buffers: u8) -> u64 {
    1u64.saturating_add(u64::from(buffers & BUF_STROKE != 0))
        .saturating_add(u64::from(buffers & BUF_DEVICE != 0))
        .saturating_add(2u64.saturating_mul(u64::from(buffers & BUF_DASH != 0)))
}

/// 浮動小数の上限を整数に（有限でなければ u64::MAX）。
fn up(x: f64) -> u64 {
    if x.is_nan() || x >= 1.8e19 {
        return u64::MAX;
    }
    if x <= 0.0 { 0 } else { x.ceil() as u64 }
}

/// 節の線を作る形（計画と見積もりが同じ口で決める）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct StrokeMode {
    /// 外形を描く座標系で作る（non-scaling と細い線）。偽ならユーザー単位で作って変形で写す
    pub(crate) device: bool,
    /// 外形を作る座標系の太さの半分
    pub(crate) half: f64,
    /// 線の色の α に掛ける（細い線の被覆。ほかは 1）
    pub(crate) alpha: f64,
    /// 破線の長さ 1 の、外形を作る座標系の長さ（pathLength の倍は別に掛ける）
    pub(crate) dash_unit: f64,
    /// 外形を作る座標系から描く座標系への最大の倍率
    pub(crate) sigma: f64,
}

/// 節の線の形を決める（`t` は描く座標系への変形）。描く座標系の太さが 1 画素以下の反エイリアスの線は、描く座標系の 1 画素の
/// 線を太さ（2 つの軸の太さの平均）の α で描く（Chrome・resvg の細い線と同じ。ADR-97 (2)）。細い線の破線の長さは、変形の
/// 面積の倍の平方根（相似の変形なら倍率そのもの）で描く座標系の長さにする。
pub(crate) fn stroke_mode(n: &PathNode, t: &Transform, scale: f64) -> StrokeMode {
    let w = n.stroke_width;
    let aa = n.flags & CRISP == 0;
    if n.flags & NON_SCALING != 0 {
        let wd = w * scale;
        if aa && wd <= 1.0 {
            return StrokeMode { device: true, half: 0.5, alpha: wd, dash_unit: scale, sigma: 1.0 };
        }
        return StrokeMode { device: true, half: wd / 2.0, alpha: 1.0, dash_unit: scale, sigma: 1.0 };
    }
    let len0 = math::hypot(t.a * w, t.b * w);
    let len1 = math::hypot(t.c * w, t.d * w);
    if aa && len0 <= 1.0 && len1 <= 1.0 {
        let geo = t.det().abs().sqrt();
        return StrokeMode { device: true, half: 0.5, alpha: (len0 + len1) / 2.0, dash_unit: geo, sigma: 1.0 };
    }
    StrokeMode { device: false, half: w / 2.0, alpha: 1.0, dash_unit: 1.0, sigma: t.max_scale() }
}

/// 節の線のペン（太さの半分は線を作る座標系。`mode` は [`stroke_mode`]）。
fn pen_of(n: &PathNode, t: &Transform, mode: &StrokeMode, q: Quality) -> Pen {
    let cap = match n.cap {
        CAP_ROUND => Cap::Round,
        CAP_SQUARE => Cap::Square,
        _ => Cap::Butt,
    };
    let join = match n.join {
        JOIN_ROUND => Join::Round,
        JOIN_BEVEL => Join::Bevel,
        _ => Join::Miter,
    };
    Pen {
        half: mode.half,
        cap,
        join,
        miter_limit: n.miter_limit,
        scale: if mode.device { 1.0 } else { t.max_scale() },
        tol: q.tol,
    }
}

/// 節を描くか（倍率を掛けた変形と、外接矩形 × 倍率が有限：有限でない点を含む節は途中まで描かずに丸ごと落とす。計画と
/// 見積もりの両方が使う。単位 2 のレビュー）。
fn drawable(n: &PathNode, scale: f64) -> Option<Transform> {
    let t = n.transform.scaled(scale);
    let b = n.bounds;
    let ok = t.finite() && [b.x0 * scale, b.y0 * scale, b.x1 * scale, b.y1 * scale].iter().all(|v| v.is_finite());
    ok.then_some(t)
}

/// 線の外形の点の外への広がり（線を作る座標系の太さの半分の倍）。
pub(crate) fn reach_factor(pen: &Pen) -> f64 {
    let mut f: f64 = 1.0;
    if pen.join == Join::Miter && pen.miter_limit > f {
        f = pen.miter_limit;
    }
    if pen.cap == Cap::Square && core::f64::consts::SQRT_2 > f {
        f = core::f64::consts::SQRT_2;
    }
    f
}

/// 節の、描く座標系の外接矩形の画素（線の広がりを含め、1 画素の余白）とキャンバスの交わり。
fn node_box(n: &PathNode, scale: f64, reach_dev: f64, canvas: PixelBox) -> PixelBox {
    let b = n.bounds;
    if b.is_empty() {
        return PixelBox { x0: 0, y0: 0, x1: 0, y1: 0 };
    }
    let lo = |v: f64| {
        let f = (v * scale - reach_dev).floor() - 1.0;
        if f.is_nan() { i64::MIN } else { f as i64 }
    };
    let hi = |v: f64| {
        let f = (v * scale + reach_dev).ceil() + 1.0;
        if f.is_nan() { i64::MAX } else { f as i64 }
    };
    PixelBox { x0: lo(b.x0), y0: lo(b.y0), x1: hi(b.x1), y1: hi(b.y1) }.intersect(&canvas)
}

/// 1 つの要素を入れ物に足す手間の上限（伸ばさない置き場：決まった手間と写し）。
const fn push_cost(size: u64) -> u64 {
    cost::OP_STEP.saturating_add(cost::copy(size))
}

/// 辺 1 本が回るセルの数の上限の足し分（行ごとに「列の差 + 1」を回るので、全体で |Δx| + |Δy| の画素の数 + 3）。
pub(crate) const CELLS_PER_EDGE: u64 = 4;
/// 計画の決まった手間（置き場を取る 8 回分と、そのほか）。
pub(crate) const PLAN_FRAME: u64 = 9 * cost::ALLOC_STEP + 256;
/// 描く段の決まった手間の足し分（行の置き場を作る・項目を選ぶ）。
pub(crate) const RENDER_SLACK: u64 = 1024;

/// 辺 1 本を足す手間（push_segment の中の点の確かめと固定小数への変換と、置き場への写しと、項目の矩形を辺の端から求め、
/// 行とセルの上限を数える分）。
const EDGE_WORK: u64 =
    push_cost(size_of::<Edge>() as u64).saturating_add(cost::OP_STEP.saturating_mul(2)).saturating_add(cost::PLAN_EDGE);
/// 点 1 つを置き場に足す手間（折れ線の置き場は点と滑らかな点の印を並べて足す）。
const POINT_WORK: u64 = push_cost(size_of::<Point>() as u64).saturating_add(push_cost(1));

/// 描く段の手間の式（`cost` に置いたもの。範囲の見積もりと同じ口）。
pub(crate) use cost::{
    RASTER_EDGE_ROW_WORK as EDGE_ROW_WORK, RASTER_EDGE_START as EDGE_START_WORK, raster_row as row_work,
};

/// 描く手間の上限（項目 1 つ。行・辺の行・セル・掃く幅と塗る画素）。
fn render_work(rows: u64, width: u64, edges: u64, cells: u64, edge_rows: u64) -> u64 {
    rows.saturating_mul(row_work(width))
        .saturating_add(edges.saturating_mul(EDGE_START_WORK))
        .saturating_add(edge_rows.saturating_mul(EDGE_ROW_WORK))
        .saturating_add(cells.saturating_mul(cost::RASTER_CELL))
}

/// 辺の行とセルの数の上限：辺の長さの和 `l`（描く座標系）からの式と、節の画素の箱（`rows` 行・`width` 列）で抑えた式の
/// 小さい方（描く段は箱の行の中だけを回り、1 本の辺の 1 行のセルは幅 ＋ 3 以下。巨大な図形でキャンバスの外の長さに
/// 比例させない。単位 1・2 のレビュー）。計画が項目ごとに数える上限（[`item_bounds`]）は、辺ごとにこの両方より小さい。
fn raster_bounds(l: f64, e: u64, rows: u64, width: u64) -> (u64, u64) {
    let by_len_rows = up(l).saturating_add(e.saturating_mul(2));
    let by_box_rows = e.saturating_mul(rows.saturating_add(2));
    let edge_rows = by_len_rows.min(by_box_rows);
    let by_len_cells = up(1.5 * l).saturating_add(e.saturating_mul(CELLS_PER_EDGE));
    let by_box_cells = by_box_rows.saturating_mul(width.saturating_add(3));
    (edge_rows, by_len_cells.min(by_box_cells))
}

/// 項目の辺 1 本の、行とセルの数の上限（辺を項目の画素の矩形 `bbox` の行で切った部分だけ：その部分の行の数と、x（矩形の
/// 列の左右 1 列の外で切る）と y の差の画素の数 ＋ 4 か、行の数 × (幅 ＋ 3) の小さい方。矩形の行にかからない辺は 0。描く段は
/// 矩形の行の中だけを回る）。
pub(crate) fn item_bounds(e: &Edge, bbox: &PixelBox) -> (u64, u64) {
    let ya = i64::from(e.y0).max(bbox.y0.saturating_mul(ONE));
    let yb = i64::from(e.y1).min(bbox.y1.saturating_mul(ONE));
    if ya >= yb {
        return (0, 0);
    }
    // x は矩形の列の左に 1・右に 1 を足した所で切る（描く段は範囲の左の外を 1 列にまとめ、右の外を落とす。
    // `raster::RowCells::cells_bound`。巨大な辺の、矩形の横の外の長さを数えない）
    let (lo, hi) = (bbox.x0.saturating_sub(1).saturating_mul(ONE), bbox.x1.saturating_add(1).saturating_mul(ONE));
    let clamp_x = |x: i64| x.max(lo).min(hi);
    let dx = clamp_x(crate::raster::x_at(e, yb)).saturating_sub(clamp_x(crate::raster::x_at(e, ya))).unsigned_abs();
    let dy = yb.saturating_sub(ya).unsigned_abs();
    let px = |v: u64| v.checked_div(ONE as u64).unwrap_or(0);
    let r0 = crate::raster::floor_div(ya, ONE);
    let r1 = crate::raster::floor_div(yb.saturating_sub(1), ONE).saturating_add(1);
    let rows_in = u64::try_from(r1.saturating_sub(r0)).unwrap_or(0);
    let rows = px(dy).saturating_add(2).min(rows_in);
    let cells = px(dx.saturating_add(dy))
        .saturating_add(CELLS_PER_EDGE)
        .min(rows_in.saturating_mul(bbox.width().saturating_add(3)));
    (rows, cells)
}

/// 節の見積もり（閉じた式）。
/// 節の線の外形が届く幅（描く座標系。線がなければ 0）。見積もりと計画の両方が使う。
fn reach_of(n: &PathNode, t: &Transform, mode: &StrokeMode, q: Quality) -> f64 {
    if n.flags & HAS_STROKE != 0 && n.stroke_width > 0.0 {
        let pen = pen_of(n, t, mode, q);
        pen.half * mode.sigma * reach_factor(&pen)
    } else {
        0.0
    }
}

/// 節の画素の箱（外接矩形 × 倍率 ± 届く幅）がキャンバスと交わらないか（見積もりと計画の両方で丸ごと飛ばす。キャンバスで
/// 決めるので範囲に依らず、画素は変わらない。描ける文書を、キャンバスの外の節の平坦化の手間で断らない。単位 2 の 4 回目の
/// レビュー）。
fn off_canvas(n: &PathNode, t: &Transform, mode: &StrokeMode, q: Quality, scale: f64, canvas: PixelBox) -> bool {
    node_box(n, scale, reach_of(n, t, mode, q), canvas).is_empty()
}

fn bound_node(
    tree: &Tree,
    n: &PathNode,
    scale: f64,
    canvas: PixelBox,
    q: Quality,
    budget: &Budget<'_>,
) -> Result<Bounds, Error> {
    let mut out = Bounds::default();
    let Some(t) = drawable(n, scale) else { return Ok(out) };
    if off_canvas(n, &t, &stroke_mode(n, &t, scale), q, scale, canvas) {
        return Ok(out);
    }
    let path = tree.path(n);
    let has_fill = n.flags & HAS_FILL != 0;
    let has_stroke = n.flags & HAS_STROKE != 0 && n.stroke_width > 0.0;
    let mode = stroke_mode(n, &t, scale);
    let stroke_space = if mode.device { t } else { Transform::IDENTITY };
    let sigma = mode.sigma;
    let pen = pen_of(n, &t, &mode, q);
    let reach_dev = if has_stroke { pen.half * sigma * reach_factor(&pen) } else { 0.0 };
    // 平坦化の点は外形の届く距離で数える（塗りの点の数以上なので、塗りの上限にも使える）
    let view = View::new(canvas, reach_dev);
    let band = View::band(canvas, reach_dev);
    let c = count(path, &t, &stroke_space, q, reach_dev, &view, &band, has_stroke, budget)?;
    let bx = node_box(n, scale, reach_dev, canvas);
    let (rows, width) = (bx.height(), bx.width());
    // 箱の境で辺が分かれうるか（x の箱の外に出る節だけ。y は切っても分かれない）。点は節の変形で写すので、外接矩形 ×
    // 倍率との丸めの差を 1 画素の余白で吸う（単位 2 のレビュー）
    let b = n.bounds;
    let edge = BOX - 1.0;
    let crosses = b.x0 * scale - reach_dev < -edge || b.x1 * scale + reach_dev > edge;
    let split: u64 = if crosses { 2 } else { 1 };
    out.scratch_points = c.max_sub.saturating_add(4);
    if has_stroke {
        out.buffers |= BUF_STROKE;
        if mode.device {
            out.buffers |= BUF_DEVICE;
        }
        if n.dashes[1] > 0 {
            out.buffers |= BUF_DASH;
        }
    }
    if has_fill {
        let e = c.points.saturating_add(c.subpaths).saturating_mul(split);
        // 描く段の行とセル：行の帯にかかる辺だけ（区間と暗黙の閉じの帯の中の長さ）
        // 続く閉じは帯の点を数えても辺を作らないので、辺の数を超えない
        let e_band = c.pts_band.saturating_add(c.subpaths).saturating_mul(split).min(e);
        let (edge_rows, cells) = raster_bounds(c.len_dev_band, e_band, rows, width);
        out.plan_work = out
            .plan_work
            .saturating_add(c.points.saturating_mul(cost::FLATTEN_POINT.saturating_add(POINT_WORK)))
            .saturating_add(c.curves.saturating_mul(cost::FLATTEN_CURVE))
            .saturating_add(c.segs.saturating_mul(cost::OP_STEP))
            .saturating_add(e.saturating_mul(EDGE_WORK))
            .saturating_add(cost::stable_sort(e, size_of::<Edge>() as u64));
        out.edges = out.edges.saturating_add(e);
        out.items = out.items.saturating_add(1);
        out.max_item_edges = out.max_item_edges.max(e);
        out.render_work = out
            .render_work
            .saturating_add(render_work(rows, width, e, cells, edge_rows))
            .saturating_add(region_search(e));
    }
    if has_stroke {
        let v = c.points;
        let s = c.subpaths;
        let r = pen.round_pieces(core::f64::consts::PI);
        let r2 = pen.round_pieces(2.0 * core::f64::consts::PI);
        let cap_pts = match pen.cap {
            Cap::Butt => 2,
            Cap::Square => 4,
            Cap::Round => r.saturating_add(1),
        };
        // round の継ぎ目と、滑らかな点の外側の円弧（ほかの継ぎ目は曲線の中と曲線の前後のつなぎ目だけ。ADR-101）
        let arc_turn = arc_turning(
            &pen,
            &Turning { sum: c.turning, curve: c.curve_turning, curves: c.curves, ..Turning::default() },
        );
        let arc_vertices =
            if pen.join == Join::Round { v } else { c.curve_points.saturating_add(c.curves.saturating_mul(2)) };
        let k_join = round_join_extra(&pen, arc_turn).saturating_add(arc_vertices);
        // 破線の数（pathLength="0" は実線）
        let dashes = if n.flags & PATH_LENGTH_ZERO != 0 { &[][..] } else { tree.dash_list(n) };
        let mut total = 0.0;
        for &d in dashes {
            budget.spend(cost::OP_STEP)?;
            match tree.dash_value(n, d) {
                Some(v) => total += v,
                None => {
                    total = 0.0;
                    break;
                }
            }
        }
        // 破線の部分の数：作る（なめる）分は道の全体の長さで、外形を作る分は見る箱にかかる長さとかかる区間の数で（箱に
        // かからない部分は作らない。ADR-30 (4)）。pathLength があれば、倍は計画が平坦化した長さで決めるので閉じた式の上限に
        // する（ユーザー単位で作る線は ⌈pathLength / 合計⌉、描く座標系で作る線は変形の最大の倍 ÷ 破線の長さ 1 の倍を掛ける。
        // 箱の効き目は数えない。単位 1・2 のレビュー）
        let half_k = (dashes.len() as u64).saturating_add(1).max(2) / 2;
        let (dash_pieces, dash_vis) = if dashes.is_empty() || total.is_nan() || total <= 0.0 {
            (0, 0)
        } else if n.path_length > 0.0 {
            let sig = if mode.device { t.max_scale() / mode.dash_unit } else { 1.0 };
            let all = up(n.path_length * sig / total).saturating_add(s.saturating_mul(2)).saturating_add(1);
            let all = all.saturating_mul(half_k);
            (all, all)
        } else {
            let period = total * mode.dash_unit;
            if period.is_nan() || period <= 0.0 {
                (0, 0)
            } else {
                let all = up(c.len_stroke / period).saturating_add(s.saturating_mul(2)).saturating_add(1);
                let vis = up(c.len_stroke_in / period).saturating_add(c.segs_in.saturating_mul(2)).saturating_add(1);
                // 計画は見えない所を周期ごとに飛ばすので、歩くのは見える分と、区間ごとに飛ばす前後の余白（2 周期ずつと
                // 端数。奇数個を 2 度並べた周期は合計の 2 倍なので 16 周期で覆う）だけ（ADR-30 (4)）
                let walked = vis.saturating_add(v.saturating_mul(16));
                (all.min(walked).saturating_mul(half_k), vis.min(all).saturating_mul(half_k))
            }
        };
        // 計画が破線の並びを断って実線にしても足りるように、部分パスの数を下限にする
        let pieces_n = if dash_pieces > 0 { dash_vis.max(s) } else { s };
        // 外形の点：部分パス（か破線）ごとに輪 2 つと端 2 つと長さ 0 の円、頂点ごとに 6、round の継ぎ目の円弧
        let per_piece = 2u64.saturating_add(cap_pts.saturating_mul(2)).saturating_add(r2).saturating_add(16);
        let out_pts = pieces_n.saturating_mul(per_piece).saturating_add(v.saturating_mul(6)).saturating_add(k_join);
        let e = out_pts.saturating_add(pieces_n.saturating_mul(2)).saturating_mul(split);
        // 外形の辺の長さの和（線を作る座標系。描く座標系へは σ を掛ける）：縁の 2 本と、頂点ごとの継ぎ目（外側は miter なら
        // 2r × miterlimit、ほかは 2r。内側で頂点を通す分 2r）、部分（か破線）ごとの端・長さ 0 の円・輪の継ぎ（2r × 12 で
        // 2 つの丸い端と円と四角い端の延びを覆う）、round の継ぎ目の円弧（r × 曲がる角の和）。外形の点ごとに数えると、頂点の
        // 多い太い線を 50 倍に見積もった（単位 2 の 3 回目のレビュー）
        let outer = if pen.join == Join::Miter && pen.miter_limit > 1.0 { pen.miter_limit } else { 1.0 };
        let per_vertex = 2.0 * pen.half * (outer + 1.0);
        let piece_len = 2.0 * pen.half * 12.0;
        // round の継ぎ目と滑らかな点の外側の円弧（ADR-101）
        let round_len = pen.half * arc_turn;
        // 描く段の行とセル：行の帯にかかる外形の辺だけ（帯の外の頂点の外形は線の届く幅の内なので帯の外）
        let out_band = pieces_n
            .saturating_mul(per_piece)
            .saturating_add(c.pts_band.saturating_mul(6))
            .saturating_add(k_join)
            .min(out_pts);
        let e_band = out_band.saturating_add(pieces_n.saturating_mul(2)).saturating_mul(split);
        let l_dev = sigma
            * (2.0 * c.len_stroke_band + (c.pts_band as f64) * per_vertex + (pieces_n as f64) * piece_len + round_len);
        let (edge_rows, cells) = raster_bounds(l_dev, e_band, rows, width);
        // pathLength の長さを測る平坦化（計画と同じ条件：破線の並びがあり pathLength がある）
        let passes: u64 = if !dashes.is_empty() && n.path_length > 0.0 { 2 } else { 1 };
        // 破線の部分の点（区間の境の 2 つと、部分の中の頂点）と、まとめる写し（部分ごとに端の 2 つ）
        let dash_pts = if dash_pieces > 0 { v.saturating_add(dash_pieces.saturating_mul(4)) } else { 0 };
        let dedup = v.saturating_add(dash_vis.saturating_mul(2)).saturating_add(s.saturating_mul(2));
        // 破線の部分が見る箱にかかるかを、部分の点ごとに見る
        let cull = if dash_pieces > 0 { v.saturating_add(dash_pieces.saturating_mul(4)) } else { 0 };
        // 頂点の継ぎ目を見る回（両側）と、破線の区間ごとの払い・閉じた部分パスの最初の部分をつなぐ写し
        let vertex_calls = dedup.saturating_mul(2);
        let dash_segs = if dash_pieces > 0 { v.saturating_add(s).saturating_mul(passes) } else { 0 };
        let joins = if dash_pieces > 0 { v.saturating_add(dash_pieces.saturating_mul(2)) } else { 0 };
        // 位相を進める手間と、区間ごとに見えない所を飛ばす手間（前と後ろで 2 度まで）
        let phase_steps =
            s.saturating_add(dash_pieces).saturating_add(if dash_pieces > 0 { v.saturating_mul(2) } else { 0 });
        // 破線の部分の端の向き（境ごとに 1 つと、部分パスごとに始めと終わり。ADR-101）
        let dash_tangents =
            if dash_pieces > 0 { dash_pieces.saturating_mul(2).saturating_add(s.saturating_mul(2)) } else { 0 };
        out.plan_work = out
            .plan_work
            .saturating_add(v.saturating_mul(passes).saturating_mul(cost::FLATTEN_POINT.saturating_add(POINT_WORK)))
            .saturating_add(c.curves.saturating_mul(passes).saturating_mul(cost::FLATTEN_CURVE))
            .saturating_add(c.segs.saturating_mul(passes).saturating_mul(cost::OP_STEP))
            .saturating_add(c.segs.saturating_mul(cost::TURN_SEGMENT))
            .saturating_add(c.segs.saturating_mul(cost::JOIN_TANGENT))
            .saturating_add(if mode.device { v.saturating_mul(POINT_WORK) } else { 0 })
            .saturating_add(dash_pts.saturating_mul(cost::OUTLINE_POINT.saturating_add(POINT_WORK)))
            .saturating_add(dedup.saturating_mul(POINT_WORK.saturating_add(cost::OP_STEP)))
            .saturating_add(vertex_calls.saturating_mul(cost::OP_STEP))
            .saturating_add(dash_segs.saturating_mul(cost::OP_STEP))
            .saturating_add(joins.saturating_mul(POINT_WORK.saturating_add(cost::OP_STEP)))
            .saturating_add(phase_steps.saturating_mul(cost::OP_STEP))
            .saturating_add(dash_tangents.saturating_mul(cost::DASH_TANGENT))
            .saturating_add(cull.saturating_mul(cost::OP_STEP.saturating_mul(2)))
            .saturating_add(out_pts.saturating_mul(cost::OUTLINE_POINT.saturating_add(cost::OP_STEP)))
            .saturating_add(pieces_n.saturating_mul(2).saturating_mul(cost::OP_STEP))
            .saturating_add(e.saturating_mul(EDGE_WORK))
            .saturating_add(cost::stable_sort(e, size_of::<Edge>() as u64));
        out.edges = out.edges.saturating_add(e);
        out.items = out.items.saturating_add(1);
        out.max_item_edges = out.max_item_edges.max(e);
        out.render_work = out
            .render_work
            .saturating_add(render_work(rows, width, e, cells, edge_rows))
            .saturating_add(region_search(e));
    }
    out.plan_work = out.plan_work.saturating_add(c.subpaths.saturating_mul(cost::OP_STEP)).saturating_add(64);
    Ok(out)
}

/// 全体の見積もりの和と、置き場の大きさ。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Totals {
    plan_work: u64,
    edges: u64,
    items: u64,
    scratch_points: u64,
    render_work: u64,
    max_item_edges: u64,
    /// 節の破線の並びの数の最大
    max_dashes: u64,
    /// 見積もり自身が払った手間（計測と試験のため）
    #[allow(dead_code, reason = "試験と計測が読む")]
    pub(crate) self_work: u64,
    /// 計画が要る一時の置き場の印
    buffers: u8,
}

impl Tree {
    /// 計画と全体を 1 度描く分の見積もり（ADR-37。決定的な関数）。キャンバスの大きさ・描く節の数のほか、計画の手間と
    /// 持ち続ける量・描く手間と一時の量を `RenderLimits` の `plan_work`・`plan_bytes`・`render_work`・`temp_bytes` と比べ、
    /// 超えれば `Limit` を返す（architecture.md §3.1。これが通れば、同じ設定の `plan`・`render` は `Limit` にならない）。
    pub fn estimate(&self, opts: &RenderOptions, cancel: &dyn Cancel) -> Result<RenderEstimate, Error> {
        let (est, _, _) = self.estimate_with(opts, Quality::DEFAULT, cancel)?;
        Ok(est)
    }

    /// 見積もりと、キャンバスの大きさと、置き場の大きさ。
    pub(crate) fn estimate_with(
        &self,
        opts: &RenderOptions,
        q: Quality,
        cancel: &dyn Cancel,
    ) -> Result<(RenderEstimate, PixelSize, Totals), Error> {
        let scale = opts.scale;
        if !(scale.is_finite() && scale > 0.0) {
            return Err(argument());
        }
        let px = self.pixel_size(scale)?;
        let lim = &opts.limits;
        let side = px.width.max(px.height);
        // 1 辺は決まった箱（±2²² 画素）の内（箱の外の辺は落とすので、それより大きなキャンバスは描けない。単位 2 のレビュー）
        let max_side = lim.canvas_side.min(BOX as u32);
        if side > max_side {
            return Err(Error::limit(Resource::CanvasSide, u64::from(side), u64::from(max_side), Stage::Estimate));
        }
        let pixels = u64::from(px.width).saturating_mul(u64::from(px.height));
        if pixels > lim.canvas_pixels {
            return Err(Error::limit(Resource::CanvasPixels, pixels, lim.canvas_pixels, Stage::Estimate));
        }
        let nodes = self.nodes.len() as u64;
        if nodes > lim.render_nodes {
            return Err(Error::limit(Resource::RenderNodes, nodes, lim.render_nodes, Stage::Estimate));
        }
        let budget = Budget::new(Stage::Estimate, lim.estimate_work, lim.estimate_bytes, cancel);
        let canvas = PixelBox { x0: 0, y0: 0, x1: i64::from(px.width), y1: i64::from(px.height) };
        let mut tot = Totals::default();
        for n in self.nodes.as_slice() {
            budget.spend(cost::OP_STEP)?;
            let b = bound_node(self, n, scale, canvas, q, &budget)?;
            // 計画が節ごとに払う分（描かない節も）
            tot.plan_work = tot.plan_work.saturating_add(b.plan_work).saturating_add(cost::OP_STEP);
            tot.edges = tot.edges.saturating_add(b.edges);
            tot.items = tot.items.saturating_add(b.items);
            tot.scratch_points = tot.scratch_points.max(b.scratch_points);
            tot.render_work = tot.render_work.saturating_add(b.render_work);
            tot.max_item_edges = tot.max_item_edges.max(b.max_item_edges);
            tot.buffers |= b.buffers;
            // 破線の並び：解いて写す・奇数個なら 2 度並べる・型を作るときになめる（2 倍の長さまで）
            let nd = u64::from(n.dashes[1]).saturating_mul(2);
            tot.max_dashes = tot.max_dashes.max(nd);
            tot.plan_work = tot
                .plan_work
                .saturating_add(nd.saturating_mul(cost::OP_STEP.saturating_mul(3).saturating_add(POINT_WORK)));
        }
        let edge_bytes = tot.edges.saturating_mul(size_of::<Edge>() as u64);
        let item_bytes = tot.items.saturating_mul(size_of::<DrawItem>() as u64);
        // 計画の一時の置き場：部分パスの点・まとめた点・破線の部分の点（どれも点の最大の分。点と滑らかな点の印 1 バイト）と、
        // 辺の並べ替えの作業の領域
        // 置き場は要るものだけ取る（塗りだけなら部分パスの点だけ。単位 2 の 4 回目のレビュー）
        let scratch = tot
            .scratch_points
            .saturating_mul((size_of::<Point>() as u64).saturating_add(1))
            .saturating_mul(buffer_count(tot.buffers))
            .saturating_add(tot.max_dashes.saturating_mul(8));
        let sort = cost::stable_sort_bytes(tot.max_item_edges, size_of::<Edge>() as u64);
        let held = edge_bytes.saturating_add(item_bytes);
        let plan = Estimate {
            work: tot
                .plan_work
                .saturating_add(tot.items.saturating_mul(push_cost(size_of::<DrawItem>() as u64)))
                // 辺を詰めて写す
                .saturating_add(cost::ALLOC_STEP.saturating_add(cost::OP_STEP).saturating_add(cost::copy(edge_bytes)))
                .saturating_add(PLAN_FRAME),
            // 詰めて写す間は上限の容量と写しの両方が生きる
            peak_bytes: held.saturating_add(scratch).saturating_add(sort).saturating_add(edge_bytes),
            result_bytes: held,
        };
        // 描く段：出力を透明にする手間と、行の置き場（セルの cover と area）と行にかかる辺の番号
        let (work, peak_bytes) = render_frame(u64::from(px.width), u64::from(px.height), tot.max_item_edges, tot.items);
        let bg = background_work(crate::paint::premultiply(self.background), pixels);
        let render =
            Estimate { work: tot.render_work.saturating_add(work).saturating_add(bg), peak_bytes, result_bytes: 0 };
        // 上限と比べる（architecture.md §3.1 の「計画と描く手間」。これが通れば、同じ設定の plan・render は Limit に
        // ならない。plan_bytes は Plan が持ち続ける量）
        check(plan.work, lim.plan_work, Resource::Work, Stage::Plan)?;
        check(plan.result_bytes, lim.plan_bytes, Resource::Memory, Stage::Plan)?;
        check(render.work, lim.render_work, Resource::Work, Stage::Render)?;
        check(render.peak_bytes, lim.temp_bytes, Resource::Memory, Stage::Render)?;
        // 計画の一時の山（持ち続ける量を除く）は temp_bytes と比べる（ADR-101。ユーザーの判断）
        check(plan.peak_bytes.saturating_sub(plan.result_bytes), lim.temp_bytes, Resource::Memory, Stage::Plan)?;
        // 範囲の見積もり自身の手間の上限（どの範囲でも：項目ごとの決まった分と 2 分の探索、辺ごとの分）。これが通れば、
        // タイルの範囲の見積もりも見積もりの段の上限で断らない（単位 2 の 4 回目のレビュー）
        let region_self = tot
            .items
            .saturating_mul(cost::OP_STEP.saturating_add(region_search(tot.max_item_edges)))
            .saturating_add(tot.edges.saturating_mul(cost::OP_STEP))
            .saturating_add(cost::OP_STEP.saturating_mul(4));
        check(region_self, lim.estimate_work, Resource::Work, Stage::Estimate)?;
        let tot = Totals { self_work: budget.work(), ..tot };
        Ok((RenderEstimate { plan, render }, px, tot))
    }

    /// 計画を作る（見積もりが上限の内なら、見積もりを上限にして作る）。
    pub fn plan(&self, opts: &RenderOptions, cancel: &dyn Cancel) -> Result<Plan, Error> {
        self.plan_with(opts, Quality::DEFAULT, cancel)
    }

    pub(crate) fn plan_with(&self, opts: &RenderOptions, q: Quality, cancel: &dyn Cancel) -> Result<Plan, Error> {
        self.plan_watch(opts, q, cancel, &mut crate::doc::Watch::default())
    }

    /// [`Tree::plan`] と同じに作り、計画の段の勘定の見張りの値（手間・確保の山・1 回の払い・返しすぎ）を `watch` に書く
    /// （試験とファズが実際の山で照らす）。
    pub(crate) fn plan_watch(
        &self,
        opts: &RenderOptions,
        q: Quality,
        cancel: &dyn Cancel,
        watch: &mut crate::doc::Watch,
    ) -> Result<Plan, Error> {
        // 見積もりが上限と比べる（超えれば Limit）
        let (est, px, tot) = self.estimate_with(opts, q, cancel)?;
        let budget = Budget::new(Stage::Plan, est.plan.work, est.plan.peak_bytes, cancel);
        let r = self.plan_in(&budget, px, tot, opts, q);
        watch.work = budget.work();
        watch.peak = budget.peak();
        watch.max_spend = budget.max_spend();
        watch.over_released = budget.over_released();
        r
    }

    /// 計画の本体（`budget` は見積もりを上限にしたもの）。
    fn plan_in(
        &self,
        budget: &Budget<'_>,
        px: PixelSize,
        tot: Totals,
        opts: &RenderOptions,
        q: Quality,
    ) -> Result<Plan, Error> {
        let b = budget_ref(budget);
        let canvas = PixelBox { x0: 0, y0: 0, x1: i64::from(px.width), y1: i64::from(px.height) };
        let cap = |n: u64| usize::try_from(n).map_err(|_| b.too_large());
        let mut edges: MVec<'_, Edge> = MVec::with_capacity(b, cap(tot.edges)?)?;
        let mut items: MVec<'_, DrawItem> = MVec::with_capacity(b, cap(tot.items)?)?;
        let sp = cap(tot.scratch_points)?;
        // 置き場は見積もりと同じ印で、要るものだけ取る
        let need = |bit: u8| tot.buffers & bit != 0;
        let buf = |on: bool| if on { PolyBuf::with_capacity(b, sp) } else { Ok(PolyBuf::new(b)) };
        let mut poly = PolyBuf::with_capacity(b, sp)?;
        let mut scratch = buf(need(BUF_STROKE))?;
        let mut dashbuf = buf(need(BUF_DASH))?;
        let mut devbuf = buf(need(BUF_DEVICE))?;
        let mut head = buf(need(BUF_DASH))?;
        let mut dash_scaled: MVec<'_, f64> = MVec::with_capacity(b, cap(tot.max_dashes)?)?;
        let mut max_item = 0u64;
        for n in self.nodes.as_slice() {
            b.spend(cost::OP_STEP)?;
            let Some(t) = drawable(n, opts.scale) else { continue };
            let mode = stroke_mode(n, &t, opts.scale);
            if off_canvas(n, &t, &mode, q, opts.scale, canvas) {
                continue;
            }
            let order: [u8; 2] =
                if n.flags & STROKE_FIRST != 0 { [HAS_STROKE, HAS_FILL] } else { [HAS_FILL, HAS_STROKE] };
            for what in order {
                let start = edges.len();
                let color = if what == HAS_FILL {
                    n.fill
                } else {
                    let c = n.stroke;
                    [c[0], c[1], c[2], c[3] * mode.alpha]
                };
                if n.flags & what == 0 || color[3].is_nan() || color[3] <= 0.0 {
                    continue;
                }
                if what == HAS_FILL {
                    fill_edges(self.path(n), &t, q, &mut poly, &mut edges, b)?;
                } else {
                    if n.stroke_width.is_nan() || n.stroke_width <= 0.0 {
                        continue;
                    }
                    let parts = StrokeParts {
                        poly: &mut poly,
                        scratch: &mut scratch,
                        dashbuf: &mut dashbuf,
                        devbuf: &mut devbuf,
                        head: &mut head,
                        dash_scaled: &mut dash_scaled,
                        canvas,
                    };
                    stroke_edges(self, n, &t, &mode, q, parts, &mut edges, b)?;
                }
                let end = edges.len();
                if end == start {
                    continue;
                }
                // 項目の画素の矩形は辺の固定小数の端から（点の浮動小数から求めると、見積もりの節の箱と丸めが 1 ulp ずれて
                // 1 画素はみ出しうる。単位 2 の 3 回目のレビュー）
                let (bbox, max_dy) = edges_box(edges.as_slice().get(start..end).unwrap_or(&[]), b)?;
                let bbox = bbox.intersect(&canvas);
                let slice = edges.as_mut_slice().get_mut(start..end).unwrap_or(&mut []);
                stable_sort(b, slice, |x, y| x.y0 < y.y0)?;
                // 範囲の見積もりの行とセルの上限（辺ごと。項目の矩形の中の行だけ。払う）
                let (mut cells, mut rows) = (0u64, 0u64);
                for e in edges.as_slice().get(start..end).unwrap_or(&[]) {
                    b.spend(cost::OP_STEP)?;
                    let (r, c) = item_bounds(e, &bbox);
                    cells = cells.saturating_add(c);
                    rows = rows.saturating_add(r);
                }
                let count = end.saturating_sub(start) as u64;
                max_item = max_item.max(count);
                let aa = u8::from(n.flags & CRISP == 0);
                let rule = u8::from(what == HAS_FILL && n.flags & EVEN_ODD != 0);
                // 辺の番号は u32 に入る（入らなければ上限を上げすぎた：黙って違う絵にしない。単位 2 のレビュー）
                let too_many = || Error::limit(Resource::Memory, u64::MAX, u64::from(u32::MAX), Stage::Plan);
                items.push(DrawItem {
                    edges: [
                        u32::try_from(start).map_err(|_| too_many())?,
                        u32::try_from(count).map_err(|_| too_many())?,
                    ],
                    bbox: [bbox.x0, bbox.y0, bbox.x1, bbox.y1],
                    cells,
                    edge_rows: rows,
                    color: crate::paint::premultiply(color),
                    rule,
                    aa,
                    pad: [0; 2],
                    max_dy,
                    pad2: [0; 4],
                })?;
            }
        }
        drop((poly, scratch, dashbuf, devbuf, head, dash_scaled));
        // 上限の容量で取った置き場を、使った長さに詰めて写す（計画が持ち続ける量を実際の量にする。単位 2 のレビュー）
        let exact = |v: MVec<'_, Edge>| -> Result<Frozen<Edge>, Error> {
            let mut e: MVec<'_, Edge> = MVec::with_capacity(b, v.len())?;
            e.append_slice(v.as_slice())?;
            Ok(e.freeze())
        };
        let edges = exact(edges)?;
        Ok(Plan {
            width: px.width,
            height: px.height,
            items: items.freeze(),
            edges,
            max_item_edges: max_item,
            limits: opts.limits,
            background: crate::paint::premultiply(self.background),
        })
    }
}

/// 描く段の、項目に依らない手間と確保（出力を行ごとに透明にする・行の置き場（セルの cover と area）・行にかかる辺の番号・
/// 項目を選ぶ）。範囲の見積もりも同じ式。
pub(crate) fn render_frame(width: u64, height: u64, max_item_edges: u64, items: u64) -> (u64, u64) {
    let clear = height.saturating_mul(cost::fill(width.saturating_mul(4)).saturating_add(cost::OP_STEP));
    let row_bytes = width.saturating_add(1).saturating_mul(16);
    let work = clear
        .saturating_add(cost::fill(row_bytes))
        .saturating_add(items.saturating_mul(cost::OP_STEP))
        .saturating_add(RENDER_SLACK);
    (work, row_bytes.saturating_add(max_item_edges.saturating_mul(4)))
}

/// 見積もりが上限の内か。
fn check(needed: u64, allowed: u64, resource: Resource, stage: Stage) -> Result<(), Error> {
    if needed > allowed { Err(Error::limit(resource, needed, allowed, stage)) } else { Ok(()) }
}

/// 辺の固定小数の端から求める画素の矩形（セルを積む列は x の端の床、行は y の端の床から天井まで。辺 1 本ごとに払う）。
/// 点は節の外接矩形 × 倍率 ± 届く幅から 1/256 画素と丸めの差しかずれないので、見積もりの節の箱（1 画素の余白）の内。
/// あわせて辺の高さの最大（固定小数）も返す。
fn edges_box(edges: &[Edge], budget: &Budget<'_>) -> Result<(PixelBox, u32), Error> {
    let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    let mut dy = 0i64;
    for e in edges {
        budget.spend(cost::OP_STEP)?;
        x0 = x0.min(i64::from(e.x0.min(e.x1)));
        x1 = x1.max(i64::from(e.x0.max(e.x1)));
        y0 = y0.min(i64::from(e.y0));
        y1 = y1.max(i64::from(e.y1));
        dy = dy.max(i64::from(e.y1).saturating_sub(i64::from(e.y0)));
    }
    if x0 > x1 {
        return Ok((PixelBox { x0: 0, y0: 0, x1: 0, y1: 0 }, 0));
    }
    let floor = |v: i64| crate::raster::floor_div(v, ONE);
    let ceil = |v: i64| floor(v.saturating_add(ONE - 1));
    // 辺は ±2³⁰ の固定小数の内なので、高さは u32 に入る
    let max_dy = u32::try_from(dy).unwrap_or(u32::MAX);
    Ok((PixelBox { x0: floor(x0), y0: floor(y0), x1: floor(x1).saturating_add(1), y1: ceil(y1) }, max_dy))
}

/// 部分パスの点と閉じたかを受け取る口。
pub(crate) type SubpathFn<'a> = dyn FnMut(Poly<'_>, bool) -> Result<(), Error> + 'a;

/// 部分パスごとに平坦化した点（ユーザー単位）を `poly` に集め、`f(点, 閉じたか)` を呼ぶ。
#[allow(clippy::too_many_arguments, reason = "平坦化の条件と置き場を分けて渡す")]
pub(crate) fn subpaths(
    path: PathRef<'_>,
    t: &Transform,
    q: Quality,
    reach: f64,
    poly: &mut PolyBuf<'_>,
    smooth: bool,
    budget: &Budget<'_>,
    f: &mut SubpathFn<'_>,
) -> Result<(), Error> {
    poly.clear();
    let mut open = false;
    // 曲線の内側の点は滑らかな点、区間のつなぎ目は角。ただし曲線の前か後ろのつなぎ目で接線が続くものは滑らかな点
    // （ADR-99 (2)・ADR-101）。前の区間の終わりの接線と曲線か、部分パスの最初の区間の始めの接線と曲線か
    let mut last: Option<(Point, bool)> = None;
    let mut first: Option<(Point, bool)> = None;
    // 描く区間があったか（長さ 0 の区間だけの部分パスは、点を重ねて長さ 0 の線として渡す）
    let mut drew = false;
    // 部分パスを渡す（線のときは、長さ 0 の区間だけの部分パスを長さ 0 の線にする）
    let emit = |poly: &mut PolyBuf<'_>, closed: bool, drew: bool, f: &mut SubpathFn<'_>| -> Result<(), Error> {
        if smooth
            && drew
            && poly.len() == 1
            && let Some(&p) = poly.pts.last()
        {
            poly.push(p, false)?;
        }
        f(poly.view(), closed)
    };
    for seg in path.segments() {
        // 区間ごとに払う（続く閉じ（`ZZZ…`）も。単位 2 の 3 回目のレビュー）
        budget.spend(cost::OP_STEP)?;
        match seg {
            PathSeg::Move(p) => {
                if open {
                    emit(poly, false, drew, f)?;
                }
                poly.clear();
                poly.push(p, false)?;
                open = true;
                last = None;
                first = None;
                drew = false;
            }
            PathSeg::Close(a, b) => {
                if open && smooth {
                    // 閉じの線分（長さがあれば）と、始めの点のつなぎ目（区間ごとにつなぎ目の向きを求める手間を払う）
                    budget.spend(cost::JOIN_TANGENT)?;
                    let closing = crate::geom::seg_tangents(&seg);
                    let start_from = match closing {
                        Some((v, _)) => {
                            if let Some((lt, lc)) = last
                                && lc
                                && crate::geom::same_direction(lt, v)
                            {
                                poly.set_last_smooth(true);
                            }
                            Some((v, false))
                        }
                        None if a == b => last,
                        None => None,
                    };
                    if let (Some((lt, lc)), Some((ft, fc))) = (start_from, first)
                        && (lc || fc)
                        && crate::geom::same_direction(lt, ft)
                    {
                        poly.set_first_smooth(true);
                    }
                    // 終わりの点が始めの点と同じなら除く（輪の始めの点の印を残す。破線の閉じのつなぎで角に戻らない）
                    if poly.len() > 1 && poly.pts.last() == poly.pts.as_slice().first() {
                        poly.pop();
                    }
                }
                if open {
                    emit(poly, true, drew, f)?;
                }
                poly.clear();
                open = false;
            }
            _ if !smooth => {
                flatten_seg(&seg, t, q.tol, reach, budget, &mut |p| poly.push(p, false))?;
            }
            _ => {
                budget.spend(cost::JOIN_TANGENT)?;
                drew = true;
                let curve = !matches!(seg, PathSeg::Line(..));
                let Some((st, et)) = crate::geom::seg_tangents(&seg) else {
                    // 長さ 0 の区間（接線がない）は点を足さない（前後のつなぎ目の滑らかさを角に戻さない。ADR-101）
                    continue;
                };
                if let Some((lt, lc)) = last
                    && (lc || curve)
                    && crate::geom::same_direction(lt, st)
                {
                    poly.set_last_smooth(true);
                }
                flatten_seg(&seg, t, q.tol, reach, budget, &mut |p| poly.push(p, curve))?;
                poly.set_last_smooth(false);
                if first.is_none() {
                    first = Some((st, curve));
                }
                last = Some((et, curve));
            }
        }
    }
    if open {
        emit(poly, false, drew, f)?;
    }
    Ok(())
}

/// 塗りの辺（部分パスはどれも閉じて塗る）。
fn fill_edges(
    path: PathRef<'_>,
    t: &Transform,
    q: Quality,
    poly: &mut PolyBuf<'_>,
    edges: &mut MVec<'_, Edge>,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    subpaths(path, t, q, 0.0, poly, false, budget, &mut |line, _| {
        let pts = line.pts;
        let Some(&first) = pts.first() else { return Ok(()) };
        let mut prev = t.apply(first);
        for &p in pts.iter().skip(1) {
            let d = t.apply(p);
            budget.spend(cost::PLAN_EDGE)?;
            push_segment(prev, d, edges)?;
            prev = d;
        }
        budget.spend(cost::PLAN_EDGE)?;
        push_segment(prev, t.apply(first), edges)
    })
}

/// 線の一時の置き場（部分パスの点・まとめた点・破線の部分の点・non-scaling の描く座標系の点・掛けた破線の並び）。
struct StrokeParts<'a, 'b> {
    poly: &'a mut PolyBuf<'b>,
    scratch: &'a mut PolyBuf<'b>,
    dashbuf: &'a mut PolyBuf<'b>,
    devbuf: &'a mut PolyBuf<'b>,
    head: &'a mut PolyBuf<'b>,
    dash_scaled: &'a mut MVec<'b, f64>,
    /// キャンバスの画素の矩形（破線の部分を落とす見る箱の元）
    canvas: PixelBox,
}

/// 外形の輪を描く座標系の辺にする口。
struct EdgeSink<'e, 'b> {
    edges: &'e mut MVec<'b, Edge>,
    /// 外形の座標から描く座標系へ
    t: Transform,
    first: Option<Point>,
    prev: Option<Point>,
    budget: &'e Budget<'e>,
}

impl EdgeSink<'_, '_> {
    fn close_loop(&mut self) -> Result<(), Error> {
        if let (Some(a), Some(f)) = (self.prev, self.first) {
            self.budget.spend(cost::OP_STEP)?;
            push_segment(a, f, self.edges)?;
        }
        self.first = None;
        self.prev = None;
        Ok(())
    }
}

impl LoopSink for EdgeSink<'_, '_> {
    fn begin(&mut self) -> Result<(), Error> {
        self.close_loop()
    }

    fn point(&mut self, p: Point) -> Result<(), Error> {
        let d = self.t.apply(p);
        match self.prev {
            Some(a) => {
                self.budget.spend(cost::PLAN_EDGE)?;
                push_segment(a, d, self.edges)?;
            }
            None => self.first = Some(d),
        }
        self.prev = Some(d);
        Ok(())
    }
}

/// 線の辺（破線・外形）。
#[allow(clippy::too_many_arguments, reason = "計画の置き場を分けて渡す")]
fn stroke_edges(
    tree: &Tree,
    n: &PathNode,
    t: &Transform,
    mode: &StrokeMode,
    q: Quality,
    parts: StrokeParts<'_, '_>,
    edges: &mut MVec<'_, Edge>,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let device = mode.device;
    let pen = pen_of(n, t, mode, q);
    // 外形を作る座標系：ふつうはユーザー単位（描く座標系へは t）、non-scaling と細い線は描く座標系（そのまま）
    let to_dev = if device { Transform::IDENTITY } else { *t };
    let reach = pen.half * mode.sigma * reach_factor(&pen);
    let path = tree.path(n);
    let dashes_src = tree.dash_list(n);
    // pathLength の倍（要素の座標系（ユーザー単位）の道の長さの和 / pathLength。non-scaling はその長さを画面の座標系の長さと
    // して描くので倍率を掛ける。Chrome と同じ：`tests/svg/m2-probe/` の nonscaling-pathlength。ADR-95 (1)）
    let mut factor = 1.0;
    if n.path_length > 0.0 && !dashes_src.is_empty() {
        let mut total = 0.0;
        subpaths(path, t, q, reach, parts.poly, false, budget, &mut |line, closed| {
            let pts = line.pts;
            let m = |p: Point| p;
            let mut a = match pts.first() {
                Some(&p) => m(p),
                None => return Ok(()),
            };
            for &p in pts.iter().skip(1) {
                budget.spend(cost::OP_STEP)?;
                let b = m(p);
                total += math::hypot(b.x - a.x, b.y - a.y);
                a = b;
            }
            if closed && let Some(&p) = pts.first() {
                let b = m(p);
                total += math::hypot(b.x - a.x, b.y - a.y);
            }
            Ok(())
        })?;
        factor = total / n.path_length;
    }
    let dash_scale = factor * mode.dash_unit;
    let StrokeParts { poly, scratch, dashbuf, devbuf, head, dash_scaled, canvas } = parts;
    dash_scaled.clear();
    // pathLength="0" は倍が無限：正の長さはみな無限になり、実線（SVG 2 §10.2・Chrome と同じ。ADR-95 (2)）
    let dashes_src = if n.flags & PATH_LENGTH_ZERO != 0 { &[][..] } else { dashes_src };
    for &d in dashes_src {
        budget.spend(cost::OP_STEP)?;
        match tree.dash_value(n, d) {
            Some(v) => dash_scaled.push(v * dash_scale)?,
            // 解けない長さがあれば実線（破線の宣言が効かない）
            None => {
                dash_scaled.clear();
                break;
            }
        }
    }
    // 奇数個は 2 度並べる（SVG 2 §13.5.6・ADR-91 (4)）
    let k = dash_scaled.len();
    if k % 2 == 1 {
        for i in 0..k {
            budget.spend(cost::OP_STEP)?;
            let v = dash_scaled.get(i).copied().unwrap_or(0.0);
            dash_scaled.push(v)?;
        }
    }
    let dashes = Dashes::new(dash_scaled.as_slice(), n.dash_offset * dash_scale, budget)?;
    let mut sink = EdgeSink { edges, t: to_dev, first: None, prev: None, budget: budget_ref(budget) };
    // 破線の部分は見る箱にかかるものだけを作る（ADR-30 (4)。見積もりと同じ箱）
    let cull = Cull { to_dev, view: View::new(canvas, reach) };
    // round の継ぎ目の点の残り：制御多角形の曲がる角の和から、見積もりと同じ式で（区間ごとに払う）
    // （滑らかな点の外側は継ぎ目の形に依らず円弧なので、どの継ぎ目でも求める。ADR-101）
    let stroke_space = if device { *t } else { Transform::IDENTITY };
    let mut turn = Turning::default();
    for seg in path.segments() {
        budget.spend(cost::TURN_SEGMENT)?;
        turn.feed(&seg, &stroke_space);
    }
    let mut round_left = round_join_extra(&pen, arc_turning(&pen, &turn));
    let round_left = &mut round_left;
    subpaths(path, t, q, reach, poly, true, budget, &mut |line, closed| {
        if device {
            // non-scaling と細い線は描く座標系の点にしてから線を作る（印はそのまま）
            devbuf.clear();
            for (i, &p) in line.pts.iter().enumerate() {
                devbuf.push(t.apply(p), line.smooth.get(i).is_some_and(|&s| s != 0))?;
            }
            stroke_one(
                devbuf.view(),
                closed,
                &pen,
                dashes.as_ref(),
                &cull,
                [&mut *dashbuf, &mut *head, &mut *scratch],
                &mut *round_left,
                &mut sink,
                budget,
            )
        } else {
            stroke_one(
                line,
                closed,
                &pen,
                dashes.as_ref(),
                &cull,
                [&mut *dashbuf, &mut *head, &mut *scratch],
                &mut *round_left,
                &mut sink,
                budget,
            )
        }
    })?;
    sink.close_loop()
}

/// 破線の部分を落とすかを決める箱（線を作る座標系から描く座標系への変形と、見る箱）。
struct Cull {
    to_dev: Transform,
    view: View,
}

impl Cull {
    /// 部分の折れ線のどれかの区間が見る箱にかかるか（点ごとに払う）。見積もりの数え（[`in_view`]）と同じく、区間そのものが
    /// 箱と交わるかで見る（外接矩形で見ると、箱の角の外を斜めに通る区間を作ってしまい、見積もりを超える。単位 1・2 の
    /// レビュー）。箱にかからない区間の外形は、キャンバスから 2 画素より外にある。
    fn visible(&self, piece: &[Point], closed: bool, budget: &Budget<'_>) -> Result<bool, Error> {
        let n = piece.len();
        let count = if closed { n } else { n.saturating_sub(1) };
        if n == 1 {
            budget.spend(cost::OP_STEP)?;
            let p = self.to_dev.apply(piece.first().copied().unwrap_or(Point::new(0.0, 0.0)));
            return Ok(self.view.meets(p.x, p.y, p.x, p.y));
        }
        for k in 0..count {
            budget.spend(cost::OP_STEP.saturating_mul(2))?;
            let next = k.saturating_add(1);
            let (Some(&a), Some(&b)) = (piece.get(k), piece.get(if next >= n { 0 } else { next })) else { continue };
            let (a, b) = (self.to_dev.apply(a), self.to_dev.apply(b));
            if self.view.inside_fraction(a, b) > 0.0 || (a == b && self.view.meets(a.x, a.y, a.x, a.y)) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Cull {
    /// 線を作る座標系の区間 a → b のうち見る箱にかかる所（区間の割合。かからなければ None）。見積もりと同じ切り方
    /// （[`View::clip`]）。有限でなければ全体。
    fn range(&self, a: Point, b: Point) -> Option<(f64, f64)> {
        let (p, q) = (self.to_dev.apply(a), self.to_dev.apply(b));
        if !p.finite() || !q.finite() {
            return Some((0.0, 1.0));
        }
        if p == q {
            return if self.view.meets(p.x, p.y, p.x, p.y) { Some((0.0, 1.0)) } else { None };
        }
        let (c0, c1) = self.view.clip(p, q)?;
        let (dx, dy) = (q.x - p.x, q.y - p.y);
        let frac = |c: Point| if dx.abs() >= dy.abs() { (c.x - p.x) / dx } else { (c.y - p.y) / dy };
        let (u0, u1) = (frac(c0), frac(c1));
        if !(u0.is_finite() && u1.is_finite()) {
            return Some((0.0, 1.0));
        }
        let (lo, hi) = if u0 <= u1 { (u0, u1) } else { (u1, u0) };
        Some((math::max(lo, 0.0), math::min(hi, 1.0)))
    }
}

/// 1 本の部分パスの線（破線があれば部分ごと。見る箱にかからない部分は作らない）。`bufs` は破線の部分・最初の部分・まとめた
/// 点の置き場。
#[allow(clippy::too_many_arguments, reason = "置き場を分けて渡す")]
fn stroke_one(
    line: Poly<'_>,
    closed: bool,
    pen: &Pen,
    dashes: Option<&Dashes<'_>>,
    cull: &Cull,
    bufs: [&mut PolyBuf<'_>; 3],
    round_left: &mut u64,
    sink: &mut EdgeSink<'_, '_>,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let [dashbuf, head, scratch] = bufs;
    match dashes {
        None => stroke_polyline(line, closed, pen, scratch, budget, round_left, sink),
        Some(d) => dash_polyline(
            line,
            closed,
            d,
            dashbuf,
            head,
            budget,
            Some(&|a, b| cull.range(a, b)),
            &mut |piece, piece_closed| {
                if !cull.visible(piece.pts, piece_closed, budget)? {
                    return Ok(());
                }
                stroke_polyline(piece, piece_closed, pen, scratch, budget, &mut *round_left, sink)
            },
        ),
    }
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
