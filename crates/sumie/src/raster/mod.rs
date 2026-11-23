//! 段 6 の一部：ラスタライズ（architecture.md §6.1・§6.2・§8・ADR-15・ADR-30・ADR-38）。
//!
//! - 辺は描く座標系の線分を、決まった箱（±2²² 画素。キャンバスにも範囲にも依らない）で切ってから、1/256 画素の固定小数に
//!   直したもの。箱の左の外の部分は同じ y の縦の線（x = −箱）にし、右・上・下の外は落とす（右の外の辺はキャンバスの画素に
//!   効かない）。y は下向きに揃え、向きは `dir`（±1）に持つ。水平な辺は持たない
//! - 反エイリアス：行ごとに、辺がかかる画素（セル）に整数の `cover`（縦の通過）と `area`（面積の重み）を積み、左から掃いて
//!   被覆を決める。行の境と列の境を横切る点は、辺の元の両端から整数の閉じた式（床の割り算）で求めるので、どの行・列から
//!   始めても同じ値になる。範囲の左の外のセルは、左の 1 列に cover だけを足す（和は同じ整数）。右の外は落とす
//! - `crispEdges`（反エイリアスなし）：画素の中心の行（y + 1/2）を辺が横切る点を閉じた式で求め、中心がその点以上の画素から
//!   巻き数を足す
//! - 被覆の値は `2 · ONE · Σcover − area`（全面で `FULL`）。非ゼロは `min(|v|, FULL)`、偶奇は `|v|` を `2 · FULL` で折り返す

use crate::error::Error;
use crate::geom::Point;
use crate::geom::flatten::BOX;
use crate::mem::{MVec, Plain};

/// 1 画素の固定小数の単位。
pub(crate) const ONE: i64 = 256;
/// 全面の被覆の値（`2 · ONE · ONE`）。
pub(crate) const FULL: i64 = 131_072;
/// 半画素。
const HALF: i64 = 128;
/// 偶奇の折り返しの周期。
const TWO_FULL: i64 = 262_144;
/// 決まった箱の固定小数（2³⁰）。
const BOX_FIXED: i64 = 1 << 30;

/// 固定小数の辺（`y0 < y1`）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Edge {
    pub(crate) x0: i32,
    pub(crate) y0: i32,
    pub(crate) x1: i32,
    pub(crate) y1: i32,
    /// 向き（もとの線分が下向きなら +1、上向きなら −1）
    pub(crate) dir: i32,
}

impl Plain for Edge {}

/// 塗りの規則。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FillRule {
    NonZero,
    EvenOdd,
}

/// 画素の座標を固定小数に（箱の内の値を丸める。箱で切ったあとなので ±2³⁰ に収まる）。
fn to_fixed(v: f64) -> i32 {
    let f = (v * ONE as f64).round();
    let c = if f < -(BOX_FIXED as f64) {
        -(BOX_FIXED as f64)
    } else if f > BOX_FIXED as f64 {
        BOX_FIXED as f64
    } else {
        f
    };
    c as i32
}

/// 固定小数の辺を足す（水平なら足さない）。
fn push_fixed(out: &mut MVec<'_, Edge>, a: Point, b: Point) -> Result<(), Error> {
    let (x0, y0, x1, y1) = (to_fixed(a.x), to_fixed(a.y), to_fixed(b.x), to_fixed(b.y));
    if y0 == y1 {
        return Ok(());
    }
    let e = if y0 < y1 { Edge { x0, y0, x1, y1, dir: 1 } } else { Edge { x0: x1, y0: y1, x1: x0, y1: y0, dir: -1 } };
    out.push(e)
}

/// 線分 a → b の上で y が `y` の点（`a.y != b.y`）。境に近い方の端から内挿する（遠い端から内挿すると、t が 1 の近くで
/// 丸まり、巨大な線分の箱の中の部分を失う。ファズと単位 2 の 3 回目のレビュー）。y はちょうど `y`。
pub(crate) fn at_y(a: Point, b: Point, y: f64) -> Point {
    let (n, f) = if (a.y - y).abs() <= (b.y - y).abs() { (a, b) } else { (b, a) };
    let s = (y - n.y) / (f.y - n.y);
    let x = n.x + (f.x - n.x) * s;
    Point::new(if x.is_finite() { x } else { n.x }, y)
}

