//! `compare`：2 つの描画（M0 は resvg と Chrome。M2 から sumie）を乗算済みの RGBA8 で比べ、差の一覧の HTML と分布を出す
//! （architecture.md §10・ADR-15・ADR-35 (1)・ADR-44 (3)）。
//!
//! `compare [--root corpus] [--filter <道の一部>] [--all] [--limit N] [--jobs N] [--fonts system|none|<dir>] [--out <dir>]`
//!
//! 測るもの（1 件ごと）：
//! - 大きさ・全画素の数・どちらかの画像で α > 0 の画素の数（ADR-15 の割合の 2 つの分母。ADR-44 (3)）
//! - 各成分の差の最大が 2・8・16 を超える画素の数
//! - 形の内側（両方の画像で 3 × 3 の近傍の α がすべて 0 か 255）の画素の数・差の最大・差が 2 を超える数
//!
//! 出力：`<out>/report.jsonl`（1 件 1 行）・`<out>/summary.txt`（群ごとの分布・ADR-56 (1) の基準と目標で通る数・候補の
//! 値での通る数）・`<out>/index.html`（全画素が分母の割合の大きい順。絵は `<out>/img/`）。
//!
//! 群（ADR-56 (1)）：図形と単色（8/255・2%・内側 2/255）・フィルタとグラデーション（16/255・1%）は基準で合否を出す。
//! 文字（M0 は両方がシステムのフォントで描く）・画像（Kura の設定の resvg は画像を読まない）・未定義の振る舞いの例（題に
//! 「(UB)」）は分布から外して別に数える。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use sumie_testkit::jsonl::Line;

use crate::corpus::{arg, workspace_root};
use crate::image::{self, Rgba};
use crate::oracle::{Chrome, Fonts, Resvg};

/// 1 件の差。
#[derive(Debug, Clone, Default)]
pub struct Metrics {
    pub w: u32,
    pub h: u32,
    pub n_all: u64,
    pub n_alpha: u64,
    pub over2: u64,
    pub over8: u64,
    pub over16: u64,
    pub interior_n: u64,
    pub interior_max: u8,
    pub interior_over2: u64,
    pub max_diff: u8,
}

/// 2 つの乗算済みの画像の差を測る（大きさが違えば None）。差の絵（成分の差の最大を 8 倍した灰色、8 を超える所は赤）も返す。
pub fn measure(a: &Rgba, b: &Rgba) -> Option<(Metrics, Rgba)> {
    if a.w != b.w || a.h != b.h {
        return None;
    }
    let (w, h) = (a.w, a.h);
    let mut m = Metrics { w, h, n_all: u64::from(w) * u64::from(h), ..Metrics::default() };
    let mut diff = Rgba::transparent(w, h);
    // α が 0 か 255 か
    let solid = |img: &Rgba, x: i64, y: i64| -> bool {
        if x < 0 || y < 0 || x >= i64::from(w) || y >= i64::from(h) {
            return true; // 外は透明とみなす
        }
        let al = img.px[((y as usize) * w as usize + x as usize) * 4 + 3];
        al == 0 || al == 255
    };
    for y in 0..h {
        for x in 0..w {
            let i = (y as usize * w as usize + x as usize) * 4;
            let d = (0..4).map(|c| a.px[i + c].abs_diff(b.px[i + c])).max().unwrap_or(0);
            if a.px[i + 3] > 0 || b.px[i + 3] > 0 {
                m.n_alpha += 1;
            }
            m.over2 += u64::from(d > 2);
            m.over8 += u64::from(d > 8);
            m.over16 += u64::from(d > 16);
            m.max_diff = m.max_diff.max(d);
            let interior = (-1..=1).all(|dy| {
                (-1..=1).all(|dx| {
                    solid(a, i64::from(x) + dx, i64::from(y) + dy) && solid(b, i64::from(x) + dx, i64::from(y) + dy)
                })
            });
            if interior {
                m.interior_n += 1;
                m.interior_max = m.interior_max.max(d);
                m.interior_over2 += u64::from(d > 2);
            }
            let g = d.saturating_mul(8);
            diff.px[i..i + 4].copy_from_slice(&if d > 8 { [255, 0, 0, 255] } else { [g, g, g, 255] });
        }
    }
    Some((m, diff))
}

