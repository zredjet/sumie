#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::indexing_slicing)]

use super::*;
use crate::cancel::Never;
use crate::style::ParseOptions;

fn size(root: &str) -> Result<Option<(f64, f64)>, Error> {
    let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" {root}/>");
    let doc = crate::doc::parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never)?;
    Ok(doc.size().map(|s| (s.width, s.height)))
}

/// usvg 0.48.1 と照らした例（FR-OUT-01）。
#[test]
fn sizes_from_attributes() {
    assert_eq!(size("width=\"10em\" height=\"2in\"").unwrap(), Some((120.0, 192.0)));
    // em は根の要素の文字の大きさ
    assert_eq!(size("width=\"10em\" height=\"2in\" style=\"font-size: 20px\"").unwrap(), Some((200.0, 192.0)));
    assert_eq!(size("width=\"0.3\" height=\"10.5\"").unwrap(), Some((0.3, 10.5)));
    assert_eq!(size("width=\"50%\" height=\"50%\" viewBox=\"0 0 120 160\"").unwrap(), Some((60.0, 80.0)));
    assert_eq!(size("width=\"auto\" height=\"50\" viewBox=\"0 0 100 50\"").unwrap(), Some((100.0, 50.0)));
    assert_eq!(size("width=\"10vw\" height=\"50\" viewBox=\"0 0 100 50\"").unwrap(), Some((100.0, 50.0)));
    assert_eq!(size("viewBox=\"0 0 100 50\"").unwrap(), Some((100.0, 50.0)));
    // viewBox がなく片方が欠ける・壊れた viewBox は中身で決まる
    assert_eq!(size("width=\"30\"").unwrap(), None);
    assert_eq!(size("width=\"30\" viewBox=\"0 0 0 50\"").unwrap(), None);
    assert_eq!(size("").unwrap(), None);
    // 0 と負は Invalid（位置は外側の svg）
    for bad in
        ["width=\"0\" height=\"10\"", "width=\"-5\" height=\"10\"", "width=\"0\" height=\"10\" viewBox=\"0 0 10 10\""]
    {
        match size(bad) {
            Err(Error::Invalid(i)) => {
                assert_eq!(i.reason, InvalidReason::Size);
                assert_eq!(i.position, Some(0));
            }
            x => panic!("{bad}: {x:?}"),
        }
    }
}

/// 画素は f32 にしてから四捨五入（0.5 は遠い側・最小 1。resvg の `to_int_size` と同じ。ADR-78 (5)）。
#[test]
fn pixel_sizes_round_like_resvg() {
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"0.3\" height=\"10.5\"/>";
    let doc = crate::doc::parse(svg, &ParseOptions::DEFAULT, &Never).unwrap();
    assert_eq!(doc.pixel_size(1.0).unwrap(), Some(PixelSize { width: 1, height: 11 }));
    assert_eq!(doc.pixel_size(2.0).unwrap(), Some(PixelSize { width: 1, height: 21 }));
    // 文書の 1 つ目のレビューで resvg と照らした例
    for (w, h, pw, ph) in [
        ("100", "233.33333333", 100, 233),
        ("12.3pt", "16", 16, 16),
        ("10mm", "10pt", 38, 13),
        ("10.49", "0.3", 10, 1),
        ("33.3333333", "2.5", 33, 3),
        ("10.0000001", "0.5", 10, 1),
    ] {
        let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\"/>");
        let doc = crate::doc::parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
        assert_eq!(doc.pixel_size(1.0).unwrap(), Some(PixelSize { width: pw, height: ph }), "{w} × {h}");
    }
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e300] {
        assert!(matches!(doc.pixel_size(bad), Err(Error::Invalid(i)) if i.reason == InvalidReason::Argument), "{bad}");
    }
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let doc = crate::doc::parse(svg, &ParseOptions::DEFAULT, &Never).unwrap();
    assert_eq!(doc.pixel_size(1.0).unwrap(), None);
    assert!(doc.pixel_size(0.0).is_err());
}