/// 線分 a → b の上で x が `x` の点（`a.x != b.x`。[`at_y`] と同じに、近い方の端から内挿する）。
pub(crate) fn at_x(a: Point, b: Point, x: f64) -> Point {
    let (n, f) = if (a.x - x).abs() <= (b.x - x).abs() { (a, b) } else { (b, a) };
    let s = (x - n.x) / (f.x - n.x);
    let y = n.y + (f.y - n.y) * s;
    Point::new(x, if y.is_finite() { y } else { n.y })
}

/// 線分を y の範囲 [lo, hi] で切った端（範囲にかからなければ None）。外の端を、範囲の境の上の点に置き換える。
pub(crate) fn clip_y(a: Point, b: Point, lo: f64, hi: f64) -> Option<(Point, Point)> {
    if (a.y < lo && b.y < lo) || (a.y > hi && b.y > hi) {
        return None;
    }
    let fix = |p: Point| {
        if p.y < lo {
            at_y(a, b, lo)
        } else if p.y > hi {
            at_y(a, b, hi)
        } else {
            p
        }
    };
    Some((fix(a), fix(b)))
}

/// 線分を x の範囲 [lo, hi] で切った端（[`clip_y`] と同じ）。
pub(crate) fn clip_x(a: Point, b: Point, lo: f64, hi: f64) -> Option<(Point, Point)> {
    if (a.x < lo && b.x < lo) || (a.x > hi && b.x > hi) {
        return None;
    }
    let fix = |p: Point| {
        if p.x < lo {
            at_x(a, b, lo)
        } else if p.x > hi {
            at_x(a, b, hi)
        } else {
            p
        }
    };
    Some((fix(a), fix(b)))
}

/// 描く座標系の線分を、決まった箱で切って固定小数の辺にし `out` に足す。有限でない点は足さない（呼び手が描かないと決める）。
/// y は箱で切り、x は箱の境で分けて、右の外の部分は捨て、左の外の部分は箱の左の縁の縦の辺にする（左から足す被覆を保つ）。
pub(crate) fn push_segment(a: Point, b: Point, out: &mut MVec<'_, Edge>) -> Result<(), Error> {
    if !a.finite() || !b.finite() || a.y == b.y {
        return Ok(());
    }
    let Some((p, q)) = clip_y(a, b, -BOX, BOX) else { return Ok(()) };
    if p.y == q.y {
        return Ok(());
    }
    // x の箱の境で分ける（p から q の向きの順。多くて 3 つ）
    let mut pts = [p, q, q, q];
    let mut n = 1usize;
    let bounds = if p.x <= q.x { [-BOX, BOX] } else { [BOX, -BOX] };
    for x in bounds {
        if (p.x < x) != (q.x < x) && p.x != q.x {
            if let Some(c) = pts.get_mut(n) {
                *c = at_x(p, q, x);
            }
            n = n.saturating_add(1);
        }
    }
    if let Some(c) = pts.get_mut(n) {
        *c = q;
    }
    for k in 0..n {
        let (Some(&u), Some(&v)) = (pts.get(k), pts.get(k.saturating_add(1))) else { continue };
        if u.y == v.y {
            continue;
        }
        let mid = (u.x + v.x) / 2.0;
        if mid > BOX {
            continue;
        }
        if mid < -BOX {
            push_fixed(out, Point::new(-BOX, u.y), Point::new(-BOX, v.y))?;
        } else {
            push_fixed(out, u, v)?;
        }
    }
    Ok(())
}

/// 床の割り算（`d > 0`）。
pub(crate) fn floor_div(n: i64, d: i64) -> i64 {
    n.checked_div_euclid(d).unwrap_or(0)
}

