//! `compare-sumie`：sumie の描いたものを resvg・Chrome と比べる（M2 から。ADR-15・ADR-56 (1)・ADR-61・ADR-89 (2)）。
//!
//! `compare-sumie [--root corpus] [--filter <道の一部>] [--all] [--limit N] [--jobs N] [--min-side N] [--cache <dir>,…]
//! [--out <dir>]`
//!
//! - 集まり：照合の集まりの `scope` が in のもの（`--all` ですべて）のうち、後のマイルストーンの機能の印（文字・フィルタ・
//!   グラデーション・pattern・mask・clip・marker・画像・`foreignObject`・blend・未定義の振る舞い・animation・script）の
//!   ないもの。sumie が `NotYet` を返したものは理由ごとに数えて外す（ADR-89 (2)）。ほかの断り（`Unsupported` の画像・入れ子の
//!   SVG・文字など）は外さずに「描けない」に数える
//! - 倍率は `compare` と同じ（長い辺が `--min-side` に満たなければ整数倍）。resvg と Chrome の絵は `--cache` のフォルダ
//!   （`compare` の出力。`report.jsonl` と `img/<id>-a.png`・`-b.png`）に同じ道・同じ入力のダイジェスト・同じ倍率のものが
//!   あれば使う（ダイジェストのない古い行は使わない）。フォルダがなければ誤り
//! - 測るもの：sumie と resvg・sumie と Chrome・resvg と Chrome の差（`compare::measure`）。合否は sumie と resvg に
//!   ADR-15 の図形と単色の基準（8/255・2%・平らな所 2/255）を当てる。resvg と Chrome が基準の内なのに sumie が両方から
//!   外れるものを不具合の候補にする。Chrome が撮れなければ sumie と resvg の合否は測り、Chrome との差だけを空にする。
//!   sumie と resvg の大きさが違うものは測れたものに入れて外れに数える。resvg が断ったものも sumie を走らせ、両方が
//!   断ったか片方だけかを書く（両方が断るものは集まりの外として分ける）
//! - M2 の合否（ADR-101。ユーザーの判断）：Chrome を撮れたもののうち、sumie と Chrome が通る数 ≥ resvg と Chrome が通る数、
//!   かつ不具合の候補がすべて既知の差（[`KNOWN`]。ADR に方式の差の理由を書いたもの）なら合格
//! - 出力：`<out>/report.jsonl`・`<out>/summary.txt`・`<out>/index.html`（sumie と resvg の差の大きい順。絵は `<out>/img/`）。
//!   `--out` は `--cache` のフォルダと同じにできない（初めに絵を消すので）

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use sumie_testkit::jsonl::Line;

use crate::compare::{DEFAULT_MIN_SIDE, Metrics, measure};
use crate::corpus::{arg, workspace_root};
use crate::image::{self, Rgba};
use crate::oracle::{Chrome, Fonts, Resvg};

/// 後のマイルストーンの機能の印（ADR-89 (2)）。
const LATER: &[&str] = &[
    "text",
    "filter",
    "gradient",
    "pattern",
    "mask",
    "clip",
    "marker",
    "image",
    "foreignObject",
    "blend",
    "ub",
    "animation",
    "script",
];

/// 既知の差（道と、方式の差の理由を書いた ADR）：resvg と Chrome は近いのに sumie が両方から外れるが、方式の差として認めた
/// もの（ADR-101）。ここにない不具合の候補が 1 つでもあれば M2 の合否は不合格。
const KNOWN: &[(&str, &str)] = &[
    ("real/flag-icons/flags/4x3/bt.svg", THIN),
    ("real/flag-icons/flags/4x3/eac.svg", THIN),
    ("real/flag-icons/flags/4x3/ec.svg", THIN),
    ("real/flag-icons/flags/4x3/va.svg", THIN),
    ("real/tabler-icons/icons/outline/creative-commons-off.svg", THICK),
    ("real/tabler-icons/icons/outline/fish-off.svg", THICK),
    ("real/tabler-icons/icons/outline/om.svg", THICK),
    ("real/tabler-icons/icons/outline/rainbow-off.svg", THICK),
    ("real/tabler-icons/icons/outline/square-rounded-chevrons-up.svg", THICK),
    ("real/tabler-icons/icons/outline/storm.svg", THICK),
    ("real/tabler-icons/icons/outline/wash-dry-3.svg", THICK),
    ("real/tabler-icons/icons/outline/world-map-off.svg", THICK),
];

