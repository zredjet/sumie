//! 計算した値の照らし（`sumie-tools values`。roadmap M1・ADR-63 (5)）。照合の集まりの SVG を、Kura の設定の usvg の木と
//! sumie の計算した値（`sumie::fuzzing::computed_values`）で読み、id のある要素ごとに、描く木に入るか・根からの変形・
//! 塗り・線（塗り・不透明度・太さ）・不透明度・表示を比べる（usvg は use を展開して群を畳むので、id のない要素は比べない）。
//!
//! - id が片方でも 2 つ以上ある要素は比べない（usvg は use で写した中身の id を消す）
//! - 図形（rect・circle・ellipse・line・polyline・polygon・path）は usvg の path と、群（g・a・switch・svg・use）は usvg の群と
//!   比べる。文字と画像は後のマイルストーン（M3・M5）で比べる
//! - 図形の不透明度は、usvg が図形の上に名前のない群を作って持たせるので、「要素と、最も近い id のある祖先までの id のない
//!   祖先の不透明度の積」と「path から最も近い名前のある群までの名前のない群の不透明度の積」を比べる（根はどちらも名前のない
//!   ものとする）
//! - 変形は根からの変形（usvg の `abs_transform`）。sumie の変形が解けない（`transform-box` が外接矩形）ものは比べない
//! - 色の α は usvg が 8 ビットに丸めるので、半分の段までの違いは同じとみなす。表示（`visibility`）は塗るものがあるときだけ
//!   比べる（usvg は塗るもののない path に見えない印を付ける）
//!
//! 差は（要素の種類, 何が）ごとに数え、例を `summary.md` に、すべてを `result.jsonl` に書く。

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use resvg::usvg;
use sumie::fuzzing::{ComputedItem, UsedPaint, computed_values};
use sumie_testkit::jsonl::Line;

use crate::corpus::{arg, check_args, num_arg, workspace_root};
use crate::oracle::{Fonts, Resvg};

/// usvg の木の節（主の木だけ。clip・mask・pattern・filter の中の木は見ない）。
#[derive(Debug, Clone)]
pub struct UNode {
    pub id: String,
    pub kind: &'static str,
    pub parent: Option<usize>,
    pub depth: usize,
    pub abs: [f32; 6],
    pub opacity: f32,
    pub fill: Option<(String, f32)>,
    pub stroke: Option<(String, f32, f32)>,
    pub visible: bool,
}

fn tf(t: usvg::Transform) -> [f32; 6] {
    [t.sx, t.ky, t.kx, t.sy, t.tx, t.ty]
}

fn paint(p: &usvg::Paint) -> String {
    match p {
        usvg::Paint::Color(c) => format!("#{:02x}{:02x}{:02x}", c.red, c.green, c.blue),
        usvg::Paint::LinearGradient(g) => format!("linear({})", g.id()),
        usvg::Paint::RadialGradient(g) => format!("radial({})", g.id()),
        usvg::Paint::Pattern(g) => format!("pattern({})", g.id()),
    }
}