/// 辺の y（固定小数）での x（床）。
pub(crate) fn x_at(e: &Edge, y: i64) -> i64 {
    let (x0, y0, x1, y1) = (i64::from(e.x0), i64::from(e.y0), i64::from(e.x1), i64::from(e.y1));
    let num = x1.saturating_sub(x0).saturating_mul(y.saturating_sub(y0));
    x0.saturating_add(floor_div(num, y1.saturating_sub(y0)))
}

/// 辺の x（固定小数）での y（床。辺は x0 ≠ x1）。
fn y_at(e: &Edge, x: i64) -> i64 {
    let (x0, y0, x1, y1) = (i64::from(e.x0), i64::from(e.y0), i64::from(e.x1), i64::from(e.y1));
    let dy = y1.saturating_sub(y0);
    let (num, den) = if x1 > x0 {
        (dy.saturating_mul(x.saturating_sub(x0)), x1.saturating_sub(x0))
    } else {
        (dy.saturating_mul(x0.saturating_sub(x)), x0.saturating_sub(x1))
    };
    y0.saturating_add(floor_div(num, den))
}

/// 1 行の被覆を集める置き場。列 `[x0, x0 + width)` の画素と、その左の 1 列（範囲の左の外の cover の和）を持つ。
pub(crate) struct RowCells<'b> {
    cover: MVec<'b, i64>,
    area: MVec<'b, i64>,
    /// 範囲の左の列（画素）
    x0: i64,
    width: usize,
    /// 触れたセルの番号の範囲（0 は左の 1 列）
    lo: usize,
    hi: usize,
    touched: bool,
}

