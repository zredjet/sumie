//! `gen-disallowed`：ライブラリの禁じる口の一覧（`crates/sumie/clippy.toml`）を、固定したツールチェーンの std の
//! ソース（rust-src）から機械的に作る（roadmap M0・ADR-41 (6)）。
//!
//! 分類（理由に書く）：
//! - **落ちうる**：std の文書に `# Panics` の節がある公開の口（core・alloc・std の浮動小数）
//! - **確保**：alloc の固有の口のうち、本体が確保する口を呼ぶもの（`reserve`・`push`・`Box::new` など）と、alloc で定義した
//!   トレイトの口（`ToOwned::to_owned`・`ToString::to_string` など）
//! - **浮動小数**：`f32`・`f64` の固有の口のうち、determinism.md §2.1 の使ってよい口でないもの（超越関数・`mul_add`・
//!   `powi`・`max`・`min`・`clamp`・`algebraic_*` など）
//! - **Unicode の表**：`char`・`str` の固有の口のうち、本体が core の Unicode の表（`unicode`）を引くもの
//! - **手で書いた分類**：determinism.md・CLAUDE.md の一覧（並べ替え・探索・時刻・環境・スレッド・`Clone::clone`・
//!   `Iterator::collect` など。std の文書からは決まらないもの）
//!
//! 生成したものから、理由を付けて許す口（[`ALLOWED`]）を除く。`--check` は今のファイルと照らす。

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use sumie_testkit::rustscan::{Kind, Token, lex};

/// 生成したファイルの置き場（workspace の根から）。
const OUT: &str = "crates/sumie/clippy.toml";

/// 符号つき・符号なしの整数（`int_impl!`・`uint_impl!` の中の口はすべてに当たる）。
const SIGNED: [&str; 6] = ["i8", "i16", "i32", "i64", "i128", "isize"];
const UNSIGNED: [&str; 6] = ["u8", "u16", "u32", "u64", "u128", "usize"];
const PRIMS: [&str; 17] = [
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "char",
    "bool", "str",
];

/// 浮動小数の使ってよい口（determinism.md §2.1：結果が一意に決まるもの・述語・ビットの変換）。
const FLOAT_OK: [&str; 33] = [
    "abs",
    "copysign",
    "signum",
    "floor",
    "ceil",
    "trunc",
    "round",
    "round_ties_even",
    "fract",
    "rem_euclid",
    "div_euclid",
    "recip",
    "sqrt",
    "total_cmp",
    "to_bits",
    "from_bits",
    "is_nan",
    "is_infinite",
    "is_finite",
    "is_normal",
    "is_subnormal",
    "classify",
    "is_sign_positive",
    "is_sign_negative",
    "to_be_bytes",
    "to_le_bytes",
    "to_ne_bytes",
    "from_be_bytes",
    "from_le_bytes",
    "from_ne_bytes",
    "next_up",
    "next_down",
    "to_int_unchecked",
];

/// 確保の口を呼ぶとみなす本体の字句（識別子）。alloc の固有の口の本体にこれがあれば「確保」に入れる。
const ALLOC_IDENTS: [&str; 32] = [
    "Concat",
    "Join",
    "reserve",
    "reserve_exact",
    "with_capacity",
    "with_capacity_in",
    "allocate",
    "allocate_zeroed",
    "RawVec",
    "RawVecInner",
    "box_new",
    "new_uninit",
    "new_uninit_in",
    "to_vec",
    "to_vec_in",
    "collect",
    "extend",
    "extend_from_slice",
    "push",
    "push_str",
    "insert",
    "clone",
    "to_owned",
    "into_boxed_slice",
    "shrink_to_fit",
    "shrink_to",
    "grow",
    "grow_amortized",
    "realloc",
    "allocate_for_layout",
    "from_box_in",
    "make_mut",
];

