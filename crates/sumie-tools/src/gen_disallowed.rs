//! `gen-disallowed`：ライブラリの禁じる口の一覧（`crates/sumie/clippy.toml`）を、固定したツールチェーンの std の
//! ソース（rust-src）から機械的に作る（roadmap M0・ADR-41 (6)）。
//!
//! 分類（理由に書く）：
//! - **落ちうる**：std の文書に `# Panics` の節がある公開の安定な口（core・alloc・std の浮動小数）。写せなかった口が
//!   あれば、理由を書いた除外の一覧（[`PANICS_UNMAPPED_OK`]）にない限り失敗する（漏れを黙って出さない）
//! - **確保**：alloc の口のうち、本体が確保する口を呼ぶもの・入れ物の型（`String`・`Vec`・`Box` …）を返すもの、と、それらを
//!   呼ぶもの（変わらなくなるまで推移させる。公開でない関数も辺に入れる）。alloc で定義したトレイトの口も
//! - **浮動小数**：`f32`・`f64` の固有の口のうち、determinism.md §2.1 の使ってよい口（と述語）でないもの
//! - **Unicode の表**：`char`・`str` の口のうち、本体が core の Unicode の表（`unicode`）を引くものと、それを呼ぶもの
//!   （推移させる）
//! - **手で書いた分類**：determinism.md・CLAUDE.md の一覧（並べ替え・探索・時刻・環境・スレッド・`Clone::clone`・
//!   `Iterator::collect`・演算子のトレイトの口など。std の文書からは決まらないもの）。core・alloc の口は、走査した口に
//!   あることを確かめる（打ち間違いを黙って無視しない）
//!
//! 生成したものから、理由を付けて許す口（[`ALLOWED`]）を除く（当たらない許す口は誤り）。`--check` は今のファイルと照らす。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use sumie_testkit::rustscan::{Kind, Token, lex, matching};

/// 生成したファイルの置き場（workspace の根から）。
const OUT: &str = "crates/sumie/clippy.toml";

/// 符号つき・符号なしの整数（`int_impl!`・`uint_impl!` の中の口はすべてに当たる）。
const SIGNED: [&str; 6] = ["i8", "i16", "i32", "i64", "i128", "isize"];
const UNSIGNED: [&str; 6] = ["u8", "u16", "u32", "u64", "u128", "usize"];
const PRIMS: [&str; 17] = [
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "char",
    "bool", "str",
];

/// 浮動小数の使ってよい口：determinism.md §2.1 の口（結果が正確に 1 つに決まるもの・ビットの変換）と、値を比べる述語。
const FLOAT_OK: [&str; 21] = [
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
    "is_sign_positive",
    "is_sign_negative",
];

/// 確保の口を呼ぶとみなす本体の字句（識別子）。
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

/// 返すと確保したとみなす入れ物の型。
const HEAP_TYPES: [&str; 11] =
    ["String", "Vec", "Box", "Rc", "Arc", "CString", "VecDeque", "BinaryHeap", "BTreeMap", "BTreeSet", "LinkedList"];

