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
