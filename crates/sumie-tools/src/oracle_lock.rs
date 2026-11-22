//! `oracle-lock`：照合の相手の resvg の依存の閉包の版を、Kura の Cargo.lock と揃える（ADR-52・ADR-57 (1)）。
//!
//! resvg の下の svgtypes・roxmltree・unicode-script などの版が Kura と違うと、色の書き方・書体名・文字の区切りの
//! 読み方が Kura の今の振る舞いとずれる。
//!
//! - `oracle-lock --kura <Kura の Cargo.lock> --rev <Kura のコミット>`：Kura の lock の resvg の閉包を
//!   `crates/sumie-tools/oracle-lock.txt` に書く（Kura の rev を頭に記録する）
//! - 試験 `oracle_closure_matches_kura`：この workspace の Cargo.lock の resvg の閉包が、記録と同じ版か（Kura だけにある
//!   名前は、Kura のほかのクレートが feature で足したもので、値に効かないので許す）。揃えるのは
//!   `cargo update -p <名前> --precise <版>`

use std::collections::BTreeMap;

use crate::corpus::{arg, workspace_root};

/// 記録のファイル（workspace の根から）。
pub const RECORD: &str = "crates/sumie-tools/oracle-lock.txt";

/// Cargo.lock の 1 つの package。
struct Pkg {
    name: String,
    version: String,
    /// crates.io の checksum（path・git の出どころにはない。差し替えを見分ける）
    checksum: String,
    deps: Vec<String>,
}

fn parse(lock: &str) -> Vec<Pkg> {
    let mut out: Vec<Pkg> = Vec::new();
    let mut in_deps = false;
    for line in lock.lines() {
        if line == "[[package]]" {
            out.push(Pkg { name: String::new(), version: String::new(), checksum: String::new(), deps: Vec::new() });
            in_deps = false;
            continue;
        }
        let Some(p) = out.last_mut() else { continue };
        if in_deps {
            if line == "]" {
                in_deps = false;
            } else if let Some(d) =
                line.trim().strip_prefix('"').and_then(|d| d.trim_end_matches(',').strip_suffix('"'))
            {
                p.deps.push(d.to_string());
            }
        } else if let Some(v) = line.strip_prefix("name = \"").and_then(|v| v.strip_suffix('"')) {
            p.name = v.to_string();
        } else if let Some(v) = line.strip_prefix("version = \"").and_then(|v| v.strip_suffix('"')) {
            p.version = v.to_string();
        } else if let Some(v) = line.strip_prefix("checksum = \"").and_then(|v| v.strip_suffix('"')) {
            p.checksum = v.to_string();
        } else if line == "dependencies = [" {
            in_deps = true;
        }
    }
    out
}

/// `root` の依存の閉包（名前 → 「版 checksum」。同じ名前の 2 つの版は「名前 版」を鍵にする）。checksum がない（path・git
/// の出どころ）ものは `-`。
pub fn closure(lock: &str, root: &str) -> Result<BTreeMap<String, String>, String> {
    let pkgs = parse(lock);
    let find = |spec: &str| -> Option<&Pkg> {
        let mut it = spec.split(' ');
        let name = it.next().unwrap_or("");
        let version = it.next();
        pkgs.iter().find(|p| p.name == name && version.is_none_or(|v| v == p.version))
    };
    let mut seen: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut stack = vec![find(root).ok_or_else(|| format!("{root} が lock にない"))?];
    while let Some(p) = stack.pop() {
        let sum = if p.checksum.is_empty() { "-".to_string() } else { p.checksum.clone() };
        if seen.insert((p.name.clone(), p.version.clone()), sum).is_some() {
            continue;
        }
        for d in &p.deps {
            stack.push(find(d).ok_or_else(|| format!("{d} が lock にない"))?);
        }
    }
    let mut out = BTreeMap::new();
    let names: Vec<&String> = seen.keys().map(|(n, _)| n).collect();
    for ((n, v), sum) in &seen {
        let key = if names.iter().filter(|x| **x == n).count() > 1 { format!("{n} {v}") } else { n.clone() };
        out.insert(key, format!("{v} {sum}"));
    }
    Ok(out)
}

