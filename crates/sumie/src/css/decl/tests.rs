#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::disallowed_types, clippy::indexing_slicing)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;
use crate::xml::lex::Span;

fn decls(src: &str, in_block: bool) -> (Vec<(String, String, bool)>, Kind) {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let s = src.as_bytes();
    let mut lex = Lexer::over(s, Span::new(0, s.len()), &budget);
    let mut out = Vec::new();
    let end = parse_declarations(&mut lex, s, in_block, &budget, &mut |it| {
        match it {
            Item::Decl(d) => {
                let name: String = crate::css::lex::decode(s, d.name).map(|c| char::from_u32(c).unwrap()).collect();
                out.push((name, String::from_utf8_lossy(d.value.of(s)).to_string(), d.important != 0));
            }
            Item::Nested => out.push(("{nested}".into(), String::new(), false)),
        }
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

fn names(src: &str, in_block: bool) -> Vec<String> {
    decls(src, in_block).0.into_iter().map(|(n, v, _)| if v.is_empty() { n } else { format!("{n}={v}") }).collect()
}

/// 規則の中の入れ子の規則（CSS Nesting）は塊までで終え、あとの宣言を読む（Chrome と同じ）。
#[test]
fn nested_rules_in_block() {
    assert_eq!(names("x { } fill: green", true), ["{nested}", "fill=green"]);
    assert_eq!(names("fill: red; x: y {a} fill: green", true), ["fill=red", "{nested}", "fill=green"]);
    assert_eq!(names(".a { } ; fill: green", true), ["{nested}", "fill=green"]);
    assert_eq!(names("& x { a: b } fill: green", true), ["{nested}", "fill=green"]);
    assert_eq!(names("@media all { } fill: green", true), ["{nested}", "fill=green"]);
    assert_eq!(names("fill: green; @foo; ", true), ["fill=green", "{nested}"]);
    // `--` の名前は値に塊を持てる
    assert_eq!(names("--x: {a} ; fill: green", true), ["--x={a}", "fill=green"]);
    // `:` のない名前の後ろの `;` までは壊れた宣言
    assert_eq!(names("fill green; stroke: blue", true), ["stroke=blue"]);
    // 入れ子の塊の `}` で外の並びが終わらない
    let (d, end) = decls("x { } } y: z", true);
    assert_eq!(d.len(), 1);
    assert_eq!(end, Kind::RBrace);
    // 閉じない塊
    let (d, end) = decls("x { a: b", true);
    assert_eq!(d, [("{nested}".into(), "".into(), false)]);
    assert_eq!(end, Kind::Eof);
}

/// かっこは種類ごとに組で読む（中の別の種類の閉じはただの字句）。
#[test]
fn brackets_pair_by_kind() {
    // `(` の中の `]`・`;`・`}` は値の続き（終わりまで 1 つの値）
    assert_eq!(names("stroke: (] ; fill: green }", true), ["stroke=(] ; fill: green }"]);
    assert_eq!(names("x: [) } y: z", true), ["x=[) } y: z"]);
    // 深さ 0 の `)`・`]` はただの字句
    assert_eq!(names("a: ) ] b; c: d", true), ["a=) ] b", "c=d"]);
}

/// `style` 属性：入れ子の規則を読まない。深さ 0 の `}` はただの字句。アットルールは `{…}` か `;` まで（Chrome と同じ）。
#[test]
fn style_attribute_recovery() {
    assert!(names("x {} fill: green", false).is_empty());
    assert_eq!(names("fill: blue } ; fill: green", false), ["fill=blue }", "fill=green"]);
    assert_eq!(names("x: y {a} ; fill: green", false), ["x=y {a}", "fill=green"]);
    assert_eq!(names("x: y {a} fill: green", false), ["x=y {a} fill: green"]);
    assert_eq!(names("@foo {} fill: green", false), ["fill=green"]);
    assert_eq!(names("fill: green; @foo {} stroke: blue", false), ["fill=green", "stroke=blue"]);
}

/// `!important` の前の値の終わりは、空白でない最後の字句まで（後ろへ走査しない）。
#[test]
fn important_value_end() {
    let (d, _) = decls("a: x  y \t ! important; b: ! important", false);
    assert_eq!(d, [("a".into(), "x  y".into(), true), ("b".into(), "".into(), true)]);
}
