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
use crate::mem::MVec;

/// 点の印：角・曲線の内側の点（平坦化で分けた点。前後の弦が同じ曲線のもの）・接線が続くつなぎ目（区間の境で、曲線の前か
/// 後ろ。ADR-99 (2)）。内側の点とつなぎ目はどちらも滑らかな点（継ぎ目を作らない）。端の向きを曲線の弦から外挿するのは
/// 内側の点だけ（つなぎ目の向こうの弦は別の区間）。
pub(crate) const CORNER: u8 = 0;
pub(crate) const INTERIOR: u8 = 1;
pub(crate) const JOINT: u8 = 2;

/// 折れ線（点と、点の印（[`CORNER`]・[`INTERIOR`]・[`JOINT`]））。印の並びが短ければ、足りない点は角。
/// 始めと終わりの向き（`start_dir`・`end_dir`）があれば、端はその向きで作る（曲線で始まる・終わる線の端は、平坦化した弦でなく
/// 接線の向き。resvg・Chrome と同じ。ADR-101）。
/// 破線の部分が曲線の最後の弦の中で始まる（最初の弦の中で終わる）と、2 つ目の点（終わりから 2 つ目の点）の角の曲線の側は
/// 部分の中の弦から外挿できない。`first_behind`・`last_ahead` は、その角から曲線にそって後ろへ（前へ）出る向きを元の折れ線で
/// 求めたもの（開いた折れ線だけで使う。単位 1 の 7 回目のレビュー）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Poly<'a> {
    pub(crate) pts: &'a [Point],
    pub(crate) smooth: &'a [u8],
    pub(crate) start_dir: Option<Point>,
    pub(crate) end_dir: Option<Point>,
    pub(crate) first_behind: Option<Point>,
    pub(crate) last_ahead: Option<Point>,
}

impl<'a> Poly<'a> {
    /// どの点も角の折れ線。
    pub(crate) fn corners(pts: &'a [Point]) -> Poly<'a> {
        Poly { pts, smooth: &[], start_dir: None, end_dir: None, first_behind: None, last_ahead: None }
    }

    fn smooth_at(&self, i: usize) -> bool {
        self.smooth.get(i).is_some_and(|&s| s != CORNER)
    }

    /// 点の印（足りなければ角）。
    pub(crate) fn flag_at(&self, i: usize) -> u8 {
        self.smooth.get(i).copied().unwrap_or(CORNER)
    }
}

/// 折れ線の置き場（点と印を並べて持つ）。
pub(crate) struct PolyBuf<'b> {
    pub(crate) pts: MVec<'b, Point>,
    pub(crate) smooth: MVec<'b, u8>,
    pub(crate) start_dir: Option<Point>,
    pub(crate) end_dir: Option<Point>,
    pub(crate) first_behind: Option<Point>,
    pub(crate) last_ahead: Option<Point>,
}

