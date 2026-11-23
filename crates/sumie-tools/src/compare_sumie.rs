//! `compare-sumie`：sumie の描いたものを resvg・Chrome と比べ、M2 の合否を出す（M2 から。ADR-15・ADR-56 (1)・ADR-61・
//! ADR-89 (2)・ADR-101 (2)・ADR-102・ADR-103）。
//!
//! `compare-sumie [--root <index.jsonl のあるフォルダ>] [--filter <道の一部>] [--all] [--limit N] [--jobs N] [--min-side N]
//! [--cache <dir>,…] [--out <dir>]`
//!
//! - 集まり：既定は照合の集まり（`corpus/index.jsonl` の `scope` が in のもの。`--all` ですべて）と、このリポジトリの試しの
//!   SVG（`tests/svg/` の m0・m1-probe・m2-probe。倍率 1 で比べる：non-scaling の線などの試しは、Chrome の `<img>` の拡大と
//!   sumie の倍率の意味が違う。単位 4 の 3 回目のレビュー）。`--root` を渡せばそのフォルダの index.jsonl だけ
//! - 比べに意味のない印（未定義の振る舞い・animation・script）のものは外す。後のマイルストーンの機能の印のものも sumie を
//!   走らせ、`NotYet`・`Unsupported` を返せば理由ごとに数えて外し、描けば測る（ADR-89 (2)。印のあるものが黙って違う絵に
//!   崩れても見えるように。単位 4 の 3 回目のレビュー）。sumie のほかの断りは、例外（ADR-78 (4) のマークアップを含む実体）の
//!   ほかは外れに数える
//! - 倍率は `compare` と同じ（長い辺が `--min-side` に満たなければ整数倍）。resvg と Chrome の絵は `--cache` のフォルダ
//!   （`compare` か `compare-sumie` の出力。`report.jsonl` と `img/<id>-a.png`・`-b.png`）に同じ道・同じ入力のダイジェスト・
//!   同じ倍率のものがあれば使う。Chrome の絵は同じ版のものだけ（版を書いていない古い写しは使い、数を書く）。フォルダが
//!   なければ誤り
//! - 測るもの：sumie と resvg・sumie と Chrome・resvg と Chrome の差（`compare::measure`。ADR-15 の図形と単色の基準：
//!   8/255 を超える画素が 2% 以下・平らな所の差 2/255 以下）と、sumie と Chrome の「大きな差の画素」（差が 128/255 以上で、
//!   相手の近傍 3 × 3 にも 64/255 以内の画素がないもの。縁のずれでなく要素の抜けや余分。ADR-103）
//! - 不具合の候補（理由を求める）：resvg と Chrome は基準の内なのに sumie が両方から外れるもの（ADR-101 (2)）・Chrome と
//!   平らな所で違うもの（resvg と通っても。ADR-103）・大きな差の画素があるもの（ADR-103）
//! - M2 の合否（ADR-101 (2)・ADR-103）：Chrome を撮れたもののうち、sumie と Chrome が通る数 ≥ resvg と Chrome が通る数、
//!   不具合の候補がすべて既知の差（[`KNOWN`]。ADR に方式の差の理由を書き、認めたときの差の上限の内）、前の回（`--cache` の
//!   compare-sumie の出力）に Chrome と通ったものが外れていなければ合格。Chrome を撮れないものがあれば「不確か」、
//!   `--root`・`--filter`・`--limit`・`--all` で一部か別の集まりを流したときは「参考」
//! - 出力：`<out>/report.jsonl`・`<out>/summary.txt`・`<out>/index.html`（外れの大きい順。絵は `<out>/img/`）。`--out` は
//!   `--cache` のフォルダと同じにできない（初めに絵を消すので）

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

/// 比べに意味がないので外す印（未定義の振る舞い・動き・スクリプト）。
const SKIP_MARKS: &[&str] = &["ub", "animation", "script"];

/// sumie の断りのうち、外れに数えない例外（ADR-78 (4)：マークアップを含む実体は断る）。
const EXPECTED_REFUSALS: &[&str] = &["MarkupEntity"];

/// 関門に足す試しの SVG のフォルダ（ワークスペースからの道。倍率 1 で比べる）。
const PROBE_DIRS: &[&str] = &["tests/svg/m0", "tests/svg/m1-probe", "tests/svg/m2-probe"];

/// 既知の差：道・理由（ADR）・認めたときの差の上限（sumie と Chrome、sumie と resvg の 8/255 を超える画素の割合、Chrome との
/// 平らな所の差と大きな差の画素の数）。上限を超えれば理由のない候補に戻す（単位 4 の 3 回目のレビュー：道だけで認めると、
/// その絵がどれだけ崩れても合格になった）。割合の上限は認めたときの値 ＋ 0.5%。
struct Known {
    path: &'static str,
    reason: &'static str,
    sc_max: f64,
    sr_max: f64,
    flat_max: u8,
    big_max: u64,
}

/// 縁の反エイリアスの差（ADR-101 (5)）の 1 件。
const fn edge(path: &'static str, reason: &'static str, sc: f64, sr: f64) -> Known {
    Known { path, reason, sc_max: sc + 0.005, sr_max: sr + 0.005, flat_max: 2, big_max: 0 }
}

/// 決めた規則による差の 1 件（平らな所の差と大きな差の画素も、認めたときの値まで）。
const fn rule(path: &'static str, reason: &'static str, sc: f64, sr: f64, flat: u8, big: u64) -> Known {
    let sc_max = if sc + 0.005 > 1.0 { 1.0 } else { sc + 0.005 };
    Known { path, reason, sc_max, sr_max: sr + 0.005, flat_max: flat, big_max: big }
}

