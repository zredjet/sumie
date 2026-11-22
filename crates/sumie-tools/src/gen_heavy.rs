//! `gen-heavy`：重い入力を作る（architecture.md §10 の表。引数は §10 に書いた値。ADR-19・ADR-35 (2)）。
//!
//! `gen-heavy [--out local/heavy]`：種と引数を固定して作り、`index.jsonl`（道・出どころ・範囲・バイト数・BLAKE3・引数）を
//! 書く。作るものは決まっている（超越関数を使わず、角は定数の表。どの OS でも同じバイト列。試験 `digests_are_fixed` が
//! BLAKE3 を照らす）ので、git には入れない。
//!
//! Kura の遅い入力 S1〜S7 は、Kura の `docs/editing.md` §9.5・`svg_worker` の注釈と、Kura の試験の入力（`SLOW_SHAPE`・
//! `SLOW_DECODE`・pattern の入れ子）から作る。S1・S4 の 1 本・pattern の入れ子は Kura の試験の入力と同じ形。ほかは説明から
//! 作った同じ性質の入力（Kura の元の入力の写しではない）。地図風の線と S5 は Kura の元の引数が分からない（architecture.md §10）。

use std::fmt::Write as _;
use std::path::PathBuf;

use sumie_testkit::chacha::ChaCha;
use sumie_testkit::jsonl::Line;

use crate::corpus::{arg, workspace_root};

/// 作る入力（名前・引数の説明・中身）。
fn inputs() -> Vec<(&'static str, &'static str, String)> {
    vec![
        (
            "map-lines",
            "4096×4096・折れ線 1,000 本 × 300 区間（30 万区間）・ランダムウォーク 1 歩 2〜12 画素・線の幅 1.5・継ぎ目と端 round・種 1",
            map_lines(),
        ),
        (
            "map-shapes",
            "4096×4096・閉じた path 45,000 本（1 本 20 頂点・半径 4〜20 画素・8 色の塗り）・種 2",
            map_shapes(),
        ),
        (
            "s1-rotated-thick-stroke",
            "100×100・translate(1e9) と rotate(-12.5) の下の、太さ 1e7 の線の 3 次曲線 2 区間（Kura の試験の SLOW_SHAPE と同じ形。Kura：release で 2.6 秒）",
            s1(),
        ),
        ("s2-many-stops", "1000×1000・stop 10 万のグラデーションで塗った全面の rect 10 枚（Kura：32 秒）", s2()),
        (
            "s3-same-offset-stops",
            "100×100・offset がすべて 0.5 の stop 40 万のグラデーション（Kura：40 万個で形 14 秒）",
            s3(),
        ),
        (
            "s4-fine-dashes-one",
            "1000×1000・3 次曲線 1 本・太さ 3・破線 0.0005 0.000501・丸い端（Kura の試験の SLOW_DECODE と同じ形。Kura：8.9 秒）",
            s4_one(),
        ),
        ("s4-fine-dashes-37", "1000×1000・SLOW_DECODE の曲線を縦にずらした 37 本（Kura：37 本で 310 秒）", s4_37()),
        (
            "s5-rotated-pattern",
            "4000×4000・patternTransform rotate(30) の 4×4 のタイルで塗った全面（Kura の元の引数は分からない。Kura：102 秒）",
            s5(),
        ),
        (
            "s6-textpath-unseparated-transform",
            "200×100・区切りのない数の transform（translate(10-5)）を持つ path へのふつうの textPath（かならず描く互換の例）",
            s6(),
        ),
        (
            "s7-turbulence-octaves",
            "1000×1000・numOctaves 1000 の feTurbulence（Kura の大きさは分からない。Kura：18 秒）",
            s7(),
        ),
        (
            "bomb-use-doubling-16",
            "200×200・use の倍々 16 段（2^16 個。usvg 0.48.1 の節の上限の内で受ける最大）",
            bomb_use(16),
        ),
        ("bomb-use-doubling-17", "200×200・use の倍々 17 段（2^17 個。usvg が節の上限で断る）", bomb_use(17)),
        (
            "bomb-pattern-nesting",
            "8×8・objectBoundingBox の pattern の入れ子 16 段・1 段に参照 2 つ（Kura の試験の形を 16 段に。2^16 の複製）",
            bomb_pattern(16),
        ),
        ("bomb-marker-dup", "1000×1000・頂点 10 万の path の marker-mid（marker の複製）", bomb_marker()),
    ]
}

