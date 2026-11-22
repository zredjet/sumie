#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::unwrap_used
)]

use super::*;
use crate::cancel::Never;
use crate::style::ParseOptions;

/// id ごとの（たどった要素, 使う塗りの fill・stroke）。
type Rows = Vec<(String, WalkItem, Used, Used)>;

fn run(svg: &str) -> Rows {
    let d = crate::doc::parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    let b = Budget::new(crate::error::Stage::Build, u64::MAX, u64::MAX, &Never);
    let mut out = Vec::new();
    walk(&d, &b, &mut |v| {
        let n = d.xml().node(v.node).unwrap();
        let id = d.xml().attr(d.input(), n, NS_NONE, b"id", &b)?.map(|x| String::from_utf8_lossy(x).to_string());
        let fill = used_paint(&d, v.node, &v.computed.fill, true, v.computed.color, &b)?;
        let stroke = used_paint(&d, v.node, &v.computed.stroke, false, v.computed.color, &b)?;
        if let Some(id) = id {
            out.push((id, *v, fill, stroke));
        }
        Ok(())
    })
    .unwrap();
    assert!(b.max_spend() < crate::budget::CANCEL_INTERVAL);
    out
}

fn get<'a>(rows: &'a Rows, id: &str) -> &'a (String, WalkItem, Used, Used) {
    rows.iter().find(|r| r.0 == id).unwrap_or_else(|| panic!("{id} がない"))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn ctm(r: &(String, WalkItem, Used, Used)) -> [f64; 6] {
    let t = r.1.ctm;
    [t.a, t.b, t.c, t.d, t.e, t.f]
}

fn ctm_is(r: &(String, WalkItem, Used, Used), want: [f64; 6]) {
    let got = ctm(r);
    assert!(got.iter().zip(want).all(|(&a, b)| close(a, b)), "{}：{got:?} ≠ {want:?}", r.0);
}

const X: &str = r#"xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink""#;

#[test]
fn inheritance_and_current_color() {
    let rows = run(&format!(
        r##"<svg {X} width="100" height="100"><g id="g" fill="currentColor" color="red" stroke="blue" stroke-opacity="25%" opacity=".5">
        <rect id="a" color="#0000ff"/><rect id="b" style="color: currentColor"/><rect id="c" fill-opacity="2" opacity="-1"/>
        <rect id="d" fill="inherit" stroke="initial"/></g></svg>"##
    ));
    // currentColor は受け継いだ先の color で解く（CSS Color 4）
    assert_eq!(get(&rows, "a").2, Used::Color(Color { r: 0.0, g: 0.0, b: 255.0, a: 1.0 }));
    assert_eq!(get(&rows, "g").2, Used::Color(Color { r: 255.0, g: 0.0, b: 0.0, a: 1.0 }));
    // color の currentColor は inherit
    assert_eq!(get(&rows, "b").2, Used::Color(Color { r: 255.0, g: 0.0, b: 0.0, a: 1.0 }));
    // 不透明度は 0〜1 に収め、opacity は受け継がない
    let c = get(&rows, "c").1.computed;
    assert_eq!((c.fill_opacity, c.stroke_opacity, c.opacity), (1.0, 0.25, 0.0));
    assert_eq!(get(&rows, "a").1.computed.opacity, 1.0);
    // inherit・initial
    assert_eq!(get(&rows, "d").3, Used::None);
    assert_eq!(get(&rows, "d").2, Used::Color(Color { r: 255.0, g: 0.0, b: 0.0, a: 1.0 }));
}

#[test]
fn lengths_resolve_in_context() {
    let rows = run(&format!(
        r##"<svg {X} width="300" height="400" viewBox="0 0 30 40" style="font-size: 20px; line-height: 1.5">
        <g font-size="10" style="line-height: 2"><rect id="em" stroke-width="0.5em"/><rect id="lh" style="stroke-width: 1lh"/>
        <rect id="rem" style="stroke-width: 1rem"/><rect id="rlh" style="stroke-width: 1rlh"/><rect id="ex" stroke-width="1ex"/></g>
        <rect id="pct" stroke-width="10%"/><rect id="vw" style="stroke-width: 10vw"/>
        <g font-size="larger"><rect id="larger" stroke-width="1em"/></g><g font-size="x-large"><rect id="xl" stroke-width="1em"/></g>
        <g font-size="50%"><rect id="half" stroke-width="1em"/></g></svg>"##
    ));
    let w = |id: &str| get(&rows, id).1.computed.stroke_width;
    assert!(close(w("em"), 5.0));
    assert!(close(w("lh"), 20.0));
    // rem・rlh は根の要素の文字の大きさ・行の高さ
    assert!(close(w("rem"), 20.0));
    assert!(close(w("rlh"), 30.0));
    assert!(close(w("ex"), 5.0));
    // % は viewBox の対角 ÷ √2（50 ÷ √2）
    assert!(close(w("pct"), 50.0 / core::f64::consts::SQRT_2 / 10.0), "{}", w("pct"));
    // vw は文書の大きさ
    assert!(close(w("vw"), 30.0));
    // 言葉は既定の大きさ（12）から、larger は親の 1.2 倍
    assert!(close(w("larger"), 24.0));
    assert!(close(w("xl"), 18.0));
    assert!(close(w("half"), 10.0));
}