/// 既知の差（ADR-101 (5)・ADR-103）。ここにない不具合の候補が 1 つでもあれば、M2 の合否は不合格。
const KNOWN: &[Known] = &[
    edge("real/flag-icons/flags/4x3/bt.svg", THIN, 0.03289, 0.02898),
    edge("real/flag-icons/flags/4x3/eac.svg", THIN, 0.02525, 0.02461),
    edge("real/flag-icons/flags/4x3/ec.svg", THIN, 0.02629, 0.02144),
    edge("real/tabler-icons/icons/outline/brand-doctrine.svg", THICK, 0.02105, 0.02002),
    edge("real/tabler-icons/icons/outline/creative-commons-off.svg", THICK, 0.02737, 0.02411),
    edge("real/tabler-icons/icons/outline/fish-off.svg", THICK, 0.02028, 0.02058),
    edge("real/tabler-icons/icons/outline/om.svg", THICK, 0.02471, 0.02437),
    edge("real/tabler-icons/icons/outline/rainbow-off.svg", THICK, 0.02137, 0.02025),
    edge("real/tabler-icons/icons/outline/square-rounded-chevrons-up.svg", THICK, 0.02075, 0.02079),
    edge("real/tabler-icons/icons/outline/storm.svg", THICK, 0.02589, 0.02542),
    edge("real/tabler-icons/icons/outline/world-map-off.svg", THICK, 0.02473, 0.02278),
    rule("resvg/tests/painting/stroke/control-points-clamping-1.svg", CLAMP, 0.00428, 0.01232, 0, 20),
    rule("resvg/tests/structure/svg/no-size.svg", SIZE, 0.57903, 0.0, 255, 91237),
    rule("tests/svg/m1-probe/size/width-only.svg", SIZE, 0.43333, 0.0, 255, 1296),
    rule("tests/svg/m1-probe/size/height-only.svg", SIZE, 0.6, 0.0, 255, 1740),
    rule("tests/svg/m1-probe/size/percent.svg", SIZE, 0.26667, 0.0, 128, 778),
    rule("tests/svg/m1-probe/size/fraction.svg", FRACTION, 1.0, 0.0, 0, 50),
    rule("tests/svg/m1-probe/size/em.svg", DEFAULTS, 0.21, 0.0, 255, 1186),
    rule("tests/svg/m1-probe/system-language.svg", DEFAULTS, 1.0, 0.0, 255, 10000),
    rule("tests/svg/m1-probe/has.svg", HAS, 1.0, 0.0, 255, 10000),
    rule("resvg/tests/painting/stroke-dasharray/even-count.svg", DASH, 0.00983, 0.01020, 0, 2),
    rule("resvg/tests/painting/stroke-dasharray/odd-count.svg", DASH, 0.01303, 0.01337, 0, 2),
    rule("resvg/tests/painting/stroke-dasharray/on-a-circle.svg", DASH, 0.01107, 0.01080, 0, 4),
    rule("resvg/tests/painting/stroke-dasharray/ws-separator.svg", DASH, 0.01287, 0.01317, 0, 2),
    rule("resvg/tests/painting/stroke-dashoffset/default.svg", DASH, 0.01287, 0.01317, 0, 2),
    rule("resvg/tests/painting/stroke-dashoffset/em-units.svg", DASH, 0.01287, 0.01317, 0, 2),
    rule("resvg/tests/painting/stroke-dashoffset/negative-value.svg", DASH, 0.01298, 0.01323, 0, 4),
    rule("resvg/tests/painting/stroke-dashoffset/px-units.svg", DASH, 0.01298, 0.01323, 0, 4),
    rule("resvg/tests/shapes/rect/ch-values.svg", FONT_UNITS, 0.35340, 0.51850, 255, 13420),
    rule("resvg/tests/shapes/rect/ex-values.svg", FONT_UNITS, 0.15040, 0.00375, 255, 5571),
    rule("resvg/tests/shapes/rect/lh-values.svg", FONT_UNITS, 0.07850, 0.65617, 255, 3117),
    rule("resvg/tests/shapes/rect/rlh-values.svg", FONT_UNITS, 0.07850, 0.65617, 255, 3117),
    rule("tests/svg/m2-probe/thin-strokes.svg", THIN_PROBE, 0.03790, 0.03290, 0, 0),
];

/// 曲線の上の破線の境（ユーザーの判断。ADR-103）。
const DASH: &str = "ADR-103：曲線の上の破線の境の位置（Skia は粗い平坦化で長さを測るので、境が 0.5 画素ほど先にずれる。sumie は描く平坦化で測る。太い線の端の数画素）";
/// フォントの寸法に依る単位（ユーザーの判断。ADR-103）。
const FONT_UNITS: &str =
    "ADR-103：ex・ch・lh・rlh はフォントなしの既定（0.5em・1.2）で解く（Chrome は書体の寸法。M5 でフォントから解く）";
/// 細い線の試し（ADR-97 (2)）。
const THIN_PROBE: &str =
    "ADR-97 (2)：細い線を描く座標系の 1 画素の線を太さの α で描く（Skia の細い線の反エイリアスと縁の画素が違う）";

/// 寄せた制御点（ADR-101 (6)）。
const CLAMP: &str =
    "ADR-101 (6)：制御点が端点にごく近い曲線の継ぎ目（Skia は寄せた制御点を除いて向きを決める。方式の差）";
