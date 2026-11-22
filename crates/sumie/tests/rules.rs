//! ライブラリの src の走査（determinism.md §9・CLAUDE.md）。clippy で拾えない規則を字句で見張る。
//!
//! - ビルドで分かれるもの：属性の `cfg(…)`・`cfg_attr`・`cfg!`（`#[cfg(test)] mod tests;` だけ許す）・`is_*_feature_detected!`・
//!   `#[path]`・`include!`・`env!`・`option_env!`・属性の外の `cfg(`・`debug_assertions`・`target_*`（マクロの引数で属性を
//!   作る形）・Cargo.toml の `[features]`・build.rs・ターゲットごとの依存
//! - std・core・alloc は表にあるモジュールだけ（絶対のパス `::std::…`・別名 `use core as c`・`extern crate` も見る）。
//!   `libm::` は math.rs だけ
//! - `mul_add`・`algebraic_*`・`unsafe`・大域の状態（`OnceLock`・`Mutex` など。`static` の型は基本の型・Plain の型・その配列と
//!   参照だけ）・順序の決まらない入れ物（mem の中も）・ポインタの値（`as_ptr`・`addr`・`*const`）・`partial_cmp`
//! - ずらし量が数の字句か定数でない `<<`・`>>`（`=` つきも。`arithmetic_side_effects` が拾わない）
//! - `mem` の外での確保（`Vec`・`String`・`Box`・`Rc`・`Arc`・`vec!`・`format!`・`.to_vec()`・`.collect()`・`.clone()`・`.cloned()` など）。
//!   mem の中でも `Vec::from`・`Default::default`・`.into()` のような数えない確保の変換
//! - lint の群の allow（どこでも）と、厳しい lint の allow・expect（mem の中だけ、理由つき、口ごと。内側の属性と、`mod`・
//!   `impl`・`trait` に付けた外側の属性はモジュールごとの外しとみなす）。マクロの引数で lint の属性を渡す形も
//! - `MVec`・`Frozen` に入れる型（`Plain`）は `#[repr(C)]`（欄も Plain）か基本の型。`Plain` の別名と、`plain!` 以外の
//!   マクロでの実装は作らない
//! - モジュールの層の向き（architecture.md §2。`crate::`・`super::`・`self::` を根から解く。`{…}` の中の `super` も）
//! - 自分の名前を呼ぶ関数（再帰）。受け手・パス・turbofish・トレイトの名前（`Walk::walk`・`<Self as Walk>::walk`）によらず
//!   拾い、関数を値として渡す形（`map(depth)`・`map(Self::depth)`・`let g = f;`）も拾う。`self.欄.名前(` の委ねと、理由を
//!   書いた表だけ許す。互いを呼ぶ輪も
//!
//! 試験のコード（`src/**/tests.rs`。`#[cfg(test)] mod tests;` でだけ入れられる）は走査しない。各規則は下の自己試験で、
//! 拾うことと拾わないことを確かめる。

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

use sumie_testkit::rustscan::{Kind, Token, lex, matching};

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

/// std・core・alloc の 2 つ目のパスで使ってよいもの。`sync`・`collections`・`vec`・`string`・`boxed` は下の特別の規則で見る。
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

/// 厳しい lint（allow・expect は mem の中だけ。`disallowed_types` は mem の中でも許さない）。
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

/// clippy の lint の群（allow すると中の厳しい lint も外れるので、どこでも許さない）。
const LINT_GROUPS: &[&str] =
    &["all", "restriction", "style", "pedantic", "nursery", "correctness", "suspicious", "complexity", "perf", "cargo"];

/// rustc の lint のうち、allow してはいけないもの。
/// `deprecated` は、非推奨の口（`str::trim_left` など）を通って禁じる口の一覧をすり抜けないように。
const RUSTC_NEVER: &[&str] =
    &["warnings", "unconditional_recursion", "unsafe_code", "arithmetic_overflow", "unused", "deprecated"];

/// mem の外で allow・expect してよい clippy の lint（ふつうの書き方の都合のもの）。
const CLIPPY_OK_ELSEWHERE: &[&str] =
    &["too_many_arguments", "type_complexity", "too_many_lines", "new_without_default"];

/// mem の外で使わない型の名前（確保する入れ物と共有の数え）。
const ALLOC_TYPES: &[&str] = &["Vec", "String", "Box", "Rc", "Arc", "CString", "Cow"];

/// どこでも（mem の中も）使わない型の名前（順序が決まらない・std の中で数えずに確保する入れ物）。
const ORDER_TYPES: &[&str] = &[
    "HashMap",
    "HashSet",
    "RandomState",
    "DefaultHasher",
    "BTreeMap",
    "BTreeSet",
    "BinaryHeap",
    "VecDeque",
    "LinkedList",
];

/// mem の外で呼ばない口の名前（`.名前(` か `.名前::<` の形）。
const ALLOC_METHODS: &[&str] = &[
    "to_vec",
    "to_owned",
    "to_string",
    "collect",
    "clone",
    "cloned",
    "extend",
    "extend_from_slice",
    "into_boxed_slice",
    "into_vec",
    "concat",
    "join",
    "repeat",
    "resize",
    "to_ascii_lowercase",
    "to_ascii_uppercase",
];

/// 大域の状態を作る名前。
const GLOBAL_STATE: &[&str] =
    &["OnceLock", "OnceCell", "LazyLock", "LazyCell", "Mutex", "RwLock", "thread_local", "Condvar"];

/// 組むときの設定の名前（属性の外に出るのは、マクロの引数で `cfg` を作る形）。
const CFG_KEYS: &[&str] = &[
    "debug_assertions",
    "overflow_checks",
    "target_os",
    "target_arch",
    "target_feature",
    "target_pointer_width",
    "target_endian",
    "target_env",
    "target_family",
    "target_vendor",
    "target_has_atomic",
];

/// ポインタの値を作る口（determinism.md §4.1：ポインタの値で順序や鍵を決めない）。
const POINTER_METHODS: &[&str] = &["as_ptr", "as_mut_ptr", "addr", "expose_provenance", "to_bits_ptr"];

/// 基本の型（`Plain` を実装してよい）。
const PRIMS: &[&str] = &[
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64", "bool",
    "char",
];

/// `Plain` にしてよい基本の型（大きさがターゲットで変わる usize・isize と、整列が版で変わった u128・i128 は入れない）。
const PLAIN_PRIMS: &[&str] = &["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "bool", "char"];

/// 再帰の走査の例外（ファイル, 関数, 理由）。同じ名前の別の型の口を呼ぶ所だけ。理由を書いて足す（当たらない例外は誤り）。
const RECURSION_OK: &[(&str, &str, &str)] = &[];

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
    /// モジュールのパス（`mem/sorted.rs` → [mem, sorted]・`mem/mod.rs` → [mem]・lib.rs → []）
    path: Vec<String>,
    /// 先頭のモジュール（lib.rs は `lib`）
    module: String,
    tokens: Vec<Token>,
    /// 属性の範囲（`#` の位置・`[` の位置・`]` の位置・内側の属性か）
    attrs: Vec<(usize, usize, usize, bool)>,
}

impl Src {
    fn new(rel: &str, text: &str) -> Src {
        let mut path: Vec<String> = rel.trim_end_matches(".rs").split('/').map(str::to_string).collect();
        if path.last().is_some_and(|l| l == "mod" || l == "lib") {
            path.pop();
        }
        let module = path.first().cloned().unwrap_or_else(|| "lib".to_string());
        let tokens = lex(text);
        let attrs = attributes(&tokens);
        Src { rel: rel.to_string(), path, module, tokens, attrs }
    }

    fn find(&self, line: u32, what: String) -> Finding {
        Finding { file: self.rel.clone(), line, what }
    }

    fn in_mem(&self) -> bool {
        self.module == "mem"
    }

    /// `k` が属性の中か（その属性を返す）。
    fn attr_of(&self, k: usize) -> Option<(usize, usize, usize, bool)> {
        self.attrs.iter().copied().find(|&(_, o, c, _)| o < k && k < c)
    }
}

