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

/// 持ち続ける量の式（`Sheet::bytes` と同じ。まとめた層の表は層の節の数の容量で取る）。
fn counts_bytes(c: &SheetCounts) -> u64 {
    let m = |n: u64, size: usize| n.saturating_mul(size as u64);
    [
        c.text,
        c.names,
        m(c.rules, size_of::<Rule>()),
        m(c.decls, size_of::<Decl>()),
        m(c.selectors, size_of::<crate::css::select::Selector>()),
        m(c.compounds, size_of::<crate::css::select::Compound>()),
        m(c.simples, size_of::<crate::css::select::Simple>()),
        m(c.conds, size_of::<Cond>()),
        m(c.layers, size_of::<Layer>()),
        m(c.layers, size_of::<u32>()),
        m(c.ns, size_of::<NsDecl>()),
        c.ns_names,
        m(c.notes, size_of::<Note>()),
    ]
    .iter()
    .sum()
}

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
    assert_eq!(sh.bytes(), counts_bytes(&counts));
    assert!(budget2.max_spend() < crate::budget::CANCEL_INTERVAL);
    sh
}

/// 規則ごとの（最初の選択子の右端の型の名前、宣言の名前と値、条件の字句、層の順位）。
type Row = (String, Vec<(String, String, bool)>, Vec<String>, u32);

fn rules(sh: &Sheet) -> Vec<Row> {
    let text = sh.text.as_slice();
    let names = sh.names.as_slice();
    // 条件はどれも真として層の順位を決める
    let budget = Box::leak(Box::new(Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never)));
    let all = vec![1u8; sh.conds.len()];
    let ranks = sh.layer_ranks(&all, budget).unwrap();
    let rank = |r: &Rule| {
        if r.layer == NONE { u32::MAX } else { ranks.as_slice()[sh.layers.as_slice()[r.layer as usize].canon as usize] }
    };
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
            (ty, decls, conds, rank(r))
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
    let text = sh.names.as_slice();
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

/// 層の順は真の条件の所で名前を書いた順（偽の `@media` の中で初めて書いた名前は順に入らない。Chrome と同じ）。
#[test]
fn layer_order_follows_true_conditions() {
    let sh = sheet(&[("@media print { @layer b; } @layer a { rect {} } @layer b { g {} }", None)]);
    let budget = Box::leak(Box::new(Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never)));
    let rank_of = |cond_ok: &[u8], ty: &str| {
        let ranks = sh.layer_ranks(cond_ok, budget).unwrap();
        let r = rules(&sh).iter().position(|x| x.0 == ty).unwrap();
        let rule = sh.rules.as_slice()[r];
        ranks.as_slice()[sh.layers.as_slice()[rule.layer as usize].canon as usize]
    };
    // print が偽：a、b の順
    assert!(rank_of(&[0], "rect") < rank_of(&[0], "g"));
    // print が真：b、a の順
    assert!(rank_of(&[1], "g") < rank_of(&[1], "rect"));
}

/// 層の名前：escape を解いて比べる。空の項目・`.` の前後の空白・全体の言葉は壊れている。
#[test]
fn layer_names() {
    // `\62` は b と同じ層
    let sh = sheet(&[("@layer \\62, a; @layer a { rect {} } @layer b { g {} }", None)]);
    let r = rules(&sh);
    let rank = |ty: &str| r.iter().find(|x| x.0 == ty).unwrap().3;
    assert!(rank("g") < rank("rect"));
    for bad in
        ["@layer b,,a;", "@layer a,;", "@layer ,a;", "@layer a .b;", "@layer a.;", "@layer initial;", "@layer a b;"]
    {
        let sh = sheet(&[(bad, None)]);
        assert_eq!(sh.layers.len(), 0, "{bad}");
        assert!(sh.notes.as_slice().iter().any(|n| n.kind == WarningKind::AtRule), "{bad}");
    }
    // 多くの層も、層の数 × log の手間で決める
    let many: String = format!("@layer {};", (0..30000).map(|i| format!("a{i}")).collect::<Vec<_>>().join(","));
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut b = SheetBuilder::new(&budget);
    b.add(many.as_bytes(), None, 0).unwrap();
    let sh = b.finish().unwrap();
    let all: Vec<u8> = Vec::new();
    sh.layer_ranks(&all, &budget).unwrap();
    assert!(budget.work() < 40_000_000, "{}", budget.work());
    assert!(budget.max_spend() < crate::budget::CANCEL_INTERVAL);
}

/// `@namespace`・`@import` を書ける所（壊れた規則と知らないアットルールは数えない。Chrome と同じ）。
#[test]
fn namespace_and_import_order() {
    const NS: &str = "@namespace s url(http://www.w3.org/2000/svg); s|rect {}";
    for (head, kept) in [
        ("x!{} ", true),
        ("@foo; ", true),
        ("@layer a; ", true),
        ("@import url(x.css); ", true),
        ("@charset \"x\"; ", true),
        ("@media all {} ", false),
        ("@layer a {} ", false),
        ("@font-face {} ", false),
        ("g {} ", false),
    ] {
        let sh = sheet(&[(&format!("{head}{NS}"), None)]);
        let n = rules(&sh).iter().filter(|x| x.0 == "rect").count();
        assert_eq!(n == 1, kept, "{head}");
    }
    // 正しい規則と @namespace のあとの @import は読まない
    let sh =
        sheet(&[("@import url(a.css); @namespace url(urn:x); @import url(b.css); rect {} @import url(c.css);", None)]);
    let names = sh.names.as_slice();
    let unread: Vec<String> = sh
        .notes
        .as_slice()
        .iter()
        .filter(|n| n.unread != 0)
        .map(|n| String::from_utf8_lossy(n.href.of(names)).to_string())
        .collect();
    assert_eq!(unread, ["a.css"]);
}

/// 当てないセレクタと入れ子の規則は警告する。
#[test]
fn unsupported_and_nested_warnings() {
    let sh = sheet(&[("rect:has(a), g {} circle { x { } fill: red }", None)]);
    let kinds: Vec<WarningKind> = sh.notes.as_slice().iter().map(|n| n.kind).collect();
    assert_eq!(kinds, [WarningKind::UnsupportedSelector, WarningKind::NestedRule]);
    // 当てる選択子のない規則も宣言は読む
    let sh = sheet(&[("rect:has(a) { fill: red } g { fill: blue }", None)]);
    assert_eq!(sh.rules.as_slice()[0].selector, NONE);
    assert_eq!(sh.rules.len(), 2);
}
