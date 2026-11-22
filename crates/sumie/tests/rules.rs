//! ライブラリの src の走査（determinism.md §9・CLAUDE.md）。clippy で拾えない規則を字句で見張る。
//!
//! - ビルドで分かれるもの：`cfg(…)`・`cfg!`・`cfg_attr`（`#[cfg(test)] mod tests;` だけ許す）・`is_*_feature_detected!`・
//!   Cargo.toml の `[features]`・build.rs
//! - std・core・alloc のモジュールは表にあるものだけ（時刻・環境変数・スレッド・ファイル・入出力・`arch` などを使わない）
//! - `mul_add`・`algebraic_*`・`unsafe`
//! - `mem` の外での確保（`Vec`・`String`・`Box`・`vec!`・`format!`・`.to_vec()`・`.collect()`・`.clone()`・`Arc::new` など）
//! - 厳しい lint の `allow`・`expect` は `mem` の中だけ（理由つき）
//! - `MVec`・`Frozen` に入れる型（`Plain`）は `#[repr(C)]` か基本の型
//! - モジュールの層の向き（architecture.md §2）
//! - 自分の名前を呼ぶ関数（再帰）
//! - 大域の状態（`OnceLock`・`Mutex`・`thread_local` など）と、`cancel.rs` の外の原子の型
//!
//! 試験のコード（`src/**/tests.rs`）は走査しない。各規則は下の自己試験で、拾うことと拾わないことを確かめる。

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

use std::path::{Path, PathBuf};

use sumie_testkit::rustscan::{Kind, Token, lex};

/// モジュールの層（architecture.md §2）。(名前, 層, 層の中の順)。同じ層の中は順の小さいものを大きいものから使ってよい
/// （矢印の向き）。層 0 の中は順を問わない。
const LAYERS: &[(&str, u32, u32)] = &[
    ("math", 0, 0),
    ("fixed", 0, 0),
    ("mem", 0, 0),
    ("budget", 0, 0),
    ("cost", 0, 0),
    ("error", 0, 0),
    ("cancel", 0, 0),
    ("report", 0, 0),
    ("unicode", 0, 0),
    ("geom", 1, 0),
    ("xml", 2, 0),
    ("css", 2, 1),
    ("values", 2, 2),
    ("style", 2, 3),
    ("doc", 2, 4),
    ("font", 3, 0),
    ("text", 3, 1),
    ("tree", 4, 0),
    ("raster", 5, 0),
    ("paint", 5, 1),
    ("filter", 5, 2),
    ("compose", 5, 3),
    ("plan", 5, 4),
    ("render", 5, 5),
    ("fuzzing", 6, 0),
    ("lib", 6, 0),
];

/// std・core・alloc の 2 つ目のパスで使ってよいもの。`sync`・`collections`・`vec` などは下の特別の規則で見る。
const STD_OK: &[&str] = &[
    "cell",
    "cmp",
    "fmt",
    "mem",
    "ops",
    "num",
    "marker",
    "convert",
    "iter",
    "slice",
    "str",
    "char",
    "array",
    "f64",
    "f32",
    "hint",
    "error",
    "option",
    "result",
    "default",
    "borrow",
    "primitive",
    "sync",
    "collections",
    "vec",
    "string",
    "boxed",
];

/// 厳しい lint（`allow`・`expect` は mem の中だけ）。
const STRICT_LINTS: &[&str] = &[
    "unwrap_used",
    "expect_used",
    "panic",
    "unreachable",
    "todo",
    "unimplemented",
    "exit",
    "indexing_slicing",
    "string_slice",
    "arithmetic_side_effects",
    "large_stack_arrays",
    "large_stack_frames",
    "disallowed_methods",
    "disallowed_types",
    "disallowed_macros",
];

/// mem の外で使わない型の名前（確保する入れ物）。
const ALLOC_TYPES: &[&str] = &[
    "Vec",
    "String",
    "Box",
    "Rc",
    "BTreeMap",
    "BTreeSet",
    "VecDeque",
    "BinaryHeap",
    "HashMap",
    "HashSet",
    "LinkedList",
];

/// mem の外で呼ばない口の名前（`.名前(` の形）。
const ALLOC_METHODS: &[&str] = &[
    "to_vec",
    "to_owned",
    "to_string",
    "collect",
    "clone",
    "extend",
    "extend_from_slice",
    "into_boxed_slice",
    "into_vec",
    "concat",
    "join",
    "repeat",
    "resize",
];