/// 大きさの規則（FR-OUT-01）。
const SIZE: &str = "FR-OUT-01・ADR-58 (1)・ADR-64 (8)：大きさは usvg の規則（幅か高さが欠ける・% なら中身の外接矩形。Chrome は書いた幅と既定の大きさ）";
/// 1 に満たない大きさ（ADR-78 (5)）。
const FRACTION: &str = "FR-OUT-01・ADR-78 (5)：画素の大きさは四捨五入で最小 1（Chrome は 0.3 の幅を引き伸ばす）";
/// 環境で決まる既定の値（ADR-66）。
const DEFAULTS: &str =
    "ADR-66：環境で決まる値は決めた既定（文字の大きさ 12・言語 en。usvg と同じ。Chrome は 16 と機械の言語）";
/// :has()（ADR-66 (1)・ADR-73 (4)）。
const HAS: &str = "ADR-66 (1)・ADR-73 (4)：`:has()` は正しいセレクタとして読むが当てない（警告。Chrome は当てる）";

/// 曲線の太い線の縁（ADR-97 (3)）。
const THICK: &str = "ADR-97 (3)・ADR-101 (5)：曲線の太い線の縁の反エイリアス（外れた画素はすべて縁の半端な画素。4 倍で描いて縮めた絵に sumie がいちばん近い）";
/// 細い線と線の縁（ADR-97 (2)(3)）。
const THIN: &str = "ADR-97 (2)(3)・ADR-101 (5)：細い線と曲線の線の縁の反エイリアス（外れた画素はすべて色の境。線を消すと Chrome との差は 2% 前後以下）";

/// 照合の集まりの 1 件。
#[derive(Debug, Clone)]
struct Item {
    /// 報告の道（照合の集まりは集まりの根から、試しの SVG はワークスペースから）
    path: String,
    /// 読むファイル
    file: PathBuf,
    features: String,
    /// 倍率を決める長い辺の下限（試しの SVG は 0：倍率 1）
    min_side: u32,
}

/// 断ったもの。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Refusal {
    #[default]
    None,
    /// resvg と sumie の両方（集まりの外）
    Both,
    /// resvg だけ（sumie は描く。Chrome と比べる）
    ResvgOnly,
    /// sumie だけ（外れに数える）
    SumieOnly,
}

/// 1 件の結果。
#[derive(Debug, Clone, Default)]
struct Outcome {
    id: usize,
    path: String,
    /// 外した理由（集まりに入らない）
    excluded: String,
    refusal: Refusal,
    /// 描けなかった理由
    error: String,
    /// Chrome が撮れなかった理由（sumie と resvg の差は測る）
    chrome_error: String,
    /// sumie と resvg の大きさが違う（外れに数える）
    size_mismatch: String,
    /// Chrome を撮った回数（やり直し）と、CSS の大きさが整数でないか（Chrome が引き伸ばす。`compare` と同じ）
    chrome_attempts: u32,
    fractional: bool,
    /// Chrome の絵を版の分からない写しから使った
    legacy_chrome: bool,
    /// 使った Chrome の絵の版（写しの版か、今撮った版。版の分からない写しは None：次の回にも版の分からない写しとして数える）
    chrome_used: Option<String>,
    sr: Option<Metrics>,
    sc: Option<Metrics>,
    rc: Option<Metrics>,
    /// sumie と Chrome の大きな差の画素の数（大きさが違えば None）
    big: Option<u64>,
    scale: u32,
    /// 描いた入力の BLAKE3（この出力を次の回の `--cache` に使えるように）
    input: String,
}

/// 基準（図形と単色。ADR-56 (1)・ADR-61）。
fn passes(m: &Metrics) -> bool {
    ratio8(m) <= 0.02 && m.flat_max <= 2
}

fn ratio8(m: &Metrics) -> f64 {
    if m.n_all == 0 { 0.0 } else { m.over8 as f64 / m.n_all as f64 }
}

impl Outcome {
    fn sr_pass(&self) -> bool {
        self.sr.as_ref().is_some_and(passes)
    }

    fn sc_pass(&self) -> bool {
        self.sc.as_ref().is_some_and(passes)
    }

    fn rc_pass(&self) -> bool {
        self.rc.as_ref().is_some_and(passes)
    }

    /// 不具合の候補の理由（候補でなければ空）。
    fn candidate(&self) -> Vec<&'static str> {
        let mut why = Vec::new();
        // sumie だけが断る・大きさが resvg と違う（Chrome と比べられないので、理由を求める。単位 4 の 4 回目のレビュー）
        if self.refusal == Refusal::SumieOnly {
            why.push("sumie だけが断る");
        }
        if !self.size_mismatch.is_empty() {
            why.push("大きさが resvg と違う");
        }
        if self.rc_pass() && !self.sr_pass() && !self.sc_pass() {
            why.push("resvg と Chrome は近いのに両方から外れる");
        }
        if self.sc.as_ref().is_some_and(|m| m.flat_max > 2) {
            why.push("Chrome と平らな所で違う");
        }
        if self.big.is_some_and(|b| b > 0) {
            why.push("Chrome と大きな差の画素がある");
        }
        why
    }

    /// 既知の差の理由（上限の内のときだけ）。
    fn known(&self) -> Option<&'static str> {
        let k = KNOWN.iter().find(|k| k.path == self.path)?;
        let sc = self.sc.as_ref()?;
        let sr_ok = self.sr.as_ref().is_none_or(|m| ratio8(m) <= k.sr_max);
        (ratio8(sc) <= k.sc_max && sr_ok && sc.flat_max <= k.flat_max && self.big.unwrap_or(0) <= k.big_max)
            .then_some(k.reason)
    }
}

