#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::arithmetic_side_effects
)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;
use crate::geom::{ArcKind, ArcSeg, endpoint_to_center};

fn flat(seg: &PathSeg, t: &Transform, tol: f64) -> Vec<Point> {
    let b = Budget::new(Stage::Plan, u64::MAX, u64::MAX, &Never);
    let mut v = Vec::new();
    flatten_seg(seg, t, tol, &b, &mut |p| {
        v.push(p);
        Ok(())
    })
    .unwrap();
    v
}

/// 点 `p` から折れ線 `pts`（`from` から始まる）までの距離の最小。
fn dist_to_polyline(p: Point, from: Point, pts: &[Point]) -> f64 {
    let mut best = f64::INFINITY;
    let mut a = from;
    for &b in pts {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let l2 = dx * dx + dy * dy;
        let t = if l2 == 0.0 { 0.0 } else { (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0) };
        let (qx, qy) = (a.x + t * dx, a.y + t * dy);
        best = best.min(((p.x - qx).powi(2) + (p.y - qy).powi(2)).sqrt());
        a = b;
    }
    best
}

#[test]
fn counts_follow_the_formulas() {
    let id = Transform::IDENTITY;
    // 線は 1
    assert_eq!(pieces(&PathSeg::Line(Point::new(0.0, 0.0), Point::new(1e9, 0.0)), &id, 0.1), 1);
    // 2 次曲線：|P0 − 2P1 + P2| = 20 → √(20 / 0.4) = √50 → 8
    let q = PathSeg::Quad(Point::new(0.0, 0.0), Point::new(5.0, 10.0), Point::new(10.0, 0.0));
    assert_eq!(pieces(&q, &id, 0.1), 8);
    // 倍率 4 で点の数はおよそ 2 倍（√）
    assert_eq!(pieces(&q, &id.scaled(4.0), 0.1), 15);
    // 半径 100 の半円：π √(100 / 0.8) = 35.1 → 36
    let ArcKind::Curve(a) =
        endpoint_to_center(Point::new(0.0, 0.0), 100.0, 100.0, 0.0, false, true, Point::new(200.0, 0.0))
    else {
        panic!()
    };
    assert_eq!(pieces(&PathSeg::ArcTo(Point::new(0.0, 0.0), a), &id, 0.1), 36);
}

#[test]
fn far_curves_are_not_split() {
    // 制御点がすべて箱の右の外：1 本の線
    let far = 1e9;
    let c = PathSeg::Cubic(
        Point::new(far, 0.0),
        Point::new(far, 1e8),
        Point::new(far + 1e8, 1e8),
        Point::new(far + 1e8, 0.0),
    );
    assert_eq!(pieces(&c, &Transform::IDENTITY, 0.1), 1);
    let pts = flat(&c, &Transform::IDENTITY, 0.1);
    assert_eq!(pts, [Point::new(far + 1e8, 0.0)]);
    // 箱をまたぐなら分ける（片側だけでない）
    let wide =
        PathSeg::Cubic(Point::new(-far, 0.0), Point::new(-far, 10.0), Point::new(far, 10.0), Point::new(far, 0.0));
    assert!(pieces(&wide, &Transform::IDENTITY, 0.1) > 1);
    // 箱の外の弧（楕円の外接矩形がすべて外）
    let a = ArcSeg {
        c: Point::new(far, far),
        m: [1e6, 0.0, 0.0, 1e6],
        theta1: 0.0,
        dtheta: 6.0,
        end: Point::new(far + 1e6, far),
    };
    assert_eq!(pieces(&PathSeg::ArcTo(Point::new(far + 1e6, far), a), &Transform::IDENTITY, 0.1), 1);
    // 有限でない・大きすぎる数は u64::MAX（見積もりが断る）
    let huge = PathSeg::Cubic(
        Point::new(-1e300, 0.0),
        Point::new(1e300, 1e300),
        Point::new(-1e300, -1e300),
        Point::new(1e300, 0.0),
    );
    assert_eq!(pieces(&huge, &Transform::IDENTITY, 0.1), u64::MAX);
}

#[test]
fn points_end_exactly_and_stay_within_tolerance() {
    let tol = 0.1;
    let t = Transform { a: 3.0, b: 1.0, c: -0.5, d: 2.0, e: 7.0, f: -3.0 };
    let p0 = Point::new(0.0, 0.0);
    let c = PathSeg::Cubic(p0, Point::new(10.0, 40.0), Point::new(50.0, -30.0), Point::new(60.0, 10.0));
    let pts = flat(&c, &t, tol);
    assert_eq!(pts.len() as u64, pieces(&c, &t, tol));
    assert_eq!(*pts.last().unwrap(), Point::new(60.0, 10.0));
    // 曲線の点（細かく取ったもの）は、描く座標系で折れ線から誤差の内
    let dev: Vec<Point> = pts.iter().map(|&p| t.apply(p)).collect();
    for i in 0..=1000 {
        let u = i as f64 / 1000.0;
        let q = t.apply(cubic_at(p0, Point::new(10.0, 40.0), Point::new(50.0, -30.0), Point::new(60.0, 10.0), u));
        assert!(dist_to_polyline(q, t.apply(p0), &dev) <= tol + 1e-9, "{u}");
    }
    // 弧
    let ArcKind::Curve(a) = endpoint_to_center(p0, 30.0, 10.0, 20.0, true, true, Point::new(40.0, 5.0)) else {
        panic!()
    };
    let s = PathSeg::ArcTo(p0, a);
    let pts = flat(&s, &t, tol);
    assert_eq!(*pts.last().unwrap(), Point::new(40.0, 5.0));
    let dev: Vec<Point> = pts.iter().map(|&p| t.apply(p)).collect();
    for i in 0..=1000 {
        let th = a.theta1 + a.dtheta * (i as f64 / 1000.0);
        let q = t.apply(a.at(th));
        assert!(dist_to_polyline(q, t.apply(p0), &dev) <= tol + 1e-9, "{i}");
    }
}

#[test]
fn flattening_pays_per_point() {
    let b = Budget::new(Stage::Plan, u64::MAX, u64::MAX, &Never);
    // 2 階の差は √2 · 10⁷ なので n = ⌈√(3 · √2 · 10⁷ / 0.4)⌉ ≈ 10,300（塊を 2 つ以上に分ける）
    let c = PathSeg::Cubic(Point::new(0.0, 0.0), Point::new(0.0, 1e7), Point::new(1e7, 1e7), Point::new(1e7, 0.0));
    let n = pieces(&c, &Transform::IDENTITY, 0.1);
    assert!(n > 10_000, "{n}");
    flatten_seg(&c, &Transform::IDENTITY, 0.1, &b, &mut |_| Ok(())).unwrap();
    assert_eq!(b.work(), n * cost::FLATTEN_POINT);
    assert!(b.max_spend() < crate::budget::CANCEL_INTERVAL);
}
