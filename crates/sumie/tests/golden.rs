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

/// 正規の倍率の字句：正の有限の f64 を Rust の `{}` で書いた字句と同じ（`1`・`0.5`・`2.25`。同じ値の字句は 1 つ）。
fn is_canonical_scale(s: &str) -> bool {
    s.parse::<f64>().is_ok_and(|v| v.is_finite() && v > 0.0 && format!("{v}") == s)
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

/// 版をまたいで同じ名前の例が変えてはいけない鍵（入力と設定。描いた結果と道の手がかりは変わってよい）。
const SAME_ACROSS_VERSIONS: &[&str] = &["input", "fonts", "decoder", "style", "scale", "region"];

#[test]
fn golden_files_are_well_formed() {
    let root = workspace();
    let current = current_version();
    let mut errs = Vec::new();
    let mut by_version: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    let mut lines_by_version: BTreeMap<u32, Vec<Line>> = BTreeMap::new();
    let all: Vec<String> =
        std::fs::read_dir(goldens()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    for n in &all {
        if !n.ends_with(".jsonl") && n != "README.md" {
            errs.push(format!("{n}：goldens/ には .jsonl と README.md だけを置く（打ち間違いを見逃さない）"));
        }
    }
    let mut names: Vec<String> = all.into_iter().filter(|n| n.ends_with(".jsonl")).collect();
    names.sort();
    // 入力の索引は、今の版の行があるときだけ作る（集まり全体のダイジェストを取るので）
    let cur_text = std::fs::read_to_string(goldens().join(format!("render-v{current}.jsonl"))).unwrap_or_default();
    let index = if cur_text.trim().is_empty() {
        Index { inputs: BTreeMap::new(), fonts: BTreeSet::new() }
    } else {
        Index { inputs: input_index(&root), fonts: font_index(&root) }
    };
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
        lines_by_version.insert(v, text.lines().filter_map(|l| Line::parse(l).ok()).collect());
    }
    assert!(by_version.contains_key(&current), "今の版のファイル render-v{current}.jsonl がない");
    // 版は 0 から今の版まで切れ目なくある
    for v in 0..=current {
        if !by_version.contains_key(&v) {
            errs.push(format!("render-v{v}.jsonl がない（版は切れ目なく並べる）"));
        }
    }
    // 版を上げたら全部の例を描き直す（古い版の例の名前は次の版にもある。ADR-41 (7)）。同じ名前の例は、入力と設定が同じ
    for (v, ns) in &by_version {
        if let Some(next) = by_version.get(&(v + 1)) {
            let missing: Vec<&String> = ns.difference(next).collect();
            if !missing.is_empty() {
                errs.push(format!("render-v{v} の例 {missing:?} が render-v{} にない", v + 1));
            }
        }
        if let (Some(a), Some(b)) = (lines_by_version.get(v), lines_by_version.get(&(v + 1))) {
            for la in a {
                let name = la.get_str("name");
                if let Some(lb) = b.iter().find(|l| l.get_str("name") == name) {
                    for k in SAME_ACROSS_VERSIONS {
                        if la.get(k) != lb.get(k) {
                            errs.push(format!(
                                "例 {name:?} の {k} が render-v{v} と render-v{} で違う（中身を変えるなら新しい名前）",
                                v + 1
                            ));
                        }
                    }
                }
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
    for bad in ["0", "1e0", "+1", "1.0", ".5", "01", "1.", "-1", "1.00000000000000001", "0.50"] {
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

// ---- D2 のゴールデン（goldens/estimate.jsonl。ADR-63 (4)・goldens/README.md） ----

/// estimate.jsonl の行の鍵（この順。値はすべて文字列）。
const ESTIMATE_KEYS: &[&str] = &["name", "input", "parse", "build", "report", "needs", "size"];

/// 見積もりの結果の字句（`ok 手間 山 持ち続ける量` か `err 誤り`）。
fn estimate_text(r: Result<sumie::Estimate, sumie::Error>) -> String {
    match r {
        Ok(e) => format!("ok {} {} {}", e.work, e.peak_bytes, e.result_bytes),
        Err(e) => format!("err {e:?}"),
    }
}

/// 入力 `data` の見積もりの行（`estimate_parse`・`estimate_build` の結果、警告と読まなかった参照の並びの数とダイジェスト、
/// 要るもの（文字・ラスターの画像・SVG の画像を 0 と 1 で）、大きさ（Rust の `{}` の字句か `content`）。読めなければ
/// 読んだあとの欄は `-`）。
fn estimate_line(name: &str, data: &[u8]) -> Line {
    let opts = sumie::ParseOptions::DEFAULT;
    let parse = estimate_text(sumie::estimate_parse(data, &opts, &sumie::Never));
    let (build, report, needs, size) = match sumie::parse(data, &opts, &sumie::Never) {
        Ok(d) => {
            let mut text = String::new();
            let mut n = 0usize;
            for w in d.report().warnings() {
                text.push_str(&format!("w {:?} {:?}\n", w.kind, w.position));
                n += 1;
            }
            for u in d.report().unread_refs() {
                let hex: String = u.href.iter().map(|b| format!("{b:02x}")).collect();
                text.push_str(&format!("u {hex} {}\n", u.position));
                n += 1;
            }
            let nd = d.needs();
            let bit = |b: bool| if b { '1' } else { '0' };
            (
                estimate_text(d.estimate_build(&sumie::Never)),
                format!("{n} {}", blake3::hash(text.as_bytes()).to_hex()),
                format!("{}{}{}", bit(nd.text), bit(nd.raster_images), bit(nd.svg_images)),
                match d.size() {
                    Some(s) => format!("{} {}", s.width, s.height),
                    None => "content".to_string(),
                },
            )
        }
        Err(_) => ("-".into(), "-".into(), "-".into(), "-".into()),
    };
    Line::new()
        .str("name", name)
        .str("input", blake3::hash(data).to_hex().to_string())
        .str("parse", parse)
        .str("build", build)
        .str("report", report)
        .str("needs", needs)
        .str("size", size)
}

/// estimate.jsonl に入れる入力：`tests/svg/` のすべてと、`corpus/` のうち BLAKE3 の頭の 16 進が 0・1・2 のもの（3/16。
/// rev ごとに作り直すファイルを小さく保つ）。名前は道。
fn estimate_inputs(index: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = index
        .iter()
        .filter(|(h, p)| p.starts_with("tests/svg/") || h.starts_with(['0', '1', '2']))
        .map(|(h, p)| (p.clone(), h.clone()))
        .collect();
    v.sort();
    v
}

/// 見積もり・誤り・警告が、このターゲットとビルドの設定でゴールデンと同じ（D2。全ターゲットで同じファイルを照らす）。
#[test]
fn estimates_match_the_golden() {
    let root = workspace();
    let text = std::fs::read_to_string(goldens().join("estimate.jsonl")).unwrap();
    let index = input_index(&root);
    let mut errs = Vec::new();
    let mut n = 0usize;
    for (i, raw) in text.lines().enumerate() {
        let line = Line::parse(raw).unwrap();
        let keys: Vec<&str> = line.fields.iter().map(|(k, _)| k.as_str()).collect();
        if keys != ESTIMATE_KEYS {
            errs.push(format!("{} 行：鍵が {ESTIMATE_KEYS:?} でない", i + 1));
            continue;
        }
        let (name, input) = (line.get_str("name").unwrap(), line.get_str("input").unwrap());
        let Some(path) = index.get(input) else {
            errs.push(format!("{} 行：入力 {input} が tests/svg・corpus にない", i + 1));
            continue;
        };
        let data = std::fs::read(root.join(path)).unwrap();
        let got = estimate_line(name, &data).to_json();
        if got != raw {
            errs.push(format!("{name}：\n  ゴールデン {raw}\n  このターゲット {got}"));
        }
        n += 1;
    }
    assert!(
        n >= 100,
        "estimate.jsonl の行が少ない（{n}）。`cargo test -p sumie --test golden -- --ignored regenerate_estimates` で作る"
    );
    assert!(
        errs.is_empty(),
        "D2 がゴールデンと違う（{} 件）：\n{}",
        errs.len(),
        errs.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
}

/// estimate.jsonl を作り直す（rev ごとに作り直してよい。`cargo test -p sumie --test golden -- --ignored regenerate_estimates`）。
#[test]
#[ignore]
fn regenerate_estimates() {
    let root = workspace();
    let index = input_index(&root);
    let mut out = String::new();
    for (path, _) in estimate_inputs(&index) {
        let data = std::fs::read(root.join(&path)).unwrap();
        out.push_str(&estimate_line(&path, &data).to_json());
        out.push('\n');
    }
    std::fs::write(goldens().join("estimate.jsonl"), out).unwrap();
}

/// 版に入る報告のダイジェスト（README の並べ方：使ったフォントの数（u32 LE）と各 32 バイトの鍵と u32 の書体の番号・
/// 見つからなかった文字の数と各 u32 の符号位置・デコーダの識別子の長さ（u32）とバイト列。M2 はフォントもデコーダもなく、
/// 12 バイトの 0）。
fn report_digest() -> String {
    blake3::hash(&[0u8; 12]).to_hex().to_string()
}

/// 描いた画素のダイジェスト（幅・高さの u32 LE と、行を詰めた RGBA8 乗算済み）。
fn pixels_digest(w: u32, h: u32, px: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&w.to_le_bytes());
    hasher.update(&h.to_le_bytes());
    hasher.update(px);
    hasher.finalize().to_hex().to_string()
}

/// 読む → 木 → 計画 → 描く（`region` は範囲。なければ全体）。描けなければ誤りの文。
fn render(data: &[u8], scale: f64, region: Option<(u32, u32, u32, u32)>) -> Result<(u32, u32, Vec<u8>), String> {
    let doc = sumie::parse(data, &sumie::ParseOptions::DEFAULT, &sumie::Never).map_err(|e| format!("{e:?}"))?;
    let tree = doc.build(&sumie::Never).map_err(|e| format!("{e:?}"))?;
    let opts = sumie::RenderOptions { scale, ..sumie::RenderOptions::DEFAULT };
    let plan = tree.plan(&opts, &sumie::Never).map_err(|e| format!("{e:?}"))?;
    let size = plan.pixel_size();
    let rect = match region {
        Some((x, y, w, h)) => sumie::PixelRect { x, y, width: w, height: h },
        None => sumie::PixelRect { x: 0, y: 0, width: size.width, height: size.height },
    };
    let mut px = vec![0u8; rect.width as usize * rect.height as usize * 4];
    let mut out = sumie::PixmapMut::new(&mut px, rect.width, rect.height, rect.width as usize * 4)
        .map_err(|e| format!("{e:?}"))?;
    plan.render_region(rect, &mut out, &sumie::Never).map_err(|e| format!("{e:?}"))?;
    Ok((rect.width, rect.height, px))
}

/// 描いた行。
fn render_line(name: &str, path: &str, data: &[u8], scale: f64, region: Option<(u32, u32, u32, u32)>) -> Option<Line> {
    let (w, h, px) = render(data, scale, region).ok()?;
    let region_text = region.map_or(String::new(), |(x, y, w, h)| format!("{x},{y},{w},{h}"));
    Some(
        Line::new()
            .str("name", name)
            .str("input", blake3::hash(data).to_hex().to_string())
            .str("path", path)
            .str("fonts", "")
            .str("decoder", "")
            .str("style", "default")
            .str("scale", format!("{scale}"))
            .str("region", region_text)
            .int("width", i128::from(w))
            .int("height", i128::from(h))
            .str("pixels", pixels_digest(w, h, &px))
            .str("report", report_digest()),
    )
}

/// 今の版の行を描き直して、画素と大きさと報告が同じ（D1。全ターゲットで同じファイル）。
#[test]
fn renders_match_the_golden() {
    let root = workspace();
    let current = current_version();
    let text = std::fs::read_to_string(goldens().join(format!("render-v{current}.jsonl"))).unwrap();
    let index = input_index(&root);
    let mut errs = Vec::new();
    let mut n = 0usize;
    for raw in text.lines() {
        let line = Line::parse(raw).unwrap();
        let get = |k: &str| line.get_str(k).unwrap().to_string();
        let Some(path) = index.get(&get("input")) else {
            errs.push(format!("{}：入力がない", get("name")));
            continue;
        };
        let data = std::fs::read(root.join(path)).unwrap();
        let scale: f64 = get("scale").parse().unwrap();
        let region = (!get("region").is_empty()).then(|| {
            let p: Vec<u32> = get("region").split(',').map(|v| v.parse().unwrap()).collect();
            (p[0], p[1], p[2], p[3])
        });
        match render_line(&get("name"), &get("path"), &data, scale, region) {
            Some(got) if got.to_json() == raw => {}
            Some(got) => {
                errs.push(format!("{}：\n  ゴールデン {raw}\n  このターゲット {}", get("name"), got.to_json()))
            }
            None => errs.push(format!("{}：描けない {:?}", get("name"), render(&data, scale, region).err())),
        }
        n += 1;
    }
    assert!(
        n >= 100,
        "render-v{current}.jsonl の行が少ない（{n}）。`cargo test -p sumie --test golden -- --ignored regenerate_renders` で作る"
    );
    assert!(
        errs.is_empty(),
        "D1 がゴールデンと違う（{} 件）：\n{}",
        errs.len(),
        errs.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
}

/// 描く行の例（名前・道・倍率・範囲）。
type RenderCase = (String, String, f64, Option<(u32, u32, u32, u32)>);

/// 描く行の例：`tests/svg/` の描けるものすべて（倍率 1）と、`corpus/` のうち BLAKE3 の頭の 16 進が 0 で描けて 512 × 512 以内の
/// もの（倍率 1）。`tests/svg/m2-probe/` は倍率 0.5・2.5 と、範囲（全体の右下の 4 分の 1・1 画素の列）も。名前は道と設定。
fn render_cases(index: &BTreeMap<String, String>) -> Vec<RenderCase> {
    let mut v = Vec::new();
    let mut paths: Vec<(&String, &String)> = index.iter().map(|(h, p)| (p, h)).collect();
    paths.sort();
    for (path, hash) in paths {
        let in_tests = path.starts_with("tests/svg/");
        if !in_tests && !hash.starts_with('0') {
            continue;
        }
        v.push((path.clone(), path.clone(), 1.0, None));
        if path.starts_with("tests/svg/m2-probe/") {
            v.push((format!("{path}@0.5"), path.clone(), 0.5, None));
            v.push((format!("{path}@2.5"), path.clone(), 2.5, None));
            v.push((format!("{path}#quarter"), path.clone(), 1.0, Some((50, 50, 50, 50))));
            v.push((format!("{path}#column"), path.clone(), 1.0, Some((37, 0, 1, 100))));
        }
    }
    v
}

/// render-v0.jsonl を作り直す（版 0 は M7 まで作り直してよい。ADR-20。`cargo test -p sumie --test golden -- --ignored
/// regenerate_renders`）。
#[test]
#[ignore]
fn regenerate_renders() {
    let root = workspace();
    assert_eq!(current_version(), 0, "凍結した版は作り直さない（追記だけ）");
    let index = input_index(&root);
    let mut out = String::new();
    for (name, path, scale, region) in render_cases(&index) {
        let data = std::fs::read(root.join(&path)).unwrap();
        if !path.starts_with("tests/svg/") {
            // 大きな集まりの例は 512 × 512 以内だけ（debug の試験を速く保つ）
            match render(&data, 1.0, Some((0, 0, 1, 1))) {
                Ok(_) => {}
                Err(_) => continue,
            }
            let doc = sumie::parse(&data, &sumie::ParseOptions::DEFAULT, &sumie::Never).unwrap();
            let tree = doc.build(&sumie::Never).unwrap();
            let px = tree.pixel_size(1.0).unwrap();
            if px.width > 512 || px.height > 512 {
                continue;
            }
        }
        if let Some(line) = render_line(&name, &path, &data, scale, region) {
            out.push_str(&line.to_json());
            out.push('\n');
        }
    }
    std::fs::write(goldens().join("render-v0.jsonl"), out).unwrap();
}