pub fn run(args: &[String]) -> Result<(), String> {
    let out = arg(args, "--out").map(PathBuf::from).unwrap_or_else(|| workspace_root().join("local/heavy"));
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut index = String::new();
    let mut changed = Vec::new();
    for (name, params, svg) in inputs() {
        let digest = blake3::hash(svg.as_bytes()).to_hex().to_string();
        if DIGESTS.iter().find(|(n, _)| *n == name).map(|(_, d)| *d) != Some(digest.as_str()) {
            changed.push(name);
        }
        let file = format!("{name}.svg");
        std::fs::write(out.join(&file), &svg).map_err(|e| e.to_string())?;
        let l = Line::new()
            .str("path", file.as_str())
            .str("source", "gen-heavy")
            .str("category", name)
            .str("scope", "in")
            .int("bytes", svg.len() as u64)
            .str("blake3", blake3::hash(svg.as_bytes()).to_hex().to_string())
            .str("features", crate::corpus::features(svg.as_bytes()).join(","))
            .str("params", params);
        index.push_str(&l.to_json());
        index.push('\n');
        println!("{name}：{} バイト", svg.len());
    }
    std::fs::write(out.join("index.jsonl"), index).map_err(|e| e.to_string())?;
    if !changed.is_empty() {
        return Err(format!(
            "記録した BLAKE3（DIGESTS）と違う入力：{changed:?}（作り方を変えたなら DIGESTS と architecture.md §10 を直す）"
        ));
    }
    Ok(())
}

fn head(w: u32, h: u32) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n"
    )
}

fn map_lines() -> String {
    let mut r = ChaCha::from_u64(1);
    let mut s = head(4096, 4096);
    s.push_str(
        "<g fill=\"none\" stroke=\"#333\" stroke-width=\"1.5\" stroke-linejoin=\"round\" stroke-linecap=\"round\">\n",
    );
    for _ in 0..1000 {
        let (mut x, mut y) = (r.below(4096) as i64, r.below(4096) as i64);
        let _ = write!(s, "<path d=\"M{x} {y}");
        for _ in 0..300 {
            let step = r.range(2, 12) as i64;
            let dir = r.below(8);
            let (dx, dy) = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)][dir as usize];
            x = (x + dx * step).clamp(0, 4095);
            y = (y + dy * step).clamp(0, 4095);
            let _ = write!(s, "L{x} {y}");
        }
        s.push_str("\"/>\n");
    }
    s.push_str("</g>\n</svg>\n");
    s
}

/// 20 等分の角の cos・sin（定数。std の超越関数は OS の libm を呼ぶので、どの OS でも同じバイト列にするため表にする）。
const ANGLES_20: [(f64, f64); 20] = [
    (1.0, 0.0),
    (0.9510565162951535, 0.3090169943749474),
    (0.8090169943749475, 0.5877852522924731),
    (0.5877852522924731, 0.8090169943749475),
    (0.30901699437494745, 0.9510565162951535),
    (6.123233995736766e-17, 1.0),
    (-0.30901699437494734, 0.9510565162951536),
    (-0.587785252292473, 0.8090169943749475),
    (-0.8090169943749473, 0.5877852522924732),
    (-0.9510565162951535, 0.3090169943749475),
    (-1.0, 1.2246467991473532e-16),
    (-0.9510565162951538, -0.3090169943749469),
    (-0.8090169943749475, -0.587785252292473),
    (-0.5877852522924732, -0.8090169943749473),
    (-0.30901699437494756, -0.9510565162951535),
    (-1.8369701987210297e-16, -1.0),
    (0.30901699437494723, -0.9510565162951536),
    (0.5877852522924729, -0.8090169943749476),
    (0.8090169943749473, -0.5877852522924734),
    (0.9510565162951535, -0.3090169943749477),
];

fn map_shapes() -> String {
    let mut r = ChaCha::from_u64(2);
    let colors = ["#e6194b", "#3cb44b", "#ffe119", "#4363d8", "#f58231", "#911eb4", "#46f0f0", "#f032e6"];
    let mut s = head(4096, 4096);
    for _ in 0..45_000 {
        let (cx, cy) = (r.below(4096) as f64, r.below(4096) as f64);
        let rad = r.range(4, 20) as f64;
        let c = colors[r.below(8) as usize];
        let _ = write!(s, "<path fill=\"{c}\" d=\"M");
        for (k, (c, sn)) in ANGLES_20.iter().enumerate() {
            // 角は決まった 20 等分。半径だけを揺らす
            let rr = rad * (0.6 + 0.4 * r.unit_f64());
            let _ = write!(s, "{}{:.2} {:.2}", if k == 0 { "" } else { "L" }, cx + rr * c, cy + rr * sn);
        }
        s.push_str("Z\"/>\n");
    }
    s.push_str("</svg>\n");
    s
}

