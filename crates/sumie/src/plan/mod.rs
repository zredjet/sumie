//! 段 5：計画と見積もり（architecture.md §3・§3.1・§4.5・§6.1・§8・ADR-37・ADR-38）。
//!
//! - 計画（`Tree::plan`）：倍率から、節ごとに平坦化した点・破線・線の外形を作り、固定小数の辺（y の順）と、画素の外接矩形・
//!   色・塗りの規則を持つ「描く項目」にする（塗りと線で 1 つずつ。`paint-order` の順）。`Plan` は範囲に依らない
//! - 見積もり（`Tree::estimate`）：点を作らずに、節ごとの数え（分ける数の和・部分パスの数・描く座標系の制御多角形の長さ・
//!   線を作る座標系の制御多角形の長さと曲がる角の和）から閉じた式で上限を求める。曲がる角の和は、平坦化した折れ線の
//!   曲がる角の和以上（内接する折れ線の曲がる角は曲線の全曲率以下、Bézier の全曲率は制御多角形の曲がる角以下）なので、
//!   round の継ぎ目の円弧の点の和の上限になる
//! - 辺の置き場は見積もりの上限の数で最初に取る（伸ばさない。持ち続ける量と山は見積もりと同じ）
//! - 範囲の見積もり（`Plan::estimate_region`）は、計画で持った辺ごとの上限（セル ≤ (|Δx| + |Δy|) / 1 画素 + 2）から求める。
//!   同じ形の式なので、全体に含まれる範囲の見積もりは `RenderEstimate.render` を超えない（FR-OUT-06）

#![allow(dead_code, reason = "公開の口（lib）と描く段から使う")]

use crate::budget::{Budget, Estimate};
use crate::cancel::Cancel;
use crate::cost;
use crate::doc::PixelSize;
use crate::error::{Error, Invalid, InvalidReason, Resource, Stage};
use crate::geom::flatten::{BOX, flatten_seg, pieces};
use crate::geom::stroke::{Cap, Dashes, Join, LoopSink, Pen, dash_polyline, stroke_polyline};
use crate::geom::{PathRef, PathSeg, Point, Transform};
use crate::limits::RenderLimits;
use crate::math;
use crate::mem::{Frozen, MVec, Plain, stable_sort};
use crate::raster::{Edge, FillRule, ONE, PixelBox, push_segment};
use crate::tree::{
    CAP_ROUND, CAP_SQUARE, CRISP, EVEN_ODD, HAS_FILL, HAS_STROKE, JOIN_BEVEL, JOIN_ROUND, NON_SCALING, PathNode,
    STROKE_FIRST, Tree,
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
}

/// 呼び手の誤り。
fn argument() -> Error {
    Error::Invalid(Invalid { reason: InvalidReason::Argument, position: None })
}

/// 節の数え（閉じた式の見積もりと、計画の置き場の大きさに使う）。
#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    /// 平坦化の点（部分パスの始めの点を含む）・部分パスの数・1 つの部分パスの点の最大
    points: u64,
    subpaths: u64,
    max_sub: u64,
    /// 描く座標系の制御多角形の長さ（線分・閉じ・曲線の制御多角形・弧は |Δθ| × 最大の半径）
    len_dev: f64,
    /// 線を作る座標系の制御多角形の長さと、曲がる角の和（部分パスごとに π を足す）
    len_stroke: f64,
    turning: f64,
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

