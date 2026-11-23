//! sumie の照合・計測・生成の道具（architecture.md §10）。本番に入らない。
//!
//! `cargo run -p sumie-tools --release -- <命令> …`

mod bench;
mod bench_sumie;
mod compare;
mod compare_sumie;
mod corpus;
mod corpus_real;
mod gen_disallowed;
mod gen_heavy;
mod image;
mod kura_compat;
mod latency;
mod lint_probe;
mod oracle;
mod oracle_lock;
mod probe;
mod probe_m2;
mod sizes;
mod values;
mod wellformed;

use std::path::PathBuf;

const USAGE: &str = "使い方：sumie-tools <命令> …
  oracle resvg <in.svg> <out.png> [--fonts system|none|<dir>] [--scale S]   resvg（Kura の設定）で描き、使った書体を出す
  oracle chrome <in.svg> <out.png>                                       Chrome のヘッドレスで描く（大きさは resvg と同じ）
  compare [--root corpus] [--filter …] [--limit N] [--jobs N] [--out …]   resvg と Chrome の差の一覧（HTML）と分布
  bench-sumie <file>… [--repeat N] [--scale s] [--tile N]   sumie の段ごとの時間と見積もり（ns／単位）
  compare-sumie [--root corpus] [--cache <dir>,…] [--filter …] [--all] [--limit N] [--jobs N] [--min-side N] [--out …]
                                                                         sumie を resvg・Chrome と比べ、M2 の合否を出す（ADR-101）
  bench [--root corpus|local/heavy] [--repeat N] [--timeout 秒] [--out …]  resvg 単体の時間と確保の山
  corpus import-resvg|fetch-w3c|fetch-real|index …                       照合の集まり（ADR-21・ADR-59。取ったものは照らしてから写す）
  gen-heavy [--out local/heavy]                                          重い入力を作る（architecture.md §10）
  kura-compat --kura <一覧> [--dir <SVG の根>]                           Kura の SvgCodec の結果と照らす（NFR-10）
  render・golden                                                         sumie で描く・ゴールデンを足す（M2 から）
  gen-disallowed [--check]                                               crates/sumie/clippy.toml を std の文書から作る
  check-lint-probe                                                       禁じる口が clippy で本当に拾われるかを確かめる
  oracle-lock --kura <Kura の Cargo.lock> --rev <コミット>                 照合の相手の依存の版を Kura と照らして記録する
  probe [--out tests/svg/m1-probe]                                       M1 の範囲を決める試しを resvg・Chrome で描いて表にする
  wellformed [--root corpus] [--kura-fuzz <dir>] [--mutate N]            XML の整形式の厳しさを roxmltree と照らす
  sizes [--root corpus] [--out local/sizes] [--fonts none]                受ける・断ると大きさを resvg と照らす（FR-OUT-02・NFR-10）
  latency [--scale N] [--filter …]                                       敵対的な入力で、ns／単位と中止を読む間の最大を測る
  values [--root corpus] [--out local/values] [--dump <in.svg>]           id のある要素の計算した値を usvg の木と照らす（ADR-63 (5)）";

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
        "compare-sumie" => compare_sumie::run(rest),
        "bench" => bench::run(rest),
        "bench-one" => bench::one(rest),
        "bench-sumie" => bench_sumie::run(rest),
        "corpus" => corpus::run(rest),
        "gen-heavy" => gen_heavy::run(rest),
        "kura-compat" => kura_compat::run(rest),
        "render" | "golden" => Err(format!("{cmd} は sumie の描く口ができる M2 から")),
        "gen-disallowed" => gen_disallowed::run(rest),
        "check-lint-probe" => lint_probe::run(rest),
        "oracle-lock" => oracle_lock::run(rest),
        "probe" => probe::run(rest),
        "wellformed" => wellformed::run(rest),
        "latency" => latency::run(rest),
        "sizes" => sizes::run(rest),
        "values" => values::run(rest),
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
    corpus::check_args(args, &["--fonts", "--scale"], &[], 3)?;
    let which = args.first().map(String::as_str).unwrap_or("");
    let input = PathBuf::from(args.get(1).ok_or("入力の SVG")?);
    let output = PathBuf::from(args.get(2).ok_or("出力の PNG")?);
    let fonts = oracle::Fonts::parse(corpus::arg(args, "--fonts").unwrap_or("system"))?;
    let scale = corpus::num_arg::<f32>(args, "--scale", 1.0)?;
    if !(scale.is_finite() && scale > 0.0) {
        return Err(format!("--scale は正の有限の数（{scale}）"));
    }
    let svg = std::fs::read(&input).map_err(|e| e.to_string())?;
    // 文字のない SVG ではシステムのフォント（約 840 MB）を読まない（大きさも値もフォントに依らない）
    let has_text = corpus::features(&svg).contains(&"text");
    let resvg = oracle::Resvg::new(if has_text { &fonts } else { &oracle::Fonts::None });
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
            let (cw, ch) = oracle::Resvg::css_size(&tree);
            let chrome = oracle::Chrome::find()?;
            let work = std::env::temp_dir().join(format!("sumie-chrome-{}", std::process::id()));
            let shot = chrome.render(&input, w, h, (cw, ch), &work);
            // 失敗しても作業のフォルダを残さない
            let _ = std::fs::remove_dir_all(&work);
            let (img, attempts) = shot?;
            image::write_png(&output, &img)?;
            println!("{w}×{h}（{}・{attempts} 回）", chrome.version);
            Ok(())
        }
        _ => Err("oracle resvg|chrome <in.svg> <out.png>".into()),
    }
}