impl<'b> RowCells<'b> {
    /// 幅 `width` の範囲（左の列 `x0`）の置き場を作る（0 で埋める）。
    pub(crate) fn new(x0: i64, width: usize, budget: &'b crate::budget::Budget<'b>) -> Result<RowCells<'b>, Error> {
        let n = width.saturating_add(1);
        let mut cover = MVec::with_capacity(budget, n)?;
        cover.resize_to(n, 0)?;
        let mut area = MVec::with_capacity(budget, n)?;
        area.resize_to(n, 0)?;
        Ok(RowCells { cover, area, x0, width, lo: 0, hi: 0, touched: false })
    }

    /// 触れたセルを 0 に戻す（触れた範囲の分を塊ごとに払う）。
    pub(crate) fn clear(&mut self, budget: &crate::budget::Budget<'_>) -> Result<(), Error> {
        if !self.touched {
            return Ok(());
        }
        let (lo, hi) = (self.lo, self.hi);
        let step = crate::budget::chunk_len(crate::cost::RASTER_SWEEP);
        let mut i = lo;
        while i <= hi {
            let end = i.saturating_add(step).min(hi.saturating_add(1));
            budget.spend((end.saturating_sub(i) as u64).saturating_mul(crate::cost::RASTER_SWEEP))?;
            for slot in self.cover.as_mut_slice().get_mut(i..end).into_iter().flatten() {
                *slot = 0;
            }
            for slot in self.area.as_mut_slice().get_mut(i..end).into_iter().flatten() {
                *slot = 0;
            }
            i = end;
        }
        self.touched = false;
        Ok(())
    }

    /// 触れたセルの番号の範囲（触れていなければ None）。
    pub(crate) fn touched(&self) -> Option<(usize, usize)> {
        self.touched.then_some((self.lo, self.hi))
    }

    /// 画素の列のセルの番号（左の外は 0、右の外は None）。
    fn index(&self, col: i64) -> Option<usize> {
        if col < self.x0 {
            return Some(0);
        }
        let i = col.saturating_sub(self.x0);
        let i = usize::try_from(i).ok()?;
        (i < self.width).then(|| i.saturating_add(1))
    }

    fn add(&mut self, idx: usize, cover: i64, area: i64) {
        if let Some(c) = self.cover.get_mut(idx) {
            *c = c.saturating_add(cover);
        }
        if let Some(a) = self.area.get_mut(idx) {
            *a = a.saturating_add(area);
        }
        if self.touched {
            self.lo = self.lo.min(idx);
            self.hi = self.hi.max(idx);
        } else {
            self.lo = idx;
            self.hi = idx;
            self.touched = true;
        }
    }

    /// 辺の、行 `row`（画素）の中の部分が回るセルの数の上限（範囲の列（左の外は 1 列にまとめる）で切った列の差 ＋ 1。回る前に
    /// 払う）。
    pub(crate) fn cells_bound(&self, e: &Edge, row: i64) -> u64 {
        let ry0 = row.saturating_mul(ONE);
        let ry1 = ry0.saturating_add(ONE);
        let ya = i64::from(e.y0).max(ry0);
        let yb = i64::from(e.y1).min(ry1);
        if ya >= yb {
            return 0;
        }
        let ca = floor_div(x_at(e, ya), ONE);
        let cb = floor_div(x_at(e, yb), ONE);
        let right = self.x0.saturating_add(i64::try_from(self.width).unwrap_or(i64::MAX));
        let lo = ca.min(cb).max(self.x0.saturating_sub(1));
        let hi = ca.max(cb).min(right);
        if hi < lo {
            return 1;
        }
        u64::try_from(hi.saturating_sub(lo)).unwrap_or(0).saturating_add(1)
    }

    /// 辺の、行 `row`（画素）の中の部分のセルを積む（反エイリアス）。セルを回るたびに `pay` で払い（塊の頭で塊の分を前払い）、
    /// 回ったセルの数を返す。
    pub(crate) fn add_edge(&mut self, e: &Edge, row: i64, pay: &mut Payer<'_>) -> Result<u64, Error> {
        let ry0 = row.saturating_mul(ONE);
        let ry1 = ry0.saturating_add(ONE);
        let ya = i64::from(e.y0).max(ry0);
        let yb = i64::from(e.y1).min(ry1);
        if ya >= yb {
            return Ok(0);
        }
        let dir = i64::from(e.dir);
        let xa = x_at(e, ya);
        let xb = x_at(e, yb);
        let ca = floor_div(xa, ONE);
        let cb = floor_div(xb, ONE);
        if ca == cb {
            let base = ca.saturating_mul(ONE);
            pay.step()?;
            if let Some(i) = self.index(ca) {
                let dy = yb.saturating_sub(ya);
                self.add(
                    i,
                    dir.saturating_mul(dy),
                    cell_area(dir, dy, xa.saturating_sub(base), xb.saturating_sub(base)),
                );
            }
            return Ok(1);
        }
        // 列の境を横切る y（閉じた式。行の中に切り詰める）
        let cross = |x: i64| y_at(e, x).max(ya).min(yb);
        let step: i64 = if cb > ca { 1 } else { -1 };
        // 範囲の左の外の部分は左の 1 列に cover だけ（範囲の左の境の y までの和）。右の外は落とす
        let left_col = self.x0;
        let right_end = self.x0.saturating_add(i64::try_from(self.width).unwrap_or(i64::MAX));
        let mut count = 0u64;
        let mut col = ca;
        let mut y_enter = ya;
        let mut x_enter = xa;
        loop {
            // この列から出る所
            let last = col == cb;
            let (x_exit, y_exit) = if last {
                (xb, yb)
            } else {
                let bx = if step > 0 { col.saturating_add(1).saturating_mul(ONE) } else { col.saturating_mul(ONE) };
                (bx, cross(bx))
            };
            if col < left_col {
                // 左の外：この列から範囲の左の境までをまとめて左の 1 列へ
                let (target_x, target_last) =
                    if step > 0 && cb >= left_col { (left_col.saturating_mul(ONE), false) } else { (xb, true) };
                let y_to = if target_last { yb } else { cross(target_x) };
                pay.step()?;
                if let Some(i) = self.index(col) {
                    self.add(i, dir.saturating_mul(y_to.saturating_sub(y_enter)), 0);
                }
                count = count.saturating_add(1);
                if target_last {
                    break;
                }
                col = left_col;
                y_enter = y_to;
                x_enter = target_x;
                continue;
            }
            if col >= right_end {
                if step > 0 {
                    break;
                }
                // 右の外から左へ来る：範囲の右の境まで飛ばす
                let bx = right_end.saturating_mul(ONE);
                if cb >= right_end {
                    break;
                }
                y_enter = cross(bx);
                x_enter = bx;
                col = right_end.saturating_sub(1);
                continue;
            }
            let base = col.saturating_mul(ONE);
            pay.step()?;
            if let Some(i) = self.index(col) {
                let dy = y_exit.saturating_sub(y_enter);
                self.add(
                    i,
                    dir.saturating_mul(dy),
                    cell_area(dir, dy, x_enter.saturating_sub(base), x_exit.saturating_sub(base)),
                );
            }
            count = count.saturating_add(1);
            if last {
                break;
            }
            y_enter = y_exit;
            x_enter = x_exit;
            col = col.saturating_add(step);
        }
        Ok(count)
    }

    /// 辺が画素の中心の行を横切れば、その点より右の画素から巻き数を足す（crispEdges）。
    pub(crate) fn add_edge_center(&mut self, e: &Edge, row: i64) -> u64 {
        let yc = row.saturating_mul(ONE).saturating_add(HALF);
        if yc < i64::from(e.y0) || yc >= i64::from(e.y1) {
            return 0;
        }
        let xc = x_at(e, yc);
        // 中心（px + 1/2）が xc 以上の最初の画素：⌈(xc − ONE/2) / ONE⌉
        let px = floor_div(xc.saturating_sub(HALF).saturating_add(ONE - 1), ONE);
        if let Some(i) = self.index(px) {
            self.add(i, i64::from(e.dir), 0);
        }
        1
    }

    /// セルの番号 `idx` の cover と area。
    pub(crate) fn cell(&self, idx: usize) -> (i64, i64) {
        (self.cover.get(idx).copied().unwrap_or(0), self.area.get(idx).copied().unwrap_or(0))
    }

    pub(crate) fn width(&self) -> usize {
        self.width
    }

    pub(crate) fn x0(&self) -> i64 {
        self.x0
    }
}

