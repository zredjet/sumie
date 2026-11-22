#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

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
        // 並びの節（XML の節の数から後ろ）への辺は、並びの参照先に開いて見る（取り除いた印はどちらかの辺）
        let mut es: Vec<(String, u8, bool)> = Vec::new();
        for e in refs.of(i as u32) {
            if (e.target as usize) < doc.nodes.len() {
                es.push((id_of(e.target), e.kind, e.removed != 0));
            } else {
                for f in refs.of(e.target) {
                    es.push((id_of(f.target), e.kind, e.removed != 0 || f.removed != 0));
                }
            }
        }
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
        // marker の辺を塗りの辺より先に張る（展開の数の倍率のため）
        [("mk".into(), EDGE_MARKER_START, false), ("g".into(), EDGE_FILL, false), ("p".into(), EDGE_STROKE, false)]
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

/// 子の辺で戻る輪を 1 度の歩きでまとめて取り除く。手間は輪の数におおよそ比例する（直す前は 4,000 個で 2.4 億単位・
/// 12,000 個で手間の上限。文書の 1 回目のレビュー）。
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

/// use の中身は use から塗りと marker を受け継ぐので、use の塗りの参照先は中身の塗る要素の数の倍で数える（文書の 2 回目の
/// レビュー：数え漏らすと展開の爆弾が estimate_build を抜ける）。
#[test]
fn use_inherited_paints_multiply() {
    let rects: String = (0..10).map(|_| "<rect/>").collect();
    let mut body = format!("<defs><g id=\"g\">{rects}</g><pattern id=\"p0\"><rect/></pattern>");
    for k in 1..=8 {
        body.push_str(&format!("<pattern id=\"p{k}\"><use href=\"#g\" fill=\"url(#p{})\"/></pattern>", k - 1));
    }
    body.push_str("</defs><rect fill=\"url(#p8)\"/>");
    let (_, _, with_use, _) = run(&body);
    // use を書かずに同じ形（g の中の rect が受け継ぐ）
    let mut body = "<defs><pattern id=\"p0\"><rect/></pattern>".to_string();
    for k in 1..=8 {
        body.push_str(&format!("<pattern id=\"p{k}\"><g fill=\"url(#p{})\">{rects}</g></pattern>", k - 1));
    }
    body.push_str("</defs><rect fill=\"url(#p8)\"/>");
    let (_, _, direct, _) = run(&body);
    assert!(with_use >= direct / 2 && direct > 100_000_000, "{with_use} {direct}");
    // marker も中身の marker を付ける要素の数の倍
    let paths: String = (0..100).map(|_| "<path d=\"M0 0L1 1\"/>").collect();
    let mk: String = (0..100).map(|_| "<rect/>").collect();
    let (_, _, m, _) = run(&format!(
        "<defs><g id=\"g\">{paths}</g><marker id=\"k\">{mk}</marker></defs><use href=\"#g\" marker-end=\"url(#k)\"/>"
    ));
    assert!(m > 100 * 100, "{m}");
}

/// 展開の数は u64 の手前で止め（数え終えていない印と重ならない）、倍々の 100 段でも読む段は手間を使い切らない（文書の
/// 2 回目のレビュー）。
#[test]
fn saturated_counts_are_not_recounted() {
    let mut body = String::from("<defs><rect id=\"l0\"/>");
    for k in 1..=100 {
        body.push_str(&format!("<g id=\"l{k}\"><use href=\"#l{p}\"/><use href=\"#l{p}\"/></g>", p = k - 1));
    }
    body.push_str("</defs><use href=\"#l100\"/>");
    let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{body}</svg>");
    let e = crate::doc::estimate_parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    assert!(e.work < 10_000_000, "{}", e.work);
    let d = crate::doc::parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    assert!(
        matches!(d.estimate_build(&Never), Err(crate::error::Error::Limit(l)) if l.resource == crate::error::Resource::Instances)
    );
}

/// P の子が多く、Q が深い形の輪も 1 度の歩きで取り除き、手間が節と辺の数に比例する（文書の 2 回目・3 回目のレビュー）。
#[test]
fn many_cycles_with_deep_chains_are_linear() {
    let work = |k: usize, d: usize| {
        let uses: String = (0..k).map(|i| format!("<use href=\"#c{i}\"/>")).collect();
        let inner: String = (0..k).map(|i| format!("<use id=\"c{i}\" href=\"#Q\"/>")).collect();
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\">{uses}<g id=\"Q\">{}<g id=\"P\">{inner}</g>{}</g></svg>",
            "<g>".repeat(d),
            "</g>".repeat(d)
        );
        crate::doc::estimate_parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap().work
    };
    let (a, b) = (work(1_000, 500), work(2_000, 500));
    assert!(b < a * 3, "{a} → {b}");
    // 深さを倍にしても手間は倍ほど（K × D に伸びない）
    let c = work(2_000, 1_000);
    assert!(c < b * 3 && c < 20_000_000, "{b} → {c}");
}

