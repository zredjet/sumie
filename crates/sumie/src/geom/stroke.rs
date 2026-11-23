//! 線の外形と破線（architecture.md §6.1・FR-PT-03・ADR-91）。
//!
//! - 入力は平坦化した折れ線（ユーザー単位。`non-scaling-stroke` は描く座標系）。続く同じ点は 1 つにまとめる
//! - 外形は閉じた輪の並びで、非ゼロで塗る。開いた部分パスは「左の縁を前へ → 終わりの端 → 右の縁を後ろへ → 始めの端」の
//!   1 つの輪、閉じた部分パスは左の縁の輪（前へ）と右の縁の輪（後ろへ）の 2 つ（向きが逆なので、縁の間だけが巻き数 ±1）
//! - 継ぎ目：外側は miter（比 `1 / sin(θ/2)` が miterlimit を超えれば bevel。miterlimit が 1 未満ならいつも bevel）・round・
//!   bevel。内側は、両側の縁の線の交わる点が両側の区間の半分の長さに収まればその点（重なりを作らない：面積の被覆は巻き数
//!   2 の重なりの縁の画素を濃くする。ADR-96 (1)）、収まらなければ頂点を通して隙間を作らない（重なる所は非ゼロで同じに塗る）
//! - 端：butt・round（半円）・square（太さの半分だけ伸ばす）。長さ 0 の部分パス（2 点以上か閉じた 1 点で、点がすべて同じ）は、
//!   round なら円、square なら軸に沿う正方形、butt は描かない。`M` だけの部分パス（1 点で開いたもの）は描かない（ADR-91 (4)）
//! - round の継ぎ目と端の円弧は、描く座標系の半径（太さの半分 × 変形の最大の倍率）と平坦化と同じ誤差の式で分ける数を決め、
//!   角を均等に分ける
//! - 破線：長さはユーザー単位（`pathLength` の倍は呼び手が掛ける）。部分パスごとに始め直す（ADR-91 (4)）。長さ 0 の破線は
//!   長さ 0 の部分パスとして端を描く

use super::Point;
use crate::budget::{Budget, chunk_len};
use crate::cost;
use crate::error::Error;
use crate::math;

/// 端の形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cap {
    Butt,
    Round,
    Square,
}

/// 継ぎ目の形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Join {
    Miter,
    Round,
    Bevel,
}

/// 線のペン。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pen {
    /// 太さの半分（0 より大きい）
    pub(crate) half: f64,
    pub(crate) cap: Cap,
    pub(crate) join: Join,
    pub(crate) miter_limit: f64,
    /// 外形の座標から描く座標系への最大の倍率（round の円弧を分ける数を決める）
    pub(crate) scale: f64,
    /// 許容の誤差（画素）
    pub(crate) tol: f64,
}

impl Pen {
    /// 角 `angle`（ラジアン）の円弧を分ける数（1 以上）。平坦化の弧と同じ式（`⌈|θ| √(r / (8 tol))⌉`）。
    pub(crate) fn round_pieces(&self, angle: f64) -> u64 {
        let r = self.half * self.scale;
        let x = angle.abs() * (r / (8.0 * self.tol)).sqrt();
        if x.is_nan() || x >= 1.8e19 {
            return u64::MAX;
        }
        let n = x.ceil();
        if n < 1.0 { 1 } else { n as u64 }
    }
}

/// 外形の輪を受け取る口。
pub(crate) trait LoopSink {
    /// 新しい輪を始める
    fn begin(&mut self) -> Result<(), Error>;
    /// 輪の点（輪は終わりで始めの点へ閉じる）
    fn point(&mut self, p: Point) -> Result<(), Error>;
}

/// 点の向き（単位の長さ。長さ 0 なら None）。
fn direction(a: Point, b: Point) -> Option<Point> {
    // 差を半分にしてから求め（どちらの点も有限なら差の半分は有限）、成分の大きさの最大で割ってから長さを求める（hypot も
    // あふれない。単位の 1 回目・2 回目のレビュー）
    let (dx, dy) = (b.x / 2.0 - a.x / 2.0, b.y / 2.0 - a.y / 2.0);
    let m = math::max(dx.abs(), dy.abs());
    if !(m > 0.0 && m.is_finite()) {
        return None;
    }
    let (ux, uy) = (dx / m, dy / m);
    let l = math::hypot(ux, uy);
    (l > 0.0 && l.is_finite()).then(|| Point::new(ux / l, uy / l))
}

