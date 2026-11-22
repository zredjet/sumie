//! `kura-compat`：Kura の今の `SvgCodec` の「描く／断る」と大きさの一覧（Kura の側で作る。`corpus/kura-request.md`）と、
//! この描画器の結果を照らす（NFR-10・ADR-44 (4)。M0 で仕組み、M7 で照らす）。
//!
//! `kura-compat --kura <Kura の一覧.jsonl> [--dir <SVG の根>] [--out local/kura-compat/<名前>.jsonl]`
//!
//! - 入力は照合の集まり（`corpus/`。実例の SVG を含む。ADR-59）。ほかのフォルダの SVG（コミットできないもの）も `--dir`
//!   に置いたまま読める。結果は git の外に書き、道を書かずに入力の BLAKE3 で指す
//! - Kura の一覧が届いたら、まず resvg（Kura の設定）の大きさと照らし（一覧の写し方の誤りを見つける）、M1 から sumie の
//!   `parse`・`pixel_size`・形（ADR-37）の結果を並べる。`kura_ok`（描く）は形と値の両方が `ok` のもの、大きさは形が
//!   `ok` のものだけ照らす（ADR-62 (4)）

use std::path::PathBuf;

use sumie_testkit::jsonl::Line;

use crate::corpus::{arg, workspace_root};
use crate::oracle::{Fonts, Resvg};

/// Kura の一覧の 1 行と、入力のダイジェスト・resvg の大きさ（か誤り）から、結果の 1 行を作る。
/// `size_same` は Kura と resvg の両方が読めたときだけ（どちらかが断ったものは `kura_ok`・`resvg_ok` で別に見る）。
pub fn result_line(k: &Line, digest: &str, resvg: &Result<(u32, u32), String>) -> Result<(Line, bool), String> {
    let path = k.get_str("path").unwrap_or("");
    match k.get_str("blake3") {
        Some(b) if b == digest => {}
        Some(_) => return Err(format!("{path} のダイジェストが Kura の一覧と違う（別の版の入力）")),
        None => return Err(format!("{path} の行に blake3 がない（入力を照らせない）")),
    }
    let shape_ok = k.get_str("shape") == Some("ok");
    let kura_ok = shape_ok && k.get_str("decode") == Some("ok");
    let kw = k.get_int("width").unwrap_or(0);
    let kh = k.get_int("height").unwrap_or(0);
    let (rw, rh, rerr) = match resvg {
        Ok((w, h)) => (i128::from(*w), i128::from(*h), String::new()),
        Err(e) => (0, 0, e.clone()),
    };
    let both = shape_ok && resvg.is_ok();
    let same = both && kw == rw && kh == rh;
    let line = Line::new()
        .str("blake3", digest)
        .str("kura_shape", k.get_str("shape").unwrap_or(""))
        .str("kura_decode", k.get_str("decode").unwrap_or(""))
        .bool("kura_ok", kura_ok)
        .bool("kura_shape_ok", shape_ok)
        .int("kura_w", kw)
        .int("kura_h", kh)
        .bool("resvg_ok", resvg.is_ok())
        .int("resvg_w", rw)
        .int("resvg_h", rh)
        .str("resvg_error", rerr.as_str())
        .bool("size_same", same)
        .str("sumie", "M1 から");
    Ok((line, both && !same))
}

pub fn run(args: &[String]) -> Result<(), String> {
    crate::corpus::check_args(args, &["--kura", "--dir", "--out"], &[], 0)?;
    let kura = PathBuf::from(arg(args, "--kura").ok_or("--kura <Kura の一覧.jsonl>")?);
    let dir = arg(args, "--dir").map(PathBuf::from).unwrap_or_else(|| workspace_root().join("corpus"));
    let out = arg(args, "--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("local/kura-compat/result.jsonl"));
    let text = std::fs::read_to_string(&kura).map_err(|e| format!("{}：{e}", kura.display()))?;
    let resvg = Resvg::new(&Fonts::System);
    let mut lines = String::new();
    let (mut n, mut size_mismatch, mut missing, mut disagree) = (0usize, 0usize, Vec::new(), 0usize);
    for l in text.lines() {
        let k = Line::parse(l)?;
        let path = k.get_str("path").ok_or("Kura の一覧に path がない")?;
        let Ok(svg) = std::fs::read(dir.join(path)) else {
            missing.push(path.to_string());
            continue;
        };
        let digest = blake3::hash(&svg).to_hex().to_string();
        let size = resvg.tree(&svg).map(|t| Resvg::size(&t));
        let (line, mismatch) = result_line(&k, &digest, &size)?;
        size_mismatch += usize::from(mismatch);
        disagree += usize::from(line.get_bool("kura_ok") != line.get_bool("resvg_ok"));
        lines.push_str(&line.to_json());
        lines.push('\n');
        n += 1;
    }
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(&out, lines).map_err(|e| e.to_string())?;
    println!(
        "{n} 件・Kura と resvg の大きさの食い違い {size_mismatch} 件・片方だけが読めた {disagree} 件 → {}",
        out.display()
    );
    if !missing.is_empty() {
        return Err(format!(
            "入力が見つからない {} 件（--dir を確かめる）：{:?}",
            missing.len(),
            &missing[..missing.len().min(5)]
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_lines() {
        let k = |shape: &str, b: Option<&str>| {
            let mut l = Line::new()
                .str("path", "a.svg")
                .str("shape", shape)
                .str("decode", if shape == "ok" { "ok" } else { shape })
                .int("width", 10)
                .int("height", 20);
            if let Some(b) = b {
                l = l.str("blake3", b);
            }
            l
        };
        let (l, bad) = result_line(&k("ok", Some("d")), "d", &Ok((10, 20))).unwrap();
        assert_eq!((l.get_bool("size_same"), bad), (Some(true), false));
        let (_, bad) = result_line(&k("ok", Some("d")), "d", &Ok((11, 20))).unwrap();
        assert!(bad);
        // Kura が断ったものは大きさを照らさない（size_same は偽、食い違いにも数えない）
        let (l, bad) = result_line(&k("Limit", Some("d")), "d", &Ok((10, 20))).unwrap();
        assert_eq!((l.get_bool("size_same"), l.get_bool("kura_ok"), bad), (Some(false), Some(false), false));
        assert!(result_line(&k("ok", Some("x")), "d", &Ok((10, 20))).is_err());
        assert!(result_line(&k("ok", None), "d", &Ok((10, 20))).is_err());
        // 値で断ったものは「描く」に数えない（大きさは形で照らす）
        let dec = Line::new().str("path", "a.svg").str("blake3", "d").str("shape", "ok").str("decode", "Limit");
        let (l, _) = result_line(&dec.int("width", 10).int("height", 20), "d", &Ok((10, 20))).unwrap();
        assert_eq!((l.get_bool("kura_ok"), l.get_bool("size_same")), (Some(false), Some(true)));
    }
}