/// 手で書いた分類（std の文書からは決まらないもの）。理由つき。
const HAND: &[(&str, &str)] = &[
    // 確保（std の中で数えずに確保する口。ADR-31）
    (
        "core::clone::Clone::clone",
        "確保：入れ物の clone は std の中で数えずに確保する（`Clone::clone` はすべて禁じ、Arc は mem::share。ADR-41 (6)）",
    ),
    ("core::clone::Clone::clone_from", "確保：同上"),
    (
        "core::iter::traits::iterator::Iterator::collect",
        "確保：std の中で数えずに確保する（mem の入れ物を使う。ADR-31）",
    ),
    ("core::iter::traits::collect::FromIterator::from_iter", "確保：同上"),
    ("core::iter::traits::collect::Extend::extend", "確保：同上"),
    // 並べ替え・探索（determinism.md §4.1・§4.2）
    ("slice::sort", "並べ替え：std の中で確保する。mem::stable_sort を使う（ADR-31）"),
    ("slice::sort_by", "並べ替え：同上"),
    ("slice::sort_by_key", "並べ替え：同上"),
    ("slice::sort_by_cached_key", "並べ替え：同上"),
    ("slice::sort_unstable", "並べ替え：安定でない（determinism.md §4.1）。mem::stable_sort を使う"),
    ("slice::sort_unstable_by", "並べ替え：同上"),
    ("slice::sort_unstable_by_key", "並べ替え：同上"),
    ("slice::select_nth_unstable", "並べ替え：同上"),
    ("slice::select_nth_unstable_by", "並べ替え：同上"),
    ("slice::select_nth_unstable_by_key", "並べ替え：同上"),
    (
        "slice::binary_search",
        "探索：等しいものが並ぶときの位置が決まらない。mem::lower_bound を使う（determinism.md §4.1）",
    ),
    ("slice::binary_search_by", "探索：同上"),
    ("slice::binary_search_by_key", "探索：同上"),
    ("slice::partition_point", "探索：mem::lower_bound にまとめる（determinism.md §4.1）"),
    // Unicode の表（determinism.md §4.3。本体の走査で拾えないものを足す）
    (
        "char::is_whitespace",
        "Unicode の表：std の表は Rust の版で上がる。XML・CSS の空白は自前の関数（determinism.md §4.3）",
    ),
    ("char::is_alphabetic", "Unicode の表：同上"),
    ("char::is_numeric", "Unicode の表：同上"),
    ("char::is_alphanumeric", "Unicode の表：同上"),
    ("char::is_lowercase", "Unicode の表：同上"),
    ("char::is_uppercase", "Unicode の表：同上"),
    ("char::is_control", "Unicode の表：同上"),
    ("char::to_lowercase", "Unicode の表：同上"),
    ("char::to_uppercase", "Unicode の表：同上"),
    ("str::trim", "Unicode の表：U+00A0・U+3000 も削る（determinism.md §4.3）。trim_ascii か自前の関数"),
    ("str::trim_start", "Unicode の表：同上"),
    ("str::trim_end", "Unicode の表：同上"),
    ("str::split_whitespace", "Unicode の表：同上"),
    ("str::to_lowercase", "Unicode の表：同上"),
    ("str::to_uppercase", "Unicode の表：同上"),
    // 時刻・環境・スレッド・プロセス（determinism.md §5・§6。tests/rules.rs も std のモジュールの表で見張る）
    ("std::time::Instant::now", "時刻を読まない（determinism.md §5）"),
    ("std::time::SystemTime::now", "時刻を読まない（determinism.md §5）"),
    ("std::env::var", "環境変数を読まない（determinism.md §5）"),
    ("std::env::var_os", "環境変数を読まない（determinism.md §5）"),
    ("std::env::vars", "環境変数を読まない（determinism.md §5）"),
    ("std::env::vars_os", "環境変数を読まない（determinism.md §5）"),
    ("std::env::args", "コマンドの引数を読まない（determinism.md §5）"),
    ("std::env::args_os", "コマンドの引数を読まない（determinism.md §5）"),
    ("std::thread::spawn", "スレッドを作らない（determinism.md §6）"),
    ("std::thread::scope", "スレッドを作らない（determinism.md §6）"),
    ("std::thread::Builder::spawn", "スレッドを作らない（determinism.md §6）"),
    ("std::process::abort", "プロセスを止めない（roadmap M0。誤りを返す）"),
    ("std::process::exit", "プロセスを止めない（誤りを返す）"),
];

/// 整数の、あふれ・0 で panic しうる口（CLAUDE.md。std の文書の節が `# Overflow behavior` などのものを足す）。
/// その型に口があるときだけ出す。
const HAND_INT: &[(&str, &str)] = &[
    ("pow", "整数：あふれると debug で panic する。checked_pow を使う（determinism.md §3）"),
    ("abs", "整数：最小の値であふれる。unsigned_abs・checked_abs を使う（determinism.md §3）"),
    ("div_euclid", "整数：0 で panic する。checked_div_euclid を使う"),
    ("rem_euclid", "整数：0 で panic する。checked_rem_euclid を使う"),
    ("div_ceil", "整数：0 で panic する。checked_* で書く"),
    ("next_multiple_of", "整数：0・あふれで panic する。checked_next_multiple_of を使う"),
    ("ilog", "整数：0 以下で panic する。checked_ilog を使う"),
    ("ilog2", "整数：0 以下で panic する。checked_ilog2 を使う"),
    ("ilog10", "整数：0 以下で panic する。checked_ilog10 を使う"),
    ("isqrt", "整数：負で panic する。checked_isqrt を使う"),
    ("next_power_of_two", "整数：あふれると debug で panic する。checked_next_power_of_two を使う"),
];

/// 生成したものから外す口と、外す理由（使ってよいと確かめたもの）。
const ALLOWED: &[(&str, &str)] = &[
    (
        "core::iter::traits::iterator::Iterator::enumerate",
        "番号は確保した配列の長さ以下で usize を超えない（文書の Panics は usize を超える数の要素のとき）",
    ),
    ("core::iter::traits::iterator::Iterator::count", "同上"),
    ("core::iter::traits::iterator::Iterator::position", "同上"),
    ("core::iter::traits::iterator::Iterator::rposition", "同上"),
    (
        "core::sync::atomic::Atomic::load",
        "中止の印を Relaxed で読むだけ（Release・AcqRel を渡したときだけ panic する。cancel.rs の 1 か所）",
    ),
];