/// 区間の長さ（差を半分にしてから求めて 2 倍。あふれれば ∞）。
fn seg_len(a: Point, b: Point) -> f64 {
    let (hx, hy) = (b.x / 2.0 - a.x / 2.0, b.y / 2.0 - a.y / 2.0);
    let m = math::max(hx.abs(), hy.abs());
    if !(m > 0.0 && m.is_finite()) {
        return if m == 0.0 { 0.0 } else { f64::INFINITY };
    }
    2.0 * m * math::hypot(hx / m, hy / m)
}

/// 左の法線（向き `d` を 90° 回したもの。y が下向きの座標で画面の右回り）。
fn normal(d: Point) -> Point {
    Point::new(-d.y, d.x)
}

fn add(p: Point, v: Point, k: f64) -> Point {
    Point::new(p.x + v.x * k, p.y + v.y * k)
}

/// 外形を作る折れ線の手間を払いながら点を出す口。
struct Emit<'s, 'b> {
    sink: &'s mut dyn LoopSink,
    budget: &'b Budget<'b>,
}

impl Emit<'_, '_> {
    fn point(&mut self, p: Point) -> Result<(), Error> {
        self.budget.spend(cost::OUTLINE_POINT)?;
        self.sink.point(p)
    }

    fn begin(&mut self) -> Result<(), Error> {
        self.budget.spend(cost::OUTLINE_POINT)?;
        self.sink.begin()
    }

    /// 中心 `c`・半径 `r` の、向き `from` から角 `sweep`（ラジアン。正は y が下向きの座標で時計回り）の円弧の点（始めの点を
    /// 除き、終わりの点を含む）。分ける数は `pen.round_pieces`。
    fn arc(&mut self, pen: &Pen, c: Point, from: Point, sweep: f64) -> Result<(), Error> {
        let n = pen.round_pieces(sweep);
        let a0 = math::atan2(from.y, from.x);
        let step = chunk_len(cost::OUTLINE_POINT) as u64;
        let nf = n as f64;
        let mut i = 1u64;
        while i <= n {
            let k = step.min(n.saturating_sub(i).saturating_add(1));
            self.budget.spend(k.saturating_mul(cost::OUTLINE_POINT))?;
            let end = i.saturating_add(k);
            while i < end {
                let a = a0 + sweep * ((i as f64) / nf);
                let p = Point::new(c.x + pen.half * math::cos(a), c.y + pen.half * math::sin(a));
                self.sink.point(p)?;
                i = i.saturating_add(1);
            }
        }
        Ok(())
    }
}

/// 続く同じ点をまとめた折れ線の点を `out` に入れ直す（`out` は呼び手の置き場）。
fn dedup(pts: &[Point], out: &mut crate::mem::MVec<'_, Point>, budget: &Budget<'_>) -> Result<(), Error> {
    out.clear();
    for &p in pts {
        budget.spend(cost::OP_STEP)?;
        if out.last() != Some(&p) {
            out.push(p)?;
        }
    }
    Ok(())
}