/// 大域の状態を作る名前。
const GLOBAL_STATE: &[&str] =
    &["OnceLock", "OnceCell", "LazyLock", "LazyCell", "Mutex", "RwLock", "thread_local", "Condvar"];

/// 基本の型（`Plain` を実装してよい）。
const PRIMS: &[&str] = &[
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64", "bool",
    "char",
];

#[derive(Debug)]
struct Finding {
    file: String,
    line: u32,
    what: String,
}

/// 走査する 1 つのファイル。
struct Src {
    /// src からの相対のパス（`mem/sorted.rs` など）
    rel: String,
    /// 先頭のモジュール（`mem`。lib.rs は `lib`）
    module: String,
    tokens: Vec<Token>,
}

impl Src {
    fn new(rel: &str, text: &str) -> Src {
        let module = rel.split('/').next().unwrap_or("").trim_end_matches(".rs").to_string();
        Src { rel: rel.to_string(), module, tokens: lex(text) }
    }

    fn find(&self, line: u32, what: String) -> Finding {
        Finding { file: self.rel.clone(), line, what }
    }

    fn in_mem(&self) -> bool {
        self.module == "mem"
    }
}

fn is_test_file(rel: &str) -> bool {
    rel.ends_with("/tests.rs") || rel == "tests.rs" || rel.contains("/tests/")
}

fn sources() -> Vec<Src> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();
    files
        .into_iter()
        .filter_map(|p| {
            let rel = p.strip_prefix(&root).ok()?.to_string_lossy().replace('\\', "/");
            if is_test_file(&rel) {
                return None;
            }
            let text = std::fs::read_to_string(&p).ok()?;
            Some(Src::new(&rel, &text))
        })
        .collect()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn ident_at(t: &[Token], k: usize, name: &str) -> bool {
    t.get(k).is_some_and(|x| x.is_ident(name))
}

fn punct_at(t: &[Token], k: usize, c: char) -> bool {
    t.get(k).is_some_and(|x| x.is_punct(c))
}

/// `t[k]` から `::` が始まるか。
fn path_sep(t: &[Token], k: usize) -> bool {
    punct_at(t, k, ':') && punct_at(t, k + 1, ':')
}

// ---- 規則 ----

/// ビルドで分かれるもの。`#[cfg(test)] mod tests;` だけを許す。
fn check_cfg(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    let mut out = Vec::new();
    for k in 0..t.len() {
        let x = &t[k];
        if x.kind != Kind::Ident {
            continue;
        }
        if matches!(
            x.text.as_str(),
            "is_x86_feature_detected"
                | "is_aarch64_feature_detected"
                | "is_arm_feature_detected"
                | "is_riscv_feature_detected"
        ) {
            out.push(s.find(x.line, format!("{} はターゲットで処理を分ける（determinism.md §7）", x.text)));
        }
        if x.text == "cfg" || x.text == "cfg_attr" {
            // #[cfg(test)] mod tests;
            let ok = x.text == "cfg"
                && k >= 2
                && punct_at(t, k - 1, '[')
                && punct_at(t, k - 2, '#')
                && punct_at(t, k + 1, '(')
                && ident_at(t, k + 2, "test")
                && punct_at(t, k + 3, ')')
                && punct_at(t, k + 4, ']')
                && ident_at(t, k + 5, "mod")
                && ident_at(t, k + 6, "tests")
                && punct_at(t, k + 7, ';');
            if !ok {
                out.push(s.find(
                    x.line,
                    "cfg はビルドで振る舞いを分ける。許すのは `#[cfg(test)] mod tests;` だけ（determinism.md §1.2・§7）".into(),
                ));
            }
        }
    }
    out
}

