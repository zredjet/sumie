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
    let t = parse_subset(src, Span::new(0, src.len()), &budget).unwrap().entities;
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
        b"<!ATTLIST a b CDATA \"x<y\">\"",
    ] {
        assert!(parse_subset(src, Span::new(0, src.len()), &budget).is_err(), "{:?}", String::from_utf8_lossy(src));
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
    let t = parse_subset(&src, Span::new(0, src.len()), &budget).unwrap().entities;
    assert_eq!(t.len(), 1);
    assert!(budget.work() >= src.len() as u64);
    assert!(budget.max_spend() <= crate::xml::lex::SCAN_CHUNK as u64);
}

#[test]
fn attlists_are_read_and_sorted() {
    let src = b"<!ATTLIST g b CDATA 'x>y' a NMTOKENS #IMPLIED c ID #REQUIRED><!ATTLIST a z (p|q) #FIXED \"p\" b NOTATION (n) 'n'><!ATTLIST g b CDATA 'later'>";
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let t = parse_subset(src, Span::new(0, src.len()), &budget).unwrap().attlists;
    type Row<'a> = (&'a [u8], &'a [u8], Option<&'a [u8]>, u32);
    let rows: Vec<Row<'_>> = t
        .as_slice()
        .iter()
        .map(|d| (d.element.of(src), d.name.of(src), (d.has_default == 1).then(|| d.value.of(src)), d.tokens))
        .collect();
    assert_eq!(
        rows,
        [
            (b"a".as_slice(), b"b".as_slice(), Some(b"n".as_slice()), 1),
            (b"a", b"z", Some(b"p"), 1),
            (b"g", b"a", None, 1),
            (b"g", b"b", Some(b"x>y"), 0),
            (b"g", b"b", Some(b"later"), 0),
            (b"g", b"c", None, 1),
        ]
    );
    assert_eq!(att_range(src, t.as_slice(), b"g", &budget).unwrap(), (2, 6));
    assert_eq!(att_range(src, t.as_slice(), b"b", &budget).unwrap(), (2, 2));
}

/// 読めない ATTLIST は最初の `>` まで飛ばして当てない（roxmltree と同じに受ける）。飛ばしたあとが壊れていれば誤り。
#[test]
fn broken_attlists_are_skipped() {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    for src in [
        b"<!ATTLIST a b CDATA>".as_slice(),
        b"<!ATTLIST a b FOO \"x\">",
        b"<!ATTLIST a b CDATA #FIXED>",
        b"<!ATTLIST a b CDATA\"x\">",
        b"<!ATTLIST a b CDATA \"x<y\">",
        b"<!ATTLIST a x \0\0CDATA \"1\"><!ATTLIST a c CDATA 'ok'>",
    ] {
        let t = parse_subset(src, Span::new(0, src.len()), &budget).unwrap().attlists;
        let names: Vec<&[u8]> = t.as_slice().iter().map(|d| d.name.of(src)).collect();
        let want: &[&[u8]] = if src.ends_with(b"'ok'>") { &[b"c"] } else { &[] };
        assert_eq!(names, want, "{:?}", String::from_utf8_lossy(src));
    }
    // 引用の中の `>` で飛ばすのをやめ、残りが字句として壊れている
    let src = b"<!ATTLIST a b FOO \"x>y\">";
    assert!(parse_subset(src, Span::new(0, src.len()), &budget).is_err());
}
