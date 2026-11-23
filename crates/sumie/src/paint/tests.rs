#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use super::*;

#[test]
fn div255_rounds_exactly() {
    for x in 0..=255u32 * 255 {
        let want = (f64::from(x) / 255.0).round() as u32;
        assert_eq!(div255(x), want, "{x}");
    }
}

#[test]
fn premultiplied_colors() {
    assert_eq!(premultiply([255.0, 0.0, 0.0, 1.0]), [255, 0, 0, 255]);
    assert_eq!(premultiply([255.0, 128.0, 0.0, 0.5]), [128, 64, 0, 128]);
    assert_eq!(premultiply([300.0, -5.0, f64::NAN, 2.0]), [255, 0, 0, 255]);
    let p = premultiply([200.0, 100.0, 50.0, 0.3]);
    assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
}

#[test]
fn source_over() {
    let mut px = [0u8, 0, 255, 255, 10, 20, 30, 40];
    blend_span(&mut px, [255, 0, 0, 255], 255);
    assert_eq!(&px[0..4], &[255, 0, 0, 255]);
    // 半分の被覆：赤 128 と下の青の半分
    let mut px = [0u8, 0, 255, 255];
    blend_span(&mut px, [255, 0, 0, 255], 128);
    assert_eq!(px, [128, 0, 127, 255]);
    // 透明の上は src のまま
    let mut px = [0u8; 4];
    blend_span(&mut px, [100, 50, 0, 200], 255);
    assert_eq!(px, [100, 50, 0, 200]);
    // 被覆 0 は変えない
    let mut px = [1u8, 2, 3, 4];
    blend_span(&mut px, [255, 255, 255, 255], 0);
    assert_eq!(px, [1, 2, 3, 4]);
}
