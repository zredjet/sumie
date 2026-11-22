#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::indexing_slicing)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;

fn m(q: &str, size: Option<(f64, f64)>, scheme: ColorScheme) -> bool {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let env = Env { color_scheme: scheme, size };
    media(q.as_bytes(), Span::new(0, q.len()), &env, &budget).unwrap()
}

#[test]
fn media_queries() {
    let s = Some((100.0, 50.0));
    let l = ColorScheme::Light;
    for (q, want) in [
        ("", true),
        ("screen", true),
        ("all", true),
        ("print", false),
        ("SCREEN", true),
        ("not print", true),
        ("not screen", false),
        ("only screen", true),
        ("print, screen", true),
        ("screen and (min-width: 50px)", true),
        ("screen and (min-width: 200px)", false),
        ("(max-width: 100px)", true),
        ("(width >= 101px)", false),
        ("(width > 99px)", true),
        ("(50px <= width <= 150px)", true),
        ("(width < 100px)", false),
        ("(width <= 100px)", true),
        ("(orientation: landscape)", true),
        ("(min-aspect-ratio: 1)", true),
        ("(prefers-color-scheme: dark)", false),
        ("(prefers-color-scheme: light)", true),
        ("(prefers-reduced-motion: reduce)", false),
        ("(prefers-reduced-motion)", false),
        ("(color)", true),
        ("(hover)", false),
        ("(bogus: 1)", false),
        ("not (bogus: 1)", false),
        ("(min-width: 10em)", false),
        ("(min-width: 3em)", true),
        ("not ((width < 10px) or (height < 10px))", true),
        ("(width > 10px) and (height > 60px)", false),
        ("screen and print", false),
        ("garbage!, screen", true),
        ("(width = 100px)", true),
        ("(width < = 100px)", false),
    ] {
        assert_eq!(m(q, s, l), want, "{q}");
    }
    // 大きさが決まらない文書では大きさの条件は偽
    assert!(!m("(min-width: 1px)", None, l));
    assert!(!m("not (min-width: 1px)", None, l));
    assert!(m("(prefers-color-scheme: dark)", None, ColorScheme::Dark));
}

#[test]
fn supports_conditions() {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let sp = |q: &str| supports(q.as_bytes(), Span::new(0, q.len()), &budget).unwrap();
    assert!(sp("(fill: green)"));
    assert!(sp("(FILL: inherit)"));
    assert!(!sp("(fill: 1px)"));
    assert!(!sp("(bogus: 1)"));
    assert!(sp("not (bogus: 1)"));
    assert!(sp("(fill: red) and (stroke-width: 2px)"));
    assert!(sp("(bogus: 1) or (fill: red)"));
    assert!(!sp("(fill: red) and (bogus: 1)"));
    assert!(sp("selector(rect > g:is(.a))"));
    assert!(!sp("selector(rect:bogus)"));
    assert!(!sp("(fill: red) and (stroke: red) or (opacity: 1)"));
    assert!(sp("((fill: red) or (bogus: 1)) and (opacity: 1)"));
    assert!(!sp("unknown(1)"));
}
