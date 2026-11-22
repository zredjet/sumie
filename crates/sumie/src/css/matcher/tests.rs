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

struct Collect<'a> {
    doc: &'a XmlDoc,
    input: &'a [u8],
    out: Vec<(String, Vec<u32>)>,
}

impl MatchSink for Collect<'_> {
    fn element(&mut self, node: u32) -> Result<(), Error> {
        let n = self.doc.node(node).unwrap();
        let name = self
            .doc
            .attr(
                self.input,
                n,
                NS_NONE,
                b"id",
                &crate::budget::Budget::new(crate::error::Stage::Parse, u64::MAX, u64::MAX, &crate::cancel::Never),
            )
            .unwrap()
            .map(|v| String::from_utf8_lossy(v).to_string())
            .unwrap_or_else(|| String::from_utf8_lossy(n.local.of(self.input)).to_string());
        self.out.push((name, Vec::new()));
        Ok(())
    }

    fn rule(&mut self, rule: u32, _spec: u32) -> Result<(), Error> {
        self.out.last_mut().unwrap().1.push(rule);
        Ok(())
    }
}

/// 文書 `svg` に CSS `css` を当て、id（なければ局所名）ごとの当たった規則の番号。
fn run(svg: &str, css: &str) -> Vec<(String, Vec<u32>)> {
    let input = svg.as_bytes();
    let b0 = Budget::new(Stage::EstimateParse, u64::MAX, u64::MAX, &Never);
    let counts = crate::xml::count(input, &Limits::DEFAULT, &b0, Stage::EstimateParse).unwrap();
    let b1 = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let doc = crate::xml::build(input, &counts, &Limits::DEFAULT, &b1, Stage::Parse).unwrap();
    let b2 = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut sb = SheetBuilder::new(&b2);
    sb.add(css.as_bytes(), None, 0).unwrap();
    let sheet = sb.finish().unwrap();
    let active = vec![1u8; sheet.rules.len()];
    let b3 = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut c = Collect { doc: &doc, input, out: Vec::new() };
    match_rules(&doc, input, &sheet, &active, &b3, &mut c).unwrap();
    assert!(b3.max_spend() < crate::budget::CANCEL_INTERVAL);
    c.out
}

/// 規則 `r` が当たった要素の名前。
fn hits(out: &[(String, Vec<u32>)], r: u32) -> Vec<String> {
    out.iter().filter(|(_, rs)| rs.contains(&r)).map(|(n, _)| n.clone()).collect()
}

const DOC: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:x="urn:x" xml:lang="en-US">
<g id="g1" class="a b"><rect id="r1"/><x:foo id="f1"/><rect id="r2" data-k="foo-bar"/><circle id="c1" lang="fr"/></g>
<g id="g2"><rect id="r3" class="b"/></g>
<a id="link" href="#x"><desc id="d1"/></a>
</svg>"##;

#[test]
fn combinators() {
    let out = run(DOC, "g rect {} g > rect {} rect + rect {} rect ~ circle {} svg > g rect {} x|foo {} foo + rect {}");
    assert_eq!(hits(&out, 0), ["r1", "r2", "r3"]);
    assert_eq!(hits(&out, 1), ["r1", "r2", "r3"]);
    // 別の名前空間の要素も兄弟に数える：r1 の直後は foo なので r2 は rect + rect に当たらない
    assert_eq!(hits(&out, 2), Vec::<String>::new());
    assert_eq!(hits(&out, 3), ["c1"]);
    assert_eq!(hits(&out, 4), ["r1", "r2", "r3"]);
    // 知らない接頭辞の規則は捨てる（x は @namespace で宣言していない）
    assert_eq!(out.iter().filter(|(n, _)| n == "f1").count(), 1);
    assert_eq!(hits(&out, 5), ["r2"]);
}

#[test]
fn simple_selectors() {
    let out = run(
        DOC,
        ".a {} .b {} #r2 {} [data-k] {} [data-k|=foo] {} [data-k^=fo] {} [data-k$=bar] {} [data-k*=\"o-b\"] {} [data-k=FOO-BAR i] {} [href] {} a:link {} rect.b {}",
    );
    assert_eq!(hits(&out, 0), ["g1"]);
    assert_eq!(hits(&out, 1), ["g1", "r3"]);
    assert_eq!(hits(&out, 2), ["r2"]);
    for r in 3..9 {
        assert_eq!(hits(&out, r), ["r2"], "規則 {r}");
    }
    assert_eq!(hits(&out, 9), ["link"]);
    assert_eq!(hits(&out, 10), ["link"]);
    assert_eq!(hits(&out, 11), ["r3"]);
}

#[test]
fn structural_and_logical() {
    let out = run(
        DOC,
        "rect:first-child {} rect:last-child {} :only-child {} rect:nth-child(3) {} rect:first-of-type {} rect:last-of-type {} :root {} desc:empty {} rect:not(.b) {} :is(circle, #r3) {} g:where(#g2) {} rect:nth-last-child(2) {}",
    );
    assert_eq!(hits(&out, 0), ["r1", "r3"]);
    assert_eq!(hits(&out, 1), ["r3"]);
    // 根の要素も兄弟のない子として数える（Selectors 4・Chrome。照らしの first-child-root）
    assert_eq!(hits(&out, 2), ["svg", "r3", "d1"]);
    assert_eq!(hits(&out, 3), ["r2"]);
    assert_eq!(hits(&out, 4), ["r1", "r3"]);
    assert_eq!(hits(&out, 5), ["r2", "r3"]);
    assert_eq!(hits(&out, 6), ["svg"]);
    assert_eq!(hits(&out, 7), ["d1"]);
    assert_eq!(hits(&out, 8), ["r1", "r2"]);
    assert_eq!(hits(&out, 9), ["c1", "r3"]);
    assert_eq!(hits(&out, 10), ["g2"]);
    assert_eq!(hits(&out, 11), ["r2"]);
}

