#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use super::*;
use crate::budget::Budget;
use crate::cancel::Never;
use crate::error::Stage;

/// 閉じた多角形（描く座標系の点）の辺を y0 の順に並べる。
fn edges_of(polys: &[&[(f64, f64)]], budget: &Budget<'_>) -> Vec<Edge> {
    let mut out = MVec::new(budget);
    for poly in polys {
        let n = poly.len();
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            push_segment(Point::new(a.0, a.1), Point::new(b.0, b.1), &mut out).unwrap();
        }
    }
    let mut v = out.as_slice().to_vec();
    v.sort_by_key(|e| e.y0);
    v
}

/// 範囲 `rect` の被覆の格子（行ごと、範囲の幅）。
fn grid(edges: &[Edge], rect: PixelBox, rule: FillRule, aa: bool) -> Vec<i64> {
    let budget = Budget::new(Stage::Render, u64::MAX, u64::MAX, &Never);
    let w = rect.width() as usize;
    let mut scan = Scan::new(rect.x0, w, edges.len(), &budget).unwrap();
    let mut g = vec![0i64; w * rect.height() as usize];
    fill_region(edges, rule, aa, rect, &mut scan, &budget, &mut |x, y, len, c| {
        for k in 0..len as i64 {
            let i = ((y - rect.y0) as usize) * w + (x + k - rect.x0) as usize;
            assert_eq!(g[i], 0, "同じ画素を 2 度");
            g[i] = c;
        }
        Ok(())
    })
    .unwrap();
    g
}

fn rect_px(x0: i64, y0: i64, x1: i64, y1: i64) -> PixelBox {
    PixelBox { x0, y0, x1, y1 }
}

#[test]
fn coverage_of_a_square() {
    let budget = Budget::new(Stage::Render, u64::MAX, u64::MAX, &Never);
    let sq: &[(f64, f64)] = &[(1.5, 1.5), (3.5, 1.5), (3.5, 3.5), (1.5, 3.5)];
    let e = edges_of(&[sq], &budget);
    let g = grid(&e, rect_px(0, 0, 5, 5), FillRule::NonZero, true);
    let at = |x: usize, y: usize| g[y * 5 + x];
    assert_eq!(at(1, 1), FULL / 4);
    assert_eq!(at(2, 2), FULL);
    assert_eq!(at(2, 1), FULL / 2);
    assert_eq!(at(1, 2), FULL / 2);
    assert_eq!(at(0, 0), 0);
    assert_eq!(at(4, 4), 0);
    // 向きを逆にしても同じ（非ゼロ）
    let rev: Vec<(f64, f64)> = sq.iter().rev().copied().collect();
    let e2 = edges_of(&[&rev], &budget);
    assert_eq!(grid(&e2, rect_px(0, 0, 5, 5), FillRule::NonZero, true), g);
}

#[test]
fn total_coverage_is_the_area() {
    let budget = Budget::new(Stage::Render, u64::MAX, u64::MAX, &Never);
    let tri: &[(f64, f64)] = &[(0.3, 0.7), (17.9, 3.1), (6.2, 15.4)];
    let e = edges_of(&[tri], &budget);
    let g = grid(&e, rect_px(0, 0, 20, 20), FillRule::NonZero, true);
    let sum: i64 = g.iter().sum();
    let area = ((17.9f64 - 0.3) * (15.4 - 0.7) - (6.2 - 0.3) * (3.1 - 0.7)).abs() / 2.0;
    let got = sum as f64 / FULL as f64;
    assert!((got - area).abs() < 0.05, "{got} {area}");
}

#[test]
fn even_odd_and_nonzero() {
    let budget = Budget::new(Stage::Render, u64::MAX, u64::MAX, &Never);
    let outer: &[(f64, f64)] = &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let inner: &[(f64, f64)] = &[(3.0, 3.0), (7.0, 3.0), (7.0, 7.0), (3.0, 7.0)];
    let e = edges_of(&[outer, inner], &budget);
    let nz = grid(&e, rect_px(0, 0, 10, 10), FillRule::NonZero, true);
    let eo = grid(&e, rect_px(0, 0, 10, 10), FillRule::EvenOdd, true);
    assert_eq!(nz[5 * 10 + 5], FULL);
    assert_eq!(eo[5 * 10 + 5], 0);
    assert_eq!(eo[11], FULL);
}