/// std のソースの中の 1 つの口。
#[derive(Debug, Clone)]
struct Item {
    /// clippy の `disallowed-methods` に書くパス
    path: String,
    /// 口の名前
    name: String,
    /// 口の持ち主
    owner: Owner,
    /// 文書（`///` と `#[doc = "…"]` の中身をつないだもの）
    docs: String,
    /// 本体の字句（宣言だけのトレイトの口は空）
    body: Vec<Token>,
    /// 定義したクレート（core・alloc・std）
    krate: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Owner {
    /// 基本の型の固有の口（`slice`・`str`・`u32`・`f64`・`char` …）
    Prim(String),
    /// 構造体・列挙の固有の口（定義のパス）
    Type(String),
    /// トレイトの口（定義のパス）
    Trait(String),
    /// モジュールの関数
    Free,
}

#[derive(Debug, Clone)]
enum Ctx {
    Impl { owner: Option<Owner>, skip: bool },
    Trait { path: String, skip: bool },
    Mod,
    Macro(String),
    Fn { item: Option<Item>, start: usize },
    Other,
}

/// `gen-disallowed [--check] [--print]`
pub fn run(args: &[String]) -> Result<(), String> {
    let check = args.iter().any(|a| a == "--check");
    let root = workspace_root()?;
    let (rustc, library) = rust_src()?;
    let mut items = Vec::new();
    for (krate, sub) in [("core", "core/src"), ("alloc", "alloc/src"), ("std", "std/src/num")] {
        let dir = library.join(sub);
        let mut files = Vec::new();
        collect_rs(&dir, &mut files);
        files.sort();
        for f in files {
            let rel = f.strip_prefix(library.join(format!("{krate}/src"))).map_err(|e| e.to_string())?;
            let module = module_path(krate, rel);
            let src = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
            let file_name = rel.to_string_lossy().to_string();
            scan_file(krate, &module, &file_name, &src, &mut items);
        }
    }
    let text = render(&rustc, &items);
    let out = root.join(OUT);
    if check {
        let now = std::fs::read_to_string(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        if now != text {
            return Err(format!(
                "{OUT} が std の文書から作り直したものと違います。`cargo run -p sumie-tools -- gen-disallowed` で作り直してください"
            ));
        }
        println!("{OUT} は作り直したものと同じ");
    } else {
        std::fs::write(&out, &text).map_err(|e| e.to_string())?;
        println!("{OUT} を書いた");
    }
    Ok(())
}

fn workspace_root() -> Result<PathBuf, String> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    here.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "workspace の根が見つからない".to_string())
}

/// 固定したツールチェーンの rustc の版と、rust-src の library の場所。
fn rust_src() -> Result<(String, PathBuf), String> {
    let run = |args: &[&str]| -> Result<String, String> {
        let o = Command::new("rustc").args(args).output().map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).to_string());
        }
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let version = run(&["--version"])?;
    let sysroot = run(&["--print", "sysroot"])?;
    let lib = PathBuf::from(sysroot).join("lib/rustlib/src/rust/library");
    if !lib.join("core/src/lib.rs").exists() {
        return Err(format!("rust-src がない：{}（rustup component add rust-src）", lib.display()));
    }
    Ok((version, lib))
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if name == "tests" || name == "benches" {
                continue;
            }
            collect_rs(&p, out);
        } else if name.ends_with(".rs") && name != "tests.rs" {
            out.push(p);
        }
    }
}

/// ファイルの場所からモジュールのパス（`core/src/num/f64.rs` → `core::num::f64`）。
fn module_path(krate: &str, rel: &Path) -> String {
    let mut parts = vec![krate.to_string()];
    let comps: Vec<String> = rel.iter().map(|c| c.to_string_lossy().to_string()).collect();
    for (k, c) in comps.iter().enumerate() {
        let last = k + 1 == comps.len();
        if last {
            let stem = c.trim_end_matches(".rs");
            if stem != "mod" && stem != "lib" {
                parts.push(stem.to_string());
            }
        } else {
            parts.push(c.clone());
        }
    }
    parts.join("::")
}

