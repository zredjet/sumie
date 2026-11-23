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
    // 描かない移動の点は大きさに入れない（続く移動・区間のないパス）。末尾の移動は入れる（resvg と同じ。単位 3 の 2 回目の
    // レビュー）
    let rect = "<rect width=\"10\" height=\"10\"/>";
    for (d, side) in [("M200 200", 10.0), ("M300 300 M0 0 L10 10", 10.0), ("M0 0 L10 10 M300 300", 300.0)] {
        let t = build(&doc(&format!("<path d=\"{d}\" stroke=\"black\"/>{rect}"))).unwrap();
        assert_eq!((t.size().width, t.size().height), (side, side), "{d}");
    }
    // 大きさを中身で決める文書の % は 100 × 100 で解く（usvg の既定の大きさ。resvg・Chrome と同じ絵。ADR-99 (1)）
    let t = build(&doc(
        "<rect width=\"40%\" height=\"40%\" fill=\"lime\"/><rect width=\"10\" height=\"10\" fill=\"blue\"/>",
    ))
    .unwrap();
    assert_eq!((t.size().width, t.size().height), (40.0, 40.0));
    let t = build(&doc("<circle cx=\"20\" cy=\"20\" r=\"10%\"/><rect width=\"5\" height=\"5\"/>")).unwrap();
    assert_eq!((t.size().width, t.size().height), (30.0, 30.0));
    let t = build(&doc("<rect x=\"50%\" width=\"10\" height=\"10\"/>")).unwrap();
    assert_eq!((t.size().width, t.size().height), (60.0, 10.0));
    // 大きさを中身で決める文書の根の変形の原点は (0, 0)（usvg と同じ大きさ。ADR-101）
    let root = |attrs: &str, body: &str| format!("<svg xmlns=\"http://www.w3.org/2000/svg\" {attrs}>{body}</svg>");
    let t = build(&root("transform=\"scale(2)\"", "<rect width=\"10\" height=\"10\"/>")).unwrap();
    assert_eq!((t.size().width, t.size().height), (20.0, 20.0));
    let t = build(&root("transform=\"scale(2)\"", "<rect width=\"80\" height=\"60\"/>")).unwrap();
    assert_eq!((t.size().width, t.size().height), (160.0, 120.0));
    // 線の太さと破線の % も 100 × 100 で解く（対角 / √2 = 100）
    let a = alpha_at(&root(
        "",
        "<rect width=\"200\" height=\"100\" fill=\"none\"/><path d=\"M0 50 H100\" stroke=\"black\" stroke-width=\"10%\" stroke-dasharray=\"20% 20%\"/>",
    ));
    assert_eq!((a(10, 50), a(10, 56), a(30, 50)), (255, 0, 0));
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
        let src = random_doc(&mut r);
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

/// 読む → 木 → 計画 → 描く（倍率 1）。画素の α を返す口。
fn alpha_at(src: &str) -> impl Fn(u32, u32) -> u8 + use<> {
    use crate::plan::RenderOptions;
    use crate::render::PixmapMut;
    let t = build(src).unwrap();
    let plan = t.plan(&RenderOptions::DEFAULT, &Never).unwrap();
    let (w, h) = (plan.pixel_size().width, plan.pixel_size().height);
    let mut px = vec![0u8; (w * h * 4) as usize];
    plan.render(&mut PixmapMut::new(&mut px, w, h, (w * 4) as usize).unwrap(), &Never).unwrap();
    move |x, y| px[((y * w + x) * 4 + 3) as usize]
}

/// pathLength="0" の破線は実線・non-scaling と pathLength は要素の座標系の長さで割って画面の長さで描く（Chrome と同じ。
/// `tests/svg/m2-probe/` の pathlength-zero・nonscaling-pathlength。ADR-95）。
#[test]
fn path_length_follows_chrome() {
    let a = alpha_at(&svg(
        "<path d=\"M10 50 H90\" pathLength=\"0\" stroke-dasharray=\"10 10\" stroke=\"green\" stroke-width=\"10\"/>",
    ));
    assert_eq!(a(25, 50), 255);
    let a =
        alpha_at(&svg("<g transform=\"scale(2)\"><path d=\"M0 10 H40\" pathLength=\"10\" stroke-dasharray=\"5 5\" \
         vector-effect=\"non-scaling-stroke\" fill=\"none\" stroke=\"green\" stroke-width=\"6\"/></g>"));
    assert_eq!((a(10, 20), a(30, 20), a(50, 20)), (255, 0, 255));
}

/// use の中の symbol は子を描き（overflow が見えるもの）、use の x・y は CSS のプロパティとしても読む（ADR-84 (6)）。
#[test]
fn symbol_children_and_use_css_position() {
    let t = build(&svg(
        "<symbol id=\"s\" overflow=\"visible\"><rect x=\"20\" y=\"20\" width=\"10\" height=\"10\" fill=\"green\"/></symbol>\
         <use href=\"#s\" width=\"100\" height=\"100\"/>\
         <defs><rect id=\"r\" width=\"5\" height=\"5\"/></defs><use href=\"#r\" style=\"x: 7px; y: 10%\"/>",
    ))
    .unwrap();
    let n = t.nodes.as_slice();
    assert_eq!(n.len(), 2);
    assert_eq!(n[0].bounds.x0, 20.0);
    assert_eq!((n[1].transform.e, n[1].transform.f), (7.0, 10.0));
}