impl<'b> PolyBuf<'b> {
    pub(crate) fn new(budget: &'b Budget<'b>) -> PolyBuf<'b> {
        PolyBuf {
            pts: MVec::new(budget),
            smooth: MVec::new(budget),
            start_dir: None,
            end_dir: None,
            first_behind: None,
            last_ahead: None,
        }
    }

    pub(crate) fn with_capacity(budget: &'b Budget<'b>, n: usize) -> Result<PolyBuf<'b>, Error> {
        Ok(PolyBuf {
            pts: MVec::with_capacity(budget, n)?,
            smooth: MVec::with_capacity(budget, n)?,
            start_dir: None,
            end_dir: None,
            first_behind: None,
            last_ahead: None,
        })
    }

    pub(crate) fn clear(&mut self) {
        self.pts.clear();
        self.smooth.clear();
        self.start_dir = None;
        self.end_dir = None;
        self.first_behind = None;
        self.last_ahead = None;
    }

    /// 最後の点を除く（閉じた部分パスで、終わりの点が始めの点と同じとき）。
    pub(crate) fn pop(&mut self) {
        self.pts.pop();
        self.smooth.pop();
    }

    /// 点を足す（`interior` なら曲線の内側の点、でなければ角）。
    pub(crate) fn push(&mut self, p: Point, interior: bool) -> Result<(), Error> {
        self.push_flag(p, if interior { INTERIOR } else { CORNER })
    }

    /// 点を印つきで足す（別の折れ線の印を写すとき）。
    pub(crate) fn push_flag(&mut self, p: Point, flag: u8) -> Result<(), Error> {
        self.pts.push(p)?;
        self.smooth.push(flag)
    }

    pub(crate) fn len(&self) -> usize {
        self.pts.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pts.is_empty()
    }

    fn truncate(&mut self, n: usize) {
        self.pts.truncate(n);
        self.smooth.truncate(n);
    }

    /// 最後の点を接線が続くつなぎ目（`joint`）か角にする（曲線の終わりの点は角）。
    pub(crate) fn set_last_smooth(&mut self, joint: bool) {
        if let Some(s) = self.smooth.last_mut() {
            *s = if joint { JOINT } else { CORNER };
        }
    }

    /// 始めの点を接線が続くつなぎ目（`joint`）か角にする（閉じた部分パスの始めのつなぎ目）。
    pub(crate) fn set_first_smooth(&mut self, joint: bool) {
        if let Some(s) = self.smooth.get_mut(0) {
            *s = if joint { JOINT } else { CORNER };
        }
    }

    pub(crate) fn view(&self) -> Poly<'_> {
        Poly {
            pts: self.pts.as_slice(),
            smooth: self.smooth.as_slice(),
            start_dir: self.start_dir,
            end_dir: self.end_dir,
            first_behind: self.first_behind,
            last_ahead: self.last_ahead,
        }
    }

    /// 別の折れ線の `skip` 番目からを後ろに足す（点ごとに払う）。
    fn append_from(&mut self, other: Poly<'_>, skip: usize, budget: &Budget<'_>) -> Result<(), Error> {
        for (i, &p) in other.pts.iter().enumerate().skip(skip) {
            budget.spend(cost::OP_STEP)?;
            self.push_flag(p, other.flag_at(i))?;
        }
        Ok(())
    }
}

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

/// 曲線で始まる（終わる）折れ線の端の向き：始めの 2 本の弦の向きから外挿する（d0 を d1 との角の半分だけ戻す：1.5 d0 −
/// 0.5 d1）。弧のように滑らかな曲線では接線とほぼ一致し、端の制御点が端点にごく近い曲線（線として書いた曲線）では曲線の
/// おおよその向きになる（resvg・Chrome と同じ。本当の接線は使わない。ADR-101）。2 つ目の点が曲線の内側の点（[`INTERIOR`]）
/// でなければ None（接線が続くつなぎ目でも、端の区間が直線なら直線の向き）。
fn curve_end_dir(line: Poly<'_>, at_start: bool) -> Option<Point> {
    let n = line.pts.len();
    if at_start {
        curve_side(line, 0, true, false)
    } else {
        // 終わりの向きは終わりの点へ向かう向き
        curve_side(line, n.checked_sub(1)?, false, false).map(neg)
    }
}

/// 点 `i` から曲線にそって出ていく向き（`ahead` なら次の点の側へ、でなければ前の点の側へ向かう向き）。隣の点が曲線の内側の
/// 点（[`INTERIOR`]）で、その先の点もあるときだけ、2 本の弦（どちらも同じ曲線のもの）から外挿する（1.5 d0 − 0.5 d1。
/// ADR-101 (7)(c)）。隣の点が角かつなぎ目なら、その側の区間は直線（か 1 本の弦の曲線）なので None。
fn curve_side(line: Poly<'_>, i: usize, ahead: bool, closed: bool) -> Option<Point> {
    let n = line.pts.len();
    let step = |k: usize| -> Option<usize> {
        match (ahead, closed) {
            (true, true) => Some(ring_next(k, n)),
            (false, true) => Some(ring_prev(k, n)),
            (true, false) => k.checked_add(1).filter(|&j| j < n),
            (false, false) => k.checked_sub(1),
        }
    };
    let j = step(i)?;
    if j == i || line.flag_at(j) != INTERIOR {
        return None;
    }
    let k = step(j)?;
    if k == i || k == j {
        return None;
    }
    let (a, b, c) = (line.pts.get(i)?, line.pts.get(j)?, line.pts.get(k)?);
    let (d0, d1) = (direction(*a, *b)?, direction(*b, *c)?);
    direction(Point::new(0.0, 0.0), Point::new(1.5 * d0.x - 0.5 * d1.x, 1.5 * d0.y - 0.5 * d1.y))
}

