//! `bench-sumie`：sumie の段ごとの時間と見積もりを測る（M2 から。ADR-19・NFR-07。roadmap M2 の完了条件の S1・S4・地図風の線）。
//!
//! `bench-sumie <file>… [--repeat N] [--scale s] [--tile N]`
//!
//! 1 件ごとに、読む・木を作る・計画・描く（全体）の時間（`--repeat` 回の最小）と、`estimate_parse`・`estimate_build`・
//! `Tree::estimate` の手間を並べ、時間 ÷ 手間（ns／単位）を出す。`--tile N` なら N × N のタイルで描いた合計の時間も
//! （範囲の描画。ADR-13）。見積もりが上限を超えれば、どの段で断ったかを書く（描く前に断る）。
//!
//! 見積もりの段（`estimate_parse`・`estimate_build`・`Tree::estimate`・全体の `estimate_region`）も時間を測り、その段が
//! 自分で払った手間の下限（中止の印を読んだ回数 × 2¹⁶。印は払った手間が 2¹⁶ の倍数を越えるたびに読む）で割った ns／単位の
//! 上限を出す（見積もりの段の時間が `estimate_work` の上限で抑えられているか。単位 4 のレビュー）。払った手間が 2¹⁶ に
//! 満たない段は、ns／単位を出さない。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use sumie_testkit::jsonl::Line;

use crate::corpus::{arg, num_arg};

fn min_time<T>(repeat: u32, mut f: impl FnMut() -> Result<T, String>) -> Result<(T, f64), String> {
    let mut best = f64::INFINITY;
    let mut out = None;
    for _ in 0..repeat {
        let t = Instant::now();
        let r = f()?;
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
        out = Some(r);
    }
    out.map(|o| (o, best)).ok_or_else(|| "回数が 0".to_string())
}

/// 中止の印を読んだ回数を数える（印は立てない）。
#[derive(Default)]
struct Polls(AtomicU64);

impl sumie::Cancel for Polls {
    fn cancelled(&self) -> bool {
        self.0.fetch_add(1, Ordering::Relaxed);
        false
    }
}

/// 中止の印を読む手間の間隔（sumie の `CANCEL_INTERVAL`）。
const POLL_INTERVAL: u64 = 1 << 16;

/// 見積もりの段を `repeat` 回呼んだ最小の時間と、1 回で払った手間の下限（印を読んだ回数 × 間隔）を行に足す。
fn estimate_stage<T>(
    l: Line,
    key: &str,
    repeat: u32,
    f: impl Fn(&dyn sumie::Cancel) -> Result<T, sumie::Error>,
) -> Line {
    let polls = Polls::default();
    let _ = f(&polls);
    let work = polls.0.load(Ordering::Relaxed).saturating_mul(POLL_INTERVAL);
    let Ok(((), ms)) = min_time(repeat, || {
        let _ = f(&sumie::Never);
        Ok(())
    }) else {
        return l;
    };
    let l = l.str(&format!("{key}_ms"), format!("{ms:.3}")).int(&format!("{key}_self_work_min"), work);
    if work > 0 { l.str(&format!("{key}_ns_per_unit_max"), format!("{:.3}", ms * 1e6 / work as f64)) } else { l }
}

