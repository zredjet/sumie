#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;

fn with<T>(src: &str, f: impl FnOnce(&mut Values<'_, '_>) -> Result<Option<T>, Error>) -> Option<T> {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let s = src.as_bytes();
    let mut v = Values::new(s, Span::new(0, s.len()), &budget);
    let r = f(&mut v).unwrap();
    // 値の全部を読んだときだけ正しい
    if v.at_end().unwrap() { r } else { None }
}

fn color(src: &str) -> Option<(f32, f32, f32, f32)> {
    match with(src, |v| v.color())? {
        ColorValue::Color(c) => Some((c.r, c.g, c.b, c.a)),
        ColorValue::CurrentColor => Some((-1.0, -1.0, -1.0, -1.0)),
    }
}

#[test]
fn colors() {
    assert_eq!(color("#0a0"), Some((0.0, 170.0, 0.0, 1.0)));
    assert_eq!(color("#00AA0080"), Some((0.0, 170.0, 0.0, 128.0 / 255.0)));
    assert_eq!(color("GREEN"), Some((0.0, 128.0, 0.0, 1.0)));
    assert_eq!(color("RebeccaPurple"), Some((102.0, 51.0, 153.0, 1.0)));
    assert_eq!(color("transparent"), Some((0.0, 0.0, 0.0, 0.0)));
    assert_eq!(color("currentColor"), Some((-1.0, -1.0, -1.0, -1.0)));
    assert_eq!(color("rgb(0, 128, 0)"), Some((0.0, 128.0, 0.0, 1.0)));
    assert_eq!(color("rgb(0% 50% 0% / 50%)"), Some((0.0, 127.5, 0.0, 0.5)));
    assert_eq!(color("rgba(0 128 0 / .25)"), Some((0.0, 128.0, 0.0, 0.25)));
    assert_eq!(color("rgb(300, -1, 1.5)"), Some((255.0, 0.0, 1.5, 1.0)));
    assert_eq!(color("hsl(120, 100%, 25%)"), Some((0.0, 127.5, 0.0, 1.0)));
    assert_eq!(color("hsl(120deg 100 25 / 1)"), Some((0.0, 127.5, 0.0, 1.0)));
    assert_eq!(color("hwb(120 0% 50%)"), Some((0.0, 127.5, 0.0, 1.0)));
    assert_eq!(color("hwb(0 60% 60%)"), Some((127.5, 127.5, 127.5, 1.0)));
    for bad in [
        "#12",
        "#12345",
        "#ggg",
        "bogus",
        "ButtonText",
        "rgb(0, 50%, 0)",
        "rgb(0 0)",
        "rgb(0, 0, 0, 0, 0)",
        "rgb(0,0 0)",
        "hsl(120, 100, 25)",
        "hwb(120, 0%, 50%)",
        "rgb(1e999, 0, 0)",
        "rgb(none, 0, 0)",
        "lab(50% 0 0)",
    ] {
        assert_eq!(color(bad), None, "{bad}");
    }
}

#[test]
fn lengths_and_angles() {
    let len = |s: &str, unitless: bool| with(s, |v| v.length(unitless));
    assert_eq!(len("10", true), Some(Length::new(10.0, Unit::None)));
    assert_eq!(len("10", false), None);
    assert_eq!(len("0", false), Some(Length::new(0.0, Unit::None)));
    assert_eq!(len("1.5PX", false), Some(Length::new(1.5, Unit::Px)));
    assert_eq!(len("2em", false), Some(Length::new(2.0, Unit::Em)));
    assert_eq!(len("50%", false), Some(Length::new(50.0, Unit::Percent)));
    assert_eq!(len("3vmin", false), Some(Length::new(3.0, Unit::Vmin)));
    assert_eq!(len("1in", false).unwrap().absolute_px(), Some(96.0));
    assert_eq!(len("10mm", false).unwrap().absolute_px(), Some(10.0 * 96.0 / 25.4));
    assert_eq!(len("1foo", true), None);
    assert_eq!(len("1e999px", true), None);
    let ang = |s: &str| with(s, |v| v.angle(false));
    assert_eq!(ang("90deg"), Some(90.0));
    assert_eq!(ang("100grad"), Some(90.0));
    assert_eq!(ang("0.25turn"), Some(90.0));
    assert_eq!(ang("90"), None);
}

