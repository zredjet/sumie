#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::disallowed_types, clippy::indexing_slicing)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;

fn decls(src: &str, in_block: bool) -> (Vec<(String, String, bool)>, Kind) {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let s = src.as_bytes();
    let mut lex = Lexer::new(s, &budget);
    let mut out = Vec::new();
    let end = parse_declarations(&mut lex, s, in_block, &budget, &mut |d| {
        let name: String = crate::css::lex::decode(s, d.name).map(|c| char::from_u32(c).unwrap()).collect();
        out.push((name, String::from_utf8_lossy(d.value.of(s)).to_string(), d.important != 0));
        Ok(())
    })
    .unwrap();
    (out, end.kind)
}

#[test]
fn declarations() {
    let (d, end) = decls(" fill : red ; stroke:url(#a;b) !important;; opacity: .5 ! IMPORTANT ", false);
    assert_eq!(
        d,
        [
            ("fill".into(), "red".into(), false),
            ("stroke".into(), "url(#a;b)".into(), true),
            ("opacity".into(), ".5".into(), true),
        ]
    );
    assert_eq!(end, Kind::Eof);
    // 壊れた宣言は `;` まで飛ばす・かっこの中の `;` は区切りでない・アットルールは飛ばす
    let (d, _) = decls("1px: x; fill red; @foo { a: b } ; f\\69ll: \"a;b\"; x: (a;b) c", false);
    assert_eq!(d, [("fill".into(), "\"a;b\"".into(), false), ("x".into(), "(a;b) c".into(), false)]);
    // 規則の中：`}` で止まる
    let (d, end) = decls("a: b; c: d } e: f", true);
    assert_eq!(d, [("a".into(), "b".into(), false), ("c".into(), "d".into(), false)]);
    assert_eq!(end, Kind::RBrace);
    // 空の値・`!important` だけ
    let (d, _) = decls("a:; b: !important; c: x!important", false);
    assert_eq!(d, [("a".into(), "".into(), false), ("b".into(), "".into(), true), ("c".into(), "x".into(), true)]);
}