/// std・core・alloc のモジュールの表。
fn check_std(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    let mut out = Vec::new();
    for k in 0..t.len() {
        let x = &t[k];
        if !(x.is_ident("std") || x.is_ident("core") || x.is_ident("alloc")) || !path_sep(t, k + 1) {
            continue;
        }
        // 直前が :: なら（crate::std など）別のもの
        if k >= 1 && punct_at(t, k - 1, ':') {
            continue;
        }
        // 2 つ目（{a, b} の並びなら 1 段目のすべて）
        let mut seconds: Vec<(usize, &Token)> = Vec::new();
        if punct_at(t, k + 3, '{') {
            let mut depth = 0i32;
            let mut j = k + 3;
            while let Some(y) = t.get(j) {
                if y.is_punct('{') {
                    depth += 1;
                } else if y.is_punct('}') {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                } else if depth == 1 && y.kind == Kind::Ident && (punct_at(t, j - 1, '{') || punct_at(t, j - 1, ',')) {
                    seconds.push((j, y));
                }
                j += 1;
            }
        } else if let Some(y) = t.get(k + 3).filter(|y| y.kind == Kind::Ident) {
            seconds.push((k + 3, y));
        }
        for (j, y) in seconds {
            let name = y.text.as_str();
            if !STD_OK.contains(&name) {
                out.push(s.find(y.line, format!("{}::{name} は使わないモジュール（rules.rs の STD_OK）", x.text)));
                continue;
            }
            let third = if path_sep(t, j + 1) { t.get(j + 3).map(|z| z.text.as_str()) } else { None };
            match name {
                "sync" if !matches!(third, Some("Arc" | "atomic")) => {
                    out.push(s.find(y.line, format!("{}::sync は Arc と atomic だけ", x.text)));
                }
                "sync" if third == Some("atomic") && s.rel != "cancel.rs" => {
                    out.push(s.find(y.line, "原子の型は cancel.rs の中止の印だけ（大域の状態を作らない）".into()));
                }
                "collections" if !(s.in_mem() && third == Some("TryReserveError")) => {
                    out.push(s.find(y.line, format!("{}::collections は mem の TryReserveError だけ", x.text)));
                }
                "vec" | "string" | "boxed" if !s.in_mem() => {
                    out.push(s.find(y.line, format!("{}::{name} は mem の中だけ（確保は mem の口だけ）", x.text)));
                }
                _ => {}
            }
        }
    }
    out
}

/// `mul_add`・`algebraic_*`・`unsafe`・大域の状態・原子の型。
fn check_forbidden_idents(s: &Src) -> Vec<Finding> {
    let mut out = Vec::new();
    for x in s.tokens.iter().filter(|x| x.kind == Kind::Ident) {
        let n = x.text.as_str();
        if n == "mul_add" || n.starts_with("algebraic_") || n == "fma" || n.starts_with("fmaf") {
            out.push(s.find(x.line, format!("{n} は使わない（determinism.md §2.3）")));
        }
        if n == "unsafe" {
            out.push(s.find(x.line, "unsafe は使わない（forbid）".into()));
        }
        if GLOBAL_STATE.contains(&n) {
            out.push(s.find(x.line, format!("{n} は大域の状態を作る（値が呼び方に依る）")));
        }
        if n.starts_with("Atomic") && s.rel != "cancel.rs" {
            out.push(s.find(x.line, format!("{n} は cancel.rs の中止の印だけ")));
        }
    }
    out
}

/// mem の外での確保。
fn check_alloc(s: &Src) -> Vec<Finding> {
    if s.in_mem() {
        return Vec::new();
    }
    let t = &s.tokens;
    let mut out = Vec::new();
    for k in 0..t.len() {
        let x = &t[k];
        if x.kind != Kind::Ident {
            continue;
        }
        let n = x.text.as_str();
        if ALLOC_TYPES.contains(&n) {
            out.push(s.find(x.line, format!("{n} は mem の外で使わない（mem の入れ物を使う。ADR-31）")));
        }
        if (n == "vec" || n == "format") && punct_at(t, k + 1, '!') {
            out.push(s.find(x.line, format!("{n}! は確保する（ADR-31）")));
        }
        if ALLOC_METHODS.contains(&n)
            && k >= 1
            && punct_at(t, k - 1, '.')
            && (punct_at(t, k + 1, '(') || path_sep(t, k + 1))
        {
            out.push(s.find(x.line, format!(".{n}() は std の中で確保する（ADR-31）")));
        }
        if n == "Arc" && path_sep(t, k + 1) {
            out.push(s.find(x.line, "Arc::… は mem の外で使わない（共有は mem::share）".into()));
        }
    }
    out
}