fn s1() -> String {
    let mut s = head(100, 100);
    s.push_str(
        "<g transform=\"translate(1e9)\"><path d=\"m 30.2,100.5 c 20.1,-30.4 60.2,-30.4 80.3,0 s 60.2,30.4 80.3,0\" \
         fill=\"none\" stroke=\"#000\" stroke-width=\"1e7\" transform=\"rotate(-12.5)\"/></g>\n</svg>\n",
    );
    s
}

fn gradient_rects(size: u32, stops: &str, n: u32) -> String {
    let mut s = head(size, size);
    let _ = writeln!(s, "<defs><linearGradient id=\"g\">{stops}</linearGradient></defs>");
    for _ in 0..n {
        let _ = writeln!(s, "<rect width=\"{size}\" height=\"{size}\" fill=\"url(#g)\"/>");
    }
    s.push_str("</svg>\n");
    s
}

fn s2() -> String {
    let mut stops = String::new();
    for k in 0..100_000u32 {
        let _ = write!(
            stops,
            "<stop offset=\"{:.5}\" stop-color=\"#{:06x}\"/>",
            f64::from(k) / 100_000.0,
            k.wrapping_mul(2_654_435_761) & 0xff_ffff
        );
    }
    gradient_rects(1000, &stops, 10)
}

fn s3() -> String {
    // stop を短く書いて 40 万個を 16 MiB（Kura の入力の上限）に収める
    let mut stops = String::new();
    for k in 0..400_000u32 {
        let _ = write!(stops, "<stop offset=\".5\" stop-color=\"#{:03x}\"/>", k.wrapping_mul(40503) & 0xfff);
    }
    gradient_rects(100, &stops, 1)
}

/// Kura の試験の SLOW_DECODE の曲線（`dy` だけ縦にずらす）。
fn slow_decode_path(dy: u32) -> String {
    format!(
        "<path d=\"M0 {0} C300 {1} 700 {2} 1000 {0}\" stroke=\"black\" stroke-width=\"3\" stroke-linecap=\"round\" \
         stroke-dasharray=\"0.0005 0.000501\" fill=\"none\"/>\n",
        5 + dy,
        dy,
        10 + dy
    )
}

fn s4_one() -> String {
    let mut s = head(1000, 1000);
    s.push_str(&slow_decode_path(0));
    s.push_str("</svg>\n");
    s
}

fn s4_37() -> String {
    let mut s = head(1000, 1000);
    for k in 0..37 {
        s.push_str(&slow_decode_path(k * 26));
    }
    s.push_str("</svg>\n");
    s
}

fn s5() -> String {
    let mut s = head(4000, 4000);
    s.push_str(
        "<defs><pattern id=\"p\" width=\"4\" height=\"4\" patternUnits=\"userSpaceOnUse\" patternTransform=\"rotate(30)\">\
         <rect width=\"2\" height=\"2\" fill=\"#c00\"/><circle cx=\"3\" cy=\"3\" r=\"1\" fill=\"#00c\"/></pattern></defs>\n\
         <rect width=\"4000\" height=\"4000\" fill=\"url(#p)\"/>\n</svg>\n",
    );
    s
}

fn s6() -> String {
    let mut s = head(200, 100);
    s.push_str(
        "<defs><path id=\"p\" transform=\"translate(10-5)\" d=\"M10 80 Q100 10 190 80\"/></defs>\n\
         <text font-family=\"sans-serif\" font-size=\"16\"><textPath xlink:href=\"#p\">sumie textPath</textPath></text>\n</svg>\n",
    );
    s
}

fn s7() -> String {
    let mut s = head(1000, 1000);
    s.push_str(
        "<filter id=\"t\" x=\"0\" y=\"0\" width=\"1\" height=\"1\"><feTurbulence baseFrequency=\"0.01\" numOctaves=\"1000\"/></filter>\n\
         <rect width=\"1000\" height=\"1000\" filter=\"url(#t)\"/>\n</svg>\n",
    );
    s
}

fn bomb_use(levels: u32) -> String {
    let mut s = head(200, 200);
    s.push_str("<defs><rect id=\"u0\" width=\"1\" height=\"1\" fill=\"#000\"/>\n");
    for k in 1..=levels {
        let _ =
            writeln!(s, "<g id=\"u{k}\"><use xlink:href=\"#u{0}\"/><use xlink:href=\"#u{0}\" x=\"0.001\"/></g>", k - 1);
    }
    let _ = writeln!(s, "</defs>\n<use xlink:href=\"#u{levels}\"/>\n</svg>");
    s
}

