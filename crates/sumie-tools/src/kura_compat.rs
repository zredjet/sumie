//! `kura-compat`：Kura の今の `SvgCodec` の「描く／断る」と大きさの一覧（Kura の側で作る。`corpus/kura-request.md`）と、
//! この描画器の結果を照らす（NFR-10・ADR-44 (4)。M0 で仕組み、M7 で照らす）。
//!
//! `kura-compat --kura <Kura の一覧.jsonl> [--dir <SVG の根>] [--out local/kura-compat/<名前>.jsonl]`
//!
//! - Kura の手元の SVG（ライセンスでコミットできない）は `--dir` に置いたまま読み、git の外に結果の一覧だけを書く。
//!   一覧には道を書かず、入力の BLAKE3 で指す
//! - M0 は sumie の読む口がないので、resvg（Kura の設定）の大きさを並べ、Kura の一覧の大きさと照らす（一覧の写し方の
//!   誤りを見つける）。M1 から sumie の `parse`・`pixel_size`・形（ADR-37）の結果を並べる

use std::path::PathBuf;

use sumie_testkit::jsonl::Line;

use crate::corpus::{arg, workspace_root};
use crate::oracle::{Fonts, Resvg};

pub fn run(args: &[String]) -> Result<(), String> {
    let kura = PathBuf::from(arg(args, "--kura").ok_or("--kura <Kura の一覧.jsonl>")?);
    let dir = arg(args, "--dir").map(PathBuf::from).unwrap_or_else(|| workspace_root().join("corpus"));
    let out = arg(args, "--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("local/kura-compat/result.jsonl"));
    let text = std::fs::read_to_string(&kura).map_err(|e| format!("{}：{e}", kura.display()))?;
    let resvg = Resvg::new(&Fonts::System);
    let mut lines = String::new();
    let (mut n, mut size_mismatch, mut missing) = (0usize, 0usize, 0usize);
    for l in text.lines() {
        let k = Line::parse(l)?;
        let path = k.get_str("path").ok_or("Kura の一覧に path がない")?;
        let file = dir.join(path);
        let Ok(svg) = std::fs::read(&file) else {
            missing += 1;
            continue;
        };
        let digest = blake3::hash(&svg).to_hex().to_string();
        if k.get_str("blake3").is_some_and(|b| b != digest) {
            return Err(format!("{path} のダイジェストが Kura の一覧と違う（別の版の入力）"));
        }
        let (rw, rh, rerr) = match resvg.tree(&svg) {
            Ok(t) => {
                let (w, h) = Resvg::size(&t);
                (i128::from(w), i128::from(h), String::new())
            }
            Err(e) => (0, 0, e),
        };
        let kw = k.get_int("width").unwrap_or(0);
        let kh = k.get_int("height").unwrap_or(0);
        let same = k.get_str("shape") != Some("ok") || (kw == rw && kh == rh);
        size_mismatch += usize::from(!same);
        let line = Line::new()
            .str("blake3", digest.as_str())
            .str("kura_shape", k.get_str("shape").unwrap_or(""))
            .str("kura_decode", k.get_str("decode").unwrap_or(""))
            .int("kura_w", kw)
            .int("kura_h", kh)
            .int("resvg_w", rw)
            .int("resvg_h", rh)
            .str("resvg_error", rerr.as_str())
            .bool("size_same", same)
            .str("sumie", "M1 から");
        lines.push_str(&line.to_json());
        lines.push('\n');
        n += 1;
    }
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(&out, lines).map_err(|e| e.to_string())?;
    println!(
        "{n} 件（入力の見つからない {missing} 件）・Kura と resvg の大きさの食い違い {size_mismatch} 件 → {}",
        out.display()
    );
    Ok(())
}
