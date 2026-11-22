//! ゴールデン（determinism.md §10。`goldens/README.md`）。全ターゲットで同じファイルを照らす。
//!
//! M0 は形の確かめだけ（描く口は M2 から。行ができたら、ここで今の版の行を描き直してダイジェストを照らす）：
//! - ファイルの名前は `render-v<N>.jsonl`（N は正規の 10 進。N ≤ 今の版）か `estimate.jsonl`。今の版のファイルがある
//! - 各行は鍵の決まった平らな JSON で、バイトが正規の形（読み直して書くと同じ）、名前が重ならない。ファイルは改行で
//!   終わり、CR を持たない
//! - 値の字句が正規（scale・region・ダイジェスト）。region は width・height と合う
//! - 今の版の行の入力はダイジェストで引ける（`tests/svg/`・`corpus/` のどれか）。`path` は手がかりで、その下の相対の道
//! - 古い版の行は形だけを見る（入力を動かしても困らない。照らすのは今の版だけ。ADR-41 (7)）。古い版の例の名前は
//!   すべて次の版にもある（版を上げたら全部の例を描き直す）

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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sumie_testkit::jsonl::{Line, Value};

/// render の行の鍵（この順）と、値が文字列か。
const RENDER_KEYS: &[(&str, bool)] = &[
    ("name", true),
    ("input", true),
    ("path", true),
    ("fonts", true),
    ("decoder", true),
    ("style", true),
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

/// 正規の 10 進の自然数（先頭の 0・符号なし）。
fn is_canonical_uint(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
}

/// 正規の倍率の字句（`1`・`0.5`・`2.25`。符号・指数・末尾の 0・先頭の余計な 0 なし。正の有限）。
fn is_canonical_scale(s: &str) -> bool {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let frac_ok = frac.is_empty() || (frac.bytes().all(|b| b.is_ascii_digit()) && !frac.ends_with('0'));
    let dot_ok = !s.contains('.') || !frac.is_empty();
    is_canonical_uint(int) && frac_ok && dot_ok && s.parse::<f64>().is_ok_and(|v| v.is_finite() && v > 0.0)
}

/// 入力の場所を引く表（BLAKE3 → 相対の道）。`tests/svg/` と `corpus/` の下の SVG。
fn input_index(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for top in ["tests/svg", "corpus"] {
        let mut stack = vec![root.join(top)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "svg") {
                    let h = blake3::hash(&std::fs::read(&p).unwrap()).to_hex().to_string();
                    let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                    out.entry(h).or_insert(rel);
                }
            }
        }
    }
    out
}

/// フォントの鍵を引く表（`tests/fonts/` の下のファイルの BLAKE3）。
fn font_index(root: &Path) -> BTreeSet<String> {
    let Ok(rd) = std::fs::read_dir(root.join("tests/fonts")) else { return BTreeSet::new() };
    rd.flatten()
        .filter(|e| e.path().is_file())
        .map(|e| blake3::hash(&std::fs::read(e.path()).unwrap()).to_hex().to_string())
        .collect()
}

/// 照らすときに引く表（今の版の行だけが使う）。
struct Index {
    inputs: BTreeMap<String, String>,
    fonts: BTreeSet<String>,
}

/// render の 1 行を確かめる（誤りの文を返す）。`index` は今の版の行のときだけ渡す。
fn check_render_line(line: &Line, index: Option<&Index>) -> Result<(), String> {
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
    let get = |k: &str| line.get_str(k).unwrap();
    for k in ["input", "pixels", "report"] {
        if !is_hex64(get(k)) {
            return Err(format!("{k} が 64 桁の小文字の 16 進でない"));
        }
    }
    let fonts = get("fonts");
    if !fonts.is_empty() && !fonts.split(',').all(is_hex64) {
        return Err("fonts が 64 桁の 16 進の並びでない".into());
    }
    let style = get("style");
    if style != "default" && !is_hex64(style) {
        return Err(format!("style が default か読みの設定のダイジェストでない：{style}"));
    }
    if !is_canonical_scale(get("scale")) {
        return Err(format!("scale が正規の正の数でない：{}", get("scale")));
    }
    let region = get("region");
    if !region.is_empty() {
        let parts: Vec<&str> = region.split(',').collect();
        if parts.len() != 4 || !parts.iter().all(|p| is_canonical_uint(p)) {
            return Err(format!("region が x,y,w,h（正規の 10 進）でない：{region}"));
        }
        let (w, h) = (parts[2].parse::<i128>().unwrap(), parts[3].parse::<i128>().unwrap());
        if w == 0 || h == 0 || Some(w) != line.get_int("width") || Some(h) != line.get_int("height") {
            return Err(format!("region の w・h が 0 か、width・height と違う：{region}"));
        }
    }
    let path = get("path");
    if path.contains("..") || path.starts_with('/') || !(path.starts_with("tests/svg/") || path.starts_with("corpus/"))
    {
        return Err(format!("path は tests/svg/ か corpus/ の下の相対の道：{path}"));
    }
    if let Some(ix) = index {
        if !ix.inputs.contains_key(get("input")) {
            return Err(format!(
                "入力のダイジェスト {} が tests/svg・corpus のどれにも当たらない（手がかり {path}）",
                get("input")
            ));
        }
        if !fonts.is_empty() && !fonts.split(',').all(|f| ix.fonts.contains(f)) {
            return Err("fonts の鍵が tests/fonts のどれにも当たらない".into());
        }
    }
    Ok(())
}