/// 受け継がない参照の並びは値ごとに 1 つの節で共有する（inherit は親の並びを指す。辺と確保が要素の数と値の長さの和に
/// 比例する。文書の 2 回目のレビュー）。
#[test]
fn reference_lists_are_shared() {
    let peak = |svg: String| crate::doc::estimate_parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    let filters: String = (0..2_000).map(|i| format!("<filter id=\"f{i}\"/>")).collect();
    let urls: String = (0..2_000).map(|i| format!("url(#f{i}) ")).collect();
    let e = peak(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><style>g g{{filter:inherit}}</style>{filters}<g style=\"filter:{urls}\">{}{}</g></svg>",
        "<g>".repeat(500),
        "</g>".repeat(500)
    ));
    assert!(e.peak_bytes < 8 << 20, "{}", e.peak_bytes);
    let e = peak(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><style>rect{{filter:{}}}</style><filter id=\"f\"/>{}</svg>",
        "url(#f) ".repeat(5_000),
        "<rect/>".repeat(5_000)
    ));
    assert!(e.work < 100_000_000, "{}", e.work);
    // 並びの輪も見つける（要素の辺を取り除く）
    let (edges, _, _, cycles) = run(
        r##"<style>.a{filter:url(#f)}</style><filter id="f"><feImage id="i" class="a"/></filter><rect id="r" class="a"/>"##,
    );
    assert_eq!(cycles, 1);
    let get = |id: &str| edges.iter().find(|(i, _)| i == id).map(|(_, e)| e.clone()).unwrap_or_default();
    assert_eq!(get("r"), [("f".into(), EDGE_FILTER, false)]);
}

/// textPath・tref の href は描き直さないので輪にしない（文書の 2 回目のレビュー）。
#[test]
fn text_hrefs_are_not_cycles() {
    for body in [
        r##"<text id="t">ab<tref xlink:href="#t"/></text>"##,
        r##"<g id="g"><text><textPath href="#g">x</textPath></text></g>"##,
    ] {
        let (_, _, _, cycles) = run(body);
        assert_eq!(cycles, 0, "{body}");
    }
}

/// marker の中の context-fill・context-stroke は、marker を付けた要素の塗りの参照先の中身を作るので、塗りの参照先は marker の
/// 中の塗る要素の数の倍で数える（入れ子にした展開の爆弾を estimate_build で断る。文書の 3 回目のレビュー）。
#[test]
fn context_paint_in_markers_multiplies() {
    let k = 4;
    let ctx: String = (0..k).map(|_| "<rect fill=\"context-fill\"/>").collect();
    let mut body = String::from("<defs><pattern id=\"p0\"><rect/></pattern>");
    for i in 1..=8 {
        body.push_str(&format!(
            "<marker id=\"m{i}\">{ctx}</marker><pattern id=\"p{i}\"><path d=\"M0 0L1 1L2 2\" fill=\"url(#p{})\" marker-start=\"url(#m{i})\" marker-end=\"url(#m{i})\"/></pattern>",
            i - 1
        ));
    }
    body.push_str("</defs><rect fill=\"url(#p8)\"/>");
    let (_, _, n, _) = run(&body);
    // 1 段ごとに (2K + 1) 倍
    assert!(n > 9u64.pow(7), "{n}");
}

/// 種つきの xorshift（試験の入力を作るだけ）。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// 参照の多い乱数の文書。
fn random_doc(r: &mut Rng, size: usize) -> String {
    let names = ["g", "rect", "use", "pattern", "mask", "filter", "clipPath", "marker", "path", "symbol"];
    let mut s = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\">");
    let mut open: Vec<&str> = Vec::new();
    for i in 0..size {
        if r.below(4) == 0
            && let Some(t) = open.pop()
        {
            s.push_str(&format!("</{t}>"));
            continue;
        }
        let name = names[r.below(names.len() as u64) as usize];
        let target = |r: &mut Rng| format!("#e{}", r.below(size as u64));
        let mut attrs = format!(" id=\"e{i}\"");
        match r.below(6) {
            0 => attrs.push_str(&format!(" href=\"{}\"", target(r))),
            1 => attrs.push_str(&format!(" fill=\"url({})\"", target(r))),
            2 => attrs.push_str(&format!(" mask=\"url({})\"", target(r))),
            3 => attrs.push_str(&format!(" filter=\"url({}) url({})\"", target(r), target(r))),
            4 => attrs.push_str(&format!(" clip-path=\"url({})\" marker-end=\"url({})\"", target(r), target(r))),
            _ => {}
        }
        if r.below(2) == 0 {
            s.push_str(&format!("<{name}{attrs}/>"));
        } else {
            s.push_str(&format!("<{name}{attrs}>"));
            open.push(name);
        }
    }
    while let Some(t) = open.pop() {
        s.push_str(&format!("</{t}>"));
    }
    s.push_str("</svg>");
    s
}