/// この workspace の閉包と記録の食い違い（版が違う・この workspace だけにある名前）。
pub fn mismatches(ours: &BTreeMap<String, String>, kura: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for (n, v) in ours {
        match kura.get(n) {
            Some(k) if k == v => {}
            Some(k) => {
                let ver = k.split(' ').next().unwrap_or("");
                out.push(format!("{n}：この workspace {v}・Kura {k}（版が違えば cargo update -p {n} --precise {ver}。checksum が違えば出どころの差し替え）"))
            }
            None => out.push(format!("{n} {v}：Kura の閉包にない")),
        }
    }
    out
}

pub fn run(args: &[String]) -> Result<(), String> {
    crate::corpus::check_args(args, &["--kura", "--rev"], &[], 0)?;
    let kura = arg(args, "--kura").ok_or("--kura <Kura の Cargo.lock>")?;
    let rev = arg(args, "--rev").ok_or("--rev <Kura のコミット>")?;
    let text = std::fs::read_to_string(kura).map_err(|e| format!("{kura}：{e}"))?;
    let c = closure(&text, "resvg")?;
    let mut s = String::from(
        "# Kura の Cargo.lock の resvg の依存の閉包（`sumie-tools oracle-lock` が書く。手で書き換えない）。\n\
         # この workspace の Cargo.lock の閉包が同じ版かを crates/sumie-tools の試験 oracle_closure_matches_kura が見る\n",
    );
    s.push_str(&format!("# kura-rev: {rev}\n"));
    for (n, v) in &c {
        s.push_str(&format!("{n} {v}\n"));
    }
    std::fs::write(workspace_root().join(RECORD), s).map_err(|e| e.to_string())?;
    let ours = std::fs::read_to_string(workspace_root().join("Cargo.lock")).map_err(|e| e.to_string())?;
    let bad = mismatches(&closure(&ours, "resvg")?, &c);
    println!("{RECORD} を書いた（{} 件）", c.len());
    if !bad.is_empty() {
        return Err(format!("この workspace の閉包が Kura と違う：\n{}", bad.join("\n")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 記録を読む（名前 → 「版 checksum」）と、記録した Kura の rev。
    fn read_record(text: &str) -> (String, BTreeMap<String, String>) {
        let mut rev = String::new();
        let mut out = BTreeMap::new();
        for l in text.lines() {
            if let Some(r) = l.strip_prefix("# kura-rev: ") {
                rev = r.trim().to_string();
            } else if !l.starts_with('#') {
                // 「名前 版 checksum」（名前に空白が入るのは「名前 版」の鍵のとき）
                let parts: Vec<&str> = l.split(' ').collect();
                if parts.len() >= 3 {
                    let (name, rest) = parts.split_at(parts.len() - 2);
                    out.insert(name.join(" "), rest.join(" "));
                }
            }
        }
        (rev, out)
    }

    #[test]
    fn oracle_closure_matches_kura() {
        let root = workspace_root();
        let ours = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
        let rec = std::fs::read_to_string(root.join(RECORD)).unwrap();
        let (rev, kura) = read_record(&rec);
        assert_eq!(rev.len(), 40, "記録に Kura の rev がない");
        let bad = mismatches(&closure(&ours, "resvg").unwrap(), &kura);
        assert!(bad.is_empty(), "照合の相手の依存の版が Kura（{rev}）と違う：\n{}", bad.join("\n"));
    }

    #[test]
    fn closure_follows_versions() {
        let lock = "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\ndependencies = [\n \"b 2.0.0\",\n \"c\",\n]\n\n\
                    [[package]]\nname = \"b\"\nversion = \"2.0.0\"\n\n[[package]]\nname = \"b\"\nversion = \"3.0.0\"\n\n\
                    [[package]]\nname = \"c\"\nversion = \"0.1.0\"\ndependencies = [\n \"b 3.0.0\",\n]\n";
        let c = closure(lock, "a").unwrap();
        assert_eq!(c.get("b 2.0.0").map(String::as_str), Some("2.0.0 -"));
        assert_eq!(c.get("b 3.0.0").map(String::as_str), Some("3.0.0 -"));
        assert_eq!(c.get("c").map(String::as_str), Some("0.1.0 -"));
        let mut k = c.clone();
        k.insert("c".into(), "0.1.0 abc".into());
        k.insert("only-kura".into(), "1.0.0".into());
        assert_eq!(mismatches(&c, &k).len(), 1);
    }
}