/// 乱数の文書（g・use・図形・規則・属性の組み合わせ）。
fn random_doc(r: &mut Rng) -> String {
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
    svg(&body)
}

/// 乱数の文書で、描く段までの確かめ（`fuzzing::render_ops`：形の約束・範囲の一致・同じ画素）を通す。
#[test]
fn render_ops_on_random_documents() {
    let mut r = Rng(0x5eed_4e4d);
    let mut drawn = 0;
    for _ in 0..200 {
        let src = random_doc(&mut r);
        let mut data = vec![(r.i(4)) as u8, r.i(256) as u8, r.i(256) as u8, r.i(256) as u8];
        data.extend_from_slice(src.as_bytes());
        assert_eq!(crate::fuzzing::render_ops(&data).check, crate::fuzzing::Check::Ok, "{src}");
        let d = crate::doc::parse(src.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
        drawn += usize::from(d.build(&Never).is_ok());
    }
    assert!(drawn > 150, "{drawn}");
}

/// use で指した svg にも use の width・height が勝ち、switch に選ばれない子も use で指せば描く。外接矩形の要る
/// transform-box の変形は描かずに断る（単位 3 のレビュー）。
#[test]
fn walk_follows_use_context() {
    let t = build(&svg(
        "<defs><svg id=\"s\" viewBox=\"0 0 10 10\" overflow=\"visible\"><rect width=\"10\" height=\"10\" fill=\"red\"/></svg></defs>\
         <use href=\"#s\" width=\"50\" height=\"50\"/>\
         <switch><rect width=\"1\" height=\"1\" fill=\"blue\"/><rect id=\"b\" x=\"20\" width=\"10\" height=\"10\" fill=\"red\"/></switch>\
         <use href=\"#b\" y=\"30\"/>",
    ))
    .unwrap();
    let n = t.nodes.as_slice();
    assert_eq!(n.len(), 3);
    assert_eq!(n[0].bounds.x1, 50.0);
    assert_eq!((n[2].bounds.x0, n[2].bounds.y0), (20.0, 30.0));
    assert_eq!(
        not_yet(
            "<rect x=\"10\" y=\"10\" width=\"20\" height=\"20\" style=\"transform-box: fill-box; transform-origin: center; transform: rotate(90deg)\"/>"
        ),
        Later::TransformBox
    );
}

/// 見積もり ≥ 実際：use で展開した長い破線・escape で書いた url の塗り・規則で共有する長い宣言（単位 3 のレビューの再現）。
#[test]
fn adversarial_estimates_hold() {
    let long_dash: String = (0..3000).map(|_| "5 5 ").collect();
    let mut a = format!("<path id=\"p\" d=\"M0 0 L10 10\" stroke=\"red\" stroke-dasharray=\"{long_dash}\"/>");
    for _ in 0..300 {
        a.push_str("<use href=\"#p\"/>");
    }
    build(&svg(&a)).unwrap();
    let id: String = (0..2000).map(|_| 'a').collect();
    let mut b = format!("<g style=\"fill: \\75 rl(#{id}) red\">");
    for _ in 0..200 {
        b.push_str("<rect width=\"5\" height=\"5\"/>");
    }
    b.push_str("</g>");
    build(&svg(&b)).unwrap();
    let mut c = format!(
        "<style>path{{stroke-dasharray:{long_dash};stroke:red}} .k{{d:path('M0 0 L5 5 {}')}}</style>",
        "L1 1 ".repeat(500)
    );
    for _ in 0..300 {
        c.push_str("<path class=\"k\"/>");
    }
    let t = build(&svg(&c)).unwrap();
    // 規則の宣言は 1 度だけ置き場に書く
    assert!(t.held_bytes() < 200_000, "{}", t.held_bytes());
    // 規則の並びの d・破線を多くの図形でない要素に当てる：鍵は当たる要素ごとに積んでから除く（単位 3 の 2 回目のレビュー）
    for (rule, el, n) in [
        ("*{stroke-dasharray:1;d:path('M0 0')}", "<a/>", 600),
        ("*{stroke-dasharray:1;d:path('M0 0')}", "<a/>", 50_000),
        ("*{stroke-dasharray: 1 2}", "<g/>", 20_000),
    ] {
        let doc = svg(&format!("<style>{rule}</style>{}", el.repeat(n)));
        build(&doc).unwrap();
        let d = crate::doc::parse(doc.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
        d.build(&Never).unwrap();
    }
    // 長い url の塗りが 1 つだけなら、ほかの図形の手間はその長さで数えない（単位 3 の 2 回目のレビュー）
    let long: String = "a".repeat(20_000);
    let doc = svg(&format!(
        "<rect fill=\"url(#{long}) red\" width=\"5\" height=\"5\"/>{}",
        "<rect fill=\"blue\" width=\"5\" height=\"5\"/>".repeat(20_000)
    ));
    build(&doc).unwrap();
    let d = crate::doc::parse(doc.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    let e = d.estimate_build(&Never).unwrap();
    assert!(e.work < 200_000_000, "{}", e.work);
    // use の中身は use から受け継ぐ（祖先の並びの外の url も数える）
    let doc = svg(&format!(
        "<defs><rect id=\"r\" width=\"5\" height=\"5\"/></defs>{}",
        format!("<use href=\"#r\" fill=\"url(#{long}) red\"/>").repeat(50)
    ));
    build(&doc).unwrap();
}

/// 読む段の見積もりが使う宣言の鍵と読んだ結果の大きさが、木の型と同じ。
#[test]
fn decl_sizes_match_the_estimate() {
    assert_eq!(size_of::<super::DeclKey>() as u64, crate::doc::weights::KEY_BYTES);
    assert_eq!(size_of::<super::DeclSlot>() as u64, crate::doc::weights::SLOT_BYTES);
}

/// 根の svg の `background` のまとめは、最後の層の色を `background-color` として描く（Chrome と同じ。外の url の画像は描かない。
/// まとめの後ろの `background-color` が勝ち、色のないまとめは透明に戻す。ADR-99 (4)）。形は Chrome と同じに確かめ（ADR-101）、
/// グラデーションと data: の画像の層は NotYet（後ろの宣言や長い名前の background-image でも消えない。ADR-102）。根を描かない
/// （viewBox の幅が 0・display: none など）ときも背景は塗る（Chrome と同じ。ADR-102）。
#[test]
fn background_shorthand() {
    use crate::plan::RenderOptions;
    use crate::render::PixmapMut;
    let pixel = |attrs: &str| -> Result<[u8; 4], Error> {
        let t = build(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\" {attrs}><rect width=\"1\" height=\"1\"/></svg>"
        ))?;
        let plan = t.plan(&RenderOptions::DEFAULT, &Never).unwrap();
        let mut px = vec![0u8; 10 * 10 * 4];
        plan.render(&mut PixmapMut::new(&mut px, 10, 10, 40).unwrap(), &Never).unwrap();
        let i = (5 * 10 + 5) * 4;
        Ok([px[i], px[i + 1], px[i + 2], px[i + 3]])
    };
    let style = |v: &str| pixel(&format!("style=\"{v}\"")).unwrap();
    const LIME: [u8; 4] = [0, 255, 0, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];
    const NONE: [u8; 4] = [0, 0, 0, 0];
    assert_eq!(style("background: lime"), LIME);
    assert_eq!(style("background: url(x.png) center / cover no-repeat #0000ff80"), [0, 0, 128, 128]);
    assert_eq!(style("background: none"), NONE);
    assert_eq!(style("background: calc(50%) lime"), LIME);
    // 色は最後の層だけ（前の層の色は壊れた形で、宣言ごと無視する）
    assert_eq!(style("background-color: lime; background: red, url(x.png)"), LIME);
    assert_eq!(style("background: url(a.png), lime"), LIME);
    // 後ろが勝つ
    assert_eq!(style("background: lime; background-color: transparent"), NONE);
    assert_eq!(style("background-color: lime; background: url(x.png)"), NONE);
    // 壊れた形は無視する（Chrome と同じ。どれも前の lime が残る）
    for bad in [
        "lime red",
        "center /",
        "banana",
        "left red top",
        "left right red",
        "1px 2px 3px red",
        "10deg red",
        "center red / cover",
        "center / -5px red",
        "repeat red no-repeat",
    ] {
        assert_eq!(style(&format!("background-color: lime; background: {bad}")), LIME, "{bad}");
    }
    // Chrome では壊れた形（単位 3 の 4 回目のレビュー）
    for bad in ["element(#a) red", "cross-fade(url(a) 50%, url(b)) red", "calc(10deg) red", "center / calc(5deg) red"] {
        assert_eq!(style(&format!("background-color: lime; background: {bad}")), LIME, "{bad}");
    }
    // 正しい形（箱の text・border-area と、長さの min・max・clamp も。単位 3 の 4 回目のレビュー）
    for good in [
        "left 10px top 5px red",
        "center / 10px auto red",
        "top left red",
        "repeat-x fixed padding-box red",
        "text red",
        "border-box text red",
        "padding-box border-area red",
        "min(1px, 2px) red",
        "clamp(1px, 2px, 3px) red",
        "center / min(5px, 2px) red",
        "url(x.png) red",
    ] {
        assert_eq!(style(&format!("background-color: lime; background: {good}")), RED, "{good}");
    }
    let not_yet = |attrs: &str| matches!(pixel(attrs), Err(Error::Unsupported(_)));
    // グラデーションと data: の画像の層はまだ描かない（包む関数の中も）
    for g in [
        "linear-gradient(red, red) lime",
        "-webkit-linear-gradient(top, red, red) lime",
        "url(a), radial-gradient(red, red)",
        "image-set(linear-gradient(red, red) 1x) lime",
        "-webkit-cross-fade(linear-gradient(red, red), url(b), 50%) lime",
        "url(\"data:image/svg+xml,%3Csvg/%3E\") lime",
        "url(data:image/gif;base64,R0lGOD) lime",
        // image() の色の画像と、前に空白のある data:（単位 3 の 5 回目のレビュー）
        "image(red) lime",
        "url(a), image-set(image(red) 1x) lime",
        "url(\" data:image/gif;base64,R0lGOD\") lime",
    ] {
        assert!(not_yet(&format!("style='background: {g}'")), "{g}");
    }
    assert!(not_yet("style=\"background-image: image(red)\""));
    // 印は後ろの background-color や長い名前の background-image で消えない（単位 3 の 4 回目のレビュー）
    assert!(not_yet("style=\"background: linear-gradient(red,red) lime; background-color: blue\""));
    assert!(not_yet("style=\"background-color: blue\"><style>svg { background: linear-gradient(red,red) }</style"));
    assert!(not_yet("style=\"background-image: linear-gradient(red,red)\""));
    assert!(not_yet("style=\"background-image: url(a.png), url('data:image/png;base64,AA')\""));
    // 画像の層を後ろで none にすれば描く
    assert_eq!(style("background: linear-gradient(red,red) lime; background-image: none"), LIME);
    assert_eq!(style("background-image: url(x.png); background-color: lime"), LIME);
    // 根を描かなくても背景は塗る（display: none と偽の条件の根は中身も描く。Chrome と同じ。ADR-102）
    assert_eq!(pixel("viewBox=\"0 0 0 0\" style=\"background: red\"").unwrap(), RED);
    assert_eq!(pixel("transform=\"scale(0)\" style=\"background: red\"").unwrap(), RED);
    assert_eq!(pixel("style=\"display: none; background: red\"").unwrap(), RED);
    assert_eq!(pixel("systemLanguage=\"xx\" style=\"background: red\"").unwrap(), RED);
    // 規則で書いても同じ（!important と後ろ勝ち）
    assert_eq!(
        pixel("class=\"k\"><style>.k { background: red !important } svg { background: lime }</style").unwrap(),
        RED
    );
}

/// 根の svg の background-color を描く（resvg・Chrome と同じ。ADR-97 (4)）。1 画素より細い線は 1 画素の線を太さの α で
/// 描く（Chrome・resvg の細い線。ADR-97 (2)）。
#[test]
fn background_and_hairlines() {
    let a = alpha_at(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"100\" style=\"background-color: #0000ff80\"><rect x=\"20\" y=\"20\" width=\"10\" height=\"10\"/></svg>",
    );
    assert_eq!((a(5, 5), a(25, 25)), (128, 255));
    let a = alpha_at(&svg("<path d=\"M10 50.5 H90\" stroke=\"black\" stroke-width=\"0.3\"/>"));
    // 1 画素の幅で、α は太さ（0.3 × 255 ≈ 77）
    assert!((70..=80).contains(&a(50, 50)), "{}", a(50, 50));
    assert_eq!(a(50, 49), 0);
}

/// 細い線は継ぎ目を作らない（Skia の細い線。角の外の画素は Chrome で 0）が、Chrome が枠の描き方で描く rect（角の丸みなし・
/// miter・miterlimit ≥ √2・破線なし・軸にそった変形）は角を四角くする（resvg の試験の枠の角の画素は Chrome で α 64。rotate(90)
/// の rect も同じ。ADR-103）。
#[test]
fn thin_rect_frames_keep_square_corners() {
    let corner = |body: &str| alpha_at(&svg(body))(0, 0);
    let s = "fill=\"none\" stroke=\"black\"";
    assert_eq!(corner(&format!("<rect x=\"1\" y=\"1\" width=\"10\" height=\"10\" {s}/>")), 64);
    assert_eq!(
        corner(&format!("<rect x=\"1\" y=\"1\" width=\"10\" height=\"10\" {s} transform=\"rotate(90 6 6)\"/>")),
        64
    );
    for other in [
        format!("<path d=\"M1 1 H11 V11 H1 Z\" {s}/>"),
        format!("<rect x=\"1\" y=\"1\" width=\"10\" height=\"10\" {s} stroke-linejoin=\"round\"/>"),
        format!("<rect x=\"1\" y=\"1\" width=\"10\" height=\"10\" {s} stroke-miterlimit=\"1.2\"/>"),
        format!("<rect x=\"1\" y=\"1\" width=\"10\" height=\"10\" rx=\"0.001\" {s}/>"),
    ] {
        assert_eq!(corner(&other), 0, "{other}");
    }
}

/// `background` のまとめを規則で多くの要素に当てても、読む段の手間は要素の数 × 値の長さにならない（宣言ごとに 1 度だけ色の
/// 範囲を求める。単位 3 の 3 回目のレビュー）。
#[test]
fn background_shorthand_is_checked_once_per_declaration() {
    let work = |layers: usize, elements: usize| {
        let value: String = (0..layers).map(|_| "url(a),").collect::<String>() + "red";
        let doc = svg(&format!("<style>*{{background: {value}}}</style>{}", "<g/>".repeat(elements)));
        crate::doc::estimate_parse(doc.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap().work
    };
    let (a, b) = (work(500, 500), work(1000, 1000));
    // 値と要素をどちらも 2 倍にしても、手間はおよそ 2 倍（2 乗なら 4 倍）
    assert!(b < a * 3, "{a} → {b}");
}

/// まだ描かないので NotYet にするもの（単位 3 の 4 回目のレビュー：どれも黙って違う絵になっていた）：CSS の個別の変形と
/// offset-path・display: contents・mask のまとめの形・clip-path の shape()。none と url ただ 1 つの mask は今のまま。
#[test]
fn more_not_yet_features() {
    let r = "<rect width=\"5\" height=\"5\"";
    for css in ["translate: 10px 10px", "rotate: 90deg", "scale: 3", "offset-path: path('M0 0 L12 12')"] {
        assert_eq!(not_yet(&format!("{r} style=\"{css}\"/>")), Later::CssTransform, "{css}");
    }
    assert_eq!(not_yet("<g style=\"display: contents\"><rect width=\"5\" height=\"5\"/></g>"), Later::DisplayContents);
    assert_eq!(not_yet(&format!("{r} style=\"display: contents\"/>")), Later::DisplayContents);
    let m = "<mask id=\"m\"><rect width=\"5\" height=\"5\"/></mask>";
    for css in ["mask: url(#m) luminance", "mask: url(#m) no-repeat", "mask: linear-gradient(transparent, transparent)"]
    {
        assert_eq!(not_yet(&format!("{m}{r} style=\"{css}\"/>")), Later::Mask, "{css}");
    }
    assert_eq!(not_yet(&format!("{m}{r} style=\"mask: url(#m)\"/>")), Later::Mask);
    for p in ["clip-path", "-webkit-clip-path"] {
        let css = format!("{p}: shape(from 0 0, line to 20px 0, line to 0 20px, close)");
        assert_eq!(not_yet(&format!("{r} style=\"{css}\"/>")), Later::Clip, "{css}");
    }
    // none は描く
    for css in ["translate: none", "rotate: none", "scale: none", "offset-path: none", "mask: none", "display: inline"]
    {
        assert!(build(&svg(&format!("{r} style=\"{css}\"/>"))).is_ok(), "{css}");
    }
}

/// 単位 3 の 5 回目のレビュー：overflow-x・overflow-y も入れ子の svg を切るかを決め（どちらかが見えない値なら切る側）、
/// content-visibility: hidden は入れ物・use・根の中身を描かず、`all` は direction・unicode-bidi のほかを同じ言葉にし、
/// `offset` のまとめは CSS の変形（NotYet）、根の display: contents は block（描く）。
#[test]
fn unit3_round5_properties() {
    let nested =
        |style: &str| format!("<svg width=\"5\" height=\"5\" style=\"{style}\"><rect width=\"9\" height=\"9\"/></svg>");
    for style in [
        "overflow: visible; overflow-x: hidden",
        "overflow-y: scroll",
        "overflow-x: visible",
        "overflow: hidden; overflow-x: visible; overflow-y: visible",
    ] {
        assert_eq!(not_yet(&nested(style)), Later::Viewport, "{style}");
    }
    for style in
        ["overflow: visible", "overflow-x: visible; overflow-y: auto", "overflow: visible; overflow-x: visible"]
    {
        assert_eq!(build(&svg(&nested(style))).unwrap().nodes.len(), 1, "{style}");
    }
    let count = |src: &str| build(src).unwrap().nodes.len();
    let r = "<rect width=\"5\" height=\"5\"/>";
    assert_eq!(count(&svg(&format!("<g style=\"content-visibility: hidden\">{r}</g>{r}"))), 1);
    assert_eq!(count(&svg(&format!("<g style=\"content-visibility: auto\">{r}</g>{r}"))), 2);
    assert_eq!(
        count(&svg(&format!(
            "<defs><g id=\"a\">{r}</g></defs><use href=\"#a\" style=\"content-visibility: hidden\"/>"
        ))),
        0
    );
    assert_eq!(
        count(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"9\" height=\"9\" style=\"content-visibility: hidden\">{r}</svg>"
        )),
        0
    );
    // all: initial は塗りを初期の黒に、all: inherit は親の塗りに（書いた属性より強い）
    let fill = |body: &str| build(&svg(body)).unwrap().nodes.as_slice().first().map(|n| n.fill);
    assert_eq!(fill(&format!("<g fill=\"red\" style=\"all: initial\">{r}</g>")), Some([0.0, 0.0, 0.0, 1.0]));
    assert_eq!(
        fill("<g fill=\"lime\"><g fill=\"red\" style=\"all: inherit\"><rect width=\"5\" height=\"5\"/></g></g>"),
        Some([0.0, 255.0, 0.0, 1.0])
    );
    // 前に書いた変形の個別のプロパティも all で初期に戻る
    assert!(build(&svg(&format!("<style>g {{ translate: 5px }}</style><g style=\"all: unset\">{r}</g>"))).is_ok());
    assert_eq!(not_yet(&format!("<g style=\"offset: path('M0 0 L9 9')\">{r}</g>")), Later::CssTransform);
    assert_eq!(
        count(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"9\" height=\"9\" style=\"display: contents\">{r}</svg>"
        )),
        1
    );
}

/// CSS の洗い出し（ADR-103）：根の箱のプロパティ（border・padding・margin・角の丸み・box-shadow・position・
/// scrollbar-gutter・根の mask）、どの要素の outline、rect の論理の大きさは、初期の値と同じ絵にならなければ NotYet。
/// `-webkit-opacity`・`-webkit-transform` は Chrome の別名として読む。
#[test]
fn css_box_properties() {
    let root = |style: &str| {
        build(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\" style=\"{style}\"><rect width=\"5\" height=\"5\"/></svg>"
        ))
    };
    let refused = |r: Result<Tree, Error>| match r {
        Err(Error::Unsupported(u)) => match u.feature {
            Feature::NotYet(l) => Some(l),
            _ => None,
        },
        _ => None,
    };
    for style in [
        "padding: 10px",
        "margin: 5px",
        "margin-inline-start: 1px",
        "border: 1px solid red",
        "border-left-style: dashed",
        "-webkit-border-before: 2px dotted",
        "border-radius: 4px",
        "-webkit-border-bottom-right-radius: 3px",
        "box-shadow: 0 0 2px red",
        "position: relative",
        "scrollbar-gutter: stable",
        "outline: auto",
        "border-image: linear-gradient(red, red) 1",
    ] {
        assert_eq!(refused(root(style)), Some(Later::CssBox), "{style}");
    }
    assert_eq!(refused(root("mask: url(#missing)")), Some(Later::Mask));
    for style in [
        "padding: 0",
        "margin: 0 auto",
        "border: none",
        "border: 0",
        "border-width: 5px; border-color: red",
        "border-radius: 0 0px 0%",
        "box-shadow: none",
        "position: static",
        "outline: none",
        "mask: none",
        "padding: initial",
        "margin: inherit",
    ] {
        assert!(root(style).is_ok(), "{style}");
    }
    // 規則で書いても同じ
    assert_eq!(
        refused(build(&svg("<style>svg { margin: 4px }</style><rect width=\"5\" height=\"5\"/>"))),
        Some(Later::CssBox)
    );
    // 根でない要素の箱のプロパティは効かない（Chrome と同じ）。outline はどの要素も、論理の大きさは rect だけ
    let r = "<rect width=\"5\" height=\"5\"";
    assert!(build(&svg(&format!("<g style=\"padding: 10px; border: 2px solid; margin: 3px\">{r}/></g>"))).is_ok());
    assert_eq!(not_yet(&format!("{r} style=\"outline: 1px solid red\"/>")), Later::CssBox);
    assert_eq!(not_yet(&format!("<g style=\"outline-style: auto\">{r}/></g>")), Later::CssBox);
    assert_eq!(not_yet(&format!("{r} style=\"inline-size: auto\"/>")), Later::CssBox);
    assert_eq!(not_yet(&format!("{r} style=\"-webkit-logical-height: 3px\"/>")), Later::CssBox);
    assert!(build(&svg(&format!("<g style=\"inline-size: 5px; outline: none\">{r}/></g>"))).is_ok());
    // outline: inherit は親の印
    assert_eq!(
        not_yet(&format!("<g style=\"outline: 1px solid\"><g style=\"outline: inherit\">{r}/></g></g>")),
        Later::CssBox
    );
    // Chrome の別名
    assert_eq!(not_yet(&format!("{r} style=\"-webkit-opacity: 0.5\"/>")), Later::Opacity);
    let t = build(&svg(&format!("{r} style=\"-webkit-transform: translate(10px, 0)\"/>"))).unwrap();
    assert_eq!(t.nodes.as_slice()[0].transform.e, 10.0);
    // CSS の width・height の大きさの言葉は auto と同じ（rect を描かない。Chrome と同じ）。属性では壊れた値
    for w in ["stretch", "min-content", "fit-content", "-webkit-fill-available"] {
        assert_eq!(build(&svg(&format!("{r} style=\"width: {w}\"/>"))).unwrap().nodes.len(), 0, "{w}");
    }
    assert_eq!(build(&svg("<rect width=\"stretch\" height=\"5\" style=\"width: 5px\"/>")).unwrap().nodes.len(), 1);
}

