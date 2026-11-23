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

/// `@media` を照らす大きさは FR-OUT-01 の属性の規則（% は viewBox に対する割合、片方は縦横の比で補う、em は既定の文字の
/// 大きさ）。`<style type>` の前後の空白は捨てない。
#[test]
fn media_size_follows_attribute_rules() {
    let stroke = |root: &str, cond: &str| -> bool {
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" {root}><style>@media {cond} {{ rect {{ stroke: red }} }}</style><rect/></svg>"
        );
        let doc = parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
        let rect =
            doc.xml().nodes.as_slice().iter().position(|n| n.local.of(svg.as_bytes()) == b"rect").unwrap() as u32;
        doc.styles().of(rect).iter().any(|s| crate::style::props::prop(s.prop).unwrap().name == b"stroke")
    };
    let pct = "width=\"50%\" height=\"50%\" viewBox=\"0 0 120 160\"";
    assert!(stroke(pct, "(width: 60px) and (height: 80px)"));
    assert!(stroke("width=\"auto\" height=\"50\" viewBox=\"0 0 100 50\"", "(width: 100px)"));
    assert!(stroke("width=\"30\" viewBox=\"0 0 100 50\"", "(height: 15px)"));
    assert!(stroke("viewBox=\"0 0 100 50\"", "(width: 100px)"));
    assert!(stroke("width=\"10em\" height=\"1in\"", "(width: 120px) and (height: 96px)"));
    // viewBox がなく片方が欠ける・% なら、大きさは中身で決まる（大きさの条件は偽）
    assert!(!stroke("width=\"10em\"", "(min-width: 1px)"));
    assert!(!stroke("width=\"50%\" height=\"10\"", "(min-width: 1px)"));
    // `type` の前後に空白があれば CSS として当てない（Chrome と同じ）
    let svg =
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"><style type=\" text/css \">rect { fill: red }</style><rect/></svg>";
    let doc = parse(svg, &ParseOptions::DEFAULT, &Never).unwrap();
    let rect = doc.xml().nodes.as_slice().iter().position(|n| n.local.of(svg) == b"rect").unwrap() as u32;
    assert!(doc.styles().of(rect).is_empty());
    let svg =
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"><style type=\"TEXT/CSS\">rect { fill: red }</style><rect/></svg>";
    let doc = parse(svg, &ParseOptions::DEFAULT, &Never).unwrap();
    let rect = doc.xml().nodes.as_slice().iter().position(|n| n.local.of(svg) == b"rect").unwrap() as u32;
    assert_eq!(doc.styles().of(rect).len(), 1);
}

/// `<style media>` は `@media` と同じに照らす。
#[test]
fn style_media_attribute() {
    let applied = |media: &str| {
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"50\"><style media=\"{media}\">rect {{ fill: red }}</style><rect/></svg>"
        );
        let doc = parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
        let rect =
            doc.xml().nodes.as_slice().iter().position(|n| n.local.of(svg.as_bytes()) == b"rect").unwrap() as u32;
        !doc.styles().of(rect).is_empty()
    };
    assert!(applied("screen"));
    assert!(applied("all and (min-width: 50px)"));
    assert!(!applied("print"));
    assert!(!applied("(min-width: 200px)"));
    assert!(applied(""));
}

