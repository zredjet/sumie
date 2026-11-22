//! 照合の集まり（ADR-21・architecture.md §10 の `corpus`）。
//!
//! - `corpus import-resvg [--archive <取ったアーカイブ>]`：決めた rev（[`RESVG_REV`]）の resvg のソースのアーカイブを取り
//!   （`--archive` がなければ curl で `local/` に）、SHA-256（[`RESVG_SHA256`]）を照らして新しい空のフォルダに取り出し、試験の
//!   SVG（`crates/resvg/tests/{tests,extra,resources}` の .svg）だけを `corpus/resvg/` に写して、ライセンスの写し・SOURCE.md・
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

/// 取り込む resvg の版（タグ `v0.48.1` のコミット。Kura が使う resvg と同じ版）と、そのアーカイブの SHA-256（M0 で取って測った値）。
pub const RESVG_REV: &str = "68b14c4c3bccdb60344c777406486b54c36ec1a4";
pub const RESVG_SHA256: &str = "41bf46fdb8abc6fc49ec6f83facff0cdab2759a7212734c92f3f6aec12c51820";

pub fn run(args: &[String]) -> Result<(), String> {
    let sub = args.first().map(String::as_str).unwrap_or("");
    let rest = args.get(1..).unwrap_or(&[]);
    match sub {
        "import-resvg" => {
            check_args(rest, &["--archive"], &[], 0)?;
            import_resvg(rest)
        }
        "resvg-source" => {
            check_args(rest, &[], &[], 0)?;
            write_resvg_source()
        }
        "fetch-w3c" => {
            check_args(rest, &["--dest"], &[], 0)?;
            fetch_w3c(rest)
        }
        "fetch-real" => {
            check_args(rest, &["--only"], &[], 0)?;
            crate::corpus_real::run(rest)
        }
        "index" => {
            check_args(rest, &["--root", "--out"], &[], 0)?;
            index(rest)
        }
        _ => Err("corpus import-resvg | resvg-source | fetch-w3c | fetch-real | index".into()),
    }
}

pub fn arg<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// 引数が知っている名前だけか。`with_value` は値を取る名前（`--名前 値`）、`flags` は値を取らない名前、`positional` は
/// 位置の引数の数の上限。打ち間違い（`--filtr`）を黙って無視して全体を回さないように。
pub fn check_args(args: &[String], with_value: &[&str], flags: &[&str], positional: usize) -> Result<(), String> {
    let mut i = 0;
    let mut pos = 0;
    while let Some(a) = args.get(i) {
        if with_value.contains(&a.as_str()) {
            if args.get(i + 1).is_none() {
                return Err(format!("{a} の値がない"));
            }
            i += 2;
        } else if flags.contains(&a.as_str()) {
            i += 1;
        } else if a.starts_with("--") {
            return Err(format!("知らない引数：{a}（使える引数：{}）", [with_value, flags].concat().join(" ")));
        } else {
            pos += 1;
            if pos > positional {
                return Err(format!("余分な引数：{a}"));
            }
            i += 1;
        }
    }
    Ok(())
}

/// 数の引数（なければ `default`。読めなければ誤り。`600s` を黙って既定の値にしない）。
pub fn num_arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> Result<T, String> {
    match arg(args, name) {
        None => Ok(default),
        Some(v) => v.parse::<T>().map_err(|_| format!("{name} の値 {v} を数として読めない")),
    }
}