/// 厳しい lint の allow・expect は mem の中だけで、理由つき。
fn check_allows(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    let mut out = Vec::new();
    for k in 0..t.len() {
        if !(t[k].is_ident("allow") || t[k].is_ident("expect")) || !punct_at(t, k + 1, '(') {
            continue;
        }
        // 属性の中か（#[ か #![ の直後）
        let in_attr = k >= 2 && punct_at(t, k - 1, '[') && (punct_at(t, k - 2, '#') || punct_at(t, k - 2, '!'));
        if !in_attr {
            continue;
        }
        let close = sumie_testkit::rustscan::matching(t, k + 1).unwrap_or(k + 1);
        let inner = &t[k + 2..close];
        let strict: Vec<&str> = inner
            .windows(4)
            .filter(|w| w[0].is_ident("clippy") && w[1].is_punct(':') && w[2].is_punct(':'))
            .map(|w| w[3].text.as_str())
            .filter(|l| STRICT_LINTS.contains(l))
            .collect();
        if strict.is_empty() {
            continue;
        }
        let inner_attr = punct_at(t, k - 2, '!');
        if !s.in_mem() || inner_attr && s.rel == "lib.rs" {
            out.push(s.find(t[k].line, format!("厳しい lint {strict:?} の allow・expect は mem の中だけ")));
        } else if !inner.iter().any(|x| x.is_ident("reason")) {
            out.push(s.find(t[k].line, format!("{strict:?} を許すときは reason = \"…\" を書く")));
        }
    }
    out
}

/// `Plain` を実装した型は `#[repr(C)]`（列挙は整数の repr も可）か基本の型。
fn check_plain(all: &[Src]) -> Vec<Finding> {
    let mut out = Vec::new();
    // 型の名前 → 定義の属性に repr(C) があるか
    let mut defs: Vec<(String, bool, String, u32)> = Vec::new();
    for s in all {
        let t = &s.tokens;
        for k in 0..t.len() {
            if (t[k].is_ident("struct") || t[k].is_ident("enum") || t[k].is_ident("union"))
                && t.get(k + 1).is_some_and(|n| n.kind == Kind::Ident)
            {
                // 定義の前の属性を、前の ; か } まで見る
                let mut j = k;
                let mut repr_ok = false;
                while j > 0 {
                    j -= 1;
                    if t[j].is_punct(';') || t[j].is_punct('}') || t[j].is_punct('{') {
                        break;
                    }
                    if t[j].is_ident("repr") && punct_at(t, j + 1, '(') {
                        let r = t.get(j + 2).map(|r| r.text.as_str()).unwrap_or("");
                        repr_ok = r == "C" || (t[k].is_ident("enum") && PRIMS.contains(&r));
                    }
                }
                defs.push((t[k + 1].text.clone(), repr_ok, s.rel.clone(), t[k].line));
            }
        }
    }
    for s in all {
        let t = &s.tokens;
        for k in 0..t.len() {
            // impl Plain for X
            if t[k].is_ident("impl")
                && ident_at(t, k + 1, "Plain")
                && ident_at(t, k + 2, "for")
                && let Some(x) = t.get(k + 3)
            {
                check_plain_type(s, x, &defs, &mut out);
            }
            // plain!(A, B, …)
            if t[k].is_ident("plain") && punct_at(t, k + 1, '!') && punct_at(t, k + 2, '(') {
                let close = sumie_testkit::rustscan::matching(t, k + 2).unwrap_or(k + 2);
                for x in t[k + 3..close].iter().filter(|x| x.kind == Kind::Ident) {
                    check_plain_type(s, x, &defs, &mut out);
                }
            }
        }
    }
    out
}

fn check_plain_type(s: &Src, x: &Token, defs: &[(String, bool, String, u32)], out: &mut Vec<Finding>) {
    if PRIMS.contains(&x.text.as_str()) {
        return;
    }
    match defs.iter().find(|d| d.0 == x.text) {
        Some((_, true, _, _)) => {}
        Some((_, false, file, line)) => out.push(s.find(
            x.line,
            format!(
                "{} は Plain だが #[repr(C)] でない（{file}:{line}。大きさが Rust の版で変わりうる。ADR-39 (3)）",
                x.text
            ),
        )),
        None => out.push(s.find(x.line, format!("{} は Plain だが src に定義がない", x.text))),
    }
}

