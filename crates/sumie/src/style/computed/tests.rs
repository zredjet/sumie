#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::indexing_slicing)]

use super::*;

fn base(viewport: Option<(f64, f64)>, doc: Option<(f64, f64)>) -> Base {
    Base { default_font: 12.0, root_font: 20.0, root_line: 30.0, viewport, doc }
}

#[test]
fn units_resolve_against_their_context() {
    let b = base(Some((300.0, 400.0)), Some((100.0, 50.0)));
    let r = |v: f64, u: Unit, axis: Axis| resolve(Length::new(v, u), 10.0, 15.0, &b, axis);
    assert_eq!(r(2.0, Unit::Em, Axis::X), Some(20.0));
    assert_eq!(r(2.0, Unit::Ex, Axis::X), Some(10.0));
    assert_eq!(r(1.0, Unit::Cap, Axis::X), Some(7.0));
    assert_eq!(r(2.0, Unit::Lh, Axis::X), Some(30.0));
    assert_eq!(r(1.0, Unit::Rem, Axis::X), Some(20.0));
    assert_eq!(r(1.0, Unit::Rlh, Axis::X), Some(30.0));
    assert_eq!(r(1.0, Unit::In, Axis::X), Some(96.0));
    assert_eq!(r(10.0, Unit::Percent, Axis::X), Some(30.0));
    assert_eq!(r(10.0, Unit::Percent, Axis::Y), Some(40.0));
    // 対角 ÷ √2（500 ÷ √2）
    let d = r(10.0, Unit::Percent, Axis::Diag).unwrap();
    assert!((d - 50.0 / core::f64::consts::SQRT_2).abs() < 1e-9, "{d}");
    // vw などは文書の大きさ
    assert_eq!(r(10.0, Unit::Vw, Axis::X), Some(10.0));
    assert_eq!(r(10.0, Unit::Vmax, Axis::X), Some(10.0));
    assert_eq!(r(10.0, Unit::Vmin, Axis::X), Some(5.0));
    assert_eq!(r(10.0, Unit::Vb, Axis::X), Some(5.0));
    // 基準がなければ解けない
    let none = base(None, None);
    assert_eq!(resolve(Length::new(10.0, Unit::Percent), 10.0, 15.0, &none, Axis::X), None);
    assert_eq!(resolve(Length::new(10.0, Unit::Vw), 10.0, 15.0, &none, Axis::X), None);
    // 有限でない値は解けない
    assert_eq!(resolve(Length::new(1e308, Unit::In), 10.0, 15.0, &none, Axis::X), None);
}

#[test]
fn initial_values() {
    let c = Computed::initial(16.0);
    assert_eq!(c.fill, CPaint::BLACK);
    assert_eq!(c.stroke, CPaint::NONE);
    assert_eq!((c.stroke_width, c.opacity, c.font_size), (1.0, 1.0, 16.0));
    assert!((c.line_height - 19.2).abs() < 1e-9);
    assert_eq!(c.transform, Transform::IDENTITY);
}
