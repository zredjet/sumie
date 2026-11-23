#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::arithmetic_side_effects
)]

use super::*;
use crate::budget::Budget;
use crate::cancel::Never;
use crate::error::Stage;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn pt_close(a: Point, b: Point) -> bool {
    close(a.x, b.x) && close(a.y, b.y)
}

/// パスを書いて区間の並びにする。
fn build(f: impl FnOnce(&mut PathWriter<'_, '_>) -> Result<(), Error>) -> Vec<PathSeg> {
    let budget = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
    let mut verbs = MVec::new(&budget);
    let mut coords = MVec::new(&budget);
    {
        let mut w = PathWriter::new(&mut verbs, &mut coords);
        f(&mut w).unwrap();
    }
    PathRef { verbs: verbs.as_slice(), coords: coords.as_slice() }.segments().collect()
}

#[test]
fn transform_basics() {
    let t = Transform { a: 2.0, b: 0.0, c: 0.0, d: 3.0, e: 1.0, f: -1.0 };
    assert_eq!(t.apply(Point::new(1.0, 1.0)), Point::new(3.0, 2.0));
    assert_eq!(t.apply_vec(Point::new(1.0, 1.0)), Point::new(2.0, 3.0));
    assert_eq!(t.det(), 6.0);
    assert!(close(t.max_scale(), 3.0));
    // 回転は長さを変えない
    let r = Transform { a: 0.6, b: 0.8, c: -0.8, d: 0.6, e: 0.0, f: 0.0 };
    assert!(close(r.max_scale(), 1.0));
    assert!(close(t.scaled(2.0).max_scale(), 6.0));
    // 大きな行列でも正しい（最大の成分で割ってから求める）
    assert!((max_singular(1e80, 0.0, 0.0, 1e79) / 1e80 - 1.0).abs() < 1e-12);
    assert!((max_singular(3e77, 0.0, 0.0, 1e77) / 3e77 - 1.0).abs() < 1e-12);
    assert_eq!(max_singular(f64::INFINITY, 0.0, 0.0, 1.0), f64::INFINITY);
    assert_eq!(max_singular(0.0, 0.0, 0.0, 0.0), 0.0);
}

/// SVG 1.1 付録 F.6.5：半円（始めと終わりが直径の両端）は中心が真ん中、旗で向きと大小が決まる。
#[test]
fn arc_endpoint_to_center() {
    let p1 = Point::new(0.0, 0.0);
    let p2 = Point::new(10.0, 0.0);
    let ArcKind::Curve(a) = endpoint_to_center(p1, 5.0, 5.0, 0.0, false, true, p2) else { panic!() };
    assert!(pt_close(a.c, Point::new(5.0, 0.0)));
    assert!(close(a.max_radius(), 5.0));
    // sweep = 1 は角が増える向き（y が下向きなので画面で時計回り）
    assert!(close(a.dtheta, core::f64::consts::PI), "{}", a.dtheta);
    assert!(pt_close(a.at(a.theta1), p1));
    assert!(pt_close(a.at(a.theta1 + a.dtheta), p2));
    // 上を回る（sweep = 1 で 0 → 10 は y が負の側を通らない：角が π から 2π へ増えると y は負）
    let mid = a.at(a.theta1 + a.dtheta / 2.0);
    assert!(close(mid.x, 5.0) && close(mid.y.abs(), 5.0));
    let ArcKind::Curve(b) = endpoint_to_center(p1, 5.0, 5.0, 0.0, false, false, p2) else { panic!() };
    assert!(close(b.dtheta, -core::f64::consts::PI));
    let mid_b = b.at(b.theta1 + b.dtheta / 2.0);
    assert!(close(mid_b.y, -mid.y));
    // 半径が足りなければ広げる（F.6.6）：半径 1 でも直径の両端なら半径 5 になる
    let ArcKind::Curve(c) = endpoint_to_center(p1, 1.0, 1.0, 0.0, false, true, p2) else { panic!() };
    assert!(close(c.max_radius(), 5.0));
    // 大きな弧と小さな弧
    let ArcKind::Curve(small) = endpoint_to_center(p1, 10.0, 10.0, 0.0, false, true, p2) else { panic!() };
    let ArcKind::Curve(large) = endpoint_to_center(p1, 10.0, 10.0, 0.0, true, true, p2) else { panic!() };
    assert!(small.dtheta.abs() < core::f64::consts::PI && large.dtheta.abs() > core::f64::consts::PI);
    assert!(close(small.dtheta.abs() + large.dtheta.abs(), 2.0 * core::f64::consts::PI));
    // 同じ点・半径 0・負の半径
    assert_eq!(endpoint_to_center(p1, 5.0, 5.0, 0.0, false, true, p1), ArcKind::Line);
    assert_eq!(endpoint_to_center(p1, 0.0, 5.0, 0.0, false, true, p2), ArcKind::Line);
    assert_eq!(
        endpoint_to_center(p1, -5.0, -5.0, 0.0, false, true, p2),
        endpoint_to_center(p1, 5.0, 5.0, 0.0, false, true, p2)
    );
    // 回した楕円：始めと終わりの点を通る
    let q = Point::new(3.0, 7.0);
    let ArcKind::Curve(r) = endpoint_to_center(p1, 6.0, 2.0, 30.0, true, false, q) else { panic!() };
    assert!(pt_close(r.at(r.theta1), p1) && pt_close(r.at(r.theta1 + r.dtheta), q));
    assert_eq!(r.end, q);
    // 巨大な半径（2 乗があふれる）の弧は、ほぼ線なので線（F.6.5 の正規化した形。単位の 1 回目のレビュー）
    for end in [Point::new(10.0, 0.0), Point::new(10.0, 3.0)] {
        match endpoint_to_center(p1, 1e200, 1e200, 0.0, false, true, end) {
            ArcKind::Line => {}
            ArcKind::Curve(a) => {
                // 弧のままなら、始めと終わりの点を通る
                assert!(pt_close(a.at(a.theta1), p1) && pt_close(a.at(a.theta1 + a.dtheta), end), "{a:?}");
            }
        }
    }
    // 小さな半径は広げて半円（2 乗が 0 にならない）
    let ArcKind::Curve(tiny) = endpoint_to_center(p1, 1e-200, 1e-200, 0.0, false, true, Point::new(10.0, 0.0)) else {
        panic!()
    };
    assert!(close(tiny.max_radius(), 5.0), "{}", tiny.max_radius());
    assert!(pt_close(tiny.at(tiny.theta1), p1));
}