/// switch は SVG の名前空間の子から最初の 1 つを選ぶ：条件の属性を持つ要素は条件が通れば、持たない要素（title・知らない
/// 要素など）はそのまま選んで何も描かない。ほかの名前空間の要素と foreignObject（ADR-78 (6)）は飛ばす（Chrome と同じ。
/// resvg の structure/switch/non-SVG-child。ADR-103）。
#[test]
fn switch_selects_like_chrome() {
    let r = "<rect width=\"5\" height=\"5\"/>";
    let count = |first: &str| build(&svg(&format!("<switch>{first}{r}</switch>"))).unwrap().nodes.len();
    for first in [
        "<random/>",
        "<title>t</title>",
        "<desc/>",
        "<linearGradient id=\"g\"/>",
        "<random systemLanguage=\"xx\"/>",
        // clipPath と discard は条件を照らさずに選ぶ。XHTML の名前空間の拡張は真（単位 3 の 6 回目のレビュー。ADR-104）
        "<clipPath id=\"c\" systemLanguage=\"xx\"/>",
        "<discard systemLanguage=\"xx\"/>",
        "<g requiredExtensions=\"http://www.w3.org/1999/xhtml\"/>",
        "<g requiredExtensions=\"http://www.w3.org/1998/Math/MathML http://www.w3.org/1999/xhtml\"/>",
    ] {
        assert_eq!(count(first), 0, "{first}");
    }
    for first in [
        "<foo:bar xmlns:foo=\"http://x\"/>",
        "<rect width=\"5\" height=\"5\" systemLanguage=\"xx\"/>",
        "<g requiredExtensions=\"http://x\"/>",
        "<g requiredExtensions=\"\"/>",
        "<g requiredExtensions=\"http://www.w3.org/1999/xhtml http://x\"/>",
        "<symbol id=\"s\" systemLanguage=\"xx\"/>",
        "<foreignObject width=\"5\" height=\"5\"/>",
        "text ",
    ] {
        assert_eq!(count(first), 1, "{first}");
    }
}