/// 曲線の太い線の縁（ADR-97 (3)）。
const THICK: &str = "ADR-97 (3)・ADR-101 (5)：曲線の太い線の縁の反エイリアス（外れた画素はすべて縁の半端な画素。4 倍で描いて縮めた絵に sumie がいちばん近い）";
/// 細い線と線の縁（ADR-97 (2)(3)）。
const THIN: &str = "ADR-97 (2)(3)・ADR-101 (5)：細い線と曲線の線の縁の反エイリアス（外れた画素はすべて色の境。線を消すと Chrome との差は 2% 前後以下）";

/// 照合の集まりの 1 件。
#[derive(Debug, Clone)]
struct Item {
    path: String,
    features: String,
}

/// 1 件の結果。
#[derive(Debug, Clone, Default)]
struct Outcome {
    id: usize,
    path: String,
    /// 外した理由（集まりに入らない）
    excluded: String,
    /// 描けなかった理由
    error: String,
    /// Chrome が撮れなかった理由（sumie と resvg の差は測る）
    chrome_error: String,
    /// sumie と resvg の大きさが違う（測れたものに入れて外れに数える）
    size_mismatch: String,
    /// Chrome を撮った回数（やり直し）と、CSS の大きさが整数でないか（Chrome が引き伸ばす。`compare` と同じ）
    chrome_attempts: u32,
    fractional: bool,
    sr: Option<Metrics>,
    sc: Option<Metrics>,
    rc: Option<Metrics>,
    scale: u32,
    /// 描いた入力の BLAKE3（この出力を次の回の `--cache` に使えるように）
    input: String,
}

/// 基準（図形と単色。ADR-56 (1)・ADR-61）。
fn passes(m: &Metrics) -> bool {
    let ratio = if m.n_all == 0 { 0.0 } else { m.over8 as f64 / m.n_all as f64 };
    ratio <= 0.02 && m.flat_max <= 2
}

fn ratio8(m: &Metrics) -> f64 {
    if m.n_all == 0 { 0.0 } else { m.over8 as f64 / m.n_all as f64 }
}

fn load_items(root: &Path, args: &[String]) -> Result<Vec<Item>, String> {
    let index = root.join("index.jsonl");
    let text = std::fs::read_to_string(&index).map_err(|e| format!("{}：{e}", index.display()))?;
    let all = args.iter().any(|a| a == "--all");
    let filter = arg(args, "--filter");
    let mut out = Vec::new();
    for l in text.lines() {
        let line = Line::parse(l)?;
        let path = line.get_str("path").unwrap_or("").to_string();
        if !all && line.get_str("scope") != Some("in") {
            continue;
        }
        if filter.is_some_and(|f| !path.contains(f)) {
            continue;
        }
        out.push(Item { path, features: line.get_str("features").unwrap_or("").to_string() });
    }
    if arg(args, "--limit").is_some() {
        out.truncate(crate::corpus::num_arg::<usize>(args, "--limit", 0)?);
    }
    Ok(out)
}

/// `compare` の出力の絵の 1 つ（フォルダ・id・倍率・入力のダイジェスト）。
#[derive(Debug, Clone)]
struct Cached {
    dir: PathBuf,
    id: u64,
    scale: u64,
    input: String,
}

/// `compare` の出力の絵の在りか（道 → 写しの並び。フォルダの順）。フォルダか `report.jsonl` がなければ誤り。
fn load_cache(dirs: &[PathBuf]) -> Result<BTreeMap<String, Vec<Cached>>, String> {
    let mut out: BTreeMap<String, Vec<Cached>> = BTreeMap::new();
    for d in dirs {
        let report = d.join("report.jsonl");
        let text = std::fs::read_to_string(&report).map_err(|e| format!("--cache：{}：{e}", report.display()))?;
        for l in text.lines() {
            let Ok(line) = Line::parse(l) else { continue };
            let (Some(p), Some(id), Some(k), Some(input)) =
                (line.get_str("path"), line.get_int("id"), line.get_int("scale"), line.get_str("input"))
            else {
                continue;
            };
            if input.is_empty() {
                continue;
            }
            out.entry(p.to_string()).or_default().push(Cached {
                dir: d.clone(),
                id: id as u64,
                scale: k as u64,
                input: input.to_string(),
            });
        }
    }
    Ok(out)
}