/// 回る前に塊ごとに払う口（`left` 個（1 つ `per` 単位）までを、`CANCEL_INTERVAL` 未満の塊に分け、塊を回り始める前に
/// 払う。ループの全体を一度に前払いしない：中止を見ない間の手間を間隔の内に保つ。単位 2 のレビュー）。
pub(crate) struct Payer<'a> {
    budget: &'a crate::budget::Budget<'a>,
    left: u64,
    per: u64,
    chunk: u64,
    credit: u64,
}

impl<'a> Payer<'a> {
    pub(crate) fn new(budget: &'a crate::budget::Budget<'a>, left: u64, per: u64) -> Payer<'a> {
        Payer { budget, left, per, chunk: crate::budget::chunk_len(per) as u64, credit: 0 }
    }

    /// 1 つ回る（塊の頭なら塊の分を払う。数えた数を超えたら 1 つずつ払う）。
    pub(crate) fn step(&mut self) -> Result<(), Error> {
        if self.credit == 0 {
            let k = self.chunk.min(self.left).max(1);
            self.budget.spend(k.saturating_mul(self.per))?;
            self.left = self.left.saturating_sub(k);
            self.credit = k;
        }
        self.credit = self.credit.saturating_sub(1);
        Ok(())
    }
}

/// 画素の矩形（`x0 ≤ x < x1`・`y0 ≤ y < y1`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PixelBox {
    pub(crate) x0: i64,
    pub(crate) y0: i64,
    pub(crate) x1: i64,
    pub(crate) y1: i64,
}

impl PixelBox {
    pub(crate) fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    pub(crate) fn intersect(&self, o: &PixelBox) -> PixelBox {
        PixelBox { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) }
    }

    pub(crate) fn width(&self) -> u64 {
        u64::try_from(self.x1.saturating_sub(self.x0)).unwrap_or(0)
    }

    pub(crate) fn height(&self) -> u64 {
        u64::try_from(self.y1.saturating_sub(self.y0)).unwrap_or(0)
    }
}