#[test]
fn shapes() {
    // 角の丸くない rect：始まり・3 本の線・閉じ
    let s = build(|w| rect(w, 1.0, 2.0, 3.0, 4.0, 0.0, 0.0));
    assert_eq!(s.len(), 5);
    assert_eq!(s[0], PathSeg::Move(Point::new(1.0, 2.0)));
    assert_eq!(s[1], PathSeg::Line(Point::new(1.0, 2.0), Point::new(4.0, 2.0)));
    assert!(matches!(s[4], PathSeg::Close(_, p) if p == Point::new(1.0, 2.0)));
    // 角の丸い rect：右上へ向かう線から始まり、4 つの弧の終わりの点は書いたとおり
    let s = build(|w| rect(w, 0.0, 0.0, 10.0, 6.0, 2.0, 1.0));
    assert_eq!(s[0], PathSeg::Move(Point::new(2.0, 0.0)));
    let ends: Vec<Point> =
        s.iter().filter_map(|x| if let PathSeg::ArcTo(_, a) = x { Some(a.end) } else { None }).collect();
    assert_eq!(ends, [Point::new(10.0, 1.0), Point::new(8.0, 6.0), Point::new(0.0, 5.0), Point::new(2.0, 0.0)]);
    for x in &s {
        if let PathSeg::ArcTo(from, a) = x {
            assert!(pt_close(a.at(a.theta1), *from) && pt_close(a.at(a.theta1 + a.dtheta), a.end));
            assert!(a.dtheta > 0.0);
        }
    }
    // 大きさ 0 は書かない
    assert!(build(|w| rect(w, 0.0, 0.0, 0.0, 6.0, 0.0, 0.0)).is_empty());
    // 楕円：(cx + rx, cy) から時計回り
    let s = build(|w| ellipse(w, 5.0, 5.0, 4.0, 2.0));
    assert_eq!(s[0], PathSeg::Move(Point::new(9.0, 5.0)));
    let PathSeg::ArcTo(_, a) = s[1] else { panic!() };
    assert_eq!(a.end, Point::new(5.0, 7.0));
    assert_eq!(s.len(), 6);
    assert!(build(|w| ellipse(w, 5.0, 5.0, 0.0, 2.0)).is_empty());
}

