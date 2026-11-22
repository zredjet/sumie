//! `corpus fetch-real`：実際の SVG の集まり（ADR-59。Kura の手元の SVG の代わり）を取り込む。
//!
//! - git の集まり（[`GIT_SETS`]）：決めた rev の GitHub のリポジトリから、決めた道の SVG を種つきで抜き取り（ChaCha8）、
//!   `corpus/real/<名前>/` に写す。ファイルは raw.githubusercontent.com から取り、git の blob の SHA-1（tree の一覧の値）を
//!   `git hash-object` で照らしてから写す。ライセンスのファイルも同じく照らして写す。`MANIFEST.jsonl`（道・バイト数・
//!   BLAKE3・blob）と `SOURCE.md` を書く
//! - Wikimedia Commons（[`COMMONS_CATEGORY`]）：秀逸な画像（ベクター）のうち、パブリックドメイン・CC0・CC BY（BY-SA は
//!   外す）の SVG を `local/commons-featured-vector/` に取り（大きいので git の外）、Commons の SHA-1 を照らす。一覧（題・
//!   URL・ライセンス・作者・SHA-1・BLAKE3）は `corpus/real/commons-featured-vector/MANIFEST.jsonl` として git に入れる
//!
//! `corpus fetch-real [--only <名前>]`（`--only` がなければ git の集まりだけ。Commons は `--only commons-featured-vector` で
//! 指したときだけ取る）。取ったものは信用しない入力なので、新しい空のフォルダに取ってから照らして写す。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sumie_testkit::chacha::ChaCha;
use sumie_testkit::jsonl::Line;

use crate::corpus::{arg, workspace_root};

/// 取りに行くときの名乗り（利用者の情報を入れない）。
const USER_AGENT: &str = "sumie-corpus-fetch/0.1";

/// git の集まり。
pub struct GitSet {
    /// `corpus/real/<name>/`
    pub name: &'static str,
    pub repo: &'static str,
    /// 固定する rev（コミット）
    pub rev: &'static str,
    pub license: &'static str,
    /// 写すライセンスのファイル（リポジトリの根）
    pub license_files: &'static [&'static str],
    /// 取る道の頭と、道に含むもの（空なら問わない）。`.svg` で終わるものだけ
    pub prefix: &'static str,
    pub contains: &'static str,
    /// 抜き取る数（0 は全部）と種
    pub take: usize,
    pub seed: u64,
    pub traits: &'static str,
    /// 帰属（CC BY のもの。空なら行を書かない）
    pub attribution: &'static str,
}

pub const GIT_SETS: &[GitSet] = &[
    GitSet {
        name: "fluent-emoji-color",
        repo: "microsoft/fluentui-emoji",
        rev: "1ffb34c752ecf5d402f04cfb4b392c77f57c54bc",
        license: "MIT",
        license_files: &["LICENSE"],
        prefix: "assets/",
        contains: "/Color/",
        take: 120,
        seed: 1,
        traits: "放射・線形のグラデーションの多い絵文字",
        attribution: "",
    },
    GitSet {
        name: "fluent-emoji-flat",
        repo: "microsoft/fluentui-emoji",
        rev: "1ffb34c752ecf5d402f04cfb4b392c77f57c54bc",
        license: "MIT",
        license_files: &["LICENSE"],
        prefix: "assets/",
        contains: "/Flat/",
        take: 150,
        seed: 2,
        traits: "平坦な塗りの絵文字",
        attribution: "",
    },
    GitSet {
        name: "twemoji",
        repo: "jdecked/twemoji",
        rev: "b6b55fef1e8636b540a6d016a4729ca8cdf2e60b",
        license: "絵は CC BY 4.0（LICENSE-GRAPHICS）・コードは MIT（LICENSE）",
        license_files: &["LICENSE", "LICENSE-GRAPHICS"],
        prefix: "assets/svg/",
        contains: "",
        take: 300,
        seed: 3,
        traits: "svgo で詰めた平坦な path の絵文字",
        attribution: "Twemoji（2014〜2021 年は Twitter, Inc、2022 年からは Jason Sofonia と Justine De Caires（jdecked/twemoji）と、ほかの貢献者による）。CC BY 4.0（https://creativecommons.org/licenses/by/4.0/）で使い、取ったバイト列のまま変えていない",
    },
    GitSet {
        name: "flag-icons",
        repo: "lipis/flag-icons",
        rev: "ac7e0eb3259eb62aa6100c435607e2fe03896a97",
        license: "MIT",
        license_files: &["LICENSE"],
        prefix: "flags/4x3/",
        contains: "",
        take: 0,
        seed: 4,
        traits: "国旗（紋章の細かい path・グラデーション）",
        attribution: "",
    },
    GitSet {
        name: "bootstrap-icons",
        repo: "twbs/icons",
        rev: "829d2bc2b87543c7d781c815ca4f0feee8cf9e45",
        license: "MIT",
        license_files: &["LICENSE"],
        prefix: "icons/",
        contains: "",
        take: 200,
        seed: 5,
        traits: "塗りのアイコン（偶奇・円弧）",
        attribution: "",
    },
    GitSet {
        name: "tabler-icons",
        repo: "tabler/tabler-icons",
        rev: "85e870dbe8680c2d6deefd062122d30ef1544a41",
        license: "MIT",
        license_files: &["LICENSE"],
        prefix: "icons/outline/",
        contains: "",
        take: 200,
        seed: 6,
        traits: "線のアイコン（丸い端と継ぎ目）",
        attribution: "",
    },
];