/// 角の点 `i` の、曲線の側の向き（入る向き・出る向き。`forward` でなければ後ろへ回る向きで）。角でなければ両方 None。
/// 曲線の側を求めるたびに `cost::CURVE_SIDE` を払う（見積もりは曲線ごとに 8 回まで）。
fn corner_tangents(
    line: Poly<'_>,
    i: usize,
    closed: bool,
    forward: bool,
    budget: &Budget<'_>,
) -> Result<(Option<Point>, Option<Point>), Error> {
    if line.flag_at(i) != CORNER {
        return Ok((None, None));
    }
    let side = |ahead: bool| -> Result<Option<Point>, Error> {
        let n = line.pts.len();
        // 破線の部分の 2 つ目と終わりから 2 つ目の角：部分の外の弦から求めた向き（元の折れ線で求めて払ったもの）
        if !closed {
            if !ahead && i == 1 && line.first_behind.is_some() {
                return Ok(line.first_behind);
            }
            if ahead && i.checked_add(2) == Some(n) && line.last_ahead.is_some() {
                return Ok(line.last_ahead);
            }
        }
        let j = if ahead { ring_next(i, n) } else { ring_prev(i, n) };
        if line.flag_at(j) != INTERIOR {
            return Ok(None);
        }
        budget.spend(cost::CURVE_SIDE)?;
        Ok(curve_side(line, i, ahead, closed))
    };
    let (ahead, behind) = (side(true)?, side(false)?);
    Ok(if forward { (behind.map(neg), ahead) } else { (ahead.map(neg), behind) })
}

/// 接線が続くつなぎ目（[`JOINT`]）の点 `i` の向き：片側が直線ならその向き、両側が曲線なら両側の外挿の平均（ADR-101 (7)(d)）。
fn joint_dir(line: Poly<'_>, i: usize, closed: bool) -> Option<Point> {
    let n = line.pts.len();
    let (prev, next) = (ring_prev(i, n), ring_next(i, n));
    match (curve_side(line, i, false, closed), curve_side(line, i, true, closed)) {
        (None, _) => direction(*line.pts.get(prev)?, *line.pts.get(i)?),
        (_, None) => direction(*line.pts.get(i)?, *line.pts.get(next)?),
        (Some(a), Some(b)) => direction(Point::new(0.0, 0.0), Point::new(b.x - a.x, b.y - a.y)),
    }
}

/// 逆の向き。
fn neg(d: Point) -> Point {
    Point::new(-d.x, -d.y)
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
    /// round の継ぎ目の円弧で、頂点ごとの 1 つを超えて使ってよい点の残り（節の全体で。呼び手が見積もりと同じ式で決める）
    round_left: &'s mut u64,
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
        self.arc_n(pen, c, from, sweep, pen.round_pieces(sweep))
    }

    /// [`Emit::arc`] を `n` 個に分けて。
    fn arc_n(&mut self, pen: &Pen, c: Point, from: Point, sweep: f64, n: u64) -> Result<(), Error> {
        let a0 = math::atan2(from.y, from.x);
        let step = chunk_len(cost::OUTLINE_POINT) as u64;
        let nf = n as f64;
        // 終わった点の数（n が u64::MAX でも終わる。単位 1 の 4 回目のレビュー）
        let mut done = 0u64;
        while done < n {
            let k = step.min(n.saturating_sub(done));
            self.budget.spend(k.saturating_mul(cost::OUTLINE_POINT))?;
            for j in 1..=k {
                let i = done.saturating_add(j);
                let a = a0 + sweep * ((i as f64) / nf);
                let p = Point::new(c.x + pen.half * math::cos(a), c.y + pen.half * math::sin(a));
                self.sink.point(p)?;
            }
            done = done.saturating_add(k);
        }
        Ok(())
    }
}

