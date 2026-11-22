#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::indexing_slicing)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;
use crate::style::props::{lookup, prop};

fn check(name: &str, value: &str, attr: bool, viewport: bool) -> bool {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let p = prop(lookup(name.as_bytes(), true).unwrap()).unwrap();
    let s = value.as_bytes();
    let cx = Ctx { attr, unitless: attr || p.unitless, viewport };
    valid(p.kind, s, Span::new(0, s.len()), cx, &budget).unwrap()
}

/// CSS の宣言として（大きさの決まる文書）。
fn ok(name: &str, value: &str) -> bool {
    check(name, value, false, true)
}

#[test]
fn values_by_property() {
    for (name, good, bad) in [
        ("fill", &["red", "none", "url(#a) blue", "currentColor", "context-fill"][..], &["", "1px", "red blue"][..]),
        ("opacity", &[".5", "50%", "2"], &["a", "1px"]),
        ("stroke-width", &["2", "2px", "10%", "0"], &["-1", "auto"]),
        // CSS の width・font-size・letter-spacing・transform-origin は単位のない数を受けない（Chrome と同じ）
        ("width", &["auto", "0", "5em", "10px"], &["-5", "10"]),
        ("stroke-miterlimit", &["4", "1"], &["0.5", "4px"]),
        ("stroke-linecap", &["round", "SQUARE"], &["rounded", "round round"]),
        ("display", &["none", "inline", "contents"], &["hidden"]),
        ("marker-start", &["none", "url(#m)", "url(\"#m\")"], &["#m", "url(#m) none"]),
        ("stroke-dasharray", &["none", "5 10", "5, 10%, 2"], &["5 -1", "5,", "red"]),
        (
            "font-family",
            &["Times New Roman, serif", "'A B', \"C\"", "default x"],
            &["a,", ",a", "1px", "'a' b", "a, inherit", "default"],
        ),
        ("font-size", &["medium", "12px", "1.5em", "larger"], &["-1px", "big", "12"]),
        ("font-weight", &["bold", "100", "950"], &["0", "1001", "heavy"]),
        ("letter-spacing", &["normal", "2px", "-1px"], &["wide", "-1"]),
        ("stroke-width", &["2", "2px"], &["-2"]),
        ("x", &["5", "-5", "5%"], &["auto"]),
        ("line-height", &["normal", "1.5", "12px", "150%"], &["-1", "a"]),
        ("color-rendering", &["optimizeQuality", "auto"], &["crispEdges"]),
        ("stroke-linejoin", &["arcs", "miter-clip", "round"], &["rounded"]),
        ("display", &["run-in", "inline"], &["compact", "marker"]),
        ("alignment-baseline", &["middle"], &["top", "center", "bottom"]),
        (
            "font",
            &[
                "12px serif",
                "italic bold 12px/1.5 'A B', serif",
                "small-caps 700 condensed 1em sans-serif",
                "caption",
                "normal normal normal normal 12px a",
                "oblique 10deg 12px a",
            ],
            &[
                "12px",
                "serif",
                "bold serif",
                "12 serif",
                "12px/ serif",
                "italic italic 12px a",
                "12px a, inherit",
                "normal normal normal normal normal 12px a",
                "caption serif",
            ],
        ),
        ("transform", &["rotate(45deg)", "none", "translate(10px, 5px) scale(2)"], &["rotate(45)", "translate(10 5)"]),
        (
            "transform-origin",
            &["center", "10px 20%", "left top 5px", "top left", "center center", "right 10px"],
            &["a", "10px 20px 5%", "top 10px", "left left", "10px left", "left top center", "10 20"],
        ),
        ("paint-order", &["normal", "stroke", "markers fill"], &["fill fill", "paint"]),
        ("filter", &["none", "url(#f)", "blur(2px) url(#g)"], &["bogus(1)", "red"]),
        ("clip-path", &["none", "url(#c)", "circle(5px) fill-box"], &["square(1)"]),
        ("d", &["none", "path(\"M0 0\")"], &["M0 0", "path(M0)"]),
        ("color", &["r\\65 d", "hsl(120deg 50% 50%)"], &["rgb(10deg, 20, 30)"]),
        ("text-decoration", &["underline red"], &[""]),
        ("color", &["#fff"], &["inherit-ish"]),
    ] {
        for v in good {
            assert!(ok(name, v), "{name}: {v} は正しい");
        }
        for v in bad {
            assert!(!ok(name, v), "{name}: {v} は壊れている");
        }
    }
}

#[test]
fn css_wide_keywords() {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let w = |s: &str| wide(s.as_bytes(), Span::new(0, s.len()), &budget).unwrap();
    assert_eq!(w(" inherit "), Some(Wide::Inherit));
    assert_eq!(w("INITIAL"), Some(Wide::Initial));
    assert_eq!(w("revert"), Some(Wide::Unset));
    assert_eq!(w("revert-layer"), Some(Wide::RevertLayer));
    assert_eq!(w("inherit red"), None);
    assert_eq!(w("red"), None);
    // transform 属性は SVG の書き方
    assert!(check("transform", "rotate(45) translate(1 2)", true, true));
    assert!(!check("transform", "rotate(45) translate(1 2)", false, true));
}

/// 属性はどの長さも単位のない数を受ける。`d` 属性はパスの書き方（形はパスを作る段で確かめる）。表示の領域の単位は
/// 大きさの決まる文書だけ。
#[test]
fn attributes_and_viewport() {
    assert!(check("width", "10", true, true));
    assert!(check("font-size", "12", true, true));
    assert!(check("transform-origin", "10 20", true, true));
    assert!(check("d", "M0 0 L10 10 Z", true, true));
    assert!(check("d", "garbage", true, true));
    assert!(ok("width", "10vw"));
    assert!(!check("width", "10vw", false, false));
    assert!(!check("x", "5vmin", true, false));
    assert!(!check("font", "1vh serif", false, false));
}