/// sumie と Chrome の大きな差の画素の数：差（4 つの値の差の最大）が 128 以上で、相手の近傍 3 × 3 にも差 64 以内の画素が
/// ないもの（どちらの向きでも。1 画素の縁のずれは数えず、細い要素の抜けや余分を数える。ADR-103）。大きさが違えば None。
fn big_diffs(s: &Rgba, c: &Rgba) -> Option<u64> {
    if s.w != c.w || s.h != c.h {
        return None;
    }
    let (w, h) = (s.w as i64, s.h as i64);
    let at = |img: &Rgba, x: i64, y: i64| -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [img.px[i], img.px[i + 1], img.px[i + 2], img.px[i + 3]]
    };
    let diff = |a: [u8; 4], b: [u8; 4]| (0..4).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0);
    let near = |p: [u8; 4], img: &Rgba, x: i64, y: i64| {
        (-1..=1).any(|dy| {
            (-1..=1).any(|dx| {
                let (xx, yy) = (x + dx, y + dy);
                (0..w).contains(&xx) && (0..h).contains(&yy) && diff(p, at(img, xx, yy)) <= 64
            })
        })
    };
    let mut n = 0u64;
    for y in 0..h {
        for x in 0..w {
            let (sp, cp) = (at(s, x, y), at(c, x, y));
            if diff(sp, cp) < 128 {
                continue;
            }
            if !near(sp, c, x, y) || !near(cp, s, x, y) {
                n += 1;
            }
        }
    }
    Some(n)
}

/// 照合の集まりの index.jsonl を読む。
fn load_index(root: &Path, all: bool, min_side: u32, out: &mut Vec<Item>) -> Result<(), String> {
    let index = root.join("index.jsonl");
    let text = std::fs::read_to_string(&index).map_err(|e| format!("{}：{e}", index.display()))?;
    for l in text.lines() {
        let line = Line::parse(l)?;
        let path = line.get_str("path").unwrap_or("").to_string();
        if !all && line.get_str("scope") != Some("in") {
            continue;
        }
        out.push(Item {
            file: root.join(&path),
            path,
            features: line.get_str("features").unwrap_or("").to_string(),
            min_side,
        });
    }
    Ok(())
}

/// 試しの SVG（[`PROBE_DIRS`] の下の .svg。名前の順）。
fn load_probes(ws: &Path, out: &mut Vec<Item>) -> Result<(), String> {
    let mut found = Vec::new();
    for d in PROBE_DIRS {
        let mut stack = vec![ws.join(d)];
        while let Some(dir) = stack.pop() {
            let rd = std::fs::read_dir(&dir).map_err(|e| format!("{}：{e}", dir.display()))?;
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "svg") {
                    found.push(p);
                }
            }
        }
    }
    found.sort();
    for p in found {
        let path = p.strip_prefix(ws).unwrap_or(&p).display().to_string();
        out.push(Item { path, file: p, features: String::new(), min_side: 0 });
    }
    Ok(())
}

/// 集まりを決める（一部か別の集まりかも返す）。
fn load_items(ws: &Path, args: &[String], min_side: u32) -> Result<(Vec<Item>, bool), String> {
    let all = args.iter().any(|a| a == "--all");
    let mut out = Vec::new();
    let root = arg(args, "--root");
    match root {
        Some(r) => load_index(Path::new(r), all, min_side, &mut out)?,
        None => {
            load_index(&ws.join("corpus"), all, min_side, &mut out)?;
            load_probes(ws, &mut out)?;
        }
    }
    let filter = arg(args, "--filter");
    if let Some(f) = filter {
        out.retain(|i| i.path.contains(f));
    }
    let limit = arg(args, "--limit").is_some();
    if limit {
        out.truncate(crate::corpus::num_arg::<usize>(args, "--limit", 0)?);
    }
    // 既定でない倍率の下限も一部とみなす（既知の差の上限は既定の倍率で測った値。単位 4 の 4 回目のレビュー）
    let other_scale = min_side != DEFAULT_MIN_SIDE;
    Ok((out, root.is_some() || filter.is_some() || limit || all || other_scale))
}

/// `compare` か `compare-sumie` の出力の絵の 1 つ（フォルダ・id・倍率・入力のダイジェスト・Chrome の版・前の合否）。
#[derive(Debug, Clone)]
struct Cached {
    dir: PathBuf,
    id: u64,
    scale: u64,
    input: String,
    chrome: Option<String>,
    sr_pass: Option<bool>,
    sc_pass: Option<bool>,
}

/// 写しの在りか（道 → 写しの並び。フォルダの順）。フォルダか `report.jsonl` がなければ誤り。
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
                chrome: line.get_str("chrome").map(str::to_string),
                sr_pass: line.get_bool("sr_pass"),
                sc_pass: line.get_bool("sc_pass"),
            });
        }
    }
    Ok(out)
}

/// sumie で描いた結果。
pub(crate) enum Drawn {
    Image(Rgba),
    /// まだ描かない・描かない機能（外す）
    Excluded(String),
    /// ほかの断り（外れ。例外の断りは外す）
    Refused(String),
}