/// 割合（分母 0 なら 0）。
fn ratio(n: u64, d: u64) -> f64 {
    if d == 0 { 0.0 } else { n as f64 / d as f64 }
}

/// 照合の集まりの 1 件。
#[derive(Debug, Clone)]
struct Item {
    path: String,
    category: String,
    features: String,
}

/// 1 件の結果。
#[derive(Debug, Clone)]
struct Outcome {
    item: Item,
    id: usize,
    metrics: Option<Metrics>,
    error: String,
}

fn load_items(root: &Path, args: &[String]) -> Result<Vec<Item>, String> {
    let index = root.join("index.jsonl");
    let text = std::fs::read_to_string(&index)
        .map_err(|e| format!("{}：{e}（先に corpus index --root {}）", index.display(), root.display()))?;
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
        out.push(Item {
            path,
            category: line.get_str("category").unwrap_or("").to_string(),
            features: line.get_str("features").unwrap_or("").to_string(),
        });
    }
    if let Some(n) = arg(args, "--limit").and_then(|n| n.parse::<usize>().ok()) {
        out.truncate(n);
    }
    Ok(out)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let ws = workspace_root();
    let root = arg(args, "--root").map(PathBuf::from).unwrap_or_else(|| ws.join("corpus"));
    let out_dir = arg(args, "--out").map(PathBuf::from).unwrap_or_else(|| ws.join("local/compare/resvg-chrome"));
    let jobs = arg(args, "--jobs").and_then(|n| n.parse::<usize>().ok()).unwrap_or(8).max(1);
    let fonts = Fonts::parse(arg(args, "--fonts").unwrap_or("system"));
    let items = load_items(&root, args)?;
    std::fs::create_dir_all(out_dir.join("img")).map_err(|e| e.to_string())?;
    let chrome = Chrome::find()?;
    eprintln!("フォントを読む…");
    let resvg = Resvg::new(&fonts);
    eprintln!("{} 件を {} 並びで描く（resvg：書体 {}、{}）", items.len(), jobs, resvg.faces(), chrome.version);
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Outcome>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for t in 0..jobs {
            let (items, next, results, resvg, chrome, root, out_dir) =
                (&items, &next, &results, &resvg, &chrome, &root, &out_dir);
            s.spawn(move || {
                let work = out_dir.join(format!("work-{t}"));
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(k) else { break };
                    let o = one(k, item, root, out_dir, &work, resvg, chrome);
                    if k % 100 == 0 {
                        eprintln!("{k}/{}", items.len());
                    }
                    results.lock().unwrap_or_else(|e| e.into_inner()).push(o);
                }
                let _ = std::fs::remove_dir_all(&work);
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    results.sort_by_key(|o| o.id);
    write_outputs(&out_dir, &results, &chrome.version)?;
    println!("{} に report.jsonl・summary.txt・index.html を書いた", out_dir.display());
    Ok(())
}

fn one(id: usize, item: &Item, root: &Path, out_dir: &Path, work: &Path, resvg: &Resvg, chrome: &Chrome) -> Outcome {
    let path = root.join(&item.path);
    let fail = |e: String| Outcome { item: item.clone(), id, metrics: None, error: e };
    let svg = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => return fail(e.to_string()),
    };
    let a = match resvg.render(&svg, 1.0) {
        Ok(o) => o.img,
        Err(e) => return fail(format!("resvg：{e}")),
    };
    let b = match chrome.render(&path, a.w, a.h, work) {
        Ok(b) => b,
        Err(e) => return fail(format!("chrome：{e}")),
    };
    let Some((m, d)) = measure(&a, &b) else { return fail("大きさが違う".into()) };
    let img = out_dir.join("img");
    let _ = image::write_png(&img.join(format!("{id}-a.png")), &a);
    let _ = image::write_png(&img.join(format!("{id}-b.png")), &b);
    let _ = image::write_png(&img.join(format!("{id}-d.png")), &d);
    Outcome { item: item.clone(), id, metrics: Some(m), error: String::new() }
}