#[test]
fn exact_bounds() {
    let budget = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
    let mut verbs = MVec::new(&budget);
    let mut coords = MVec::new(&budget);
    let mut w = PathWriter::new(&mut verbs, &mut coords);
    // 制御点は y = 10 まで伸びるが、曲線の頂きは 7.5（3 次曲線の極）
    w.move_to(Point::new(0.0, 0.0)).unwrap();
    w.cubic_to(Point::new(0.0, 10.0), Point::new(10.0, 10.0), Point::new(10.0, 0.0)).unwrap();
    let p = PathRef { verbs: verbs.as_slice(), coords: coords.as_slice() };
    let r = bounds(p, &Transform::IDENTITY);
    assert!(close(r.y1, 7.5) && close(r.x0, 0.0) && close(r.x1, 10.0) && close(r.y0, 0.0), "{r:?}");
    // 円は中心 ± 半径（変形で倍）
    let mut verbs = MVec::new(&budget);
    let mut coords = MVec::new(&budget);
    let mut w = PathWriter::new(&mut verbs, &mut coords);
    ellipse(&mut w, 5.0, 5.0, 3.0, 3.0).unwrap();
    let p = PathRef { verbs: verbs.as_slice(), coords: coords.as_slice() };
    let r = bounds(p, &Transform::IDENTITY);
    assert!(close(r.x0, 2.0) && close(r.x1, 8.0) && close(r.y0, 2.0) && close(r.y1, 8.0), "{r:?}");
    // 45 度回した楕円（半径 4 と 2）の外接矩形の幅は √(2·(16 + 4)) = √40
    let rot = Transform {
        a: core::f64::consts::FRAC_1_SQRT_2,
        b: core::f64::consts::FRAC_1_SQRT_2,
        c: -core::f64::consts::FRAC_1_SQRT_2,
        d: core::f64::consts::FRAC_1_SQRT_2,
        e: 0.0,
        f: 0.0,
    };
    let mut verbs = MVec::new(&budget);
    let mut coords = MVec::new(&budget);
    let mut w = PathWriter::new(&mut verbs, &mut coords);
    ellipse(&mut w, 0.0, 0.0, 4.0, 2.0).unwrap();
    let p = PathRef { verbs: verbs.as_slice(), coords: coords.as_slice() };
    let r = bounds(p, &rot);
    assert!((r.x1 - r.x0 - 40f64.sqrt()).abs() < 1e-9, "{r:?}");
    // 2 次曲線の極
    let s = PathSeg::Quad(Point::new(0.0, 0.0), Point::new(5.0, 10.0), Point::new(10.0, 0.0));
    let mut r = Rect::EMPTY;
    add_bounds(&mut r, &s);
    assert!(close(r.y1, 5.0));
    // 弧の極が掃く角の外なら入れない（4 分の 1 の弧）
    let a = PathSeg::ArcTo(
        Point::new(1.0, 0.0),
        ArcSeg {
            c: Point::new(0.0, 0.0),
            m: [1.0, 0.0, 0.0, 1.0],
            theta1: 0.0,
            dtheta: QUARTER,
            end: Point::new(0.0, 1.0),
        },
    );
    let mut r = Rect::EMPTY;
    add_bounds(&mut r, &a);
    assert!(close(r.x0, 0.0) && close(r.x1, 1.0) && close(r.y0, 0.0) && close(r.y1, 1.0), "{r:?}");
}

#[test]
fn rect_ops() {
    let mut r = Rect::EMPTY;
    assert!(r.is_empty());
    r.add(Point::new(1.0, 2.0));
    assert!(!r.is_empty());
    let u = r.union(&Rect { x0: -1.0, y0: 0.0, x1: 0.0, y1: 5.0 });
    assert_eq!(u, Rect { x0: -1.0, y0: 0.0, x1: 1.0, y1: 5.0 });
    assert_eq!(Rect::EMPTY.union(&r), r);
    let t = Transform { a: 0.0, b: 1.0, c: -1.0, d: 0.0, e: 0.0, f: 0.0 };
    assert_eq!(u.transformed(&t), Rect { x0: -5.0, y0: -1.0, x1: 0.0, y1: 1.0 });
}

/// ほぼ 2 次の 3 次曲線でも極を落とさない（桁の落ちない根の公式。単位 1 の 3 回目のレビュー）。
#[test]
fn nearly_quadratic_cubic_bounds() {
    let seg = PathSeg::Cubic(
        Point::new(0.0, 0.0),
        Point::new(1e6, 0.0),
        Point::new(0.0, 0.0),
        Point::new(-2.9999999999999995e6, 0.0),
    );
    let mut r = Rect::EMPTY;
    add_bounds(&mut r, &seg);
    assert!(r.x1 > 374_000.0, "{r:?}");
    let seg =
        PathSeg::Cubic(Point::new(0.0, 10.0), Point::new(-8e6, 40.0), Point::new(-8e6, 60.0), Point::new(5e-10, 90.0));
    let mut r = Rect::EMPTY;
    add_bounds(&mut r, &seg);
    assert!(r.x0 < -5.9e6, "{r:?}");
    let seg = PathSeg::Cubic(
        Point::new(0.0, 0.0),
        Point::new(0.0, 2000.0),
        Point::new(1000.0, 0.0),
        Point::new(1000.0, -5999.999999999999),
    );
    let mut r = Rect::EMPTY;
    add_bounds(&mut r, &seg);
    assert!(r.y1 > 400.0, "{r:?}");
}