/// sumie で描く（倍率 `k`）。
pub(crate) fn render_sumie(svg: &[u8], k: u32) -> Drawn {
    let refused = |e: String| {
        if EXPECTED_REFUSALS.iter().any(|x| e.contains(x)) {
            Drawn::Excluded(format!("例外の断り（ADR-78 (4)）：{e}"))
        } else {
            Drawn::Refused(e)
        }
    };
    let doc = match sumie::parse(svg, &sumie::ParseOptions::DEFAULT, &sumie::Never) {
        Ok(d) => d,
        Err(sumie::Error::Unsupported(u)) => return refused(format!("sumie：未対応：{:?}", u.feature)),
        Err(e) => return Drawn::Refused(format!("sumie：{e:?}")),
    };
    let tree = match doc.build(&sumie::Never) {
        Ok(t) => t,
        Err(sumie::Error::Unsupported(u)) => {
            return match u.feature {
                sumie::Feature::NotYet(l) => Drawn::Excluded(format!("NotYet：{l:?}")),
                // 文字・画像・入れ子の SVG（後の機能）は描かないものとして外す
                f @ (sumie::Feature::TextWithoutFonts | sumie::Feature::Image | sumie::Feature::NestedSvg) => {
                    Drawn::Excluded(format!("描かない機能：{f:?}"))
                }
                f => refused(format!("sumie：未対応：{f:?}")),
            };
        }
        Err(e) => return Drawn::Refused(format!("sumie：{e:?}")),
    };
    let opts = sumie::RenderOptions { scale: f64::from(k), ..sumie::RenderOptions::DEFAULT };
    let plan = match tree.plan(&opts, &sumie::Never) {
        Ok(p) => p,
        Err(e) => return Drawn::Refused(format!("sumie：{e:?}")),
    };
    let size = plan.pixel_size();
    let mut px = vec![0u8; size.width as usize * size.height as usize * 4];
    let drawn = sumie::PixmapMut::new(&mut px, size.width, size.height, size.width as usize * 4)
        .and_then(|mut out| plan.render(&mut out, &sumie::Never));
    match drawn {
        Ok(()) => Drawn::Image(Rgba { w: size.width, h: size.height, px }),
        Err(e) => Drawn::Refused(format!("sumie：{e:?}")),
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    crate::corpus::check_args(
        args,
        &["--root", "--out", "--jobs", "--min-side", "--filter", "--limit", "--cache"],
        &["--all"],
        0,
    )?;
    let ws = workspace_root();
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
    let (items, partial) = load_items(&ws, args, min_side)?;
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
            let (items, next, results, resvg, chrome, out_dir, cache) =
                (&items, &next, &results, &resvg, &chrome, &out_dir, &cache);
            let spawned = std::thread::Builder::new().stack_size(256 << 20).spawn_scoped(s, move || {
                let work = out_dir.join(format!("work-{t}"));
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(k) else { break };
                    let o = one(k, item, out_dir, &work, resvg, chrome, cache);
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
    write_outputs(&out_dir, &results, &chrome.version, &cache, partial)?;
    println!("{} に report.jsonl・summary.txt・index.html を書いた", out_dir.display());
    if let Ok(text) = std::fs::read_to_string(out_dir.join("summary.txt"))
        && let Some(i) = text.find("## M2 の合否")
    {
        println!("{}", text[i..].lines().take(5).collect::<Vec<_>>().join("\n"));
    }
    Ok(())
}

/// 写しの Chrome の絵（同じ版か、版を書いていない古い写し）。
fn cached_chrome(c: &Cached, version: &str) -> Option<(Rgba, bool)> {
    let legacy = match &c.chrome {
        Some(v) if v != version => return None,
        Some(_) => false,
        None => true,
    };
    let b = image::read_png(&c.dir.join("img").join(format!("{}-b.png", c.id))).ok()?;
    Some((b, legacy))
}

fn one(
    id: usize,
    item: &Item,
    out_dir: &Path,
    work: &Path,
    resvg: &Resvg,
    chrome: &Chrome,
    cache: &BTreeMap<String, Vec<Cached>>,
) -> Outcome {
    let mut o = Outcome { id, path: item.path.clone(), scale: 1, ..Outcome::default() };
    if let Some(f) = item.features.split(',').find(|f| SKIP_MARKS.contains(f)) {
        o.excluded = format!("印：{f}");
        return o;
    }
    let svg = match std::fs::read(&item.file) {
        Ok(b) => b,
        Err(e) => {
            o.error = e.to_string();
            o.refusal = Refusal::SumieOnly;
            return o;
        }
    };
    let digest = blake3::hash(&svg).to_hex().to_string();
    o.input = digest.clone();
    let img = out_dir.join("img");
    let shot = |o: &mut Outcome, w: u32, h: u32, css: (f32, f32), k: u32| -> Option<Rgba> {
        let cached = cache
            .get(&item.path)
            .and_then(|v| v.iter().find(|c| c.scale == u64::from(k) && c.input == digest))
            .and_then(|c| cached_chrome(c, &chrome.version))
            .filter(|(b, _)| b.w == w && b.h == h);
        if let Some((b, legacy)) = cached {
            o.legacy_chrome = legacy;
            o.chrome_used = (!legacy).then(|| chrome.version.clone());
            return Some(b);
        }
        match chrome.render(&item.file, w, h, css, work) {
            Ok((b, attempts)) => {
                o.chrome_attempts = attempts;
                o.chrome_used = Some(chrome.version.clone());
                Some(b)
            }
            Err(e) => {
                o.chrome_error = format!("chrome：{e}");
                None
            }
        }
    };
    let (k, css) = match resvg.tree(&svg) {
        Ok(t) => {
            let (w, h) = Resvg::size(&t);
            let k = crate::compare::scale_for(w, h, item.min_side);
            let (cw, ch) = Resvg::css_size(&t);
            o.fractional = cw.fract() != 0.0 || ch.fract() != 0.0;
            (k, (cw * k as f32, ch * k as f32))
        }
        Err(e) => {
            // resvg が断った：sumie が描けば Chrome と比べる（倍率 1・sumie の大きさで）
            match render_sumie(&svg, 1) {
                Drawn::Image(c) => {
                    o.refusal = Refusal::ResvgOnly;
                    o.error = format!("resvg だけが断る：{e}");
                    let _ = image::write_png(&img.join(format!("{id}-s.png")), &c);
                    if let Some(b) = shot(&mut o, c.w, c.h, (c.w as f32, c.h as f32), 1) {
                        let _ = image::write_png(&img.join(format!("{id}-b.png")), &b);
                        o.sc = measure(&c, &b).map(|x| x.0);
                        o.big = big_diffs(&c, &b);
                    }
                }
                Drawn::Excluded(reason) => o.excluded = reason,
                Drawn::Refused(s) => {
                    o.refusal = Refusal::Both;
                    o.error = format!("両方が断る：resvg：{e}・{s}");
                }
            }
            return o;
        }
    };
    o.scale = k;
    // sumie だけが断っても resvg と Chrome は撮る（resvg と Chrome の差を測って候補を決める。単位 4 の 4 回目のレビュー）
    let c = match render_sumie(&svg, k) {
        Drawn::Image(c) => Some(c),
        Drawn::Excluded(reason) => {
            o.excluded = reason;
            return o;
        }
        Drawn::Refused(e) => {
            o.error = e;
            o.refusal = Refusal::SumieOnly;
            None
        }
    };
    let cached = cache.get(&item.path).and_then(|v| v.iter().find(|c| c.scale == u64::from(k) && c.input == digest));
    let a = match cached.and_then(|c| image::read_png(&c.dir.join("img").join(format!("{}-a.png", c.id))).ok()) {
        Some(a) => a,
        None => match resvg.render(&svg, k as f32) {
            Ok(r) => r.img,
            Err(e) => {
                // resvg の描画だけが断った：sumie が描けば Chrome と比べる（resvg だけが断るものと同じ。単位 4 の 4 回目の
                // レビュー）
                let Some(c) = c else {
                    o.error = format!("両方が断る：resvg：{e}・{}", o.error);
                    o.refusal = Refusal::Both;
                    return o;
                };
                o.refusal = Refusal::ResvgOnly;
                o.error = format!("resvg だけが断る：{e}");
                let _ = image::write_png(&img.join(format!("{id}-s.png")), &c);
                if let Some(b) = shot(&mut o, c.w, c.h, css, k) {
                    let _ = image::write_png(&img.join(format!("{id}-b.png")), &b);
                    o.sc = measure(&c, &b).map(|x| x.0);
                    o.big = big_diffs(&c, &b);
                }
                return o;
            }
        },
    };
    let b = shot(&mut o, a.w, a.h, css, k);
    let _ = image::write_png(&img.join(format!("{id}-a.png")), &a);
    if let Some(b) = &b {
        let _ = image::write_png(&img.join(format!("{id}-b.png")), b);
        o.rc = measure(&a, b).map(|x| x.0);
    }
    let Some(c) = c else { return o };
    let _ = image::write_png(&img.join(format!("{id}-s.png")), &c);
    match measure(&c, &a) {
        Some((m, d)) => {
            let _ = image::write_png(&img.join(format!("{id}-d.png")), &d);
            o.sr = Some(m);
        }
        None => o.size_mismatch = format!("大きさが違う（sumie {}×{}・resvg {}×{}）", c.w, c.h, a.w, a.h),
    }
    if let Some(b) = &b {
        o.sc = measure(&c, b).map(|x| x.0);
        o.big = big_diffs(&c, b);
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

fn write_outputs(
    out_dir: &Path,
    results: &[Outcome],
    chrome: &str,
    cache: &BTreeMap<String, Vec<Cached>>,
    partial: bool,
) -> Result<(), String> {
    let mut jsonl = String::new();
    for o in results {
        let mut l = Line::new()
            .int("id", o.id as u64)
            .str("path", o.path.as_str())
            .int("scale", o.scale)
            .str("input", o.input.as_str());
        // 使った Chrome の絵の版（版の分からない写しを使ったものは書かない：次の回も版の分からない写しとして数える）
        if let Some(v) = o.chrome_used.as_deref() {
            l = l.str("chrome", v);
        }
        if let Some(m) = o.sr.as_ref().or(o.sc.as_ref()) {
            l = l.int("w", m.w).int("h", m.h).int("n_all", m.n_all);
        }
        l = metrics_line(l, "sr", &o.sr);
        l = metrics_line(l, "sc", &o.sc);
        l = metrics_line(l, "rc", &o.rc);
        if let Some(b) = o.big {
            l = l.int("big", b);
        }
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
    let probes = set.iter().filter(|o| o.path.starts_with("tests/svg/")).count();
    let _ = writeln!(s, "## 集まり（ADR-89 (2)）");
    let _ = writeln!(
        s,
        "全 {} 件・外した {} 件・M2 の集まり {} 件（うち試しの SVG {probes} 件）",
        results.len(),
        results.len() - set.len(),
        set.len()
    );
    let _ = writeln!(s, "外した理由：");
    for (r, n) in &reasons {
        let _ = writeln!(s, "  {r}：{n}");
    }
    let by = |r: Refusal| -> Vec<&&Outcome> { set.iter().filter(|o| o.refusal == r).collect() };
    let (both, resvg_only, sumie_only) = (by(Refusal::Both), by(Refusal::ResvgOnly), by(Refusal::SumieOnly));
    // sumie と resvg：resvg が描いたもの（sumie だけの断り・大きさの違いは外れ）
    let with_resvg: Vec<&&Outcome> =
        set.iter().filter(|o| matches!(o.refusal, Refusal::None | Refusal::SumieOnly)).collect();
    let _ = writeln!(
        s,
        "\n## sumie と resvg（ADR-15 の図形と単色の基準：8/255 を超える画素が 2% 以下・平らな所の差 2/255 以下）"
    );
    let pass = with_resvg.iter().filter(|o| o.sr_pass()).count();
    let mismatched = with_resvg.iter().filter(|o| !o.size_mismatch.is_empty()).count();
    let _ = writeln!(
        s,
        "比べた {} 件（大きさが違う {mismatched} 件と sumie だけが断る {} 件は外れ）・両方が断る {} 件・resvg だけが断る {} 件",
        with_resvg.len(),
        sumie_only.len(),
        both.len(),
        resvg_only.len()
    );
    let _ = writeln!(s, "基準で通る {pass}/{}", with_resvg.len());
    let measured: Vec<&&&Outcome> = with_resvg.iter().filter(|o| o.sr.is_some()).collect();
    let goal = measured.iter().filter(|o| o.sr.as_ref().is_some_and(|m| ratio8(m) <= 0.005 && m.flat_max <= 2)).count();
    let _ = writeln!(s, "目標（0.5%）で通る {goal}/{}", with_resvg.len());
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
    // Chrome との差（両方が断るもののほか）
    let chrome_set: Vec<&&Outcome> = set.iter().filter(|o| o.refusal != Refusal::Both).collect();
    let no_chrome: Vec<&&&Outcome> =
        chrome_set.iter().filter(|o| o.refusal != Refusal::SumieOnly && !o.chrome_error.is_empty()).collect();
    let with_chrome: Vec<&&&Outcome> =
        chrome_set.iter().filter(|o| o.refusal == Refusal::SumieOnly || o.chrome_error.is_empty()).collect();
    let pass_c = with_chrome.iter().filter(|o| o.sc_pass()).count();
    let rc_pass = with_chrome.iter().filter(|o| o.rc_pass()).count();
    let retried = with_chrome.iter().filter(|o| o.chrome_attempts > 1).count();
    let fractional = with_chrome.iter().filter(|o| o.fractional).count();
    let legacy = with_chrome.iter().filter(|o| o.legacy_chrome).count();
    let _ = writeln!(s, "\n## Chrome との差（{} 件。sumie だけが断るものは外れ）", with_chrome.len());
    let _ = writeln!(s, "sumie と Chrome が同じ基準で通る {pass_c}・resvg と Chrome が通る {rc_pass}");
    let _ = writeln!(
        s,
        "Chrome を撮れなかった {} 件・やり直して撮れた {retried} 件・大きさが整数でない（Chrome が引き伸ばす）{fractional} 件・版の分からない写しを使った {legacy} 件",
        no_chrome.len()
    );
    // 不具合の候補
    let cand: Vec<(&&&Outcome, Vec<&'static str>)> =
        with_chrome.iter().map(|o| (*o, o.candidate())).filter(|(_, w)| !w.is_empty()).collect();
    let unknown: Vec<&(&&&Outcome, Vec<&'static str>)> = cand.iter().filter(|(o, _)| o.known().is_none()).collect();
    let _ = writeln!(
        s,
        "\n## 不具合の候補（resvg と Chrome は近いのに sumie が両方から外れる・Chrome と平らな所で違う・Chrome と大きな差の画素がある）：{} 件（既知の差 {}・理由のないもの {}）",
        cand.len(),
        cand.len() - unknown.len(),
        unknown.len()
    );
    for (o, why) in &cand {
        let detail = format!(
            "Chrome と {:.3}%・平らな所 {}・大きな差 {}",
            o.sc.as_ref().map_or(0.0, ratio8) * 100.0,
            o.sc.as_ref().map_or(0, |m| m.flat_max),
            o.big.unwrap_or(0)
        );
        match o.known() {
            Some(r) => {
                let _ = writeln!(s, "  {}（id {}。{detail}）：既知の差：{r}", o.path, o.id);
            }
            None => {
                let _ = writeln!(s, "  {}（id {}。{detail}）：理由なし（{}）", o.path, o.id, why.join("・"));
            }
        }
    }
    // KNOWN に挙げたのに候補でなくなったもの（直ったか、集まりから外れた。一覧を見直す）
    let stale: Vec<&str> = KNOWN.iter().map(|k| k.path).filter(|p| !cand.iter().any(|(o, _)| o.path == *p)).collect();
    if !stale.is_empty() && !partial {
        let _ = writeln!(s, "既知の差に挙げたが候補でないもの（一覧を見直す）：");
        for p in &stale {
            let _ = writeln!(s, "  {p}");
        }
    }
    // 前の回との件ごとの比べ（Chrome と通ったものが外れれば不合格。resvg とだけ外れたものは並べるだけ）。同じ入力と倍率の
    // 写しだけを照らす（単位 4 の 4 回目のレビュー）
    let prev = |o: &Outcome| {
        cache.get(&o.path).and_then(|v| {
            v.iter().find(|c| {
                (c.sc_pass.is_some() || c.sr_pass.is_some()) && c.input == o.input && c.scale == u64::from(o.scale)
            })
        })
    };
    // 前の回に集まりの内（合否のある写し）だったのに、今回は外したか sumie だけが断った・大きさが違うもの（新しい NotYet の
    // 広がりや断りを見逃さない。入力が同じものだけ。単位 4 の 4 回目のレビュー）
    let was_in = |o: &Outcome| {
        cache
            .get(&o.path)
            .is_some_and(|v| v.iter().any(|c| (c.sc_pass.is_some() || c.sr_pass.is_some()) && c.input == o.input))
    };
    let set_lost: Vec<&Outcome> = results
        .iter()
        .filter(|o| !o.excluded.is_empty() || o.refusal == Refusal::SumieOnly || !o.size_mismatch.is_empty())
        .filter(|o| was_in(o))
        .collect();
    let sc_lost: Vec<&&&Outcome> = with_chrome
        .iter()
        .copied()
        .filter(|o| !o.sc_pass() && prev(o).is_some_and(|c| c.sc_pass == Some(true)))
        .collect();
    let sr_lost: Vec<&&&Outcome> =
        with_resvg.iter().filter(|o| !o.sr_pass() && prev(o).is_some_and(|c| c.sr_pass == Some(true))).collect();
    let compared_prev = set.iter().filter(|o| prev(o).is_some()).count();
    let _ = writeln!(s, "\n## 前の回との比べ（写しに合否のある {compared_prev} 件）");
    let _ = writeln!(
        s,
        "Chrome と通っていて外れたもの：{} 件・resvg と通っていて外れたもの：{} 件",
        sc_lost.len(),
        sr_lost.len()
    );
    for o in &sc_lost {
        let _ = writeln!(s, "  Chrome：{}（id {}）", o.path, o.id);
    }
    for o in &sr_lost {
        let _ = writeln!(s, "  resvg：{}（id {}）", o.path, o.id);
    }
    let _ = writeln!(s, "前の回に集まりの内だったのに外したか断ったもの：{} 件", set_lost.len());
    for o in &set_lost {
        let why = if !o.excluded.is_empty() {
            o.excluded.as_str()
        } else if !o.error.is_empty() {
            o.error.as_str()
        } else {
            o.size_mismatch.as_str()
        };
        let _ = writeln!(s, "  {}（id {}）：{why}", o.path, o.id);
    }
    let ok_count = pass_c >= rc_pass;
    let ok_known = unknown.is_empty();
    let ok_prev = sc_lost.is_empty() && set_lost.is_empty();
    let verdict = if partial {
        "参考（一部か別の集まりを流した）"
    } else if !no_chrome.is_empty() {
        "不確か（Chrome を撮れないものがある）"
    } else if cache.is_empty() {
        "不確か（前の回の写しがない：--cache で前の回を渡す）"
    } else if ok_count && ok_known && ok_prev {
        "合格"
    } else {
        "不合格"
    };
    let yes = |b: bool| if b { "満たす" } else { "満たさない" };
    let _ = writeln!(
        s,
        "\n## M2 の合否（ADR-101 (2)・ADR-103・ADR-104）：{verdict}\n  sumie と Chrome が通る数 {pass_c} ≥ resvg と Chrome が通る数 {rc_pass}：{}\n  理由のない不具合の候補 0 件（{} 件）：{}\n  前の回に Chrome と通ったもの・集まりの内だったものが外れない（{} 件・{} 件）：{}\n  Chrome を撮れないもの 0 件（{} 件）：{}",
        yes(ok_count),
        unknown.len(),
        yes(ok_known),
        sc_lost.len(),
        set_lost.len(),
        yes(ok_prev),
        no_chrome.len(),
        yes(no_chrome.is_empty())
    );
    for (title, list) in [
        ("sumie だけが断る（外れ）", &sumie_only),
        ("両方が断る（集まりの外）", &both),
        ("resvg だけが断る（sumie は描き、Chrome と比べた）", &resvg_only),
    ] {
        if !list.is_empty() {
            let _ = writeln!(s, "\n## {title}");
            for o in list.iter() {
                let _ = writeln!(s, "  {}：{}", o.path, o.error);
            }
        }
    }
    if !no_chrome.is_empty() {
        let _ = writeln!(s, "\n## Chrome を撮れなかった");
        for o in &no_chrome {
            let _ = writeln!(s, "  {}：{}", o.path, o.chrome_error);
        }
    }
    let mism: Vec<&&&Outcome> = with_resvg.iter().filter(|o| !o.size_mismatch.is_empty()).collect();
    if !mism.is_empty() {
        let _ = writeln!(s, "\n## 大きさが違う（外れ）");
        for o in &mism {
            let _ = writeln!(s, "  {}：{}", o.path, o.size_mismatch);
        }
    }
    std::fs::write(out_dir.join("summary.txt"), &s).map_err(|e| e.to_string())?;

    // 外れの大きい順の一覧（大きさが違うもの・sumie だけが断るものを先に）
    let key = |o: &Outcome| -> f64 {
        if !o.size_mismatch.is_empty() || o.refusal == Refusal::SumieOnly {
            2.0
        } else {
            o.sr.as_ref().or(o.sc.as_ref()).map_or(0.0, ratio8)
        }
    };
    let mut order: Vec<&&Outcome> = set.iter().collect();
    order.sort_by(|a, b| key(b).total_cmp(&key(a)));
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>sumie と resvg・Chrome</title><style>body{font-family:sans-serif}\
         img{max-width:220px;image-rendering:pixelated;background:repeating-conic-gradient(#ccc 0 25%,#fff 0 50%) 0 0/16px 16px}\
         td{vertical-align:top;padding:4px}</style><table><tr><th>道</th><th>sumie</th><th>resvg</th><th>Chrome</th><th>差（sumie と resvg）</th></tr>",
    );
    for o in order {
        let m = o.sr.as_ref();
        let note = if !o.size_mismatch.is_empty() {
            o.size_mismatch.clone()
        } else if !o.error.is_empty() {
            o.error.clone()
        } else {
            format!(
                "8/255 超 {:.3}%・平らな所 {}・最大 {}",
                m.map_or(0.0, ratio8) * 100.0,
                m.map_or(0, |m| m.flat_max),
                m.map_or(0, |m| m.max_diff)
            )
        };
        let _ = write!(
            html,
            "<tr><td>{}<br>id {}・倍率 {}<br>{}<br>Chrome と {}・大きな差 {}</td>\
             <td><img src=img/{id}-s.png></td><td><img src=img/{id}-a.png></td><td><img src=img/{id}-b.png></td>\
             <td><img src=img/{id}-d.png></td></tr>",
            o.path.replace('<', "&lt;"),
            o.id,
            o.scale,
            note.replace('<', "&lt;"),
            if o.sc_pass() { "通る" } else { "外れる" },
            o.big.map_or("-".to_string(), |b| b.to_string()),
            id = o.id
        );
    }
    html.push_str("</table>");
    std::fs::write(out_dir.join("index.html"), html).map_err(|e| e.to_string())?;
    Ok(())
}