/// ADR-15・ADR-56 (1) の基準で分ける群。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Plain,
    Filter,
    Text,
    Image,
    Ub,
}

/// 基準（差の閾値・全画素に対する割合の上限・形の内側の差の上限）。
struct Criterion {
    over: u8,
    ratio: f64,
    interior: Option<u8>,
}

impl Group {
    const ALL: [Group; 5] = [Group::Plain, Group::Filter, Group::Text, Group::Image, Group::Ub];

    fn label(self) -> &'static str {
        match self {
            Group::Plain => "図形と単色（8/255・2%・内側 2/255。ADR-56 (1)）",
            Group::Filter => "フィルタ・グラデーションを含む（16/255・1%。ADR-35 (1)）",
            Group::Text => "文字を含む（M0 は両方がシステムのフォントで描く。分布から外す）",
            Group::Image => "画像を含む（Kura の設定の resvg は画像の参照を読まない。分布から外す）",
            Group::Ub => "未定義の振る舞いの例（題に (UB)。分布から外す）",
        }
    }

    /// 合否の基準（分布から外す群は None）。
    fn criterion(self) -> Option<Criterion> {
        match self {
            Group::Plain => Some(Criterion { over: 8, ratio: 0.02, interior: Some(2) }),
            Group::Filter => Some(Criterion { over: 16, ratio: 0.01, interior: None }),
            _ => None,
        }
    }
}

/// 図形と単色の群の目標（合否にしない。ADR-56 (1)）。
const PLAIN_GOAL_RATIO: f64 = 0.005;

fn group(item: &Item) -> Group {
    let f = |n: &str| item.features.split(',').any(|x| x == n);
    if f("ub") {
        Group::Ub
    } else if f("image") {
        Group::Image
    } else if f("text") {
        Group::Text
    } else if f("filter") || f("gradient") {
        Group::Filter
    } else {
        Group::Plain
    }
}

/// 差の閾値を超えた画素の数。
fn over(m: &Metrics, t: u8) -> u64 {
    if t >= 16 { m.over16 } else { m.over8 }
}

/// 基準で通るか（割合と、決めてあれば形の内側の両方）。
fn passes(m: &Metrics, c: &Criterion) -> bool {
    ratio(over(m, c.over), m.n_all) <= c.ratio && c.interior.is_none_or(|lim| m.interior_max <= lim)
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    v[i]
}