/// 単位 3 の 6 回目のレビュー：CSS の箱のプロパティは Chrome と同じ形だけを受け（壊れた値は前の宣言を生かす）、根の中身を
/// 描かなくても確かめ、rect の論理の大きさは全体の言葉でも勝つ（all の分けたものは除く）。offset は道があるときだけ、
/// overflow: overlay は auto、CSS の width の大きさの関数は auto、背景の image-set の文字列と tab を含む URL も data:。
#[test]
fn unit3_round6() {
    let root = |attrs: &str| {
        build(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\" {attrs}><rect width=\"5\" height=\"5\"/></svg>"
        ))
    };
    let notyet = |r: Result<Tree, Error>| match r {
        Err(Error::Unsupported(u)) => match u.feature {
            Feature::NotYet(l) => Some(l),
            _ => None,
        },
        _ => None,
    };
    for style in [
        "border: 5px solid red; border: 5px soild red",
        "border-radius: 4px / 2px",
        "scrollbar-gutter: stable both-edges",
        "border-image-source: linear-gradient(red, red)",
        "-webkit-mask-box-image-source: linear-gradient(black, transparent)",
        "outline: thick dashed",
    ] {
        assert_eq!(notyet(root(&format!("style=\"{style}\""))), Some(Later::CssBox), "{style}");
    }
    for style in [
        "border: banana",
        "padding: banana",
        "border-radius: 0 / 0",
        "outline: 2px red",
        "position: banana",
        "margin: 0 auto 0",
        "border: 5px red",
    ] {
        assert!(root(&format!("style=\"{style}\"")).is_ok(), "{style}");
    }
    // 根の中身を描かない文書（viewBox の幅が 0）でも根の箱を確かめる
    assert_eq!(notyet(root("viewBox=\"0 0 0 10\" style=\"border: 5px solid red\"")), Some(Later::CssBox));
    assert_eq!(notyet(root("viewBox=\"0 0 0 10\" style=\"outline: 5px solid red\"")), Some(Later::CssBox));
    // 論理の大きさは全体の言葉でも幅の属性に勝つ。all の分けたものは勝たない
    let r = "<rect width=\"5\" height=\"5\"";
    for w in ["initial", "inherit", "unset", "revert"] {
        assert_eq!(not_yet(&format!("{r} style=\"inline-size: {w}\"/>")), Later::CssBox, "{w}");
    }
    assert_eq!(build(&svg(&format!("{r} style=\"all: initial; width: 5px; height: 5px\"/>"))).unwrap().nodes.len(), 1);
    // offset は道があるときだけ
    for css in ["offset: auto", "offset: normal", "offset: 30px 30px none", "offset: none"] {
        assert!(build(&svg(&format!("{r} style=\"{css}\"/>"))).is_ok(), "{css}");
    }
    for css in ["offset: border-box", "offset: path('M0 0 L5 5')", "offset: ray(45deg)"] {
        assert_eq!(not_yet(&format!("{r} style=\"{css}\"/>")), Later::CssTransform, "{css}");
    }
    // overflow: overlay は auto（入れ子の svg を切らない）
    let nested = "<svg width=\"5\" height=\"5\" style=\"overflow: overlay\"><rect width=\"9\" height=\"9\"/></svg>";
    assert_eq!(build(&svg(nested)).unwrap().nodes.len(), 1);
    // width の大きさの関数は auto
    for f in ["calc-size(auto, size)", "anchor-size(width)", "fit-content(10px)"] {
        assert_eq!(build(&svg(&format!("{r} style=\"width: {f}\"/>"))).unwrap().nodes.len(), 0, "{f}");
    }
    // 背景の image-set の文字列の data: と、tab を含む URL の data:
    for bg in ["image-set(\"data:image/png;base64,AA\" 1x) lime", "url(\"da\tta:image/png;base64,AA\") lime"] {
        let doc = svg(&format!("<style>svg {{ background: {bg} }}</style>"));
        assert_eq!(notyet(build(&doc)), Some(Later::BackgroundImage), "{bg}");
    }
    assert!(build(&svg("<style>svg { background: image-set(\"a.png\" 1x) lime }</style>")).is_ok());
}