pub fn run(args: &[String]) -> Result<(), String> {
    crate::corpus::check_args(args, &["--repeat", "--scale", "--tile"], &[], usize::MAX)?;
    let repeat = num_arg::<u32>(args, "--repeat", 3)?.max(1);
    let scale: f64 = arg(args, "--scale").map(|s| s.parse().map_err(|e| format!("{e}"))).transpose()?.unwrap_or(1.0);
    if !(scale.is_finite() && scale > 0.0) {
        return Err(format!("--scale は正の有限の数（{scale}）"));
    }
    let tile = num_arg::<u32>(args, "--tile", 0)?;
    let files: Vec<&String> = {
        let mut v = Vec::new();
        let mut skip = false;
        for a in args {
            if skip {
                skip = false;
                continue;
            }
            if a.starts_with("--") {
                skip = true;
                continue;
            }
            v.push(a);
        }
        v
    };
    for f in files {
        let svg = std::fs::read(f).map_err(|e| format!("{f}：{e}"))?;
        let mut l = Line::new().str("file", f.as_str());
        let popts = sumie::ParseOptions::DEFAULT;
        let pe = sumie::estimate_parse(&svg, &popts, &sumie::Never);
        let (doc, parse_ms) =
            match min_time(repeat, || sumie::parse(&svg, &popts, &sumie::Never).map_err(|e| format!("{e:?}"))) {
                Ok(x) => x,
                Err(e) => {
                    println!("{}", l.str("refused", format!("parse：{e}")).to_json());
                    continue;
                }
            };
        let parse_work = pe.map(|e| e.work).unwrap_or(0);
        l = estimate_stage(l, "estimate_parse", repeat, |c| sumie::estimate_parse(&svg, &popts, c));
        l = estimate_stage(l, "estimate_build", repeat, |c| doc.estimate_build(c));
        l = l
            .int("parse_work", parse_work)
            .str("parse_ms", format!("{parse_ms:.3}"))
            .str("parse_ns_per_unit", format!("{:.3}", parse_ms * 1e6 / parse_work.max(1) as f64));
        let be = match doc.estimate_build(&sumie::Never) {
            Ok(e) => e,
            Err(e) => {
                println!("{}", l.str("refused", format!("estimate_build：{e:?}")).to_json());
                continue;
            }
        };
        let (tree, build_ms) = match min_time(repeat, || doc.build(&sumie::Never).map_err(|e| format!("{e:?}"))) {
            Ok(x) => x,
            Err(e) => {
                println!("{}", l.int("build_work", be.work).str("refused", format!("build：{e}")).to_json());
                continue;
            }
        };
        l = l
            .int("build_work", be.work)
            .str("build_ms", format!("{build_ms:.3}"))
            .str("build_ns_per_unit", format!("{:.3}", build_ms * 1e6 / be.work.max(1) as f64));
        let ropts = sumie::RenderOptions { scale, ..sumie::RenderOptions::DEFAULT };
        l = estimate_stage(l, "estimate_render", repeat, |c| tree.estimate(&ropts, c));
        let re = match tree.estimate(&ropts, &sumie::Never) {
            Ok(e) => e,
            Err(e) => {
                // 上限を超えた見積もりは、要る量を段の手間の欄にも書く
                if let sumie::Error::Limit(le) = &e {
                    let key = if le.stage == sumie::Stage::Render { "render_work" } else { "plan_work" };
                    l = l.int(key, le.needed);
                }
                println!("{}", l.str("refused", format!("Tree::estimate：{e:?}")).to_json());
                continue;
            }
        };
        let (plan, plan_ms) = match min_time(repeat, || tree.plan(&ropts, &sumie::Never).map_err(|e| format!("{e:?}")))
        {
            Ok(x) => x,
            Err(e) => {
                println!("{}", l.int("plan_work", re.plan.work).str("refused", format!("plan：{e}")).to_json());
                continue;
            }
        };
        let size = plan.pixel_size();
        let full = sumie::PixelRect { x: 0, y: 0, width: size.width, height: size.height };
        l = estimate_stage(l, "estimate_region", repeat, |c| plan.estimate_region(full, c));
        let mut px = vec![0u8; size.width as usize * size.height as usize * 4];
        let ((), render_ms) = min_time(repeat, || {
            let mut out = sumie::PixmapMut::new(&mut px, size.width, size.height, size.width as usize * 4)
                .map_err(|e| format!("{e:?}"))?;
            plan.render(&mut out, &sumie::Never).map_err(|e| format!("{e:?}"))
        })?;
        l = l
            .int("width", size.width)
            .int("height", size.height)
            .int("plan_work", re.plan.work)
            .str("plan_ms", format!("{plan_ms:.3}"))
            .str("plan_ns_per_unit", format!("{:.3}", plan_ms * 1e6 / re.plan.work.max(1) as f64))
            .int("render_work", re.render.work)
            .str("render_ms", format!("{render_ms:.3}"))
            .str("render_ns_per_unit", format!("{:.3}", render_ms * 1e6 / re.render.work.max(1) as f64));
        if tile > 0 {
            let ((), tiles_ms) = min_time(repeat, || {
                let mut buf = vec![0u8; tile as usize * tile as usize * 4];
                let mut y = 0;
                while y < size.height {
                    let mut x = 0;
                    while x < size.width {
                        let rect = sumie::PixelRect {
                            x,
                            y,
                            width: tile.min(size.width - x),
                            height: tile.min(size.height - y),
                        };
                        let mut out = sumie::PixmapMut::new(&mut buf, rect.width, rect.height, rect.width as usize * 4)
                            .map_err(|e| format!("{e:?}"))?;
                        plan.render_region(rect, &mut out, &sumie::Never).map_err(|e| format!("{e:?}"))?;
                        x += tile;
                    }
                    y += tile;
                }
                Ok(())
            })?;
            l = l.int("tile", tile).str("tiles_ms", format!("{tiles_ms:.3}"));
        }
        println!("{}", l.to_json());
    }
    Ok(())
}
