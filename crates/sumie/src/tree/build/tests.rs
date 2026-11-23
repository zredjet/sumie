#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use crate::cancel::Never;
use crate::doc::Watch;
use crate::error::{Error, Feature, Later};
use crate::style::ParseOptions;
use crate::tree::{HAS_FILL, HAS_STROKE, Tree};

fn svg(body: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"100\" height=\"100\">{body}</svg>"
    )
}

/// 読んで木を作る（見積もり ≥ 実際と、1 回の払いが中止の間隔の内を確かめる）。
fn build(src: &str) -> Result<Tree, Error> {
    let d = crate::doc::parse(src.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    let est = d.estimate_build(&Never);
    let mut w = Watch::default();
    let r = d.build_watch(&Never, &mut w);
    if let (Ok(e), Ok(_)) = (&est, &r) {
        assert!(w.work <= e.work, "手間の見積もり {} < 実際 {}: {src}", e.work, w.work);
        assert!(w.peak <= e.peak_bytes, "確保の見積もり {} < 実際 {}: {src}", e.peak_bytes, w.peak);
        assert!(w.max_spend < crate::budget::CANCEL_INTERVAL);
        assert!(!w.over_released);
        let t = r.as_ref().unwrap();
        assert!(t.held_bytes() <= e.result_bytes, "結果の見積もり {} < 実際 {}", e.result_bytes, t.held_bytes());
        // 見積もりの上限で作っても同じ
        let t2 = d.build(&Never).unwrap();
        assert_eq!(t2.nodes.as_slice(), t.nodes.as_slice());
    }
    match (est, r) {
        (Err(e), _) => Err(e),
        (Ok(_), r) => r,
    }
}

fn not_yet(src: &str) -> Later {
    match build(&svg(src)) {
        Err(Error::Unsupported(u)) => match u.feature {
            Feature::NotYet(l) => l,
            f => panic!("{f:?}"),
        },
        r => panic!("{:?}", r.map(|t| t.nodes.len())),
    }
}

#[test]
fn shapes_make_nodes() {
    let t = build(&svg("<rect x=\"10\" y=\"20\" width=\"30\" height=\"40\" fill=\"#ff0000\"/>\
         <circle cx=\"50\" cy=\"50\" r=\"10%\" fill=\"none\" stroke=\"blue\" stroke-width=\"2\"/>\
         <ellipse cx=\"5\" cy=\"5\" rx=\"3\" fill=\"lime\"/>\
         <line x1=\"0\" y1=\"0\" x2=\"10\" y2=\"10\" stroke=\"black\"/>\
         <line x1=\"0\" y1=\"0\" x2=\"10\" y2=\"10\"/>\
         <polygon points=\"0,0 10,0 10,10\" fill-opacity=\"0.5\"/>\
         <polyline points=\"0,0 10,0 10,10,\" fill=\"none\" stroke=\"red\"/>\
         <path d=\"M0 0 L5 5\" stroke=\"red\"/>\
         <path style=\"d: path('M 1 1 L 2 2')\" stroke=\"red\"/>\
         <rect width=\"0\" height=\"5\"/>"))
    .unwrap();
    let n = t.nodes.as_slice();
    // 塗りも線もない line・大きさ 0 の rect は節にならない
    assert_eq!(n.len(), 8);
    assert_eq!(n[0].fill, [255.0, 0.0, 0.0, 1.0]);
    assert_eq!(n[0].flags & (HAS_FILL | HAS_STROKE), HAS_FILL);
    let b = n[0].bounds;
    assert_eq!((b.x0, b.y0, b.x1, b.y1), (10.0, 20.0, 40.0, 60.0));
    // r の % は対角を √2 で割ったもの（100 × 100 なら 100）
    let b = n[1].bounds;
    assert!((b.x1 - 60.0).abs() < 1e-9 && (b.x0 - 40.0).abs() < 1e-9, "{b:?}");
    assert_eq!(n[1].flags & (HAS_FILL | HAS_STROKE), HAS_STROKE);
    // ellipse の ry の auto は rx
    let b = n[2].bounds;
    assert!((b.y1 - 8.0).abs() < 1e-9, "{b:?}");
    assert_eq!(n[4].fill[3], 0.5);
    assert_eq!(t.size().width, 100.0);
}

#[test]
fn use_shares_geometry() {
    let t = build(&svg("<defs><path id=\"p\" d=\"M0 0 L10 0 L10 10 Z\"/></defs>\
         <use href=\"#p\" x=\"5\"/><use xlink:href=\"#p\" x=\"20\" fill=\"red\"/>\
         <style>.d { stroke-dasharray: 5 3; stroke: black }</style>\
         <rect class=\"d\" width=\"5\" height=\"5\"/><rect class=\"d\" width=\"6\" height=\"6\"/>"))
    .unwrap();
    let n = t.nodes.as_slice();
    assert_eq!(n.len(), 4);
    assert_eq!(n[0].verbs, n[1].verbs);
    assert_eq!(n[0].transform.e, 5.0);
    assert_eq!(n[1].transform.e, 20.0);
    // use の塗りを中身が受け継ぐ
    assert_eq!(n[1].fill, [255.0, 0.0, 0.0, 1.0]);
    // 同じ規則の破線は 1 度だけ
    assert_eq!(n[2].dashes, n[3].dashes);
    assert_eq!(t.dashes.len(), 2);
}

#[test]
fn context_paint_in_use() {
    let t = build(&svg(
        "<defs><rect id=\"r\" width=\"5\" height=\"5\" fill=\"context-stroke\" stroke=\"context-fill\"/></defs>\
         <use href=\"#r\" fill=\"blue\" stroke=\"red\"/><rect width=\"5\" height=\"5\" fill=\"context-fill\"/>",
    ))
    .unwrap();
    let n = t.nodes.as_slice();
    // use の外の context-fill は none
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].fill, [255.0, 0.0, 0.0, 1.0]);
    assert_eq!(n[0].stroke, [0.0, 0.0, 255.0, 1.0]);
}

