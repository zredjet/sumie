//! ゴールデン（determinism.md §10。`goldens/README.md`）。全ターゲットで同じファイルを照らす。
//!
//! M0 は形式の確かめだけ（描く口は M2 から。行ができたら、ここで描き直してダイジェストを照らす）：
//! - ファイルの名前は `render-v<N>.jsonl`（N ≤ 今の版）か `estimate.jsonl`。今の版のファイルがある
//! - 各行は鍵の決まった平らな JSON で、名前が重ならず、ダイジェストは 64 桁の 16 進
//! - `input` は `path` のファイルの BLAKE3 と同じ

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::disallowed_types
)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sumie_testkit::jsonl::{Line, Value};

/// render の行の鍵（この順）と、値が文字列か。
const RENDER_KEYS: &[(&str, bool)] = &[
    ("name", true),
    ("input", true),
    ("path", true),
    ("fonts", true),
    ("decoder", true),
    ("scale", true),
    ("region", true),
    ("width", false),
    ("height", false),
    ("pixels", true),
    ("report", true),
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn goldens() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens")
}

fn current_version() -> u32 {
    sumie::RENDERER.strip_prefix("sumie-render/").unwrap().parse().unwrap()
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// render の 1 行を確かめる（誤りの文を返す）。
fn check_render_line(line: &Line, root: &Path) -> Result<(), String> {
    let keys: Vec<&str> = line.fields.iter().map(|(k, _)| k.as_str()).collect();
    let want: Vec<&str> = RENDER_KEYS.iter().map(|(k, _)| *k).collect();
    if keys != want {
        return Err(format!("鍵が違う：{keys:?}（期待 {want:?}）"));
    }
    for ((k, v), (_, is_str)) in line.fields.iter().zip(RENDER_KEYS) {
        match (v, is_str) {
            (Value::Str(_), true) => {}
            (Value::Int(n), false) if *n > 0 && *n <= i128::from(u32::MAX) => {}
            _ => return Err(format!("{k} の値の型か範囲が違う：{v:?}")),
        }
    }
    for k in ["input", "pixels", "report"] {
        if !is_hex64(line.get_str(k).unwrap()) {
            return Err(format!("{k} が 64 桁の 16 進でない"));
        }
    }
    let fonts = line.get_str("fonts").unwrap();
    if !fonts.is_empty() && !fonts.split(',').all(is_hex64) {
        return Err("fonts が 64 桁の 16 進の並びでない".into());
    }
    let scale = line.get_str("scale").unwrap();
    if !scale.parse::<f64>().is_ok_and(|s| s.is_finite() && s > 0.0) {
        return Err(format!("scale が正の数でない：{scale}"));
    }
    let region = line.get_str("region").unwrap();
    if !region.is_empty() && !(region.split(',').count() == 4 && region.split(',').all(|p| p.parse::<u32>().is_ok())) {
        return Err(format!("region が x,y,w,h でない：{region}"));
    }
    let path = root.join(line.get_str("path").unwrap());
    let bytes = std::fs::read(&path).map_err(|e| format!("入力 {} を読めない：{e}", path.display()))?;
    if blake3::hash(&bytes).to_hex().as_str() != line.get_str("input").unwrap() {
        return Err(format!("入力 {} の BLAKE3 が input と違う", path.display()));
    }
    Ok(())
}

/// ファイルの全部の行を確かめる。
fn check_file(text: &str, root: &Path, render: bool) -> Vec<String> {
    let mut errs = Vec::new();
    let mut names = BTreeSet::new();
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = match Line::parse(raw) {
            Ok(l) => l,
            Err(e) => {
                errs.push(format!("{n} 行：JSON でない：{e}"));
                continue;
            }
        };
        match line.get_str("name") {
            Some(name) if !name.is_empty() => {
                if !names.insert(name.to_string()) {
                    errs.push(format!("{n} 行：名前 {name} が重なる"));
                }
            }
            _ => errs.push(format!("{n} 行：name がない")),
        }
        if render && let Err(e) = check_render_line(&line, root) {
            errs.push(format!("{n} 行：{e}"));
        }
    }
    errs
}

#[test]
fn golden_files_are_well_formed() {
    let root = workspace();
    let current = current_version();
    let mut seen_current = false;
    let mut errs = Vec::new();
    let mut names: Vec<String> = std::fs::read_dir(goldens())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".jsonl"))
        .collect();
    names.sort();
    for name in names {
        let text = std::fs::read_to_string(goldens().join(&name)).unwrap();
        if name == "estimate.jsonl" {
            errs.extend(check_file(&text, &root, false).into_iter().map(|e| format!("{name}：{e}")));
            continue;
        }
        let Some(v) = name.strip_prefix("render-v").and_then(|s| s.strip_suffix(".jsonl")) else {
            errs.push(format!("{name}：render-v<N>.jsonl か estimate.jsonl でない"));
            continue;
        };
        let Ok(v) = v.parse::<u32>() else {
            errs.push(format!("{name}：版が数でない"));
            continue;
        };
        if v > current {
            errs.push(format!("{name}：今の版 {current} より新しい"));
        }
        seen_current |= v == current;
        errs.extend(check_file(&text, &root, true).into_iter().map(|e| format!("{name}：{e}")));
    }
    assert!(seen_current, "今の版のファイル render-v{current}.jsonl がない");
    assert!(errs.is_empty(), "ゴールデンの形が違う：\n{}", errs.join("\n"));
}

/// 確かめる関数が、壊れた行を拾う（自己試験）。
#[test]
fn checker_rejects_broken_lines() {
    let root = workspace();
    // 入力に使えるファイル（このファイル自身）
    let me = "crates/sumie/tests/golden.rs";
    let digest = blake3::hash(&std::fs::read(root.join(me)).unwrap()).to_hex().to_string();
    let h = "0".repeat(64);
    let good = Line::new()
        .str("name", "a")
        .str("input", digest.as_str())
        .str("path", me)
        .str("fonts", "")
        .str("decoder", "")
        .str("scale", "1")
        .str("region", "")
        .int("width", 10)
        .int("height", 20)
        .str("pixels", h.as_str())
        .str("report", h.as_str());
    assert_eq!(check_file(&good.to_json(), &root, true), Vec::<String>::new());
    let twice = format!("{}\n{}", good.to_json(), good.to_json());
    assert_eq!(check_file(&twice, &root, true).len(), 1, "同じ名前");
    let mut bad = good.clone();
    bad.fields[1].1 = Value::Str(h.clone());
    assert_eq!(check_file(&bad.to_json(), &root, true).len(), 1, "input が違う");
    let mut bad = good.clone();
    bad.fields[5].1 = Value::Str("0".into());
    assert_eq!(check_file(&bad.to_json(), &root, true).len(), 1, "scale が 0");
    let mut bad = good.clone();
    bad.fields[6].1 = Value::Str("1,2,3".into());
    assert_eq!(check_file(&bad.to_json(), &root, true).len(), 1, "region");
    let mut bad = good.clone();
    bad.fields.swap(0, 1);
    assert_eq!(check_file(&bad.to_json(), &root, true).len(), 1, "鍵の順");
    assert_eq!(check_file("{oops", &root, true).len(), 1);
}
