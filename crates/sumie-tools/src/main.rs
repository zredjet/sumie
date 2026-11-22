//! sumie の照合・計測・生成の道具（architecture.md §10）。本番に入らない。
//!
//! `cargo run -p sumie-tools --release -- <命令> …`

mod bench;
mod compare;
mod corpus;
mod gen_disallowed;
mod gen_heavy;
mod image;
mod lint_probe;
mod oracle;

use std::path::PathBuf;

const USAGE: &str = "使い方：sumie-tools <命令> …
  oracle resvg <in.svg> <out.png> [--fonts system|none|<dir>] [--scale S]   resvg（Kura の設定）で描き、使った書体を出す
  oracle chrome <in.svg> <out.png>                                       Chrome のヘッドレスで描く（大きさは resvg と同じ）
  compare [--root corpus] [--filter …] [--limit N] [--jobs N] [--out …]   resvg と Chrome の差の一覧（HTML）と分布
  bench [--root corpus|local/heavy] [--repeat N] [--timeout 秒] [--out …]  resvg 単体の時間と確保の山
  corpus import-resvg|fetch-w3c|index …                                  照合の集まり（ADR-21）
  gen-heavy [--out local/heavy]                                          重い入力を作る（architecture.md §10）
  render・golden                                                         sumie で描く・ゴールデンを足す（M2 から）
  gen-disallowed [--check]                                               crates/sumie/clippy.toml を std の文書から作る
  check-lint-probe                                                       禁じる口が clippy で本当に拾われるかを確かめる";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let rest = &args[1..];
    let result = match cmd.as_str() {
        "oracle" => oracle_cmd(rest),
        "compare" => compare::run(rest),
        "bench" => bench::run(rest),
        "bench-one" => bench::one(rest),
        "corpus" => corpus::run(rest),
        "gen-heavy" => gen_heavy::run(rest),
        "render" | "golden" => Err(format!("{cmd} は sumie の描く口ができる M2 から")),
        "gen-disallowed" => gen_disallowed::run(rest),
        "check-lint-probe" => lint_probe::run(rest),
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("知らない命令：{other}\n{USAGE}")),
    };
    if let Err(e) = result {
        eprintln!("誤り：{e}");
        std::process::exit(1);
    }
}

fn oracle_cmd(args: &[String]) -> Result<(), String> {
    let which = args.first().map(String::as_str).unwrap_or("");
    let input = PathBuf::from(args.get(1).ok_or("入力の SVG")?);
    let output = PathBuf::from(args.get(2).ok_or("出力の PNG")?);
    let fonts = oracle::Fonts::parse(corpus::arg(args, "--fonts").unwrap_or("system"));
    let scale = corpus::arg(args, "--scale").and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
    let svg = std::fs::read(&input).map_err(|e| e.to_string())?;
    let resvg = oracle::Resvg::new(&fonts);
    match which {
        "resvg" => {
            let out = resvg.render(&svg, scale)?;
            image::write_png(&output, &out.img)?;
            println!("{}×{}・使った書体：{:?}", out.img.w, out.img.h, out.fonts_used);
            Ok(())
        }
        "chrome" => {
            let tree = resvg.tree(&svg)?;
            let (w, h) = oracle::Resvg::size(&tree);
            let chrome = oracle::Chrome::find()?;
            let work = std::env::temp_dir().join(format!("sumie-chrome-{}", std::process::id()));
            let img = chrome.render(&input, w, h, &work)?;
            let _ = std::fs::remove_dir_all(&work);
            image::write_png(&output, &img)?;
            println!("{w}×{h}（{}）", chrome.version);
            Ok(())
        }
        _ => Err("oracle resvg|chrome <in.svg> <out.png>".into()),
    }
}