#[test]
fn not_yet_features() {
    assert_eq!(not_yet("<rect width=\"5\" height=\"5\" opacity=\"0.5\"/>"), Later::Opacity);
    assert_eq!(not_yet("<g opacity=\"0\"><rect width=\"5\" height=\"5\"/></g>"), Later::Opacity);
    assert_eq!(not_yet("<linearGradient id=\"g\"/><rect width=\"5\" height=\"5\" fill=\"url(#g)\"/>"), Later::Gradient);
    assert_eq!(not_yet("<pattern id=\"g\"/><rect width=\"5\" height=\"5\" stroke=\"url(#g)\"/>"), Later::Pattern);
    assert_eq!(
        not_yet(
            "<clipPath id=\"c\"><rect width=\"1\" height=\"1\"/></clipPath><rect width=\"5\" height=\"5\" clip-path=\"url(#c)\"/>"
        ),
        Later::Clip
    );
    assert_eq!(not_yet("<rect width=\"5\" height=\"5\" style=\"clip-path: circle(5px)\"/>"), Later::Clip);
    assert_eq!(not_yet("<rect width=\"5\" height=\"5\" style=\"filter: blur(2px)\"/>"), Later::Filter);
    assert_eq!(not_yet("<rect width=\"5\" height=\"5\" style=\"mix-blend-mode: multiply\"/>"), Later::Blend);
    assert_eq!(not_yet("<g style=\"isolation: isolate\"/>"), Later::Blend);
    assert_eq!(
        not_yet("<marker id=\"m\"/><path d=\"M0 0 L5 5\" stroke=\"red\" marker-end=\"url(#m)\"/>"),
        Later::Marker
    );
    assert_eq!(not_yet("<svg width=\"5\" height=\"5\"><rect width=\"9\" height=\"9\"/></svg>"), Later::Viewport);
    // 正しくない参照は効かない（そのまま描く）・overflow: visible の入れ子の svg は切らない
    let t = build(&svg("<rect width=\"5\" height=\"5\" clip-path=\"url(#none)\" fill=\"url(#none) red\"/>\
         <svg width=\"5\" height=\"5\" overflow=\"visible\"><rect width=\"9\" height=\"9\"/></svg>\
         <rect width=\"5\" height=\"5\" fill=\"url(#none)\"/>"))
    .unwrap();
    assert_eq!(t.nodes.len(), 2);
    assert_eq!(t.nodes.as_slice()[0].fill, [255.0, 0.0, 0.0, 1.0]);
}

