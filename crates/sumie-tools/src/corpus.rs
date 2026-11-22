//! 照合の集まり（ADR-21・architecture.md §10 の `corpus`）。
//!
//! - `corpus import-resvg --src <resvg のソースの根> --rev <コミット> --sha256 <アーカイブの SHA-256>`：resvg の試験の SVG
//!   （`crates/resvg/tests/{tests,extra,resources}` の .svg）だけを `corpus/resvg/` に写し、ライセンスの写し・SOURCE.md・
//!   MANIFEST.jsonl（道・バイト数・BLAKE3）を書く。resvg の描いた PNG・画像・フォントは写さない
//! - `corpus fetch-w3c [--dest local/w3c-svg11-20110816]`：W3C SVG 1.1 第 2 版の試験のアーカイブを取り（SHA-256 を照らす）、
//!   `svg/*.svg` だけを git の外（`local/`）に置く（ライセンスの書き方が食い違うため。ADR-21 の承認・corpus/README.md）
//! - `corpus index [--root corpus] [--out <file>]`：根の下の SVG の一覧（道・出どころ・分類・使う機能・範囲）を JSONL に書く

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use sumie_testkit::jsonl::Line;

/// W3C SVG 1.1 第 2 版の試験のアーカイブ（2011-08-16 版）と、その SHA-256（M0 で取って測った値）。
pub const W3C_URL: &str = "https://www.w3.org/Graphics/SVG/Test/20110816/archives/W3C_SVG_11_TestSuite.tar.gz";
pub const W3C_SHA256: &str = "b5f46cca1ad79b670f9179770b2366c57efd5c671d084144090feab4b7ff1030";
pub const W3C_BYTES: u64 = 14_651_624;

pub fn run(args: &[String]) -> Result<(), String> {
    let sub = args.first().map(String::as_str).unwrap_or("");
    let rest = args.get(1..).unwrap_or(&[]);
    match sub {
        "import-resvg" => import_resvg(rest),
        "fetch-w3c" => fetch_w3c(rest),
        "index" => index(rest),
        _ => Err("corpus import-resvg | fetch-w3c | index".into()),
    }
}

pub fn arg<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap_or_else(|_| PathBuf::from("."))
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