/// 根の svg の display: none と偽の条件は描くことを止めない（Chrome と同じ。ADR-102。子の display: none は効く）。
#[test]
fn root_display_and_conditions_are_ignored() {
    let x = "xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"";
    for attrs in [
        "style=\"display:none\"",
        "display=\"none\"",
        "requiredExtensions=\"http://example.com/x\"",
        "systemLanguage=\"xx\"",
    ] {
        let t = build(&format!("<svg {x} {attrs}><rect width=\"10\" height=\"10\" fill=\"blue\"/></svg>")).unwrap();
        assert_eq!(t.nodes.len(), 1, "{attrs}");
    }
    let t = build(&format!("<svg {x}><style>svg {{ display: none }}</style><rect width=\"10\" height=\"10\"/></svg>"))
        .unwrap();
    assert_eq!(t.nodes.len(), 1);
    let t = build(&format!("<svg {x}><rect width=\"10\" height=\"10\" display=\"none\"/></svg>")).unwrap();
    assert_eq!(t.nodes.len(), 0);
}

/// 大きさを中身で決める文書：% は書いた幅・高さ（% は 100 に対する割合）、ない側は 100 で解き（usvg・Chrome と同じ。ADR-102）、
/// 根の変形の原点は既定だけ 0 0（書いた % は表示の領域で解く。usvg と同じ）。viewBox の幅か高さが 0 なら大きさは中身で決め、
/// 何も描かない（ADR-78 (7)）。
#[test]
fn content_sized_documents() {
    let size = |root: &str, body: &str| {
        let t = build(&format!("<svg xmlns=\"http://www.w3.org/2000/svg\" {root}>{body}</svg>")).unwrap();
        (t.size.width, t.size.height, t.nodes.len())
    };
    assert_eq!(size("width=\"40\"", "<rect width=\"50%\" height=\"30\"/>"), (20.0, 30.0, 1));
    assert_eq!(size("height=\"40\"", "<rect width=\"10\" height=\"50%\"/>"), (10.0, 20.0, 1));
    assert_eq!(size("width=\"30%\"", "<rect width=\"50%\" height=\"10\"/>"), (15.0, 10.0, 1));
    // 向きのない % は対角 √((40² + 100²) / 2) で解く（resvg の 8 × 8 と同じ半径）
    let r10 = 0.1 * ((40.0f64 * 40.0 + 100.0 * 100.0) / 2.0).sqrt();
    assert!((size("width=\"40\"", "<circle r=\"10%\" cx=\"10\" cy=\"10\"/>").0 - (10.0 + r10)).abs() < 1e-9);
    assert_eq!(size("", "<rect width=\"50%\" height=\"10\"/>"), (50.0, 10.0, 1));
    // 根の変形の原点：既定は 0 0、書いた % は 100 × 100 で解く
    let r = "<rect width=\"10\" height=\"10\"/>";
    assert_eq!(size("transform=\"scale(2)\"", r), (20.0, 20.0, 1));
    assert_eq!(size("transform=\"scale(2)\" style=\"transform-origin: 10% 10%\"", r), (10.0, 10.0, 1));
    assert_eq!(size("transform=\"scale(2)\" style=\"transform-origin: initial\"", r), (20.0, 20.0, 1));
    // 根の inherit も初期の値（単位 3 の 5 回目のレビュー）
    assert_eq!(size("transform=\"scale(2)\" style=\"transform-origin: inherit\"", r), (20.0, 20.0, 1));
    // viewBox の幅か高さが 0：大きさは中身、描かない。幾何に効かない NotYet（不透明度・混ぜ方・グラデーション）で断らない
    assert_eq!(size("viewBox=\"0 0 0 10\"", "<rect width=\"10\" height=\"30\"/>"), (10.0, 30.0, 0));
    for attrs in ["opacity=\"0.5\"", "style=\"mix-blend-mode: multiply\"", "fill=\"url(#g)\""] {
        let body = format!("<linearGradient id=\"g\"/><rect width=\"10\" height=\"30\" {attrs}/>");
        assert_eq!(size("viewBox=\"0 0 0 10\"", &body), (10.0, 30.0, 0), "{attrs}");
    }
}

/// 文脈の塗りの元が use の currentColor：描く要素（中身）の color で解く（Chrome と同じ。ADR-102）。
#[test]
fn context_paint_current_color_uses_the_drawn_element() {
    let t = build(&svg("<defs><rect id=\"r\" width=\"5\" height=\"5\" fill=\"context-fill\" color=\"blue\"/></defs>\
         <use href=\"#r\" fill=\"currentColor\" color=\"lime\"/>"))
    .unwrap();
    let n = t.nodes.as_slice().first().copied().unwrap();
    assert_eq!(n.fill, [0.0, 0.0, 255.0, 1.0]);
}