#[test]
fn content_size() {
    let doc = |body: &str| format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{body}</svg>");
    let t = build(&doc("<rect x=\"10\" y=\"10\" width=\"30\" height=\"20\" visibility=\"hidden\"/>\
        <rect width=\"500\" height=\"500\" display=\"none\"/>"))
    .unwrap();
    assert_eq!((t.size().width, t.size().height), (40.0, 30.0));
    assert_eq!(t.nodes.len(), 0);
    let t = build(&doc("")).unwrap();
    assert_eq!((t.size().width, t.size().height), (100.0, 100.0));
    assert!(matches!(build(&doc("<rect x=\"-10\" y=\"-10\" width=\"5\" height=\"5\"/>")), Err(Error::Invalid(_))));
}

#[test]
fn use_cycles_draw_once() {
    // 取り除いた参照の先は 1 段だけ描き直す（ADR-86 (1)）。終わること・見積もりが足りること
    let t = build(&svg("<g id=\"a\"><rect width=\"1\" height=\"1\"/><use href=\"#b\"/></g>\
         <g id=\"b\"><rect width=\"2\" height=\"2\"/><use href=\"#a\"/></g>\
         <use id=\"s\" href=\"#s\"/>"))
    .unwrap();
    // a：rect・(b の rect)、b：rect・(a の rect)
    assert_eq!(t.nodes.len(), 4);
    // 4 つの輪（前の例の 1 段の描き直しの中で、また取り除いた参照に当たる）
    let t = build(&svg("<g id=\"p1\"><use href=\"#p4\"/><rect width=\"1\" height=\"1\"/></g>\
         <g id=\"p2\"><use href=\"#p3\"/></g><g id=\"p3\"><use href=\"#p1\"/></g>\
         <g id=\"p4\"><use href=\"#p2\"/></g>"))
    .unwrap();
    assert!(!t.nodes.is_empty());
}

