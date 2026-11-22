//! `check-lint-probe`：crates/sumie/clippy.toml の禁じる口が本当に拾われるかを、試しのクレート（scripts/lint-probe）で
//! 確かめる（clippy は引けないパスを黙って無視することがあるため）。
//!
//! - `// 期待: <パス> …` の行：そのパスがすべて「use of a disallowed …」で拾われること
//! - `// 許す` の行：何も拾われないこと
//! - clippy が設定のパスを引けない（does not refer to a reachable function）ものがないこと

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

pub fn run(_args: &[String]) -> Result<(), String> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).ok_or("workspace の根が見つからない")?;
    let probe = root.join("scripts/lint-probe");
    let out = Command::new("cargo")
        .args(["clippy", "--quiet", "--message-format=short", "--manifest-path"])
        .arg(probe.join("Cargo.toml"))
        .env("CLIPPY_CONF_DIR", root.join("crates/sumie"))
        .env("CARGO_TARGET_DIR", root.join("target/lint-probe"))
        .output()
        .map_err(|e| format!("cargo clippy を起動できない：{e}"))?;
    let text = String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    let unreachable: Vec<&str> = text.lines().filter(|l| l.contains("does not refer to a reachable")).collect();
    if !unreachable.is_empty() {
        return Err(format!("clippy が引けない口が crates/sumie/clippy.toml にある：\n{}", unreachable.join("\n")));
    }
    // 組めない（誤りがある）なら照らせない。警告（disallowed の拾い）は誤りでない
    if !out.status.success() || text.lines().any(|l| l.contains(": error[") || l.starts_with("error[")) {
        return Err(format!("試しのクレートが組めない：\n{text}"));
    }
    // 行 → 拾われた口
    let mut got: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for l in text.lines() {
        let Some(rest) = l.strip_prefix("src/lib.rs:") else { continue };
        let Some((line, tail)) = rest.split_once(':') else { continue };
        let Ok(line) = line.parse::<usize>() else { continue };
        if let Some(i) = tail.find("use of a disallowed ") {
            let t = &tail[i..];
            if let (Some(a), Some(b)) = (t.find('`'), t.rfind('`'))
                && b > a
            {
                got.entry(line).or_default().insert(t[a + 1..b].to_string());
            }
        }
    }
    let src = std::fs::read_to_string(probe.join("src/lib.rs")).map_err(|e| e.to_string())?;
    let mut problems = Vec::new();
    let (mut want_n, mut ok_n) = (0usize, 0usize);
    for (k, l) in src.lines().enumerate() {
        let line = k + 1;
        let empty = BTreeSet::new();
        let have = got.get(&line).unwrap_or(&empty);
        if l.trim_start().starts_with("//") {
            continue; // 注釈だけの行（説明）
        }
        if let Some(i) = l.find("// 期待: ") {
            for p in l[i + "// 期待: ".len()..].split_whitespace() {
                want_n += 1;
                if !have.contains(p) {
                    problems.push(format!("{line} 行：{p} が拾われない（拾われたもの：{have:?}）"));
                }
            }
        } else if l.contains("// 許す") {
            ok_n += 1;
            if !have.is_empty() {
                problems.push(format!("{line} 行：許す口なのに拾われた：{have:?}"));
            }
        }
    }
    if !problems.is_empty() {
        return Err(format!("禁じる口の試しが合わない：\n{}", problems.join("\n")));
    }
    println!("禁じる口の試し：期待の {want_n} の口がすべて拾われ、許す {ok_n} 行は拾われない");
    Ok(())
}
