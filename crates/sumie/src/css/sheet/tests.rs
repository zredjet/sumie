#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use super::*;
use crate::cancel::Never;
use crate::css::select::{S_TYPE, Simple};
use crate::error::Stage;

fn sheet(sheets: &[(&str, Option<&str>)]) -> Sheet {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    // 数える段と同じに伸ばしながら読み、測った量で読み直して同じになることも確かめる
    let mut a = SheetBuilder::new(&budget);
    for (k, (css, media)) in sheets.iter().enumerate() {
        a.add(css.as_bytes(), media.map(str::as_bytes), k as u32 * 100).unwrap();
    }
    let counts = a.counts();
    let budget2 = Box::leak(Box::new(Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never)));
    let mut b = SheetBuilder::with_counts(budget2, &counts).unwrap();
    for (k, (css, media)) in sheets.iter().enumerate() {
        b.add(css.as_bytes(), media.map(str::as_bytes), k as u32 * 100).unwrap();
    }
    assert_eq!(b.counts(), counts);
    let sh = b.finish().unwrap();
    assert_eq!(sh.bytes(), counts.bytes());
    assert!(budget2.max_spend() < crate::budget::CANCEL_INTERVAL);
    sh
}

/// 規則ごとの（最初の選択子の右端の型の名前、宣言の名前と値、条件の字句、層の順位）。
type Row = (String, Vec<(String, String, bool)>, Vec<String>, u32);

fn rules(sh: &Sheet) -> Vec<Row> {
    let text = sh.text.as_slice();
    let names = sh.names.as_slice();
    sh.rules
        .as_slice()
        .iter()
        .map(|r| {
            let sel = sh.selectors.as_slice()[r.selector as usize];
            let comp = sh.compounds.as_slice()[sel.subject as usize];
            let first: Simple = sh.simples.as_slice()[comp.first as usize];
            let ty =
                if first.kind == S_TYPE { String::from_utf8_lossy(first.a.of(names)).to_string() } else { "?".into() };
            let decls = sh.decls.as_slice()[r.decls.start as usize..r.decls.end as usize]
                .iter()
                .map(|d| {
                    (
                        String::from_utf8_lossy(d.name.of(text)).to_string(),
                        String::from_utf8_lossy(d.value.of(text)).to_string(),
                        d.important != 0,
                    )
                })
                .collect();
            let mut conds = Vec::new();
            let mut c = r.cond;
            while c != NONE {
                let cond = sh.conds.as_slice()[c as usize];
                conds.push(String::from_utf8_lossy(cond.text.of(text)).to_string());
                c = cond.parent;
            }
            (ty, decls, conds, sh.rank(r))
        })
        .collect()
}

#[test]
fn rules_and_declarations() {
    let sh = sheet(&[("rect { fill: red; stroke: blue !important } /* c */ g{opacity:.5}", None)]);
    let r = rules(&sh);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].0, "rect");
    assert_eq!(r[0].1, [("fill".into(), "red".into(), false), ("stroke".into(), "blue".into(), true)]);
    assert_eq!(r[1].0, "g");
    assert_eq!(sh.rules.as_slice().iter().map(|r| r.order).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(r[0].3, u32::MAX);
}

#[test]
fn recovery() {
    // 壊れたセレクタの規則は塊ごと捨て、次の規則は当てる・`<!--`・`-->` は飛ばす・閉じない規則は終わりで閉じる
    let sh = sheet(&[("<!-- g {fill:green} --> circle:bogus {x:y} path { fill: blue", None)]);
    let r = rules(&sh);
    assert_eq!(r.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["g", "path"]);
    assert_eq!(r[1].1, [("fill".into(), "blue".into(), false)]);
    let warnings: Vec<WarningKind> = sh.notes.as_slice().iter().map(|n| n.kind).collect();
    assert_eq!(warnings, [WarningKind::InvalidSelector]);
    // 前置きの `[` は `]` まで読む（CSS Syntax の simple block）。閉じなければ終わりまで前置きになり、規則は捨てる（Chrome と同じ）
    let sh = sheet(&[("rect[ {fill:red} g {fill:green}", None)]);
    assert_eq!(rules(&sh).len(), 0);
    let sh = sheet(&[("rect[ {fill:red}] {fill: red} g {fill:green}", None)]);
    assert_eq!(rules(&sh).iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["g"]);
    // 前置きのかっこの中の `{` は塊でない（閉じないので規則は捨てる）
    let sh = sheet(&[("rect:is(.a {fill:red} g {fill: green}", None)]);
    assert_eq!(rules(&sh).len(), 0);
}