/// Kura の試験（`nested_pattern_clones_are_refused_before_usvg`）と同じ形の入れ子：1 段に 2 つの rect が下の段を塗る。
fn bomb_pattern(levels: u32) -> String {
    let mut s = head(8, 8);
    s.push_str("<defs>\n<pattern id=\"p0\" width=\".5\" height=\".5\"><rect width=\"1\" height=\"1\"/></pattern>\n");
    for k in 1..=levels {
        let _ = writeln!(
            s,
            "<pattern id=\"p{k}\" width=\".5\" height=\".5\"><rect width=\".5\" height=\".5\" fill=\"url(#p{0})\"/>\
             <rect x=\".5\" width=\".5\" height=\".5\" fill=\"url(#p{0})\"/></pattern>",
            k - 1
        );
    }
    let _ = writeln!(s, "</defs>\n<rect width=\"8\" height=\"8\" fill=\"url(#p{levels})\"/>\n</svg>");
    s
}

fn bomb_marker() -> String {
    let mut s = head(1000, 1000);
    s.push_str("<defs><marker id=\"m\" markerWidth=\"4\" markerHeight=\"4\" refX=\"2\" refY=\"2\"><circle cx=\"2\" cy=\"2\" r=\"2\"/></marker></defs>\n");
    s.push_str("<path fill=\"none\" stroke=\"#000\" marker-mid=\"url(#m)\" d=\"M0 0");
    let mut r = ChaCha::from_u64(3);
    for _ in 0..100_000 {
        let _ = write!(s, "L{} {}", r.below(1000), r.below(1000));
    }
    s.push_str("\"/>\n</svg>\n");
    s
}

/// 作る入力の BLAKE3（作り方を変えたら、architecture.md §10 の表と一緒に直す）。
const DIGESTS: &[(&str, &str)] = &[
    ("map-lines", "cca37a98e0e289bb524c31aa72435c9122526b62e9ed1f2aeee1841b76244fc4"),
    ("map-shapes", "99abf148456a3872d08be745c65d3ad23f089e945acd67c624ebf9d3d39e4b38"),
    ("s1-rotated-thick-stroke", "e12cbb6d9c4ee95a7279be38555520fe7ea01f7a4bfc66ac92ce5e8bf6f9d748"),
    ("s2-many-stops", "d8b1e62227d7b512d2905863edc4e1aaf9442236d5ce879043078ff4e3d9590f"),
    ("s3-same-offset-stops", "3f2c0950c0bfb58fb5b1c43c6c8ef86e7dc5dbb9ba73e14b76e275c0e0dbfabe"),
    ("s4-fine-dashes-one", "772e750d536f64b5326dbf308b3e46ab2495fd7b97fa758a9b9168513df4471f"),
    ("s4-fine-dashes-37", "c1fd8fa8b3cf0d7da4de4554404f0dd6186a3de4b1145b2dbc72a0d79f5815cb"),
    ("s5-rotated-pattern", "b6ac7e20be8be602a98141b11094942b2d4f6d92bdfa23b52884d1bebec8ef00"),
    ("s6-textpath-unseparated-transform", "2abda8d64c2b0ad7b3cdc82df8b3891e4852408ebcdb28a3904a2bd5a37847b7"),
    ("s7-turbulence-octaves", "cd2c2b47e3dd31d659874df464c684278089bb84cdb9e61adee2e06c4fcb6ec7"),
    ("bomb-use-doubling-16", "3233f0ca197d41d7ab6624966529a74b7ac992936e6183eae7abe52d9018ce1d"),
    ("bomb-use-doubling-17", "2abd61f9d7157222e552f0bd2aaf8919ef90f2693fe595c453437bc6acd89228"),
    ("bomb-pattern-nesting", "2a89dc3a517be9d62bba45239b80e0dbaae2318bebf32a74f813cfca953e9017"),
    ("bomb-marker-dup", "ad5ab0a3fcb6033f71926fc4591371efe13ebf69a2e17fe1215e04f29cac960f"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_are_fixed() {
        let got: Vec<(&str, String)> =
            inputs().into_iter().map(|(n, _, svg)| (n, blake3::hash(svg.as_bytes()).to_hex().to_string())).collect();
        assert_eq!(got.len(), DIGESTS.len());
        for ((n, d), (en, ed)) in got.iter().zip(DIGESTS) {
            assert_eq!(n, en);
            assert_eq!(d, ed, "{n} のバイト列が変わった（作り方を変えたなら DIGESTS と architecture.md §10 を直す）");
        }
    }
}