/// 属性の範囲（`#[…]`・`#![…]`）。
fn attributes(t: &[Token]) -> Vec<(usize, usize, usize, bool)> {
    let mut out = Vec::new();
    let mut k = 0;
    while k < t.len() {
        if t[k].is_punct('#') {
            let inner = punct_at(t, k + 1, '!');
            let open = if inner { k + 2 } else { k + 1 };
            if punct_at(t, open, '[')
                && let Some(close) = matching(t, open)
            {
                out.push((k, open, close, inner));
                k = close;
            }
        }
        k += 1;
    }
    out
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

/// `t[k]` の直前が `::` か。
fn after_sep(t: &[Token], k: usize) -> bool {
    k >= 2 && punct_at(t, k - 1, ':') && punct_at(t, k - 2, ':')
}

/// パスの前に来うるキーワード（`use ::std::…` の `use` は前のパスの欄でない）。
const KEYWORDS: &[&str] = &[
    "use", "pub", "as", "in", "return", "let", "if", "match", "mut", "const", "static", "type", "where", "impl", "for",
    "dyn", "else", "move", "ref", "while", "break",
];

/// `t[k]` がパスの根か（直前が `::` でないか、`::` の前に欄の識別子・`>` がない＝先頭の `::`）。
fn is_path_root(t: &[Token], k: usize) -> bool {
    if !after_sep(t, k) {
        return true;
    }
    let Some(prev) = k.checked_sub(3).and_then(|i| t.get(i)) else { return true };
    let segment = (prev.kind == Kind::Ident && !KEYWORDS.contains(&prev.text.as_str())) || prev.is_punct('>');
    !segment
}

/// `{a, b::c, self as d}` の 1 段目の識別子（`open` は `{`）。
fn group_heads(t: &[Token], open: usize) -> Vec<usize> {
    let close = matching(t, open).unwrap_or(open);
    let mut depth = 0i32;
    let mut out = Vec::new();
    for j in open..close {
        if t[j].is_punct('{') {
            depth += 1;
        } else if t[j].is_punct('}') {
            depth -= 1;
        } else if depth == 1 && (punct_at(t, j - 1, '{') || punct_at(t, j - 1, ',')) {
            out.push(j);
        }
    }
    out
}

// ---- 規則 ----

/// ビルドで分かれるもの・走査の外からコードを入れるもの。
fn check_cfg(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    let mut out = Vec::new();
    // 属性
    for &(hash, open, close, _) in &s.attrs {
        let Some(head) = t.get(open + 1) else { continue };
        let inner = &t[open + 1..close];
        if head.is_ident("cfg") {
            // #[cfg(test)] mod tests;
            let ok = inner.len() == 4
                && ident_at(inner, 2, "test")
                && ident_at(t, close + 1, "mod")
                && ident_at(t, close + 2, "tests")
                && punct_at(t, close + 3, ';');
            if !ok {
                out.push(s.find(
                    t[hash].line,
                    "cfg はビルドで振る舞いを分ける。許すのは `#[cfg(test)] mod tests;` だけ（determinism.md §1.2・§7）".into(),
                ));
            }
        } else if head.is_ident("cfg_attr") {
            out.push(s.find(t[hash].line, "cfg_attr はビルドで振る舞いを分ける（determinism.md §7）".into()));
        } else if head.is_ident("path") {
            out.push(s.find(t[hash].line, "#[path] は src の外のコードを入れられる（走査の外になる）".into()));
        }
    }
    for k in 0..t.len() {
        let x = &t[k];
        if x.kind != Kind::Ident || s.attr_of(k).is_some() {
            continue;
        }
        let bang = punct_at(t, k + 1, '!');
        match x.text.as_str() {
            "is_x86_feature_detected"
            | "is_aarch64_feature_detected"
            | "is_arm_feature_detected"
            | "is_riscv_feature_detected" => {
                out.push(s.find(x.line, format!("{} はターゲットで処理を分ける（determinism.md §7）", x.text)));
            }
            "cfg" if bang => out.push(s.find(x.line, "cfg! はビルドで振る舞いを分ける（determinism.md §7）".into())),
            "include" if bang => out.push(s.find(x.line, "include! は src の外のコードを入れられる".into())),
            "env" | "option_env" if bang => {
                out.push(s.find(x.line, format!("{}! は組むときの環境変数を読む（determinism.md §5）", x.text)));
            }
            "cfg" | "cfg_attr" if punct_at(t, k + 1, '(') => out.push(
                s.find(x.line, format!("{}(…) を属性の外に書かない（マクロの引数で属性を作ると走査が見ない）", x.text)),
            ),
            n if CFG_KEYS.contains(&n) => out.push(s.find(
                x.line,
                format!("{n} は組むときの設定の名前（属性の外に出るのはマクロで cfg を作る形。determinism.md §7）"),
            )),
            "mod" if ident_at(t, k + 1, "tests") => {
                // #[cfg(test)] の直後か
                let ok = k >= 1 && s.attrs.iter().any(|&(_, o, c, _)| c + 1 == k && ident_at(t, o + 1, "cfg"));
                if !ok {
                    out.push(
                        s.find(x.line, "mod tests は `#[cfg(test)] mod tests;` の形だけ（走査から外れる）".into()),
                    );
                }
            }
            _ => {}
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
        if x.is_ident("extern") && ident_at(t, k + 1, "crate") {
            out.push(s.find(x.line, "extern crate は使わない（別名で表をすり抜けられる）".into()));
            continue;
        }
        if !(x.is_ident("std") || x.is_ident("core") || x.is_ident("alloc")) || !is_path_root(t, k) {
            continue;
        }
        // extern crate std は上で拾った
        if k >= 1 && ident_at(t, k - 1, "crate") {
            continue;
        }
        if s.attr_of(k).is_some() {
            continue;
        }
        if ident_at(t, k + 1, "as") {
            out.push(s.find(x.line, format!("{} の別名は使わない（表をすり抜けられる）", x.text)));
            continue;
        }
        if !path_sep(t, k + 1) {
            // `use std;` など、パスでない使い方
            if ident_at(t, k.wrapping_sub(1), "use") || punct_at(t, k + 1, ';') {
                out.push(s.find(x.line, format!("{} をそのまま入れない（モジュールを名指す）", x.text)));
            }
            continue;
        }
        // 2 つ目（{a, b} の並びなら 1 段目のすべて）
        let seconds: Vec<usize> = if punct_at(t, k + 3, '{') {
            group_heads(t, k + 3)
        } else if t.get(k + 3).is_some_and(|y| y.kind == Kind::Ident || y.is_punct('*')) {
            vec![k + 3]
        } else {
            Vec::new()
        };
        for j in seconds {
            let y = &t[j];
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
                "sync" if third == Some("Arc") && !s.in_mem() => {
                    out.push(s.find(y.line, "Arc は mem の中だけ（共有は mem の包み）".into()));
                }
                "collections" if !(s.in_mem() && third == Some("TryReserveError")) => {
                    out.push(s.find(y.line, format!("{}::collections は mem の TryReserveError だけ", x.text)));
                }
                "vec" | "string" | "boxed" if !s.in_mem() => {
                    out.push(s.find(y.line, format!("{}::{name} は mem の中だけ（確保は mem の口だけ）", x.text)));
                }
                "fmt" if third == Some("format") => {
                    out.push(s.find(y.line, format!("{}::fmt::format は確保する", x.text)));
                }
                _ => {}
            }
        }
    }
    out
}

/// `mul_add`・`algebraic_*`・`unsafe`・大域の状態・原子の型・順序の決まらない入れ物・内側で変わる `static`。
fn check_forbidden_idents(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    let mut out = Vec::new();
    for k in 0..t.len() {
        let x = &t[k];
        if x.kind != Kind::Ident {
            continue;
        }
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
        if ORDER_TYPES.contains(&n) {
            out.push(
                s.find(x.line, format!("{n} は使わない（順序が決まらないか、std の中で数えずに確保する。mem の中も）")),
            );
        }
        if n == "static" && ident_at(t, k + 1, "mut") {
            out.push(s.find(x.line, "static mut は使わない".into()));
        }
        let method = k >= 1 && punct_at(t, k - 1, '.') && punct_at(t, k + 1, '(');
        if (method && POINTER_METHODS.contains(&n)) || n == "addr_of" || n == "addr_of_mut" {
            out.push(s.find(x.line, format!("{n} はポインタの値を作る（順序や鍵に使わない。determinism.md §4.1）")));
        }
        if (n == "const" || n == "mut") && k >= 1 && punct_at(t, k - 1, '*') && t[k - 1].joint(x) {
            out.push(s.find(x.line, format!("*{n} は生のポインタ（ポインタの値を使わない。determinism.md §4.1）")));
        }
        if n == "libm" && s.rel != "math.rs" {
            out.push(s.find(x.line, "libm は crate::math を通して使う（±0・NaN の扱いを揃える。CLAUDE.md）".into()));
        }
        if n == "partial_cmp" && punct_at(t, k + 1, '(') && !(k >= 1 && ident_at(t, k - 1, "fn")) {
            out.push(s.find(
                x.line,
                "partial_cmp は全順序でない（total_cmp か整数の鍵で比べる。determinism.md §4.1）".into(),
            ));
        }
        if n == "Plain" && ident_at(t, k + 1, "as") {
            out.push(s.find(x.line, "Plain の別名を作らない（Plain の見張りをすり抜ける）".into()));
        }
    }
    out
}

/// `t[k]` と `t[k + 1]` が空白なしで接するか。
fn joint_at(t: &[Token], k: usize) -> bool {
    matches!((t.get(k), t.get(k + 1)), (Some(a), Some(b)) if a.joint(b))
}

/// ずらし量が数の字句か定数（大文字の名前）でない `<<`・`>>`・`<<=`・`>>=`。clippy の `arithmetic_side_effects` は
/// これを拾わず、桁の数以上のずらしは debug でだけ panic して release では折り返す（値がビルドで分かれる）。
/// 演算子は前後に空白がある（rustfmt の形）ものだけを見る：型の閉じ `Vec<Vec<u8>>` と `<<T as X>::Y` は前か後ろが接する。
fn check_shifts(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    let mut out = Vec::new();
    for k in 1..t.len() {
        let op = if t[k].is_punct('<') {
            '<'
        } else if t[k].is_punct('>') {
            '>'
        } else {
            continue;
        };
        if !punct_at(t, k + 1, op) || !joint_at(t, k) || joint_at(t, k - 1) || s.attr_of(k).is_some() {
            continue;
        }
        let mut last = k + 1;
        if punct_at(t, k + 2, '=') && joint_at(t, k + 1) {
            last = k + 2;
        }
        if joint_at(t, last) {
            continue;
        }
        let Some(x) = t.get(last + 1) else { continue };
        let number = x.kind == Kind::Literal && x.text.as_bytes().first().is_some_and(u8::is_ascii_digit);
        let constant = x.kind == Kind::Ident
            && x.text.len() >= 2
            && x.text.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_');
        // 数のあとに続く演算（`a << 2 + b` は `a << (2 + b)`）・呼び出し
        let more = t.get(last + 2).is_some_and(|y| {
            y.kind == Kind::Punct && ["+", "-", "*", "/", "%", ".", "(", "["].contains(&y.text.as_str())
        });
        if !(number || constant) || more {
            out.push(s.find(
                t[k].line,
                format!(
                    "ずらし量が数でない {op}{op} は、桁の数以上で debug だけ panic し release では折り返す（checked_shl・checked_shr を使う。determinism.md §3）"
                ),
            ));
        }
    }
    out
}

/// `static` の型は基本の型・`str`・Plain の型と、その配列・タプル・参照だけ（内側で変わる型を包んだものを入れない）。
fn check_statics(all: &[Src]) -> Vec<Finding> {
    let names: Vec<String> = plain_targets(all).into_iter().map(|(_, x)| x.text).collect();
    let mut out = Vec::new();
    for s in all {
        let t = &s.tokens;
        for k in 0..t.len() {
            if !t[k].is_ident("static") || s.attr_of(k).is_some() {
                continue;
            }
            // static [mut] 名前: 型 = …;
            let mut j = k + 1;
            if ident_at(t, j, "mut") {
                j += 1;
            }
            if !punct_at(t, j + 1, ':') {
                continue;
            }
            let mut depth = 0i32;
            let mut m = j + 2;
            while let Some(x) = t.get(m) {
                if x.is_punct('[') || x.is_punct('(') || x.is_punct('<') {
                    depth += 1;
                } else if x.is_punct(']') || x.is_punct(')') || x.is_punct('>') {
                    depth -= 1;
                } else if depth <= 0 && (x.is_punct('=') || x.is_punct(';')) {
                    break;
                }
                let ok = match x.kind {
                    Kind::Ident => {
                        let n = x.text.as_str();
                        PRIMS.contains(&n)
                            || n == "str"
                            || names.iter().any(|p| p == n)
                            || (n.len() >= 2
                                && n.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_'))
                    }
                    Kind::Lifetime => x.text == "static",
                    Kind::Literal => true,
                    Kind::Punct => ["&", "[", "]", ";", "(", ")", ","].contains(&x.text.as_str()),
                    _ => false,
                };
                if !ok {
                    out.push(s.find(
                        t[k].line,
                        format!("static の型に {} を入れない（基本の型・Plain の型・その配列と参照だけ。大域の状態を作らない）", x.text),
                    ));
                    break;
                }
                m += 1;
            }
        }
    }
    out
}

/// mem の外での確保。
fn check_alloc(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    if s.in_mem() {
        return check_alloc_in_mem(s);
    }
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
        if n == "format_args" && punct_at(t, k + 1, '!') {
            out.push(s.find(
                x.line,
                "format_args! を直に書かない（それ自体は確保しないが、fmt::format などの確保する口に渡せる。文字は Display の write! で書く）".into(),
            ));
        }
        let call = punct_at(t, k + 1, '(') || path_sep(t, k + 1);
        if ALLOC_METHODS.contains(&n) && k >= 1 && punct_at(t, k - 1, '.') && call {
            out.push(s.find(x.line, format!(".{n}() は std の中で確保する（ADR-31）")));
        }
    }
    out
}

/// mem の中で、clippy の一覧が拾わない確保の変換（`Vec::from`・`String::from`・`Box::from`・`Default::default`・`.into()`）。
/// mem の入れ物の口は `try_reserve_exact` で確保して数える。数えない変換は使わない。
fn check_alloc_in_mem(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    let mut out = Vec::new();
    for k in 0..t.len() {
        let x = &t[k];
        if x.kind != Kind::Ident {
            continue;
        }
        let n = x.text.as_str();
        let conv = path_sep(t, k + 1)
            && t.get(k + 3).is_some_and(|y| y.is_ident("from") || y.is_ident("default") || y.is_ident("from_iter"));
        if (ALLOC_TYPES.contains(&n) || n == "Default") && conv {
            out.push(s.find(x.line, format!("{n}::… は数えずに確保しうる（mem の口は try_reserve_exact で確保する）")));
        }
        if n == "into" && k >= 1 && punct_at(t, k - 1, '.') && punct_at(t, k + 1, '(') {
            out.push(
                s.find(x.line, ".into() は確保する変換になりうる（数の変換は u64::from のように型を名指す）".into()),
            );
        }
    }
    out
}

/// 属性（`close` は `]`）のあとの項目のキーワード（ほかの属性・文書の注釈・可視性・`unsafe` は飛ばす）。
fn item_after(t: &[Token], close: usize) -> Option<&str> {
    let mut j = close + 1;
    loop {
        let x = t.get(j)?;
        if x.is_punct('#') {
            let open = if punct_at(t, j + 1, '!') { j + 2 } else { j + 1 };
            j = matching(t, open)? + 1;
        } else if x.kind == Kind::Doc || x.is_ident("unsafe") || x.is_ident("default") {
            j += 1;
        } else if x.is_ident("pub") {
            j += 1;
            if punct_at(t, j, '(') {
                j = matching(t, j)? + 1;
            }
        } else {
            return Some(x.text.as_str());
        }
    }
}

/// lint の allow・expect・warn。
fn check_allows(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    let mut out = Vec::new();
    // マクロの引数で lint の属性を渡す形（`m!(allow(clippy::x))`）は下の走査が見ない
    for k in 0..t.len() {
        let x = &t[k];
        let lint_attr = ["allow", "expect", "warn", "deny", "forbid"].contains(&x.text.as_str());
        if x.kind == Kind::Ident
            && lint_attr
            && punct_at(t, k + 1, '(')
            && s.attr_of(k).is_none()
            && !(k >= 1 && (punct_at(t, k - 1, '.') || punct_at(t, k - 1, ':') || ident_at(t, k - 1, "fn")))
        {
            out.push(s.find(
                x.line,
                format!("{}(…) を属性の外に書かない（マクロの引数で lint を外すと走査が見ない）", x.text),
            ));
        }
    }
    for &(hash, open, close, inner_attr) in &s.attrs {
        let Some(head) = t.get(open + 1) else { continue };
        if !(head.is_ident("allow") || head.is_ident("expect") || head.is_ident("warn")) {
            continue;
        }
        // mod・impl・trait に付けた外側の属性は、中の口をまとめて外す（内側の属性と同じに扱う）
        let module_wide = inner_attr || matches!(item_after(t, close), Some("mod" | "impl" | "trait"));
        let body = &t[open + 1..close];
        let line = t[hash].line;
        let has_reason = body.iter().any(|x| x.is_ident("reason"));
        let mut j = 0;
        while j < body.len() {
            let x = &body[j];
            if x.is_ident("clippy") && path_sep(body, j + 1) {
                let Some(l) = body.get(j + 3) else { break };
                let name = l.text.as_str();
                if LINT_GROUPS.contains(&name) {
                    out.push(
                        s.find(line, format!("clippy::{name} の群を外すと厳しい lint も外れる（どこでも許さない）")),
                    );
                } else if STRICT_LINTS.contains(&name) {
                    if !s.in_mem() {
                        out.push(s.find(line, format!("厳しい lint clippy::{name} を外すのは mem の中だけ")));
                    } else if module_wide {
                        out.push(s.find(
                            line,
                            format!("clippy::{name} をモジュール・impl・トレイトごと外さない（口ごとに expect で）"),
                        ));
                    } else if name == "disallowed_types" {
                        out.push(s.find(line, "disallowed_types は mem の中でも外さない".into()));
                    } else if !has_reason {
                        out.push(s.find(line, format!("clippy::{name} を外すときは reason = \"…\" を書く")));
                    }
                } else if !s.in_mem() && !CLIPPY_OK_ELSEWHERE.contains(&name) {
                    out.push(s.find(line, format!("clippy::{name} の allow は表（CLIPPY_OK_ELSEWHERE）にない")));
                }
                j += 4;
                continue;
            }
            if x.kind == Kind::Ident
                && RUSTC_NEVER.contains(&x.text.as_str())
                && !(j >= 1 && punct_at(body, j - 1, ':'))
            {
                out.push(s.find(line, format!("{} を外さない", x.text)));
            }
            j += 1;
        }
    }
    out
}

/// 型の定義（名前・repr が許すものか・欄の型の字句）。
struct Def {
    name: String,
    repr_ok: bool,
    is_enum: bool,
    /// 欄の型（構造体）。列挙は欄を持つ変種があれば None
    fields: Option<Vec<Vec<Token>>>,
    file: String,
    line: u32,
}

fn collect_defs(all: &[Src]) -> Vec<Def> {
    let mut defs = Vec::new();
    for s in all {
        let t = &s.tokens;
        for k in 0..t.len() {
            let is_struct = t[k].is_ident("struct") || t[k].is_ident("union");
            let is_enum = t[k].is_ident("enum");
            if !(is_struct || is_enum) || !t.get(k + 1).is_some_and(|n| n.kind == Kind::Ident) || s.attr_of(k).is_some()
            {
                continue;
            }
            // 定義の前の属性
            let mut repr_ok = false;
            for &(_, open, close, _) in s.attrs.iter().filter(|a| a.2 < k) {
                // 定義に付いた属性か（属性と定義の間に ; や } がない）
                let between = &t[close + 1..k];
                if between.iter().any(|x| x.is_punct(';') || x.is_punct('}') || x.is_punct('{')) {
                    continue;
                }
                if ident_at(t, open + 1, "repr") {
                    let r = t.get(open + 3).map(|r| r.text.as_str()).unwrap_or("");
                    repr_ok = r == "C" || (is_enum && PLAIN_PRIMS.contains(&r));
                }
            }
            // 本体
            let mut j = k + 2;
            if punct_at(t, j, '<') {
                let mut d = 0;
                while j < t.len() {
                    if t[j].is_punct('<') {
                        d += 1;
                    } else if t[j].is_punct('>') {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    j += 1;
                }
                j += 1;
            }
            let fields = if is_enum {
                let open = (j..t.len()).find(|&m| t[m].is_punct('{'));
                open.and_then(|o| {
                    let c = matching(t, o)?;
                    // 欄を持つ変種（( か { を中に持つ）がなければ空の欄
                    if t[o + 1..c].iter().any(|x| x.is_punct('(') || x.is_punct('{')) { None } else { Some(Vec::new()) }
                })
            } else if punct_at(t, j, '{') {
                let c = matching(t, j).unwrap_or(j);
                Some(split_fields(&t[j + 1..c], true))
            } else if punct_at(t, j, '(') {
                let c = matching(t, j).unwrap_or(j);
                Some(split_fields(&t[j + 1..c], false))
            } else {
                Some(Vec::new())
            };
            defs.push(Def {
                name: t[k + 1].text.clone(),
                repr_ok,
                is_enum,
                fields,
                file: s.rel.clone(),
                line: t[k].line,
            });
        }
    }
    defs
}

/// 欄の並びを型の字句に分ける（名前つきなら `名前:` を除く。可視性・属性も除く）。
fn split_fields(body: &[Token], named: bool) -> Vec<Vec<Token>> {
    let mut out = Vec::new();
    let mut cur: Vec<Token> = Vec::new();
    let mut depth = 0i32;
    for x in body {
        if x.is_punct('<') || x.is_punct('(') || x.is_punct('[') {
            depth += 1;
        } else if x.is_punct('>') || x.is_punct(')') || x.is_punct(']') {
            depth -= 1;
        }
        if depth == 0 && x.is_punct(',') {
            out.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(x.clone());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.into_iter()
        .map(|f| {
            let mut f: Vec<Token> = f.into_iter().filter(|x| x.kind != Kind::Doc).collect();
            // 欄の属性（#[…]）
            while f.first().is_some_and(|x| x.is_punct('#')) && f.get(1).is_some_and(|x| x.is_punct('[')) {
                let c = matching(&f, 1).unwrap_or(f.len() - 1);
                f.drain(0..=c);
            }
            if f.first().is_some_and(|x| x.is_ident("pub")) {
                f.remove(0);
                if f.first().is_some_and(|x| x.is_punct('(')) {
                    let c = matching(&f, 0).unwrap_or(0);
                    f.drain(0..=c);
                }
            }
            if named && let Some(p) = f.iter().position(|x| x.is_punct(':')) {
                f.drain(0..=p);
            }
            f
        })
        .filter(|f| !f.is_empty())
        .collect()
}

/// `Plain` を実装した型は `#[repr(C)]`（列挙は整数の repr も可）か基本の型で、欄の型も Plain（基本の型・Plain の型・その配列）。
/// `macro_rules! 名前 { … }` の範囲と名前。
fn macro_ranges(t: &[Token]) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for k in 0..t.len() {
        if t[k].is_ident("macro_rules")
            && punct_at(t, k + 1, '!')
            && let Some(name) = t.get(k + 2)
            && let Some(open) = (k + 3..t.len()).find(|&j| t[j].is_punct('{') || t[j].is_punct('('))
            && let Some(close) = matching(t, open)
        {
            out.push((open, close, name.text.clone()));
        }
    }
    out
}

/// Plain を実装した型の名前（`impl …::Plain for X`・`plain!(…)`）。マクロの定義の中の `impl Plain for $t` は `$` の字句。
fn plain_targets(all: &[Src]) -> Vec<(&Src, Token)> {
    let mut targets: Vec<(&Src, Token)> = Vec::new();
    for s in all {
        let t = &s.tokens;
        for k in 0..t.len() {
            if t[k].is_ident("impl") {
                // impl <…> パス::Plain for X
                let mut j = k + 1;
                if punct_at(t, j, '<') {
                    let mut d = 0;
                    while j < t.len() {
                        if t[j].is_punct('<') {
                            d += 1;
                        } else if t[j].is_punct('>') {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        j += 1;
                    }
                    j += 1;
                }
                let mut last = None;
                while let Some(x) = t.get(j) {
                    if x.kind == Kind::Ident && !x.is_ident("for") {
                        last = Some(x.text.as_str());
                        j += 1;
                    } else if x.is_punct(':') {
                        j += 1;
                    } else {
                        break;
                    }
                }
                if last == Some("Plain")
                    && ident_at(t, j, "for")
                    && let Some(x) = t.get(j + 1)
                {
                    targets.push((s, x.clone()));
                }
            }
            if t[k].is_ident("plain") && punct_at(t, k + 1, '!') && punct_at(t, k + 2, '(') {
                let close = matching(t, k + 2).unwrap_or(k + 2);
                for x in t[k + 3..close].iter().filter(|x| x.kind == Kind::Ident) {
                    targets.push((s, x.clone()));
                }
            }
        }
    }
    targets
}

fn check_plain(all: &[Src]) -> Vec<Finding> {
    let defs = collect_defs(all);
    let mut out = Vec::new();
    let targets = plain_targets(all);
    let names: Vec<&str> = targets.iter().map(|(_, x)| x.text.as_str()).collect();
    let ok_type = |f: &[Token]| -> bool {
        match f {
            [x] => {
                x.kind == Kind::Ident && (PLAIN_PRIMS.contains(&x.text.as_str()) || names.contains(&x.text.as_str()))
            }
            [open, x, semi, ..] if open.is_punct('[') && semi.is_punct(';') => {
                x.kind == Kind::Ident && (PLAIN_PRIMS.contains(&x.text.as_str()) || names.contains(&x.text.as_str()))
            }
            _ => false,
        }
    };
    for (s, x) in &targets {
        // マクロの定義の中（impl Plain for $t）は plain! だけ許す（plain! を呼ぶ所で見る。ほかの名前のマクロは呼ぶ所を
        // 見ないので作らない）。ほかの名前でない型（タプル・参照・ポインタ・配列）は Plain にしない
        if x.is_punct('$') {
            let in_plain = macro_ranges(&s.tokens)
                .iter()
                .any(|(o, c, n)| n == "plain" && s.tokens[*o].line <= x.line && x.line <= s.tokens[*c].line);
            if !in_plain {
                out.push(s.find(x.line, "Plain の実装をマクロで作るのは plain! だけ（呼ぶ所を見るため）".into()));
            }
            continue;
        }
        if x.kind != Kind::Ident {
            out.push(s.find(x.line, format!("Plain にするのは基本の型か名前の型だけ（{} …）", x.text)));
            continue;
        }
        if PLAIN_PRIMS.contains(&x.text.as_str()) {
            continue;
        }
        if PRIMS.contains(&x.text.as_str()) {
            out.push(s.find(x.line, format!("{} は Plain にしない（大きさ・整列がターゲットか版で変わる）", x.text)));
            continue;
        }
        let same: Vec<&Def> = defs.iter().filter(|d| d.name == x.text).collect();
        if same.is_empty() {
            out.push(s.find(x.line, format!("{} は Plain だが src に定義がない", x.text)));
        }
        for d in same {
            if !d.repr_ok {
                out.push(s.find(
                    x.line,
                    format!(
                        "{} は Plain だが #[repr(C)] でない（{}:{}。大きさが Rust の版で変わりうる。ADR-39 (3)）",
                        x.text, d.file, d.line
                    ),
                ));
            }
            match &d.fields {
                None => out.push(s.find(x.line, format!("{} は Plain の列挙だが欄を持つ変種がある", x.text))),
                Some(fs) if !d.is_enum => {
                    for f in fs {
                        if !ok_type(f) {
                            let ty: Vec<&str> = f.iter().map(|y| y.text.as_str()).collect();
                            out.push(s.find(
                                x.line,
                                format!(
                                    "{} の欄の型 {} が Plain でない（基本の型・Plain の型・その配列だけ）",
                                    x.text,
                                    ty.join("")
                                ),
                            ));
                        }
                    }
                }
                Some(_) => {}
            }
        }
    }
    out
}

/// パスの根（`crate`・`super`・`self`）から始まる参照のモジュールの名前を解く。
/// 戻り値は (名前の字句の位置, 解いたモジュール。根の中の項目なら None)。
fn resolve_paths(s: &Src) -> Vec<(usize, Option<String>)> {
    let t = &s.tokens;
    let mut out = Vec::new();
    for k in 0..t.len() {
        let x = &t[k];
        if !(x.is_ident("crate") || x.is_ident("super") || x.is_ident("self")) || !path_sep(t, k + 1) {
            continue;
        }
        if !is_path_root(t, k) || s.attr_of(k).is_some() {
            continue;
        }
        // 基のパス
        let base: Vec<String> = match x.text.as_str() {
            "crate" => Vec::new(),
            "self" => s.path.clone(),
            _ => {
                let mut b = s.path.clone();
                b.pop();
                b
            }
        };
        // (基, 位置)。`{…}` の組の頭の super・self も、外の基から続けて解く（`use super::{super::render::R}`）
        let mut work: Vec<(Vec<String>, usize)> = vec![(base, k + 3)];
        while let Some((mut base, mut j)) = work.pop() {
            loop {
                if ident_at(t, j, "super") && path_sep(t, j + 1) {
                    base.pop();
                    j += 3;
                } else if ident_at(t, j, "self") && path_sep(t, j + 1) {
                    j += 3;
                } else {
                    break;
                }
            }
            if punct_at(t, j, '{') {
                for h in group_heads(t, j) {
                    work.push((base.clone(), h));
                }
                continue;
            }
            match base.first() {
                Some(m) => out.push((j, Some(m.clone()))),
                None => {
                    let name = t.get(j).map(|y| y.text.clone()).unwrap_or_default();
                    out.push((j, if LAYERS.iter().any(|l| l.0 == name) { Some(name) } else { None }));
                }
            }
        }
    }
    out.sort_by_key(|x| x.0);
    out
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
    for (h, m) in resolve_paths(s) {
        let line = t.get(h).map(|x| x.line).unwrap_or(1);
        let text = t.get(h).map(|x| x.text.as_str()).unwrap_or("");
        let Some(m) = m else {
            if text == "*" || text == "self" {
                out.push(s.find(line, format!("crate::{text} は使わない（どのモジュールを使うかが字句で分からない）")));
            } else if my_layer != 6 {
                out.push(s.find(line, format!("crate::{text} は根（層 6）か表にないモジュールを使う")));
            }
            continue;
        };
        if m == s.module {
            continue;
        }
        let Some(&(_, layer, order)) = LAYERS.iter().find(|l| l.0 == m) else {
            out.push(s.find(line, format!("モジュール {m} が LAYERS の表にない")));
            continue;
        };
        let up = layer > my_layer || (layer == my_layer && my_layer != 0 && order > my_order);
        if up {
            out.push(s.find(
                line,
                format!("{}（層 {my_layer}）が上の {m}（層 {layer}）を使う（architecture.md §2）", s.module),
            ));
        }
    }
    out
}

/// impl と trait の塊（`{` と `}` の位置と、自分の側の名前：impl の型とトレイト、trait の名前）。
fn impl_blocks(t: &[Token]) -> Vec<(usize, usize, Vec<String>)> {
    let mut out = Vec::new();
    for k in 0..t.len() {
        if t[k].is_ident("trait")
            && let Some(n) = t.get(k + 1).filter(|n| n.kind == Kind::Ident)
            && let Some(open) = (k + 2..t.len()).find(|&j| t[j].is_punct('{') || t[j].is_punct(';'))
            && t[open].is_punct('{')
            && let Some(close) = matching(t, open)
        {
            out.push((open, close, vec![n.text.clone()]));
            continue;
        }
        if !t[k].is_ident("impl") {
            continue;
        }
        let mut j = k + 1;
        let mut depth = 0i32;
        let mut names: Vec<&str> = Vec::new();
        let mut after_for: Option<&str> = None;
        let mut seen_for = false;
        while let Some(x) = t.get(j) {
            if x.is_punct('<') {
                depth += 1;
            } else if x.is_punct('>') && !punct_at(t, j - 1, '-') {
                depth -= 1;
            } else if depth == 0 && (x.is_punct('{') || x.is_punct(';')) {
                break;
            } else if depth == 0 && x.is_ident("for") {
                seen_for = true;
            } else if depth == 0 && x.kind == Kind::Ident && !x.is_ident("where") {
                if seen_for && after_for.is_none() {
                    after_for = Some(x.text.as_str());
                } else {
                    names.push(x.text.as_str());
                }
            } else if depth == 0 && x.is_ident("where") {
                break;
            }
            j += 1;
        }
        let open = (j..t.len()).find(|&m| t[m].is_punct('{'));
        let Some(open) = open else { continue };
        let mut owners: Vec<String> = Vec::new();
        if let Some(ty) = after_for {
            owners.push(ty.to_string());
        }
        if let Some(tr) = names.last() {
            owners.push(tr.to_string());
        }
        if let Some(close) = matching(t, open)
            && !owners.is_empty()
        {
            out.push((open, close, owners));
        }
    }
    out
}

/// `t[k]` を囲む塊の自分の側の名前（いちばん内側の impl・trait）。
fn owners_at(impls: &[(usize, usize, Vec<String>)], k: usize) -> Vec<&str> {
    impls
        .iter()
        .rfind(|(o, c, _)| *o < k && k < *c)
        .map(|(_, _, n)| n.iter().map(String::as_str).collect())
        .unwrap_or_default()
}

/// `t[m]` の前の `::` をたどったパスの根が自分の側か（`Self`・`self`・`crate`・`super`・impl の型・トレイトの名前、
/// `<Self as Walk>::`・`<X as T>::` の X・T）。turbofish（`X::<T>::`）は越えてたどる。
fn local_path_root(t: &[Token], m: usize, owners: &[&str]) -> bool {
    let mut r = m - 3;
    loop {
        if t[r].is_punct('>') {
            // 開きの < を探す
            let mut d = 0i32;
            let mut q = r;
            loop {
                if t[q].is_punct('>') && !(q >= 1 && punct_at(t, q - 1, '-')) {
                    d += 1;
                } else if t[q].is_punct('<') {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                if q == 0 {
                    return false;
                }
                q -= 1;
            }
            if after_sep(t, q) && q >= 3 {
                // turbofish：その前の欄から続ける
                r = q - 3;
                continue;
            }
            return t[q..r]
                .iter()
                .any(|x| x.kind == Kind::Ident && (x.text == "Self" || owners.contains(&x.text.as_str())));
        }
        if r >= 3 && after_sep(t, r) && (t[r - 3].kind == Kind::Ident || t[r - 3].is_punct('>')) {
            r -= 3;
            continue;
        }
        break;
    }
    let root = t[r].text.as_str();
    matches!(root, "Self" | "self" | "crate" | "super") || owners.contains(&root)
}

/// 関数の引数と本体で束ねた名前（`let x`・`let mut x`・`for x`・`ref x`・`|x|`・引数の `x:`）。値として渡す形の判定から除く。
fn bound_names(t: &[Token], fn_kw: usize, close: usize) -> Vec<&str> {
    let mut out = Vec::new();
    for m in fn_kw + 2..close {
        let x = &t[m];
        if x.kind != Kind::Ident {
            continue;
        }
        let after_kw = ident_at(t, m - 1, "let")
            || ident_at(t, m - 1, "mut")
            || ident_at(t, m - 1, "for")
            || ident_at(t, m - 1, "ref");
        let param = punct_at(t, m + 1, ':') && !punct_at(t, m + 2, ':') && !punct_at(t, m - 1, ':');
        let closure = (punct_at(t, m - 1, '|') || punct_at(t, m - 1, ','))
            && (punct_at(t, m + 1, '|') || punct_at(t, m + 1, ','))
            && (punct_at(t, m - 1, '|') || punct_at(t, m + 1, '|'));
        if after_kw || param || closure {
            out.push(x.text.as_str());
        }
    }
    out
}

/// 本体の `t[m]` が自分の側の関数を値として渡す形か（`map(depth)`・`map(Self::depth)`・`let g = f;`）。
fn value_ref(t: &[Token], m: usize, owners: &[&str], bound: &[&str]) -> bool {
    let next_ok = punct_at(t, m + 1, ')') || punct_at(t, m + 1, ',') || punct_at(t, m + 1, ';');
    if !next_ok || punct_at(t, m - 1, '.') {
        return false;
    }
    if after_sep(t, m) {
        return local_path_root(t, m, owners);
    }
    let prev_ok = punct_at(t, m - 1, '(') || punct_at(t, m - 1, ',') || punct_at(t, m - 1, '=');
    prev_ok && !bound.contains(&t[m].text.as_str())
}

/// 自分の名前を呼ぶ関数（再帰しない。明示の積み上げで書く）。
fn check_recursion(s: &Src) -> Vec<Finding> {
    let t = &s.tokens;
    let impls = impl_blocks(t);
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
        let close = matching(t, open).unwrap_or(open);
        if RECURSION_OK.iter().any(|(f, n, _)| *f == s.rel && *n == name.text) {
            continue;
        }
        let owners = owners_at(&impls, k);
        let bound = bound_names(t, k, close);
        for m in open + 1..close {
            if !t[m].is_ident(&name.text) {
                continue;
            }
            let call = punct_at(t, m + 1, '(') || (path_sep(t, m + 1) && punct_at(t, m + 3, '<'));
            if ident_at(t, m - 1, "fn") {
                continue;
            }
            if !call {
                if value_ref(t, m, &owners, &bound) {
                    out.push(s.find(
                        t[m].line,
                        format!("{} が自分を値として渡す（再帰しない。明示の積み上げで書く）", name.text),
                    ));
                }
                continue;
            }
            let recursive = if punct_at(t, m - 1, '.') {
                // 受け手：self.名前( は自分。self.欄.名前( は委ね（別の型）。ほかの受け手は同じ型でありうるので拾う
                let delegation = m >= 4
                    && t[m - 2].kind == Kind::Ident
                    && punct_at(t, m - 3, '.')
                    && ident_at(t, m - 4, "self")
                    && !punct_at(t, m - 5, '.');
                !delegation
            } else if after_sep(t, m) {
                // パス：根が Self・self・crate・super・impl の型・トレイトなら自分（libm::sin( などは別のもの）
                local_path_root(t, m, &owners)
            } else {
                true
            };
            if recursive {
                out.push(s.find(t[m].line, format!("{} が自分を呼ぶ（再帰しない。明示の積み上げで書く）", name.text)));
            }
        }
    }
    out
}

/// 関数の本体から、自分の側の口を呼ぶ・値として渡す名前を集める（`名前(`・`self.名前(`・`Self::名前(`・impl の型や
/// トレイト・crate・super・self から始まるパスの `名前(`・`map(名前)`・`map(Self::名前)`。`self.欄.名前(` の委ねと、ほかの
/// 型・クレートのパスは別のものとみなす）。
fn local_calls(t: &[Token], fn_kw: usize, open: usize, close: usize, owners: &[&str]) -> Vec<String> {
    let bound = bound_names(t, fn_kw, close);
    let mut out = Vec::new();
    for m in open + 1..close {
        if t[m].kind != Kind::Ident || ident_at(t, m - 1, "fn") {
            continue;
        }
        let call = punct_at(t, m + 1, '(') || (path_sep(t, m + 1) && punct_at(t, m + 3, '<'));
        if !call {
            if value_ref(t, m, owners, &bound) {
                out.push(t[m].text.clone());
            }
            continue;
        }
        let local = if punct_at(t, m - 1, '.') {
            ident_at(t, m - 2, "self") && !punct_at(t, m - 3, '.')
        } else if after_sep(t, m) {
            local_path_root(t, m, owners)
        } else {
            true
        };
        if local {
            out.push(t[m].text.clone());
        }
    }
    out
}

/// 互いを呼ぶ再帰（呼び出しの輪）。関数の名前で辺を張り、2 つ以上の関数の輪を拾う（自分を呼ぶのは check_recursion）。
fn check_mutual_recursion(all: &[Src]) -> Vec<Finding> {
    // 名前 → (ファイル, 行, 呼ぶ名前)
    let mut fns: Vec<(String, String, u32, Vec<String>)> = Vec::new();
    for s in all {
        let t = &s.tokens;
        let impls = impl_blocks(t);
        for k in 0..t.len() {
            if !t[k].is_ident("fn") {
                continue;
            }
            let Some(name) = t.get(k + 1).filter(|n| n.kind == Kind::Ident) else { continue };
            let Some(open) = (k + 2..t.len()).find(|&j| t[j].is_punct('{') || t[j].is_punct(';')) else { continue };
            if !t[open].is_punct('{') {
                continue;
            }
            let close = matching(t, open).unwrap_or(open);
            let calls = local_calls(t, k, open, close, &owners_at(&impls, k));
            fns.push((name.text.clone(), s.rel.clone(), name.line, calls));
        }
    }
    let names: Vec<&str> = fns.iter().map(|f| f.0.as_str()).collect();
    let idx = |n: &str| names.iter().position(|x| *x == n);
    // 名前ごとの辺（同じ名前の関数はまとめる）
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); fns.len()];
    for (i, f) in fns.iter().enumerate() {
        let me = idx(&f.0).unwrap_or(i);
        for c in &f.3 {
            if let Some(j) = idx(c)
                && j != me
                && !edges[me].contains(&j)
            {
                edges[me].push(j);
            }
        }
    }
    // 輪を探す（色つきの深さ優先。試験のコードなので再帰でよい）
    let mut color = vec![0u8; fns.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut cycles: Vec<Vec<usize>> = Vec::new();
    fn dfs(v: usize, edges: &[Vec<usize>], color: &mut [u8], stack: &mut Vec<usize>, cycles: &mut Vec<Vec<usize>>) {
        color[v] = 1;
        stack.push(v);
        for &w in &edges[v] {
            if color[w] == 1 {
                let from = stack.iter().position(|&x| x == w).unwrap_or(0);
                cycles.push(stack[from..].to_vec());
            } else if color[w] == 0 {
                dfs(w, edges, color, stack, cycles);
            }
        }
        stack.pop();
        color[v] = 2;
    }
    for v in 0..fns.len() {
        if color[v] == 0 && idx(&fns[v].0) == Some(v) {
            dfs(v, &edges, &mut color, &mut stack, &mut cycles);
        }
    }
    cycles
        .into_iter()
        .map(|c| {
            let f = &fns[c[0]];
            let ring: Vec<&str> = c.iter().map(|&i| fns[i].0.as_str()).collect();
            Finding {
                file: f.1.clone(),
                line: f.2,
                what: format!(
                    "関数が互いを呼ぶ（{} → {}。再帰しない。明示の積み上げで書く）",
                    ring.join(" → "),
                    ring[0]
                ),
            }
        })
        .collect()
}

fn check_all(all: &[Src]) -> Vec<Finding> {
    let mut out = Vec::new();
    for s in all {
        out.extend(check_cfg(s));
        out.extend(check_std(s));
        out.extend(check_forbidden_idents(s));
        out.extend(check_shifts(s));
        out.extend(check_alloc(s));
        out.extend(check_allows(s));
        out.extend(check_layers(s));
        out.extend(check_recursion(s));
    }
    out.extend(check_plain(all));
    out.extend(check_statics(all));
    out.extend(check_mutual_recursion(all));
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
    // 再帰の例外は、今の src のその関数に当たる（当たらない例外を残さない）
    for (file, name, _) in RECURSION_OK {
        let hit = all
            .iter()
            .any(|s| s.rel == *file && s.tokens.windows(2).any(|w| w[0].is_ident("fn") && w[1].is_ident(name)));
        assert!(hit, "RECURSION_OK の {file} の {name} が src にない（例外から消す）");
    }
}

/// Cargo.toml：[features]・build.rs・ターゲットごとの依存・組むときの依存がなく、本番の依存は libm だけ（既定の feature なし）。
fn check_cargo(toml: &str, has_build_rs: bool) -> Vec<String> {
    let mut errs = Vec::new();
    let sections: Vec<&str> = toml.lines().map(str::trim).filter(|l| l.starts_with('[')).collect();
    for s in &sections {
        if *s == "[features]" {
            errs.push("[features] を持たない（ビルドで振る舞いを分けない）".to_string());
        }
        if s.starts_with("[target.") {
            errs.push(format!("{s}：ターゲットごとの依存を持たない"));
        }
        if *s == "[build-dependencies]" {
            errs.push("[build-dependencies] を持たない".to_string());
        }
    }
    if toml.lines().any(|l| l.trim_start().starts_with("build =")) || has_build_rs {
        errs.push("build.rs を持たない".to_string());
    }
    let deps = toml.split("\n[dependencies]").nth(1).unwrap_or("").split("\n[").next().unwrap_or("");
    let lines: Vec<&str> = deps.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    let names: Vec<&str> = lines.iter().filter_map(|l| l.split('=').next().map(str::trim)).collect();
    if names != ["libm"] {
        errs.push(format!("本番の依存は libm だけ（{names:?}）"));
    }
    // lint を受け継ぐ（1 行消すと unwrap_used・indexing_slicing などが黙って切れる）
    let lints = toml.split("\n[lints]").nth(1).unwrap_or("").split("\n[").next().unwrap_or("");
    if !lints.lines().any(|l| l.split('#').next().unwrap_or("").replace(' ', "") == "workspace=true") {
        errs.push("[lints] workspace = true がない（workspace の厳しい lint を受け継がない）".to_string());
    }
    if let Some(l) = lines.first()
        && (!l.contains("version = \"=0.2.16\"")
            || !l.contains("default-features = false")
            || l.contains("features = ["))
    {
        errs.push(format!("libm は =0.2.16・default-features = false・feature なし（{l}）"));
    }
    errs
}

/// 根の Cargo.toml：`[workspace.lints.rust]` に `unsafe_code = "forbid"`、`[workspace.lints.clippy]` に厳しい lint を
/// すべて deny で持つ（CLAUDE.md の一覧）。
fn check_workspace_lints(toml: &str) -> Vec<String> {
    let section = |name: &str| -> Vec<(String, String)> {
        let whole = format!("\n{toml}");
        let body = whole.split(&format!("\n[{name}]")).nth(1).unwrap_or("").split("\n[").next().unwrap_or("");
        body.lines()
            .filter_map(|l| {
                let l = l.split('#').next().unwrap_or("").trim();
                let (k, v) = l.split_once('=')?;
                Some((k.trim().to_string(), v.trim().trim_matches('"').to_string()))
            })
            .collect::<Vec<_>>()
    };
    let mut errs = Vec::new();
    let rust = section("workspace.lints.rust");
    if !rust.iter().any(|(k, v)| k == "unsafe_code" && v == "forbid") {
        errs.push("[workspace.lints.rust] に unsafe_code = \"forbid\" がない".to_string());
    }
    let clippy = section("workspace.lints.clippy");
    for name in STRICT_LINTS {
        if !clippy.iter().any(|(k, v)| k == name && (v == "deny" || v == "forbid")) {
            errs.push(format!("[workspace.lints.clippy] に {name} = \"deny\" がない"));
        }
    }
    errs
}

#[test]
fn library_cargo_toml() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let toml = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    let errs = check_cargo(&toml, dir.join("build.rs").exists());
    assert!(errs.is_empty(), "{errs:?}");
    let root = std::fs::read_to_string(dir.join("../../Cargo.toml")).unwrap();
    let errs = check_workspace_lints(&root);
    assert!(errs.is_empty(), "{errs:?}");
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
    assert_eq!(hits("math.rs", "fn f() -> bool { cfg!(debug_assertions) }"), 2); // cfg! と設定の名前
    assert_eq!(hits("math.rs", "#[cfg_attr(test, derive(Debug))] struct A;"), 1);
    assert_eq!(hits("math.rs", "#[cfg(test)] mod tests { }"), 1);
    assert_eq!(hits("math.rs", "#[cfg(test)] mod other;"), 1);
    assert_eq!(hits("math.rs", "mod tests;"), 1, "cfg(test) のない mod tests は本番に入るのに走査から外れる");
    assert_eq!(hits("math.rs", "fn f() -> bool { is_x86_feature_detected!(\"avx2\") }"), 1);
    assert_eq!(hits("math.rs", "#[path = \"../../x.rs\"] mod x;"), 1);
    assert_eq!(hits("math.rs", "include!(\"../../x.rs\");"), 1);
    assert_eq!(hits("math.rs", "const A: &str = env!(\"HOME\"); const B: Option<&str> = option_env!(\"X\");"), 2);
    // マクロの引数で属性を作る形（cfg( と設定の名前）
    let meta = "macro_rules! m { ($a:meta) => { #[$a] fn f() {} }; }\nm!(cfg(debug_assertions));";
    assert_eq!(hits("math.rs", meta), 2);
    assert_eq!(hits("math.rs", "m!(cfg_attr(test, derive(Debug)));"), 1);
    assert_eq!(hits("math.rs", "const A: bool = n!(target_arch);"), 1);
    // 識別子の cfg・注釈と文字列の中は数えない
    assert_eq!(hits("math.rs", "fn f(cfg: u8) -> u8 { cfg }"), 0);
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
    assert_eq!(hits("mem/mod.rs", "use std::sync::Arc;"), 0);
    assert_eq!(hits("math.rs", "use std::sync::Arc;"), 2); // Arc は mem の中だけ（表と確保の型）
    assert_eq!(hits("math.rs", "use std::sync::Mutex;"), 2); // sync の表と、大域の状態
    assert_eq!(hits("cancel.rs", "use std::sync::atomic::{AtomicBool, Ordering};"), 0);
    assert_eq!(hits("math.rs", "use std::sync::atomic::AtomicU64;"), 2);
    assert_eq!(hits("mem/mod.rs", "use std::collections::TryReserveError; use std::vec::Vec;"), 0);
    assert_eq!(hits("math.rs", "use std::collections::BTreeMap;"), 2); // collections と、順序の型
    assert_eq!(hits("math.rs", "use std::vec::Vec;"), 2);
    // 絶対のパス・別名・extern crate
    assert_eq!(hits("math.rs", "fn f() { let _ = ::std::fs::read(\"x\"); }"), 1);
    assert_eq!(hits("math.rs", "fn f() -> u64 { ::core::arch::aarch64::vaddd_u64(1, 2) }"), 1);
    assert_eq!(hits("math.rs", "use ::std::time::Instant;"), 1);
    assert_eq!(hits("math.rs", "use core as c;"), 1);
    assert_eq!(hits("math.rs", "extern crate std as s;"), 1);
    assert_eq!(hits("math.rs", "use std;"), 1);
    assert_eq!(hits("math.rs", "fn f() -> String { std::fmt::format(format_args!(\"x\")) }"), 3);
    // ほかのパスの中の std は別のもの
    assert_eq!(hits("math.rs", "fn f() { let _ = libm::std::x; }"), 0);
}

#[test]
fn rule_forbidden_idents() {
    assert_eq!(hits("math.rs", "fn f(a: f64) -> f64 { a.mul_add(1.0, 2.0) }"), 1);
    assert_eq!(hits("math.rs", "fn f(a: f64) -> f64 { a.algebraic_add(1.0) }"), 1);
    assert_eq!(hits("math.rs", "unsafe fn f() {}"), 1);
    assert_eq!(hits("math.rs", "fn f(a: f64) -> f64 { a * 2.0 + 1.0 }"), 0);
    assert_eq!(hits("cancel.rs", "static N: AtomicU64 = AtomicU64::new(0);"), 1);
    assert_eq!(hits("math.rs", "static T: [u8; 2] = [1, 2];"), 0);
    assert_eq!(hits("math.rs", "static mut X: u8 = 0;"), 1);
    assert_eq!(
        hits("mem/mod.rs", "#[expect(clippy::disallowed_types, reason = \"x\")] fn f(_: HashMap<u8, u8>) {}"),
        2
    );
    assert_eq!(hits("math.rs", "fn f(a: f32) -> f32 { libm::fmaf(a, a, a) }"), 1);
    // 大域の状態
    assert_eq!(hits("math.rs", "thread_local! { static X: u8 = 0; }"), 1);
    assert_eq!(hits("math.rs", "static O: OnceLock<u8> = OnceLock::new();"), 3); // OnceLock ×2 と static の型
    assert_eq!(hits("math.rs", "static C: Cell<u8> = Cell::new(0);"), 1);
    assert_eq!(hits("math.rs", "static R: RefCell<u8> = RefCell::new(0);"), 1);
    assert_eq!(hits("math.rs", "static X: crate::cancel::Flag = crate::cancel::Flag::new();"), 1);
    assert_eq!(hits("math.rs", "static X: ([u8; 1], AtomicBool) = ([0], AtomicBool::new(false));"), 3);
    assert_eq!(hits("math.rs", "static S: &str = \"x\"; static T: [(u32, u8); 2] = [(1, 2), (3, 4)];"), 0);
    let plain =
        "#[repr(C)] struct Pt { x: u32 }\nimpl Plain for Pt {}\nstatic P: [Pt; TABLE_LEN] = [Pt { x: 1 }; TABLE_LEN];";
    assert_eq!(hits("geom.rs", plain), 0);
    // ポインタの値・libm・partial_cmp
    assert_eq!(hits("math.rs", "fn f(a: &[u8]) -> usize { a.as_ptr() as usize }"), 1);
    assert_eq!(hits("math.rs", "fn f(p: *const u8, q: *mut u8) {}"), 2);
    assert_eq!(hits("math.rs", "fn f(a: &u8) -> usize { core::ptr::addr_of!(*a).addr() }"), 3); // ptr・addr_of・addr
    assert_eq!(hits("math.rs", "fn f(a: &u8, b: u8) -> u8 { *a * b }"), 0);
    assert_eq!(hits("geom.rs", "fn f(x: f64) -> f64 { libm::fmax(x, 0.0) }"), 1);
    assert_eq!(hits("math.rs", "fn f(x: f64) -> f64 { libm::fmax(x, 0.0) }"), 0);
    assert_eq!(hits("math.rs", "fn f(a: f64, b: f64) -> bool { a.partial_cmp(&b).is_some() }"), 1);
    assert_eq!(hits("math.rs", "impl PartialOrd for A { fn partial_cmp(&self, o: &A) -> Option<O> { None } }"), 0);
}

#[test]
fn rule_shifts() {
    assert_eq!(hits("math.rs", "fn f(a: u32, b: u32) -> u32 { a << b }"), 1);
    assert_eq!(hits("math.rs", "fn f(a: u32, b: u32) -> u32 { a >> b }"), 1);
    assert_eq!(hits("math.rs", "fn f(mut a: u32, b: u32) { a <<= b; a >>= b; }"), 2);
    assert_eq!(hits("math.rs", "fn f(a: u32, b: u32) -> u32 { a << 2 + b }"), 1);
    assert_eq!(hits("math.rs", "fn f(a: u32, b: u8) -> u32 { a >> (b % 8) }"), 1);
    // 数・定数のずらしは見ない（rustc の arithmetic_overflow が見る）
    assert_eq!(hits("math.rs", "fn f(mut e: u32) -> u32 { e >>= 1; (e << 3) >> SHIFT_BITS }"), 0);
    assert_eq!(hits("math.rs", "fn f(a: u32, b: u32) -> u32 { a.checked_shl(b).unwrap_or(0) }"), 0);
    // 型の閉じと修飾つきのパス
    assert_eq!(
        hits("math.rs", "fn f(x: Option<Option<u8>>) -> Option<Option<u8>> { let y: Option<Option<u8>> = x; y }"),
        0
    );
    assert_eq!(hits("math.rs", "fn g() -> u32 { <<u8 as A>::B as C>::d() }"), 0);
}

#[test]
fn rule_alloc_outside_mem() {
    assert_eq!(hits("doc.rs", "fn f() { let v: Vec<u8> = Vec::new(); }"), 2);
    assert_eq!(hits("doc.rs", "fn f() { let _ = vec![1]; let _ = format!(\"x\"); }"), 2);
    assert_eq!(hits("doc.rs", "fn f(a: &[u8]) { let _ = a.to_vec(); let _ = a.iter().collect::<X>(); }"), 2);
    assert_eq!(hits("doc.rs", "fn f(a: &X) { let _ = a.clone(); let _ = a.iter().cloned(); }"), 2);
    assert_eq!(hits("doc.rs", "fn f(t: &str) -> usize { t.to_ascii_lowercase().len() }"), 1);
    assert_eq!(hits("doc.rs", "fn f(a: &Arc<u8>) -> usize { a.len() }"), 1);
    assert_eq!(hits("mem/mod.rs", "fn f() { let v: Vec<u8> = Vec::new(); let _ = v.clone(); }"), 0);
    assert_eq!(hits("doc.rs", "fn f() { let _ = Box::new(1); let _ = String::new(); let _: Cow<str> = x; }"), 3);
    assert_eq!(hits("doc.rs", "fn f(v: &mut X, a: &[&str]) { v.extend(a); let _ = a.join(\",\"); }"), 2);
    assert_eq!(hits("doc.rs", "fn f() -> u8 { format_args!(\"x\"); 0 }"), 1);
    // mem の中の数えない変換
    assert_eq!(hits("mem/mod.rs", "fn f() -> Vec<u8> { Vec::from([1u8]) }"), 1);
    assert_eq!(hits("mem/mod.rs", "fn f() -> Box<[u8]> { Default::default() }"), 1);
    assert_eq!(hits("mem/mod.rs", "fn f(a: &[u8]) -> Box<[u8]> { a.into() }"), 1);
    assert_eq!(hits("mem/mod.rs", "fn f(a: u32) -> u64 { u64::from(a) }"), 0);
    // 自分の型の名前の一部は拾わない
    assert_eq!(hits("doc.rs", "struct MVecLike; fn f(x: &MVecLike) {}"), 0);
}

#[test]
fn rule_allows() {
    assert_eq!(hits("doc.rs", "#[allow(clippy::indexing_slicing)] fn f() {}"), 1);
    assert_eq!(hits("doc.rs", "#[allow(clippy::too_many_arguments)] fn f() {}"), 0);
    assert_eq!(hits("doc.rs", "#[allow(clippy::needless_range_loop)] fn f() {}"), 1, "表にない lint");
    assert_eq!(hits("mem/mod.rs", "#[allow(clippy::disallowed_methods)] fn f() {}"), 1);
    assert_eq!(hits("mem/mod.rs", "#[expect(clippy::disallowed_methods, reason = \"包み\")] fn f() {}"), 0);
    assert_eq!(hits("mem/mod.rs", "#![expect(clippy::disallowed_methods, reason = \"包み\")]"), 1);
    assert_eq!(hits("lib.rs", "#![allow(clippy::panic)]"), 1);
    // lint の群・warnings
    assert_eq!(hits("doc.rs", "#[allow(clippy::restriction)] fn f() {}"), 1);
    assert_eq!(hits("doc.rs", "#[allow(clippy::style)] fn f() {}"), 1);
    assert_eq!(hits("mem/mod.rs", "#[expect(clippy::all, reason = \"x\")] fn f() {}"), 1);
    assert_eq!(hits("lib.rs", "#![allow(clippy::nursery)]"), 1);
    assert_eq!(hits("doc.rs", "#[allow(warnings)] fn f() {}"), 1);
    assert_eq!(hits("doc.rs", "#[allow(deprecated)] fn f() {}"), 1);
    assert_eq!(hits("doc.rs", "#[allow(unused)] fn f() {}"), 1);
    assert_eq!(hits("doc.rs", "#[allow(unconditional_recursion)] fn f() {}"), 1);
    // mod・impl・trait に付けた外側の属性はモジュールごとの外し
    assert_eq!(hits("mem/mod.rs", "#[expect(clippy::disallowed_methods, reason = \"x\")] mod sort;"), 1);
    assert_eq!(hits("mem/mod.rs", "#[expect(clippy::indexing_slicing, reason = \"x\")]\nimpl M { fn f() {} }"), 1);
    assert_eq!(hits("mem/mod.rs", "#[expect(clippy::indexing_slicing, reason = \"x\")]\npub(crate) trait T {}"), 1);
    assert_eq!(
        hits(
            "mem/mod.rs",
            "#[expect(clippy::indexing_slicing, reason = \"x\")]\n/// 説明\n#[inline]\npub(crate) fn f() {}"
        ),
        0
    );
    // マクロの引数で lint の属性を渡す形
    assert_eq!(hits("doc.rs", "m!(allow(clippy::panic));"), 1);
    assert_eq!(hits("doc.rs", "fn f(o: Option<u8>) -> u8 { o.expect(\"x\") }"), 0); // clippy が拾う
    assert_eq!(hits("doc.rs", "#[warn(clippy::unwrap_used)] fn f() {}"), 1);
    assert_eq!(hits("doc.rs", "#[expect(dead_code, reason = \"M1 から\")] fn f() {}"), 0);
}

#[test]
fn rule_plain() {
    let ok =
        "#[repr(C)]\n#[derive(Clone, Copy)]\nstruct A { x: u32, y: [u8; 4] }\nimpl Plain for A {}\nplain!(u8, u32);";
    assert_eq!(hits("mem/mod.rs", ok), 0);
    let bad = "#[derive(Clone, Copy)]\nstruct B { x: u32 }\nimpl Plain for B {}";
    assert_eq!(hits("mem/mod.rs", bad), 1);
    let en = "#[repr(u8)]\n#[derive(Clone, Copy)]\nenum E { A, B }\nplain!(E);";
    assert_eq!(hits("mem/mod.rs", en), 0);
    assert_eq!(hits("mem/mod.rs", "#[repr(u8)] enum F { A(u8) } plain!(F);"), 1);
    assert_eq!(hits("mem/mod.rs", "plain!(Missing);"), 1);
    assert_eq!(hits("mem/mod.rs", "impl Plain for (u8, u32, u8) {}"), 1);
    assert_eq!(hits("mem/mod.rs", "impl Plain for &'static str {}"), 1);
    assert_eq!(hits("mem/mod.rs", "impl Plain for [u64; 4] {}"), 1);
    assert_eq!(hits("mem/mod.rs", "plain!(usize, i128);"), 2);
    assert_eq!(hits("geom.rs", "#[repr(C)] struct U { a: usize }\nimpl Plain for U {}"), 1);
    assert_eq!(hits("geom.rs", "#[repr(C)] struct S<T> { a: T }\nimpl<T: Plain> Plain for S<T> {}"), 1);
    assert_eq!(hits("mem/mod.rs", "macro_rules! plain { ($($t:ty),*) => { $(impl Plain for $t {})* }; }"), 0);
    assert_eq!(hits("mem/mod.rs", "macro_rules! mk { ($t:ty) => { impl Plain for $t {} }; }"), 1);
    assert_eq!(hits("geom.rs", "use crate::mem::Plain as P;\nstruct B { x: u32 }\nimpl P for B {}"), 1);
    assert_eq!(hits("mem/mod.rs", "#[repr(usize)] #[derive(Clone, Copy)] enum E { A } plain!(E);"), 1);
    // 欄の属性は型に数えない
    assert_eq!(hits("geom.rs", "#[repr(C)] struct A(#[doc = \"x\"] u32);\nimpl Plain for A {}"), 0);
    // パスつき・型の引数つきの impl
    assert_eq!(hits("geom.rs", "struct P { x: u32 }\nimpl crate::mem::Plain for P {}"), 1);
    assert_eq!(hits("geom.rs", "struct Q<T> { x: T }\nimpl<T: Copy> mem::Plain for Q<T> {}"), 2);
    // 欄の型
    assert_eq!(hits("geom.rs", "#[repr(C)] struct R { a: (u8, u32), b: Option<u32> }\nimpl Plain for R {}"), 2);
    assert_eq!(
        hits("geom.rs", "#[repr(C)] struct S { a: u8 }\n#[repr(C)] struct T { s: S, t: [S; 2] }\nplain!(S, T);"),
        0
    );
    // 同じ名前の 2 つの定義は両方を見る
    let two = [
        Src::new("geom.rs", "#[repr(C)] struct P { x: u32 }\nimpl Plain for P {}"),
        Src::new("text.rs", "struct P { x: u32 }"),
    ];
    assert_eq!(check_plain(&two).len(), 1);
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
    // super・self・glob・根の項目
    assert_eq!(hits("mem/sort.rs", "use super::MVec;"), 0);
    assert_eq!(hits("math.rs", "use super::render::Render;"), 1);
    assert_eq!(hits("geom/path.rs", "use super::super::render::Render;"), 1);
    // {…} の中の super・self も外の基から解く
    assert_eq!(hits("mem/sort.rs", "use super::{super::render::R};"), 1);
    assert_eq!(hits("mem/sort.rs", "use super::{MVec, super::budget::Budget, self::x};"), 0);
    assert_eq!(hits("math.rs", "use crate::*;"), 1);
    assert_eq!(hits("math.rs", "fn f() { crate::render_all(); }"), 1);
    assert_eq!(hits("math.rs", "use crate::{self as c};"), 1);
    assert_eq!(hits("math.rs", "const V: &str = crate::RENDERER;"), 1);
    assert_eq!(hits("fuzzing.rs", "const V: &str = crate::RENDERER;"), 0);
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
    // 木のたどりの典型
    assert_eq!(
        hits("math.rs", "impl Node { fn depth(&self) -> u32 { self.kids.iter().map(|c| c.depth()).sum() } }"),
        1
    );
    assert_eq!(hits("math.rs", "impl Node { fn depth(&self) -> u32 { Node::depth(self) } }"), 1);
    assert_eq!(hits("math.rs", "fn walk<T>(x: T) { walk::<T>(x) }"), 1);
    assert_eq!(hits("math.rs", "fn walk(x: u8) { crate::math::walk(x) }"), 1);
    assert_eq!(hits("geom/path.rs", "fn walk(x: u8) { super::path::walk(x) }"), 1);
    // 関数を値として渡す形
    assert_eq!(hits("math.rs", "fn depth(n: &N) -> u32 { n.kids.iter().map(depth).fold(0, g) }"), 1);
    assert_eq!(
        hits("math.rs", "impl N { fn depth(&self) -> u32 { self.kids.iter().map(Self::depth).fold(0, g) } }"),
        1
    );
    assert_eq!(hits("math.rs", "fn f(n: u32) -> u32 { let g: fn(u32) -> u32 = f; g(n - 1) }"), 1);
    // トレイトの名前・修飾つきのパス・turbofish
    assert_eq!(hits("math.rs", "impl Walk for N { fn walk(&self) { for c in &self.kids { Walk::walk(c) } } }"), 1);
    assert_eq!(hits("math.rs", "impl Walk for N { fn walk(&self) { <Self as Walk>::walk(self) } }"), 1);
    assert_eq!(hits("math.rs", "trait Walk { fn walk(&self) { Walk::walk(self) } }"), 1);
    assert_eq!(hits("math.rs", "impl F { fn make() -> F { F::<u8>::make() } }"), 1);
    // 欄・束ねた名前・ほかの型のパスは拾わない
    assert_eq!(hits("math.rs", "impl A { fn len(&self) -> usize { self.len } }"), 0);
    assert_eq!(hits("math.rs", "fn depth(n: u32) -> u32 { let depth = n; g(depth) }"), 0);
    assert_eq!(hits("math.rs", "fn walk(x: u8) { other::walk(x); h(other::walk) }"), 0);
}

#[test]
fn rule_mutual_recursion() {
    assert_eq!(hits("math.rs", "fn a() { b() } fn b() { a() }"), 1);
    assert_eq!(
        hits(
            "mem/mod.rs",
            "impl M { fn ensure(&mut self) { self.copy_in() } fn copy_in(&mut self) { self.ensure() } }"
        ),
        1
    );
    assert_eq!(hits("math.rs", "fn a() { b() } fn b() { c() } fn c() { a() }"), 1);
    assert_eq!(hits("math.rs", "fn a() { b() } fn b() {}"), 0);
    assert_eq!(hits("math.rs", "fn a() { run(b) } fn b() { a() }"), 1);
    assert_eq!(hits("math.rs", "impl W for N { fn a(&self) { W::b(self) } fn b(&self) { self.a() } }"), 1);
    // 委ね・ほかの型のパスは辺にしない
    assert_eq!(
        hits("mem/mod.rs", "impl M { fn len(&self) { self.v.len() } fn get(&self) { Other::len(); self.v.get() } }"),
        0
    );
    // ファイルをまたぐ輪
    let two = [
        Src::new("math.rs", "pub(crate) fn a() { crate::cost::b() }"),
        Src::new("cost.rs", "pub(crate) fn b() { crate::math::a() }"),
    ];
    assert_eq!(check_mutual_recursion(&two).len(), 1);
}

#[test]
fn rule_cargo_toml() {
    let good = "[package]\nname = \"sumie\"\n\n[dependencies]\nlibm = { version = \"=0.2.16\", default-features = false }\n\n[dev-dependencies]\nblake3 = \"=1.8.7\"\n\n[lints]\nworkspace = true\n";
    assert!(check_cargo(good, false).is_empty());
    assert_eq!(check_cargo(&good.replace("workspace = true", "workspace = false"), false).len(), 1);
    assert_eq!(check_cargo(&good.replace("\n[lints]\nworkspace = true\n", ""), false).len(), 1);
    // 根の lint
    let mut ws = String::from("[workspace.lints.rust]\nunsafe_code = \"forbid\"\n\n[workspace.lints.clippy]\n");
    for l in STRICT_LINTS {
        ws.push_str(&format!("{l} = \"deny\"\n"));
    }
    assert!(check_workspace_lints(&ws).is_empty());
    assert_eq!(check_workspace_lints(&ws.replace("indexing_slicing = \"deny\"\n", "")).len(), 1);
    assert_eq!(check_workspace_lints(&ws.replace("panic = \"deny\"", "panic = \"warn\"")).len(), 1);
    assert_eq!(check_workspace_lints(&ws.replace("\"forbid\"", "\"deny\"")).len(), 1);
    assert_eq!(check_cargo(&format!("{good}[features]\nx = []\n"), false).len(), 1);
    assert_eq!(check_cargo(&format!("{good}[target.'cfg(unix)'.dependencies]\nlibc = \"1\"\n"), false).len(), 1);
    assert_eq!(check_cargo(&format!("{good}[build-dependencies]\ncc = \"1\"\n"), false).len(), 1);
    assert_eq!(check_cargo(good, true).len(), 1);
    let feats = good.replace("default-features = false }", "default-features = false, features = [\"arch\"] }");
    assert_eq!(check_cargo(&feats, false).len(), 1);
    let more = good.replace("[dev-dependencies]", "rand = \"1\"\n\n[dev-dependencies]");
    assert_eq!(check_cargo(&more, false).len(), 1);
}
