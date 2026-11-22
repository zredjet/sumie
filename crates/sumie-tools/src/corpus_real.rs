//! `corpus fetch-real`：実際の SVG の集まり（ADR-59。Kura の手元の SVG の代わり）を取り込む。
//!
//! - git の集まり（[`GIT_SETS`]）：決めた rev の GitHub のリポジトリから、決めた道の SVG を種つきで抜き取り（ChaCha8）、
//!   `corpus/real/<名前>/` に写す。ファイルは raw.githubusercontent.com から取り、git の blob の SHA-1（tree の一覧の値）を
//!   `git hash-object` で照らしてから写す。ライセンスのファイルも同じく照らして写す。`MANIFEST.jsonl`（道・バイト数・
//!   BLAKE3・blob）と `SOURCE.md` を書く
//!
//! `corpus fetch-real [--only <名前>]`（`--only` がなければすべての集まり）。Wikimedia Commons は使わない（ADR-63 (2)）。取ったものは信用しない入力なので、新しい空のフォルダに取ってから照らして写す。

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

/// 1 件の大きさの上限（Kura の入力の上限）。
const MAX_BYTES: u64 = 16 << 20;

pub fn run(args: &[String]) -> Result<(), String> {
    let only = arg(args, "--only");
    let root = workspace_root();
    for set in GIT_SETS.iter().filter(|s| only.is_none_or(|o| o == s.name)) {
        fetch_git_set(&root, set)?;
    }
    if only.is_some_and(|o| !GIT_SETS.iter().any(|s| s.name == o)) {
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

/// (URL, 置き場) の並びを curl でまとめて取る（GitHub の raw は並べて取ってよい）。
fn download(items: &[(String, PathBuf)], work: &Path) -> Result<(), String> {
    let mut cfg = String::new();
    for (url, to) in items {
        let to = to.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        let _ = writeln!(cfg, "url = \"{url}\"\noutput = \"{to}\"");
    }
    let cfg_path = work.join("curl.cfg");
    std::fs::write(&cfg_path, cfg).map_err(|e| e.to_string())?;
    let st = Command::new("curl")
        .args(["-sSfL", "--create-dirs", "-A", USER_AGENT, "--parallel", "--parallel-max", "8"])
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
        download(&items, &work)?;
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

// ---- 小さな JSON の読み（GitHub の API の返事。道具の側なので再帰でよい）----

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