/// 続く同じ点をまとめた折れ線を `out` に入れ直す（`out` は呼び手の置き場。まとめた点は、どれかが角なら角、内側の点と
/// つなぎ目が混じればつなぎ目）。
fn dedup(line: Poly<'_>, out: &mut PolyBuf<'_>, budget: &Budget<'_>) -> Result<(), Error> {
    out.clear();
    for (i, &p) in line.pts.iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        let flag = line.flag_at(i);
        if out.pts.last() == Some(&p) {
            // まとめた点は、どれかが角なら角、内側の点とつなぎ目が混じればつなぎ目
            if let Some(s) = out.smooth.last_mut()
                && *s != flag
            {
                *s = if *s == CORNER || flag == CORNER { CORNER } else { JOINT };
            }
        } else {
            out.push_flag(p, flag)?;
        }
    }
    Ok(())
}

/// 1 本の部分パスの折れ線 `pts`（`closed` なら閉じる）の外形の輪を `sink` に渡す。`scratch` は点をまとめる置き場。
pub(crate) fn stroke_polyline(
    line: Poly<'_>,
    closed: bool,
    pen: &Pen,
    scratch: &mut PolyBuf<'_>,
    budget: &Budget<'_>,
    round_left: &mut u64,
    sink: &mut dyn LoopSink,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    let pts = line.pts;
    if pts.is_empty() || (pts.len() == 1 && !closed) {
        return Ok(());
    }
    dedup(line, scratch, budget)?;
    // 閉じた折れ線の終わりが始めと同じなら 1 つにする（始めの点の印は残す）
    if closed && scratch.len() > 1 && scratch.pts.last() == scratch.pts.as_slice().first() {
        let n = scratch.len().saturating_sub(1);
        scratch.truncate(n);
    }
    scratch.start_dir = line.start_dir;
    scratch.end_dir = line.end_dir;
    // 部分の外から求めた角の向きは、その角が 2 つ目（終わりから 2 つ目）の点のまま残るときだけ使う（端の点とまとまれば捨てる）
    let m = pts.len();
    scratch.first_behind = line.first_behind.filter(|_| !closed && pts.first() != pts.get(1));
    scratch.last_ahead =
        line.last_ahead.filter(|_| !closed && m >= 2 && pts.get(m.saturating_sub(2)) != pts.get(m.saturating_sub(1)));
    let view = scratch.view();
    let v = view.pts;
    let mut e = Emit { sink, budget, round_left };
    if v.len() == 1 {
        return zero_length(&mut e, pen, v.first().copied().unwrap_or(Point::new(0.0, 0.0)));
    }
    if closed {
        // 左の縁の輪（前へ）と右の縁の輪（後ろへ）。点が 2 つだけなら前への輪だけ（両端の折り返しの継ぎ目が入った 1 周で
        // 線の全体を覆う。後ろへの輪は同じ所をもう 1 度覆い、面積の被覆が縁の画素を濃くする。単位 1 の 4 回目のレビュー）
        let n = v.len();
        let loops: &[bool] = if n == 2 { &[true] } else { &[true, false] };
        for &forward in loops {
            e.begin()?;
            for k in 0..n {
                let i = if forward { k } else { n.saturating_sub(1).saturating_sub(k) };
                let (back, ahead) = (ring_prev(i, n), ring_next(i, n));
                let (a, b) = if forward { (back, ahead) } else { (ahead, back) };
                let (Some(&p), Some(&a), Some(&b)) = (v.get(i), v.get(a), v.get(b)) else { continue };
                let tan = corner_tangents(view, i, true, forward, budget)?;
                vertex(&mut e, pen, a, p, b, forward, view.smooth_at(i), tan)?;
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
    let unit = |d: Option<Point>| d.and_then(|d| direction(Point::new(0.0, 0.0), d));
    let d0 = unit(view.start_dir)
        .or_else(|| curve_end_dir(view, true))
        .or_else(|| direction(first, second))
        .unwrap_or(Point::new(1.0, 0.0));
    e.point(add(first, normal(d0), pen.half))?;
    for i in 1..n.saturating_sub(1) {
        let (Some(&a), Some(&p), Some(&b)) = (v.get(ring_prev(i, n)), v.get(i), v.get(ring_next(i, n))) else {
            continue;
        };
        let tan = corner_tangents(view, i, false, true, budget)?;
        vertex(&mut e, pen, a, p, b, true, view.smooth_at(i), tan)?;
    }
    let dn = unit(view.end_dir)
        .or_else(|| curve_end_dir(view, false))
        .or_else(|| direction(before_last, last))
        .unwrap_or(d0);
    cap(&mut e, pen, last, dn)?;
    for i in (1..n.saturating_sub(1)).rev() {
        let (Some(&a), Some(&p), Some(&b)) = (v.get(ring_next(i, n)), v.get(i), v.get(ring_prev(i, n))) else {
            continue;
        };
        let tan = corner_tangents(view, i, false, false, budget)?;
        vertex(&mut e, pen, a, p, b, false, view.smooth_at(i), tan)?;
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
#[allow(clippy::too_many_arguments, reason = "頂点の前後の点と、曲線の側の向きを分けて渡す")]
fn vertex(
    e: &mut Emit<'_, '_>,
    pen: &Pen,
    a: Point,
    p: Point,
    b: Point,
    forward: bool,
    smooth: bool,
    tan: (Option<Point>, Option<Point>),
) -> Result<(), Error> {
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
    if cross > 0.0 && smooth {
        // 曲線の内側の点で交わる点が区間に収まらない（曲がりの半径が太さの半分より小さい）：頂点を通さずに縁の点どうしを
        // つなぐ（縁は曲線を写したものになり、折り返した所の巻き数で中に穴が残る。Chrome・resvg（Skia）は曲線の縁を曲線の
        // まま写すので同じ。ADR-99 (2)）
        return e.point(start_out);
    }
    if cross > 0.0 || (cross == 0.0 && dot < 0.0 && !forward) {
        // 左は内側で交わる点が区間に収まらない（鋭い折れ・短い区間）：頂点を通す
        e.point(p)?;
        return e.point(start_out);
    }
    if cross == 0.0 && dot >= 0.0 {
        // まっすぐ：何も足さない
        return Ok(());
    }
    // 滑らかな点の外側は、継ぎ目の形に依らず円弧（縁は曲線を写したもの。尖りに半径 h の丸が付く。resvg・Chrome と同じ。
    // ADR-101）
    let join = if smooth { Join::Round } else { pen.join };
    // 曲線に接する角の外側の継ぎ目は、曲線の側を弦でなく外挿した向きで作る（miter の先・miterlimit の判定・bevel の向きを
    // Chrome・resvg にそろえる。ADR-101 (7)(f)）。縁は弦の法線の点 → 向きの法線の点 → 継ぎ目 → 向きの法線の点 → 弦の法線の点と
    // つなぐ（間は頂点から太さの半分の円の内なので線の中）。向きの曲がる側が弦と逆なら弦のまま
    let (m1, m2, jc, jd) = match tan {
        (t_in, t_out) if !smooth && (t_in.is_some() || t_out.is_some()) => {
            let (t1, t2) = (t_in.unwrap_or(d1), t_out.unwrap_or(d2));
            let tc = t1.x * t2.y - t1.y * t2.x;
            if tc < 0.0 { (normal(t1), normal(t2), tc, t1.x * t2.x + t1.y * t2.y) } else { (n1, n2, cross, dot) }
        }
        _ => (n1, n2, cross, dot),
    };
    if m1 != n1 {
        e.point(add(p, m1, pen.half))?;
    }
    match join {
        Join::Bevel => {}
        Join::Round => {
            // 外側の円弧（曲がる角だけ。左の縁が外なので負の向き。折り返し（外積 0・内積が負）は半回り）
            let turn = if jc == 0.0 { -core::f64::consts::PI } else { math::atan2(jc, jd) };
            // 頂点ごとに 1 つと、節の残りの分まで（丸めで乱れた折れ線でも見積もりの式を超えない。単位 1 の 4 回目のレビュー）
            let n = pen.round_pieces(turn).min(e.round_left.saturating_add(1));
            *e.round_left = e.round_left.saturating_sub(n.saturating_sub(1));
            e.arc_n(pen, p, m1, turn, n)?;
            // 円弧は向きの法線の点で終わる
            return if m2 != n2 { e.point(start_out) } else { Ok(()) };
        }
        Join::Miter => {
            // 比の 2 乗の逆数 = (1 + cos φ) / 2 ≥ 1 / limit²
            let limit = pen.miter_limit;
            if limit >= 1.0 && (1.0 + jd) * limit * limit >= 2.0 {
                let k = pen.half / (1.0 + jd);
                e.point(Point::new(p.x + (m1.x + m2.x) * k, p.y + (m1.y + m2.y) * k))?;
            }
        }
    }
    if m2 != n2 {
        e.point(add(p, m2, pen.half))?;
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
    /// 線の部分の和が正か（境の足し算が進まないとき、残りを線にするか。作るときに 1 度だけ求める）
    on_positive: bool,
}

impl<'a> Dashes<'a> {
    /// 並びと位相のずれから作る（偶数個・0 以上・合計 0 より大を確かめ、外れれば None で実線）。並びをなめる分を払う。
    pub(crate) fn new(dashes: &'a [f64], offset: f64, budget: &Budget<'_>) -> Result<Option<Dashes<'a>>, Error> {
        if dashes.is_empty() || !dashes.len().is_multiple_of(2) {
            return Ok(None);
        }
        let mut total = 0.0;
        let mut on_positive = false;
        for (i, &d) in dashes.iter().enumerate() {
            budget.spend(cost::OP_STEP)?;
            if d.is_nan() || d < 0.0 || !d.is_finite() {
                return Ok(None);
            }
            total += d;
            on_positive |= i.is_multiple_of(2) && d > 0.0;
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
        Ok(Some(Dashes { dashes, total, phase, start: idx, start_left: left, on_positive }))
    }
}

/// 破線の部分を受け取る口（部分の折れ線と、閉じた線として描くか）。
pub(crate) type PieceFn<'f> = &'f mut dyn FnMut(Poly<'_>, bool) -> Result<(), Error>;

/// 区間 a → b のうち見える所（区間の割合の範囲。見えなければ None）を返す口。見えない所の破線は作らないので、境を周期ごとに
/// 算術で飛ばす（ADR-30 (4)。単位 1 の 4 回目のレビュー）。
pub(crate) type VisibleFn<'f> = &'f dyn Fn(Point, Point) -> Option<(f64, f64)>;

/// 部分パスの折れ線 `pts` を破線に分け、線の部分ごとに `piece`（その部分の折れ線）を呼ぶ。`buf` は部分の点の置き場、
/// `head` は閉じた部分パスの最初の部分を最後の部分につなぐための置き場（resvg・Chrome と同じ。ADR-92 (1)）。閉じた長さ 0 の
/// 部分パスは描かない（Chrome と同じ）。区間の長さが有限でないか、境の足し算が進まなければ（破線の値が区間の長さに比べて
/// 小さすぎる）、その部分パスの残りを分けない。
#[allow(clippy::too_many_arguments, reason = "置き場を分けて渡す")]
pub(crate) fn dash_polyline(
    line: Poly<'_>,
    closed: bool,
    dashes: &Dashes<'_>,
    buf: &mut PolyBuf<'_>,
    head: &mut PolyBuf<'_>,
    budget: &Budget<'_>,
    visible: Option<VisibleFn<'_>>,
    piece: PieceFn<'_>,
) -> Result<(), Error> {
    let pts = line.pts;
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
    let np = pts.len();
    let count = if closed { np } else { np.saturating_sub(1) };
    // 破線の部分の端の向き：区間 k の割合 u の所の向きを、両端の頂点の向き（滑らかな点は前後の弦の二等分の向き、開いた
    // 線の始めと終わりは接線）から内挿する（弦の向きのままだと、曲線の上の端が弦の角の半分まで傾く。ADR-101）
    let seg_dir = |k: usize| -> Option<Point> {
        let (a, b) = (pts.get(k)?, pts.get(ring_next(k, np))?);
        direction(*a, *b)
    };
    let tangent = |k: usize, u: f64| -> Result<Option<Point>, Error> {
        budget.spend(cost::DASH_TANGENT)?;
        let Some(dk) = seg_dir(k) else { return Ok(None) };
        let unit = |d: Option<Point>| d.and_then(|d| direction(Point::new(0.0, 0.0), d));
        let bisect = |a: Option<Point>, b: Point| {
            a.and_then(|a| direction(Point::new(0.0, 0.0), Point::new(a.x + b.x, a.y + b.y))).unwrap_or(b)
        };
        // 頂点の向き：曲線の内側の点は前後の弦の二等分、つなぎ目は片側の直線か両側の外挿の平均、曲線に接する角は曲線の
        // 側の外挿（ADR-101 (7)(d)）
        let ta = if k == 0 && !closed {
            unit(line.start_dir).or_else(|| curve_end_dir(line, true)).unwrap_or(dk)
        } else {
            match line.flag_at(k) {
                INTERIOR => bisect(seg_dir(ring_prev(k, np)), dk),
                JOINT => joint_dir(line, k, closed).unwrap_or(dk),
                _ => curve_side(line, k, true, closed).unwrap_or(dk),
            }
        };
        let kb = ring_next(k, np);
        let tb = if !closed && kb.saturating_add(1) >= np {
            unit(line.end_dir).or_else(|| curve_end_dir(line, false)).unwrap_or(dk)
        } else {
            match line.flag_at(kb) {
                INTERIOR => seg_dir(kb).map_or(dk, |dn| bisect(Some(dk), dn)),
                JOINT => joint_dir(line, kb, closed).unwrap_or(dk),
                _ => curve_side(line, kb, false, closed).map_or(dk, neg),
            }
        };
        Ok(unit(Some(Point::new(ta.x * (1.0 - u) + tb.x * u, ta.y * (1.0 - u) + tb.y * u))).or(Some(dk)))
    };
    // 部分が区間 k の中で始まる（終わる）とき、区間の終わり（頭）の角から曲線にそって後ろへ（前へ）出る向き（区間の頭
    // （終わり）が曲線の内側の点のときだけ外挿する。単位 1 の 7 回目のレビュー）
    let behind_of = |k: usize| -> Result<Option<Point>, Error> {
        let kb = ring_next(k, np);
        if line.flag_at(kb) != CORNER || line.flag_at(k) != INTERIOR {
            return Ok(None);
        }
        budget.spend(cost::CURVE_SIDE)?;
        Ok(curve_side(line, kb, false, closed))
    };
    let ahead_of = |k: usize| -> Result<Option<Point>, Error> {
        let kb = ring_next(k, np);
        if line.flag_at(k) != CORNER || line.flag_at(kb) != INTERIOR {
            return Ok(None);
        }
        budget.spend(cost::CURVE_SIDE)?;
        Ok(curve_side(line, k, true, closed))
    };
    if on {
        buf.push_flag(first, line.flag_at(0))?;
        // 開いた線の始めも、曲線で始まれば (c) の向き（最初の部分が最初の弦の中で終わっても。単位 1 の 6 回目のレビュー）
        buf.start_dir = tangent(0, 0.0)?;
    }
    // 境の足し算が進まない（今の位置に比べて値が小さすぎる）ときは長さ 0 の部分と同じに切り替えて続け、並びを 1 周しても
    // 進まなければ、部分パスの残りを線の部分の和が正なら線、0 なら隙間として扱う（残りを捨てない。Skia は破線が多すぎると
    // 実線で描く。単位 1 の 3 回目のレビュー。ADR-97 (5)）
    let mut stall = 0usize;
    let on_sum_positive = dashes.on_positive;
    for k in 0..count {
        budget.spend(cost::OP_STEP)?;
        let (Some(&a), Some(&b)) = (pts.get(k), pts.get(ring_next(k, pts.len()))) else { continue };
        let seg = seg_len(a, b);
        if !seg.is_finite() {
            break;
        }
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        // 区間の見える所（長さで。見えない区間は全体を見えない所として飛ばす）
        let (vis_lo, vis_hi) = match visible {
            None => (0.0, seg),
            Some(f) => match f(a, b) {
                Some((u0, u1)) => (u0 * seg, u1 * seg),
                None => (seg, seg),
            },
        };
        let mut t0 = 0.0;
        // 区間の頭は前の区間の続き（破線の中ほど）。境に来てから飛ばす
        let mut at_boundary = false;
        // 区間の中の破線の境
        while seg - t0 > remain {
            // 見えない所は、境の上（並びの番号・線か隙間・残りが周期で変わらない）から周期ごとに飛ばす。前後に 2 周期の
            // 余白を残し、閉じた部分パスの頭を貯めている間は飛ばさない
            if at_boundary && !holding {
                let target = if t0 < vis_lo {
                    vis_lo
                } else if t0 > vis_hi {
                    seg
                } else {
                    t0
                };
                let periods = ((target - t0) / dashes.total).floor() - 2.0;
                let skipped = t0 + periods * dashes.total;
                if periods >= 1.0 && skipped > t0 {
                    budget.spend(cost::OP_STEP)?;
                    t0 = skipped;
                    stall = 0;
                    if on {
                        let u = t0 / seg;
                        buf.clear();
                        buf.push(Point::new(a.x + dx * u, a.y + dy * u), false)?;
                        buf.start_dir = tangent(k, u)?;
                        buf.first_behind = behind_of(k)?;
                    }
                    if seg - t0 <= remain {
                        break;
                    }
                }
            }
            budget.spend(cost::OUTLINE_POINT)?;
            at_boundary = true;
            let next_t = t0 + remain;
            if next_t > t0 {
                stall = 0;
            } else {
                stall = stall.saturating_add(1);
                if stall > n.saturating_add(1) {
                    // 1 周しても進まない：残りは線か隙間のどちらか 1 つ
                    let u = t0 / seg;
                    let at = Point::new(a.x + dx * u, a.y + dy * u);
                    if on_sum_positive && !on {
                        buf.clear();
                        buf.push(at, false)?;
                        buf.start_dir = tangent(k, u)?;
                        buf.first_behind = behind_of(k)?;
                    } else if !on_sum_positive {
                        buf.clear();
                    }
                    on = on_sum_positive;
                    remain = f64::INFINITY;
                    break;
                }
            }
            t0 = next_t;
            let u = t0 / seg;
            let at = Point::new(a.x + dx * u, a.y + dy * u);
            if on {
                buf.push(at, false)?;
                // 区間の頭（角）で終わる部分は、前の区間の向きで終わる
                buf.end_dir =
                    if u == 0.0 && (k > 0 || closed) { tangent(ring_prev(k, np), 1.0)? } else { tangent(k, u)? };
                if u > 0.0 {
                    buf.last_ahead = ahead_of(k)?;
                }
                if holding {
                    head.append_from(buf.view(), 0, budget)?;
                    head.start_dir = buf.start_dir;
                    head.end_dir = buf.end_dir;
                    head.first_behind = buf.first_behind;
                    head.last_ahead = buf.last_ahead;
                    holding = false;
                } else {
                    piece(buf.view(), false)?;
                }
                buf.clear();
            } else {
                buf.clear();
                buf.push(at, false)?;
                buf.start_dir = tangent(k, u)?;
                buf.first_behind = behind_of(k)?;
            }
            on = !on;
            idx = ring_next(idx, n);
            remain = dashes.dashes.get(idx).copied().unwrap_or(0.0);
        }
        // 区間の残りを進んだら、進まない境の数えを戻す（隙間 0 の破線で境が角ごとに 2 つずつ進まずに続いても、区間を
        // またいで溜めない）
        if seg > t0 {
            stall = 0;
        }
        remain -= seg - t0;
        if on {
            budget.spend(cost::OUTLINE_POINT)?;
            buf.push_flag(b, line.flag_at(ring_next(k, pts.len())))?;
        }
    }
    if holding {
        // 最初の部分が終わらなかった（閉じた部分パスの全体が線）：閉じた線として描く（始めの角も継ぎ目。ADR-92 (1)・
        // Chrome と同じ。単位 1 の 2 回目のレビュー）
        if buf.len() > 1 {
            piece(buf.view(), true)?;
        }
        return Ok(());
    }
    if on && buf.len() > 1 {
        buf.end_dir = if closed { tangent(np.saturating_sub(1), 1.0)? } else { tangent(np.saturating_sub(2), 1.0)? };
        // 最後の部分が終わりの点でちょうど終わる（残りが 0）なら、最初の部分につながない（境が角に当たるほかの所と同じく
        // 継ぎ目を作らない。Chrome と同じ。resvg はつなぐ。tests/painting/stroke-dasharray/n-0）
        if hold_head && !head.is_empty() && remain > 0.0 {
            // 最後の部分（始めの点で終わる）に最初の部分をつなぐ（始めの点を 2 度入れない）
            buf.append_from(head.view(), 1, budget)?;
            buf.end_dir = head.end_dir;
            buf.last_ahead = head.last_ahead;
            head.clear();
        }
        piece(buf.view(), false)?;
    }
    if !head.is_empty() {
        piece(head.view(), false)?;
    }
    Ok(())
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
