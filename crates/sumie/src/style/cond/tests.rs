#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::indexing_slicing)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;

fn m(q: &str, size: Option<(f64, f64)>, scheme: ColorScheme) -> bool {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let env = Env { color_scheme: scheme, size, font_size: 16.0 };
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
    let sp = |q: &str| {
        supports(q.as_bytes(), Span::new(0, q.len()), crate::css::select::NsTable::EMPTY, &budget).unwrap() == Tri::True
    };
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
    let sp = |q: &str| {
        supports(q.as_bytes(), Span::new(0, q.len()), crate::css::select::NsTable::EMPTY, &budget).unwrap() == Tri::True
    };
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
    let sp = |q: &str| {
        supports(q.as_bytes(), Span::new(0, q.len()), crate::css::select::NsTable::EMPTY, &budget).unwrap() == Tri::True
    };
    assert!(sp("(--x: 1)"));
    assert!(sp("(selector(a))"));
    // 1 つの複雑なセレクタだけ。当てないセレクタ（`:has()` など）は読めないので分からない（単位 3 の 9 回目のレビュー）
    assert!(!sp("selector(rect, a)"));
    let tri =
        |q: &str| supports(q.as_bytes(), Span::new(0, q.len()), crate::css::select::NsTable::EMPTY, &budget).unwrap();
    assert_eq!(tri("selector(:has(a))"), Tri::Unknown);
    assert_eq!(tri("not selector(:has(a))"), Tri::Unknown);
    assert_eq!(tri("font-tech(color-COLRv1)"), Tri::Unknown);
    assert_eq!(tri("foo(bar)"), Tri::False);
}

/// かっこの中が条件として読めなければ、そのかっこは知らない形として続ける（Media Queries 4・CSS Conditional 3）。言葉の
/// 特徴の知らない値は分からない。長さの単位は初期の文字の大きさと表示の領域で解く。
#[test]
fn general_enclosed_and_feature_values() {
    let s = Some((100.0, 50.0));
    let l = ColorScheme::Light;
    for (q, want) in [
        ("(not foo) or (min-width: 1px)", true),
        ("((color) foo) or (color)", true),
        ("not (prefers-color-scheme: bogus)", false),
        ("not (orientation: bogus)", false),
        ("(prefers-color-scheme: light)", true),
        ("(min-width: 12ex)", true),
        ("(min-width: 13ex)", false),
        ("(width: 100vw)", true),
        ("(height: 100vh)", true),
        ("(overflow-block: scroll)", true),
        ("(-webkit-min-device-pixel-ratio: 1)", true),
        ("(not (color)) or (color)", true),
    ] {
        assert_eq!(m(q, s, l), want, "{q}");
    }
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let sp = |q: &str| {
        supports(q.as_bytes(), Span::new(0, q.len()), crate::css::select::NsTable::EMPTY, &budget).unwrap() == Tri::True
    };
    assert!(sp("(not foo) or (fill: red)"));
    assert!(!sp("(fill: red;)"));
    // 閉じないまま終わったかっこは閉じたものとする（CSS Syntax。単位 3 の 9 回目のレビュー）
    assert!(sp("((fill: red)"));
    assert!(sp("(fill: red) and ((fill: blue)"));
    assert!(!sp("(fill: red) and ((fill: banana)"));
    let tri =
        |q: &str| supports(q.as_bytes(), Span::new(0, q.len()), crate::css::select::NsTable::EMPTY, &budget).unwrap();
    assert_eq!(tri("((rotate: 1deg)"), Tri::Unknown);
    // @import の supports(…)：宣言の形・条件の形・どちらでもない形
    let imp = |q: &str| {
        supports_import(q.as_bytes(), Span::new(0, q.len()), crate::css::select::NsTable::EMPTY, &budget).unwrap()
    };
    assert_eq!(imp("fill: red"), Tri::True);
    assert_eq!(imp("fill: banana"), Tri::False);
    assert_eq!(imp("foo: bar"), Tri::False);
    assert_eq!(imp("rotate: 1deg"), Tri::Unknown);
    assert_eq!(imp("(fill: red) and (rotate: 1deg)"), Tri::Unknown);
    assert_eq!(imp("fill red"), Tri::False);
    assert_eq!(imp("(fill: red) banana"), Tri::False);
}

/// `not` の先読みは同じ字句の口で行い、`not` の長い並びでも手間は長さに比例する。
#[test]
fn many_not_queries_stay_linear() {
    let q = format!("{} screen", "not a,".repeat(20_000));
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let env = Env { color_scheme: ColorScheme::Light, size: None, font_size: 16.0 };
    assert!(media(q.as_bytes(), Span::new(0, q.len()), &env, &budget).unwrap());
    assert!(budget.work() < 40 * q.len() as u64, "{}", budget.work());
}
