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

fn events(src: &str) -> Result<Vec<Event>, Error> {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut lex = Lexer::new(src.as_bytes(), 0, &budget);
    let mut out = Vec::new();
    loop {
        let e = lex.next()?;
        out.push(e);
        if e == Event::Eof {
            // 終わりのあとも Eof を返し続ける
            assert_eq!(lex.next()?, Event::Eof);
            return Ok(out);
        }
        assert!(out.len() < 1000, "進まない");
    }
}

#[test]
fn event_sequence() {
    let src = "<!DOCTYPE s [<!ENTITY a 'x'>]><s a=\"1\"><t/>x<![CDATA[y]]></s>";
    let e = events(src).unwrap();
    let kinds: Vec<&str> = e
        .iter()
        .map(|e| match e {
            Event::Doctype { .. } => "doctype",
            Event::Start { .. } => "start",
            Event::Attr { .. } => "attr",
            Event::StartEnd { empty: true } => "empty",
            Event::StartEnd { empty: false } => "open",
            Event::End { .. } => "end",
            Event::Text { .. } => "text",
            Event::CData { .. } => "cdata",
            Event::Eof => "eof",
        })
        .collect();
    assert_eq!(kinds, ["doctype", "start", "attr", "open", "start", "empty", "text", "cdata", "end", "eof"]);
    let Event::Doctype { subset: Some(s) } = e[0] else { panic!() };
    assert_eq!(s.of(src.as_bytes()), b"<!ENTITY a 'x'>");
}

#[test]
fn pays_every_byte_once() {
    let src = format!("<s>{}</s>", "a".repeat(100_000));
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut lex = Lexer::new(src.as_bytes(), 0, &budget);
    while lex.next().unwrap() != Event::Eof {}
    assert_eq!(budget.work(), src.len() as u64);
    assert!(budget.max_spend() <= SCAN_CHUNK as u64);
}