/// 手で書いた分類（std の文書からは決まらないもの）。理由つき。
const HAND: &[(&str, &str)] = &[
    // 確保（std の中で数えずに確保する口。ADR-31）
    (
        "core::clone::Clone::clone",
        "確保：入れ物の clone は std の中で数えずに確保する（`Clone::clone` はすべて禁じ、Arc は mem::share。ADR-41 (6)）",
    ),
    ("core::clone::Clone::clone_from", "確保：同上"),
    ("core::iter::traits::iterator::Iterator::cloned", "確保：要素を clone する（Copy の要素は copied を使う）"),
    ("core::option::Option::cloned", "確保：同上"),
    ("core::result::Result::cloned", "確保：同上"),
    (
        "core::iter::traits::iterator::Iterator::collect",
        "確保：std の中で数えずに確保する（mem の入れ物を使う。ADR-31）",
    ),
    ("core::iter::traits::collect::FromIterator::from_iter", "確保：同上"),
    ("core::iter::traits::collect::Extend::extend", "確保：同上"),
    // 演算子のトレイトの口（arithmetic_side_effects は a / b を拾うが Div::div(a, b) を拾わない）
    ("core::ops::arith::Add::add", "演算子の口：あふれうる。checked_* を使う（arithmetic_side_effects の抜け道）"),
    ("core::ops::arith::Sub::sub", "演算子の口：同上"),
    ("core::ops::arith::Mul::mul", "演算子の口：同上"),
    ("core::ops::arith::Div::div", "演算子の口：同上（0 で panic する）"),
    ("core::ops::arith::Rem::rem", "演算子の口：同上（0 で panic する）"),
    ("core::ops::arith::Neg::neg", "演算子の口：同上"),
    ("core::ops::arith::AddAssign::add_assign", "演算子の口：同上"),
    ("core::ops::arith::SubAssign::sub_assign", "演算子の口：同上"),
    ("core::ops::arith::MulAssign::mul_assign", "演算子の口：同上"),
    ("core::ops::arith::DivAssign::div_assign", "演算子の口：同上"),
    ("core::ops::arith::RemAssign::rem_assign", "演算子の口：同上"),
    ("core::ops::bit::Shl::shl", "演算子の口：桁より大きいずらしで panic する。checked_shl を使う"),
    ("core::ops::bit::Shr::shr", "演算子の口：同上"),
    ("core::ops::bit::ShlAssign::shl_assign", "演算子の口：同上"),
    ("core::ops::bit::ShrAssign::shr_assign", "演算子の口：同上"),
    // 落ちうる（文書に節がないもの）
    (
        "core::char::from_digit",
        "落ちうる：非推奨の包みで、中で radix が 36 を超えると panic する（char::from_digit と同じ）",
    ),
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

/// 生成したものから外す口と、外す理由（使ってよいと確かめたもの）。当たらない許す口は誤り。
const ALLOWED: &[(&str, &str)] = &[
    (
        "core::iter::traits::iterator::Iterator::enumerate",
        "番号は確保した配列の長さ以下で usize を超えない（文書の Panics は usize を超える数の要素のとき）",
    ),
    ("core::iter::traits::iterator::Iterator::count", "同上"),
    ("core::iter::traits::iterator::Iterator::position", "同上"),
    (
        "core::sync::atomic::Atomic::load",
        "中止の印を Relaxed で読むだけ（Release・AcqRel を渡したときだけ panic する。cancel.rs の 1 か所）",
    ),
];

/// 文書に Panics の節があるが、口として写さない（写せない）もの。(ファイル, 関数, 理由)。
const PANICS_UNMAPPED_OK: &[(&str, &str, &str)] = &[
    (
        "core/src/ptr/const_ptr.rs",
        "align_offset",
        "生のポインタの口（ライブラリは unsafe を使わず、生のポインタを作らない）。clippy のパスの書き方も決まらない",
    ),
    ("core/src/ptr/mut_ptr.rs", "align_offset", "同上"),
    (
        "core/src/sync/atomic.rs",
        "load",
        "原子の整数の型（atomic_int! が作る）の口。定義のパスは Atomic::load と同じで、そちらで扱う。原子の型は cancel.rs の AtomicBool だけ（tests/rules.rs）",
    ),
    ("core/src/sync/atomic.rs", "store", "同上（Atomic::store）"),
];

/// std のソースの中の 1 つの関数（辺の点。公開でないものも）。
#[derive(Debug, Clone)]
struct FnInfo {
    name: String,
    krate: String,
    file: String,
    line: u32,
    docs: String,
    body: Vec<Token>,
    /// 返り値の型の字句の識別子
    ret: Vec<String>,
    /// 写した口のパス（写さないものは空）
    paths: Vec<String>,
    /// 持ち主（写したもの）
    owner: Option<Owner>,
    /// 写すべき口か（公開・安定・unsafe でない・トレイトの実装の中でない・関数の中でない）
    should_map: bool,
    /// トレイトの実装の中の口か（推移の辺に入れない）
    in_trait_impl: bool,
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
enum ImplOwner {
    /// 写せる持ち主
    Owner(Owner),
    /// マクロの中の `impl $T`（型はマクロを呼ぶ所から決める）
    MacroVar,
    /// トレイトの実装・写せない型
    None,
}

#[derive(Debug, Clone)]
enum Ctx {
    Impl { owner: ImplOwner, trait_impl: bool, skip: bool },
    Trait { path: String, skip: bool },
    Mod,
    Macro(String),
    Fn { idx: usize, start: usize },
    Other,
}

/// `gen-disallowed [--check]`
pub fn run(args: &[String]) -> Result<(), String> {
    let check = args.iter().any(|a| a == "--check");
    let root = workspace_root()?;
    let (rustc, library) = rust_src()?;
    let mut fns = Vec::new();
    let mut defined: BTreeSet<String> = BTreeSet::new();
    for (krate, sub) in [("core", "core/src"), ("alloc", "alloc/src"), ("std", "std/src/num")] {
        let dir = library.join(sub);
        let mut files = Vec::new();
        collect_rs(&dir, &mut files);
        files.sort();
        for f in files {
            let rel = f.strip_prefix(library.join(format!("{krate}/src"))).map_err(|e| e.to_string())?;
            let module = module_path(krate, rel);
            let src = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
            let file_name = format!("{krate}/src/{}", rel.to_string_lossy());
            defined.extend(type_defs(&module, &lex(&src)));
            fns.extend(scan_file(krate, &module, &file_name, &src));
        }
    }
    drop_undefined_types(&mut fns, &defined);
    let text = render(&rustc, &fns)?;
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

/// ファイルで定義した型とトレイトのパス（`core::cell::RefCell` など）。
fn type_defs(module: &str, t: &[Token]) -> Vec<String> {
    let mut out = Vec::new();
    for k in 0..t.len() {
        if t[k].kind == Kind::Ident
            && matches!(t[k].text.as_str(), "struct" | "enum" | "union" | "trait")
            && let Some(n) = t.get(k + 1).filter(|n| n.kind == Kind::Ident)
        {
            out.push(format!("{module}::{}", n.text));
        }
    }
    out
}

/// 持ち主の型の定義がクレートの中に見つからない口（別のクレートの型への口・解けなかった名前）は写さない
/// （clippy がパスを引けない）。
fn drop_undefined_types(fns: &mut [FnInfo], defined: &BTreeSet<String>) {
    for f in fns {
        if let Some(Owner::Type(p)) = &f.owner
            && !defined.contains(p)
        {
            f.paths.clear();
            f.owner = None;
        }
    }
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
        if k + 1 == comps.len() {
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

/// 1 つのファイルを走査して、すべての関数を返す（写せる公開の口には `paths` が入る）。
fn scan_file(krate: &str, module: &str, file: &str, src: &str) -> Vec<FnInfo> {
    let t = lex(src);
    let names = Names::collect(krate, module, &t);
    let macro_types = macro_invocation_types(&t);
    let mut fns: Vec<FnInfo> = Vec::new();
    let mut stack: Vec<Ctx> = Vec::new();
    // かっこの深さ（{} の文脈ごとに 0 から数え直す）
    let mut paren: Vec<i64> = vec![0];
    let mut start = 0usize;
    let mut i = 0usize;
    while i < t.len() {
        let tok = &t[i];
        if tok.kind == Kind::Punct {
            match tok.text.as_str() {
                // 属性の中（`#[doc = m!{…}]` など）は飛ばす：中の {} を文脈に積まない
                "#" => {
                    let open = if t.get(i + 1).is_some_and(|x| x.is_punct('!')) { i + 2 } else { i + 1 };
                    if t.get(open).is_some_and(|x| x.is_punct('['))
                        && let Some(close) = matching(&t, open)
                    {
                        i = close + 1;
                        continue;
                    }
                }
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
                    if let Some(f) = fn_info(krate, module, file, header, &stack, &macro_types) {
                        fns.push(f);
                    }
                    start = i + 1;
                }
                "{" => {
                    let header = &t[start..i];
                    let ctx = classify(krate, module, file, header, &stack, &names, &macro_types, &mut fns, i + 1);
                    stack.push(ctx);
                    paren.push(0);
                    start = i + 1;
                }
                "}" => {
                    paren.pop();
                    if paren.is_empty() {
                        paren.push(0);
                    }
                    if let Some(Ctx::Fn { idx, start: body_start }) = stack.pop()
                        && let Some(f) = fns.get_mut(idx)
                    {
                        f.body = t[body_start.min(i)..i].to_vec();
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    fns
}

/// マクロの名前 → 呼ぶ所に並んだ基本の型（`from_str_int_impl! { signed isize i8 … }` → isize・i8 …）。
fn macro_invocation_types(t: &[Token]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for k in 0..t.len() {
        if t[k].kind != Kind::Ident || !t.get(k + 1).is_some_and(|x| x.is_punct('!')) {
            continue;
        }
        if k >= 1 && t[k - 1].is_ident("macro_rules") {
            continue;
        }
        let Some(open) = t.get(k + 2).filter(|x| x.is_punct('{') || x.is_punct('(') || x.is_punct('[')).map(|_| k + 2)
        else {
            continue;
        };
        let Some(close) = matching(t, open) else { continue };
        // 型の並びだけを渡す呼び方（識別子が基本の型と signed・unsigned だけ）に限る。ほかの識別子（型の名前・属性）が
        // 混ざる呼び方（atomic_int! など）は、どの型の口になるか字句で決まらないので使わない
        let idents: Vec<&Token> = t[open + 1..close].iter().filter(|x| x.kind == Kind::Ident).collect();
        let only_types =
            idents.iter().all(|x| PRIMS.contains(&x.text.as_str()) || x.is_ident("signed") || x.is_ident("unsigned"));
        let tys: Vec<String> =
            idents.iter().filter(|x| PRIMS.contains(&x.text.as_str())).map(|x| x.text.clone()).collect();
        if only_types && !tys.is_empty() {
            let e = out.entry(t[k].text.clone()).or_default();
            for ty in tys {
                if !e.contains(&ty) {
                    e.push(ty);
                }
            }
        }
    }
    out
}

/// 字句の並びから属性（`#[…]`・`#![…]`）を除き、属性の中身と、文書の中身を返す。
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
                let end = matching(header, j).unwrap_or(header.len().saturating_sub(1));
                let inner = &header[(j + 1).min(end)..end];
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
    // マクロの繰り返しの印（`$(`・`)*`・`,`）が見出しの頭に混ざるのを除く
    let lead = rest
        .iter()
        .take_while(|t| t.kind == Kind::Punct && matches!(t.text.as_str(), "$" | "(" | ")" | "*" | "+" | ","))
        .count();
    rest.drain(..lead);
    (rest, attrs, docs)
}

/// 安定でない・隠した・試験だけの口か。
fn is_skipped(attrs: &[String]) -> bool {
    attrs.iter().any(|a| a.starts_with("unstable") || a == "doc_hidden" || a.starts_with("cfg ( test"))
}

/// 見出しの字句（属性を除いたもの）から、可視性・修飾を除いた残りと、公開かどうか。
fn strip_vis(rest: &[Token]) -> (bool, Vec<Token>) {
    let mut k = 0usize;
    let mut public = false;
    if rest.first().is_some_and(|t| t.is_ident("pub")) {
        k = 1;
        if rest.get(1).is_some_and(|t| t.is_punct('(')) {
            // pub(crate) などは公開でない
            k = matching(rest, 1).map_or(rest.len(), |c| c + 1);
        } else {
            public = true;
        }
    }
    let mut out: Vec<Token> = rest[k.min(rest.len())..].to_vec();
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

/// `{` の前の見出しから文脈を決める（関数なら `fns` に足す）。
#[allow(clippy::too_many_arguments)]
fn classify(
    krate: &str,
    module: &str,
    file: &str,
    header: &[Token],
    stack: &[Ctx],
    names: &Names,
    macro_types: &BTreeMap<String, Vec<String>>,
    fns: &mut Vec<FnInfo>,
    body_start: usize,
) -> Ctx {
    let (rest, attrs, _docs) = split_header(header);
    let skip = is_skipped(&attrs);
    let (public, body) = strip_vis(&rest);
    let Some(first) = body.first() else { return Ctx::Other };
    let unsafe_head = first.is_ident("unsafe");
    let kw = if unsafe_head { body.get(1) } else { Some(first) };
    let rest_after = if unsafe_head { body.get(2..).unwrap_or(&[]) } else { body.get(1..).unwrap_or(&[]) };
    let Some(kw) = kw else { return Ctx::Other };
    if kw.is_ident("impl") {
        let in_fn = stack.iter().any(|c| matches!(c, Ctx::Fn { .. }));
        let (owner, trait_impl) = if in_fn { (ImplOwner::None, false) } else { impl_owner(module, rest_after, names) };
        return Ctx::Impl { owner, trait_impl, skip: skip || unsafe_head };
    }
    if kw.is_ident("trait") {
        let name = rest_after.first().map(|t| t.text.clone()).unwrap_or_default();
        return Ctx::Trait { path: format!("{module}::{name}"), skip: skip || !public || unsafe_head };
    }
    if kw.is_ident("mod") {
        return Ctx::Mod;
    }
    if kw.is_ident("macro_rules") {
        return Ctx::Macro(body.get(2).map(|t| t.text.clone()).unwrap_or_default());
    }
    if kw.is_ident("fn") {
        if let Some(f) = fn_info(krate, module, file, header, stack, macro_types) {
            // 本体は閉じたときに body_start から切り出す
            let idx = fns.len();
            fns.push(f);
            return Ctx::Fn { idx, start: body_start };
        }
        return Ctx::Other;
    }
    Ctx::Other
}

/// `impl` の見出し（`impl` のあと）から持ち主を決める。トレイトの実装は None（トレイトの定義の側で見る）。
fn impl_owner(module: &str, h: &[Token], names: &Names) -> (ImplOwner, bool) {
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
            return (ImplOwner::None, true);
        }
    }
    let Some(first) = h.first() else { return (ImplOwner::None, false) };
    if first.is_punct('$') {
        return (ImplOwner::MacroVar, false);
    }
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
                return (ImplOwner::None, false); // 配列（[T; N]）は扱わない
            }
        }
        return (ImplOwner::Owner(Owner::Prim("slice".to_string())), false);
    }
    if first.kind != Kind::Ident || first.is_ident("dyn") {
        return (ImplOwner::None, false);
    }
    if PRIMS.contains(&first.text.as_str()) {
        return (ImplOwner::Owner(Owner::Prim(first.text.clone())), false);
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
    let Some(name) = segs.last().cloned() else { return (ImplOwner::None, false) };
    if segs.first().is_some_and(|s| s == "crate") && segs.len() > 1 {
        let krate = module.split("::").next().unwrap_or("core");
        let rest = segs[1..].join("::");
        return (ImplOwner::Owner(Owner::Type(format!("{krate}::{rest}"))), false);
    }
    if segs.len() > 1 {
        return (ImplOwner::None, false); // 外のパス（self::・super:: など）は扱わない
    }
    (ImplOwner::Owner(Owner::Type(names.resolve(module, &name))), false)
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
                // type A = B<…>; と type A<T> = B<…>;
                if let Some(a) = t.get(k + 1).filter(|a| a.kind == Kind::Ident) {
                    let mut m = k + 2;
                    while m < t.len() && !t[m].is_punct('=') && !t[m].is_punct(';') {
                        m += 1;
                    }
                    if t.get(m).is_some_and(|e| e.is_punct('='))
                        && let Some(b) = t.get(m + 1).filter(|b| b.kind == Kind::Ident)
                    {
                        aliases.push((a.text.clone(), b.text.clone()));
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

/// `fn` の見出しから関数の情報を作る（写せる公開の口なら `paths` を入れる）。
fn fn_info(
    krate: &str,
    module: &str,
    file: &str,
    header: &[Token],
    stack: &[Ctx],
    macro_types: &BTreeMap<String, Vec<String>>,
) -> Option<FnInfo> {
    let (rest, attrs, docs) = split_header(header);
    let (public, body) = strip_vis(&rest);
    let is_unsafe = body.first().is_some_and(|t| t.is_ident("unsafe"));
    let body: &[Token] = if is_unsafe { body.get(1..).unwrap_or(&[]) } else { &body };
    if !body.first().is_some_and(|t| t.is_ident("fn")) {
        return None;
    }
    let name_tok = body.get(1).filter(|t| t.kind == Kind::Ident)?;
    let name = name_tok.text.clone();
    let line = name_tok.line;
    // 返り値の型の識別子
    let ret = return_idents(body);
    let skipped = is_skipped(&attrs);
    // いちばん近い文脈（Other を飛ばす）
    let mut owner: Option<Owner> = None;
    let mut ctx_skip = false;
    let mut in_trait_impl = false;
    let mut nested = false;
    let mut macro_name: Option<String> = None;
    let mut macro_var = false;
    let mut decided = false;
    for c in stack.iter().rev() {
        match c {
            Ctx::Other => continue,
            Ctx::Fn { .. } => {
                nested = true;
                decided = true;
            }
            Ctx::Impl { owner: o, trait_impl, skip } => {
                ctx_skip = *skip;
                in_trait_impl = *trait_impl;
                match o {
                    ImplOwner::Owner(o) if public => owner = Some(o.clone()),
                    ImplOwner::MacroVar => macro_var = true,
                    _ => {}
                }
                decided = true;
            }
            Ctx::Trait { path, skip } => {
                ctx_skip = *skip;
                owner = Some(Owner::Trait(path.clone()));
                decided = true;
            }
            Ctx::Macro(m) => {
                if macro_name.is_none() {
                    macro_name = Some(m.clone());
                }
                continue;
            }
            Ctx::Mod => {
                if public {
                    owner = Some(Owner::Free);
                }
                decided = true;
            }
        }
        if decided {
            break;
        }
    }
    // マクロの中の impl $T：impl の外のマクロの名前を探す
    if macro_var && macro_name.is_none() {
        macro_name = stack.iter().rev().find_map(|c| if let Ctx::Macro(m) = c { Some(m.clone()) } else { None });
    }
    if !decided && public {
        // ファイルの頭の関数か、int_impl!・uint_impl! の中の口（core/src/num/{int,uint}_macros.rs）
        owner = match macro_name.as_deref() {
            Some("int_impl") | Some("uint_impl") => Some(Owner::Prim(String::new())),
            Some(_) => None,
            None => Some(Owner::Free),
        };
    }
    let mappable = !skipped && !ctx_skip && !is_unsafe && !nested && !in_trait_impl;
    let should_map = mappable && (public || matches!(owner, Some(Owner::Trait(_))));
    let mut paths = Vec::new();
    if mappable {
        let types: Vec<String> = match (macro_name.as_deref(), macro_var, &owner) {
            (Some("int_impl"), _, Some(Owner::Prim(_))) => SIGNED.iter().map(|s| s.to_string()).collect(),
            (Some("uint_impl"), _, Some(Owner::Prim(_))) => UNSIGNED.iter().map(|s| s.to_string()).collect(),
            (Some(m), true, _) if public => macro_types.get(m).cloned().unwrap_or_default(),
            _ => Vec::new(),
        };
        if !types.is_empty() {
            for ty in &types {
                paths.push(format!("{ty}::{name}"));
            }
            owner = Some(Owner::Prim(types.join(",")));
        } else {
            match &owner {
                // 別のクレートの型に付けた固有の口（alloc が core の型に付けたものなど）は、clippy がパスを引けない
                Some(Owner::Type(p)) if p.split("::").next() != Some(krate) => owner = None,
                Some(Owner::Prim(p)) if !p.is_empty() => paths.push(format!("{p}::{name}")),
                Some(Owner::Type(p)) | Some(Owner::Trait(p)) => paths.push(format!("{p}::{name}")),
                Some(Owner::Free) => paths.push(format!("{module}::{name}")),
                _ => {}
            }
        }
    }
    Some(FnInfo {
        name,
        krate: krate.to_string(),
        file: file.to_string(),
        line,
        docs,
        body: Vec::new(),
        ret,
        paths,
        owner,
        should_map,
        in_trait_impl,
    })
}

/// 見出しの `->` のあとの識別子。
fn return_idents(h: &[Token]) -> Vec<String> {
    let Some(arrow) = h.windows(2).position(|w| w[0].is_punct('-') && w[1].is_punct('>')) else { return Vec::new() };
    h[arrow + 2..]
        .iter()
        .take_while(|t| !t.is_ident("where"))
        .filter(|t| t.kind == Kind::Ident)
        .map(|t| t.text.clone())
        .collect()
}

fn has_panics(docs: &str) -> bool {
    docs.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("# Panics") || l.starts_with("# Panic")
    })
}

/// 本体が `names` のどれかを関数として呼ぶか（`名前(` か `名前::<`）。
fn calls_any(body: &[Token], names: &BTreeSet<String>) -> bool {
    body.iter().enumerate().any(|(k, t)| {
        t.kind == Kind::Ident
            && names.contains(&t.text)
            && body.get(k + 1).is_some_and(|n| n.is_punct('(') || n.is_punct(':'))
    })
}

/// 種から始めて、本体が印のついた関数を呼ぶ関数に、変わらなくなるまで印を広げる。印は関数ごと。辺は名前で張るが、
/// その名前の関数が 1 つしかないときだけにする（`new`・`get` のような多くの型が持つ名前を通って、確保しない口にまで
/// 印が広がらないように）。戻り値は `fns` と同じ並びの印。
fn propagate(fns: &[&FnInfo], seed: impl Fn(&FnInfo) -> bool) -> Vec<bool> {
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for f in fns {
        *count.entry(f.name.as_str()).or_default() += 1;
    }
    let mut marked: Vec<bool> = fns.iter().map(|f| seed(f)).collect();
    loop {
        let names: BTreeSet<String> = fns
            .iter()
            .zip(&marked)
            .filter(|(f, m)| **m && count.get(f.name.as_str()) == Some(&1))
            .map(|(f, _)| f.name.clone())
            .collect();
        let mut changed = false;
        for (f, m) in fns.iter().zip(marked.iter_mut()) {
            if !*m && calls_any(&f.body, &names) {
                *m = true;
                changed = true;
            }
        }
        if !changed {
            return marked;
        }
    }
}

/// 分類して clippy.toml の中身を作る。
fn render(rustc: &str, fns: &[FnInfo]) -> Result<String, String> {
    let mut reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut add = |path: &str, why: &str| {
        let e = reasons.entry(path.to_string()).or_default();
        if !e.iter().any(|w| w == why) {
            e.push(why.to_string());
        }
    };
    let mapped: Vec<&FnInfo> = fns.iter().filter(|f| !f.paths.is_empty()).collect();
    let all_paths: BTreeSet<&str> = mapped.iter().flat_map(|f| f.paths.iter().map(String::as_str)).collect();
    // 1. 落ちうる（写せなかった口は除外の一覧にない限り誤り）
    let mut unmapped = Vec::new();
    for f in fns.iter().filter(|f| has_panics(&f.docs)) {
        if !f.paths.is_empty() {
            for p in &f.paths {
                add(p, "落ちうる：std の文書に Panics の節がある。範囲を確かめた包みか checked_* を使う");
            }
        } else if f.should_map && !PANICS_UNMAPPED_OK.iter().any(|(file, n, _)| *file == f.file && *n == f.name) {
            unmapped.push(format!("{}:{} {}", f.file, f.line, f.name));
        }
    }
    if !unmapped.is_empty() {
        return Err(format!(
            "Panics の節がある公開の口のうち、clippy のパスに写せなかったもの（走査を直すか、PANICS_UNMAPPED_OK に理由を書く）：\n{}",
            unmapped.join("\n")
        ));
    }
    // 2. 確保（alloc の関数の辺で推移させる。トレイトの実装の中の口は辺に入れない）
    let alloc_fns: Vec<&FnInfo> = fns.iter().filter(|f| f.krate == "alloc" && !f.in_trait_impl).collect();
    let alloc_marked = propagate(&alloc_fns, |f| {
        f.body.iter().any(|t| t.kind == Kind::Ident && ALLOC_IDENTS.contains(&t.text.as_str()))
            || f.ret.iter().any(|r| HEAP_TYPES.contains(&r.as_str()))
    });
    for (f, m) in alloc_fns.iter().zip(&alloc_marked) {
        if f.paths.is_empty() || f.name.starts_with("try_") {
            continue;
        }
        let trait_in_alloc = matches!(&f.owner, Some(Owner::Trait(_)));
        if trait_in_alloc || *m {
            for p in &f.paths {
                add(p, "確保：std の中で数えずに確保する。mem の入れ物を使う（ADR-31）");
            }
        }
    }
    // 3. 浮動小数
    for f in &mapped {
        if let Some(Owner::Prim(p)) = &f.owner
            && (p == "f32" || p == "f64")
            && !FLOAT_OK.contains(&f.name.as_str())
        {
            for path in &f.paths {
                add(
                    path,
                    "浮動小数：値が環境・版で変わりうるか、決めた規則と違う。crate::math を使う（determinism.md §2.2・§2.3）",
                );
            }
        }
    }
    // 4. Unicode の表（core の関数の辺で推移させる。ASCII の口は除く）
    let core_fns: Vec<&FnInfo> = fns.iter().filter(|f| f.krate == "core" && !f.in_trait_impl).collect();
    let uni = propagate(&core_fns, |f| f.body.iter().any(|t| t.is_ident("unicode")));
    for (f, m) in core_fns.iter().zip(&uni) {
        if let Some(Owner::Prim(p)) = &f.owner
            && (p == "char" || p == "str")
            && !f.name.contains("ascii")
            && *m
        {
            for path in &f.paths {
                add(path, "Unicode の表：std の表は Rust の版で上がる。自前の表と関数を使う（determinism.md §4.3）");
            }
        }
    }
    // 5. 手で書いた分類（core・alloc・基本の型の口は、走査した口にあることを確かめる）
    let mut typos = Vec::new();
    for (p, why) in HAND {
        if !p.starts_with("std::") && !all_paths.contains(p) {
            typos.push(*p);
        }
        add(p, why);
    }
    for ty in SIGNED.iter().chain(UNSIGNED.iter()) {
        for (name, why) in HAND_INT {
            let p = format!("{ty}::{name}");
            if all_paths.contains(p.as_str()) {
                add(&p, why);
            }
        }
    }
    if !typos.is_empty() {
        return Err(format!("手で書いた口が走査した口にない（打ち間違いか、版で消えた）：{typos:?}"));
    }
    // 許す口を除く（当たらないものは誤り）
    let mut allowed_note = String::new();
    let mut unused = Vec::new();
    for (p, why) in ALLOWED {
        if reasons.remove(*p).is_some() {
            let _ = writeln!(allowed_note, "#   {p}：{why}");
        } else {
            unused.push(*p);
        }
    }
    if !unused.is_empty() {
        return Err(format!("許す口が一覧に当たらない（ALLOWED から消す）：{unused:?}"));
    }
    let mut s = String::new();
    let _ = writeln!(s, "# ライブラリ（crates/sumie）の厳しい規則。手で書き換えない：");
    let _ = writeln!(s, "#   cargo run -p sumie-tools -- gen-disallowed        （作り直す）");
    let _ = writeln!(s, "#   cargo run -p sumie-tools -- gen-disallowed --check（照らす。scripts/verify.sh・CI）");
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
    Ok(s)
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
    { path = "core::env", reason = "組むときの環境変数を読む（determinism.md §5）" },
    { path = "core::option_env", reason = "組むときの環境変数を読む（determinism.md §5）" },
    # include! は組み込みのマクロで clippy が拾えないので、tests/rules.rs の走査が拾う
    { path = "std::println", reason = "出力しない" },
    { path = "std::eprintln", reason = "出力しない" },
    { path = "std::print", reason = "出力しない" },
    { path = "std::eprint", reason = "出力しない" },
    { path = "std::dbg", reason = "出力しない" },
]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(file: &str, src: &str) -> Vec<FnInfo> {
        let krate = file.split('/').next().unwrap_or("core");
        let rel = file.split_once("/src/").map(|(_, r)| r).unwrap_or(file);
        scan_file(krate, &module_path(krate, Path::new(rel)), file, src)
    }

    fn paths(fns: &[FnInfo]) -> Vec<String> {
        let mut p: Vec<String> = fns.iter().flat_map(|f| f.paths.clone()).collect();
        p.sort();
        p
    }

    #[test]
    fn inherent_slice_and_trait_impls() {
        let src = "impl<T> [T] {\n/// # Panics\npub fn split_at(&self) {}\nfn private(&self) {}\n}\n\
                   impl Foo for Bar { pub fn x(&self) {} }\npub trait Tr { fn need(&self); fn given(&self) {} }";
        let fns = scan("core/src/slice/mod.rs", src);
        assert_eq!(paths(&fns), ["core::slice::Tr::given", "core::slice::Tr::need", "slice::split_at"]);
        assert!(has_panics(&fns.iter().find(|f| f.name == "split_at").unwrap().docs));
        assert!(fns.iter().any(|f| f.name == "x" && f.in_trait_impl && f.paths.is_empty()));
        assert!(fns.iter().any(|f| f.name == "private" && f.paths.is_empty() && !f.should_map));
    }

    #[test]
    fn macro_impl_with_var_type() {
        let src = "macro_rules! from_str_int_impl {\n ($s:ident $($int_ty:ty)+) => {$(\n impl $int_ty {\n\
                   /// # Panics\n pub const fn from_str_radix(s: &str) -> u8 { 0 }\n }\n )*}\n}\n\
                   from_str_int_impl! { signed isize i8 }\nfrom_str_int_impl! { unsigned u8 }";
        let fns = scan("core/src/num/mod.rs", src);
        assert_eq!(paths(&fns), ["i8::from_str_radix", "isize::from_str_radix", "u8::from_str_radix"]);
    }

    #[test]
    fn int_impl_macro_fns() {
        let src = "macro_rules! uint_impl {\n () => {\n /// # Panics\n pub const fn ilog2(self) -> u32 { 0 }\n }\n}";
        let fns = scan("core/src/num/uint_macros.rs", src);
        assert_eq!(paths(&fns).len(), 6);
        assert!(paths(&fns).contains(&"u32::ilog2".to_string()));
    }

    #[test]
    fn braces_inside_attributes_do_not_open_contexts() {
        let src = "impl str {\n#[doc = m!{ a ? b : c }]\n/// # Panics\npub fn after(&self) {}\n}";
        let fns = scan("core/src/str/mod.rs", src);
        assert_eq!(paths(&fns), ["str::after"]);
    }

    #[test]
    fn use_and_alias_resolution() {
        let src = "use crate::sync::atomic::Atomic;\npub type AtomicBool = Atomic<bool>;\n\
                   impl AtomicBool { pub fn load(&self) -> bool { true } }\n\
                   use core::wtf8::Wtf8;\nimpl Wtf8 { pub fn to_owned(&self) {} }";
        let fns = scan("alloc/src/x.rs", src);
        assert_eq!(paths(&fns), ["alloc::sync::atomic::Atomic::load"]);
        // core の型に alloc が付けた口は写さない
        assert!(fns.iter().any(|f| f.name == "to_owned" && f.paths.is_empty()));
    }

    #[test]
    fn unstable_hidden_and_unsafe_are_skipped() {
        let src = "impl Foo {\n#[unstable(feature = \"x\")]\npub fn u(&self) {}\n#[doc(hidden)]\npub fn h(&self) {}\n\
                   pub unsafe fn s(&self) {}\npub fn ok(&self) {}\n}\nstruct Foo;";
        let fns = scan("core/src/foo.rs", src);
        assert_eq!(paths(&fns), ["core::foo::Foo::ok"]);
    }

    #[test]
    fn propagation_marks_callers() {
        let a = "impl [u8] { pub fn to_ascii_lowercase(&self) -> Vec<u8> { let mut v = self.to_vec(); v } }\n\
                 impl str { pub fn to_ascii_lowercase(&self) -> String { let b = self.as_bytes().to_ascii_lowercase(); x(b) } }\n\
                 fn helper() { inner_alloc() }\nfn inner_alloc() { let v = RawVec::new(); }\npub fn uses() { helper() }";
        let fns = scan("alloc/src/str.rs", a);
        let refs: Vec<&FnInfo> = fns.iter().collect();
        let marked = propagate(&refs, |f| f.body.iter().any(|t| t.is_ident("to_vec") || t.is_ident("RawVec")));
        let is = |n: &str| refs.iter().zip(&marked).filter(|(f, _)| f.name == n).all(|(_, m)| *m);
        // slice の to_ascii_lowercase は種。str の同じ名前の口へは辺を張らない（名前が 2 つある）
        assert!(refs.iter().zip(&marked).any(|(f, m)| f.name == "to_ascii_lowercase" && *m));
        assert!(!refs.iter().zip(&marked).all(|(f, m)| f.name != "to_ascii_lowercase" || *m));
        assert!(is("inner_alloc") && is("helper") && is("uses"));
        assert_eq!(return_idents(&lex("fn f(&self) -> Result<String, E> where T: X")), ["Result", "String", "E"]);
    }

    #[test]
    fn module_paths() {
        assert_eq!(module_path("core", Path::new("num/f64.rs")), "core::num::f64");
        assert_eq!(module_path("core", Path::new("slice/mod.rs")), "core::slice");
        assert_eq!(module_path("alloc", Path::new("lib.rs")), "alloc");
    }
}
