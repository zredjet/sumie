//! RGBA8（乗算済み）の画像と PNG の読み書き（照合の道具。architecture.md §10）。
//!
//! 照合は乗算済みの RGBA8 で測る（ADR-41 (8)）。PNG はストレートなので、読むときに決めた式で乗算済みに直し、書くときに
//! 戻す（式は ADR-52）。

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

/// RGBA8 乗算済みの画像（行を詰める）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Rgba {
    pub fn transparent(w: u32, h: u32) -> Rgba {
        Rgba { w, h, px: vec![0; w as usize * h as usize * 4] }
    }

    #[cfg(test)]
    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.w as usize + x as usize) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }

    /// 左上の `w × h` を切り出す（はみ出す分は透明）。
    pub fn crop(&self, w: u32, h: u32) -> Rgba {
        let mut out = Rgba::transparent(w, h);
        for y in 0..h.min(self.h) {
            let n = w.min(self.w) as usize * 4;
            let src = y as usize * self.w as usize * 4;
            let dst = y as usize * w as usize * 4;
            out.px[dst..dst + n].copy_from_slice(&self.px[src..src + n]);
        }
        out
    }
}

/// ストレートの 1 成分を乗算済みにする：`(c × a + 127) / 255`（半分は切り上げ。ADR-52）。
pub fn premultiply(c: u8, a: u8) -> u8 {
    ((u32::from(c) * u32::from(a) + 127) / 255) as u8
}

/// 乗算済みの 1 成分をストレートに戻す：`(p × 255 + a / 2) / a`（a = 0 なら 0。書き出しの絵のためだけ）。
pub fn demultiply(p: u8, a: u8) -> u8 {
    if a == 0 {
        return 0;
    }
    ((u32::from(p) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
}

/// PNG を読み、乗算済みの RGBA8 にする（グレー・RGB・パレット・16 ビットも 8 ビットの RGBA に広げる）。
pub fn read_png(path: &Path) -> Result<Rgba, String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut d = png::Decoder::new(BufReader::new(f));
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = d.read_info().map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (info.width, info.height);
    let mut out = Rgba::transparent(w, h);
    let n = w as usize * h as usize;
    for i in 0..n {
        let (r, g, b, a) = match info.color_type {
            png::ColorType::Rgba => (buf[i * 4], buf[i * 4 + 1], buf[i * 4 + 2], buf[i * 4 + 3]),
            png::ColorType::Rgb => (buf[i * 3], buf[i * 3 + 1], buf[i * 3 + 2], 255),
            png::ColorType::GrayscaleAlpha => (buf[i * 2], buf[i * 2], buf[i * 2], buf[i * 2 + 1]),
            png::ColorType::Grayscale => (buf[i], buf[i], buf[i], 255),
            png::ColorType::Indexed => return Err("パレットが広がらなかった".into()),
        };
        out.px[i * 4..i * 4 + 4].copy_from_slice(&[premultiply(r, a), premultiply(g, a), premultiply(b, a), a]);
    }
    Ok(out)
}

/// 乗算済みの画像を PNG に書く（ストレートに戻す）。
pub fn write_png(path: &Path, img: &Rgba) -> Result<(), String> {
    let mut straight = img.px.clone();
    for p in straight.as_chunks_mut::<4>().0 {
        let a = p[3];
        for c in &mut p[..3] {
            *c = demultiply(*c, a);
        }
    }
    let f = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut e = png::Encoder::new(BufWriter::new(f), img.w, img.h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    let mut w = e.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&straight).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiply_rounds_half_up() {
        assert_eq!(premultiply(255, 128), 128);
        assert_eq!(premultiply(255, 0), 0);
        assert_eq!(premultiply(1, 128), 1); // 0.502 → 1
        assert_eq!(premultiply(100, 255), 100);
        for a in 1..=255u8 {
            for c in [0u8, 1, 127, 128, 254, 255] {
                let p = premultiply(c, a);
                assert!(p <= a);
                assert!(u16::from(demultiply(p, a).abs_diff(c)) <= 255 / u16::from(a) + 1);
            }
        }
    }

    #[test]
    fn png_round_trip() {
        let dir = std::env::temp_dir().join(format!("sumie-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = Rgba::transparent(3, 2);
        img.px[0..4].copy_from_slice(&[128, 0, 0, 128]);
        img.px[4..8].copy_from_slice(&[0, 255, 0, 255]);
        let p = dir.join("a.png");
        write_png(&p, &img).unwrap();
        assert_eq!(read_png(&p).unwrap(), img);
        assert_eq!(img.crop(2, 3).get(1, 0), [0, 255, 0, 255]);
        assert_eq!(img.crop(2, 3).get(1, 2), [0, 0, 0, 0]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