/// Wikimedia Commons の分類（秀逸な画像・ベクター）。
pub const COMMONS_CATEGORY: &str = "Category:Featured pictures on Wikimedia Commons - vector";
const COMMONS_NAME: &str = "commons-featured-vector";
/// 1 件の大きさの上限（Kura の入力の上限）。
const MAX_BYTES: u64 = 16 << 20;

pub fn run(args: &[String]) -> Result<(), String> {
    let only = arg(args, "--only");
    let root = workspace_root();
    for set in GIT_SETS.iter().filter(|s| only.is_none_or(|o| o == s.name)) {
        fetch_git_set(&root, set)?;
    }
    // Commons は名前で指したときだけ取る（Wikimedia は自動の取り出しを 429 で断るので、間を空けて頼むときだけ。ADR-59 (3)）
    if only == Some(COMMONS_NAME) {
        fetch_commons(&root)?;
    } else if only.is_some_and(|o| !GIT_SETS.iter().any(|s| s.name == o)) {
        return Err(format!("知らない集まり：{}", only.unwrap_or("")));
    }
    Ok(())
}

// ---- git の集まり ----

/// tree の一覧の 1 つ（道・blob・バイト数）。
struct Blob {
    path: String,
    sha: String,
    size: u64,
}

fn curl_text(url: &str) -> Result<String, String> {
    let o = Command::new("curl")
        .args(["-sSfL", "-A", USER_AGENT])
        .arg(url)
        .output()
        .map_err(|e| format!("curl を起動できない：{e}"))?;
    if !o.status.success() {
        return Err(format!("取れなかった：{url}：{}", String::from_utf8_lossy(&o.stderr)));
    }
    String::from_utf8(o.stdout).map_err(|_| format!("{url} の返事が UTF-8 でない"))
}

fn tree(repo: &str, rev: &str) -> Result<Vec<Blob>, String> {
    let text = curl_text(&format!("https://api.github.com/repos/{repo}/git/trees/{rev}?recursive=1"))?;
    let j = parse_json(&text)?;
    if j.get("truncated").and_then(J::as_bool) != Some(false) {
        return Err(format!("{repo} の tree の一覧が切れている（truncated）"));
    }
    let mut out = Vec::new();
    for e in j.get("tree").and_then(J::as_arr).ok_or("tree がない")? {
        if e.get("type").and_then(J::as_str) != Some("blob") {
            continue;
        }
        let path = e.get("path").and_then(J::as_str).ok_or("path がない")?;
        let sha = e.get("sha").and_then(J::as_str).ok_or("sha がない")?;
        let size = e.get("size").and_then(J::as_u64).ok_or("size がない")?;
        out.push(Blob { path: path.to_string(), sha: sha.to_string(), size });
    }
    Ok(out)
}

/// 道の並び（バイトの順）から、種つきで `take` 個を抜き取り、道の順に並べ直す（0 か多すぎるなら全部）。
pub fn sample(mut paths: Vec<String>, take: usize, seed: u64) -> Vec<String> {
    paths.sort();
    if take == 0 || take >= paths.len() {
        return paths;
    }
    let mut r = ChaCha::from_u64(seed);
    let n = paths.len();
    for i in 0..take {
        let j = i + r.below((n - i) as u64) as usize;
        paths.swap(i, j);
    }
    paths.truncate(take);
    paths.sort();
    paths
}