/// 行ごとの処理の置き場（セルと、行にかかる辺の番号）。
pub(crate) struct Scan<'b> {
    pub(crate) cells: RowCells<'b>,
    active: MVec<'b, u32>,
}

impl<'b> Scan<'b> {
    /// 範囲の幅の置き場を作る（行にかかる辺の番号は `max_edges` 個まで。伸ばさない）。
    pub(crate) fn new(
        x0: i64,
        width: usize,
        max_edges: usize,
        budget: &'b crate::budget::Budget<'b>,
    ) -> Result<Scan<'b>, Error> {
        Ok(Scan { cells: RowCells::new(x0, width, budget)?, active: MVec::with_capacity(budget, max_edges)? })
    }
}

/// 範囲 `rect` の行ごとに、辺 `edges`（`y0` の順に並べたもの）の被覆を求め、被覆の同じ画素の並びごとに
/// `span(x, y, 長さ, 被覆)` を呼ぶ（被覆 0 は呼ばない。被覆は 0〜`FULL`）。`rect` の x は `scan` を作った範囲の中。払う手間は
/// 行・かかる辺・回ったセル・掃いた幅の分（`cost` の式）。
/// 辺が範囲の右の境（固定小数 `right`）より右にあるか（セルは右の外を捨てるので、画素に効かない）。
pub(crate) fn skips_right(e: &Edge, right: i64) -> bool {
    i64::from(e.x0.min(e.x1)) >= right
}