/// 1 本の部分パスの折れ線 `pts`（`closed` なら閉じる）の外形の輪を `sink` に渡す。`scratch` は点をまとめる置き場。
pub(crate) fn stroke_polyline(
    pts: &[Point],
    closed: bool,
    pen: &Pen,
    scratch: &mut crate::mem::MVec<'_, Point>,
    budget: &Budget<'_>,
    sink: &mut dyn LoopSink,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    if pts.is_empty() || (pts.len() == 1 && !closed) {
        return Ok(());
    }
    dedup(pts, scratch, budget)?;
    // 閉じた折れ線の終わりが始めと同じなら 1 つにする
    if closed && scratch.len() > 1 && scratch.last() == scratch.as_slice().first() {
        let n = scratch.len().saturating_sub(1);
        scratch.truncate(n);
    }
    let v = scratch.as_slice();
    let mut e = Emit { sink, budget };
    if v.len() == 1 {
        return zero_length(&mut e, pen, v.first().copied().unwrap_or(Point::new(0.0, 0.0)));
    }
    if closed {
        // 左の縁の輪（前へ）と右の縁の輪（後ろへ）
        let n = v.len();
        for forward in [true, false] {
            e.begin()?;
            for k in 0..n {
                let i = if forward { k } else { n.saturating_sub(1).saturating_sub(k) };
                let (back, ahead) = (ring_prev(i, n), ring_next(i, n));
                let (a, b) = if forward { (back, ahead) } else { (ahead, back) };
                let (Some(&p), Some(&a), Some(&b)) = (v.get(i), v.get(a), v.get(b)) else { continue };
                vertex(&mut e, pen, a, p, b, forward)?;
            }
        }
        return Ok(());
    }
    // 開いた折れ線：左の縁を前へ・終わりの端・右の縁を後ろへ・始めの端
    let n = v.len();
    let first = v.first().copied().unwrap_or(Point::new(0.0, 0.0));
    let second = v.get(1).copied().unwrap_or(first);
    let last = v.last().copied().unwrap_or(first);
    let before_last = v.get(n.saturating_sub(2)).copied().unwrap_or(last);
    e.begin()?;
    let d0 = direction(first, second).unwrap_or(Point::new(1.0, 0.0));
    e.point(add(first, normal(d0), pen.half))?;
    for i in 1..n.saturating_sub(1) {
        let (Some(&a), Some(&p), Some(&b)) = (v.get(ring_prev(i, n)), v.get(i), v.get(ring_next(i, n))) else {
            continue;
        };
        vertex(&mut e, pen, a, p, b, true)?;
    }
    let dn = direction(before_last, last).unwrap_or(d0);
    cap(&mut e, pen, last, dn)?;
    for i in (1..n.saturating_sub(1)).rev() {
        let (Some(&a), Some(&p), Some(&b)) = (v.get(ring_next(i, n)), v.get(i), v.get(ring_prev(i, n))) else {
            continue;
        };
        vertex(&mut e, pen, a, p, b, false)?;
    }
    cap(&mut e, pen, first, Point::new(-d0.x, -d0.y))?;
    Ok(())
}

/// 輪の `n` 個のうち `i` の前と次の番号。
fn ring_prev(i: usize, n: usize) -> usize {
    if i == 0 { n.saturating_sub(1) } else { i.saturating_sub(1) }
}

fn ring_next(i: usize, n: usize) -> usize {
    let j = i.saturating_add(1);
    if j >= n { 0 } else { j }
}

/// 頂点 `p`（前の点 `a`・次の点 `b`）の左の縁の点：入る区間の終わりの縁・継ぎ目・出る区間の始めの縁。ちょうどの折り返し
/// （外積 0・内積が負）は左右のどちらも外側に見えるので、前へ回る縁（`forward`）だけを外側にし、後ろへ回る縁は内側として
/// 頂点を通す（継ぎ目を 2 度作らない。塗る所は同じ。単位 1 の 2 回目のレビュー）。
fn vertex(e: &mut Emit<'_, '_>, pen: &Pen, a: Point, p: Point, b: Point, forward: bool) -> Result<(), Error> {
    e.budget.spend(cost::OP_STEP)?;
    let (Some(d1), Some(d2)) = (direction(a, p), direction(p, b)) else { return Ok(()) };
    let (n1, n2) = (normal(d1), normal(d2));
    let end_in = add(p, n1, pen.half);
    let start_out = add(p, n2, pen.half);
    // 曲がる向き：外積が負なら左が外側（y が下向きの座標で、左の縁が外）
    let cross = d1.x * d2.y - d1.y * d2.x;
    let dot = d1.x * d2.x + d1.y * d2.y;
    if cross > 0.0 {
        // 左は内側：両側の縁の線の交わる点（内側の miter の点）が、両側の区間の半分の長さの内に収まれば、その 1 点にする
        // （頂点を通すと縁の近くに巻き数 2 の重なりができ、面積の被覆が縁の画素を濃くする。隣の頂点の点とも交わらない）
        let k = pen.half * cross / (1.0 + dot);
        let room = 0.5 * math::min(seg_len(a, p), seg_len(p, b));
        if 1.0 + dot > 0.0 && k.is_finite() && k <= room {
            let s = pen.half / (1.0 + dot);
            return e.point(Point::new(p.x + (n1.x + n2.x) * s, p.y + (n1.y + n2.y) * s));
        }
    }
    e.point(end_in)?;
    if cross > 0.0 || (cross == 0.0 && dot < 0.0 && !forward) {
        // 左は内側で交わる点が区間に収まらない（鋭い折れ・短い区間）：頂点を通す
        e.point(p)?;
        return e.point(start_out);
    }
    if cross == 0.0 && dot >= 0.0 {
        // まっすぐ：何も足さない
        return Ok(());
    }
    match pen.join {
        Join::Bevel => {}
        Join::Round => {
            // 外側の円弧（曲がる角だけ。左の縁が外なので負の向き。折り返し（外積 0・内積が負）は半回り）
            let turn = if cross == 0.0 { -core::f64::consts::PI } else { math::atan2(cross, dot) };
            return e.arc(pen, p, n1, turn);
        }
        Join::Miter => {
            // 比の 2 乗の逆数 = (1 + cos φ) / 2 ≥ 1 / limit²
            let limit = pen.miter_limit;
            if limit >= 1.0 && (1.0 + dot) * limit * limit >= 2.0 {
                let k = pen.half / (1.0 + dot);
                e.point(Point::new(p.x + (n1.x + n2.x) * k, p.y + (n1.y + n2.y) * k))?;
            }
        }
    }
    e.point(start_out)
}