/// 種つきの乱数の文書（g・use・図形・規則・属性の組み合わせ）で、見積もり ≥ 実際（手間・確保・結果）を確かめる。
#[test]
fn estimate_covers_random_documents() {
    let mut r = Rng(0x5eed_b011d);
    let (mut parsed, mut built) = (0, 0);
    let (mut ratio_w, mut ratio_b, mut min_w) = (0.0f64, 0.0f64, 1.0f64);
    for _ in 0..300 {
        let big = r.i(4) == 0;
        let n = 1 + r.i(if big { 400 } else { 30 }) as usize;
        let mut body = String::from(
            "<style>.a{fill:red;stroke:blue;stroke-dasharray:1 2 3} .b{d:path('M0 0 L3 3 Q 5 5 6 0 Z')}</style>",
        );
        let mut open = 0;
        for i in 0..n {
            let id = format!("e{i}");
            let tgt = format!("e{}", r.i(n as u64));
            let k = r.i(9);
            let s = match k {
                0 => {
                    open += 1;
                    format!("<g id=\"{id}\" transform=\"translate(1 2)\">")
                }
                1 if open > 0 => {
                    open -= 1;
                    "</g>".to_string()
                }
                1 => String::new(),
                2 => format!("<use id=\"{id}\" href=\"#{tgt}\" x=\"3\"/>"),
                3 => format!("<rect id=\"{id}\" class=\"a\" width=\"{}\" height=\"5%\" rx=\"1em\"/>", r.i(50)),
                4 => format!("<path id=\"{id}\" class=\"b\" stroke=\"black\"/>"),
                5 => format!(
                    "<path id=\"{id}\" d=\"M0 0 C 1 2 3 4 5 6 A 3 4 10 1 1 9 9 z m 1 1 h 3\" fill=\"url(#none) green\"/>"
                ),
                6 => format!("<polygon id=\"{id}\" points=\"0 0 5 5 9 0 3 3\" stroke=\"red\" stroke-width=\"2\"/>"),
                7 => format!("<defs><circle id=\"{id}\" r=\"3\" fill=\"context-stroke\"/></defs>"),
                _ => format!("<symbol id=\"{id}\" overflow=\"visible\"><ellipse rx=\"2\" ry=\"3\"/></symbol>"),
            };
            body.push_str(&s);
        }
        for _ in 0..open {
            body.push_str("</g>");
        }
        let src = svg(&body);
        let Ok(d) = crate::doc::parse(src.as_bytes(), &ParseOptions::DEFAULT, &Never) else { continue };
        parsed += 1;
        let r = build(&src);
        if r.is_ok() {
            built += 1;
            let e = d.estimate_build(&Never).unwrap();
            let mut w = Watch::default();
            d.build_watch(&Never, &mut w).unwrap();
            ratio_w = ratio_w.max(w.work as f64 / e.work as f64);
            ratio_b = ratio_b.max(w.peak as f64 / e.peak_bytes as f64);
            min_w = min_w.min(w.work as f64 / e.work as f64);
        }
    }
    eprintln!("parsed {parsed} built {built} work max {ratio_w:.3} min {min_w:.3} bytes max {ratio_b:.3}");
    assert!(parsed >= 150 && built >= 50, "{parsed} {built}");
}

/// 種つきの xorshift。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn i(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// 大きな文書（パス・図形・use・規則）で、見積もりが実際を下回らず、大きく離れないこと（手間の比を表に出す）。
#[test]
fn estimate_is_tight_on_large_documents() {
    let mut cases = Vec::new();
    let mut a = String::new();
    for i in 0..3000 {
        a.push_str(&format!(
            "<path d=\"M{i} 0 C {i} 10 20 30 40 50 L 60 70 Q 80 90 100 0 A 5 5 0 0 1 10 10 Z\" fill=\"#123456\" stroke=\"red\"/>"
        ));
    }
    cases.push(("paths", svg(&a)));
    let mut b = String::from("<style>.c{fill:blue;stroke:black;stroke-width:2}</style>");
    for i in 0..3000 {
        b.push_str(&format!("<rect class=\"c\" x=\"{i}\" y=\"3\" width=\"10\" height=\"20\" rx=\"2\"/>"));
    }
    cases.push(("rects", svg(&b)));
    let mut c = String::from("<defs><g id=\"icon\"><path d=\"M0 0 L10 0 L10 10 Z\"/><circle r=\"3\"/></g></defs>");
    for i in 0..3000 {
        c.push_str(&format!("<use href=\"#icon\" x=\"{i}\" fill=\"green\"/>"));
    }
    cases.push(("uses", svg(&c)));
    for (name, src) in &cases {
        let d = crate::doc::parse(src.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
        let e = d.estimate_build(&Never).unwrap();
        let mut w = Watch::default();
        let t = d.build_watch(&Never, &mut w).unwrap();
        let rw = w.work as f64 / e.work as f64;
        let rb = w.peak as f64 / e.peak_bytes as f64;
        let rr = t.held_bytes() as f64 / e.result_bytes as f64;
        eprintln!(
            "{name}: work {} / {} = {rw:.3}  peak {} / {} = {rb:.3}  result {rr:.3}",
            w.work, e.work, w.peak, e.peak_bytes
        );
        assert!(rw <= 1.0 && rb <= 1.0 && rr <= 1.0);
    }
}

/// 読む段の見積もりが使う節の大きさが、木の型と同じ。
#[test]
fn node_size_matches_the_estimate() {
    assert_eq!(size_of::<crate::tree::PathNode>() as u64, crate::doc::weights::PATH_NODE_BYTES);
}