/// 節を数える。`t` は描く座標系への変形、`stroke_space` は線を作る座標系への変形（ユーザー単位なら単位の行列）。
fn count(
    path: PathRef<'_>,
    t: &Transform,
    stroke_space: &Transform,
    q: Quality,
    reach: f64,
    budget: &Budget<'_>,
) -> Result<Counts, Error> {
    let mut c = Counts::default();
    let mut sub = 0u64;
    let mut last_dir: Option<Point> = None;
    let mut first_dir: Option<Point> = None;
    let mut dirs: [Point; 4] = [Point::new(0.0, 0.0); 4];
    let len = |a: Point, b: Point, m: &Transform| {
        let (p, r) = (m.apply(a), m.apply(b));
        math::hypot(r.x - p.x, r.y - p.y)
    };
    let close_sub = |c: &mut Counts, sub: &mut u64, first: Option<Point>, last: Option<Point>| {
        c.max_sub = c.max_sub.max(*sub);
        if let (Some(f), Some(l)) = (first, last) {
            c.turning += turn_between(l, f);
        }
        *sub = 0;
    };
    for seg in path.segments() {
        budget.spend(cost::OP_STEP)?;
        let n = match seg {
            PathSeg::Move(_) => {
                close_sub(&mut c, &mut sub, first_dir, last_dir);
                c.subpaths = c.subpaths.saturating_add(1);
                c.turning += core::f64::consts::PI;
                first_dir = None;
                last_dir = None;
                sub = 1;
                c.points = c.points.saturating_add(1);
                continue;
            }
            PathSeg::Close(a, b) => {
                c.len_dev += len(a, b, t);
                c.len_stroke += len(a, b, stroke_space);
                let d = stroke_space.apply_vec(Point::new(b.x - a.x, b.y - a.y));
                if let Some(l) = last_dir {
                    c.turning += turn_between(l, d);
                }
                if d.x != 0.0 || d.y != 0.0 {
                    last_dir = Some(d);
                }
                close_sub(&mut c, &mut sub, first_dir, last_dir);
                first_dir = None;
                last_dir = None;
                continue;
            }
            _ => pieces(&seg, t, q.tol, reach),
        };
        c.points = c.points.saturating_add(n);
        sub = sub.saturating_add(n);
        // 制御多角形の向きの並びと長さ
        let mut k = 0usize;
        let push_dir = |d: Point, dirs: &mut [Point; 4], k: &mut usize| {
            if let Some(slot) = dirs.get_mut(*k) {
                *slot = d;
                *k = k.saturating_add(1);
            }
        };
        let s = stroke_space;
        match seg {
            PathSeg::Line(a, b) => {
                c.len_dev += len(a, b, t);
                c.len_stroke += len(a, b, s);
                push_dir(s.apply_vec(Point::new(b.x - a.x, b.y - a.y)), &mut dirs, &mut k);
            }
            PathSeg::Quad(a, b, e) => {
                c.len_dev += len(a, b, t) + len(b, e, t);
                c.len_stroke += len(a, b, s) + len(b, e, s);
                for (p, r) in [(a, b), (b, e)] {
                    push_dir(s.apply_vec(Point::new(r.x - p.x, r.y - p.y)), &mut dirs, &mut k);
                }
            }
            PathSeg::Cubic(a, b, d, e) => {
                c.len_dev += len(a, b, t) + len(b, d, t) + len(d, e, t);
                c.len_stroke += len(a, b, s) + len(b, d, s) + len(d, e, s);
                for (p, r) in [(a, b), (b, d), (d, e)] {
                    push_dir(s.apply_vec(Point::new(r.x - p.x, r.y - p.y)), &mut dirs, &mut k);
                }
            }
            PathSeg::ArcTo(_, arc) => {
                let r_dev = arc.transformed(t).max_radius();
                let r_s = arc.transformed(s).max_radius();
                c.len_dev += arc.dtheta.abs() * r_dev;
                c.len_stroke += arc.dtheta.abs() * r_s;
                c.turning += arc.dtheta.abs();
                // 始めと終わりの接線
                let tangent = |th: f64| {
                    let [m0, m1, m2, m3] = arc.m;
                    let (sn, cs) = (math::sin(th), math::cos(th));
                    let sign = if arc.dtheta < 0.0 { -1.0 } else { 1.0 };
                    s.apply_vec(Point::new(sign * (-m0 * sn + m1 * cs), sign * (-m2 * sn + m3 * cs)))
                };
                push_dir(tangent(arc.theta1), &mut dirs, &mut k);
                push_dir(tangent(arc.theta1 + arc.dtheta), &mut dirs, &mut k);
                // 弧の中の曲がりは上で足したので、2 つの接線の間は数えない
                if let (Some(&d0), Some(&d1)) = (dirs.first(), dirs.get(1)) {
                    if let Some(l) = last_dir {
                        c.turning += turn_between(l, d0);
                    }
                    if first_dir.is_none() && (d0.x != 0.0 || d0.y != 0.0) {
                        first_dir = Some(d0);
                    }
                    if d1.x != 0.0 || d1.y != 0.0 {
                        last_dir = Some(d1);
                    }
                }
                continue;
            }
            _ => {}
        }
        for d in dirs.iter().take(k).copied() {
            if d.x == 0.0 && d.y == 0.0 {
                continue;
            }
            if let Some(l) = last_dir {
                c.turning += turn_between(l, d);
            }
            if first_dir.is_none() {
                first_dir = Some(d);
            }
            last_dir = Some(d);
        }
    }
    c.max_sub = c.max_sub.max(sub);
    Ok(c)
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
}

