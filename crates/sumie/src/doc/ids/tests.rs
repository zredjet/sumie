#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::disallowed_types, clippy::indexing_slicing)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;
use crate::limits::Limits;

fn table(src: &str) -> (XmlDoc, Ids) {
    let input = src.as_bytes();
    let b = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let counts = crate::xml::count(input, &Limits::DEFAULT, &b, Stage::Parse).unwrap();
    let doc = crate::xml::build(input, &counts, &Limits::DEFAULT, &b, Stage::Parse).unwrap();
    let ids = Ids::build(&doc, input, &b).unwrap();
    assert!(b.max_spend() < crate::budget::CANCEL_INTERVAL);
    (doc, ids)
}

#[test]
fn first_element_wins() {
    let src = r#"<svg xmlns="http://www.w3.org/2000/svg"><g id="b"/><rect id="a"/><circle id="b"/><path x:id="c" xmlns:x="urn:x"/><line id="&#x63;"/></svg>"#;
    let (doc, ids) = table(src);
    let b = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let local =
        |n: Option<u32>| n.map(|n| String::from_utf8_lossy(doc.node(n).unwrap().local.of(src.as_bytes())).to_string());
    assert_eq!(local(ids.find(&doc, src.as_bytes(), b"a", &b).unwrap()).as_deref(), Some("rect"));
    assert_eq!(local(ids.find(&doc, src.as_bytes(), b"b", &b).unwrap()).as_deref(), Some("g"));
    // 名前空間のある id は id でない。文字の参照は解いた値
    assert_eq!(local(ids.find(&doc, src.as_bytes(), b"c", &b).unwrap()).as_deref(), Some("line"));
    assert_eq!(ids.find(&doc, src.as_bytes(), b"z", &b).unwrap(), None);
    assert_eq!(ids.entries.len(), 3);
}