/// 計った機械の CPU の名前（記録に書く。Chrome の CPU のラスタは命令の種類で縁の値が変わりうる）。
pub fn cpu_name() -> String {
    if let Ok(o) = Command::new("sysctl").args(["-n", "machdep.cpu.brand_string"]).output()
        && o.status.success()
    {
        return String::from_utf8_lossy(&o.stdout).trim().to_string();
    }
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| std::env::consts::ARCH.to_string())
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
    let (rev, sha) = (RESVG_REV, RESVG_SHA256);
    let root = workspace_root();
    let archive = match arg(args, "--archive") {
        Some(a) => PathBuf::from(a),
        None => {
            let a = root.join(format!("local/resvg-{rev}.tar.gz"));
            if !a.exists() {
                std::fs::create_dir_all(root.join("local")).map_err(|e| e.to_string())?;
                let url = format!("https://codeload.github.com/linebender/resvg/tar.gz/{rev}");
                let st = Command::new("curl")
                    .args(["-sSfL", "-o"])
                    .arg(&a)
                    .arg(&url)
                    .status()
                    .map_err(|e| format!("curl を起動できない：{e}"))?;
                if !st.success() {
                    return Err(format!("取れなかった：{url}"));
                }
            }
            a
        }
    };
    // アーカイブの SHA-256 が違うとき（GitHub はその場で作るアーカイブのバイト列を約束しない）は、コミットした
    // MANIFEST.jsonl の BLAKE3 で写すファイルを 1 件ずつ照らす（どちらも合わなければ誤り）
    let got = sha256_of(&archive)?;
    let expected = if got == sha {
        None
    } else {
        let m = std::fs::read_to_string(root.join("corpus/resvg/MANIFEST.jsonl")).map_err(|_| {
            format!(
                "SHA-256 が違い（{got}、期待 {sha}）、照らす MANIFEST.jsonl もない。{} を消して取り直す",
                archive.display()
            )
        })?;
        let mut map = BTreeSet::new();
        for l in m.lines() {
            let line = Line::parse(l)?;
            map.insert((
                line.get_str("path").unwrap_or("").to_string(),
                line.get_str("blake3").unwrap_or("").to_string(),
            ));
        }
        eprintln!("アーカイブの SHA-256 が違う（{got}）。MANIFEST.jsonl の BLAKE3 で 1 件ずつ照らす");
        Some(map)
    };
    // 取り出すのは新しい空のフォルダ（取ったものは信用しない）
    let unpack = root.join(format!("local/resvg-src-{}", std::process::id()));
    if unpack.exists() {
        return Err(format!("{} がもうある", unpack.display()));
    }
    std::fs::create_dir_all(&unpack).map_err(|e| e.to_string())?;
    let st =
        Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&unpack).status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("tar で取り出せない".into());
    }
    let src = unpack.join(format!("resvg-{rev}"));
    let result = import_resvg_from(&src, expected.as_ref());
    let _ = std::fs::remove_dir_all(&unpack);
    result
}

fn import_resvg_from(src: &Path, expected: Option<&BTreeSet<(String, String)>>) -> Result<(), String> {
    let tests = src.join("crates/resvg/tests");
    if !tests.is_dir() {
        return Err(format!("{} がない", tests.display()));
    }
    let mut files = Vec::new();
    for sub in ["tests", "extra", "resources"] {
        walk_svgs(&tests.join(sub), &mut files);
    }
    files.sort();
    if let Some(exp) = expected {
        let mut got = BTreeSet::new();
        for f in &files {
            let rel = f.strip_prefix(&tests).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
            let bytes = std::fs::read(f).map_err(|e| e.to_string())?;
            got.insert((rel, blake3::hash(&bytes).to_hex().to_string()));
        }
        if &got != exp {
            return Err("取り出したファイルが MANIFEST.jsonl の道・BLAKE3 と合わない".into());
        }
    }
    let dest = workspace_root().join("corpus/resvg");
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
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
    std::fs::write(dest.join("SOURCE.md"), resvg_source_md(files.len())).map_err(|e| e.to_string())?;
    println!("corpus/resvg に {} 件を写した", files.len());
    Ok(())
}

/// `corpus/resvg/SOURCE.md` の中身（取り込みと `corpus resvg-source` の両方が書く。試験が今のファイルと照らす）。
pub fn resvg_source_md(n: usize) -> String {
    let (rev, sha) = (RESVG_REV, RESVG_SHA256);
    format!(
        "# 出どころ：resvg の試験\n\n\
         | 項目 | 値 |\n|---|---|\n\
         | リポジトリ | https://github.com/linebender/resvg |\n\
         | 版 | タグ `v0.48.1`（コミット `{rev}`。Kura が使う resvg と同じ版） |\n\
         | 取ったもの | `https://codeload.github.com/linebender/resvg/tar.gz/{rev}`（SHA-256 `{sha}`） |\n\
         | 写したもの | `crates/resvg/tests/` の `tests/`・`extra/`・`resources/` の `.svg`（{n} 件。道は `crates/resvg/tests/` からの相対のまま） |\n\
         | 写さなかったもの | resvg の描いた PNG（照合の相手の絵はゴールデンにしない）・画像・フォント・試験のコード |\n\
         | ライセンス | MIT OR Apache-2.0（リポジトリの README の License の節。`LICENSE-MIT`・`LICENSE-APACHE` はリポジトリの根の写し。試験の README にほかの定めはない） |\n\
         | 一覧 | `MANIFEST.jsonl`（道・バイト数・BLAKE3） |\n\n\
         作り直し：`cargo run -p sumie-tools -- corpus import-resvg [--archive <取ったアーカイブ>]`（rev と SHA-256 は道具の定数。\
         アーカイブの SHA-256 が違うときは、この MANIFEST.jsonl の BLAKE3 で 1 件ずつ照らす）\n"
    )
}

