#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::disallowed_types)]

use super::*;
use crate::cancel::Never;
use crate::error::{InvalidReason, Resource};

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
    // 確保の上限でも両方が同じ誤り。estimate_work・estimate_bytes は読む段には効かない（ADR-70）
    let mut opts = ParseOptions::DEFAULT;
    opts.limits.parse_bytes = est.peak_bytes - 1;
    assert_eq!(estimate_parse(SVG, &opts, &Never).unwrap_err(), parse(SVG, &opts, &Never).unwrap_err());
    let mut opts = ParseOptions::DEFAULT;
    opts.limits.estimate_work = 10;
    opts.limits.estimate_bytes = 10;
    assert!(parse(SVG, &opts, &Never).is_ok());
}

/// `<style>` の規則が当たり、条件・報告もまとまる。
#[test]
fn styles_and_report() {
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"50\"><style type=\"TEXT/CSS\">@import url(a\\29 .css); rect { fill: red } @media (min-width: 60px) { rect { stroke: blue } } @import url(b.css); circle:bogus {}</style><style type=\"text/foo\">rect { opacity: .5 }</style><rect fill=\"nonsense\"/></svg>";
    let doc = parse(svg, &ParseOptions::DEFAULT, &Never).unwrap();
    let rect = doc.xml().nodes.as_slice().iter().position(|n| n.local.of(svg) == b"rect").unwrap() as u32;
    let names: Vec<&[u8]> =
        doc.styles().of(rect).iter().map(|s| crate::style::props::prop(s.prop).unwrap().name).collect();
    assert_eq!(names, [b"fill".as_slice(), b"stroke"]);
    let kinds: Vec<_> = doc.report().warnings().map(|w| w.kind).collect();
    // 壊れたセレクタ、属性の壊れた値、規則のあとの @import（捨てる）
    assert!(kinds.contains(&crate::report::WarningKind::InvalidSelector));
    assert!(kinds.contains(&crate::report::WarningKind::InvalidValue));
    assert!(kinds.contains(&crate::report::WarningKind::AtRule));
    // href は escape を解いた形
    let unread: Vec<&[u8]> = doc.report().unread_refs().map(|u| u.href).collect();
    assert_eq!(unread, [b"a).css".as_slice()]);
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

/// 見積もりは parse と同じ道の実際（ADR-70）で、印がどこで立っても、立ってから止まるまでの手間は 2 × CANCEL_INTERVAL の内
/// （NFR-06）。
#[test]
fn estimate_is_exact_and_cancel_is_prompt() {
    use crate::budget::CANCEL_INTERVAL as CI;
    use std::sync::atomic::{AtomicU32, Ordering};
    struct After(AtomicU32);
    impl Cancel for After {
        fn cancelled(&self) -> bool {
            self.0.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_err()
        }
    }
    let big = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><style>g {{ fill: red }} .a g {{ stroke: blue }}</style>{}</svg>",
        "<g a=\"1\" class=\"a\"/>".repeat(4_000)
    );
    let big = big.as_bytes();
    let opts = ParseOptions::DEFAULT;
    let e = estimate_parse(big, &opts, &Never).unwrap();
    let mut w = Watch::default();
    let d = parse_watch(big, &opts, &Never, &mut w).unwrap();
    assert_eq!((w.work, w.peak, d.held_bytes()), (e.work, e.peak_bytes, e.result_bytes));
    let k = w.work / CI;
    let mut cancelled = 0;
    // 40 ほどの所で印を立てる（ほかの所も同じ形）
    let step = (k / 40).max(1);
    for calls in (0..=(k + 1)).step_by(step as usize) {
        let c = After(AtomicU32::new(calls as u32));
        let mut w2 = Watch::default();
        if parse_watch(big, &opts, &c, &mut w2).err() == Some(Error::Cancelled) {
            cancelled += 1;
            assert!(w2.work - calls * CI < 2 * CI, "{calls} 回目");
        }
    }
    assert!(cancelled >= (k / step).saturating_sub(1), "{cancelled}");
}