/// sumie で描く（倍率 `k`）。`NotYet` は Ok(Err(理由))（外す）。ほかの断りは Err（描けない）。
fn render_sumie(svg: &[u8], k: u32) -> Result<Result<Rgba, String>, String> {
    let doc = sumie::parse(svg, &sumie::ParseOptions::DEFAULT, &sumie::Never).map_err(|e| format!("sumie：{e:?}"))?;
    let tree = match doc.build(&sumie::Never) {
        Ok(t) => t,
        Err(sumie::Error::Unsupported(u)) => match u.feature {
            sumie::Feature::NotYet(l) => return Ok(Err(format!("NotYet：{l:?}"))),
            f => return Err(format!("sumie：未対応：{f:?}")),
        },
        Err(e) => return Err(format!("sumie：{e:?}")),
    };
    let opts = sumie::RenderOptions { scale: f64::from(k), ..sumie::RenderOptions::DEFAULT };
    let plan = tree.plan(&opts, &sumie::Never).map_err(|e| format!("sumie：{e:?}"))?;
    let size = plan.pixel_size();
    let mut px = vec![0u8; size.width as usize * size.height as usize * 4];
    let mut out = sumie::PixmapMut::new(&mut px, size.width, size.height, size.width as usize * 4)
        .map_err(|e| format!("sumie：{e:?}"))?;
    plan.render(&mut out, &sumie::Never).map_err(|e| format!("sumie：{e:?}"))?;
    Ok(Ok(Rgba { w: size.width, h: size.height, px }))
}