/// 浮動小数の上限を整数に（有限でなければ u64::MAX）。
fn up(x: f64) -> u64 {
    if x.is_nan() || x >= 1.8e19 {
        return u64::MAX;
    }
    if x <= 0.0 { 0 } else { x.ceil() as u64 }
}

/// 節の線のペン（太さの半分は線を作る座標系。`scale` は描く座標系への最大の倍率）。
fn pen_of(n: &PathNode, t: &Transform, scale: f64, q: Quality) -> Pen {
    let non_scaling = n.flags & NON_SCALING != 0;
    let half = if non_scaling { n.stroke_width / 2.0 * scale } else { n.stroke_width / 2.0 };
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
        half,
        cap,
        join,
        miter_limit: n.miter_limit,
        scale: if non_scaling { 1.0 } else { t.max_scale() },
        tol: q.tol,
    }
}

/// 線の外形の点の外への広がり（線を作る座標系の太さの半分の倍）。
fn reach_factor(pen: &Pen) -> f64 {
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

/// 辺 1 本を足す手間（push_segment の中の点の確かめと固定小数への変換と、置き場への写し）。
const EDGE_WORK: u64 = push_cost(size_of::<Edge>() as u64).saturating_add(cost::OP_STEP);
/// 点 1 つを置き場に足す手間。
const POINT_WORK: u64 = push_cost(size_of::<Point>() as u64);

/// 描く手間の上限（項目 1 つ。行・辺の行・セル・掃く幅と塗る画素）。
fn render_work(rows: u64, width: u64, edges: u64, cells: u64, edge_rows: u64) -> u64 {
    let per_row = cost::RASTER_ROW
        .saturating_add(cost::RASTER_SWEEP)
        .saturating_add(width.saturating_mul(cost::RASTER_SWEEP.saturating_add(cost::PAINT_PIXEL)));
    rows.saturating_mul(per_row)
        .saturating_add(edges.saturating_mul(cost::RASTER_EDGE_ROW))
        .saturating_add(edge_rows.saturating_mul(cost::RASTER_EDGE_ROW))
        .saturating_add(cells.saturating_mul(cost::RASTER_CELL))
}

/// 節の見積もり（閉じた式）。
fn bound_node(
    tree: &Tree,
    n: &PathNode,
    scale: f64,
    canvas: PixelBox,
    q: Quality,
    budget: &Budget<'_>,
) -> Result<Bounds, Error> {
    let mut out = Bounds::default();
    let t = n.transform.scaled(scale);
    if !t.finite() {
        return Ok(out);
    }
    let path = tree.path(n);
    let has_fill = n.flags & HAS_FILL != 0;
    let has_stroke = n.flags & HAS_STROKE != 0 && n.stroke_width > 0.0;
    let non_scaling = n.flags & NON_SCALING != 0;
    let stroke_space = if non_scaling { t } else { Transform::IDENTITY };
    let sigma = if non_scaling { 1.0 } else { t.max_scale() };
    let pen = pen_of(n, &t, scale, q);
    let reach_dev = if has_stroke { pen.half * sigma * reach_factor(&pen) } else { 0.0 };
    // 平坦化の点は外形の届く距離で数える（塗りの点の数以上なので、塗りの上限にも使える）
    let c = count(path, &t, &stroke_space, q, reach_dev, budget)?;
    let bx = node_box(n, scale, reach_dev, canvas);
    let (rows, width) = (bx.height(), bx.width());
    // 箱の境で辺が分かれうるか（x の箱の外に出る節だけ。y は切っても分かれない）
    let b = n.bounds;
    let crosses = b.x0 * scale - reach_dev < -BOX || b.x1 * scale + reach_dev > BOX;
    let split: u64 = if crosses { 2 } else { 1 };
    out.scratch_points = c.max_sub.saturating_add(4);
    if has_fill {
        let e = c.points.saturating_add(c.subpaths).saturating_mul(split);
        let l = 2.0 * c.len_dev;
        let cells = up(1.5 * l).saturating_add(e.saturating_mul(CELLS_PER_EDGE));
        let edge_rows = up(l).saturating_add(e.saturating_mul(2));
        out.plan_work = out
            .plan_work
            .saturating_add(c.points.saturating_mul(cost::FLATTEN_POINT.saturating_add(POINT_WORK)))
            .saturating_add(e.saturating_mul(EDGE_WORK))
            .saturating_add(cost::stable_sort(e, size_of::<Edge>() as u64));
        out.edges = out.edges.saturating_add(e);
        out.items = out.items.saturating_add(1);
        out.max_item_edges = out.max_item_edges.max(e);
        out.render_work = out.render_work.saturating_add(render_work(rows, width, e, cells, edge_rows));
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
        let k_join = if pen.join == Join::Round {
            let kk = (pen.half * pen.scale / (8.0 * pen.tol)).sqrt();
            up(kk * c.turning).saturating_add(v)
        } else {
            0
        };
        // 破線の数
        let dashes = tree.dash_list(n);
        let mut total = 0.0;
        for &d in dashes {
            total += d;
        }
        let factor = if n.path_length > 0.0 { c.len_stroke / n.path_length } else { 1.0 };
        let dash_scale = if non_scaling { factor * scale } else { factor };
        let dash_pieces = if dashes.is_empty() || (total * dash_scale).is_nan() || total * dash_scale <= 0.0 {
            0
        } else {
            let half_k = (dashes.len() as u64).saturating_add(1) / 2;
            up(c.len_stroke / (total * dash_scale))
                .saturating_add(s.saturating_mul(2))
                .saturating_add(1)
                .saturating_mul(half_k.max(1))
        };
        let pieces_n = if dash_pieces > 0 { dash_pieces } else { s };
        // 外形の点：部分パス（か破線）ごとに輪 2 つと端 2 つと長さ 0 の円、頂点ごとに 6、round の継ぎ目の円弧
        let per_piece = 2u64.saturating_add(cap_pts.saturating_mul(2)).saturating_add(r2).saturating_add(16);
        let out_pts = pieces_n.saturating_mul(per_piece).saturating_add(v.saturating_mul(6)).saturating_add(k_join);
        let e = out_pts.saturating_add(pieces_n.saturating_mul(2)).saturating_mul(split);
        // 外形の辺の長さの和（描く座標系）：縁の 2 倍と、点ごとに 2r × max(miterlimit, 2)
        let spread = 2.0 * pen.half * if pen.miter_limit > 2.0 { pen.miter_limit } else { 2.0 };
        let l_dev = sigma * (2.0 * c.len_stroke + (out_pts as f64) * spread);
        let cells = up(1.5 * l_dev).saturating_add(e.saturating_mul(CELLS_PER_EDGE));
        let edge_rows = up(l_dev).saturating_add(e.saturating_mul(2));
        let passes: u64 = if dash_pieces > 0 && n.path_length > 0.0 { 2 } else { 1 };
        // 破線の部分の点（区間の境の 2 つと、部分の中の頂点）と、まとめる写し（部分ごとに端の 2 つ）
        let dash_pts = if dash_pieces > 0 { v.saturating_add(dash_pieces.saturating_mul(4)) } else { 0 };
        let dedup = v.saturating_add(dash_pieces.saturating_mul(2)).saturating_add(s.saturating_mul(2));
        // 頂点の継ぎ目を見る回（両側）と、破線の区間ごとの払い・閉じた部分パスの最初の部分をつなぐ写し
        let vertex_calls = dedup.saturating_mul(2);
        let dash_segs = if dash_pieces > 0 { v.saturating_add(s).saturating_mul(passes) } else { 0 };
        let joins = if dash_pieces > 0 { v.saturating_add(dash_pieces.saturating_mul(2)) } else { 0 };
        let phase_steps = s.saturating_add(dash_pieces).saturating_mul(dashes.len() as u64);
        out.plan_work = out
            .plan_work
            .saturating_add(v.saturating_mul(passes).saturating_mul(cost::FLATTEN_POINT.saturating_add(POINT_WORK)))
            .saturating_add(if non_scaling { v.saturating_mul(POINT_WORK) } else { 0 })
            .saturating_add(dash_pts.saturating_mul(cost::OUTLINE_POINT.saturating_add(POINT_WORK)))
            .saturating_add(dedup.saturating_mul(POINT_WORK.saturating_add(cost::OP_STEP)))
            .saturating_add(vertex_calls.saturating_mul(cost::OP_STEP))
            .saturating_add(dash_segs.saturating_mul(cost::OP_STEP))
            .saturating_add(joins.saturating_mul(POINT_WORK.saturating_add(cost::OP_STEP)))
            .saturating_add(phase_steps.saturating_mul(cost::OP_STEP))
            .saturating_add(out_pts.saturating_mul(cost::OUTLINE_POINT.saturating_add(cost::OP_STEP)))
            .saturating_add(pieces_n.saturating_mul(2).saturating_mul(cost::OP_STEP))
            .saturating_add(e.saturating_mul(EDGE_WORK))
            .saturating_add(cost::stable_sort(e, size_of::<Edge>() as u64));
        out.edges = out.edges.saturating_add(e);
        out.items = out.items.saturating_add(1);
        out.max_item_edges = out.max_item_edges.max(e);
        out.render_work = out.render_work.saturating_add(render_work(rows, width, e, cells, edge_rows));
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
}

impl Tree {
    /// 計画と全体を 1 度描く分の見積もり（ADR-37。決定的な関数）。
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
        if side > lim.canvas_side {
            return Err(Error::limit(
                Resource::CanvasSide,
                u64::from(side),
                u64::from(lim.canvas_side),
                Stage::Estimate,
            ));
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
            tot.plan_work = tot.plan_work.saturating_add(b.plan_work);
            tot.edges = tot.edges.saturating_add(b.edges);
            tot.items = tot.items.saturating_add(b.items);
            tot.scratch_points = tot.scratch_points.max(b.scratch_points);
            tot.render_work = tot.render_work.saturating_add(b.render_work);
            tot.max_item_edges = tot.max_item_edges.max(b.max_item_edges);
            tot.max_dashes = tot.max_dashes.max(u64::from(n.dashes[1]));
            tot.plan_work = tot
                .plan_work
                .saturating_add(u64::from(n.dashes[1]).saturating_mul(cost::OP_STEP.saturating_add(POINT_WORK)));
        }
        let edge_bytes = tot.edges.saturating_mul(size_of::<Edge>() as u64);
        let item_bytes = tot.items.saturating_mul(size_of::<DrawItem>() as u64);
        // 計画の一時の置き場：部分パスの点・まとめた点・破線の部分の点（どれも点の最大の分）と、辺の並べ替えの作業の領域
        let scratch = tot
            .scratch_points
            .saturating_mul(size_of::<Point>() as u64)
            .saturating_mul(5)
            .saturating_add(tot.max_dashes.saturating_mul(8));
        let sort = cost::stable_sort_bytes(tot.max_item_edges, size_of::<Edge>() as u64);
        let held = edge_bytes.saturating_add(item_bytes);
        let plan = Estimate {
            work: tot
                .plan_work
                .saturating_add(tot.items.saturating_mul(push_cost(size_of::<DrawItem>() as u64)))
                .saturating_add(PLAN_FRAME),
            peak_bytes: held.saturating_add(scratch).saturating_add(sort),
            result_bytes: held,
        };
        // 描く段：出力を透明にする手間と、行の置き場（セルの cover と area）と行にかかる辺の番号
        let (work, peak_bytes) = render_frame(u64::from(px.width), u64::from(px.height), tot.max_item_edges, tot.items);
        let render = Estimate { work: tot.render_work.saturating_add(work), peak_bytes, result_bytes: 0 };
        Ok((RenderEstimate { plan, render }, px, tot))
    }

    /// 計画を作る（見積もりが上限の内なら、見積もりを上限にして作る）。
    pub fn plan(&self, opts: &RenderOptions, cancel: &dyn Cancel) -> Result<Plan, Error> {
        self.plan_with(opts, Quality::DEFAULT, cancel)
    }

    pub(crate) fn plan_with(&self, opts: &RenderOptions, q: Quality, cancel: &dyn Cancel) -> Result<Plan, Error> {
        let (est, px, tot) = self.estimate_with(opts, q, cancel)?;
        let lim = &opts.limits;
        check(est.plan.work, lim.plan_work, Resource::Work, Stage::Plan)?;
        check(est.plan.peak_bytes, lim.plan_bytes, Resource::Memory, Stage::Plan)?;
        check(est.render.work, lim.render_work, Resource::Work, Stage::Render)?;
        check(est.render.peak_bytes, lim.temp_bytes, Resource::Memory, Stage::Render)?;
        let budget = Budget::new(Stage::Plan, est.plan.work, est.plan.peak_bytes, cancel);
        let b = &budget;
        let canvas = PixelBox { x0: 0, y0: 0, x1: i64::from(px.width), y1: i64::from(px.height) };
        let cap = |n: u64| usize::try_from(n).map_err(|_| b.too_large());
        let mut edges: MVec<'_, Edge> = MVec::with_capacity(b, cap(tot.edges)?)?;
        let mut items: MVec<'_, DrawItem> = MVec::with_capacity(b, cap(tot.items)?)?;
        let sp = cap(tot.scratch_points)?;
        let mut poly: MVec<'_, Point> = MVec::with_capacity(b, sp)?;
        let mut scratch: MVec<'_, Point> = MVec::with_capacity(b, sp)?;
        let mut dashbuf: MVec<'_, Point> = MVec::with_capacity(b, sp)?;
        let mut devbuf: MVec<'_, Point> = MVec::with_capacity(b, sp)?;
        let mut head: MVec<'_, Point> = MVec::with_capacity(b, sp)?;
        let mut dash_scaled: MVec<'_, f64> = MVec::with_capacity(b, cap(tot.max_dashes)?)?;
        let mut max_item = 0u64;
        for n in self.nodes.as_slice() {
            b.spend(cost::OP_STEP)?;
            let t = n.transform.scaled(opts.scale);
            if !t.finite() {
                continue;
            }
            let order: [u8; 2] =
                if n.flags & STROKE_FIRST != 0 { [HAS_STROKE, HAS_FILL] } else { [HAS_FILL, HAS_STROKE] };
            for what in order {
                let start = edges.len();
                let mut dev = DevBox::new();
                let color = if what == HAS_FILL { n.fill } else { n.stroke };
                if n.flags & what == 0 || color[3].is_nan() || color[3] <= 0.0 {
                    continue;
                }
                if what == HAS_FILL {
                    fill_edges(self.path(n), &t, q, &mut poly, &mut edges, &mut dev, b)?;
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
                    };
                    stroke_edges(self, n, &t, opts.scale, q, parts, &mut edges, &mut dev, b)?;
                }
                let end = edges.len();
                if end == start {
                    continue;
                }
                let bbox = dev.pixels().intersect(&canvas);
                let slice = edges.as_mut_slice().get_mut(start..end).unwrap_or(&mut []);
                stable_sort(b, slice, |x, y| x.y0 < y.y0)?;
                let (mut cells, mut rows) = (0u64, 0u64);
                for e in edges.as_slice().get(start..end).unwrap_or(&[]) {
                    let dx = i64::from(e.x1).saturating_sub(i64::from(e.x0)).unsigned_abs();
                    let dy = i64::from(e.y1).saturating_sub(i64::from(e.y0)).unsigned_abs();
                    let px = |v: u64| v.checked_div(ONE as u64).unwrap_or(0);
                    cells = cells.saturating_add(px(dx.saturating_add(dy))).saturating_add(CELLS_PER_EDGE);
                    rows = rows.saturating_add(px(dy)).saturating_add(2);
                }
                let count = end.saturating_sub(start) as u64;
                max_item = max_item.max(count);
                let aa = u8::from(n.flags & CRISP == 0);
                let rule = u8::from(what == HAS_FILL && n.flags & EVEN_ODD != 0);
                items.push(DrawItem {
                    edges: [u32::try_from(start).unwrap_or(u32::MAX), u32::try_from(count).unwrap_or(u32::MAX)],
                    bbox: [bbox.x0, bbox.y0, bbox.x1, bbox.y1],
                    cells,
                    edge_rows: rows,
                    color: crate::paint::premultiply(color),
                    rule,
                    aa,
                    pad: [0; 2],
                })?;
            }
        }
        drop((poly, scratch, dashbuf, devbuf, head, dash_scaled));
        Ok(Plan {
            width: px.width,
            height: px.height,
            items: items.freeze(),
            edges: edges.freeze(),
            max_item_edges: max_item,
            limits: opts.limits,
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

/// 描く座標系の点の外接矩形（箱で切る前。有限な点だけ）。
struct DevBox {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl DevBox {
    fn new() -> DevBox {
        DevBox { x0: f64::INFINITY, y0: f64::INFINITY, x1: f64::NEG_INFINITY, y1: f64::NEG_INFINITY }
    }

    fn add(&mut self, p: Point) {
        if p.finite() {
            self.x0 = math::min(self.x0, p.x);
            self.y0 = math::min(self.y0, p.y);
            self.x1 = math::max(self.x1, p.x);
            self.y1 = math::max(self.y1, p.y);
        }
    }

    /// 画素の矩形（1 画素の余白。外接矩形がなければ空）。
    fn pixels(&self) -> PixelBox {
        if !(self.x0 <= self.x1 && self.y0 <= self.y1) {
            return PixelBox { x0: 0, y0: 0, x1: 0, y1: 0 };
        }
        let f = |v: f64| if v.is_nan() { 0 } else { v as i64 };
        PixelBox {
            x0: f(self.x0.floor()).saturating_sub(1),
            y0: f(self.y0.floor()).saturating_sub(1),
            x1: f(self.x1.ceil()).saturating_add(1),
            y1: f(self.y1.ceil()).saturating_add(1),
        }
    }
}

/// 部分パスの点と閉じたかを受け取る口。
type SubpathFn<'a> = dyn FnMut(&[Point], bool) -> Result<(), Error> + 'a;

/// 部分パスごとに平坦化した点（ユーザー単位）を `poly` に集め、`f(点, 閉じたか)` を呼ぶ。
fn subpaths(
    path: PathRef<'_>,
    t: &Transform,
    q: Quality,
    reach: f64,
    poly: &mut MVec<'_, Point>,
    budget: &Budget<'_>,
    f: &mut SubpathFn<'_>,
) -> Result<(), Error> {
    poly.clear();
    let mut open = false;
    for seg in path.segments() {
        match seg {
            PathSeg::Move(p) => {
                if open {
                    f(poly.as_slice(), false)?;
                }
                poly.clear();
                poly.push(p)?;
                open = true;
            }
            PathSeg::Close(..) => {
                if open {
                    f(poly.as_slice(), true)?;
                }
                poly.clear();
                open = false;
            }
            _ => {
                flatten_seg(&seg, t, q.tol, reach, budget, &mut |p| poly.push(p))?;
            }
        }
    }
    if open {
        f(poly.as_slice(), false)?;
    }
    Ok(())
}

/// 塗りの辺（部分パスはどれも閉じて塗る）。
fn fill_edges(
    path: PathRef<'_>,
    t: &Transform,
    q: Quality,
    poly: &mut MVec<'_, Point>,
    edges: &mut MVec<'_, Edge>,
    dev: &mut DevBox,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    subpaths(path, t, q, 0.0, poly, budget, &mut |pts, _| {
        let Some(&first) = pts.first() else { return Ok(()) };
        let mut prev = t.apply(first);
        dev.add(prev);
        for &p in pts.iter().skip(1) {
            let d = t.apply(p);
            dev.add(d);
            budget.spend(cost::OP_STEP)?;
            push_segment(prev, d, edges)?;
            prev = d;
        }
        budget.spend(cost::OP_STEP)?;
        push_segment(prev, t.apply(first), edges)
    })
}

/// 線の一時の置き場（部分パスの点・まとめた点・破線の部分の点・non-scaling の描く座標系の点・掛けた破線の並び）。
struct StrokeParts<'a, 'b> {
    poly: &'a mut MVec<'b, Point>,
    scratch: &'a mut MVec<'b, Point>,
    dashbuf: &'a mut MVec<'b, Point>,
    devbuf: &'a mut MVec<'b, Point>,
    head: &'a mut MVec<'b, Point>,
    dash_scaled: &'a mut MVec<'b, f64>,
}

/// 外形の輪を描く座標系の辺にする口。
struct EdgeSink<'e, 'b> {
    edges: &'e mut MVec<'b, Edge>,
    dev: &'e mut DevBox,
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
        self.dev.add(d);
        match self.prev {
            Some(a) => {
                self.budget.spend(cost::OP_STEP)?;
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
    scale: f64,
    q: Quality,
    parts: StrokeParts<'_, '_>,
    edges: &mut MVec<'_, Edge>,
    dev: &mut DevBox,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let non_scaling = n.flags & NON_SCALING != 0;
    let pen = pen_of(n, t, scale, q);
    // 外形を作る座標系：ふつうはユーザー単位（描く座標系へは t）、non-scaling は描く座標系（そのまま）
    let to_dev = if non_scaling { Transform::IDENTITY } else { *t };
    let sigma = if non_scaling { 1.0 } else { t.max_scale() };
    let reach = pen.half * sigma * reach_factor(&pen);
    let path = tree.path(n);
    let dashes_src = tree.dash_list(n);
    // pathLength の倍（線を作る座標系の長さの和 / pathLength）
    let mut factor = 1.0;
    if n.path_length > 0.0 && !dashes_src.is_empty() {
        let mut total = 0.0;
        subpaths(path, t, q, reach, parts.poly, budget, &mut |pts, closed| {
            let m = |p: Point| if non_scaling { t.apply(p) } else { p };
            let mut a = match pts.first() {
                Some(&p) => m(p),
                None => return Ok(()),
            };
            for &p in pts.iter().skip(1) {
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
    let dash_scale = if non_scaling { factor * scale } else { factor };
    let StrokeParts { poly, scratch, dashbuf, devbuf, head, dash_scaled } = parts;
    dash_scaled.clear();
    for &d in dashes_src {
        budget.spend(cost::OP_STEP)?;
        dash_scaled.push(d * dash_scale)?;
    }
    let dashes = Dashes::new(dash_scaled.as_slice(), n.dash_offset * dash_scale);
    let mut sink = EdgeSink { edges, dev, t: to_dev, first: None, prev: None, budget: budget_ref(budget) };
    subpaths(path, t, q, reach, poly, budget, &mut |pts, closed| {
        if non_scaling {
            // non-scaling は描く座標系の点にしてから線を作る
            devbuf.clear();
            for &p in pts {
                devbuf.push(t.apply(p))?;
            }
            stroke_one(
                devbuf.as_slice(),
                closed,
                &pen,
                dashes.as_ref(),
                [&mut *dashbuf, &mut *head, &mut *scratch],
                &mut sink,
                budget,
            )
        } else {
            stroke_one(
                pts,
                closed,
                &pen,
                dashes.as_ref(),
                [&mut *dashbuf, &mut *head, &mut *scratch],
                &mut sink,
                budget,
            )
        }
    })?;
    sink.close_loop()
}

/// 1 本の部分パスの線（破線があれば部分ごと）。`bufs` は破線の部分・最初の部分・まとめた点の置き場。
fn stroke_one(
    pts: &[Point],
    closed: bool,
    pen: &Pen,
    dashes: Option<&Dashes<'_>>,
    bufs: [&mut MVec<'_, Point>; 3],
    sink: &mut EdgeSink<'_, '_>,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let [dashbuf, head, scratch] = bufs;
    match dashes {
        None => stroke_polyline(pts, closed, pen, scratch, budget, sink),
        Some(d) => dash_polyline(pts, closed, d, dashbuf, head, budget, &mut |piece| {
            stroke_polyline(piece, false, pen, scratch, budget, sink)
        }),
    }
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
