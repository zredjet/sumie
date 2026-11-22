//! 大きさの照らし（`sumie-tools sizes`。FR-OUT-02・NFR-10・roadmap M1）。照合の集まりの SVG を、Kura の設定の resvg と
//! sumie の `parse` で読み、受ける・断ると大きさを比べる。
//!
//! - 同じ：どちらも読み、sumie の `Document::size` が resvg の木の大きさと同じ（倍率 1 の画素の大きさも同じ）
//! - 中身で決まる：sumie の `Document::size` が `None`（木を作るまで決まらない。M2 から比べる）
//! - 違う：どちらも読み、大きさが違う
//! - sumie が断る：resvg は読むのに sumie の `parse` が誤り（NFR-10 に触れる。一覧にして承認を得る）
//! - resvg が断る：resvg が誤り（sumie は読んでも断ってもよい）

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sumie_testkit::jsonl::Line;

use crate::corpus::{arg, check_args, workspace_root};
use crate::oracle::{Fonts, Resvg};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    Same,
    Content,
    Differ,
    SumieRefuses,
    ResvgRefuses,
}

fn walk_svgs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_svgs(&p, out);
        } else if p.extension().is_some_and(|x| x == "svg") {
            out.push(p);
        }
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    check_args(args, &["--root", "--out", "--fonts"], &[], 0)?;
    let ws = workspace_root();
    let root = arg(args, "--root").map(PathBuf::from).unwrap_or_else(|| ws.join("corpus"));
    let out_dir = arg(args, "--out").map(PathBuf::from).unwrap_or_else(|| ws.join("local/sizes"));
    // 大きさはフォントに依らない（中身で決まる文書は M2 から比べる）。既定は書体なし
    let fonts = Fonts::parse(arg(args, "--fonts").unwrap_or("none"))?;
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let resvg = Resvg::new(&fonts);
    let mut files = Vec::new();
    walk_svgs(&root, &mut files);
    files.sort();
    let mut counts: std::collections::BTreeMap<Verdict, usize> = Default::default();
    let mut lines = String::new();
    let mut rows = String::new();
    let opts = sumie::ParseOptions::DEFAULT;
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().to_string();
        let data = std::fs::read(f).map_err(|e| e.to_string())?;
        let r = resvg.tree(&data);
        let s = sumie::parse(&data, &opts, &sumie::Never);
        let (verdict, rtext, stext) = match (&r, &s) {
            (Err(e), _) => {
                (Verdict::ResvgRefuses, e.clone(), s.as_ref().map_or_else(|e| format!("{e:?}"), |_| "読む".into()))
            }
            (Ok(_), Err(e)) => (Verdict::SumieRefuses, "読む".into(), format!("{e:?}")),
            (Ok(t), Ok(d)) => {
                let (rw, rh) = Resvg::css_size(t);
                let (pw, ph) = Resvg::size(t);
                match d.size() {
                    None => (Verdict::Content, format!("{rw}×{rh}"), "中身で決まる".into()),
                    Some(sz) => {
                        let px = d.pixel_size(1.0).ok().flatten();
                        // resvg は f32 で持つ。f32 にして比べる
                        let same = (sz.width as f32) == rw
                            && (sz.height as f32) == rh
                            && px.is_some_and(|p| (p.width, p.height) == (pw, ph));
                        let v = if same { Verdict::Same } else { Verdict::Differ };
                        (
                            v,
                            format!("{rw}×{rh}（{pw}×{ph}）"),
                            format!("{}×{}（{:?}）", sz.width, sz.height, px.map(|p| (p.width, p.height))),
                        )
                    }
                }
            }
        };
        *counts.entry(verdict).or_default() += 1;
        let key = format!("{verdict:?}");
        let l = Line::new()
            .str("name", rel.as_str())
            .str("verdict", key.as_str())
            .str("resvg", rtext.as_str())
            .str("sumie", stext.as_str());
        lines.push_str(&l.to_json());
        lines.push('\n');
        if matches!(verdict, Verdict::Differ | Verdict::SumieRefuses) {
            let _ = writeln!(rows, "| `{rel}` | {key} | {rtext} | {stext} |");
        }
    }
    std::fs::write(out_dir.join("result.jsonl"), lines).map_err(|e| e.to_string())?;
    let mut md = String::from("# 大きさの照らし（resvg と sumie）\n\n| 判定 | 件数 |\n|---|---|\n");
    for (v, n) in &counts {
        let _ = writeln!(md, "| {v:?} | {n} |");
    }
    md.push_str("\n## 違う・sumie が断る\n\n| 名前 | 判定 | resvg | sumie |\n|---|---|---|---|\n");
    md.push_str(&rows);
    std::fs::write(out_dir.join("summary.md"), &md).map_err(|e| e.to_string())?;
    println!("{} 件 → {}", files.len(), out_dir.join("summary.md").display());
    for (v, n) in &counts {
        println!("  {v:?}: {n}");
    }
    Ok(())
}