/// usvg の主の木を行きがけの順に並べる。
pub fn usvg_nodes(tree: &usvg::Tree) -> Vec<UNode> {
    let mut out = Vec::new();
    let root = tree.root();
    out.push(UNode {
        id: root.id().to_string(),
        kind: "root",
        parent: None,
        depth: 0,
        abs: tf(root.abs_transform()),
        opacity: root.opacity().get(),
        fill: None,
        stroke: None,
        visible: true,
    });
    let mut stack: Vec<(&usvg::Node, usize, usize)> = root.children().iter().rev().map(|n| (n, 1, 0)).collect();
    while let Some((n, d, parent)) = stack.pop() {
        let me = out.len();
        let mut node = UNode {
            id: String::new(),
            kind: "",
            parent: Some(parent),
            depth: d,
            abs: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            opacity: 1.0,
            fill: None,
            stroke: None,
            visible: true,
        };
        match n {
            usvg::Node::Group(g) => {
                node.id = g.id().to_string();
                node.kind = "group";
                node.abs = tf(g.abs_transform());
                node.opacity = g.opacity().get();
                for c in g.children().iter().rev() {
                    stack.push((c, d + 1, me));
                }
            }
            usvg::Node::Path(p) => {
                node.id = p.id().to_string();
                node.kind = "path";
                node.abs = tf(p.abs_transform());
                node.fill = p.fill().map(|f| (paint(f.paint()), f.opacity().get()));
                node.stroke = p.stroke().map(|s| (paint(s.paint()), s.opacity().get(), s.width().get()));
                node.visible = p.is_visible();
            }
            usvg::Node::Image(i) => {
                node.id = i.id().to_string();
                node.kind = "image";
                node.abs = tf(i.abs_transform());
                node.visible = i.is_visible();
            }
            usvg::Node::Text(t) => {
                node.id = t.id().to_string();
                node.kind = "text";
                node.abs = tf(t.abs_transform());
            }
        }
        out.push(node);
    }
    out
}

/// sumie の側の要素（借りずに持つ）。
#[derive(Debug, Clone)]
struct SNode {
    node: u32,
    parent: Option<u32>,
    local: String,
    svg: bool,
    id: Option<String>,
    rendered: bool,
    ctm: [f64; 6],
    ctm_known: bool,
    opacity: f64,
    fill: Paint,
    fill_opacity: f64,
    stroke: Paint,
    stroke_opacity: f64,
    stroke_width: f64,
    visible: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Paint {
    None,
    Color([f32; 4]),
    Server(String, String),
    Cut,
    Context,
}

impl Paint {
    fn of(p: UsedPaint<'_>) -> Paint {
        match p {
            UsedPaint::None => Paint::None,
            UsedPaint::Color(c) => Paint::Color(c),
            UsedPaint::Server { local, id } => {
                Paint::Server(String::from_utf8_lossy(local).into(), String::from_utf8_lossy(id).into())
            }
            UsedPaint::Cut => Paint::Cut,
            _ => Paint::Context,
        }
    }
}

fn snode(c: &ComputedItem<'_>) -> SNode {
    SNode {
        node: c.node,
        parent: c.parent,
        local: String::from_utf8_lossy(c.local).into(),
        svg: c.svg,
        id: c.id.map(|x| String::from_utf8_lossy(x).into()),
        rendered: c.rendered,
        ctm: c.ctm,
        ctm_known: c.ctm_known,
        opacity: c.opacity,
        fill: Paint::of(c.fill),
        fill_opacity: c.fill_opacity,
        stroke: Paint::of(c.stroke),
        stroke_opacity: c.stroke_opacity,
        stroke_width: c.stroke_width,
        visible: c.visible,
    }
}

fn kind_of(local: &str) -> &'static str {
    match local {
        "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "path" => "図形",
        "g" | "a" | "switch" | "svg" | "use" => "群",
        "text" | "tspan" | "textPath" | "tref" => "文字",
        "image" => "画像",
        _ => "ほか",
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-3 * b.abs().max(1.0)
}

/// sumie の塗りを usvg の書き方の文字列にする（比べる形）。
fn paint_text(p: &Paint, opacity: f64) -> String {
    match p {
        Paint::None => "none".into(),
        Paint::Color(c) => format!(
            "#{:02x}{:02x}{:02x} {:.3}",
            c[0].round() as u8,
            c[1].round() as u8,
            c[2].round() as u8,
            opacity * f64::from(c[3])
        ),
        Paint::Server(local, id) => format!("{local}({id}) {opacity:.3}"),
        Paint::Cut => "輪で除いた".into(),
        Paint::Context => "context".into(),
    }
}

/// 塗りが同じか（色は ±1・不透明度は 1e-3。参照先は種類と id を比べる。usvg が objectBoundingBox の参照先に付けた新しい id
/// （`linearGradient1` など）なら種類だけ）。
fn paint_same(s: &Paint, s_op: f64, u: Option<&(String, f32)>) -> bool {
    match (s, u) {
        (Paint::None | Paint::Cut, None) => true,
        (Paint::Color(c), Some((t, op))) if t.starts_with('#') && t.len() == 7 => {
            let ch = |k: usize| u8::from_str_radix(&t[1 + 2 * k..3 + 2 * k], 16).unwrap_or(0);
            // usvg は色の α を 8 ビットに丸めて持つ（sumie は丸めない）。α の半分の段までは同じとみなす
            let (a, b) = (s_op * f64::from(c[3]), f64::from(*op));
            (0..3).all(|k| (f32::from(ch(k)) - c[k]).abs() <= 1.0)
                && (near(a, b) || (a - b).abs() <= 0.5 / 255.0 + 1e-6)
        }
        (Paint::Server(local, id), Some((t, op))) => {
            let kind = match local.as_str() {
                "linearGradient" => "linear(",
                "radialGradient" => "radial(",
                _ => "pattern(",
            };
            // usvg が作り直した参照先（`linearGradient1` など）でなければ id も比べる
            let uid = t.strip_prefix(kind).and_then(|r| r.strip_suffix(')')).unwrap_or("");
            let generated = ["linearGradient", "radialGradient", "pattern"]
                .iter()
                .any(|p| uid.strip_prefix(p).is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())));
            t.starts_with(kind) && (generated || uid == id) && near(s_op, f64::from(*op))
        }
        _ => false,
    }
}