pub fn run(args: &[String]) -> Result<(), String> {
    crate::corpus::check_args(
        args,
        &["--root", "--out", "--jobs", "--min-side", "--filter", "--limit", "--cache"],
        &["--all"],
        0,
    )?;
    let ws = workspace_root();
    let root = arg(args, "--root").map(PathBuf::from).unwrap_or_else(|| ws.join("corpus"));
    let out_dir = arg(args, "--out").map(PathBuf::from).unwrap_or_else(|| ws.join("local/compare/sumie"));
    let jobs = crate::corpus::num_arg::<usize>(args, "--jobs", 8)?.max(1);
    let min_side = crate::corpus::num_arg::<u32>(args, "--min-side", DEFAULT_MIN_SIDE)?;
    let cache_dirs: Vec<PathBuf> =
        arg(args, "--cache").map(|c| c.split(',').map(PathBuf::from).collect()).unwrap_or_default();
    let cache = load_cache(&cache_dirs)?;
    // 出力のフォルダは初めに絵を消すので、写しのフォルダと同じにしない
    if let Ok(out) = out_dir.canonicalize()
        && cache_dirs.iter().any(|d| d.canonicalize().is_ok_and(|d| d == out))
    {
        return Err(format!(
            "--out（{}）が --cache のフォルダと同じ（初めに絵を消すので写しを失う）",
            out_dir.display()
        ));
    }
    let items = load_items(&root, args)?;
    for f in ["report.jsonl", "summary.txt", "index.html"] {
        let _ = std::fs::remove_file(out_dir.join(f));
    }
    let _ = std::fs::remove_dir_all(out_dir.join("img"));
    std::fs::create_dir_all(out_dir.join("img")).map_err(|e| e.to_string())?;
    let chrome = Chrome::find()?;
    let resvg = Resvg::new(&Fonts::parse("none")?);
    eprintln!("{} 件（絵の写し {} 件）を {jobs} 並びで比べる（{}）", items.len(), cache.len(), chrome.version);
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Outcome>> = Mutex::new(Vec::new());
    std::thread::scope(|s| -> Result<(), String> {
        for t in 0..jobs {
            let (items, next, results, resvg, chrome, root, out_dir, cache) =
                (&items, &next, &results, &resvg, &chrome, &root, &out_dir, &cache);
            let spawned = std::thread::Builder::new().stack_size(256 << 20).spawn_scoped(s, move || {
                let work = out_dir.join(format!("work-{t}"));
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(k) else { break };
                    let o = one(k, item, root, out_dir, &work, resvg, chrome, cache, min_side);
                    if k % 100 == 0 {
                        eprintln!("{k}/{}", items.len());
                    }
                    results.lock().unwrap_or_else(|e| e.into_inner()).push(o);
                }
                let _ = std::fs::remove_dir_all(&work);
            });
            spawned.map_err(|e| format!("働き手を作れない：{e}"))?;
        }
        Ok(())
    })?;
    let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    results.sort_by_key(|o| o.id);
    write_outputs(&out_dir, &results, &chrome.version)?;
    println!("{} に report.jsonl・summary.txt・index.html を書いた", out_dir.display());
    if let Ok(text) = std::fs::read_to_string(out_dir.join("summary.txt"))
        && let Some(i) = text.find("## M2 の合否")
    {
        println!("{}", text[i..].lines().take(3).collect::<Vec<_>>().join("\n"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn one(
    id: usize,
    item: &Item,
    root: &Path,
    out_dir: &Path,
    work: &Path,
    resvg: &Resvg,
    chrome: &Chrome,
    cache: &BTreeMap<String, Vec<Cached>>,
    min_side: u32,
) -> Outcome {
    let mut o = Outcome { id, path: item.path.clone(), scale: 1, ..Outcome::default() };
    if let Some(f) = item.features.split(',').find(|f| LATER.contains(f)) {
        o.excluded = format!("印：{f}");
        return o;
    }
    let path = root.join(&item.path);
    let svg = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            o.error = e.to_string();
            return o;
        }
    };
    let (k, css) = match resvg.tree(&svg) {
        Ok(t) => {
            let (w, h) = Resvg::size(&t);
            let k = crate::compare::scale_for(w, h, min_side);
            let (cw, ch) = Resvg::css_size(&t);
            o.fractional = cw.fract() != 0.0 || ch.fract() != 0.0;
            (k, (cw * k as f32, ch * k as f32))
        }
        Err(e) => {
            // 両方が断ったか、resvg だけかを書く
            o.error = match render_sumie(&svg, 1) {
                Ok(Ok(_)) => format!("resvg だけが断る：{e}"),
                Ok(Err(reason)) => {
                    o.excluded = reason;
                    return o;
                }
                Err(s) => format!("両方が断る：resvg：{e}・{s}"),
            };
            return o;
        }
    };
    o.scale = k;
    let c = match render_sumie(&svg, k) {
        Ok(Ok(c)) => c,
        Ok(Err(reason)) => {
            o.excluded = reason;
            return o;
        }
        Err(e) => {
            o.error = e;
            return o;
        }
    };
    let digest = blake3::hash(&svg).to_hex().to_string();
    o.input = digest.clone();
    let cached = cache.get(&item.path).and_then(|v| v.iter().find(|c| c.scale == u64::from(k) && c.input == digest));
    let from_cache = |suffix: &str| -> Option<Rgba> {
        let c = cached?;
        image::read_png(&c.dir.join("img").join(format!("{}-{suffix}.png", c.id))).ok()
    };
    let a = match from_cache("a") {
        Some(a) => a,
        None => match resvg.render(&svg, k as f32) {
            Ok(r) => r.img,
            Err(e) => {
                o.error = format!("resvg：{e}");
                return o;
            }
        },
    };
    let b = match from_cache("b").filter(|b| b.w == a.w && b.h == a.h) {
        Some(b) => Some(b),
        None => match chrome.render(&path, a.w, a.h, css, work) {
            Ok((b, attempts)) => {
                o.chrome_attempts = attempts;
                Some(b)
            }
            Err(e) => {
                o.chrome_error = format!("chrome：{e}");
                None
            }
        },
    };
    let img = out_dir.join("img");
    let _ = image::write_png(&img.join(format!("{id}-s.png")), &c);
    let _ = image::write_png(&img.join(format!("{id}-a.png")), &a);
    if let Some(b) = &b {
        let _ = image::write_png(&img.join(format!("{id}-b.png")), b);
    }
    match measure(&c, &a) {
        Some((m, d)) => {
            let _ = image::write_png(&img.join(format!("{id}-d.png")), &d);
            o.sr = Some(m);
        }
        None => o.size_mismatch = format!("大きさが違う（sumie {}×{}・resvg {}×{}）", c.w, c.h, a.w, a.h),
    }
    if let Some(b) = &b {
        o.sc = measure(&c, b).map(|x| x.0);
        o.rc = measure(&a, b).map(|x| x.0);
    }
    o
}

fn metrics_line(l: Line, key: &str, m: &Option<Metrics>) -> Line {
    match m {
        Some(m) => l
            .int(&format!("{key}_over8"), m.over8)
            .int(&format!("{key}_flat_max"), m.flat_max)
            .int(&format!("{key}_max"), m.max_diff)
            .bool(&format!("{key}_pass"), passes(m)),
        None => l,
    }
}

fn write_outputs(out_dir: &Path, results: &[Outcome], chrome: &str) -> Result<(), String> {
    let mut jsonl = String::new();
    for o in results {
        let mut l = Line::new()
            .int("id", o.id as u64)
            .str("path", o.path.as_str())
            .int("scale", o.scale)
            .str("input", o.input.as_str());
        if let Some(m) = &o.sr {
            l = l.int("w", m.w).int("h", m.h).int("n_all", m.n_all);
        }
        l = metrics_line(l, "sr", &o.sr);
        l = metrics_line(l, "sc", &o.sc);
        l = metrics_line(l, "rc", &o.rc);
        l = l
            .str("excluded", o.excluded.as_str())
            .str("error", o.error.as_str())
            .str("size_mismatch", o.size_mismatch.as_str())
            .str("chrome_error", o.chrome_error.as_str())
            .int("chrome_attempts", u64::from(o.chrome_attempts))
            .bool("fractional_size", o.fractional);
        jsonl.push_str(&l.to_json());
        jsonl.push('\n');
    }
    std::fs::write(out_dir.join("report.jsonl"), jsonl).map_err(|e| e.to_string())?;

    let mut s = String::new();
    let _ = writeln!(
        s,
        "sumie（{}）と resvg 0.48.1・{chrome} の差（乗算済み RGBA8）。CPU：{}\n",
        sumie::RENDERER,
        crate::corpus::cpu_name()
    );
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    for o in results.iter().filter(|o| !o.excluded.is_empty()) {
        *reasons.entry(o.excluded.clone()).or_default() += 1;
    }
    let set: Vec<&Outcome> = results.iter().filter(|o| o.excluded.is_empty()).collect();
    let marked = results.iter().filter(|o| o.excluded.starts_with("印：")).count();
    let _ = writeln!(s, "## 集まり（ADR-89 (2)）");
    let _ = writeln!(s, "全 {} 件・後の機能の印で外す {marked} 件・M2 の集まり {} 件", results.len(), set.len());
    let _ = writeln!(s, "外した理由：");
    for (r, n) in &reasons {
        let _ = writeln!(s, "  {r}：{n}");
    }
    // 測れたもの：sumie と resvg の差を測ったものと、大きさが違ったもの（外れに数える）
    let measured: Vec<&Outcome> =
        set.iter().copied().filter(|o| o.sr.is_some() || !o.size_mismatch.is_empty()).collect();
    let no_chrome = measured.iter().filter(|o| !o.chrome_error.is_empty()).count();
    let unmeasured: Vec<&Outcome> =
        set.iter().copied().filter(|o| o.sr.is_none() && o.size_mismatch.is_empty()).collect();
    let both: Vec<&&Outcome> = unmeasured.iter().filter(|o| o.error.starts_with("両方が断る")).collect();
    let resvg_only: Vec<&&Outcome> = unmeasured.iter().filter(|o| o.error.starts_with("resvg だけが断る")).collect();
    let errors: Vec<&&Outcome> = unmeasured
        .iter()
        .filter(|o| !o.error.starts_with("両方が断る") && !o.error.starts_with("resvg だけが断る"))
        .collect();
    let _ = writeln!(
        s,
        "\n## sumie と resvg（ADR-15 の図形と単色の基準：8/255 を超える画素が 2% 以下・平らな所の差 2/255 以下）"
    );
    let pass = measured.iter().filter(|o| o.sr.as_ref().is_some_and(passes)).count();
    let mismatched = measured.iter().filter(|o| !o.size_mismatch.is_empty()).count();
    let _ = writeln!(
        s,
        "測れた {} 件（うち大きさが違う {mismatched} 件は外れ）・sumie が描けない {} 件・両方が断る {} 件・resvg だけが断る {} 件",
        measured.len(),
        errors.len(),
        both.len(),
        resvg_only.len()
    );
    let _ = writeln!(s, "基準で通る {pass}/{}", measured.len());
    if no_chrome > 0 {
        let _ = writeln!(s, "Chrome が撮れなかった {no_chrome} 件（sumie と resvg の合否には入れ、Chrome との差は空）");
    }
    let retried = measured.iter().filter(|o| o.chrome_attempts > 1).count();
    let fractional = measured.iter().filter(|o| o.fractional).count();
    let _ = writeln!(
        s,
        "Chrome をやり直して撮れたもの：{retried} 件・大きさが整数でないもの（Chrome が引き伸ばす）：{fractional} 件"
    );
    let goal = measured.iter().filter(|o| o.sr.as_ref().is_some_and(|m| ratio8(m) <= 0.005 && m.flat_max <= 2)).count();
    let _ = writeln!(s, "目標（0.5%）で通る {goal}/{}", measured.len());
    let mut r: Vec<f64> = measured.iter().filter_map(|o| o.sr.as_ref().map(ratio8)).collect();
    r.sort_by(f64::total_cmp);
    if !r.is_empty() {
        let p = |q: f64| r[((r.len() - 1) as f64 * q).round() as usize] * 100.0;
        let _ = writeln!(
            s,
            "8/255 を超える画素の割合：中央 {:.3}%・90% {:.3}%・99% {:.3}%・最大 {:.3}%",
            p(0.5),
            p(0.9),
            p(0.99),
            p(1.0)
        );
    }
    let flat_bad = measured.iter().filter(|o| o.sr.as_ref().is_some_and(|m| m.flat_max > 2)).count();
    let _ = writeln!(s, "平らな所の差が 2/255 を超える：{flat_bad} 件");
    // Chrome を撮れたものの中で比べる（M2 の合否）
    let with_chrome: Vec<&&Outcome> = measured.iter().filter(|o| o.chrome_error.is_empty()).collect();
    let pass_c = with_chrome.iter().filter(|o| o.sc.as_ref().is_some_and(passes)).count();
    let rc_pass = with_chrome.iter().filter(|o| o.rc.as_ref().is_some_and(passes)).count();
    let _ = writeln!(s, "\n## Chrome との差（Chrome を撮れた {} 件）", with_chrome.len());
    let _ = writeln!(s, "sumie と Chrome が同じ基準で通る {pass_c}・resvg と Chrome が通る {rc_pass}");
    let cand: Vec<&&&Outcome> = with_chrome
        .iter()
        .filter(|o| {
            o.rc.as_ref().is_some_and(passes)
                && !o.sr.as_ref().is_some_and(passes)
                && !o.sc.as_ref().is_some_and(passes)
        })
        .collect();
    let known = |o: &Outcome| KNOWN.iter().find(|(p, _)| *p == o.path).map(|(_, r)| *r);
    let unknown: Vec<&&&&Outcome> = cand.iter().filter(|o| known(o).is_none()).collect();
    let _ = writeln!(
        s,
        "\n## 不具合の候補（resvg と Chrome は近いのに sumie が両方から外れる）：{} 件（既知の差 {}・理由のないもの {}）",
        cand.len(),
        cand.len() - unknown.len(),
        unknown.len()
    );
    for o in &cand {
        match known(o) {
            Some(r) => {
                let _ = writeln!(s, "  {}（id {}）：既知の差：{r}", o.path, o.id);
            }
            None => {
                let _ = writeln!(s, "  {}（id {}）：理由なし", o.path, o.id);
            }
        }
    }
    // KNOWN に挙げたのに候補でなくなったもの（直ったか、集まりから外れた。一覧を見直す）
    let stale: Vec<&str> = KNOWN.iter().map(|(p, _)| *p).filter(|p| !cand.iter().any(|o| o.path == *p)).collect();
    if !stale.is_empty() {
        let _ = writeln!(s, "既知の差に挙げたが候補でないもの（一覧を見直す）：");
        for p in &stale {
            let _ = writeln!(s, "  {p}");
        }
    }
    let ok_count = pass_c >= rc_pass;
    let ok_known = unknown.is_empty();
    let _ = writeln!(
        s,
        "\n## M2 の合否（ADR-101）：{}\n  sumie と Chrome が通る数 {pass_c} ≥ resvg と Chrome が通る数 {rc_pass}：{}\n  理由のない不具合の候補 0 件（{} 件）：{}",
        if ok_count && ok_known { "合格" } else { "不合格" },
        if ok_count { "満たす" } else { "満たさない" },
        unknown.len(),
        if ok_known { "満たす" } else { "満たさない" }
    );
    if !errors.is_empty() {
        let _ = writeln!(s, "\n## sumie が描けない（集まりの中）");
        for o in &errors {
            let _ = writeln!(s, "  {}：{}", o.path, o.error);
        }
    }
    for (title, list) in [("両方が断る（照合の外）", &both), ("resvg だけが断る（sumie は描く）", &resvg_only)]
    {
        if !list.is_empty() {
            let _ = writeln!(s, "\n## {title}");
            for o in list.iter() {
                let _ = writeln!(s, "  {}：{}", o.path, o.error);
            }
        }
    }
    let mism: Vec<&&Outcome> = measured.iter().filter(|o| !o.size_mismatch.is_empty()).collect();
    if !mism.is_empty() {
        let _ = writeln!(s, "\n## 大きさが違う（外れ）");
        for o in &mism {
            let _ = writeln!(s, "  {}：{}", o.path, o.size_mismatch);
        }
    }
    std::fs::write(out_dir.join("summary.txt"), &s).map_err(|e| e.to_string())?;

    // 差の大きい順の一覧
    let mut order: Vec<&&Outcome> = measured.iter().collect();
    order.sort_by(|a, b| {
        let ra = a.sr.as_ref().map_or(0.0, ratio8);
        let rb = b.sr.as_ref().map_or(0.0, ratio8);
        rb.total_cmp(&ra)
    });
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>sumie と resvg・Chrome</title><style>body{font-family:sans-serif}\
         img{max-width:220px;image-rendering:pixelated;background:repeating-conic-gradient(#ccc 0 25%,#fff 0 50%) 0 0/16px 16px}\
         td{vertical-align:top;padding:4px}</style><table><tr><th>道</th><th>sumie</th><th>resvg</th><th>Chrome</th><th>差（sumie と resvg）</th></tr>",
    );
    for o in order {
        let m = o.sr.as_ref();
        let _ = write!(
            html,
            "<tr><td>{}<br>id {}・倍率 {}<br>8/255 超 {:.3}%・平らな所 {}・最大 {}<br>Chrome と {}</td>\
             <td><img src=img/{id}-s.png></td><td><img src=img/{id}-a.png></td><td><img src=img/{id}-b.png></td>\
             <td><img src=img/{id}-d.png></td></tr>",
            o.path.replace('<', "&lt;"),
            o.id,
            o.scale,
            m.map_or(0.0, ratio8) * 100.0,
            m.map_or(0, |m| m.flat_max),
            m.map_or(0, |m| m.max_diff),
            if o.sc.as_ref().is_some_and(passes) { "通る" } else { "外れる" },
            id = o.id
        );
    }
    html.push_str("</table>");
    std::fs::write(out_dir.join("index.html"), html).map_err(|e| e.to_string())?;
    Ok(())
}