fn import_resvg(args: &[String]) -> Result<(), String> {
    let src = PathBuf::from(arg(args, "--src").ok_or("--src <resvg のソースの根>")?);
    let rev = arg(args, "--rev").ok_or("--rev <コミット>")?;
    let sha = arg(args, "--sha256").ok_or("--sha256 <アーカイブの SHA-256>")?;
    let tests = src.join("crates/resvg/tests");
    if !tests.is_dir() {
        return Err(format!("{} がない", tests.display()));
    }
    let root = workspace_root();
    let dest = root.join("corpus/resvg");
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    let mut files = Vec::new();
    for sub in ["tests", "extra", "resources"] {
        walk_svgs(&tests.join(sub), &mut files);
    }
    files.sort();
    let mut manifest = String::new();
    for f in &files {
        let rel = f.strip_prefix(&tests).map_err(|e| e.to_string())?;
        let to = dest.join(rel);
        std::fs::create_dir_all(to.parent().ok_or("親がない")?).map_err(|e| e.to_string())?;
        let bytes = std::fs::read(f).map_err(|e| e.to_string())?;
        std::fs::write(&to, &bytes).map_err(|e| e.to_string())?;
        let line = Line::new()
            .str("path", rel.to_string_lossy().replace('\\', "/"))
            .int("bytes", bytes.len() as u64)
            .str("blake3", blake3::hash(&bytes).to_hex().to_string());
        manifest.push_str(&line.to_json());
        manifest.push('\n');
    }
    for l in ["LICENSE-MIT", "LICENSE-APACHE"] {
        std::fs::copy(src.join(l), dest.join(l)).map_err(|e| format!("{l}：{e}"))?;
    }
    std::fs::write(dest.join("MANIFEST.jsonl"), manifest).map_err(|e| e.to_string())?;
    std::fs::write(
        dest.join("SOURCE.md"),
        format!(
            "# 出どころ：resvg の試験\n\n\
             | 項目 | 値 |\n|---|---|\n\
             | リポジトリ | https://github.com/linebender/resvg |\n\
             | 版 | タグ `v0.48.1`（コミット `{rev}`。Kura が使う resvg と同じ版） |\n\
             | 取ったもの | `https://codeload.github.com/linebender/resvg/tar.gz/{rev}`（SHA-256 `{sha}`） |\n\
             | 写したもの | `crates/resvg/tests/` の `tests/`・`extra/`・`resources/` の `.svg`（{} 件。道は `crates/resvg/tests/` からの相対のまま） |\n\
             | 写さなかったもの | resvg の描いた PNG（照合の相手の絵はゴールデンにしない）・画像・フォント・試験のコード |\n\
             | ライセンス | MIT OR Apache-2.0（リポジトリの README の License の節。`LICENSE-MIT`・`LICENSE-APACHE` はリポジトリの根の写し。試験の README にほかの定めはない） |\n\
             | 一覧 | `MANIFEST.jsonl`（道・バイト数・BLAKE3） |\n\n\
             作り直し：`cargo run -p sumie-tools -- corpus import-resvg --src <取り出した根> --rev {rev} --sha256 {sha}`\n",
            files.len()
        ),
    )
    .map_err(|e| e.to_string())?;
    println!("corpus/resvg に {} 件を写した", files.len());
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String, String> {
    for (cmd, args) in [("shasum", vec!["-a", "256"]), ("sha256sum", vec![])] {
        if let Ok(o) = Command::new(cmd).args(&args).arg(path).output()
            && o.status.success()
        {
            return Ok(String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap_or("").to_string());
        }
    }
    Err("shasum も sha256sum もない".into())
}

fn fetch_w3c(args: &[String]) -> Result<(), String> {
    let root = workspace_root();
    let dest = arg(args, "--dest").map(PathBuf::from).unwrap_or_else(|| root.join("local/w3c-svg11-20110816"));
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    let archive = dest.join("W3C_SVG_11_TestSuite.tar.gz");
    if !archive.exists() {
        let st = Command::new("curl")
            .args(["-sSfL", "-o"])
            .arg(&archive)
            .arg(W3C_URL)
            .status()
            .map_err(|e| format!("curl を起動できない：{e}"))?;
        if !st.success() {
            return Err("取れなかった".into());
        }
    }
    let got = sha256_of(&archive)?;
    if got != W3C_SHA256 {
        return Err(format!("SHA-256 が違う：{got}（期待 {W3C_SHA256}）。{} を消して取り直す", archive.display()));
    }
    let raw = dest.join("raw");
    std::fs::create_dir_all(&raw).map_err(|e| e.to_string())?;
    let st = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&raw)
        .arg("svg")
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("tar で取り出せない".into());
    }
    let svg = dest.join("svg");
    std::fs::create_dir_all(&svg).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    walk_svgs(&raw.join("svg"), &mut files);
    files.sort();
    for f in &files {
        let name = f.file_name().ok_or("名前がない")?;
        std::fs::copy(f, svg.join(name)).map_err(|e| e.to_string())?;
    }
    std::fs::remove_dir_all(&raw).map_err(|e| e.to_string())?;
    std::fs::write(
        dest.join("SOURCE.md"),
        format!(
            "# W3C SVG 1.1 第 2 版の試験（git の外）\n\n- 取ったもの：{W3C_URL}（{W3C_BYTES} バイト、SHA-256 `{W3C_SHA256}`）\n\
             - 置いたもの：`svg/*.svg`（{} 件。中身は変えない）\n\
             - ライセンス：試験の総覧のページは W3C Document License と W3C Member agreement、W3C の試験のライセンスの方針の\n  \
             ページは W3C test suite license と 3-clause BSD を示し、食い違う。各ファイルの頭は W3C の著作権の表示。そのため\n  \
             リポジトリには入れず、照合の結果の一覧だけを残す（ADR-21 の承認、2026-11-22）\n",
            files.len()
        ),
    )
    .map_err(|e| e.to_string())?;
    println!("{} に {} 件を置いた", svg.display(), files.len());
    Ok(())
}

