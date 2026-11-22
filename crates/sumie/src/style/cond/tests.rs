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
    let sp =
        |q: &str| supports(q.as_bytes(), Span::new(0, q.len()), crate::css::select::NsTable::EMPTY, &budget).unwrap();
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

/// 書き方の確かめ（Media Queries 4 の文法。Chrome 157 と同じ）と、特徴ごとの値の形。
#[test]
fn media_grammar_and_value_types() {
    let s = Some((100.0, 50.0));
    let l = ColorScheme::Light;
    for (q, want) in [
        ("(min-aspect-ratio: 1/1)", true),
        ("(aspect-ratio: 2/1)", true),
        ("(aspect-ratio: 2 / 1)", true),
        ("(max-aspect-ratio: 1/1)", false),
        ("(aspect-ratio > 1/1)", true),
        ("screen and (color) or (monochrome)", false),
        ("screen and ((color) or (monochrome))", true),
        ("not (monochrome) and (color)", false),
        ("(color) and not (monochrome)", false),
        ("(100px <= width >= 50px)", false),
        ("(50px < width < 150px)", true),
        ("(150px > width > 50px)", true),
        ("(50px = width = 50px)", false),
        ("(min-width: 100)", false),
        ("(min-width: 0)", true),
        ("(resolution: 1px)", false),
        ("(resolution: 96dpi)", true),
        ("(min-resolution: 1x)", true),
        ("(color: 8.0)", false),
        ("(color: 8)", true),
        ("((color) foo, (color)", false),
        ("(color) foo, (color)", true),
    ] {
        assert_eq!(m(q, s, l), want, "{q}");
    }
}

#[test]
fn supports_general_enclosed_and_important() {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let sp =
        |q: &str| supports(q.as_bytes(), Span::new(0, q.len()), crate::css::select::NsTable::EMPTY, &budget).unwrap();
    // 知らない形は偽（CSS Conditional 3 §6.1）
    assert!(sp("not (foo)"));
    assert!(sp("(fill: red) and (not (foo))"));
    assert!(!sp("(foo)"));
    assert!(sp("not unknown(1)"));
    // `!important` と名前の escape
    assert!(sp("(fill: red !important)"));
    assert!(sp("(f\\69ll: red)"));
    // `not` の段に and を書けない
    assert!(!sp("not (fill: red) and (stroke: red)"));
    // 表示の領域の単位は形だけを見る
    assert!(sp("(width: 10vw)"));
}

#[test]
fn supports_selector_and_custom_properties() {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let sp =
        |q: &str| supports(q.as_bytes(), Span::new(0, q.len()), crate::css::select::NsTable::EMPTY, &budget).unwrap();
    assert!(sp("(--x: 1)"));
    assert!(sp("(selector(a))"));
    // 1 つの複雑なセレクタだけ。当てないセレクタは読めないもの
    assert!(!sp("selector(rect, a)"));
    assert!(!sp("selector(:has(a))"));
    assert!(sp("not selector(:has(a))"));
}