/// URL の道の部分（`/` は残し、ほかの予約の文字と空白・ASCII でない文字を % にする）。
fn encode_path(p: &str) -> String {
    let mut s = String::new();
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            let _ = write!(s, "%{b:02X}");
        }
    }
    s
}

/// 取り方（GitHub の raw は並べて、Wikimedia は 1 件ずつ間を空けて。Wikimedia は並べると 429 で断る）。
#[derive(Clone, Copy)]
enum Pace {
    Parallel,
    Gentle,
}

/// (URL, 置き場) の並びを curl でまとめて取る。
fn download(items: &[(String, PathBuf)], work: &Path, pace: Pace) -> Result<(), String> {
    let mut cfg = String::new();
    for (url, to) in items {
        let to = to.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        let _ = writeln!(cfg, "url = \"{url}\"\noutput = \"{to}\"");
    }
    let cfg_path = work.join("curl.cfg");
    std::fs::write(&cfg_path, cfg).map_err(|e| e.to_string())?;
    let pace_args: &[&str] = match pace {
        Pace::Parallel => &["--parallel", "--parallel-max", "8"],
        // 1 分に 20 件まで・429 は待ってやり直す
        Pace::Gentle => &["--rate", "20/m", "--retry", "5", "--retry-delay", "10"],
    };
    let st = Command::new("curl")
        .args(["-sSfL", "--create-dirs", "-A", USER_AGENT])
        .args(pace_args)
        .arg("-K")
        .arg(&cfg_path)
        .status()
        .map_err(|e| format!("curl を起動できない：{e}"))?;
    let _ = std::fs::remove_file(&cfg_path);
    if !st.success() {
        return Err("取れないファイルがあった（curl）".into());
    }
    Ok(())
}

/// ファイルの git の blob の SHA-1（`git hash-object`）。
fn git_blob_shas(files: &[PathBuf]) -> Result<Vec<String>, String> {
    let mut child = Command::new("git")
        .args(["hash-object", "--no-filters", "--stdin-paths"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git を起動できない：{e}"))?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().ok_or("git の標準入力がない")?;
        for f in files {
            writeln!(stdin, "{}", f.display()).map_err(|e| e.to_string())?;
        }
    }
    let o = child.wait_with_output().map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err("git hash-object が失敗した".into());
    }
    Ok(String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect())
}

