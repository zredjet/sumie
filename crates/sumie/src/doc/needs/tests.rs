#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

use super::*;
use crate::cancel::Never;
use crate::style::ParseOptions;

fn needs_of(body: &str) -> Needs {
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"10\" height=\"10\">{body}</svg>"
    );
    crate::doc::parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap().needs()
}

const PNG: &str = "data:image/png;base64,AAAA";

#[test]
fn images_on_the_drawn_paths() {
    let raster = |b: &str| needs_of(b).raster_images;
    assert!(raster(&format!("<image href=\"{PNG}\"/>")));
    assert!(raster("<image xlink:href=\"a.jpg\"/>"));
    // 参照されない defs・display: none・選ばれない switch・条件が偽・clipPath の中は数えない
    assert!(!raster(&format!("<defs><image href=\"{PNG}\"/></defs>")));
    assert!(!raster(&format!("<g display=\"none\"><image href=\"{PNG}\"/></g>")));
    assert!(!raster(&format!("<g style=\"display: none\"><image href=\"{PNG}\"/></g>")));
    assert!(!raster(&format!("<switch><rect/><image href=\"{PNG}\"/></switch>")));
    assert!(!raster(&format!("<image systemLanguage=\"xx\" href=\"{PNG}\"/>")));
    assert!(!raster(&format!("<clipPath id=\"c\"><image href=\"{PNG}\"/></clipPath><rect clip-path=\"url(#c)\"/>")));
    assert!(!raster(&format!("<bogus><image href=\"{PNG}\"/></bogus>")));
    // 参照の辺で入った入れ物の中は数える
    assert!(raster(&format!("<defs><image id=\"i\" href=\"{PNG}\"/></defs><use href=\"#i\"/>")));
    assert!(raster(&format!("<pattern id=\"p\"><image href=\"{PNG}\"/></pattern><rect fill=\"url(#p)\"/>")));
    assert!(raster(&format!("<mask id=\"m\"><image href=\"{PNG}\"/></mask><rect mask=\"url(#m)\"/>")));
    assert!(raster(&format!("<filter id=\"f\"><feImage href=\"{PNG}\"/></filter><rect filter=\"url(#f)\"/>")));
    // SVG の画像
    let n = needs_of("<image href=\"data:image/svg+xml;utf8,&lt;svg/&gt;\"/><image href=\"b.SVG?x=1\"/>");
    assert!(n.svg_images && !n.raster_images);
    // 文字は要素の有無
    assert!(needs_of("<defs><text>a</text></defs>").text);
    assert!(!needs_of("<rect/>").text);
}

/// 描かない入れ物が参照より前に子の辺で来ても、参照の辺で来たら中をたどる。switch が選ばない子・条件が偽の祖先の中の
/// 要素も、参照の辺で入れば描く（Chrome・usvg と同じ。文書の 1 回目のレビュー）。
#[test]
fn referenced_images_regardless_of_order_and_context() {
    let raster = |b: &str| needs_of(b).raster_images;
    let img = format!("<image href=\"{PNG}\"/>");
    for doc in [
        format!("<rect fill=\"url(#p)\"/><pattern id=\"p\">{img}</pattern>"),
        format!("<pattern id=\"p\">{img}</pattern><rect fill=\"url(#p)\"/>"),
        format!("<use href=\"#s\"/><symbol id=\"s\">{img}</symbol>"),
        format!("<rect filter=\"url(#f)\"/><filter id=\"f\"><feImage href=\"{PNG}\"/></filter>"),
        format!("<rect mask=\"url(#m)\"/><mask id=\"m\">{img}</mask>"),
        format!("<path d=\"M0 0L1 1\" marker-end=\"url(#k)\"/><marker id=\"k\">{img}</marker>"),
        format!("<switch><rect/><image id=\"b\" href=\"{PNG}\"/></switch><use href=\"#b\"/>"),
        format!("<g systemLanguage=\"xx\"><image id=\"c\" href=\"{PNG}\"/></g><use href=\"#c\"/>"),
    ] {
        assert!(raster(&doc), "{doc}");
    }
    // 自分の条件が偽なら参照しても描かない。図形・文字の中の画像は描かない
    for doc in [
        format!("<image id=\"d\" systemLanguage=\"xx\" href=\"{PNG}\"/><use href=\"#d\"/>"),
        format!("<rect>{img}</rect>"),
        format!("<text>{img}</text>"),
        format!("<g style=\"display: n\\6f ne\">{img}</g>"),
    ] {
        assert!(!raster(&doc), "{doc}");
    }
}

/// 参照の辺で入った描かない入れ物には display と自分の条件が効かない（SVG 1.1・2・resvg。文書の 2 回目のレビュー）。
/// 結果は文書の順に依らず、filter の中の fe… でない要素と、根が svg でない文書の画像は数えない。
#[test]
fn referenced_containers_ignore_display_and_conditions() {
    let raster = |b: &str| needs_of(b).raster_images;
    let img = format!("<image href=\"{PNG}\"/>");
    for attr in ["display=\"none\"", "style=\"display:none\"", "systemLanguage=\"xx\"", "requiredExtensions=\"\""] {
        for doc in [
            format!("<pattern id=\"p\" {attr}>{img}</pattern><rect fill=\"url(#p)\"/>"),
            format!("<marker id=\"k\" {attr}>{img}</marker><path d=\"M0 0L1 1\" marker-end=\"url(#k)\"/>"),
            format!("<mask id=\"m\" {attr}>{img}</mask><rect mask=\"url(#m)\"/>"),
            format!("<symbol id=\"s\" {attr}>{img}</symbol><use href=\"#s\"/>"),
            format!("<filter id=\"f\" {attr}><feImage href=\"{PNG}\"/></filter><rect filter=\"url(#f)\"/>"),
        ] {
            assert!(raster(&doc), "{doc}");
        }
    }
    // 文書の順に依らない（use から文字の中の a を参照する）
    let a = needs_of(&format!("<use href=\"#a\"/><text><a id=\"a\">{img}</a></text>")).raster_images;
    let b = needs_of(&format!("<text><a id=\"a\">{img}</a></text><use href=\"#a\"/>")).raster_images;
    assert_eq!(a, b);
    // filter の中に直に置いた image は描かない
    assert!(!raster(&format!("<filter id=\"f\">{img}</filter><rect filter=\"url(#f)\"/>")));
    let svg = format!("<g xmlns=\"http://www.w3.org/2000/svg\">{img}</g>");
    let d = crate::doc::parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    assert!(!d.needs().raster_images);
}

/// 輪として取り除いた参照の先（use の文脈では輪でないもの）も、M2 が 1 段だけ描き直すので要るものに入れる（ADR-86 (1)）。
#[test]
fn removed_cycle_edges_are_followed() {
    let n = needs_of(&format!(
        "<pattern id=\"p\"><use href=\"#r\"/><image href=\"{PNG}\"/></pattern><g fill=\"url(#p)\"><rect id=\"r\"/></g>"
    ));
    assert!(n.raster_images);
}