/// モジュールの層の向き。
fn check_layers(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    let mut out = Vec::new();
    let Some(&(_, my_layer, my_order)) = LAYERS.iter().find(|l| l.0 == s.module) else {
        return vec![
            s.find(1, format!("モジュール {} が rules.rs の LAYERS の表にない（architecture.md §2）", s.module)),
        ];
    };
    for k in 0..t.len() {
        if !t[k].is_ident("crate") || !path_sep(t, k + 1) {
            continue;
        }
        let mut targets: Vec<&Token> = Vec::new();
        if punct_at(t, k + 3, '{') {
            let close = sumie_testkit::rustscan::matching(t, k + 3).unwrap_or(k + 3);
            let mut depth = 0;
            for j in k + 3..close {
                if t[j].is_punct('{') {
                    depth += 1;
                } else if t[j].is_punct('}') {
                    depth -= 1;
                } else if depth == 1 && t[j].kind == Kind::Ident && (punct_at(t, j - 1, '{') || punct_at(t, j - 1, ','))
                {
                    targets.push(&t[j]);
                }
            }
        } else if let Some(x) = t.get(k + 3).filter(|x| x.kind == Kind::Ident) {
            targets.push(x);
        }
        for x in targets {
            let Some(&(_, layer, order)) = LAYERS.iter().find(|l| l.0 == x.text) else {
                // crate の根の定数など（RENDERER）は lib の層
                if x.text.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && my_layer == 6 {
                    continue;
                }
                if x.text.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                    out.push(s.find(x.line, format!("crate::{} は根（層 6）を使う", x.text)));
                }
                continue;
            };
            let up = layer > my_layer || (layer == my_layer && my_layer != 0 && order > my_order);
            if up {
                out.push(s.find(
                    x.line,
                    format!("{}（層 {my_layer}）が上の {}（層 {layer}）を使う（architecture.md §2）", s.module, x.text),
                ));
            }
        }
    }
    out
}

/// 自分の名前を呼ぶ関数（再帰しない。明示の積み上げで書く）。
fn check_recursion(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    let mut out = Vec::new();
    for k in 0..t.len() {
        if !t[k].is_ident("fn") {
            continue;
        }
        let Some(name) = t.get(k + 1).filter(|n| n.kind == Kind::Ident) else { continue };
        // 本体の開き（; が先なら本体なし）
        let mut j = k + 2;
        let mut depth = 0i32;
        let open = loop {
            let Some(x) = t.get(j) else { break None };
            if x.is_punct('(') || x.is_punct('[') || x.is_punct('<') {
                depth += 1;
            } else if x.is_punct(')') || x.is_punct(']') || (x.is_punct('>') && !punct_at(t, j - 1, '-')) {
                depth -= 1;
            } else if depth <= 0 && x.is_punct(';') {
                break None;
            } else if depth <= 0 && x.is_punct('{') {
                break Some(j);
            }
            j += 1;
        };
        let Some(open) = open else { continue };
        let close = sumie_testkit::rustscan::matching(t, open).unwrap_or(open);
        for m in open + 1..close {
            if !t[m].is_ident(&name.text) || !punct_at(t, m + 1, '(') {
                continue;
            }
            let prev_dot = punct_at(t, m - 1, '.');
            let prev_path = m >= 2 && punct_at(t, m - 1, ':') && punct_at(t, m - 2, ':');
            let self_call = prev_dot && ident_at(t, m - 2, "self") && !punct_at(t, m - 3, '.');
            // Self::名前( と self::名前( は自分、ほかのパス（libm::sin( など）は別のもの
            let path_call = prev_path && (ident_at(t, m - 3, "Self") || ident_at(t, m - 3, "self"));
            let free_call = !prev_dot && !prev_path && !ident_at(t, m - 1, "fn");
            if self_call || path_call || free_call {
                out.push(s.find(t[m].line, format!("{} が自分を呼ぶ（再帰しない。明示の積み上げで書く）", name.text)));
            }
        }
    }
    out
}

fn check_all(all: &[Src]) -> Vec<Finding> {
    let mut out = Vec::new();
    for s in all {
        out.extend(check_cfg(s));
        out.extend(check_std(s));
        out.extend(check_forbidden_idents(s));
        out.extend(check_alloc(s));
        out.extend(check_allows(s));
        out.extend(check_layers(s));
        out.extend(check_recursion(s));
    }
    out.extend(check_plain(all));
    out
}