/// 1 つの差。
struct Diff {
    file: String,
    id: String,
    kind: &'static str,
    what: &'static str,
    sumie: String,
    usvg: String,
}

/// 1 つの文書を比べる。
fn compare(
    rel: &str,
    tree: &usvg::Tree,
    items: &[SNode],
    out: &mut Vec<Diff>,
    compared: &mut BTreeMap<&'static str, usize>,
) {
    let un = usvg_nodes(tree);
    let mut uid: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, n) in un.iter().enumerate() {
        if !n.id.is_empty() && i != 0 {
            uid.entry(n.id.as_str()).or_default().push(i);
        }
    }
    let mut sid: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, s) in items.iter().enumerate() {
        if let Some(id) = &s.id {
            sid.entry(id.as_str()).or_default().push(i);
        }
    }
    let by_node: BTreeMap<u32, usize> = items.iter().enumerate().map(|(i, s)| (s.node, i)).collect();
    // sumie の図形の不透明度の積（最も近い id のある祖先まで。根は名前のないもの）
    let s_chain = |i: usize| -> f64 {
        let mut p = items[i].opacity;
        let mut cur = items[i].parent;
        while let Some(n) = cur {
            let Some(&k) = by_node.get(&n) else { break };
            let a = &items[k];
            if a.id.is_some() && a.parent.is_some() {
                break;
            }
            p *= a.opacity;
            cur = a.parent;
        }
        p
    };
    let u_chain = |i: usize| -> f64 {
        let mut p = 1.0f64;
        let mut cur = un[i].parent;
        while let Some(k) = cur {
            if !un[k].id.is_empty() && k != 0 {
                break;
            }
            p *= f64::from(un[k].opacity);
            cur = un[k].parent;
        }
        p
    };
    for (id, list) in &sid {
        let [si] = list.as_slice() else { continue };
        let s = &items[*si];
        if !s.svg {
            continue;
        }
        let kind = kind_of(&s.local);
        if matches!(kind, "文字" | "画像" | "ほか") || s.parent.is_none() {
            continue;
        }
        let us = uid.get(id).cloned().unwrap_or_default();
        if us.len() > 1 {
            continue;
        }
        let want = if kind == "図形" { "path" } else { "group" };
        let u = us.first().copied().filter(|&k| un[k].kind == want);
        let mut diff = |what: &'static str, sv: String, uv: String| {
            out.push(Diff { file: rel.to_string(), id: id.to_string(), kind, what, sumie: sv, usvg: uv });
        };
        *compared.entry(kind).or_default() += 1;
        let Some(u) = u else {
            if s.rendered {
                diff("有無", "描く".into(), us.first().map_or("ない".into(), |&k| format!("{} として", un[k].kind)));
            }
            continue;
        };
        if !s.rendered {
            diff("有無", "描かない".into(), "描く".into());
            continue;
        }
        let un_u = &un[u];
        if s.ctm_known && !(0..6).all(|k| near(s.ctm[k], f64::from(un_u.abs[k]))) {
            diff("変形", format!("{:?}", s.ctm.map(|x| (x * 1000.0).round() / 1000.0)), format!("{:?}", un_u.abs));
        }
        if kind == "群" {
            if !near(s.opacity, f64::from(un_u.opacity)) {
                diff("不透明度", format!("{:.3}", s.opacity), format!("{:.3}", un_u.opacity));
            }
            continue;
        }
        let (so, uo) = (s_chain(*si), u_chain(u));
        if !near(so, uo) {
            diff("不透明度", format!("{so:.3}"), format!("{uo:.3}"));
        }
        if !paint_same(&s.fill, s.fill_opacity, un_u.fill.as_ref()) {
            diff("塗り", paint_text(&s.fill, s.fill_opacity), format!("{:?}", un_u.fill));
        }
        // 太さが 0 の線は描かない
        let stroke = if s.stroke_width > 0.0 { s.stroke.clone() } else { Paint::None };
        let ustroke = un_u.stroke.as_ref().map(|(p, o, _)| (p.clone(), *o));
        if !paint_same(&stroke, s.stroke_opacity, ustroke.as_ref()) {
            diff("線", paint_text(&stroke, s.stroke_opacity), format!("{:?}", un_u.stroke));
        } else if let Some((_, _, w)) = &un_u.stroke
            && !near(s.stroke_width, f64::from(*w))
        {
            diff("線の太さ", format!("{:.4}", s.stroke_width), format!("{w}"));
        }
        // 塗るものがない path は、usvg が見えない印を付ける（表示は塗るものがあるときだけ比べる）
        let paints = !matches!(s.fill, Paint::None | Paint::Cut) || !matches!(stroke, Paint::None | Paint::Cut);
        if paints && s.visible != un_u.visible {
            diff("表示", format!("{}", s.visible), format!("{}", un_u.visible));
        }
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    check_args(args, &["--root", "--out", "--dump", "--examples", "--filter"], &[], 0)?;
    let resvg = Resvg::new(&Fonts::parse("none")?);
    if let Some(p) = arg(args, "--dump") {
        return dump(&resvg, p);
    }
    let ws = workspace_root();
    let root = arg(args, "--root").map(PathBuf::from).unwrap_or_else(|| ws.join("corpus"));
    let out_dir = arg(args, "--out").map(PathBuf::from).unwrap_or_else(|| ws.join("local/values"));
    let examples = num_arg::<usize>(args, "--examples", 8)?;
    let filter = arg(args, "--filter");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    crate::sizes::walk_svgs(&root, &mut files);
    files.sort();
    let opts = sumie::ParseOptions::DEFAULT;
    let mut diffs = Vec::new();
    let mut compared: BTreeMap<&'static str, usize> = BTreeMap::new();
    let (mut docs, mut skipped) = (0usize, 0usize);
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().to_string();
        if filter.is_some_and(|x| !rel.contains(x)) {
            continue;
        }
        let data = std::fs::read(f).map_err(|e| e.to_string())?;
        let (Ok(tree), Ok(doc)) = (resvg.tree(&data), sumie::parse(&data, &opts, &sumie::Never)) else {
            skipped += 1;
            continue;
        };
        let mut items = Vec::new();
        computed_values(&doc, &mut |c| items.push(snode(c))).map_err(|e| format!("{rel}：{e:?}"))?;
        docs += 1;
        compare(&rel, &tree, &items, &mut diffs, &mut compared);
    }
    // 書き出し
    let mut lines = String::new();
    for d in &diffs {
        let l = Line::new()
            .str("name", d.file.as_str())
            .str("id", d.id.as_str())
            .str("kind", d.kind)
            .str("what", d.what)
            .str("sumie", d.sumie.as_str())
            .str("usvg", d.usvg.as_str());
        lines.push_str(&l.to_json());
        lines.push('\n');
    }
    std::fs::write(out_dir.join("result.jsonl"), lines).map_err(|e| e.to_string())?;
    let mut groups: BTreeMap<(&str, &str), Vec<&Diff>> = BTreeMap::new();
    for d in &diffs {
        groups.entry((d.kind, d.what)).or_default().push(d);
    }
    let mut md = format!(
        "# 計算した値の照らし（usvg と sumie）\n\n文書 {docs} 件（どちらかが読めない {skipped} 件を除く）。比べた要素："
    );
    for (k, n) in &compared {
        let _ = write!(md, " {k} {n}");
    }
    md.push_str("\n\n| 要素 | 何が | 件数 | 文書の数 |\n|---|---|---|---|\n");
    for ((k, w), ds) in &groups {
        let files: std::collections::BTreeSet<&str> = ds.iter().map(|d| d.file.as_str()).collect();
        let _ = writeln!(md, "| {k} | {w} | {} | {} |", ds.len(), files.len());
    }
    for ((k, w), ds) in &groups {
        let _ = writeln!(md, "\n## {k}・{w}\n\n| 文書 | id | sumie | usvg |\n|---|---|---|---|");
        for d in ds.iter().take(examples) {
            let _ = writeln!(md, "| `{}` | `{}` | {} | {} |", d.file, d.id, d.sumie, d.usvg);
        }
    }
    std::fs::write(out_dir.join("summary.md"), &md).map_err(|e| e.to_string())?;
    println!("文書 {docs} 件・差 {} 件 → {}", diffs.len(), out_dir.join("summary.md").display());
    for ((k, w), ds) in &groups {
        println!("  {k}・{w}：{}", ds.len());
    }
    Ok(())
}

