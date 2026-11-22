#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::disallowed_types)]

use super::*;
use crate::cancel::Never;
use crate::error::InvalidReason;

const SVG: &[u8] =
    b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><rect width=\"5\" height=\"5\"/></svg>";

#[test]
fn estimate_covers_parse() {
    let opts = ParseOptions::DEFAULT;
    let est = estimate_parse(SVG, &opts, &Never).unwrap();
    let doc = parse(SVG, &opts, &Never).unwrap();
    assert_eq!(doc.held_bytes(), est.result_bytes);
    assert_eq!(doc.xml().nodes.len(), 2);
    assert_eq!(doc.input(), SVG);
    assert_eq!(doc.report().warnings().len(), 0);
}

#[test]
fn limits_refuse_in_both() {
    // 本体の手間の上限を見積もりより小さくすると、estimate_parse も parse も同じ Limit
    let est = estimate_parse(SVG, &ParseOptions::DEFAULT, &Never).unwrap();
    let mut opts = ParseOptions::DEFAULT;
    opts.limits.parse_work = est.work - 1;
    let a = estimate_parse(SVG, &opts, &Never).unwrap_err();
    let b = parse(SVG, &opts, &Never).unwrap_err();
    assert_eq!(a, b);
    assert!(matches!(a, Error::Limit(l) if l.resource == Resource::Work && l.stage == Stage::Parse));
    let mut opts = ParseOptions::DEFAULT;
    opts.limits.parse_bytes = est.peak_bytes - 1;
    assert!(matches!(parse(SVG, &opts, &Never), Err(Error::Limit(l)) if l.resource == Resource::Memory));
    // ちょうどなら通る
    let mut opts = ParseOptions::DEFAULT;
    opts.limits.parse_work = est.work;
    opts.limits.parse_bytes = est.peak_bytes;
    assert!(parse(SVG, &opts, &Never).is_ok());
    // 見積もりの上限は数える段で効く
    let mut opts = ParseOptions::DEFAULT;
    opts.limits.estimate_work = 10;
    assert!(matches!(parse(SVG, &opts, &Never), Err(Error::Limit(l)) if l.stage == Stage::EstimateParse));
}

#[test]
fn bad_options_are_argument_errors() {
    for size in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut opts = ParseOptions::DEFAULT;
        opts.style.font_size = size;
        for r in [estimate_parse(SVG, &opts, &Never).map(|_| ()), parse(SVG, &opts, &Never).map(|_| ())] {
            assert!(matches!(r, Err(Error::Invalid(i)) if i.reason == InvalidReason::Argument && i.position.is_none()));
        }
    }
}

#[test]
fn cancel_is_seen() {
    use std::sync::atomic::AtomicBool;
    let big = format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{}</svg>", "<g/>".repeat(100_000));
    let stop = AtomicBool::new(true);
    assert_eq!(parse(big.as_bytes(), &ParseOptions::DEFAULT, &stop).unwrap_err(), Error::Cancelled);
    assert_eq!(estimate_parse(big.as_bytes(), &ParseOptions::DEFAULT, &stop).unwrap_err(), Error::Cancelled);
}
