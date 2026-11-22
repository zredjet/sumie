#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use super::*;
use crate::cancel::Never;
use crate::css::sheet::SheetBuilder;
use crate::error::Stage;
use crate::limits::Limits;
use crate::style::ColorScheme;

/// 文書と CSS をカスケードし、id ごとの（プロパティの名前 → 値の文字列・どこから）。
type Row = (String, Vec<(String, String, u8)>);

fn run(svg: &str, css: &str) -> Vec<Row> {
    let input = svg.as_bytes();
    let b0 = Budget::new(Stage::EstimateParse, u64::MAX, u64::MAX, &Never);
    let counts = crate::xml::count(input, &Limits::DEFAULT, &b0, Stage::EstimateParse).unwrap();
    let b1 = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let doc = crate::xml::build(input, &counts, &Limits::DEFAULT, &b1, Stage::Parse).unwrap();
    let b2 = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut sb = SheetBuilder::new(&b2);
    sb.add(css.as_bytes(), None, 0).unwrap();
    let sheet = sb.finish().unwrap();
    let b3 = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let env = Env { color_scheme: ColorScheme::Light, size: Some((100.0, 100.0)) };
    let st = cascade(&doc, input, &sheet, &env, &b3).unwrap();
    assert!(b3.max_spend() < crate::budget::CANCEL_INTERVAL);
    assert_eq!(st.index.len(), doc.nodes.len() + 1);
    let mut out = Vec::new();
    for (i, n) in doc.nodes.as_slice().iter().enumerate() {
        if n.kind != crate::xml::KIND_ELEMENT {
            continue;
        }
        let id = doc
            .attr(input, n, NS_NONE, b"id")
            .map(|v| String::from_utf8_lossy(v).to_string())
            .unwrap_or_else(|| String::from_utf8_lossy(n.local.of(input)).to_string());
        let specs = st
            .of(i as u32)
            .iter()
            .map(|s| {
                let buf = match s.src {
                    SRC_SHEET => sheet.text.as_slice(),
                    SRC_STORE => doc.store.as_slice(),
                    _ => input,
                };
                let name = String::from_utf8_lossy(prop(s.prop).unwrap().name).to_string();
                let v = match s.wide {
                    1 => "inherit".to_string(),
                    2 => "initial".to_string(),
                    3 => "unset".to_string(),
                    _ => String::from_utf8_lossy(s.value.of(buf)).to_string(),
                };
                (name, v, s.origin)
            })
            .collect();
        out.push((id, specs));
    }
    out
}

fn get<'a>(out: &'a [Row], id: &str, prop: &str) -> Option<&'a str> {
    out.iter().find(|(i, _)| i == id)?.1.iter().find(|(p, _, _)| p == prop).map(|(_, v, _)| v.as_str())
}

const SVG: &str = "xmlns=\"http://www.w3.org/2000/svg\"";

#[test]
fn cascade_order() {
    let svg = format!(
        "<svg {SVG}><rect id=\"a\" class=\"c\" fill=\"red\" stroke=\"blue\" style=\"stroke: green; opacity: .5 !important\"/><rect id=\"b\" fill=\"red\"/></svg>"
    );
    let css = "rect { fill: yellow; stroke: black !important; opacity: 1 !important } #b { fill: purple } .c { fill: orange }";
    let out = run(&svg, css);
    // 規則は属性に勝ち、詳細度の高い規則が勝つ
    assert_eq!(get(&out, "a", "fill"), Some("orange"));
    assert_eq!(get(&out, "b", "fill"), Some("purple"));
    // 規則の !important は style 属性に勝ち、style 属性の !important は規則の !important に勝つ
    assert_eq!(get(&out, "a", "stroke"), Some("black"));
    assert_eq!(get(&out, "a", "opacity"), Some(".5"));
    // 同じ規則の後の宣言が勝つ・壊れた値は前の宣言を残す
    let out = run(&svg, "rect { fill: blue; fill: nonsense } rect { stroke: green; stroke: 1px }");
    assert_eq!(get(&out, "b", "fill"), Some("blue"));
    assert_eq!(get(&out, "b", "stroke"), Some("green"));
    // 属性より規則が強いので、a の style 属性（stroke: green）と規則が同じ値
    assert_eq!(get(&out, "a", "stroke"), Some("green"));
}

#[test]
fn layers_and_conditions() {
    let svg = format!("<svg {SVG}><rect id=\"a\"/></svg>");
    let css = "@layer base, top; @layer top { rect { fill: red; stroke: red !important } } @layer base { rect { fill: blue; stroke: blue !important } } rect { opacity: .3 } @layer top { rect { opacity: .9 } }";
    let out = run(&svg, css);
    // 後の層が勝つ・!important は前の層が勝つ・層のない規則が層の規則に勝つ
    assert_eq!(get(&out, "a", "fill"), Some("red"));
    assert_eq!(get(&out, "a", "stroke"), Some("blue"));
    assert_eq!(get(&out, "a", "opacity"), Some(".3"));
    let out = run(
        &svg,
        "@media print { rect { fill: red } } @media (min-width: 50px) { rect { stroke: green } } @supports (bogus: 1) { rect { opacity: .5 } }",
    );
    assert_eq!(get(&out, "a", "fill"), None);
    assert_eq!(get(&out, "a", "stroke"), Some("green"));
    assert_eq!(get(&out, "a", "opacity"), None);
}

#[test]
fn presentation_attributes() {
    let svg = format!(
        "<svg {SVG}><text id=\"t\" x=\"1 2 3\" font-size=\"20\" FILL=\"red\" mix-blend-mode=\"multiply\" transform=\"rotate(45)\"/><rect id=\"r\" x=\"5\" width=\"bogus\" style=\"marker: url(#m); FILL: Green\"/></svg>"
    );
    let out = run(&svg, "");
    // text の x は幾何のプロパティでない・大文字の属性は読まない・mix-blend-mode は属性で読まない
    assert_eq!(get(&out, "t", "x"), None);
    assert_eq!(get(&out, "t", "font-size"), Some("20"));
    assert_eq!(get(&out, "t", "fill"), None);
    assert_eq!(get(&out, "t", "mix-blend-mode"), None);
    assert_eq!(get(&out, "t", "transform"), Some("rotate(45)"));
    assert_eq!(get(&out, "r", "x"), Some("5"));
    // 壊れた属性の値は無視・style 属性のプロパティの名前は大文字小文字を問わない・marker は 3 つに分ける
    assert_eq!(get(&out, "r", "width"), None);
    assert_eq!(get(&out, "r", "fill"), Some("Green"));
    for m in ["marker-start", "marker-mid", "marker-end"] {
        assert_eq!(get(&out, "r", m), Some("url(#m)"));
    }
    // 全体の言葉
    let out = run(&format!("<svg {SVG}><g id=\"g\" fill=\"inherit\" style=\"stroke: unset\"/></svg>"), "");
    assert_eq!(get(&out, "g", "fill"), Some("inherit"));
    assert_eq!(get(&out, "g", "stroke"), Some("unset"));
}

#[test]
fn foreign_elements_have_no_styles() {
    let svg = format!("<svg {SVG} xmlns:x=\"urn:x\"><x:a id=\"f\" fill=\"red\"/><rect id=\"r\"/></svg>");
    let out = run(&svg, "* { stroke: blue }");
    assert!(out.iter().find(|(i, _)| i == "f").unwrap().1.is_empty());
    assert_eq!(get(&out, "r", "stroke"), Some("blue"));
}