#[test]
fn language() {
    let out = run(DOC, ":lang(en) {} :lang(fr) {} :lang(en-us) {} :lang(de, fr) {}");
    // xml:lang は受け継ぐ。c1 は lang="fr"
    assert!(hits(&out, 0).contains(&"r1".to_string()));
    assert!(!hits(&out, 0).contains(&"c1".to_string()));
    assert_eq!(hits(&out, 1), ["c1"]);
    assert!(hits(&out, 2).contains(&"g2".to_string()));
    assert_eq!(hits(&out, 3), ["c1"]);
}

#[test]
fn nested_lists_inside_compounds() {
    // :is の中の複雑なセレクタ（子孫）も同じ要素の旗として決まる
    let out = run(DOC, "rect:is(g.a rect) {} :not(g rect, svg, g) {} rect:is(:not(.b), #r3) {}");
    assert_eq!(hits(&out, 0), ["r1", "r2"]);
    assert_eq!(hits(&out, 1), ["f1", "c1", "link", "d1"]);
    assert_eq!(hits(&out, 2), ["r1", "r2", "r3"]);
}

#[test]
fn many_rules_and_elements_are_bounded() {
    // 規則 2,000 × 要素 2,000：手間は（複合の数 ＋ 単純な選択子）× 要素の数に比例し、後戻りしない
    let css: String = (0..2000).map(|i| format!(".c{i} {{}}")).collect();
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\">{}</svg>",
        (0..2000).map(|i| format!("<rect class=\"c{}\"/>", i % 10)).collect::<String>()
    );
    let out = run(&svg, &css);
    assert_eq!(hits(&out, 3).len(), 200);
    // 深い入れ子と長い子孫の鎖（組み合わせで増えない）
    let deep = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\">{}<rect id=\"z\"/>{}</svg>",
        "<g>".repeat(60),
        "</g>".repeat(60)
    );
    let out = run(&deep, "y g g g g g g rect {} g g g g g g rect {}");
    assert_eq!(hits(&out, 0), Vec::<String>::new());
    assert_eq!(hits(&out, 1), ["z"]);
}

/// 既定の名前空間は型のない複合にも効き、`:is()` などの中の右端の複合には効かない（Chrome と同じ）。
#[test]
fn default_namespace_on_compounds() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:x="urn:x"><x:g class="a"><rect id="r1"/></x:g><g class="a"><rect id="r2"/></g></svg>"##;
    let css = "@namespace url(http://www.w3.org/2000/svg); .a > rect {} #r1 {} *|*:is(#r1) {} *|*:not(#r1) {}";
    let out = run(svg, css);
    assert_eq!(hits(&out, 0), ["r2"]);
    assert_eq!(hits(&out, 1), ["r1"]);
    assert_eq!(hits(&out, 2), ["r1"]);
    assert!(!hits(&out, 3).contains(&"r1".to_string()));
    let other = "@namespace url(urn:x); #r1 {} *|*:is(#r1) {}";
    let out = run(svg, other);
    assert!(hits(&out, 0).is_empty());
    assert_eq!(hits(&out, 1), ["r1"]);
}

/// `:empty`：空の CDATA は子に数えない。空白の文字は数える（Chrome と同じ）。`:dir(ltr)` はすべてに当たる。
#[test]
fn empty_and_dir() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg"><g id="a"><![CDATA[]]></g><g id="b"> </g><g id="c"><![CDATA[ ]]></g><g id="d"/></svg>"##;
    let out = run(svg, "g:empty {} g:dir(ltr) {} g:dir(rtl) {}");
    assert_eq!(hits(&out, 0), ["a", "d"]);
    assert_eq!(hits(&out, 1), ["a", "b", "c", "d"]);
    assert!(hits(&out, 2).is_empty());
}

/// 複合ごと・属性ごとに払う：当てない複合の多い規則でも、手間は複合の数 × 要素の数より小さくならない。
#[test]
fn work_covers_compounds_and_attributes() {
    let n = 300;
    let css: String = format!("{} {{}}", vec![":hover"; n].join(" "));
    let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{}</svg>", "<g/>".repeat(n));
    let input = svg.as_bytes();
    let b0 = Budget::new(Stage::EstimateParse, u64::MAX, u64::MAX, &Never);
    let counts = crate::xml::count(input, &Limits::DEFAULT, &b0, Stage::EstimateParse).unwrap();
    let b1 = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let doc = crate::xml::build(input, &counts, &Limits::DEFAULT, &b1, Stage::Parse).unwrap();
    let b2 = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut sb = SheetBuilder::new(&b2);
    sb.add(css.as_bytes(), None, 0).unwrap();
    let sheet = sb.finish().unwrap();
    let active = vec![1u8; sheet.rules.len()];
    let b3 = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut c = Collect { doc: &doc, input, out: Vec::new() };
    match_rules(&doc, input, &sheet, &active, &b3, &mut c).unwrap();
    let elements = (n + 1) as u64;
    assert!(b3.work() >= elements * n as u64 * cost::OP_STEP, "{}", b3.work());
    // 属性の多い要素の :lang は、属性を 1 度だけ走査する
    let attrs: String = (0..2000).map(|i| format!(" a{i}=\"\"")).collect();
    let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\"{attrs} lang=\"en\">{}</svg>", "<g/>".repeat(100));
    let css: String = (0..50).map(|_| ":lang(en) {}").collect();
    let out = run(&svg, &css);
    assert_eq!(hits(&out, 49).len(), 101);
}