fn fetch_git_set(root: &Path, set: &GitSet) -> Result<(), String> {
    let blobs = tree(set.repo, set.rev)?;
    let wanted: Vec<String> = blobs
        .iter()
        .filter(|b| b.path.starts_with(set.prefix) && b.path.contains(set.contains) && b.path.ends_with(".svg"))
        .filter(|b| b.size <= MAX_BYTES)
        .map(|b| b.path.clone())
        .collect();
    let candidates = wanted.len();
    let chosen = sample(wanted, set.take, set.seed);
    let mut files: Vec<String> = chosen.clone();
    files.extend(set.license_files.iter().map(|s| s.to_string()));
    // 新しい空のフォルダに取る
    let work = root.join(format!("local/real-fetch-{}-{}", set.name, std::process::id()));
    if work.exists() {
        return Err(format!("{} がもうある", work.display()));
    }
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        let items: Vec<(String, PathBuf)> = files
            .iter()
            .map(|p| {
                (
                    format!("https://raw.githubusercontent.com/{}/{}/{}", set.repo, set.rev, encode_path(p)),
                    work.join("files").join(p),
                )
            })
            .collect();
        download(&items, &work, Pace::Parallel)?;
        let paths: Vec<PathBuf> = items.iter().map(|(_, p)| p.clone()).collect();
        let shas = git_blob_shas(&paths)?;
        for (p, got) in files.iter().zip(&shas) {
            let want =
                blobs.iter().find(|b| &b.path == p).map(|b| b.sha.as_str()).ok_or(format!("{p} が一覧にない"))?;
            if got != want {
                return Err(format!("{p} の blob が違う：{got}（期待 {want}）"));
            }
        }
        // 照らせたので写す
        let dest = root.join("corpus/real").join(set.name);
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
        }
        let mut manifest = String::new();
        let mut total = 0u64;
        for p in &chosen {
            let bytes = std::fs::read(work.join("files").join(p)).map_err(|e| e.to_string())?;
            let to = dest.join(p);
            std::fs::create_dir_all(to.parent().ok_or("親がない")?).map_err(|e| e.to_string())?;
            std::fs::write(&to, &bytes).map_err(|e| e.to_string())?;
            total += bytes.len() as u64;
            let blob = blobs.iter().find(|b| &b.path == p).map(|b| b.sha.as_str()).unwrap_or("");
            let l = Line::new()
                .str("path", p.as_str())
                .int("bytes", bytes.len() as u64)
                .str("blake3", blake3::hash(&bytes).to_hex().to_string())
                .str("git_blob", blob);
            manifest.push_str(&l.to_json());
            manifest.push('\n');
        }
        for l in set.license_files {
            std::fs::copy(work.join("files").join(l), dest.join(l)).map_err(|e| format!("{l}：{e}"))?;
        }
        std::fs::write(dest.join("MANIFEST.jsonl"), manifest).map_err(|e| e.to_string())?;
        let picked = if set.take == 0 || set.take >= candidates {
            format!("当てはまる {candidates} 件の全部")
        } else {
            format!("当てはまる {candidates} 件から {} 件を種つきで抜き取り（ChaCha8・種 {}）", chosen.len(), set.seed)
        };
        let source = format!(
            "# 出どころ：{name}\n\n\
             | 項目 | 値 |\n|---|---|\n\
             | リポジトリ | https://github.com/{repo} |\n\
             | 版 | コミット `{rev}` |\n\
             | 取ったもの | `{prefix}` の下{contains}の `.svg`（16 MiB 以下）。{picked}。合わせて {total} バイト |\n\
             | 取り方 | `https://raw.githubusercontent.com/{repo}/{rev}/<道>` から取り、tree の一覧の git の blob の SHA-1 を照らした |\n\
             | ライセンス | {license}（{files} はリポジトリの根の写し） |\n\
             | 性質 | {traits} |\n\
             {attribution}\
             | 一覧 | `MANIFEST.jsonl`（道・バイト数・BLAKE3・git の blob） |\n\n\
             作り直し：`cargo run -p sumie-tools -- corpus fetch-real --only {name}`（ADR-59）\n",
            name = set.name,
            repo = set.repo,
            rev = set.rev,
            prefix = set.prefix,
            contains = if set.contains.is_empty() {
                String::new()
            } else {
                format!("（道に `{}` を含むもの）", set.contains)
            },
            license = set.license,
            files = set.license_files.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join("・"),
            traits = set.traits,
            attribution =
                if set.attribution.is_empty() { String::new() } else { format!("| 帰属 | {} |\n", set.attribution) },
        );
        std::fs::write(dest.join("SOURCE.md"), source).map_err(|e| e.to_string())?;
        println!("corpus/real/{}：{} 件（候補 {candidates} 件）・{total} バイト", set.name, chosen.len());
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

// ---- Wikimedia Commons ----