/// 点 `p` で向き `d` に終わる端（左の縁の点から右の縁の点まで）。
fn cap(e: &mut Emit<'_, '_>, pen: &Pen, p: Point, d: Point) -> Result<(), Error> {
    let n = normal(d);
    let left = add(p, n, pen.half);
    let right = add(p, n, -pen.half);
    match pen.cap {
        Cap::Butt => {
            e.point(left)?;
            e.point(right)
        }
        Cap::Square => {
            e.point(left)?;
            e.point(add(left, d, pen.half))?;
            e.point(add(right, d, pen.half))?;
            e.point(right)
        }
        Cap::Round => {
            e.point(left)?;
            // 左から先を通って右へ（半回り。左の縁の向き n から −n へ、d の側を通る：負の向き）
            e.arc(pen, p, n, -core::f64::consts::PI)
        }
    }
}

/// 長さ 0 の部分パスの端（round は円、square は軸に沿う正方形、butt は描かない）。
fn zero_length(e: &mut Emit<'_, '_>, pen: &Pen, p: Point) -> Result<(), Error> {
    match pen.cap {
        Cap::Butt => Ok(()),
        Cap::Square => {
            e.begin()?;
            let h = pen.half;
            e.point(Point::new(p.x - h, p.y - h))?;
            e.point(Point::new(p.x + h, p.y - h))?;
            e.point(Point::new(p.x + h, p.y + h))?;
            e.point(Point::new(p.x - h, p.y + h))
        }
        Cap::Round => {
            e.begin()?;
            let start = Point::new(p.x + pen.half, p.y);
            e.point(start)?;
            e.arc(pen, p, Point::new(1.0, 0.0), 2.0 * core::f64::consts::PI)
        }
    }
}

/// 破線の型（並び・合計・始めの位相と、その位相の並びの番号と残り）。`dashes` は偶数個で、どれも 0 以上、合計は 0 より
/// 大きい（作るときに確かめる）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Dashes<'a> {
    pub(crate) dashes: &'a [f64],
    pub(crate) total: f64,
    /// 部分パスの頭の位相（`stroke-dashoffset` を合計で割った余り。0 以上で合計未満）
    pub(crate) phase: f64,
    /// 頭の位相が入る並びの番号と、その部分の中ですでに進んだ長さ（部分パスごとに求め直さない）
    start: usize,
    start_left: f64,
}

impl<'a> Dashes<'a> {
    /// 並びと位相のずれから作る（偶数個・0 以上・合計 0 より大を確かめ、外れれば None で実線）。並びをなめる分を払う。
    pub(crate) fn new(dashes: &'a [f64], offset: f64, budget: &Budget<'_>) -> Result<Option<Dashes<'a>>, Error> {
        if dashes.is_empty() || !dashes.len().is_multiple_of(2) {
            return Ok(None);
        }
        let mut total = 0.0;
        for &d in dashes {
            budget.spend(cost::OP_STEP)?;
            if d.is_nan() || d < 0.0 || !d.is_finite() {
                return Ok(None);
            }
            total += d;
        }
        if total.is_nan() || total <= 0.0 || !total.is_finite() {
            return Ok(None);
        }
        let phase = math::rem_euclid(offset, total);
        let phase = if phase.is_finite() && phase < total { phase } else { 0.0 };
        // 位相を並びの番号と、その中の残りに（位相がちょうど長さ 0 の部分の頭なら、そこから始める：頭の長さ 0 の線も端を描く）
        let n = dashes.len();
        let mut idx = 0usize;
        let mut left = phase;
        let mut steps = 0usize;
        while let Some(&d) = dashes.get(idx) {
            if left < d || (d == 0.0 && left == 0.0) {
                break;
            }
            left -= d;
            idx = ring_next(idx, n);
            budget.spend(cost::OP_STEP)?;
            steps = steps.saturating_add(1);
            // 丸めで並びを 1 周しても止まらなければ、頭から始める
            if steps > n {
                idx = 0;
                left = 0.0;
                break;
            }
        }
        Ok(Some(Dashes { dashes, total, phase, start: idx, start_left: left }))
    }
}