fn report(f: &[Finding]) -> String {
    f.iter().map(|f| format!("  src/{}:{}：{}", f.file, f.line, f.what)).collect::<Vec<_>>().join("\n")
}

#[test]
fn src_follows_the_rules() {
    let all = sources();
    assert!(all.iter().any(|s| s.rel == "lib.rs"), "src/lib.rs が見つからない");
    let f = check_all(&all);
    assert!(f.is_empty(), "src の規則に反する所：\n{}", report(&f));
}

#[test]
fn library_has_no_features_and_no_build_script() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let toml = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    assert!(
        !toml.lines().any(|l| l.trim_start().starts_with("[features]")),
        "ライブラリは [features] を持たない（ビルドで振る舞いを分けない）"
    );
    assert!(!toml.contains("build ="), "ライブラリは build.rs を持たない");
    assert!(!dir.join("build.rs").exists(), "ライブラリは build.rs を持たない");
    // 本番の依存は libm だけ（ADR-02。cargo tree での確かめは scripts/verify.sh）
    let deps = toml.split("[dependencies]").nth(1).unwrap_or("").split("\n[").next().unwrap_or("");
    let names: Vec<&str> = deps
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split('=').next().map(str::trim))
        .collect();
    assert_eq!(names, ["libm"], "本番の依存は libm だけ");
}

// ---- 規則の自己試験（拾うこと・拾わないこと）----

fn one(rel: &str, text: &str) -> Vec<Finding> {
    check_all(&[Src::new(rel, text)])
}

fn hits(rel: &str, text: &str) -> usize {
    one(rel, text).len()
}

#[test]
fn rule_cfg() {
    assert_eq!(hits("math.rs", "#[cfg(test)]\nmod tests;"), 0);
    assert_eq!(hits("math.rs", "#[cfg(target_arch = \"x86_64\")] fn f() {}"), 1);
    assert_eq!(hits("math.rs", "#[cfg(debug_assertions)] fn f() {}"), 1);
    assert_eq!(hits("math.rs", "fn f() -> bool { cfg!(debug_assertions) }"), 1);
    assert_eq!(hits("math.rs", "#[cfg_attr(test, derive(Debug))] struct A;"), 1);
    assert_eq!(hits("math.rs", "#[cfg(test)] mod tests { }"), 1);
    assert_eq!(hits("math.rs", "fn f() -> bool { is_x86_feature_detected!(\"avx2\") }"), 1);
    // 注釈と文字列の中は数えない
    assert_eq!(hits("math.rs", "// #[cfg(target_os = \"macos\")]\nconst S: &str = \"cfg(test)\";"), 0);
}

#[test]
fn rule_std_modules() {
    assert_eq!(hits("math.rs", "use std::cmp::Ordering; use core::fmt;"), 0);
    assert_eq!(hits("budget.rs", "use std::cell::Cell;"), 0);
    assert_eq!(hits("math.rs", "use std::time::Instant;"), 1);
    assert_eq!(hits("math.rs", "fn f() { let _ = std::env::var(\"A\"); }"), 1);
    assert_eq!(hits("math.rs", "use std::{fs, io};"), 2);
    assert_eq!(hits("math.rs", "use core::arch::x86_64::*;"), 1);
    assert_eq!(hits("math.rs", "use std::thread;"), 1);
    assert_eq!(hits("math.rs", "use std::sync::Arc;"), 0);
    assert_eq!(hits("math.rs", "use std::sync::Mutex;"), 2); // sync の表と、大域の状態
    assert_eq!(hits("cancel.rs", "use std::sync::atomic::{AtomicBool, Ordering};"), 0);
    assert_eq!(hits("math.rs", "use std::sync::atomic::AtomicU64;"), 2);
    assert_eq!(hits("mem/mod.rs", "use std::collections::TryReserveError; use std::vec::Vec;"), 0);
    assert_eq!(hits("math.rs", "use std::collections::BTreeMap;"), 2); // collections と、確保する型
    assert_eq!(hits("math.rs", "use std::vec::Vec;"), 2);
}

#[test]
fn rule_forbidden_idents() {
    assert_eq!(hits("math.rs", "fn f(a: f64) -> f64 { a.mul_add(1.0, 2.0) }"), 1);
    assert_eq!(hits("math.rs", "fn f(a: f64) -> f64 { a.algebraic_add(1.0) }"), 1);
    assert_eq!(hits("math.rs", "unsafe fn f() {}"), 1);
    assert_eq!(hits("math.rs", "fn f(a: f64) -> f64 { a * 2.0 + 1.0 }"), 0);
}

