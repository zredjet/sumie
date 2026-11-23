//! 平坦化（architecture.md §6.1・ADR-30・ADR-38）。
//!
//! - 曲線を分ける数は、描く座標系に写した制御点（弧は楕円の行列）と許容の誤差 `tol`（画素）だけから、先に閉じた式で
//!   決める（適応的な分割をしない。切る矩形を見ない）。点は t（弧は角）を均等に分けた位置で求める
//!   - 2 次曲線：2 階の導関数は一定で `2(P0 − 2P1 + P2)`。区間の長さ h の弦の誤差は `|B''| h² / 8` 以下なので、
//!     `n = ⌈√(|P0 − 2P1 + P2| / (4 tol))⌉`
//!   - 3 次曲線：`|B''(t)| ≤ 6 max(|P0 − 2P1 + P2|, |P1 − 2P2 + P3|)` なので、`n = ⌈√(3M / (4 tol))⌉`
//!   - 弧：`|P''(θ)| ≤ r`（行列の最大の特異値）なので、`n = ⌈|Δθ| √(r / (8 tol))⌉`
//! - 制御点（弧は楕円の外接矩形）がすべて決まった箱（±2²² 画素）の外の同じ側にある曲線は分けない（1 本の線にする。箱の
//!   外の線は固定小数に直す前に切る）。巨大な図形でも点の数が増えない
//! - 点はユーザー単位で求める（線の外形と破線をユーザー単位で作るため）。最後の点は書いたとおりの終わりの点

use super::{PathSeg, Point, Transform, cubic_at, quad_at, transform_seg};
use crate::budget::{Budget, chunk_len};
use crate::cost;
use crate::error::Error;
use crate::math;

/// 決まった箱の半分の幅（画素）。キャンバスにも範囲にも依らない（ADR-30 (2)）。
pub(crate) const BOX: f64 = 4_194_304.0;

/// 浮動小数の数を分ける数にする（有限でないか大きすぎれば `u64::MAX`、1 未満は 1）。
fn to_count(x: f64) -> u64 {
    if x.is_nan() || x >= 1.8e19 {
        return u64::MAX;
    }
    let n = x.ceil();
    if n < 1.0 { 1 } else { n as u64 }
}

/// 点の向きの大きさ。
fn norm(x: f64, y: f64) -> f64 {
    math::hypot(x, y)
}

/// 点がすべて、箱を `reach` だけ広げた矩形の外の同じ側にあるか（線は外形が中心の線から `reach` まで届くので、その分を
/// 広げて見る。塗りは 0。単位の 1 回目のレビュー）。
fn outside_one_side(pts: &[Point], reach: f64) -> bool {
    let b = BOX + reach;
    let all = |f: &dyn Fn(&Point) -> bool| pts.iter().all(f);
    all(&|p| p.x < -b) || all(&|p| p.x > b) || all(&|p| p.y < -b) || all(&|p| p.y > b)
}

/// 描く座標系の区間を分ける数（1 以上）。線・閉じ・始まりは 1。`reach` は外形が届く距離（画素。塗りは 0）。
pub(crate) fn pieces_device(seg: &PathSeg, tol: f64, reach: f64) -> u64 {
    match *seg {
        PathSeg::Move(_) | PathSeg::Line(..) | PathSeg::Close(..) => 1,
        PathSeg::Quad(p0, p1, p2) => {
            if outside_one_side(&[p0, p1, p2], reach) {
                return 1;
            }
            let m = norm(p0.x - 2.0 * p1.x + p2.x, p0.y - 2.0 * p1.y + p2.y);
            to_count((m / (4.0 * tol)).sqrt())
        }
        PathSeg::Cubic(p0, p1, p2, p3) => {
            if outside_one_side(&[p0, p1, p2, p3], reach) {
                return 1;
            }
            let a = norm(p0.x - 2.0 * p1.x + p2.x, p0.y - 2.0 * p1.y + p2.y);
            let b = norm(p1.x - 2.0 * p2.x + p3.x, p1.y - 2.0 * p2.y + p3.y);
            to_count((3.0 * math::max(a, b) / (4.0 * tol)).sqrt())
        }
        PathSeg::ArcTo(p, a) => {
            let [m0, m1, m2, m3] = a.m;
            let ex = norm(m0, m1);
            let ey = norm(m2, m3);
            let corners = [Point::new(a.c.x - ex, a.c.y - ey), Point::new(a.c.x + ex, a.c.y + ey), p, a.end];
            if outside_one_side(&corners, reach) {
                return 1;
            }
            let r = a.max_radius();
            to_count(a.dtheta.abs() * (r / (8.0 * tol)).sqrt())
        }
    }
}

/// ユーザー単位の区間を、変形 `t` の下で分ける数。
pub(crate) fn pieces(seg: &PathSeg, t: &Transform, tol: f64, reach: f64) -> u64 {
    pieces_device(&transform_seg(seg, t), tol, reach)
}

/// 区間を平坦化した点（ユーザー単位）を、始めの点を除いて順に `out` に渡す（最後は終わりの点）。分ける数は描く座標系で
/// 決める（[`pieces`]）。曲線は分ける数を求める決まった手間（[`cost::FLATTEN_CURVE`]）を払い、分ける数の分の手間を塊ごとに
/// 前払いする。`Move`・`Close` は何も渡さない（呼び手が扱う）。
pub(crate) fn flatten_seg(
    seg: &PathSeg,
    t: &Transform,
    tol: f64,
    reach: f64,
    budget: &Budget<'_>,
    out: &mut dyn FnMut(Point) -> Result<(), Error>,
) -> Result<(), Error> {
    let n = match seg {
        PathSeg::Move(_) | PathSeg::Close(..) => return Ok(()),
        PathSeg::Line(_, b) => {
            budget.spend(cost::FLATTEN_POINT)?;
            return out(*b);
        }
        _ => {
            budget.spend(cost::FLATTEN_CURVE)?;
            pieces(seg, t, tol, reach)
        }
    };
    let step = chunk_len(cost::FLATTEN_POINT) as u64;
    let nf = n as f64;
    // 終わった点の数（n が u64::MAX でも終わる。単位 1 の 4 回目のレビュー）
    let mut done = 0u64;
    while done < n {
        let k = step.min(n.saturating_sub(done));
        budget.spend(k.saturating_mul(cost::FLATTEN_POINT))?;
        for j in 1..=k {
            let i = done.saturating_add(j);
            let p = if i == n {
                end_point(seg)
            } else {
                let u = (i as f64) / nf;
                match *seg {
                    PathSeg::Quad(p0, p1, p2) => quad_at(p0, p1, p2, u),
                    PathSeg::Cubic(p0, p1, p2, p3) => cubic_at(p0, p1, p2, p3, u),
                    PathSeg::ArcTo(from, a) => a.at_offset(from, a.dtheta * u),
                    _ => end_point(seg),
                }
            };
            out(p)?;
        }
        done = done.saturating_add(k);
    }
    Ok(())
}

/// 区間の終わりの点。
pub(crate) fn end_point(seg: &PathSeg) -> Point {
    match *seg {
        PathSeg::Move(p) | PathSeg::Line(_, p) | PathSeg::Quad(_, _, p) | PathSeg::Cubic(_, _, _, p) => p,
        PathSeg::ArcTo(_, a) => a.end,
        PathSeg::Close(_, s) => s,
    }
}

#[cfg(test)]
mod tests;