/// 破線の部分を受け取る口（部分の折れ線と、閉じた線として描くか）。
pub(crate) type PieceFn<'f> = &'f mut dyn FnMut(&[Point], bool) -> Result<(), Error>;

/// 部分パスの折れ線 `pts` を破線に分け、線の部分ごとに `piece`（その部分の折れ線）を呼ぶ。`buf` は部分の点の置き場、
/// `head` は閉じた部分パスの最初の部分を最後の部分につなぐための置き場（resvg・Chrome と同じ。ADR-92 (1)）。閉じた長さ 0 の
/// 部分パスは描かない（Chrome と同じ）。区間の長さが有限でないか、境の足し算が進まなければ（破線の値が区間の長さに比べて
/// 小さすぎる）、その部分パスの残りを分けない。
#[allow(clippy::too_many_arguments, reason = "置き場を分けて渡す")]
pub(crate) fn dash_polyline(
    pts: &[Point],
    closed: bool,
    dashes: &Dashes<'_>,
    buf: &mut crate::mem::MVec<'_, Point>,
    head: &mut crate::mem::MVec<'_, Point>,
    budget: &Budget<'_>,
    piece: PieceFn<'_>,
) -> Result<(), Error> {
    let Some(&first) = pts.first() else { return Ok(()) };
    if closed {
        let mut all_same = true;
        for &p in pts {
            budget.spend(cost::OP_STEP)?;
            if p != first {
                all_same = false;
                break;
            }
        }
        if all_same {
            return Ok(());
        }
    }
    let n = dashes.dashes.len();
    let mut idx = dashes.start;
    let mut remain = dashes.dashes.get(idx).copied().unwrap_or(0.0) - dashes.start_left;
    let mut on = idx.is_multiple_of(2);
    // 閉じた部分パスで頭が線の中なら、最初の部分は最後の部分につなぐ（最初の部分が終わるまで head に貯める）
    let hold_head = closed && on;
    let mut holding = hold_head;
    head.clear();
    buf.clear();
    if on {
        buf.push(first)?;
    }
    let count = if closed { pts.len() } else { pts.len().saturating_sub(1) };
    'segs: for k in 0..count {
        budget.spend(cost::OP_STEP)?;
        let (Some(&a), Some(&b)) = (pts.get(k), pts.get(ring_next(k, pts.len()))) else { continue };
        let seg = seg_len(a, b);
        if !seg.is_finite() {
            break;
        }
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let mut t0 = 0.0;
        // 区間の中の破線の境
        while seg - t0 > remain {
            budget.spend(cost::OUTLINE_POINT)?;
            let next_t = t0 + remain;
            if next_t == t0 && remain > 0.0 {
                // 足しても進まない
                break 'segs;
            }
            t0 = next_t;
            let u = t0 / seg;
            let at = Point::new(a.x + dx * u, a.y + dy * u);
            if on {
                buf.push(at)?;
                if holding {
                    head.append_slice(buf.as_slice())?;
                    holding = false;
                } else {
                    piece(buf.as_slice(), false)?;
                }
                buf.clear();
            } else {
                buf.clear();
                buf.push(at)?;
            }
            on = !on;
            idx = ring_next(idx, n);
            remain = dashes.dashes.get(idx).copied().unwrap_or(0.0);
        }
        remain -= seg - t0;
        if on {
            budget.spend(cost::OUTLINE_POINT)?;
            buf.push(b)?;
        }
    }
    if holding {
        // 最初の部分が終わらなかった（閉じた部分パスの全体が線）：閉じた線として描く（始めの角も継ぎ目。ADR-92 (1)・
        // Chrome と同じ。単位 1 の 2 回目のレビュー）
        if buf.len() > 1 {
            piece(buf.as_slice(), true)?;
        }
        return Ok(());
    }
    if on && buf.len() > 1 {
        if hold_head && !head.is_empty() {
            // 最後の部分（始めの点で終わる）に最初の部分をつなぐ（始めの点を 2 度入れない）
            for &p in head.as_slice().iter().skip(1) {
                budget.spend(cost::OP_STEP)?;
                buf.push(p)?;
            }
            head.clear();
        }
        piece(buf.as_slice(), false)?;
    }
    if !head.is_empty() {
        piece(head.as_slice(), false)?;
    }
    Ok(())
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