#[test]
fn crisp_edges_sample_the_centre() {
    let budget = Budget::new(Stage::Render, u64::MAX, u64::MAX, &Never);
    let sq: &[(f64, f64)] = &[(1.3, 1.6), (3.5, 1.6), (3.5, 3.4), (1.3, 3.4)];
    let e = edges_of(&[sq], &budget);
    let g = grid(&e, rect_px(0, 0, 5, 5), FillRule::NonZero, false);
    let at = |x: usize, y: usize| g[y * 5 + x];
    // 中心 1.5 は 1.3 より右 → 中。行 1 の中心 1.5 は 1.6 より上 → 外
    assert_eq!(at(1, 2), FULL);
    assert_eq!(at(1, 1), 0);
    // 中心 3.5 はちょうど右の縁：縁は外（中心が縁以上の画素から足し、右の縁で引く）
    assert_eq!(at(3, 2), 0);
    assert_eq!(at(2, 3), 0);
    assert!(g.iter().all(|&c| c == 0 || c == FULL));
}

#[test]
fn shapes_beyond_the_box() {
    let budget = Budget::new(Stage::Render, u64::MAX, u64::MAX, &Never);
    // 左の外（箱の外）から右へ 5.5 まで：画素 0〜4 は全部、5 は半分
    let big: &[(f64, f64)] = &[(-1e12, -3.0), (5.5, -3.0), (5.5, 30.0), (-1e12, 30.0)];
    let e = edges_of(&[big], &budget);
    let g = grid(&e, rect_px(0, 0, 8, 2), FillRule::NonZero, true);
    assert_eq!(&g[0..8], &[FULL, FULL, FULL, FULL, FULL, FULL / 2, 0, 0]);
    // 右の外まで伸びる形は範囲の右の終わりまで塗る
    let right: &[(f64, f64)] = &[(2.0, 0.0), (1e12, 0.0), (1e12, 1.0), (2.0, 1.0)];
    let e = edges_of(&[right], &budget);
    let g = grid(&e, rect_px(0, 0, 8, 1), FillRule::NonZero, true);
    assert_eq!(g, [0, 0, FULL, FULL, FULL, FULL, FULL, FULL]);
    // 有限でない点は辺にしない
    let mut out = MVec::new(&budget);
    push_segment(Point::new(f64::INFINITY, 0.0), Point::new(1.0, 1.0), &mut out).unwrap();
    assert!(out.is_empty());
}

/// 種つきの xorshift。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn f(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() % 1_000_000) as f64 / 1_000_000.0)
    }

    fn i(&mut self, n: i64) -> i64 {
        (self.next() % n as u64) as i64
    }
}

/// 範囲の被覆は、全体の被覆の同じ矩形とビット単位で同じ（ADR-13・ADR-30）。
#[test]
fn regions_match_the_full_render() {
    let budget = Budget::new(Stage::Render, u64::MAX, u64::MAX, &Never);
    let mut r = Rng(0x5eed_2a5e_0001);
    for case in 0..200 {
        // 乱れた多角形 3 つ（キャンバスの外にも出る）
        let mut polys: Vec<Vec<(f64, f64)>> = Vec::new();
        for _ in 0..3 {
            let n = 3 + r.i(8) as usize;
            polys.push((0..n).map(|_| (r.f(-20.0, 60.0), r.f(-20.0, 60.0))).collect());
        }
        let refs: Vec<&[(f64, f64)]> = polys.iter().map(|p| p.as_slice()).collect();
        let e = edges_of(&refs, &budget);
        for (rule, aa) in [(FillRule::NonZero, true), (FillRule::EvenOdd, true), (FillRule::NonZero, false)] {
            let full = grid(&e, rect_px(0, 0, 40, 40), rule, aa);
            for _ in 0..8 {
                let x0 = r.i(40);
                let y0 = r.i(40);
                let x1 = x0 + 1 + r.i(40 - x0);
                let y1 = y0 + 1 + r.i(40 - y0);
                let part = grid(&e, rect_px(x0, y0, x1, y1), rule, aa);
                let w = (x1 - x0) as usize;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let a = full[(y * 40 + x) as usize];
                        let b = part[((y - y0) as usize) * w + (x - x0) as usize];
                        assert_eq!(a, b, "{case} 件目 ({x}, {y}) 範囲 {x0},{y0},{x1},{y1}");
                    }
                }
            }
        }
    }
}

#[test]
fn scanning_pays() {
    let budget = Budget::new(Stage::Render, u64::MAX, u64::MAX, &Never);
    let big: &[(f64, f64)] = &[(0.0, 0.0), (3000.0, 1.0), (3000.0, 2000.0), (0.0, 2000.0)];
    let e = edges_of(&[big], &budget);
    let b = Budget::new(Stage::Render, u64::MAX, u64::MAX, &Never);
    let mut scan = Scan::new(0, 3000, e.len(), &b).unwrap();
    fill_region(&e, FillRule::NonZero, true, rect_px(0, 0, 3000, 2000), &mut scan, &b, &mut |_, _, _, _| Ok(()))
        .unwrap();
    assert!(b.work() > 2000 * cost_row());
    assert!(b.max_spend() < crate::budget::CANCEL_INTERVAL);
}

fn cost_row() -> u64 {
    crate::cost::RASTER_ROW
}
