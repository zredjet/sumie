#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::disallowed_types, clippy::indexing_slicing)]

use super::*;
use crate::cancel::Never;
use crate::style::ParseOptions;

/// id ごとの（辺の行き先の id と種類・取り除いたか）。
type EdgeRows = Vec<(String, Vec<(String, u8, bool)>)>;

/// 文書を読み、辺と、描かない要素の id と、展開の数と輪の警告の数。
fn run(body: &str) -> (EdgeRows, Vec<String>, u64, usize) {
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"10\" height=\"10\">{body}</svg>"
    );
    let d = crate::doc::parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    let doc = d.xml();
    let input = svg.as_bytes();
    let b = Budget::new(crate::error::Stage::Parse, u64::MAX, u64::MAX, &Never);
    let id_of = |n: u32| {
        doc.attr(input, doc.node(n).unwrap(), NS_NONE, b"id", &b)
            .unwrap()
            .map(|v| String::from_utf8_lossy(v).to_string())
            .unwrap_or_else(|| String::from_utf8_lossy(doc.node(n).unwrap().local.of(input)).to_string())
    };
    let refs = d.refs();
    let mut edges = Vec::new();
    let mut hidden = Vec::new();
    for (i, n) in doc.nodes.as_slice().iter().enumerate() {
        if n.kind != KIND_ELEMENT {
            continue;
        }
        let es: Vec<_> = refs.of(i as u32).iter().map(|e| (id_of(e.target), e.kind, e.removed != 0)).collect();
        if !es.is_empty() {
            edges.push((id_of(i as u32), es));
        }
        if refs.hidden.as_slice()[i] != 0 {
            hidden.push(id_of(i as u32));
        }
    }
    let cycles = d.report().warnings().filter(|w| w.kind == crate::report::WarningKind::ReferenceCycle).count();
    (edges, hidden, refs.instances, cycles)
}

#[test]
fn edges_by_kind_and_inheritance() {
    let (edges, _, _, cycles) = run(
        r##"<defs><linearGradient id="g"/><pattern id="p"><rect id="pr"/></pattern><clipPath id="c"/><mask id="m"/><filter id="f"/><marker id="mk"/></defs>
        <g id="grp" fill="url(#g)" style="stroke: url('#p') red"><rect id="r1"/><path id="p1" marker-start="url(#mk)"/><g fill="inherit"><circle id="c1" fill="initial"/></g></g>
        <rect id="r2" clip-path="url(#c)" mask="url(#m)" style="filter: url(#f) blur(1px) url(#nope)"/><use id="u" xlink:href="#r2"/>"##,
    );
    let get = |id: &str| edges.iter().find(|(i, _)| i == id).map(|(_, e)| e.clone()).unwrap_or_default();
    // 受け継いだ塗りは塗る要素から（g からは張らない）
    assert!(get("grp").is_empty());
    assert_eq!(get("r1"), [("g".into(), EDGE_FILL, false), ("p".into(), EDGE_STROKE, false)]);
    assert_eq!(
        get("p1"),
        [("g".into(), EDGE_FILL, false), ("p".into(), EDGE_STROKE, false), ("mk".into(), EDGE_MARKER_START, false)]
    );
    // initial は受け継ぎを切る
    assert_eq!(get("c1"), [("p".into(), EDGE_STROKE, false)]);
    // 辺はプロパティの番号の順（clip-path・filter・mask）
    assert_eq!(
        get("r2"),
        [("c".into(), EDGE_CLIP, false), ("f".into(), EDGE_FILTER, false), ("m".into(), EDGE_MASK, false)]
    );
    assert_eq!(get("u"), [("r2".into(), EDGE_USE, false)]);
    assert_eq!(cycles, 0);
}