pub(crate) fn fill_region(
    edges: &[Edge],
    rule: FillRule,
    aa: bool,
    rect: PixelBox,
    scan: &mut Scan<'_>,
    budget: &crate::budget::Budget<'_>,
    span: &mut dyn FnMut(i64, i64, u64, i64) -> Result<(), Error>,
) -> Result<(), Error> {
    use crate::cost;
    scan.active.clear();
    if rect.is_empty() || edges.is_empty() {
        return Ok(());
    }
    let mut next = 0usize;
    let mut row = rect.y0;
    let right = rect.x1.saturating_mul(ONE);
    while row < rect.y1 {
        budget.spend(cost::RASTER_ROW)?;
        let row_start = row.saturating_mul(ONE);
        let row_end = row_start.saturating_add(ONE);
        // かかり始める辺を足す（範囲の右の外にある辺は、どの画素にも効かないので足さない。[`skips_right`]）
        while let Some(e) = edges.get(next) {
            if i64::from(e.y0) >= row_end {
                break;
            }
            let idx = u32::try_from(next).map_err(|_| budget.too_large())?;
            next = next.saturating_add(1);
            if skips_right(e, right) {
                budget.spend(cost::OP_STEP)?;
                continue;
            }
            budget.spend(cost::RASTER_EDGE_ROW)?;
            scan.active.push(idx)?;
        }
        // 終わった辺を除く（順は保つ。かかる辺ごとに、塊の頭で払う）
        let mut keep = 0usize;
        let n = scan.active.len();
        let mut pay = Payer::new(budget, n as u64, cost::OP_STEP);
        for k in 0..n {
            pay.step()?;
            let Some(&i) = scan.active.get(k) else { continue };
            let alive = edges.get(i as usize).is_some_and(|e| i64::from(e.y1) > row_start);
            if alive {
                if let Some(slot) = scan.active.get_mut(keep) {
                    *slot = i;
                }
                keep = keep.saturating_add(1);
            }
        }
        scan.active.truncate(keep);
        if scan.active.is_empty() && next >= edges.len() {
            break;
        }
        // セルを積む
        scan.cells.clear(budget)?;
        for k in 0..scan.active.len() {
            let Some(&i) = scan.active.get(k) else { continue };
            let Some(e) = edges.get(i as usize) else { continue };
            // セルは回りながら塊ごとに払う（1 行の中で全部の列を回る辺でも、中止を見ない間は間隔の内。単位 2 のレビュー）
            budget.spend(cost::RASTER_EDGE_ROW)?;
            if aa {
                let bound = scan.cells.cells_bound(e, row);
                let mut pay = Payer::new(budget, bound, cost::RASTER_CELL);
                scan.cells.add_edge(e, row, &mut pay)?;
            } else {
                budget.spend(cost::RASTER_CELL)?;
                scan.cells.add_edge_center(e, row);
            }
        }
        // 掃く
        if let Some((lo, hi)) = scan.cells.touched() {
            let width = scan.cells.width();
            let x0 = scan.cells.x0();
            let mut pay = Payer::new(budget, (hi.saturating_sub(lo) as u64).saturating_add(1), cost::RASTER_SWEEP);
            let mut acc: i64 = 0;
            let mut run_start: i64 = 0;
            let mut run_len: u64 = 0;
            let mut run_cov: i64 = 0;
            let value = |acc: i64, area: i64| -> i64 {
                if aa {
                    coverage(acc.saturating_mul(2 * ONE).saturating_sub(area), rule)
                } else if inside(acc, rule) {
                    FULL
                } else {
                    0
                }
            };
            for idx in lo..=hi {
                pay.step()?;
                let (c, a) = scan.cells.cell(idx);
                acc = acc.saturating_add(c);
                if idx == 0 {
                    continue;
                }
                let col = x0.saturating_add(i64::try_from(idx).unwrap_or(0)).saturating_sub(1);
                let cov = value(acc, a);
                if run_len > 0 && cov == run_cov {
                    run_len = run_len.saturating_add(1);
                } else {
                    if run_len > 0 && run_cov != 0 {
                        span(run_start, row, run_len, run_cov)?;
                    }
                    run_start = col;
                    run_len = 1;
                    run_cov = cov;
                }
            }
            // 触れた最後のセルの右は、範囲の右の終わりまで同じ被覆
            let tail_from = x0.saturating_add(i64::try_from(hi).unwrap_or(0));
            let tail_end = x0.saturating_add(i64::try_from(width).unwrap_or(0)).min(rect.x1);
            let tail = value(acc, 0);
            if tail_from < tail_end {
                let len = u64::try_from(tail_end.saturating_sub(tail_from)).unwrap_or(0);
                if run_len > 0 && tail == run_cov {
                    run_len = run_len.saturating_add(len);
                } else {
                    if run_len > 0 && run_cov != 0 {
                        span(run_start, row, run_len, run_cov)?;
                    }
                    run_start = tail_from;
                    run_len = len;
                    run_cov = tail;
                }
            }
            if run_len > 0 && run_cov != 0 {
                span(run_start, row, run_len, run_cov)?;
            }
        }
        row = row.saturating_add(1);
    }
    Ok(())
}

/// セルの面積の重み（向き × 縦の通過 × セルの中の入る x と出る x の和）。
fn cell_area(dir: i64, dy: i64, fx0: i64, fx1: i64) -> i64 {
    dir.saturating_mul(dy).saturating_mul(fx0.saturating_add(fx1))
}

/// 反エイリアスの被覆の値（`2 · ONE · acc − area`）を規則で 0〜`FULL` にする。
pub(crate) fn coverage(v: i64, rule: FillRule) -> i64 {
    let a = v.saturating_abs();
    match rule {
        FillRule::NonZero => a.min(FULL),
        FillRule::EvenOdd => {
            let m = a.checked_rem_euclid(TWO_FULL).unwrap_or(0);
            if m > FULL { TWO_FULL.saturating_sub(m) } else { m }
        }
    }
}

/// crispEdges の巻き数を規則で「中か」にする。
pub(crate) fn inside(winding: i64, rule: FillRule) -> bool {
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding.checked_rem_euclid(2).unwrap_or(0) != 0,
    }
}

/// 被覆（0〜`FULL`）を 8 ビットの α にする（仮の丸め：四捨五入。ADR-89 (4) で差の例を見てから決める）。
pub(crate) fn coverage_alpha(c: i64) -> u8 {
    let v = c.saturating_mul(255).saturating_add(FULL / 2).checked_div(FULL).unwrap_or(0);
    u8::try_from(v.max(0)).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests;
