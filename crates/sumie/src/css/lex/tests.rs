#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use super::*;
use crate::budget::Budget;
use crate::cancel::Never;
use crate::error::Stage;

fn tokens(src: &str) -> Vec<Token> {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut lex = Lexer::new(src.as_bytes(), &budget);
    let mut out = Vec::new();
    loop {
        let t = lex.next().unwrap();
        if t.kind == Kind::Eof {
            assert_eq!(lex.next().unwrap().kind, Kind::Eof);
            return out;
        }
        assert!(t.span.end > t.span.start, "進まない：{t:?}");
        out.push(t);
        assert!(out.len() < 10_000);
    }
}

fn kinds(src: &str) -> Vec<Kind> {
    tokens(src).iter().map(|t| t.kind).filter(|&k| k != Kind::Whitespace).collect()
}

fn text(src: &str, t: &Token) -> String {
    let d: Vec<u32> = if matches!(t.kind, Kind::Str | Kind::Url) {
        decode_string(src.as_bytes(), t.inner).collect()
    } else {
        decode(src.as_bytes(), t.inner).collect()
    };
    d.iter().map(|&c| char::from_u32(c).unwrap()).collect()
}

#[test]
fn basic_kinds() {
    use Kind::*;
    assert_eq!(
        kinds("rect.a > #b, [x=\"y\"] { fill: rgb(1, 2%, 3px) !important; } @media screen {}"),
        [
            Ident, Delim, Ident, Delim, Hash, Comma, LBracket, Ident, Delim, Str, RBracket, LBrace, Ident, Colon,
            Function, Number, Comma, Percentage, Comma, Dimension, RParen, Delim, Ident, Semicolon, RBrace, AtKeyword,
            Ident, LBrace, RBrace
        ]
    );
    assert_eq!(kinds("<!-- a -->"), [Cdo, Ident, Cdc]);
    assert_eq!(kinds("/* c */a/* unclosed"), [Ident]);
    assert_eq!(kinds("url(a b) url( x.png ) url(\"q\") url(a\"b)"), [BadUrl, Url, Function, Str, RParen, BadUrl]);
    // 改行で壊れた文字列のあとの `'` は、終わりまでの文字列を始める
    assert_eq!(kinds("'a\nb'"), [BadString, Ident, Str]);
}

#[test]
fn numbers() {
    let t = tokens("1 -2.5 +.5e2 1e999 10px 50% 3e 1.");
    let n: Vec<(Kind, f64, bool, bool)> =
        t.iter().filter(|t| t.kind != Kind::Whitespace).map(|t| (t.kind, t.value, t.flag, t.finite)).collect();
    assert_eq!(n[0], (Kind::Number, 1.0, true, true));
    assert_eq!(n[1], (Kind::Number, -2.5, false, true));
    assert_eq!(n[2], (Kind::Number, 50.0, false, true));
    assert_eq!(n[3], (Kind::Number, 0.0, false, false));
    assert_eq!(n[4].0, Kind::Dimension);
    assert_eq!(n[5], (Kind::Percentage, 50.0, true, true));
    // `3e` は単位 e の dimension、`1.` は数と delim
    assert_eq!(n[6].0, Kind::Dimension);
    assert_eq!((n[7].0, n[8].0), (Kind::Number, Kind::Delim));
}

#[test]
fn escapes_and_names() {
    let src = ".\\61 b #\\72  \\31 23 r\\65 ct 'a\\\nb\\\\' \"\\0\" -\\-x --y a\\";
    let t: Vec<Token> = tokens(src).into_iter().filter(|t| t.kind != Kind::Whitespace).collect();
    assert_eq!(text(src, &t[1]), "ab");
    assert!(t[1].escaped);
    assert_eq!((t[2].kind, text(src, &t[2])), (Kind::Hash, "r".to_string()));
    assert_eq!(text(src, &t[3]), "123");
    assert_eq!(text(src, &t[4]), "rect");
    assert_eq!(text(src, &t[5]), "ab\\");
    assert_eq!(text(src, &t[6]), "\u{FFFD}");
    assert_eq!((t[7].kind, text(src, &t[7])), (Kind::Ident, "--x".to_string()));
    assert_eq!((t[8].kind, text(src, &t[8])), (Kind::Ident, "--y".to_string()));
    // 終わりの `\` は名前の中で U+FFFD
    assert_eq!(text(src, &t[9]), "a\u{FFFD}");
    assert!(ident_is(src.as_bytes(), t[4].inner, t[4].escaped, b"rect"));
    assert!(!ident_is(src.as_bytes(), t[4].inner, t[4].escaped, b"rec"));
    // hash の形：識別子でない頭は id の印がない
    let t = tokens("#1a #-x #a");
    assert_eq!(t.iter().filter(|t| t.kind == Kind::Hash).map(|t| t.flag).collect::<Vec<_>>(), [false, true, true]);
}

/// どの 3 バイトの並びでも進み、払いが中止の間隔を超えない。
#[test]
fn all_three_byte_sequences() {
    let alphabet = b"\\/*\"'()-+.#@<!e0 \n%a;";
    for &x in alphabet {
        for &y in alphabet {
            for &z in alphabet {
                let s = [x, y, z];
                let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
                let mut lex = Lexer::new(&s, &budget);
                let mut n = 0;
                while lex.next().unwrap().kind != Kind::Eof {
                    n += 1;
                    assert!(n < 10);
                }
            }
        }
    }
}

#[test]
fn pays_in_chunks() {
    let src = format!("a{{fill:{}}}", "1".repeat(300_000));
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut lex = Lexer::new(src.as_bytes(), &budget);
    while lex.next().unwrap().kind != Kind::Eof {}
    assert!(budget.work() >= src.len() as u64);
    assert!(budget.max_spend() < crate::budget::CANCEL_INTERVAL);
}