#[test]
fn conditions_and_media_attribute() {
    let sh = sheet(&[
        (
            "@media screen and (min-width: 10px) { @supports (fill: red) { rect { fill: red } } g { x: y } } circle {}",
            Some("print"),
        ),
        ("@media print { path {} }", None),
    ]);
    let r = rules(&sh);
    assert_eq!(r[0].0, "rect");
    assert_eq!(r[0].2, ["(fill: red)", "screen and (min-width: 10px)", "print"]);
    assert_eq!(r[1].2, ["screen and (min-width: 10px)", "print"]);
    assert_eq!(r[2].2, ["print"]);
    assert_eq!(r[3].2, ["print"]);
    assert_eq!(
        sh.conds.as_slice().iter().map(|c| c.kind).collect::<Vec<_>>(),
        [COND_MEDIA, COND_MEDIA, COND_SUPPORTS, COND_MEDIA]
    );
}

#[test]
fn layers_rank() {
    // 順位：a.y < a.x（名前の初めての順） < a（層そのもの） < b < 名前のない層 < 層のない規則
    let sh = sheet(&[(
        "@layer a, b; @layer b { rect {} } @layer a { @layer y { g {} } circle {} } @layer a.x { path {} } @layer { line {} } text {}",
        None,
    )]);
    let r = rules(&sh);
    let rank = |ty: &str| r.iter().find(|x| x.0 == ty).unwrap().3;
    assert!(rank("g") < rank("path"));
    assert!(rank("path") < rank("circle"));
    assert!(rank("circle") < rank("rect"));
    assert!(rank("rect") < rank("line"));
    assert_eq!(rank("text"), u32::MAX);
    // 壊れた @layer は塊ごと飛ばす
    let sh = sheet(&[("@layer a b { rect {} } @layer a, { g {} } circle {}", None)]);
    assert_eq!(rules(&sh).iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["circle"]);
}

#[test]
fn namespaces_and_other_at_rules() {
    let sh = sheet(&[
        (
            "@charset \"utf-8\"; @import url(a.css); @namespace svg url(http://www.w3.org/2000/svg); svg|rect {} @namespace x \"urn:x\"; x|g {} @font-face { src: url(f.woff), url(\"g.ttf\") } @keyframes k { from {} } line {}",
            None,
        ),
        // `@namespace` は `<style>` ごと
        ("svg|path {} circle {}", None),
    ]);
    let r = rules(&sh);
    // 規則のあとの @namespace は効かないので x|g は壊れたセレクタ
    assert_eq!(r.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["rect", "line", "circle"]);
    let text = sh.text.as_slice();
    let unread: Vec<String> = sh
        .notes
        .as_slice()
        .iter()
        .filter(|n| n.unread != 0)
        .map(|n| String::from_utf8_lossy(n.href.of(text)).to_string())
        .collect();
    assert_eq!(unread, ["a.css", "f.woff", "g.ttf"]);
    assert!(
        sh.notes
            .as_slice()
            .iter()
            .filter(|n| n.unread == 0)
            .all(|n| matches!(n.kind, WarningKind::AtRule | WarningKind::InvalidSelector))
    );
}

/// どの 3 バイトの並びでも止まり、落ちない。
#[test]
fn all_three_byte_sequences() {
    let alphabet = b"{}()[];:@,a ";
    for &x in alphabet {
        for &y in alphabet {
            for &z in alphabet {
                let css = [x, y, z, b'{', x, y, z];
                let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
                let mut a = SheetBuilder::new(&budget);
                a.add(&css, None, 0).unwrap();
                a.finish().unwrap();
            }
        }
    }
}
