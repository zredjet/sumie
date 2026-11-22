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
    // 単位を換えたあとも有限のものだけ
    assert_eq!(ang("1e308rad"), None);
    assert_eq!(len("1e307in", false).unwrap().absolute_px(), None);
    // フォントの要る単位と表示の領域の単位（ADR-74）
    assert_eq!(len("2ch", false), Some(Length::new(2.0, Unit::Ch)));
    assert_eq!(len("1IC", false), Some(Length::new(1.0, Unit::Ic)));
    assert_eq!(len("1cap", false), Some(Length::new(1.0, Unit::Cap)));
    assert_eq!(len("1lh", false), Some(Length::new(1.0, Unit::Lh)));
    assert_eq!(len("1rlh", false), Some(Length::new(1.0, Unit::Rlh)));
    assert_eq!(len("5vi", false), Some(Length::new(5.0, Unit::Vi)));
    assert!(Unit::Vb.viewport() && Unit::Vw.viewport() && !Unit::Em.viewport());
}

/// 色の関数：角度は色相だけに書ける。名前と 16 進の escape は解く（Chrome と同じ）。
#[test]
fn color_angles_and_escapes() {
    for bad in ["rgb(10deg,20,30)", "rgb(1turn 0 0)", "hsl(120 50deg 50%)", "hwb(120 10deg 20%)"] {
        assert_eq!(color(bad), None, "{bad}");
    }
    assert!(color("hsl(120deg 50% 50%)").is_some());
    assert!(color("hwb(0.5turn 10% 20%)").is_some());
    assert_eq!(color("r\\65 d"), Some((255.0, 0.0, 0.0, 1.0)));
    assert_eq!(color("#\\66 00"), Some((255.0, 0.0, 0.0, 1.0)));
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

fn tf(src: &str) -> Option<[f64; 6]> {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let s = src.as_bytes();
    transform_attr(s, Span::new(0, s.len()), &budget).unwrap().map(|m| [m.a, m.b, m.c, m.d, m.e, m.f])
}

fn tcss(src: &str, viewport: bool) -> bool {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let s = src.as_bytes();
    transform_css_valid(s, Span::new(0, s.len()), viewport, &budget).unwrap()
}

/// `transform` 属性は SVG 1.1 の書き方だけ（Chrome 157 で照らした）。
#[test]
fn transform_attribute() {
    assert_eq!(tf("translate(10,20)"), Some([1.0, 0.0, 0.0, 1.0, 10.0, 20.0]));
    // 区切りのない数・省いた `,`・関数の間の `,`・名前と `(` の間の空白
    assert_eq!(tf("translate(10-5)scale(2), scale(.5.5)"), Some([1.0, 0.0, 0.0, 1.0, 10.0, -5.0]));
    assert_eq!(tf("matrix(1 0 0 1 5 6)"), Some([1.0, 0.0, 0.0, 1.0, 5.0, 6.0]));
    assert_eq!(tf("translate(10, 20) , scale(2)"), Some([2.0, 0.0, 0.0, 2.0, 10.0, 20.0]));
    assert_eq!(tf("translate(+.5e1)"), Some([1.0, 0.0, 0.0, 1.0, 5.0, 0.0]));
    let r = tf("rotate (45)").unwrap();
    assert!((r[0] - core::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    let r = tf("rotate(90 10 10)").unwrap();
    assert!((r[0]).abs() < 1e-12 && (r[1] - 1.0).abs() < 1e-12 && (r[4] - 20.0).abs() < 1e-9 && r[5].abs() < 1e-9);
    assert_eq!(tf(""), Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
    assert_eq!(tf(" "), Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
    for bad in [
        "rotate(45deg)",
        "translate(10px)",
        "translate(1in)",
        "translateX(5)",
        "skew(30)",
        "scaleX(2)",
        "TRANSLATE(5)",
        "translate(10 /**/ 20)",
        "scale(2,)",
        "translate(10,,20)",
        ", translate(1)",
        "translate(1),",
        "translate(1e400)",
        "rotate(45 10)",
        "scale(50%)",
        "translate(1) none",
        "translate(1,2,3)",
        "bogus(1)",
        "scale()",
        "matrix(1e999 0 0 1 0 0)",
        "translate(1) x",
        "tr\\61nslate(1)",
    ] {
        assert_eq!(tf(bad), None, "{bad}");
    }
}

/// CSS の `transform` は CSS Transforms の 2 次元の関数（長さはどの単位も受ける）。
#[test]
fn transform_property() {
    for good in [
        "none",
        "translate(10px, 2em) scale(2)",
        "rotate(90deg)",
        "translate(-50%, -50%)",
        "translate(1rem) translate(1ex) translate(1ch)",
        "scale(50%) rotateZ(1turn)",
        "skew(10deg, 5deg) translateY(0)",
        "matrix(1, 0, 0, 1, 0, 0)",
        "translate(10vw)",
    ] {
        assert!(tcss(good, true), "{good}");
    }
    for bad in [
        "translate(10 20)",
        "rotate(90)",
        "translate(10px 5px)",
        "rotateX(10deg)",
        "translate3d(1px, 2px, 3px)",
        "",
        "rotate(90deg) none",
        "translate(1px,)",
        "scale(1px)",
    ] {
        assert!(!tcss(bad, true), "{bad}");
    }
    // 大きさの決まらない文書では vw は壊れた値
    assert!(!tcss("translate(10vw)", false));
}

#[test]
fn view_box_and_aspect() {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let vb = |s: &str| view_box(s.as_bytes(), Span::new(0, s.len()), &budget).unwrap();
    assert_eq!(vb("0 0 100 50"), Some(ViewBox { x: 0.0, y: 0.0, w: 100.0, h: 50.0 }));
    assert_eq!(vb("0,0,100,50"), Some(ViewBox { x: 0.0, y: 0.0, w: 100.0, h: 50.0 }));
    assert_eq!(vb("-1 -2 0 0"), Some(ViewBox { x: -1.0, y: -2.0, w: 0.0, h: 0.0 }));
    for bad in ["0 0 100", "0 0 -1 5", "0 0 1 1 1", "0 0 1 1,", "a b c d", "0 0 /**/ 10 10"] {
        assert_eq!(vb(bad), None, "{bad}");
    }
    let ar = |s: &str| aspect(s.as_bytes(), Span::new(0, s.len()), &budget).unwrap();
    assert_eq!(ar("xMinYMax slice"), Some(Aspect { align: Align::XMinYMax, slice: true }));
    assert_eq!(ar(" none meet "), Some(Aspect { align: Align::None, slice: false }));
    // 言葉は大文字小文字を区別し、SVG 2 で除かれた defer は書けない（Chrome と同じ）
    for bad in ["xmidymid", "xMidYMid bogus", "xMidYMid SLICE", "defer none", "xMidYMid meet slice"] {
        assert_eq!(ar(bad), None, "{bad}");
    }
}

/// viewBox の変形（SVG 2 §8.2）：揃え方・meet・slice・none。
#[test]
fn view_box_transforms() {
    let vb = ViewBox { x: 10.0, y: 0.0, w: 20.0, h: 10.0 };
    let t = |a: Align, slice: bool| view_box_transform(&vb, Aspect { align: a, slice }, 100.0, 100.0).unwrap();
    // meet：小さい方の倍率（5）で、真ん中に揃える（縦に 50 の余り）
    assert_eq!(t(Align::XMidYMid, false), Transform { a: 5.0, b: 0.0, c: 0.0, d: 5.0, e: -50.0, f: 25.0 });
    assert_eq!(t(Align::XMinYMax, false), Transform { a: 5.0, b: 0.0, c: 0.0, d: 5.0, e: -50.0, f: 50.0 });
    // slice：大きい方の倍率（10）で、横にはみ出す（-100 の余り）
    assert_eq!(t(Align::XMaxYMin, true), Transform { a: 10.0, b: 0.0, c: 0.0, d: 10.0, e: -200.0, f: 0.0 });
    // none：縦横べつの倍率
    assert_eq!(t(Align::None, false), Transform { a: 5.0, b: 0.0, c: 0.0, d: 10.0, e: -50.0, f: 0.0 });
    // 幅か高さが 0 の viewBox は描かない
    assert_eq!(view_box_transform(&ViewBox { w: 0.0, ..vb }, Aspect::DEFAULT, 1.0, 1.0), None);
}