/// Commons のライセンスの短い名前が、取ってよいもの（パブリックドメイン・CC0・CC BY。BY-SA などは外す）か。
pub fn commons_license_ok(short: &str) -> bool {
    let s = short.trim();
    if s == "Public domain" || s.starts_with("CC0") || s.starts_with("PD") {
        return true;
    }
    // "CC BY 4.0"・"CC BY 3.0"（"CC BY-SA 3.0"・"CC BY-NC" は外す）
    s.strip_prefix("CC BY ").is_some_and(|v| v.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
}

/// HTML の印を除き、空白を詰める（作者の欄）。
pub fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

/// Commons の 1 件。
struct CommonsFile {
    title: String,
    url: String,
    size: u64,
    sha1: String,
    license: String,
    license_url: String,
    artist: String,
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn commons_list() -> Result<(Vec<CommonsFile>, usize), String> {
    let mut out = Vec::new();
    let mut seen = 0usize;
    let mut cont: Option<String> = None;
    loop {
        let mut url = format!(
            "https://commons.wikimedia.org/w/api.php?action=query&format=json&formatversion=2&generator=categorymembers\
             &gcmtitle={}&gcmtype=file&gcmlimit=50&prop=imageinfo&iiprop=url%7Csize%7Csha1%7Cmime%7Cextmetadata\
             &iiextmetadatafilter=LicenseShortName%7CLicenseUrl%7CArtist",
            url_encode(COMMONS_CATEGORY)
        );
        if let Some(c) = &cont {
            let _ = write!(url, "&gcmcontinue={}&continue=gcmcontinue%7C%7C", url_encode(c));
        }
        if cont.is_some() {
            // 一覧の API も間を空けて頼む
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        let j = parse_json(&curl_text(&url)?)?;
        for p in j.get("query").and_then(|q| q.get("pages")).and_then(J::as_arr).unwrap_or(&[]) {
            seen += 1;
            let title = p.get("title").and_then(J::as_str).unwrap_or("").to_string();
            let Some(ii) = p.get("imageinfo").and_then(J::as_arr).and_then(|a| a.first()) else { continue };
            if ii.get("mime").and_then(J::as_str) != Some("image/svg+xml") {
                continue;
            }
            let meta = |k: &str| {
                ii.get("extmetadata")
                    .and_then(|m| m.get(k))
                    .and_then(|v| v.get("value"))
                    .and_then(J::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            let license = meta("LicenseShortName");
            let size = ii.get("size").and_then(J::as_u64).unwrap_or(u64::MAX);
            if !commons_license_ok(&license) || size > MAX_BYTES {
                continue;
            }
            out.push(CommonsFile {
                title,
                url: ii.get("url").and_then(J::as_str).unwrap_or("").to_string(),
                size,
                sha1: ii.get("sha1").and_then(J::as_str).unwrap_or("").to_string(),
                license,
                license_url: meta("LicenseUrl"),
                artist: strip_html(&meta("Artist")),
            });
        }
        cont = j.get("continue").and_then(|c| c.get("gcmcontinue")).and_then(J::as_str).map(str::to_string);
        if cont.is_none() {
            break;
        }
    }
    out.sort_by(|a, b| a.title.cmp(&b.title));
    Ok((out, seen))
}

/// 題からファイルの名前（`File:` を除き、ASCII の英数と `-_.` 以外を `_` に。重なれば番号を足す）。
pub fn file_name(title: &str, taken: &[String]) -> String {
    let t = title.strip_prefix("File:").unwrap_or(title);
    let mut base: String =
        t.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    if !base.to_ascii_lowercase().ends_with(".svg") {
        base.push_str(".svg");
    }
    let mut name = base.clone();
    let mut k = 2;
    while taken.contains(&name) {
        name = format!("{}-{k}.svg", base.trim_end_matches(".svg"));
        k += 1;
    }
    name
}

fn sha1_of(path: &Path) -> Result<String, String> {
    let o = Command::new("shasum").args(["-a", "1"]).arg(path).output().map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err("shasum が失敗した".into());
    }
    Ok(String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap_or("").to_string())
}

fn fetch_commons(root: &Path) -> Result<(), String> {
    let (list, seen) = commons_list()?;
    let work = root.join(format!("local/real-fetch-commons-{}", std::process::id()));
    if work.exists() {
        return Err(format!("{} がもうある", work.display()));
    }
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        let mut names: Vec<String> = Vec::new();
        for f in &list {
            let n = file_name(&f.title, &names);
            names.push(n);
        }
        let items: Vec<(String, PathBuf)> =
            list.iter().zip(&names).map(|(f, n)| (f.url.clone(), work.join("files").join(n))).collect();
        download(&items, &work, Pace::Gentle)?;
        let dest = root.join("local").join(COMMONS_NAME);
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
        let mut manifest = String::new();
        let mut total = 0u64;
        for ((f, n), (_, p)) in list.iter().zip(&names).zip(&items) {
            let got = sha1_of(p)?;
            if got != f.sha1 {
                return Err(format!("{} の SHA-1 が違う：{got}（期待 {}）", f.title, f.sha1));
            }
            let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
            if bytes.len() as u64 != f.size {
                return Err(format!("{} の大きさが違う", f.title));
            }
            std::fs::write(dest.join(n), &bytes).map_err(|e| e.to_string())?;
            total += f.size;
            let l = Line::new()
                .str("path", n.as_str())
                .str("title", f.title.as_str())
                .str("url", f.url.as_str())
                .str("license", f.license.as_str())
                .str("license_url", f.license_url.as_str())
                .str("artist", f.artist.as_str())
                .int("bytes", f.size)
                .str("sha1", f.sha1.as_str())
                .str("blake3", blake3::hash(&bytes).to_hex().to_string());
            manifest.push_str(&l.to_json());
            manifest.push('\n');
        }
        let rec = root.join("corpus/real").join(COMMONS_NAME);
        std::fs::create_dir_all(&rec).map_err(|e| e.to_string())?;
        std::fs::write(rec.join("MANIFEST.jsonl"), manifest).map_err(|e| e.to_string())?;
        std::fs::write(
            rec.join("SOURCE.md"),
            format!(
                "# 出どころ：Wikimedia Commons の秀逸な画像（ベクター）\n\n\
                 | 項目 | 値 |\n|---|---|\n\
                 | 出どころ | https://commons.wikimedia.org/wiki/{cat} |\n\
                 | 取ったもの | 分類の {seen} 件のうち、SVG で、ライセンスがパブリックドメイン・CC0・CC BY（BY-SA・NC は外す）で、16 MiB 以下の {n} 件（合わせて {total} バイト） |\n\
                 | 取り方 | Commons の API の一覧の URL から取り、Commons の SHA-1 を照らした |\n\
                 | 置き場 | git の外（`local/{name}/`）。大きいので。この一覧だけを git に入れる |\n\
                 | ライセンス | ファイルごと（`MANIFEST.jsonl` の `license`・`license_url`）。CC BY のものの作者は `artist`（帰属） |\n\
                 | 性質 | 編集ソフト（Inkscape・Illustrator など）の書き出し：文字・フィルタ・mask・大きな地図と図 |\n\n\
                 分類の中身は Commons の側で変わりうる。作り直し：`cargo run -p sumie-tools -- corpus fetch-real --only {name}`\n\
                 （ADR-59。中身が変わったら一覧の差を見る）\n",
                cat = COMMONS_CATEGORY.replace(' ', "_"),
                n = list.len(),
                name = COMMONS_NAME,
            ),
        )
        .map_err(|e| e.to_string())?;
        println!("local/{COMMONS_NAME}：{} 件（分類の {seen} 件のうち）・{total} バイト", list.len());
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

// ---- 小さな JSON の読み（GitHub と Commons の API の返事。道具の側なので再帰でよい）----

#[derive(Debug, Clone, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub fn get(&self, k: &str) -> Option<&J> {
        match self {
            J::Obj(kv) => kv.iter().find(|(key, _)| key == k).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let J::Str(s) = self { Some(s) } else { None }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let J::Bool(b) = self { Some(*b) } else { None }
    }
    pub fn as_arr(&self) -> Option<&[J]> {
        if let J::Arr(a) = self { Some(a) } else { None }
    }
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            J::Num(n) if *n >= 0.0 && n.fract() == 0.0 && *n < 9.0e15 => Some(*n as u64),
            _ => None,
        }
    }
}

pub fn parse_json(s: &str) -> Result<J, String> {
    let b = s.as_bytes();
    let mut i = 0;
    let v = value(b, &mut i)?;
    skip_ws(b, &mut i);
    if i != b.len() {
        return Err(format!("JSON の後ろに余りがある（{i} バイト目）"));
    }
    Ok(v)
}

fn skip_ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
}

fn value(b: &[u8], i: &mut usize) -> Result<J, String> {
    skip_ws(b, i);
    match b.get(*i) {
        Some(b'{') => {
            *i += 1;
            let mut kv = Vec::new();
            skip_ws(b, i);
            if b.get(*i) == Some(&b'}') {
                *i += 1;
                return Ok(J::Obj(kv));
            }
            loop {
                skip_ws(b, i);
                let k = string(b, i)?;
                skip_ws(b, i);
                if b.get(*i) != Some(&b':') {
                    return Err(format!("JSON の : がない（{i}）", i = *i));
                }
                *i += 1;
                kv.push((k, value(b, i)?));
                skip_ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b'}') => {
                        *i += 1;
                        return Ok(J::Obj(kv));
                    }
                    _ => return Err(format!("JSON の物が閉じない（{i}）", i = *i)),
                }
            }
        }
        Some(b'[') => {
            *i += 1;
            let mut a = Vec::new();
            skip_ws(b, i);
            if b.get(*i) == Some(&b']') {
                *i += 1;
                return Ok(J::Arr(a));
            }
            loop {
                a.push(value(b, i)?);
                skip_ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b']') => {
                        *i += 1;
                        return Ok(J::Arr(a));
                    }
                    _ => return Err(format!("JSON の並びが閉じない（{i}）", i = *i)),
                }
            }
        }
        Some(b'"') => Ok(J::Str(string(b, i)?)),
        Some(b't') if b[*i..].starts_with(b"true") => {
            *i += 4;
            Ok(J::Bool(true))
        }
        Some(b'f') if b[*i..].starts_with(b"false") => {
            *i += 5;
            Ok(J::Bool(false))
        }
        Some(b'n') if b[*i..].starts_with(b"null") => {
            *i += 4;
            Ok(J::Null)
        }
        Some(c) if c.is_ascii_digit() || *c == b'-' => {
            let start = *i;
            while *i < b.len() && (b[*i].is_ascii_digit() || b"+-.eE".contains(&b[*i])) {
                *i += 1;
            }
            let t = std::str::from_utf8(&b[start..*i]).map_err(|e| e.to_string())?;
            t.parse::<f64>().map(J::Num).map_err(|_| format!("JSON の数が読めない：{t}"))
        }
        _ => Err(format!("JSON の値が読めない（{i}）", i = *i)),
    }
}