#[test]
fn transforms_and_viewports() {
    let rows = run(&format!(
        r##"<svg {X} width="100" height="50" viewBox="0 0 20 10">
        <g id="g" transform="translate(1 2)"><rect id="r" transform="scale(2)"/></g>
        <rect id="o" transform="rotate(90)" transform-origin="10 10"/>
        <rect id="css" style="transform: translate(1px, 50%) rotate(0.25turn)"/>
        <rect id="box" style="transform: rotate(10deg); transform-box: fill-box"/>
        <rect id="pctbox" style="transform: translate(50%); transform-box: fill-box"/>
        <defs><rect id="inDefs"/></defs><use id="u" xlink:href="#inDefs" x="5" y="1" transform="rotate(90)"/>
        <svg id="s" x="2" y="2" width="4" height="4" viewBox="0 0 2 2"><rect id="in"/></svg>
        <svg id="s0" width="0"><rect id="in0"/></svg></svg>"##
    ));
    // 根の viewBox の変形（5 倍）× 祖先 × 自分
    ctm_is(get(&rows, "g"), [5.0, 0.0, 0.0, 5.0, 5.0, 10.0]);
    ctm_is(get(&rows, "r"), [10.0, 0.0, 0.0, 10.0, 5.0, 10.0]);
    // transform-origin のまわりの回転：(0, 0) は (20, 0) に（根の 5 倍を掛けて (100, 0)）
    ctm_is(get(&rows, "o"), [0.0, 5.0, -5.0, 0.0, 100.0, 0.0]);
    // CSS の transform：50% は参照の箱（view-box：viewBox の 20 × 10）の高さ
    ctm_is(get(&rows, "css"), [0.0, 5.0, -5.0, 0.0, 5.0, 25.0]);
    // 外接矩形の要る箱は解けない印（単位の行列なら解ける）
    assert!(!get(&rows, "box").1.ctm_known);
    assert!(!get(&rows, "pctbox").1.ctm_known);
    assert!(get(&rows, "r").1.ctm_known);
    // use は自分の変形のあとに x・y
    ctm_is(get(&rows, "u"), [0.0, 5.0, -5.0, 0.0, -5.0, 25.0]);
    // 入れ子の svg：自分には表示の領域の変形を含めず、子に x・y と viewBox（2 倍）
    ctm_is(get(&rows, "s"), [5.0, 0.0, 0.0, 5.0, 0.0, 0.0]);
    ctm_is(get(&rows, "in"), [10.0, 0.0, 0.0, 10.0, 10.0, 10.0]);
    assert!(get(&rows, "s").1.rendered && get(&rows, "in").1.rendered);
    // 幅 0 の入れ子の svg は描かない
    assert!(!get(&rows, "s0").1.rendered && !get(&rows, "in0").1.rendered);
}

#[test]
fn rendered_subtrees() {
    let rows = run(&format!(
        r##"<svg {X} width="10" height="10"><g id="none" display="none"><rect id="under"/></g><defs><rect id="def"/></defs>
        <switch><rect id="fr" systemLanguage="fr"/><rect id="any"/><rect id="late"/></switch>
        <foo id="unknown"><rect id="inUnknown"/></foo><g id="vis" visibility="hidden"><rect id="hid"/><rect id="shown" visibility="visible"/></g>
        <clipPath id="clip"><rect id="inClip"/></clipPath><rect id="plain"><rect id="inRect"/></rect>
        <g id="zero" transform="matrix(0 0 0 0 0 0)"><rect id="inZero"/></g><text id="t"><tspan id="ts"/></text></svg>"##
    ));
    let r = |id: &str| get(&rows, id).1.rendered;
    // 図形の子・逆にできない変形の部分木も描かない
    for id in
        ["none", "under", "def", "fr", "late", "unknown", "inUnknown", "clip", "inClip", "inRect", "zero", "inZero"]
    {
        assert!(!r(id), "{id}");
    }
    for id in ["any", "vis", "hid", "shown", "plain", "t", "ts"] {
        assert!(r(id), "{id}");
    }
    // visibility は受け継ぐ
    assert_eq!(get(&rows, "hid").1.computed.visibility, crate::style::computed::HIDDEN);
    assert_eq!(get(&rows, "shown").1.computed.visibility, crate::style::computed::VISIBLE);
}