/// 1 つのファイルを走査して、公開の口を `items` に足す。
fn scan_file(krate: &str, module: &str, file: &str, src: &str, items: &mut Vec<Item>) {
    let t = lex(src);
    let names = Names::collect(krate, module, &t);
    let mut stack: Vec<Ctx> = Vec::new();
    // かっこの深さ（{} の文脈ごとに 0 から数え直す）
    let mut paren: Vec<i64> = vec![0];
    let mut start = 0usize;
    let mut i = 0usize;
    while i < t.len() {
        let tok = &t[i];
        if tok.kind == Kind::Punct {
            match tok.text.as_str() {
                "(" | "[" => {
                    if let Some(p) = paren.last_mut() {
                        *p += 1;
                    }
                }
                ")" | "]" => {
                    if let Some(p) = paren.last_mut() {
                        *p -= 1;
                    }
                }
                ";" if paren.last() == Some(&0) => {
                    // 本体のない口（トレイトの宣言だけの口）
                    let header = &t[start..i];
                    if let Some(item) = fn_item(krate, module, file, header, &stack) {
                        items.push(item);
                    }
                    start = i + 1;
                }
                "{" => {
                    let header = &t[start..i];
                    let ctx = classify(krate, module, file, header, &stack, i, &names);
                    stack.push(ctx);
                    paren.push(0);
                    start = i + 1;
                }
                "}" => {
                    paren.pop();
                    if paren.is_empty() {
                        paren.push(0);
                    }
                    if let Some(Ctx::Fn { item: Some(mut item), start: s }) = stack.pop() {
                        item.body = t[s..i].to_vec();
                        items.push(item);
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
}

/// 字句の並びから属性（`#[…]`・`#![…]`）を除き、属性の中身の識別子と、文書の中身を返す。
fn split_header(header: &[Token]) -> (Vec<Token>, Vec<String>, String) {
    let mut rest = Vec::new();
    let mut attrs = Vec::new();
    let mut docs = String::new();
    let mut k = 0usize;
    while k < header.len() {
        let tok = &header[k];
        if tok.kind == Kind::Doc {
            docs.push_str(&tok.text);
            docs.push('\n');
            k += 1;
            continue;
        }
        if tok.is_punct('#') {
            let mut j = k + 1;
            if header.get(j).is_some_and(|t| t.is_punct('!')) {
                j += 1;
            }
            if header.get(j).is_some_and(|t| t.is_punct('[')) {
                // 対応する ]
                let mut depth = 0i64;
                let mut end = j;
                for (m, t) in header.iter().enumerate().skip(j) {
                    if t.is_punct('[') {
                        depth += 1;
                    } else if t.is_punct(']') {
                        depth -= 1;
                        if depth == 0 {
                            end = m;
                            break;
                        }
                    }
                }
                let inner = &header[j + 1..end.max(j + 1)];
                let head = inner.first().map(|t| t.text.clone()).unwrap_or_default();
                if head == "doc" {
                    for s in inner.iter().filter(|t| t.kind == Kind::Str) {
                        docs.push_str(&s.text);
                        docs.push('\n');
                    }
                    if inner.iter().any(|t| t.is_ident("hidden")) {
                        attrs.push("doc_hidden".to_string());
                    }
                } else {
                    let text: Vec<String> = inner.iter().map(|t| t.text.clone()).collect();
                    attrs.push(text.join(" "));
                }
                k = end + 1;
                continue;
            }
        }
        rest.push(tok.clone());
        k += 1;
    }
    (rest, attrs, docs)
}

/// 安定でない・隠した・試験だけの口か。
fn is_skipped(attrs: &[String]) -> bool {
    attrs.iter().any(|a| {
        a.starts_with("unstable")
            || a == "doc_hidden"
            || a.starts_with("cfg ( test")
            || a.starts_with("rustc_deprecated_safe_2024")
    })
}

/// 見出しの字句（属性を除いたもの）から、可視性・修飾を除いた残りと、公開かどうか。
fn strip_vis(rest: &[Token]) -> (bool, Vec<Token>) {
    let mut k = 0usize;
    let mut public = false;
    if rest.first().is_some_and(|t| t.is_ident("pub")) {
        k = 1;
        if rest.get(1).is_some_and(|t| t.is_punct('(')) {
            // pub(crate) などは公開でない
            let mut depth = 0i64;
            for (m, t) in rest.iter().enumerate().skip(1) {
                if t.is_punct('(') {
                    depth += 1;
                } else if t.is_punct(')') {
                    depth -= 1;
                    if depth == 0 {
                        k = m + 1;
                        break;
                    }
                }
            }
        } else {
            public = true;
        }
    }
    let mut out: Vec<Token> = rest[k.min(rest.len())..].to_vec();
    // 修飾
    while let Some(f) = out.first() {
        if f.kind == Kind::Ident && matches!(f.text.as_str(), "default" | "const" | "async" | "safe") {
            out.remove(0);
        } else if f.is_ident("extern") && out.get(1).is_some_and(|t| t.kind == Kind::Str) {
            out.drain(0..2);
        } else {
            break;
        }
    }
    (public, out)
}

/// `{` の前の見出しから文脈を決める。
fn classify(krate: &str, module: &str, file: &str, header: &[Token], stack: &[Ctx], open: usize, names: &Names) -> Ctx {
    let (rest, attrs, _docs) = split_header(header);
    let skip = is_skipped(&attrs);
    let (public, body) = strip_vis(&rest);
    let Some(first) = body.first() else { return Ctx::Other };
    if first.is_ident("unsafe") {
        // unsafe impl・unsafe trait・unsafe fn
        if body.get(1).is_some_and(|t| t.is_ident("fn")) {
            return Ctx::Fn { item: None, start: open + 1 };
        }
        if body.get(1).is_some_and(|t| t.is_ident("impl")) {
            return Ctx::Impl { owner: None, skip: true };
        }
        if body.get(1).is_some_and(|t| t.is_ident("trait")) {
            return Ctx::Trait { path: String::new(), skip: true };
        }
    }
    if first.is_ident("impl") {
        let in_fn = stack.iter().any(|c| matches!(c, Ctx::Fn { .. }));
        let owner = if in_fn { None } else { impl_owner(module, &body[1..], names) };
        return Ctx::Impl { owner, skip };
    }
    if first.is_ident("trait") {
        let name = body.get(1).map(|t| t.text.clone()).unwrap_or_default();
        return Ctx::Trait { path: format!("{module}::{name}"), skip: skip || !public };
    }
    if first.is_ident("mod") {
        return Ctx::Mod;
    }
    if first.is_ident("macro_rules") {
        return Ctx::Macro(body.get(2).map(|t| t.text.clone()).unwrap_or_default());
    }
    if first.is_ident("fn") {
        let item = fn_item(krate, module, file, header, stack);
        return Ctx::Fn { item, start: open + 1 };
    }
    Ctx::Other
}

/// `impl` の見出し（`impl` のあと）から持ち主を決める。トレイトの実装は None（トレイトの定義の側で見る）。
fn impl_owner(module: &str, h: &[Token], names: &Names) -> Option<Owner> {
    let mut k = 0usize;
    // 型の引数 <…>
    if h.first().is_some_and(|t| t.is_punct('<')) {
        let mut depth = 0i64;
        for (m, t) in h.iter().enumerate() {
            if t.is_punct('<') {
                depth += 1;
            } else if t.is_punct('>') && !(m > 0 && h[m - 1].is_punct('-')) {
                depth -= 1;
                if depth == 0 {
                    k = m + 1;
                    break;
                }
            }
        }
    }
    let h = &h[k.min(h.len())..];
    // トレイトの実装か（深さ 0 の for。for<'a> は除く）
    let mut depth = 0i64;
    for (m, t) in h.iter().enumerate() {
        if t.is_punct('<') {
            depth += 1;
        } else if t.is_punct('>') && !(m > 0 && h[m - 1].is_punct('-')) {
            depth -= 1;
        } else if (depth == 0 && t.is_ident("for") && !h.get(m + 1).is_some_and(|n| n.is_punct('<')))
            || (t.is_punct('!') && m == 0)
        {
            // トレイトの実装（impl Trait for Type）と否定の実装（impl !Send for Type）
            return None;
        }
    }
    let first = h.first()?;
    if first.is_punct('[') {
        let mut d = 0i64;
        for t in h {
            if t.is_punct('[') {
                d += 1;
            } else if t.is_punct(']') {
                d -= 1;
                if d == 0 {
                    break;
                }
            } else if d == 1 && t.is_punct(';') {
                return None; // 配列（[T; N]）は扱わない
            }
        }
        return Some(Owner::Prim("slice".to_string()));
    }
    if first.kind != Kind::Ident || first.is_ident("dyn") {
        return None;
    }
    if PRIMS.contains(&first.text.as_str()) {
        return Some(Owner::Prim(first.text.clone()));
    }
    // パス A::B::C<…>
    let mut segs = Vec::new();
    let mut m = 0usize;
    while let Some(t) = h.get(m) {
        if t.kind == Kind::Ident && !t.is_ident("where") {
            segs.push(t.text.clone());
            if h.get(m + 1).is_some_and(|c| c.is_punct(':')) && h.get(m + 2).is_some_and(|c| c.is_punct(':')) {
                m += 3;
                continue;
            }
        }
        break;
    }
    let name = segs.last()?.clone();
    if segs.first().is_some_and(|s| s == "crate") && segs.len() > 1 {
        let krate = module.split("::").next().unwrap_or("core");
        let rest = segs[1..].join("::");
        return Some(Owner::Type(format!("{krate}::{rest}")));
    }
    if segs.len() > 1 {
        return None; // 外のパス（self::・super:: など）は扱わない
    }
    Some(Owner::Type(names.resolve(module, &name)))
}

/// ファイルの中の名前の表：定義した型・型の別名・`use` で入れた名前（impl の型の定義の場所を解くため）。
struct Names {
    defined: Vec<String>,
    aliases: Vec<(String, String)>,
    uses: Vec<(String, String)>,
}

impl Names {
    fn collect(krate: &str, module: &str, t: &[Token]) -> Names {
        let mut defined = Vec::new();
        let mut aliases = Vec::new();
        let mut uses = Vec::new();
        let mut k = 0usize;
        while k < t.len() {
            let tok = &t[k];
            if tok.kind == Kind::Ident && matches!(tok.text.as_str(), "struct" | "enum" | "union" | "trait") {
                if let Some(n) = t.get(k + 1).filter(|n| n.kind == Kind::Ident) {
                    defined.push(n.text.clone());
                }
            } else if tok.is_ident("type") {
                // type A = B<…>;
                if let (Some(a), Some(eq), Some(b)) = (t.get(k + 1), t.get(k + 2), t.get(k + 3)) {
                    if a.kind == Kind::Ident && eq.is_punct('=') && b.kind == Kind::Ident {
                        aliases.push((a.text.clone(), b.text.clone()));
                    } else if a.kind == Kind::Ident && eq.is_punct('<') {
                        // type A<T> = B<…>;
                        let mut m = k + 2;
                        while m < t.len() && !t[m].is_punct('=') && !t[m].is_punct(';') {
                            m += 1;
                        }
                        if let Some(b) = t.get(m + 1).filter(|b| b.kind == Kind::Ident)
                            && t[m].is_punct('=')
                        {
                            aliases.push((a.text.clone(), b.text.clone()));
                        }
                    }
                }
            } else if tok.is_ident("use") {
                // use a::b::C; と use a::b::{C, D};（1 段の {} だけ）
                let mut segs: Vec<String> = Vec::new();
                let mut m = k + 1;
                while let Some(x) = t.get(m) {
                    if x.kind == Kind::Ident {
                        segs.push(x.text.clone());
                        m += 1;
                    } else if x.is_punct(':') {
                        m += 1;
                    } else {
                        break;
                    }
                }
                let base = |segs: &[String]| -> String {
                    let mut out: Vec<String> = Vec::new();
                    for (i, s) in segs.iter().enumerate() {
                        match s.as_str() {
                            "crate" if i == 0 => out.push(krate.to_string()),
                            "self" if i == 0 => out.extend(module.split("::").map(str::to_string)),
                            "super" => {
                                if out.is_empty() {
                                    out.extend(module.split("::").map(str::to_string));
                                }
                                out.pop();
                            }
                            _ => out.push(s.clone()),
                        }
                    }
                    out.join("::")
                };
                if t.get(m).is_some_and(|x| x.is_punct('{')) {
                    let prefix = base(&segs);
                    let mut j = m + 1;
                    while let Some(x) = t.get(j) {
                        if x.is_punct('}') || x.is_punct('{') {
                            break;
                        }
                        if x.kind == Kind::Ident
                            && t.get(j + 1).is_some_and(|n| n.is_punct(',') || n.is_punct('}'))
                            && !t.get(j.wrapping_sub(1)).is_some_and(|p| p.is_punct(':'))
                        {
                            uses.push((x.text.clone(), format!("{prefix}::{}", x.text)));
                        }
                        j += 1;
                    }
                } else if t.get(m).is_some_and(|x| x.is_punct(';'))
                    && segs.len() > 1
                    && let Some(last) = segs.last()
                {
                    uses.push((last.clone(), base(&segs)));
                }
            }
            k += 1;
        }
        Names { defined, aliases, uses }
    }

    /// impl の型の名前から定義のパスを決める（別名はたどり、ファイルで定義していなければ use を引く）。
    fn resolve(&self, module: &str, name: &str) -> String {
        let mut name = name.to_string();
        if let Some((_, b)) = self.aliases.iter().find(|(a, _)| *a == name) {
            name = b.clone();
        }
        if self.defined.contains(&name) {
            return format!("{module}::{name}");
        }
        if let Some((_, p)) = self.uses.iter().find(|(a, _)| *a == name) {
            return p.clone();
        }
        format!("{module}::{name}")
    }
}

/// `fn` の見出しから口を作る（持ち主が決まり、公開で安定なものだけ）。
fn fn_item(krate: &str, module: &str, _file: &str, header: &[Token], stack: &[Ctx]) -> Option<Item> {
    let (rest, attrs, docs) = split_header(header);
    if is_skipped(&attrs) {
        return None;
    }
    let (public, body) = strip_vis(&rest);
    if body.first().is_some_and(|t| t.is_ident("unsafe")) {
        return None;
    }
    if !body.first().is_some_and(|t| t.is_ident("fn")) {
        return None;
    }
    let name = body.get(1).filter(|t| t.kind == Kind::Ident)?.text.clone();
    // いちばん近い文脈（Other を飛ばす）
    let mut owners = Vec::new();
    let mut inside_macro: Option<String> = None;
    let mut skip_ctx = false;
    for c in stack.iter().rev() {
        match c {
            Ctx::Other => continue,
            Ctx::Fn { .. } => return None, // 関数の中の関数
            Ctx::Impl { owner, skip } => {
                if *skip || !public {
                    return None;
                }
                owners.push(owner.clone()?);
                break;
            }
            Ctx::Trait { path, skip } => {
                skip_ctx = *skip;
                owners.push(Owner::Trait(path.clone()));
                break;
            }
            Ctx::Macro(m) => {
                inside_macro = Some(m.clone());
                continue;
            }
            Ctx::Mod => {
                if !public {
                    return None;
                }
                owners.push(Owner::Free);
                break;
            }
        }
    }
    if skip_ctx {
        return None;
    }
    if owners.is_empty() {
        if let Some(m) = &inside_macro {
            // int_impl!・uint_impl! の中の口は整数の型すべてに当たる（core/src/num/{int,uint}_macros.rs）
            if !public {
                return None;
            }
            let types: &[&str] = match m.as_str() {
                "int_impl" => &SIGNED,
                "uint_impl" => &UNSIGNED,
                _ => return None,
            };
            // 1 つ目を返し、残りは呼び手が写す（render で展開する）
            return Some(Item {
                path: format!("{{ints:{}}}::{name}", types.join(",")),
                name,
                owner: Owner::Prim(types.join(",")),
                docs,
                body: Vec::new(),
                krate: krate.to_string(),
            });
        }
        if !public {
            return None;
        }
        owners.push(Owner::Free);
    }
    let owner = owners.pop()?;
    // 別のクレートの型に付けた固有の口（alloc が core の型に付けたものなど）は、clippy がパスを引けないので扱わない
    if let Owner::Type(p) = &owner
        && p.split("::").next() != Some(krate)
    {
        return None;
    }
    let path = match &owner {
        Owner::Prim(p) => format!("{p}::{name}"),
        Owner::Type(p) | Owner::Trait(p) => format!("{p}::{name}"),
        Owner::Free => format!("{module}::{name}"),
    };
    Some(Item { path, name, owner, docs, body: Vec::new(), krate: krate.to_string() })
}

fn has_panics(docs: &str) -> bool {
    docs.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("# Panics") || l.starts_with("# Panic")
    })
}

fn body_has(item: &Item, names: &[&str]) -> bool {
    item.body.iter().any(|t| t.kind == Kind::Ident && names.contains(&t.text.as_str()))
}

/// 整数のマクロの口を型ごとに広げる。
fn expand(items: &[Item]) -> Vec<Item> {
    let mut out = Vec::new();
    for it in items {
        if let Some(rest) = it.path.strip_prefix("{ints:") {
            let (types, name) = rest.split_once("}::").unwrap_or(("", ""));
            for ty in types.split(',') {
                let mut c = it.clone();
                c.path = format!("{ty}::{name}");
                c.owner = Owner::Prim(ty.to_string());
                out.push(c);
            }
        } else {
            out.push(it.clone());
        }
    }
    out
}

/// 分類して clippy.toml の中身を作る。
fn render(rustc: &str, raw: &[Item]) -> String {
    let items = expand(raw);
    let mut reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut add = |path: &str, why: &str| {
        let e = reasons.entry(path.to_string()).or_default();
        if !e.iter().any(|w| w == why) {
            e.push(why.to_string());
        }
    };
    // 1. 落ちうる
    for it in &items {
        if has_panics(&it.docs) {
            add(&it.path, "落ちうる：std の文書に Panics の節がある。範囲を確かめた包みか checked_* を使う");
        }
    }
    // 2. 確保（alloc の固有の口で本体が確保する口を呼ぶもの・alloc で定義したトレイトの口）
    for it in &items {
        if it.krate != "alloc" || it.name.starts_with("try_") {
            continue;
        }
        let alloc_owner = matches!(&it.owner, Owner::Type(_) | Owner::Prim(_));
        let alloc_trait = matches!(&it.owner, Owner::Trait(_));
        if (alloc_owner && (body_has(it, &ALLOC_IDENTS) || returns_owned_new(it))) || alloc_trait {
            add(&it.path, "確保：std の中で数えずに確保する。mem の入れ物を使う（ADR-31）");
        }
    }
    // 3. 浮動小数
    for it in &items {
        if let Owner::Prim(p) = &it.owner
            && (p == "f32" || p == "f64")
            && !FLOAT_OK.contains(&it.name.as_str())
        {
            add(
                &it.path,
                "浮動小数：値が環境・版で変わりうるか、決めた規則と違う。crate::math を使う（determinism.md §2.2・§2.3）",
            );
        }
    }
    // 4. Unicode の表（char・str の口で本体が unicode を引くもの。char の禁じた口を呼ぶ str の口も）
    let mut uni: Vec<String> = Vec::new();
    for _round in 0..4 {
        for it in &items {
            if let Owner::Prim(p) = &it.owner {
                if p != "char" && p != "str" {
                    continue;
                }
                if it.name.contains("ascii") || uni.contains(&it.path) {
                    continue;
                }
                let calls_uni = it.body.iter().any(|t| {
                    t.kind == Kind::Ident
                        && (t.text == "unicode" || uni.iter().any(|u| u.rsplit("::").next() == Some(t.text.as_str())))
                });
                if calls_uni {
                    uni.push(it.path.clone());
                }
            }
        }
    }
    for p in &uni {
        add(p, "Unicode の表：std の表は Rust の版で上がる。自前の表と関数を使う（determinism.md §4.3）");
    }
    // 5. 手で書いた分類
    for (p, why) in HAND {
        add(p, why);
    }
    let int_names: Vec<&str> =
        items.iter().filter(|i| matches!(&i.owner, Owner::Prim(_))).map(|i| i.path.as_str()).collect();
    for ty in SIGNED.iter().chain(UNSIGNED.iter()) {
        for (name, why) in HAND_INT {
            let p = format!("{ty}::{name}");
            if int_names.contains(&p.as_str()) {
                add(&p, why);
            }
        }
    }
    // 許す口を除く
    let mut allowed_note = String::new();
    for (p, why) in ALLOWED {
        if reasons.remove(*p).is_some() {
            let _ = writeln!(allowed_note, "#   {p}：{why}");
        }
    }
    let mut s = String::new();
    let _ = writeln!(s, "# ライブラリ（crates/sumie）の厳しい規則。手で書き換えない：");
    let _ = writeln!(s, "#   cargo run -p sumie-tools -- gen-disallowed        （作り直す）");
    let _ = writeln!(s, "#   cargo run -p sumie-tools -- gen-disallowed --check（照らす。scripts/verify.sh）");
    let _ = writeln!(
        s,
        "# 元：{rustc} の rust-src（core・alloc・std/src/num）。分類は crates/sumie-tools/src/gen_disallowed.rs"
    );
    let _ = writeln!(s, "# 道具には別のゆるい設定（crates/sumie-tools/clippy.toml）。ADR-41 (6)");
    let _ = writeln!(s, "#");
    let _ = writeln!(s, "# 生成したものから外した口（使ってよいと確かめたもの）：");
    s.push_str(&allowed_note);
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "# 試験の関数（#[test]・#[cfg(test)]）では unwrap・expect・panic・添字を許す。結合テストは頭で allow する"
    );
    let _ = writeln!(s, "allow-unwrap-in-tests = true");
    let _ = writeln!(s, "allow-expect-in-tests = true");
    let _ = writeln!(s, "allow-panic-in-tests = true");
    let _ = writeln!(s, "allow-indexing-slicing-in-tests = true");
    let _ = writeln!(s, "allow-dbg-in-tests = true");
    let _ = writeln!(s);
    let _ = writeln!(s, "# 小さいスタック（128 KiB。NFR-03）で回すため、スタックに大きなものを置かない");
    let _ = writeln!(s, "array-size-threshold = 4096");
    let _ = writeln!(s, "stack-size-threshold = 16384");
    let _ = writeln!(s);
    let _ = writeln!(s, "disallowed-methods = [");
    for (p, why) in &reasons {
        let _ = writeln!(s, "    {{ path = \"{p}\", reason = \"{}\" }},", why.join("／").replace('"', "'"));
    }
    let _ = writeln!(s, "]");
    let _ = writeln!(s);
    s.push_str(TYPES_AND_MACROS);
    s
}

/// alloc の型の、新しい入れ物を作って返す関連の関数（`Box::new`・`Arc::new`・`String::from_utf8_lossy` など）か。
fn returns_owned_new(it: &Item) -> bool {
    let Owner::Type(p) = &it.owner else { return false };
    let heap = [
        "Box",
        "Rc",
        "Arc",
        "Vec",
        "String",
        "VecDeque",
        "BinaryHeap",
        "BTreeMap",
        "BTreeSet",
        "LinkedList",
        "CString",
    ];
    let ty = p.rsplit("::").next().unwrap_or("");
    if !heap.contains(&ty) {
        return false;
    }
    // new 系で、本体が Box・RawVec・内部の確保に触れるもの
    it.name.starts_with("new") || it.name.starts_with("from") || it.name == "pin"
}

/// 手で書いた型とマクロ（determinism.md §4・CLAUDE.md）。
const TYPES_AND_MACROS: &str = r#"disallowed-types = [
    { path = "std::collections::HashMap", reason = "順序：反復の順がプロセスごとに変わる（determinism.md §4.1）。mem::SortedMap" },
    { path = "std::collections::HashSet", reason = "順序：同上。mem::SortedVec" },
    { path = "std::hash::RandomState", reason = "順序：同上" },
    { path = "std::hash::DefaultHasher", reason = "順序：ハッシュの値が Rust の版で変わりうる" },
    { path = "alloc::collections::BTreeMap", reason = "確保：std の中で数えずに確保する（ADR-31）。mem::SortedMap" },
    { path = "alloc::collections::BTreeSet", reason = "確保：同上。mem::SortedVec" },
    { path = "alloc::collections::BinaryHeap", reason = "確保：同上" },
    { path = "alloc::collections::VecDeque", reason = "確保：同上" },
    { path = "alloc::collections::LinkedList", reason = "確保：同上" },
    { path = "std::time::Instant", reason = "時刻を読まない（determinism.md §5）" },
    { path = "std::time::SystemTime", reason = "時刻を読まない（determinism.md §5）" },
]

disallowed-macros = [
    { path = "core::assert", reason = "debug だけでなく panic する。誤りを返す（CLAUDE.md）" },
    { path = "core::assert_eq", reason = "同上" },
    { path = "core::assert_ne", reason = "同上" },
    { path = "core::debug_assert", reason = "debug だけで panic し、ビルドで振る舞いが分かれる（CLAUDE.md）" },
    { path = "core::debug_assert_eq", reason = "同上" },
    { path = "core::debug_assert_ne", reason = "同上" },
    { path = "alloc::vec", reason = "確保：std の中で数えずに確保する（ADR-31）" },
    { path = "alloc::format", reason = "確保：同上" },
    { path = "std::println", reason = "出力しない" },
    { path = "std::eprintln", reason = "出力しない" },
    { path = "std::print", reason = "出力しない" },
    { path = "std::eprint", reason = "出力しない" },
    { path = "std::dbg", reason = "出力しない" },
]
"#;