fn hex4(b: &[u8], at: usize) -> Result<u32, String> {
    let h = b.get(at..at + 4).ok_or("\\u の後ろが短い")?;
    u32::from_str_radix(std::str::from_utf8(h).map_err(|e| e.to_string())?, 16).map_err(|e| e.to_string())
}

fn string(b: &[u8], i: &mut usize) -> Result<String, String> {
    if b.get(*i) != Some(&b'"') {
        return Err(format!("JSON の文字列でない（{i}）", i = *i));
    }
    *i += 1;
    let mut out: Vec<u8> = Vec::new();
    while let Some(&c) = b.get(*i) {
        match c {
            b'"' => {
                *i += 1;
                return String::from_utf8(out).map_err(|e| e.to_string());
            }
            b'\\' => {
                let e = *b.get(*i + 1).ok_or("\\ の後ろがない")?;
                *i += 2;
                match e {
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'/' => out.push(b'/'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        let mut cp = hex4(b, *i)?;
                        *i += 4;
                        if (0xD800..0xDC00).contains(&cp) && b.get(*i..*i + 2) == Some(b"\\u") {
                            let lo = hex4(b, *i + 2)?;
                            if (0xDC00..0xE000).contains(&lo) {
                                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                *i += 6;
                            }
                        }
                        let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    _ => return Err("JSON の知らない逃がし".into()),
                }
            }
            _ => {
                out.push(c);
                *i += 1;
            }
        }
    }
    Err("JSON の文字列が閉じない".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_and_helpers() {
        let j = parse_json(r#"{"a": [1, {"b": "x\"yé😀"}], "t": true, "n": null}"#).unwrap();
        assert_eq!(j.get("a").and_then(J::as_arr).map(|a| a.len()), Some(2));
        let b = j.get("a").and_then(J::as_arr).unwrap()[1].get("b").and_then(J::as_str).unwrap().to_string();
        assert_eq!(b, "x\"yé😀");
        assert_eq!(j.get("t").and_then(J::as_bool), Some(true));
        assert!(parse_json("{\"a\": 1,}").is_err());
        assert!(commons_license_ok("Public domain") && commons_license_ok("CC BY 4.0") && commons_license_ok("CC0"));
        assert!(
            !commons_license_ok("CC BY-SA 3.0") && !commons_license_ok("CC BY-NC 4.0") && !commons_license_ok("GFDL")
        );
        assert_eq!(strip_html("<a href=\"x\">Ann  B</a>\n (talk)"), "Ann B (talk)");
        assert_eq!(file_name("File:Café map.svg", &[]), "Caf__map.svg");
        assert_eq!(file_name("File:a.svg", &["a.svg".to_string()]), "a-2.svg");
        assert_eq!(encode_path("assets/Grinning face/Color/a.svg"), "assets/Grinning%20face/Color/a.svg");
    }

    #[test]
    fn sampling_is_fixed_and_sorted() {
        let paths: Vec<String> = (0..100).map(|k| format!("p{k:03}.svg")).collect();
        let a = sample(paths.clone(), 10, 7);
        let b = sample(paths.iter().rev().cloned().collect(), 10, 7);
        assert_eq!(a, b, "入力の並びに依らない");
        assert_eq!(a.len(), 10);
        assert!(a.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(sample(paths.clone(), 0, 1).len(), 100);
        assert_ne!(sample(paths, 10, 8), a);
    }
}