/// 1 つの文書の usvg の木と sumie の計算した値を並べて出す。
fn dump(resvg: &Resvg, path: &str) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let tree = resvg.tree(&data)?;
    let mut s = String::from("## usvg\n");
    for n in usvg_nodes(&tree) {
        let _ = writeln!(
            s,
            "{}{} id={:?} abs={:?} op={} fill={:?} stroke={:?} vis={}",
            "  ".repeat(n.depth),
            n.kind,
            n.id,
            n.abs,
            n.opacity,
            n.fill,
            n.stroke,
            n.visible
        );
    }
    s.push_str("## sumie\n");
    let doc = sumie::parse(&data, &sumie::ParseOptions::DEFAULT, &sumie::Never).map_err(|e| format!("{e:?}"))?;
    computed_values(&doc, &mut |c| {
        let n = snode(c);
        let _ = writeln!(
            s,
            "{} id={:?} 描く={} 変形={:?}{} op={} fill={} stroke={} 太さ={} 表示={}",
            n.local,
            n.id,
            n.rendered,
            n.ctm,
            if n.ctm_known { "" } else { "（解けない）" },
            n.opacity,
            paint_text(&n.fill, n.fill_opacity),
            paint_text(&n.stroke, n.stroke_opacity),
            n.stroke_width,
            n.visible
        );
    })
    .map_err(|e| format!("{e:?}"))?;
    print!("{s}");
    Ok(())
}
