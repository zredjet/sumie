//! sumie の照合・計測・生成の道具（architecture.md §10）。本番に入らない。
//!
//! `cargo run -p sumie-tools --release -- <命令> …`

mod gen_disallowed;
mod lint_probe;

const USAGE: &str = "使い方：sumie-tools <命令> …
  gen-disallowed [--check]   crates/sumie/clippy.toml を std の文書から作る（--check は照らすだけ）
  check-lint-probe           禁じる口が clippy で本当に拾われるかを試しのクレートで確かめる";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let rest = &args[1..];
    let result = match cmd.as_str() {
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