#[test]
fn paint_servers_and_fallbacks() {
    let rows = run(&format!(
        r##"<svg {X} width="10" height="10"><defs><linearGradient id="lg"/><pattern id="p"><rect id="self" fill="url(#p)"/></pattern></defs>
        <rect id="a" fill="url(#lg)" stroke="url('#missing') #00ff00"/><rect id="b" fill="url(#a) none" stroke="url(#a)"/>
        <rect id="c" fill="url(other.svg#lg) currentColor" color="#ff0000"/><rect id="e" style="fill: url(\23 lg)"/></svg>"##
    ));
    assert!(matches!(get(&rows, "a").2, Used::Server(_)));
    assert_eq!(get(&rows, "a").3, Used::Color(Color { r: 0.0, g: 255.0, b: 0.0, a: 1.0 }));
    // 塗りの参照先でない要素は代わり（書いていなければ none）
    assert_eq!(get(&rows, "b").2, Used::None);
    assert_eq!(get(&rows, "b").3, Used::None);
    // 外の文書は読まずに代わり
    assert_eq!(get(&rows, "c").2, Used::Color(Color { r: 255.0, g: 0.0, b: 0.0, a: 1.0 }));
    // escape を解いて引く
    assert!(matches!(get(&rows, "e").2, Used::Server(_)));
    // 輪で取り除いた塗り
    assert_eq!(get(&rows, "self").2, Used::Cut);
}

/// 根の要素が名前空間のない `svg` の文書は、名前空間のない要素を SVG の要素として読む（usvg と同じ。ADR-78 (2)）。
#[test]
fn namespace_less_root_svg_is_svg() {
    let rows = run(
        r##"<svg width="20" height="10" viewBox="0 0 10 5"><style>#a{fill:red}</style><rect id="a" width="5" height="5"/><g id="g" fill="#00ff00"><rect id="b"/></g><g xmlns=""><rect id="c"/></g></svg>"##,
    );
    assert!(get(&rows, "a").1.rendered && get(&rows, "b").1.rendered);
    assert_eq!(get(&rows, "a").2, Used::Color(Color { r: 255.0, g: 0.0, b: 0.0, a: 1.0 }));
    assert_eq!(get(&rows, "b").2, Used::Color(Color { r: 0.0, g: 255.0, b: 0.0, a: 1.0 }));
    ctm_is(get(&rows, "a"), [2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
    // xmlns="" を書いた部分木は名前空間なしのまま（描かない）
    assert!(!get(&rows, "c").1.rendered);
    // 根に xmlns="" を書いた文書・根が svg でない文書は名前空間なし
    for doc in [
        r#"<svg xmlns="" width="10" height="10"><rect id="d"/></svg>"#,
        r#"<foo><svg width="10" height="10"><rect id="d"/></svg></foo>"#,
    ] {
        let rows = run(doc);
        assert!(!get(&rows, "d").1.rendered, "{doc}");
    }
    // 大きさも読む
    let d = crate::doc::parse(b"<svg width=\"20\" height=\"10\"/>", &ParseOptions::DEFAULT, &Never).unwrap();
    assert_eq!(d.size().map(|s| (s.width, s.height)), Some((20.0, 10.0)));
}

/// 幅か高さが 0 の viewBox の svg は描かない（大きさは viewBox がないものとして決める。ADR-78 (7)）。
#[test]
fn zero_view_box_is_not_drawn() {
    let rows = run(&format!(
        r##"<svg {X} width="10" height="10"><svg id="s" viewBox="0 0 0 10"><rect id="in"/></svg><rect id="out"/></svg>"##
    ));
    assert!(!get(&rows, "s").1.rendered && !get(&rows, "in").1.rendered);
    assert!(get(&rows, "out").1.rendered);
    let src = format!(r##"<svg {X} id="root" width="10" height="10" viewBox="0 0 10 0"><rect id="r"/></svg>"##);
    let rows = run(&src);
    assert!(!get(&rows, "root").1.rendered && !get(&rows, "r").1.rendered);
    let d = crate::doc::parse(src.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    assert_eq!(d.size().map(|s| (s.width, s.height)), Some((10.0, 10.0)));
}