/// ファイルの全部の行を確かめ、例の名前を返す。
fn check_file(text: &str, render: bool, index: Option<&Index>) -> (Vec<String>, BTreeSet<String>) {
    let mut errs = Vec::new();
    let mut names = BTreeSet::new();
    if text.contains('\r') {
        errs.push("CR がある".to_string());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        errs.push("改行で終わらない".to_string());
    }
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = match Line::parse(raw) {
            Ok(l) => l,
            Err(e) => {
                errs.push(format!("{n} 行：JSON でない：{e}"));
                continue;
            }
        };
        if line.to_json() != raw {
            errs.push(format!("{n} 行：正規の形でない（空白・エスケープ・数の書き方）"));
        }
        match line.get_str("name") {
            Some(name) if !name.is_empty() => {
                if !names.insert(name.to_string()) {
                    errs.push(format!("{n} 行：名前 {name} が重なる"));
                }
            }
            _ => errs.push(format!("{n} 行：name がない")),
        }
        if render && let Err(e) = check_render_line(&line, index) {
            errs.push(format!("{n} 行：{e}"));
        }
    }
    (errs, names)
}

#[test]
fn golden_files_are_well_formed() {
    let root = workspace();
    let current = current_version();
    let index = Index { inputs: input_index(&root), fonts: font_index(&root) };
    let mut errs = Vec::new();
    let mut by_version: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
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
            errs.extend(check_file(&text, false, None).0.into_iter().map(|e| format!("{name}：{e}")));
            continue;
        }
        let v = name.strip_prefix("render-v").and_then(|s| s.strip_suffix(".jsonl")).filter(|v| is_canonical_uint(v));
        let Some(v) = v.and_then(|v| v.parse::<u32>().ok()) else {
            errs.push(format!("{name}：render-v<N>.jsonl（N は正規の 10 進）か estimate.jsonl でない"));
            continue;
        };
        if v > current {
            errs.push(format!("{name}：今の版 {current} より新しい"));
        }
        let ix = if v == current { Some(&index) } else { None };
        let (e, ns) = check_file(&text, true, ix);
        errs.extend(e.into_iter().map(|e| format!("{name}：{e}")));
        by_version.insert(v, ns);
    }
    assert!(by_version.contains_key(&current), "今の版のファイル render-v{current}.jsonl がない");
    // 版を上げたら全部の例を描き直す（古い版の例の名前は次の版にもある。ADR-41 (7)）
    for (v, ns) in &by_version {
        if let Some(next) = by_version.get(&(v + 1)) {
            let missing: Vec<&String> = ns.difference(next).collect();
            if !missing.is_empty() {
                errs.push(format!("render-v{v} の例 {missing:?} が render-v{} にない", v + 1));
            }
        }
    }
    assert!(errs.is_empty(), "ゴールデンの形が違う：\n{}", errs.join("\n"));
}

/// 確かめる関数が、壊れた行を拾う（自己試験）。
#[test]
fn checker_rejects_broken_lines() {
    let digest = "1".repeat(64);
    let index = Index { inputs: [(digest.clone(), "tests/svg/a.svg".to_string())].into(), fonts: BTreeSet::new() };
    let h = "0".repeat(64);
    let good = Line::new()
        .str("name", "a")
        .str("input", digest.as_str())
        .str("path", "tests/svg/a.svg")
        .str("fonts", "")
        .str("decoder", "")
        .str("style", "default")
        .str("scale", "1.5")
        .str("region", "")
        .int("width", 10)
        .int("height", 20)
        .str("pixels", h.as_str())
        .str("report", h.as_str());
    let text = |l: &Line| format!("{}\n", l.to_json());
    let errs = |t: &str| check_file(t, true, Some(&index)).0.len();
    assert_eq!(errs(&text(&good)), 0);
    assert_eq!(errs(&format!("{}{}", text(&good), text(&good))), 1, "同じ名前");
    assert_eq!(errs(&good.to_json()), 1, "改行で終わらない");
    assert_eq!(errs(&text(&good).replace('\n', "\r\n")), 1, "CR");
    assert_eq!(errs(&text(&good).replacen(':', ": ", 1)), 1, "正規の形でない");
    let set = |k: usize, v: &str| {
        let mut l = good.clone();
        l.fields[k].1 = Value::Str(v.to_string());
        text(&l)
    };
    assert_eq!(errs(&set(1, &h)), 1, "入力のダイジェストが当たらない");
    assert_eq!(check_file(&set(1, &h), true, None).0.len(), 0, "古い版は入力を引かない");
    assert_eq!(errs(&set(2, "../x.svg")), 1, "path の ..");
    assert_eq!(errs(&set(2, "/tmp/x.svg")), 1, "path が絶対");
    assert_eq!(errs(&set(3, &h)), 1, "fonts の鍵が tests/fonts にない");
    assert_eq!(errs(&set(5, "mine")), 1, "style");
    for bad in ["0", "1e0", "+1", "1.0", ".5", "01", "1.", "-1"] {
        assert_eq!(errs(&set(6, bad)), 1, "scale {bad}");
    }
    assert_eq!(errs(&set(7, "1,2,3")), 1, "region の数");
    assert_eq!(errs(&set(7, "0,0,10,20")), 0);
    assert_eq!(errs(&set(7, "0,0,10,21")), 1, "region と height");
    assert_eq!(errs(&set(7, "0,+1,10,20")), 1, "region の +");
    assert_eq!(errs(&set(7, "0,0,0,20")), 1, "region の w = 0");
    let mut swapped = good.clone();
    swapped.fields.swap(0, 1);
    assert_eq!(errs(&text(&swapped)), 1, "鍵の順");
    assert_eq!(errs("{oops\n"), 1);
}

#[test]
fn canonical_lexemes() {
    assert!(is_canonical_scale("1") && is_canonical_scale("0.5") && is_canonical_scale("2.25"));
    assert!(is_canonical_uint("0") && is_canonical_uint("10") && !is_canonical_uint("01") && !is_canonical_uint(""));
}