/// 木を作る段の見積もり（M1 の形）：展開の数の上限と、描く木に入りうる画像・文字。
#[test]
fn estimate_build_checks_instances_and_needs() {
    let est = |body: &str| {
        let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">{body}</svg>");
        let doc = parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
        doc.estimate_build(&Never)
    };
    assert!(est("<rect/>").is_ok());
    // use の爆弾（2^25 を超える展開）
    let mut bomb = String::from("<rect id=\"l0\"/>");
    for k in 1..=25 {
        bomb.push_str(&format!("<g id=\"l{k}\"><use href=\"#l{}\"/><use href=\"#l{}\"/></g>", k - 1, k - 1));
    }
    assert!(
        matches!(est(&bomb), Err(Error::Limit(l)) if l.resource == Resource::Instances && l.stage == Stage::EstimateBuild)
    );
    assert!(
        matches!(est("<text>a</text>"), Err(Error::Unsupported(u)) if u.feature == crate::error::Feature::TextWithoutFonts)
    );
    assert!(
        matches!(est("<image href=\"a.png\"/>"), Err(Error::Unsupported(u)) if u.feature == crate::error::Feature::Image)
    );
    // content-visibility: hidden の入れ物の中の画像は描かないので要らない（単位 3 の 6 回目のレビュー）
    assert!(est("<g style=\"content-visibility: hidden\"><image href=\"a.png\"/></g>").is_ok());
    assert!(est("<g style=\"content-visibility: auto\"><image href=\"a.png\"/></g>").is_err());
    // use と、use から参照する symbol の content-visibility: hidden も（単位 3 の 7 回目のレビュー）
    assert!(est("<defs><g id=\"i\"><image href=\"a.png\"/></g></defs><use href=\"#i\" style=\"content-visibility: hidden\"/>").is_ok());
    assert!(
        est("<symbol id=\"s\" style=\"content-visibility: hidden\"><image href=\"a.png\"/></symbol><use href=\"#s\"/>")
            .is_ok()
    );
    assert!(est("<symbol id=\"s\"><image href=\"a.png\"/></symbol><use href=\"#s\"/>").is_err());
    // content-visibility: hidden の pattern の href の先は描く（use だけを飛ばす。単位 3 の 8 回目のレビュー）
    assert!(
        est("<pattern id=\"p\" width=\"5\" height=\"5\"><image href=\"a.png\"/></pattern><pattern id=\"q\" href=\"#p\" style=\"content-visibility: hidden\"/><rect width=\"5\" height=\"5\" fill=\"url(#q)\"/>")
            .is_err()
    );
    // 根の display: none と偽の条件は描くのを止めないので、中の画像も要る（ADR-102 (1)。単位 3 の 5 回目のレビュー）
    for attrs in ["display=\"none\"", "style=\"display: none\"", "systemLanguage=\"xx\""] {
        let src = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" {attrs}><image href=\"a.png\"/></svg>");
        let doc = parse(src.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
        assert!(
            matches!(doc.estimate_build(&Never), Err(Error::Unsupported(u)) if u.feature == crate::error::Feature::Image),
            "{attrs}"
        );
    }
}

/// 読まなかった参照：CSS の url は escape を解いた形で載せ、feImage の外の画像は <image> と同じに要るものにする（文書の
/// 1 回目のレビュー）。
#[test]
fn unread_refs_are_decoded_and_feimage_is_an_image() {
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect style="fill: url(a\29 .svg#p)"/><filter id="f"><feImage href="x.png"/></filter><rect filter="url(#f)"/></svg>"##;
    let d = crate::doc::parse(svg, &crate::style::ParseOptions::DEFAULT, &Never).unwrap();
    let unread: Vec<String> = d.report().unread_refs().map(|u| String::from_utf8_lossy(u.href).to_string()).collect();
    assert_eq!(unread, ["a).svg#p"]);
    assert!(d.needs().raster_images);
}

/// escape を解くと長くなる読まなかった参照（`\0` は U+FFFD）でも、報告の容量を解いた長さで数える（文書の 3 回目のレビュー）。
#[test]
fn unread_refs_longer_after_decoding() {
    for style in [r"fill:url(\0.png)", r"fill:url('\0\0\0')"] {
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><rect style=\"{style}\"/></svg>"
        );
        let d = crate::doc::parse(svg.as_bytes(), &crate::style::ParseOptions::DEFAULT, &Never).unwrap();
        assert_eq!(d.report().unread_refs().count(), 1, "{style}");
    }
}
