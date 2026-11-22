//! `gen-heavy`：重い入力を作る（architecture.md §10 の表。引数は §10 に書いた値。ADR-19・ADR-35 (2)）。
//!
//! `gen-heavy [--out local/heavy]`：種と引数を固定して作り、`index.jsonl`（道・出どころ・範囲・バイト数・BLAKE3・引数）を
//! 書く。作るものは決まっている（同じ道具の版なら同じバイト列）ので、git には入れない。
//!
//! Kura の遅い入力 S1〜S7 は、Kura の `docs/editing.md` §9.5 と `svg_worker` の注釈の説明から作る（Kura の元の入力の写しでは
//! ない。同じ性質を持つ入力）。

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
            "1000×1000・rotate(33) の下で、太さ 400 の線の 3 次曲線 40 区間（座標 ±50,000）",
            s1(),
        ),
        ("s2-many-stops", "1000×1000・stop 10 万のグラデーションで塗った rect 10 枚", s2()),
        ("s3-same-offset-stops", "1000×1000・offset がすべて 0.5 の stop 30 万のグラデーション", s3()),
        ("s4-fine-dashes-round-caps", "1000×1000・長さ 1000 の線 37 本・太さ 8・破線 0.01 0.01・丸い端", s4()),
        ("s5-rotated-pattern", "4000×4000・patternTransform rotate(30) の 4×4 のタイルで塗った全面", s5()),
        (
            "s6-textpath-unseparated-transform",
            "200×100・区切りのない数の transform（translate(10-5)）を持つ path へのふつうの textPath（かならず描く互換の例）",
            s6(),
        ),
        ("s7-turbulence-octaves", "2000×2000・numOctaves 1000 の feTurbulence", s7()),
        ("bomb-use-doubling", "200×200・use の倍々 20 段（2^20 個）", bomb_use()),
        ("bomb-pattern-nesting", "200×200・objectBoundingBox の pattern の入れ子 24 段", bomb_pattern()),
        ("bomb-marker-dup", "1000×1000・頂点 10 万の path の marker-mid（marker の複製）", bomb_marker()),
    ]
}

pub fn run(args: &[String]) -> Result<(), String> {
    let out = arg(args, "--out").map(PathBuf::from).unwrap_or_else(|| workspace_root().join("local/heavy"));
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut index = String::new();
    for (name, params, svg) in inputs() {
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
    std::fs::write(out.join("index.jsonl"), index).map_err(|e| e.to_string())
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

fn map_shapes() -> String {
    let mut r = ChaCha::from_u64(2);
    let colors = ["#e6194b", "#3cb44b", "#ffe119", "#4363d8", "#f58231", "#911eb4", "#46f0f0", "#f032e6"];
    let mut s = head(4096, 4096);
    for _ in 0..45_000 {
        let (cx, cy) = (r.below(4096) as f64, r.below(4096) as f64);
        let rad = r.range(4, 20) as f64;
        let c = colors[r.below(8) as usize];
        let _ = write!(s, "<path fill=\"{c}\" d=\"M");
        for k in 0..20 {
            // 角は決まった 20 等分。半径だけを揺らす（超越関数は道具の側なので std でよい）
            let a = std::f64::consts::TAU * f64::from(k) / 20.0;
            let rr = rad * (0.6 + 0.4 * r.unit_f64());
            let _ = write!(s, "{}{:.2} {:.2}", if k == 0 { "" } else { "L" }, cx + rr * a.cos(), cy + rr * a.sin());
        }
        s.push_str("Z\"/>\n");
    }
    s.push_str("</svg>\n");
    s
}

fn s1() -> String {
    let mut s = head(1000, 1000);
    s.push_str(
        "<path transform=\"rotate(33 500 500)\" fill=\"none\" stroke=\"#06c\" stroke-width=\"400\" d=\"M-50000 0",
    );
    for k in 0..40 {
        let x = -50_000 + (k + 1) * 2500;
        let y = if k % 2 == 0 { 50_000 } else { -50_000 };
        let _ = write!(s, "C{} {y} {} {} {x} 0", x - 2000, x - 500, -y);
    }
    s.push_str("\"/>\n</svg>\n");
    s
}

fn gradient_rects(stops: &str, n: u32) -> String {
    let mut s = head(1000, 1000);
    let _ = writeln!(s, "<defs><linearGradient id=\"g\">{stops}</linearGradient></defs>");
    for k in 0..n {
        let _ = writeln!(s, "<rect x=\"0\" y=\"{}\" width=\"1000\" height=\"100\" fill=\"url(#g)\"/>", k * 100);
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
            (k * 2654435761) & 0xff_ffff
        );
    }
    gradient_rects(&stops, 10)
}

fn s3() -> String {
    let mut stops = String::new();
    for k in 0..300_000u32 {
        let _ = write!(stops, "<stop offset=\"0.5\" stop-color=\"#{:06x}\"/>", (k * 40503) & 0xff_ffff);
    }
    gradient_rects(&stops, 1)
}

fn s4() -> String {
    let mut s = head(1000, 1000);
    s.push_str("<g stroke=\"#000\" stroke-width=\"8\" stroke-linecap=\"round\" stroke-dasharray=\"0.01 0.01\">\n");
    for k in 0..37 {
        let _ = writeln!(s, "<path d=\"M0 {0} L1000 {0}\"/>", 10 + k * 26);
    }
    s.push_str("</g>\n</svg>\n");
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
    let mut s = head(2000, 2000);
    s.push_str(
        "<filter id=\"t\" x=\"0\" y=\"0\" width=\"1\" height=\"1\"><feTurbulence baseFrequency=\"0.01\" numOctaves=\"1000\"/></filter>\n\
         <rect width=\"2000\" height=\"2000\" filter=\"url(#t)\"/>\n</svg>\n",
    );
    s
}

fn bomb_use() -> String {
    let mut s = head(200, 200);
    s.push_str("<defs><rect id=\"u0\" width=\"1\" height=\"1\" fill=\"#000\"/>\n");
    for k in 1..=20 {
        let _ =
            writeln!(s, "<g id=\"u{k}\"><use xlink:href=\"#u{0}\"/><use xlink:href=\"#u{0}\" x=\"0.001\"/></g>", k - 1);
    }
    s.push_str("</defs>\n<use xlink:href=\"#u20\"/>\n</svg>\n");
    s
}

fn bomb_pattern() -> String {
    let mut s = head(200, 200);
    s.push_str("<defs>\n<pattern id=\"p0\" width=\"0.5\" height=\"0.5\"><rect width=\"1\" height=\"1\" fill=\"#000\"/></pattern>\n");
    for k in 1..=24 {
        let _ = writeln!(
            s,
            "<pattern id=\"p{k}\" width=\"0.5\" height=\"0.5\"><rect width=\"100\" height=\"100\" fill=\"url(#p{})\"/></pattern>",
            k - 1
        );
    }
    s.push_str("</defs>\n<rect width=\"200\" height=\"200\" fill=\"url(#p24)\"/>\n</svg>\n");
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
