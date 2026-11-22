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

/// 見積もりは parse の全体（数える段 ＋ 本体）を上から押さえ（ADR-68 (1)）、印がどこで立っても（段の間でも）、立ってから
/// 止まるまでの手間は 2 × CANCEL_INTERVAL の内（ADR-68 (2)・NFR-06）。
#[test]
fn whole_parse_is_estimated_and_cancel_is_prompt() {
    use crate::budget::CANCEL_INTERVAL as CI;
    use std::sync::atomic::{AtomicU32, Ordering};
    struct After(AtomicU32);
    impl Cancel for After {
        fn cancelled(&self) -> bool {
            self.0.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_err()
        }
    }
    let big = format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{}</svg>", "<g a=\"1\"/>".repeat(25_000));
    let big = big.as_bytes();
    let opts = ParseOptions::DEFAULT;
    let e = estimate_parse(big, &opts, &Never).unwrap();
    let mut w = Watch::default();
    parse_watch(big, &opts, &Never, &mut w).unwrap();
    assert!(w.count_work + w.body_work <= e.work);
    assert!(w.count_peak.max(w.body_peak) <= e.peak_bytes);
    let k = w.count_work / CI;
    let mut cancelled = 0;
    for calls in 0..=(k + 2 + w.body_work / CI) {
        let c = After(AtomicU32::new(calls as u32));
        let mut w2 = Watch::default();
        if parse_watch(big, &opts, &c, &mut w2).err() == Some(Error::Cancelled) {
            cancelled += 1;
            // 印が立った時点の手間（数える段の中なら calls 回目の確かめ、段の間なら数える段の終わり、本体なら本体の確かめ）
            let flag_at = if calls <= k { calls * CI } else { w.count_work + (calls - k - 1) * CI };
            assert!(w2.count_work + w2.body_work - flag_at < 2 * CI, "{calls} 回目");
        }
    }
    assert!(cancelled > k);
}