/// SVG の中の要素の名前（接頭辞を除いた局所名）。
fn element_names(svg: &[u8]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i + 1 < svg.len() {
        if svg[i] == b'<' && svg[i + 1].is_ascii_alphabetic() {
            let mut j = i + 1;
            while j < svg.len()
                && (svg[j].is_ascii_alphanumeric() || svg[j] == b':' || svg[j] == b'-' || svg[j] == b'_')
            {
                j += 1;
            }
            let name = String::from_utf8_lossy(&svg[i + 1..j]).to_string();
            let local = name.rsplit(':').next().unwrap_or("").to_string();
            out.insert(local);
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// 使う機能の印（分類の並べ・マイルストーンごとの範囲の選び方に使う）。
pub fn features(svg: &[u8]) -> Vec<&'static str> {
    let names = element_names(svg);
    let has = |n: &str| names.contains(n);
    let any = |f: &dyn Fn(&str) -> bool| names.iter().any(|n| f(n));
    let text = std::str::from_utf8(svg).unwrap_or("");
    let mut f = Vec::new();
    if has("text") || has("tspan") || has("textPath") || has("tref") {
        f.push("text");
    }
    if has("filter") || any(&|n| n.starts_with("fe")) || text.contains("filter:") {
        f.push("filter");
    }
    if has("linearGradient") || has("radialGradient") {
        f.push("gradient");
    }
    for (n, tag) in [
        ("pattern", "pattern"),
        ("mask", "mask"),
        ("clipPath", "clip"),
        ("marker", "marker"),
        ("image", "image"),
        ("use", "use"),
        ("switch", "switch"),
        ("style", "style"),
        ("symbol", "symbol"),
        ("foreignObject", "foreignObject"),
        ("script", "script"),
    ] {
        if has(n) {
            f.push(tag);
        }
    }
    if any(&|n| n.starts_with("animate") || n == "set") {
        f.push("animation");
    }
    if text.contains("mix-blend-mode") || text.contains("isolation") {
        f.push("blend");
    }
    f
}

fn index(args: &[String]) -> Result<(), String> {
    let ws = workspace_root();
    let root = arg(args, "--root").map(PathBuf::from).unwrap_or_else(|| ws.join("corpus"));
    let out = arg(args, "--out").map(PathBuf::from).unwrap_or_else(|| root.join("index.jsonl"));
    let mut files = Vec::new();
    walk_svgs(&root, &mut files);
    files.sort();
    let mut text = String::new();
    for f in &files {
        let rel = f.strip_prefix(&root).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(f).map_err(|e| e.to_string())?;
        let (source, category, scope) = classify(&rel);
        let line = Line::new()
            .str("path", rel.as_str())
            .str("source", source)
            .str("category", category)
            .str("scope", scope)
            .int("bytes", bytes.len() as u64)
            .str("blake3", blake3::hash(&bytes).to_hex().to_string())
            .str("features", features(&bytes).join(","));
        text.push_str(&line.to_json());
        text.push('\n');
    }
    std::fs::write(&out, text).map_err(|e| e.to_string())?;
    println!("{} に {} 件", out.display(), files.len());
    Ok(())
}

/// 道から出どころ・分類・範囲を決める。範囲は `in` か `out:<理由>`（アニメーション・スクリプト・操作は v1 の外）。
fn classify(rel: &str) -> (String, String, String) {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.first() == Some(&"resvg") {
        let cat = match parts.get(1) {
            Some(&"tests") => parts.get(2..parts.len().saturating_sub(1)).map(|p| p.join("/")).unwrap_or_default(),
            Some(x) => (*x).to_string(),
            None => String::new(),
        };
        return ("resvg".into(), cat, "in".into());
    }
    let name = parts.last().copied().unwrap_or("");
    if parts.contains(&"svg") || name.contains("-t.svg") || name.contains("-f.svg") || name.contains("-b.svg") {
        let cat = name.split('-').next().unwrap_or("").to_string();
        let scope = match cat.as_str() {
            "animate" => "out:animation",
            "interact" => "out:interaction",
            "script" | "svgdom" => "out:script",
            _ => "in",
        };
        return ("w3c-svg11".into(), cat, scope.into());
    }
    ("local".into(), parts.first().copied().unwrap_or("").to_string(), "in".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_and_classify() {
        let f = features(
            b"<svg><defs><linearGradient/><filter><feGaussianBlur/></filter></defs><text>a</text><svg:use/></svg>",
        );
        assert_eq!(f, ["text", "filter", "gradient", "use"]);
        assert_eq!(classify("resvg/tests/painting/stroke-linejoin/miter.svg").1, "painting/stroke-linejoin");
        assert_eq!(
            classify("svg/animate-elem-02-t.svg"),
            ("w3c-svg11".into(), "animate".into(), "out:animation".into())
        );
        assert_eq!(classify("svg/shapes-rect-01-t.svg").2, "in");
    }
}