fn write_outputs(out_dir: &Path, results: &[Outcome], chrome: &str) -> Result<(), String> {
    // 1 件 1 行
    let mut jsonl = String::new();
    for o in results {
        let mut l = Line::new()
            .int("id", o.id as u64)
            .str("path", o.item.path.as_str())
            .str("category", o.item.category.as_str());
        let g = group(&o.item);
        l = l.str("features", o.item.features.as_str()).str("group", g.label());
        if let Some(m) = &o.metrics {
            l = l
                .int("w", m.w)
                .int("h", m.h)
                .int("n_all", m.n_all)
                .int("n_alpha", m.n_alpha)
                .int("over2", m.over2)
                .int("over8", m.over8)
                .int("over16", m.over16)
                .int("interior_n", m.interior_n)
                .int("interior_max", m.interior_max)
                .int("interior_over2", m.interior_over2)
                .int("max_diff", m.max_diff);
            if let Some(c) = g.criterion() {
                l = l.bool("pass", passes(m, &c));
            }
        }
        l = l.str("error", o.error.as_str());
        jsonl.push_str(&l.to_json());
        jsonl.push('\n');
    }
    std::fs::write(out_dir.join("report.jsonl"), jsonl).map_err(|e| e.to_string())?;

    // 群ごとの分布と、候補の基準で通る数
    let mut summary = String::new();
    let _ = writeln!(summary, "resvg 0.48.1（Kura の設定）と {chrome} の差（乗算済み RGBA8）\n");
    for g in Group::ALL {
        let ms: Vec<&Metrics> =
            results.iter().filter(|o| group(&o.item) == g).filter_map(|o| o.metrics.as_ref()).collect();
        let errors = results.iter().filter(|o| group(&o.item) == g && o.metrics.is_none()).count();
        let _ = writeln!(summary, "## {}：{} 件（描けなかった {errors} 件）", g.label(), ms.len());
        if ms.is_empty() {
            continue;
        }
        let fil = g == Group::Filter;
        if let Some(c) = g.criterion() {
            let n = ms.iter().filter(|m| passes(m, &c)).count();
            let _ = writeln!(summary, "  ADR-56 (1) の基準で通る（割合と内側の両方）：{n}/{}", ms.len());
            if g == Group::Plain {
                let goal = Criterion { ratio: PLAIN_GOAL_RATIO, ..c };
                let n = ms.iter().filter(|m| passes(m, &goal)).count();
                let _ = writeln!(summary, "  目標（8/255・0.5%・内側 2/255）で通る：{n}/{}", ms.len());
            }
        }
        let t_over = if fil { 16 } else { 8 };
        let over = |m: &Metrics| over(m, t_over);
        let mut r_all: Vec<f64> = ms.iter().map(|m| ratio(over(m), m.n_all)).collect();
        let mut r_alpha: Vec<f64> = ms.iter().map(|m| ratio(over(m), m.n_alpha)).collect();
        let mut inner: Vec<f64> = ms.iter().map(|m| f64::from(m.interior_max)).collect();
        let t = if fil { 16 } else { 8 };
        for (name, v) in [("全画素が分母", &mut r_all), ("α > 0 の画素が分母", &mut r_alpha)] {
            let _ = writeln!(
                summary,
                "  差 > {t}/255 の割合（{name}）：中央 {:.4}%・90% {:.4}%・95% {:.4}%・99% {:.4}%・最大 {:.4}%",
                percentile(v, 0.5) * 100.0,
                percentile(v, 0.9) * 100.0,
                percentile(v, 0.95) * 100.0,
                percentile(v, 0.99) * 100.0,
                percentile(v, 1.0) * 100.0
            );
        }
        let _ = writeln!(
            summary,
            "  形の内側の差の最大：中央 {}・90% {}・99% {}・最大 {}",
            percentile(&mut inner, 0.5),
            percentile(&mut inner, 0.9),
            percentile(&mut inner, 0.99),
            percentile(&mut inner, 1.0)
        );
        for limit in [0.001, 0.0025, 0.005, 0.01, 0.02, 0.05] {
            let pass_all = ms.iter().filter(|m| ratio(over(m), m.n_all) <= limit).count();
            let pass_alpha = ms.iter().filter(|m| ratio(over(m), m.n_alpha) <= limit).count();
            let _ = writeln!(
                summary,
                "  割合 ≤ {:.2}% で通る：全画素 {pass_all}/{}・α > 0 {pass_alpha}/{}",
                limit * 100.0,
                ms.len(),
                ms.len()
            );
        }
        if !fil {
            for lim in [2u8, 4, 8, 16] {
                let n = ms.iter().filter(|m| m.interior_max <= lim).count();
                let _ = writeln!(summary, "  内側の差 ≤ {lim}/255 で通る：{n}/{}", ms.len());
            }
        }
        let _ = writeln!(summary);
    }
    std::fs::write(out_dir.join("summary.txt"), &summary).map_err(|e| e.to_string())?;

    // HTML（全画素が分母の割合の大きい順。絵は上から 500 件）
    let mut order: Vec<&Outcome> = results.iter().collect();
    let key = |o: &Outcome| o.metrics.as_ref().map_or(f64::INFINITY, |m| ratio(m.over8, m.n_all));
    order.sort_by(|a, b| key(b).total_cmp(&key(a)));
    let mut h = String::new();
    let _ = write!(
        h,
        "<!doctype html><html lang=\"ja\"><head><meta charset=\"utf-8\"><title>差の一覧</title><style>\
         body{{font:13px/1.5 -apple-system,sans-serif;margin:16px;background:#fafafa;color:#222}}\
         table{{border-collapse:collapse}}td,th{{border:1px solid #ccc;padding:4px;vertical-align:top}}\
         img{{width:150px;height:auto;image-rendering:pixelated;background:repeating-conic-gradient(#ddd 0 25%,#fff 0 50%) 50%/16px 16px}}\
         pre{{background:#fff;padding:8px;border:1px solid #ddd;white-space:pre-wrap}}</style></head><body>\
         <h1>resvg（左）と Chrome（中）の差（右。灰色は差 × 8、赤は差 &gt; 8/255）</h1><pre>{}</pre><table>\
         <tr><th>#</th><th>道・群</th><th>差 &gt; 8（全画素）</th><th>差 &gt; 16（全画素）</th><th>内側の最大</th><th>合否</th><th>絵</th></tr>",
        escape(&summary)
    );
    for (rank, o) in order.iter().enumerate() {
        let g = group(&o.item);
        let cells = match &o.metrics {
            Some(m) => format!(
                "<td>{:.3}%</td><td>{:.3}%</td><td>{}</td><td>{}</td>",
                ratio(m.over8, m.n_all) * 100.0,
                ratio(m.over16, m.n_all) * 100.0,
                m.interior_max,
                match g.criterion() {
                    Some(c) if passes(m, &c) => "通る",
                    Some(_) => "外れる",
                    None => "—",
                }
            ),
            None => format!("<td colspan=4>{}</td>", escape(&o.error)),
        };
        let pics = if rank < 500 && o.metrics.is_some() {
            format!("<img src=\"img/{0}-a.png\"><img src=\"img/{0}-b.png\"><img src=\"img/{0}-d.png\">", o.id)
        } else {
            String::new()
        };
        let _ = write!(
            h,
            "<tr><td>{}</td><td>{}<br><small>{}</small></td>{cells}<td>{pics}</td></tr>",
            o.id,
            escape(&o.item.path),
            escape(g.label())
        );
    }
    h.push_str("</table></body></html>\n");
    std::fs::write(out_dir.join("index.html"), h).map_err(|e| e.to_string())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_differences_and_interior() {
        let mut a = Rgba::transparent(5, 5);
        let mut b = Rgba::transparent(5, 5);
        // 両方で不透明の塊（3 × 3）。真ん中だけが内側
        for y in 1..4 {
            for x in 1..4 {
                let i = (y * 5 + x) * 4;
                a.px[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
                b.px[i..i + 4].copy_from_slice(&[250, 0, 0, 255]);
            }
        }
        // 縁に半透明の差
        a.px[3] = 100;
        let (m, _) = measure(&a, &b).unwrap();
        assert_eq!(m.n_all, 25);
        assert_eq!(m.n_alpha, 10);
        assert_eq!(m.over2, 10);
        assert_eq!(m.over8, 1);
        assert_eq!(m.max_diff, 100);
        assert!(m.interior_n >= 1);
        assert_eq!(m.interior_max, 5);
        assert!(measure(&a, &Rgba::transparent(4, 5)).is_none());
    }

    #[test]
    fn criteria_and_groups() {
        let m = Metrics { n_all: 1000, over8: 20, over16: 10, interior_max: 2, ..Metrics::default() };
        let plain = Group::Plain.criterion().unwrap();
        assert!(passes(&m, &plain));
        assert!(!passes(&Metrics { over8: 21, ..m.clone() }, &plain));
        assert!(!passes(&Metrics { interior_max: 3, ..m.clone() }, &plain));
        let fil = Group::Filter.criterion().unwrap();
        assert!(passes(&m, &fil));
        assert!(!passes(&Metrics { over16: 11, ..m.clone() }, &fil));
        assert!(Group::Text.criterion().is_none());
        let item = |f: &str| Item { path: String::new(), category: String::new(), features: f.into() };
        assert_eq!(group(&item("text,ub")), Group::Ub);
        assert_eq!(group(&item("filter,image")), Group::Image);
        assert_eq!(group(&item("gradient")), Group::Filter);
        assert_eq!(group(&item("")), Group::Plain);
    }
}