#[test]
fn paints() {
    let p = |s: &str| with(s, |v| v.paint());
    assert_eq!(p("none"), Some(Paint::None));
    assert_eq!(p("context-stroke"), Some(Paint::ContextStroke));
    assert!(matches!(p("red"), Some(Paint::Color(ColorValue::Color(_)))));
    let src = "url(#a) red";
    match p(src) {
        Some(Paint::Url { href, fallback: Fallback::Color(_), .. }) => assert_eq!(href.of(src.as_bytes()), b"#a"),
        x => panic!("{x:?}"),
    }
    assert!(matches!(p("url(\"#b\") none"), Some(Paint::Url { fallback: Fallback::None, .. })));
    assert!(matches!(p("url(#b)"), Some(Paint::Url { fallback: Fallback::Unset, .. })));
    assert_eq!(p("url(#b) bogus"), None);
    assert_eq!(p("red blue"), None);
}

fn tf(src: &str, attr: bool) -> Option<[f64; 6]> {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let s = src.as_bytes();
    transform(s, Span::new(0, s.len()), attr, 10.0, &budget).unwrap().map(|m| [m.a, m.b, m.c, m.d, m.e, m.f])
}

#[test]
fn transforms() {
    assert_eq!(tf("translate(10,20)", true), Some([1.0, 0.0, 0.0, 1.0, 10.0, 20.0]));
    // 区切りのない数・省いた `,`・関数の間の `,`
    assert_eq!(tf("translate(10-5)scale(2), scale(.5.5)", true), Some([1.0, 0.0, 0.0, 1.0, 10.0, -5.0]));
    assert_eq!(tf("matrix(1 0 0 1 5 6)", true), Some([1.0, 0.0, 0.0, 1.0, 5.0, 6.0]));
    let r = tf("rotate(90 10 10)", true).unwrap();
    assert!((r[0]).abs() < 1e-12 && (r[1] - 1.0).abs() < 1e-12 && (r[4] - 20.0).abs() < 1e-9 && r[5].abs() < 1e-9);
    assert_eq!(tf("", true), Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
    // CSS の書き方：単位が要り、`,` は省けない。em は文字の大きさから
    assert_eq!(tf("translate(10px, 2em) scale(2)", false), Some([2.0, 0.0, 0.0, 2.0, 10.0, 20.0]));
    assert_eq!(tf("rotate(90deg)", false).map(|m| m[1]), Some(1.0));
    assert_eq!(tf("none", false), Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
    for (bad, attr) in [
        ("translate(10 20)", false),
        ("rotate(90)", false),
        ("rotate(90 10)", true),
        ("translate(1,2,3)", true),
        ("bogus(1)", true),
        ("scale()", true),
        ("translate(10%)", true),
        ("matrix(1e999 0 0 1 0 0)", true),
        ("", false),
        ("translate(1) x", true),
    ] {
        assert_eq!(tf(bad, attr), None, "{bad}");
    }
}

#[test]
fn view_box_and_aspect() {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let vb = |s: &str| view_box(s.as_bytes(), Span::new(0, s.len()), &budget).unwrap();
    assert_eq!(vb("0 0 100 50"), Some(ViewBox { x: 0.0, y: 0.0, w: 100.0, h: 50.0 }));
    assert_eq!(vb("0,0,100,50"), Some(ViewBox { x: 0.0, y: 0.0, w: 100.0, h: 50.0 }));
    assert_eq!(vb("-1 -2 0 0"), Some(ViewBox { x: -1.0, y: -2.0, w: 0.0, h: 0.0 }));
    for bad in ["0 0 100", "0 0 -1 5", "0 0 1 1 1", "0 0 1 1,", "a b c d"] {
        assert_eq!(vb(bad), None, "{bad}");
    }
    let ar = |s: &str| aspect(s.as_bytes(), Span::new(0, s.len()), &budget).unwrap();
    assert_eq!(ar("xMinYMax slice"), Some(Aspect { align: Align::XMinYMax, slice: true }));
    assert_eq!(ar("defer none"), Some(Aspect { align: Align::None, slice: false }));
    assert_eq!(ar("xmidymid"), None);
    assert_eq!(ar("xMidYMid bogus"), None);
}
