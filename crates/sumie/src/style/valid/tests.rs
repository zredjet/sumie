#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::indexing_slicing)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;
use crate::style::props::{lookup, prop};

fn ok(name: &str, value: &str) -> bool {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let kind = prop(lookup(name.as_bytes(), true).unwrap()).unwrap().kind;
    let s = value.as_bytes();
    valid(kind, s, Span::new(0, s.len()), false, &budget).unwrap()
}

#[test]
fn values_by_property() {
    for (name, good, bad) in [
        ("fill", &["red", "none", "url(#a) blue", "currentColor", "context-fill"][..], &["", "1px", "red blue"][..]),
        ("opacity", &[".5", "50%", "2"], &["a", "1px"]),
        ("stroke-width", &["2", "2px", "10%", "0"], &["-1", "auto"]),
        ("width", &["auto", "10", "5em"], &["-5"]),
        ("stroke-miterlimit", &["4", "1"], &["0.5", "4px"]),
        ("stroke-linecap", &["round", "SQUARE"], &["rounded", "round round"]),
        ("display", &["none", "inline", "contents"], &["hidden"]),
        ("marker-start", &["none", "url(#m)", "url(\"#m\")"], &["#m", "url(#m) none"]),
        ("stroke-dasharray", &["none", "5 10", "5, 10%, 2"], &["5 -1", "5,", "red"]),
        ("font-family", &["Times New Roman, serif", "'A B', \"C\""], &["a,", ",a", "1px"]),
        ("font-size", &["medium", "12", "1.5em", "larger"], &["-1px", "big"]),
        ("font-weight", &["bold", "100", "950"], &["0", "1001", "heavy"]),
        ("letter-spacing", &["normal", "2px", "-1"], &["wide"]),
        ("transform", &["rotate(45deg)", "none", "translate(10px, 5px) scale(2)"], &["rotate(45)", "translate(10 5)"]),
        ("transform-origin", &["center", "10px 20%", "left top 5px"], &["a", "10px 20px 5%"]),
        ("paint-order", &["normal", "stroke", "markers fill"], &["fill fill", "paint"]),
        ("filter", &["none", "url(#f)", "blur(2px) url(#g)"], &["bogus(1)", "red"]),
        ("clip-path", &["none", "url(#c)", "circle(5px) fill-box"], &["square(1)"]),
        ("d", &["none", "path(\"M0 0\")"], &["M0 0", "path(M0)"]),
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
    assert_eq!(w("revert-layer"), Some(Wide::Unset));
    assert_eq!(w("inherit red"), None);
    assert_eq!(w("red"), None);
    // transform 属性は SVG の書き方
    let kind = prop(lookup(b"transform", false).unwrap()).unwrap().kind;
    let s = b"rotate(45) translate(1 2)";
    assert!(valid(kind, s, Span::new(0, s.len()), true, &budget).unwrap());
    assert!(!valid(kind, s, Span::new(0, s.len()), false, &budget).unwrap());
}