/// `corpus resvg-source`：取り直さずに SOURCE.md だけを今の雛形で書き直す（件数は MANIFEST.jsonl の行の数）。
fn write_resvg_source() -> Result<(), String> {
    let dir = workspace_root().join("corpus/resvg");
    let n = std::fs::read_to_string(dir.join("MANIFEST.jsonl")).map_err(|e| e.to_string())?.lines().count();
    std::fs::write(dir.join("SOURCE.md"), resvg_source_md(n)).map_err(|e| e.to_string())?;
    println!("corpus/resvg/SOURCE.md を書いた（{n} 件）");
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

/// 属性 `name` の値（`name="…"`・`name='…'`。前が空白のもの）を並べる。
fn attr_values<'a>(text: &'a str, name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    for q in ['"', '\''] {
        let pat = format!("{name}={q}");
        let mut from = 0;
        while let Some(i) = text[from..].find(&pat) {
            let at = from + i;
            from = at + pat.len();
            let before = text[..at].chars().next_back();
            if !before.is_some_and(|c| c.is_ascii_whitespace()) {
                continue;
            }
            if let Some(end) = text[from..].find(q) {
                out.push(&text[from..from + end]);
            }
        }
    }
    out
}

/// 使う機能の印（分類の並べ・マイルストーンごとの範囲の選び方・ADR-15 の群分けに使う）。
/// - `filter`：`<filter>`・`fe*` の要素・`filter:` の書き方・`none` でない `filter` の属性（CSS のフィルタ関数 `blur(5)` も）
/// - `image`：`<image>` と、要素を指さない（`#` で始まらない）参照の `feImage`（Kura の設定の resvg は画像を読まない）
/// - `ub`：題（`<title>`）に「(UB)」とある、resvg の試験の未定義の振る舞いの例（ADR-56 (1)。分布から外す）
pub fn features(svg: &[u8]) -> Vec<&'static str> {
    let names = element_names(svg);
    let has = |n: &str| names.contains(n);
    let any = |f: &dyn Fn(&str) -> bool| names.iter().any(|n| f(n));
    let text = std::str::from_utf8(svg).unwrap_or("");
    let mut f = Vec::new();
    if has("text") || has("tspan") || has("textPath") || has("tref") {
        f.push("text");
    }
    let filter_attr = attr_values(text, "filter").iter().any(|v| v.trim() != "none");
    if has("filter") || any(&|n| n.starts_with("fe")) || text.contains("filter:") || filter_attr {
        f.push("filter");
    }
    if has("linearGradient") || has("radialGradient") {
        f.push("gradient");
    }
    let fe_image_external = has("feImage") && {
        // feImage の要素の中の href（xlink:href も）が # で始まらない
        text.split("<feImage").skip(1).any(|rest| {
            let tag = rest.split('>').next().unwrap_or("");
            attr_values(&format!(" {tag}"), "href").iter().any(|v| !v.starts_with('#'))
                || attr_values(&format!(" {tag}"), "xlink:href").iter().any(|v| !v.starts_with('#'))
        })
    };
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
    if fe_image_external && !f.contains(&"image") {
        f.push("image");
    }
    if any(&|n| n.starts_with("animate") || n == "set") {
        f.push("animation");
    }
    if text.contains("mix-blend-mode") || text.contains("isolation") {
        f.push("blend");
    }
    let ub = text.split("<title>").nth(1).and_then(|t| t.split("</title>").next()).is_some_and(|t| t.contains("(UB)"));
    if ub {
        f.push("ub");
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
    // W3C の試験かは、fetch-w3c が根に書く SOURCE.md の印で決める（道の名前の一部で決めない）
    let w3c = std::fs::read_to_string(root.join("SOURCE.md")).is_ok_and(|t| t.starts_with("# W3C SVG 1.1"));
    let mut text = String::new();
    for f in &files {
        let rel = f.strip_prefix(&root).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(f).map_err(|e| e.to_string())?;
        let (source, category, scope) = classify(&rel, w3c);
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
/// `w3c` は根が W3C の試験（fetch-w3c の置き場）か。
fn classify(rel: &str, w3c: bool) -> (String, String, String) {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.first() == Some(&"resvg") {
        let cat = match parts.get(1) {
            Some(&"tests") => parts.get(2..parts.len().saturating_sub(1)).map(|p| p.join("/")).unwrap_or_default(),
            Some(x) => (*x).to_string(),
            None => String::new(),
        };
        return ("resvg".into(), cat, "in".into());
    }
    if parts.first() == Some(&"real") {
        // 実際の SVG の集まり（corpus/real/<名前>/…。ADR-59）
        return ("real".into(), parts.get(1).copied().unwrap_or("").to_string(), "in".into());
    }
    let name = parts.last().copied().unwrap_or("");
    if w3c {
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
    fn resvg_source_md_is_current() {
        let dir = workspace_root().join("corpus/resvg");
        let n = std::fs::read_to_string(dir.join("MANIFEST.jsonl")).unwrap().lines().count();
        let now = std::fs::read_to_string(dir.join("SOURCE.md")).unwrap();
        assert_eq!(
            now,
            resvg_source_md(n),
            "corpus/resvg/SOURCE.md が今の雛形と違う（corpus resvg-source で書き直す）"
        );
    }

    #[test]
    fn args_are_checked() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(check_args(&a(&["--root", "x", "--all"]), &["--root"], &["--all"], 0).is_ok());
        assert!(check_args(&a(&["--filtr", "x"]), &["--filter"], &[], 0).is_err());
        assert!(check_args(&a(&["--root"]), &["--root"], &[], 0).is_err());
        assert!(check_args(&a(&["in.svg", "out.png", "x"]), &[], &[], 2).is_err());
        assert_eq!(num_arg::<u64>(&a(&["--timeout", "600"]), "--timeout", 60), Ok(600));
        assert!(num_arg::<u64>(&a(&["--timeout", "600s"]), "--timeout", 60).is_err());
        assert_eq!(num_arg::<u64>(&a(&[]), "--timeout", 60), Ok(60));
    }

    #[test]
    fn features_and_classify() {
        let f = features(
            b"<svg><defs><linearGradient/><filter><feGaussianBlur/></filter></defs><text>a</text><svg:use/></svg>",
        );
        assert_eq!(f, ["text", "filter", "gradient", "use"]);
        // CSS のフィルタ関数・none・外の画像の feImage・要素を指す feImage・未定義の振る舞いの題
        assert_eq!(features(b"<svg><rect filter=\"blur(5)\"/></svg>"), ["filter"]);
        assert_eq!(features(b"<svg><rect filter='url(#f)'/></svg>"), ["filter"]);
        assert!(features(b"<svg><rect filter=\"none\"/></svg>").is_empty());
        assert!(features(b"<svg><rect data-filter=\"blur(5)\"/></svg>").is_empty());
        let img = features(b"<svg><filter><feImage xlink:href=\"data:image/png;base64,AA\"/></filter></svg>");
        assert_eq!(img, ["filter", "image"]);
        let el = features(b"<svg><filter><feImage href=\"#r\"/></filter></svg>");
        assert_eq!(el, ["filter"]);
        assert_eq!(features(b"<svg><title>Recursive (UB)</title><rect/></svg>"), ["ub"]);
        assert_eq!(classify("resvg/tests/painting/stroke-linejoin/miter.svg", false).1, "painting/stroke-linejoin");
        assert_eq!(
            classify("svg/animate-elem-02-t.svg", true),
            ("w3c-svg11".into(), "animate".into(), "out:animation".into())
        );
        assert_eq!(classify("svg/shapes-rect-01-t.svg", true).2, "in");
        assert_eq!(
            classify("real/twemoji/assets/svg/1f600.svg", false),
            ("real".into(), "twemoji".into(), "in".into())
        );
        // W3C の根でなければ、名前が -t.svg で終わっても W3C にしない
        assert_eq!(classify("icons/outline/letter-t.svg", false).0, "local");
        assert_eq!(classify("svg/animate-x-t.svg", false).2, "in");
    }
}