#[test]
fn cycles_are_cut_once() {
    // 自分を指す use、祖先を指す use、pattern の中身が祖先から受け継いだ塗りで自分を描く
    let (edges, _, _, cycles) = run(r##"<use id="u0" href="#u0"/><g id="a"><use id="u1" href="#a"/></g>
        <g fill="url(#p)"><pattern id="p"><rect id="pr"/></pattern></g>"##);
    let get = |id: &str| edges.iter().find(|(i, _)| i == id).map(|(_, e)| e.clone()).unwrap_or_default();
    assert_eq!(get("u0"), [("u0".into(), EDGE_USE, true)]);
    assert_eq!(get("u1"), [("a".into(), EDGE_USE, true)]);
    assert_eq!(get("pr"), [("p".into(), EDGE_FILL, true)]);
    assert_eq!(cycles, 3);
    // 参照の辺で先に入った子に、子の辺で戻る輪（参照の辺を取り除く）
    let (edges, _, _, cycles) = run(
        r##"<use id="u0" href="#C"/><g id="P"><g id="C"><use id="uc" href="#X"/></g></g><g id="X"><use id="ux" href="#P"/></g>"##,
    );
    let removed: usize = edges.iter().map(|(_, e)| e.iter().filter(|x| x.2).count()).sum();
    assert_eq!(removed, 1);
    assert_eq!(cycles, 1);
}

#[test]
fn instances_count_expansions() {
    // use の爆弾：10 段 × 2 倍
    let mut body = String::from("<rect id=\"l0\"/>");
    for k in 1..=10 {
        body.push_str(&format!("<g id=\"l{k}\"><use href=\"#l{}\"/><use href=\"#l{}\"/></g>", k - 1, k - 1));
    }
    let (_, _, instances, _) = run(&body);
    assert!(instances > 2048, "{instances}");
    // defs の中の use の鎖は、描く参照の分だけ数える（n² にしない）。参照されない defs の中身は数えない
    let mut chain = String::from("<defs><rect id=\"c0\"/>");
    for k in 1..=1000 {
        chain.push_str(&format!("<g id=\"c{k}\"><use href=\"#c{}\"/></g>", k - 1));
    }
    chain.push_str("</defs>");
    let (_, _, unused, _) = run(&chain);
    assert_eq!(unused, 2, "根と defs");
    let (_, _, used, _) = run(&format!("{chain}<use href=\"#c1000\"/>"));
    // 根・defs・use と、c1000 から c1 までの g と use の 2 つずつ・c0
    assert_eq!(used, 3 + 2 * 1000 + 1);
    // symbol は use から入ったときだけ中身を数える
    let (_, _, a, _) = run(r##"<symbol id="s"><rect/><rect/></symbol>"##);
    let (_, _, b, _) = run(r##"<symbol id="s"><rect/><rect/></symbol><use href="#s"/><use href="#s"/>"##);
    // use は自分と、symbol と中身の 3 つ
    assert_eq!((a, b), (2, 2 + 2 * (1 + 3)));
    // グラデーションへの塗りは中身を数えない
    let (_, _, a, _) = run(r##"<linearGradient id="g"><stop/><stop/></linearGradient><rect fill="url(#g)"/>"##);
    let (_, _, b, _) = run(r##"<linearGradient id="g"><stop/><stop/></linearGradient><rect/>"##);
    assert_eq!(a, b);
}

#[test]
fn switch_and_conditions() {
    let (_, hidden, _, _) = run(
        r##"<switch><desc id="d"/><rect id="fr" systemLanguage="fr"/><rect id="en" systemLanguage="de, en-US"/><rect id="late"/></switch>
        <rect id="ext" requiredExtensions=""/><g id="g" systemLanguage="ja"><rect id="inner"/></g><rect id="feat" requiredFeatures="http://www.w3.org/TR/SVG11/feature#Shape"/>"##,
    );
    // 印は自分のものだけ（g の中の inner は、たどる側が g から受け継ぐ）
    assert_eq!(hidden, ["fr", "late", "ext", "g"]);
}

/// switch は foreignObject を選ばない（draw.io の書き出しの代わりの中身。ADR-78 (6)）。
#[test]
fn switch_skips_foreign_object() {
    let (_, hidden, _, _) = run(
        r##"<switch><foreignObject id="fo" requiredFeatures="http://www.w3.org/TR/SVG11/feature#Extensibility"><p xmlns="http://www.w3.org/1999/xhtml">x</p></foreignObject><text id="t">x</text><rect id="r"/></switch>"##,
    );
    assert!(!hidden.contains(&"t".to_string()), "{hidden:?}");
    assert!(hidden.contains(&"r".to_string()), "{hidden:?}");
}

/// 受け継がない参照（clip-path・mask・filter）の `inherit` は親の辺を写す（輪を見落とさず、展開を数え漏らさない。文書の
/// 1 回目のレビュー）。
#[test]
fn inherit_of_non_inherited_references() {
    let (edges, _, _, cycles) =
        run(r##"<g id="g" clip-path="url(#c)"><clipPath id="c" clip-path="inherit"><rect/></clipPath></g>"##);
    let get = |id: &str| edges.iter().find(|(i, _)| i == id).map(|(_, e)| e.clone()).unwrap_or_default();
    assert_eq!(get("c"), [("c".into(), EDGE_CLIP, true)]);
    assert_eq!(cycles, 1);
    let (_, _, _, cycles) = run(r##"<g filter="url(#f)"><filter id="f" filter="inherit"/></g>"##);
    assert_eq!(cycles, 1);
    let rects: String = (0..100).map(|_| "<rect/>").collect();
    let mask = format!("<defs><mask id=\"m\">{rects}</mask></defs>");
    let (_, _, plain, _) = run(&format!("{mask}<g mask=\"url(#m)\"><g><g/></g></g>"));
    let (_, _, inherited, _) =
        run(&format!("{mask}<g mask=\"url(#m)\"><g mask=\"inherit\"><g mask=\"inherit\"/></g></g>"));
    assert!(inherited >= plain + 200, "{plain} → {inherited}");
}

/// 種類の合わない参照先（塗りに g、clip-path に rect、グラデーションの href に pattern など）は辺にせず、輪にしない
/// （文書の 1 回目のレビュー。ADR-78 (8)）。
#[test]
fn wrong_kind_targets_make_no_edges() {
    for body in [
        r##"<g id="x"><use id="u1" href="#top"/></g><g id="top"><rect fill="url(#x) red"/></g>"##,
        r##"<g id="g"><rect fill="url(#g)"/></g>"##,
        r##"<g id="g"><rect clip-path="url(#g)" mask="url(#g)" filter="url(#g)"/></g>"##,
        r##"<g id="g"><path d="M0 0" marker-start="url(#g)"/></g>"##,
        r##"<pattern id="p"><linearGradient id="l" href="#p"/><rect fill="url(#l)"/></pattern>"##,
    ] {
        let (_, _, _, cycles) = run(body);
        assert_eq!(cycles, 0, "{body}");
    }
    // 種類が合えば今までどおり輪
    let (_, _, _, cycles) = run(r##"<pattern id="p"><rect fill="url(#p)"/></pattern>"##);
    assert_eq!(cycles, 1);
}

/// 子の辺で戻る輪を 1 度の歩きでまとめて取り除く（参照の辺を取り除いたら、その段から上を巻き戻す）。手間は輪の数に
/// おおよそ比例する（直す前は 4,000 個で 2.4 億単位・12,000 個で手間の上限。文書の 1 回目のレビュー）。
#[test]
fn many_child_edge_cycles_in_one_pass() {
    let work = |k: usize| {
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><use href=\"#c\"/><g id=\"top\"><g id=\"c\">{}</g></g></svg>",
            "<use href=\"#top\"/>".repeat(k)
        );
        let e = crate::doc::estimate_parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
        let d = crate::doc::parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
        let cycles = d.report().warnings().filter(|w| w.kind == crate::report::WarningKind::ReferenceCycle).count();
        (e.work, cycles)
    };
    let (a, ca) = work(2_000);
    let (b, cb) = work(4_000);
    assert_eq!((ca, cb), (2_000, 4_000));
    assert!(b < a * 3, "{a} → {b}");
}