/// 輪を取り除いたあとの図に輪がなく、取り除いた辺はどれももとの図の輪の上にある（乱数の文書 3,000 件。文書の 3 回目の
/// レビュー）。
#[test]
fn removed_edges_are_on_cycles_and_leave_no_cycle() {
    let mut r = Rng(0x0c0f_fee5_1234_5678);
    let mut removed_total = 0usize;
    for case in 0..3_000 {
        let size = 5 + r.below(40) as usize;
        let svg = random_doc(&mut r, size);
        let d = crate::doc::parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
        let doc = d.xml();
        let refs = d.refs();
        let total = refs.start.len() - 1;
        // 図（子の辺と、文字の辺を除く参照の辺）。`all` は取り除く前の図
        let mut live: Vec<Vec<usize>> = vec![Vec::new(); total];
        let mut all: Vec<Vec<usize>> = vec![Vec::new(); total];
        for (i, n) in doc.nodes.as_slice().iter().enumerate() {
            let mut c = n.first_child;
            while c != NONE {
                let x = doc.node(c).unwrap();
                if x.kind == KIND_ELEMENT {
                    live[i].push(c as usize);
                    all[i].push(c as usize);
                }
                c = x.next_sibling;
            }
        }
        let mut removed = Vec::new();
        for (i, adj) in live.iter_mut().enumerate() {
            for e in refs.of(i as u32) {
                if e.kind == EDGE_TEXT {
                    continue;
                }
                all[i].push(e.target as usize);
                if e.removed == 0 {
                    adj.push(e.target as usize);
                } else {
                    removed.push((i, e.target as usize));
                }
            }
        }
        removed_total += removed.len();
        // 残りの図に輪がない（入りの数で並べきれる）
        let mut indeg = vec![0usize; total];
        for adj in &live {
            for &t in adj {
                indeg[t] += 1;
            }
        }
        let mut queue: Vec<usize> = (0..total).filter(|&i| indeg[i] == 0).collect();
        let mut seen = 0usize;
        while let Some(v) = queue.pop() {
            seen += 1;
            for &t in &live[v] {
                indeg[t] -= 1;
                if indeg[t] == 0 {
                    queue.push(t);
                }
            }
        }
        assert_eq!(seen, total, "{case} 件目：輪が残る\n{svg}");
        // 取り除いた辺 u → v は、もとの図で v から u に届く（輪の上にある）
        for &(u, v) in &removed {
            let mut mark = vec![false; total];
            let mut st = vec![v];
            mark[v] = true;
            while let Some(x) = st.pop() {
                for &y in &all[x] {
                    if !mark[y] {
                        mark[y] = true;
                        st.push(y);
                    }
                }
            }
            assert!(mark[u], "{case} 件目：輪でない辺 {u} → {v} を取り除いた\n{svg}");
        }
        // 警告の数は取り除いた辺の数
        let warns = d.report().warnings().filter(|w| w.kind == crate::report::WarningKind::ReferenceCycle).count();
        assert_eq!(warns, removed.len(), "{case} 件目\n{svg}");
    }
    assert!(removed_total > 1_000, "{removed_total}");
}

/// 輪の鎖（pattern X の中の mask の鎖が X を塗る）でも 1 度の歩きで取り除き、手間が鎖の長さに比例する（文書の 3 回目の
/// レビュー：取り残しの歩きは輪ごとに歩き直していた）。
#[test]
fn chained_cycles_in_one_walk() {
    let work = |k: usize| {
        let masks: String = (1..=k)
            .map(|i| format!("<mask id=\"Y{i}\"><rect fill=\"url(#X)\" mask=\"url(#Y{})\"/></mask>", i - 1))
            .collect();
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><pattern id=\"X\"><mask id=\"Y0\"><rect/></mask>{masks}</pattern><rect mask=\"url(#Y{k})\"/></svg>"
        );
        crate::doc::estimate_parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap().work
    };
    let (a, b) = (work(1_000), work(2_000));
    assert!(b < a * 3, "{a} → {b}");
}