#[test]
fn rule_alloc_outside_mem() {
    assert_eq!(hits("doc.rs", "fn f() { let v: Vec<u8> = Vec::new(); }"), 2);
    assert_eq!(hits("doc.rs", "fn f() { let _ = vec![1]; let _ = format!(\"x\"); }"), 2);
    assert_eq!(hits("doc.rs", "fn f(a: &[u8]) { let _ = a.to_vec(); let _ = a.iter().collect::<X>(); }"), 2);
    assert_eq!(hits("doc.rs", "fn f(a: &X) { let _ = a.clone(); }"), 1);
    assert_eq!(hits("doc.rs", "fn f(a: &Arc<u8>) { let _ = Arc::clone(a); }"), 1);
    assert_eq!(hits("doc.rs", "fn f(a: &Arc<u8>) -> usize { a.len() }"), 0);
    assert_eq!(hits("mem/mod.rs", "fn f() { let v: Vec<u8> = Vec::new(); let _ = v.clone(); }"), 0);
    // 自分の型の名前の一部は拾わない
    assert_eq!(hits("doc.rs", "struct MVecLike; fn f(x: &MVecLike) {}"), 0);
}

#[test]
fn rule_allows() {
    assert_eq!(hits("doc.rs", "#[allow(clippy::indexing_slicing)] fn f() {}"), 1);
    assert_eq!(hits("doc.rs", "#[allow(clippy::too_many_arguments)] fn f() {}"), 0);
    assert_eq!(hits("mem/mod.rs", "#[allow(clippy::disallowed_methods)] fn f() {}"), 1);
    assert_eq!(hits("mem/mod.rs", "#[expect(clippy::disallowed_methods, reason = \"包み\")] fn f() {}"), 0);
    assert_eq!(hits("lib.rs", "#![allow(clippy::panic)]"), 1);
}

#[test]
fn rule_plain() {
    let ok = "#[repr(C)]\n#[derive(Clone, Copy)]\nstruct A { x: u32 }\nimpl Plain for A {}\nplain!(u8, u32);";
    assert_eq!(hits("mem/mod.rs", ok), 0);
    let bad = "#[derive(Clone, Copy)]\nstruct B { x: u32 }\nimpl Plain for B {}";
    assert_eq!(hits("mem/mod.rs", bad), 1);
    let en = "#[repr(u8)]\n#[derive(Clone, Copy)]\nenum E { A, B }\nplain!(E);";
    assert_eq!(hits("mem/mod.rs", en), 0);
    assert_eq!(hits("mem/mod.rs", "plain!(Missing);"), 1);
}

#[test]
fn rule_layers() {
    assert_eq!(hits("mem/mod.rs", "use crate::budget::Budget; use crate::error::Error;"), 0);
    assert_eq!(hits("math.rs", "use crate::geom::Path;"), 1);
    assert_eq!(hits("xml.rs", "use crate::css::Rule;"), 1);
    assert_eq!(hits("css.rs", "use crate::xml::Node;"), 0);
    assert_eq!(hits("css.rs", "use crate::{xml::Node, doc::Document};"), 1);
    assert_eq!(hits("lib.rs", "pub use crate::error::Error;"), 0);
    assert_eq!(hits("unknown.rs", "fn f() {}"), 1);
}

#[test]
fn rule_recursion() {
    assert_eq!(hits("math.rs", "fn f(n: u32) -> u32 { if n == 0 { 0 } else { f(n - 1) } }"), 1);
    assert_eq!(hits("math.rs", "impl A { fn g(&self) -> u32 { self.g() } }"), 1);
    assert_eq!(hits("math.rs", "impl A { fn g(&self) -> u32 { Self::g(self) } }"), 1);
    assert_eq!(hits("math.rs", "impl A { fn len(&self) -> usize { self.v.len() } }"), 0);
    assert_eq!(hits("math.rs", "fn f(n: u32) -> u32 { let g = |x: u32| x; g(n) }"), 0);
    assert_eq!(hits("math.rs", "trait T { fn f(&self); }"), 0);
    assert_eq!(hits("math.rs", "pub fn sin(x: f64) -> f64 { libm::sin(x) }"), 0);
}
