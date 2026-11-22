//! `bench`：resvg 単体の時間と確保の山を測る（ADR-19・ADR-35 (2)・NFR-07。M2 から sumie と並べる）。
//!
//! `bench [--root corpus|local/heavy] [--filter <道の一部>] [--all] [--repeat N] [--timeout 秒] [--fonts system] [--out <file>]`
//!
//! 1 件ずつ子のプロセス（`bench-one`）で測る（遅い入力を時間で止め、確保の山をほかの件と混ぜない）。時間は木を作る分と
//! 描く分の最小（`--repeat` 回）、確保の山は 1 回目の木を作る・描くの間の山（allocation-counter。スレッドごと。ADR-46）。
//! `--timeout` は 1 回あたりの秒（子は `--repeat` 倍で止める）。フォントの集まりを作る時間と量は数えない（Kura の集まりは
//! プロセスで 1 つ）。
//!
//! 計り方の違い（ADR-57 (4)）：時間は数える確保器（allocation-counter。確保ごとに数え、`alloc_zeroed`・`realloc` は既定の
//! 写しになる）のもとで測り、道具の release の設定（workspace の既定）は Kura の設定（codegen-units = 1・thin LTO）と違う。
//! どちらも resvg と sumie を同じ道具で測る比には効きにくいので、比（NFR-07）だけを約束に使い、絶対の時間は参考にする。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use resvg::tiny_skia;
use sumie_testkit::jsonl::Line;

use crate::corpus::{arg, workspace_root};
use crate::oracle::{Fonts, Resvg};

pub fn run(args: &[String]) -> Result<(), String> {
    let ws = workspace_root();
    let root = arg(args, "--root").map(PathBuf::from).unwrap_or_else(|| ws.join("corpus"));
    let repeat = arg(args, "--repeat").and_then(|n| n.parse::<u32>().ok()).unwrap_or(3).max(1);
    let timeout = Duration::from_secs(arg(args, "--timeout").and_then(|n| n.parse::<u64>().ok()).unwrap_or(60));
    let fonts = arg(args, "--fonts").unwrap_or("system");
    let out = arg(args, "--out").map(PathBuf::from).unwrap_or_else(|| ws.join("local/bench/resvg.jsonl"));
    let index = std::fs::read_to_string(root.join("index.jsonl"))
        .map_err(|e| format!("{}/index.jsonl：{e}（先に corpus index）", root.display()))?;
    let all = args.iter().any(|a| a == "--all");
    let filter = arg(args, "--filter");
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let mut lines = String::new();
    let (mut n, mut total_ms, mut timeouts) = (0usize, 0.0f64, 0usize);
    for l in index.lines() {
        let line = Line::parse(l)?;
        let path = line.get_str("path").unwrap_or("");
        if (!all && line.get_str("scope") != Some("in")) || filter.is_some_and(|f| !path.contains(f)) {
            continue;
        }
        let file = root.join(path);
        let r = run_child(&exe, &file, repeat, fonts, timeout.saturating_mul(repeat))?;
        let mut rl = Line::parse(&r).unwrap_or_else(|_| Line::new().str("error", r.as_str()));
        rl.fields.insert(0, ("path".into(), sumie_testkit::jsonl::Value::Str(path.into())));
        if rl.get_bool("timeout") == Some(true) {
            timeouts += 1;
        }
        if let (Some(p), Some(rn)) = (rl.get_int("parse_us"), rl.get_int("render_us")) {
            total_ms += (p + rn) as f64 / 1000.0;
        }
        eprintln!("{}", rl.to_json());
        lines.push_str(&rl.to_json());
        lines.push('\n');
        n += 1;
    }
    std::fs::write(&out, lines).map_err(|e| e.to_string())?;
    println!("{n} 件（時間切れ {timeouts} 件）・合計 {total_ms:.1} ms → {}", out.display());
    Ok(())
}

fn run_child(exe: &Path, file: &Path, repeat: u32, fonts: &str, timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(exe)
        .arg("bench-one")
        .arg(file)
        .args(["--repeat", &repeat.to_string(), "--fonts", fonts])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            let mut s = String::new();
            if let Some(mut o) = child.stdout.take() {
                use std::io::Read;
                let _ = o.read_to_string(&mut s);
            }
            if !st.success() {
                return Ok(Line::new().str("error", format!("子が終わった：{st}")).to_json());
            }
            return Ok(s.trim().to_string());
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Line::new().bool("timeout", true).int("timeout_s", timeout.as_secs()).to_json());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 子のプロセス：1 件を測って JSON の 1 行を出す。
pub fn one(args: &[String]) -> Result<(), String> {
    let file = PathBuf::from(args.first().ok_or("bench-one <file>")?);
    let repeat = arg(args, "--repeat").and_then(|n| n.parse::<u32>().ok()).unwrap_or(3).max(1);
    let fonts = Fonts::parse(arg(args, "--fonts").unwrap_or("system"));
    let svg = std::fs::read(&file).map_err(|e| e.to_string())?;
    // 文字を含むときだけフォントを読む（Kura もフォントの集まりは文字のある SVG で初めて作る）
    let has_text = crate::corpus::features(&svg).contains(&"text");
    let resvg = Resvg::new(if has_text { &fonts } else { &Fonts::None });
    let (mut best_parse, mut best_render) = (u128::MAX, u128::MAX);
    let mut size = (0, 0);
    let mut peak = 0u64;
    for k in 0..repeat {
        let mut err = None;
        let info = allocation_counter::measure(|| {
            let t = Instant::now();
            let tree = match resvg.tree(&svg) {
                Ok(t) => t,
                Err(e) => {
                    err = Some(e);
                    return;
                }
            };
            let parse = t.elapsed().as_micros();
            size = Resvg::size(&tree);
            let t = Instant::now();
            let Some(mut pm) = tiny_skia::Pixmap::new(size.0.max(1), size.1.max(1)) else {
                err = Some(format!("画素の領域を作れない（{}×{}）", size.0, size.1));
                return;
            };
            resvg::render(&tree, tiny_skia::Transform::default(), &mut pm.as_mut());
            let render = t.elapsed().as_micros();
            best_parse = best_parse.min(parse);
            best_render = best_render.min(render);
        });
        if let Some(e) = err {
            println!("{}", Line::new().str("error", e).to_json());
            return Ok(());
        }
        if k == 0 {
            peak = info.bytes_max;
        }
    }
    println!(
        "{}",
        Line::new()
            .int("w", size.0)
            .int("h", size.1)
            .int("parse_us", best_parse as u64)
            .int("render_us", best_render as u64)
            .int("peak_bytes", peak)
            .bool("text", has_text)
            .to_json()
    );
    Ok(())
}
