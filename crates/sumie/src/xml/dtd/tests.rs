#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;

#[test]
fn sorted_and_first_wins() {
    let src = b"<!ENTITY b \"2\"><!ENTITY a \"1\"><!ENTITY b \"3\">";
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let t = entities(src, Span::new(0, src.len()), &budget).unwrap();
    let names: Vec<&[u8]> = t.as_slice().iter().map(|e| e.name.of(src)).collect();
    assert_eq!(names, [b"a".as_slice(), b"b", b"b"]);
    let b = lookup(src, t.as_slice(), b"b", &budget).unwrap().unwrap();
    assert_eq!(b.value.of(src), b"2");
    assert!(lookup(src, t.as_slice(), b"c", &budget).unwrap().is_none());
}

#[test]
fn broken_declarations() {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    for src in [
        b"<!ENTITY a \"1\"".as_slice(),
        b"<!ENTITY \"1\">",
        b"<!ENTITY a \"1>",
        b"<!ELEMENT a",
        b"<!-- x",
        b"x",
        b"<!FOO a>",
        b"<!ENTITYa \"1\">",
        b"<!ENTITY a\"1\">",
        b"<!ENTITY a junk>",
        b"<!ATTLIST a b CDATA \"x>y\"",
    ] {
        assert!(entities(src, Span::new(0, src.len()), &budget).is_err(), "{:?}", String::from_utf8_lossy(src));
    }
}

/// 読み直すバイトは塊ずつ払う（1 回の払いは塊を超えない）。
#[test]
fn pays_in_chunks() {
    let mut src = b"<!ATTLIST a b CDATA \"".to_vec();
    src.extend(std::iter::repeat_n(b'x', 200_000));
    src.extend(b"\"><!ENTITY e \"");
    src.extend(std::iter::repeat_n(b'y', 200_000));
    src.extend(b"\">");
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let t = entities(&src, Span::new(0, src.len()), &budget).unwrap();
    assert_eq!(t.len(), 1);
    assert!(budget.work() >= src.len() as u64);
    assert!(budget.max_spend() <= crate::xml::lex::SCAN_CHUNK as u64);
}
