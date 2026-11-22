//! `probe`：M1 で範囲を決めるための照らし（roadmap M1 の依頼文：CSS のセレクタと SVG 2 のプレゼンテーション属性の最終の
//! 範囲は、resvg と照らした結果を見せてから決める）。
//!
//! `probe [--out tests/svg/m1-probe]`：試しの SVG を作って書き、resvg（Kura の設定）と Chrome で描いて、決めた点の色が
//! 「効いたときの色」と「効かないときの色」のどちらに近いかを表にする。大きさの試しは resvg の大きさ（か誤り）を並べる。
//! 結果は `<out>/README.md`（表）と `local/probe/result.jsonl` に書く。試しの SVG はこのリポジトリのために書いたもの。

use std::fmt::Write as _;
use std::path::PathBuf;

use sumie_testkit::jsonl::Line;

use crate::corpus::{arg, check_args, workspace_root};
use crate::oracle::{Chrome, Fonts, Resvg};

/// 色の試し。キャンバスは 100 × 100、点はふつう真ん中。
struct Probe {
    group: &'static str,
    name: &'static str,
    /// 何を見るか（表の説明）
    what: &'static str,
    body: String,
    yes: [u8; 3],
    no: [u8; 3],
    point: (u32, u32),
}

const GREEN: [u8; 3] = [0, 128, 0];
const RED: [u8; 3] = [255, 0, 0];
const BLACK: [u8; 3] = [0, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];

fn svg(body: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"100\" height=\"100\">\n{body}\n</svg>\n"
    )
}

/// セレクタの試し：`<style>` の規則が当たれば緑、当たらなければプレゼンテーション属性の赤。
fn sel(name: &'static str, what: &'static str, css: &str, content: &str) -> Probe {
    Probe {
        group: "セレクタ",
        name,
        what,
        body: svg(&format!("<style>{css}</style>\n{content}")),
        yes: GREEN,
        no: RED,
        point: (50, 50),
    }
}

/// 当てる要素（id r・class "a b"・data-x）を g の中に置いた中身。`before` は r の前の兄弟、`after` は後ろの兄弟。
fn target(before: &str, after: &str) -> String {
    format!(
        "<g id=\"g1\" class=\"grp\">{before}<rect id=\"r\" class=\"a b\" data-x=\"foo-bar\" lang=\"en\" width=\"100\" height=\"100\" fill=\"red\"/>{after}</g>"
    )
}

fn probes() -> Vec<Probe> {
    let t = target("", "");
    let mut v = vec![
        sel("type", "型", "rect { fill: green }", &t),
        sel("type-upper", "型の大文字（XML では区別する。当たらないのが正しい）", "RECT { fill: green }", &t),
        sel("universal", "`*`", "* { fill: green }", &t),
        sel("id", "`#id`", "#r { fill: green }", &t),
        sel("class", "`.class`", ".a { fill: green }", &t),
        sel("class-two", "`.a.b`", ".a.b { fill: green }", &t),
        sel("attr-exists", "`[a]`", "[data-x] { fill: green }", &t),
        sel("attr-eq", "`[a=v]`", "[data-x=\"foo-bar\"] { fill: green }", &t),
        sel("attr-word", "`[a~=v]`", "[class~=\"b\"] { fill: green }", &t),
        sel("attr-dash", "`[a|=v]`", "[data-x|=\"foo\"] { fill: green }", &t),
        sel("attr-prefix", "`[a^=v]`", "[data-x^=\"fo\"] { fill: green }", &t),
        sel("attr-suffix", "`[a$=v]`", "[data-x$=\"ar\"] { fill: green }", &t),
        sel("attr-substr", "`[a*=v]`", "[data-x*=\"o-b\"] { fill: green }", &t),
        sel("attr-case-i", "`[a=v i]`（大文字小文字を無視する印）", "[data-x=\"FOO-BAR\" i] { fill: green }", &t),
        sel("first-child", "`:first-child`", "rect:first-child { fill: green }", &t),
        sel("last-child", "`:last-child`", "rect:last-child { fill: green }", &target("<desc/>", "")),
        sel("nth-child", "`:nth-child(2)`", "rect:nth-child(2) { fill: green }", &target("<desc/>", "")),
        sel("nth-child-an-b", "`:nth-child(2n+1)`", "rect:nth-child(2n+1) { fill: green }", &t),
        sel("nth-child-odd", "`:nth-child(odd)`", "rect:nth-child(odd) { fill: green }", &t),
        sel("nth-last-child", "`:nth-last-child(1)`", "rect:nth-last-child(1) { fill: green }", &target("<desc/>", "")),
        sel("only-child", "`:only-child`", "rect:only-child { fill: green }", &t),
        sel("first-of-type", "`:first-of-type`", "rect:first-of-type { fill: green }", &target("<desc/>", "")),
        sel("last-of-type", "`:last-of-type`", "rect:last-of-type { fill: green }", &target("", "<desc/>")),
        sel("nth-of-type", "`:nth-of-type(1)`", "rect:nth-of-type(1) { fill: green }", &target("<desc/>", "")),
        sel("only-of-type", "`:only-of-type`", "rect:only-of-type { fill: green }", &target("<desc/>", "")),
        sel("not", "`:not(.c)`", "rect:not(.c) { fill: green }", &t),
        sel("is", "`:is(.a, .z)`", "rect:is(.a, .z) { fill: green }", &t),
        sel("where", "`:where(.a)`", "rect:where(.a) { fill: green }", &t),
        sel("has", "`:has(> rect)`", "g:has(> rect) > rect { fill: green }", &t),
        sel("root", "`:root`", ":root rect { fill: green }", &t),
        sel("empty", "`:empty`", "rect:empty { fill: green }", &t),
        sel("lang", "`:lang(en)`", "rect:lang(en) { fill: green }", &t),
        sel("hover", "`:hover`（動く擬似クラス。当たらないのが正しい）", "rect:hover { fill: green }", &t),
        sel("pseudo-element", "`::before`（擬似要素。当たらないのが正しい）", "rect::before { fill: green }", &t),
        sel("descendant", "子孫 `g rect`", "g rect { fill: green }", &t),
        sel("child", "子 `g > rect`", "g > rect { fill: green }", &t),
        sel("adjacent", "直後の兄弟 `desc + rect`", "desc + rect { fill: green }", &target("<desc/>", "")),
        sel("sibling", "後の兄弟 `desc ~ rect`", "desc ~ rect { fill: green }", &target("<desc/><title/>", "")),
        sel("list", "並び `circle, rect`", "circle, rect { fill: green }", &t),
        sel(
            "list-invalid",
            "並びの 1 つが壊れている（規則ごと捨てるのが正しい）",
            "rect, :bogus(1) { fill: green }",
            &t,
        ),
        sel(
            "namespace",
            "名前空間 `svg|rect`（`@namespace`）",
            "@namespace svg url(http://www.w3.org/2000/svg); svg|rect { fill: green }",
            &t,
        ),
        sel("namespace-any", "`*|rect`", "*|rect { fill: green }", &t),
        sel("specificity", "詳細度（`#r` が後の `.a` に勝つ）", "#r { fill: green } .a { fill: red }", &t),
        sel("order", "同じ詳細度は後が勝つ", ".a { fill: red } .b { fill: green }", &t),
        sel("deep-descendant", "長い子孫の鎖", "svg g rect { fill: green }", &t),
    ];
    // カスケードの順（style 属性・!important）
    v.push(Probe {
        group: "カスケード",
        name: "style-attr-over-rule",
        what: "`style` 属性が規則に勝つ",
        body: svg("<style>rect { fill: red }</style><rect width=\"100\" height=\"100\" style=\"fill: green\"/>"),
        yes: GREEN,
        no: RED,
        point: (50, 50),
    });
    v.push(Probe {
        group: "カスケード",
        name: "important-over-style-attr",
        what: "規則の `!important` が `style` 属性に勝つ",
        body: svg(
            "<style>rect { fill: green !important }</style><rect width=\"100\" height=\"100\" style=\"fill: red\"/>",
        ),
        yes: GREEN,
        no: RED,
        point: (50, 50),
    });
    v.push(Probe {
        group: "カスケード",
        name: "style-attr-important",
        what: "`style` 属性の `!important` が規則の `!important` に勝つ",
        body: svg(
            "<style>rect { fill: red !important }</style><rect width=\"100\" height=\"100\" style=\"fill: green !important\"/>",
        ),
        yes: GREEN,
        no: RED,
        point: (50, 50),
    });
    v.push(Probe {
        group: "カスケード",
        name: "style-type-other",
        what: "`<style type=\"text/foo\">` は読まない（当たらないのが正しい）",
        body: svg(
            "<style type=\"text/foo\">rect { fill: green }</style><rect width=\"100\" height=\"100\" fill=\"red\"/>",
        ),
        yes: GREEN,
        no: RED,
        point: (50, 50),
    });
    v.push(Probe {
        group: "カスケード",
        name: "style-cdata",
        what: "`<style>` の CDATA",
        body: svg("<style><![CDATA[ rect { fill: green } ]]></style><rect width=\"100\" height=\"100\" fill=\"red\"/>"),
        yes: GREEN,
        no: RED,
        point: (50, 50),
    });
    v.push(Probe {
        group: "カスケード",
        name: "style-after-element",
        what: "要素のあとに書いた `<style>` も当たる",
        body: svg("<rect width=\"100\" height=\"100\" fill=\"red\"/><style>rect { fill: green }</style>"),
        yes: GREEN,
        no: RED,
        point: (50, 50),
    });
    // アットルール
    for (name, what, css) in [
        ("media-screen", "`@media screen`", "@media screen { rect { fill: green } }"),
        ("media-all", "`@media all`", "@media all { rect { fill: green } }"),
        ("media-print", "`@media print`（当たらないのが正しい）", "@media print { rect { fill: green } }"),
        ("media-min-width", "`@media (min-width: 1px)`", "@media (min-width: 1px) { rect { fill: green } }"),
        ("supports", "`@supports (fill: green)`", "@supports (fill: green) { rect { fill: green } }"),
        ("import-then-rule", "`@import` のあとの規則", "@import url(x.css); rect { fill: green }"),
        (
            "font-face-then-rule",
            "`@font-face` のあとの規則",
            "@font-face { font-family: x; src: url(x.ttf) } rect { fill: green }",
        ),
        ("charset-then-rule", "`@charset` のあとの規則", "@charset \"utf-8\"; rect { fill: green }"),
        ("layer", "`@layer`", "@layer a { rect { fill: green } }"),
        ("unknown-at-then-rule", "知らないアットルールのあとの規則", "@foo bar { x: y } rect { fill: green }"),
    ] {
        v.push(Probe {
            group: "アットルール",
            name,
            what,
            body: svg(&format!("<style>{css}</style><rect width=\"100\" height=\"100\" fill=\"red\"/>")),
            yes: GREEN,
            no: RED,
            point: (50, 50),
        });
    }
    // SVG 2 のプレゼンテーション属性と、CSS だけのプロパティ
    let bg_red = "<rect width=\"100\" height=\"100\" fill=\"red\"/>";
    let p = |name: &'static str, what: &'static str, body: String, yes: [u8; 3], no: [u8; 3]| Probe {
        group: "SVG 2 の属性・プロパティ",
        name,
        what,
        body: svg(&body),
        yes,
        no,
        point: (50, 50),
    };
    v.push(p(
        "transform-attr",
        "`transform` 属性（SVG 1.1。比べの基準）",
        format!(
            "{bg_red}<rect x=\"-100\" width=\"100\" height=\"100\" fill=\"green\" transform=\"translate(100 0)\"/>"
        ),
        GREEN,
        RED,
    ));
    v.push(p(
        "transform-css-rule",
        "CSS の `transform` プロパティ（`<style>`）",
        format!(
            "<style>.m {{ transform: translate(100px, 0px) }}</style>{bg_red}<rect class=\"m\" x=\"-100\" width=\"100\" height=\"100\" fill=\"green\"/>"
        ),
        GREEN,
        RED,
    ));
    v.push(p(
        "transform-css-style-attr",
        "CSS の `transform` プロパティ（`style` 属性）",
        format!(
            "{bg_red}<rect style=\"transform: translate(100px, 0px)\" x=\"-100\" width=\"100\" height=\"100\" fill=\"green\"/>"
        ),
        GREEN,
        RED,
    ));
    v.push(p(
        "transform-origin",
        "`transform-origin` 属性",
        format!(
            "{bg_red}<rect width=\"100\" height=\"100\" fill=\"green\" transform=\"rotate(180)\" transform-origin=\"50 50\"/>"
        ),
        GREEN,
        RED,
    ));
    v.push(p(
        "paint-order",
        "`paint-order` 属性",
        "<rect width=\"100\" height=\"100\" fill=\"green\" stroke=\"red\" stroke-width=\"300\" paint-order=\"stroke\"/>".into(),
        GREEN,
        RED,
    ));
    v.push(p(
        "mix-blend-mode",
        "`mix-blend-mode` 属性（multiply で白を重ねると下の緑のまま）",
        "<rect width=\"100\" height=\"100\" fill=\"green\"/><rect width=\"100\" height=\"100\" fill=\"white\" mix-blend-mode=\"multiply\"/>".into(),
        GREEN,
        WHITE,
    ));
    v.push(p(
        "mix-blend-mode-css",
        "CSS の `mix-blend-mode`（`style` 属性）",
        "<rect width=\"100\" height=\"100\" fill=\"green\"/><rect width=\"100\" height=\"100\" fill=\"white\" style=\"mix-blend-mode: multiply\"/>".into(),
        GREEN,
        WHITE,
    ));
    v.push(p(
        "isolation",
        "`isolation` 属性（切り離すと multiply の下は透明で、白のまま）",
        "<rect width=\"100\" height=\"100\" fill=\"green\"/><g isolation=\"isolate\"><rect width=\"100\" height=\"100\" fill=\"white\" mix-blend-mode=\"multiply\"/></g>".into(),
        WHITE,
        GREEN,
    ));
    v.push(p(
        "mask-type",
        "`mask-type=\"alpha\"` 属性（黒い不透明の中身で、α なら見える）",
        format!(
            "<defs><mask id=\"k\" mask-type=\"alpha\"><rect width=\"100\" height=\"100\" fill=\"black\"/></mask></defs>{bg_red}<rect width=\"100\" height=\"100\" fill=\"green\" mask=\"url(#k)\"/>"
        ),
        GREEN,
        RED,
    ));
    v.push(p(
        "vector-effect",
        "`vector-effect=\"non-scaling-stroke\"` 属性",
        format!(
            "{bg_red}<g transform=\"scale(1 0.01)\"><path d=\"M0 5000 H100\" stroke=\"green\" stroke-width=\"100\" vector-effect=\"non-scaling-stroke\"/></g>"
        ),
        GREEN,
        RED,
    ));
    v.push(p(
        "geometry-css-width",
        "CSS の `width`・`height`（rect の幾何のプロパティ）",
        format!("<style>.s {{ width: 100px; height: 100px }}</style>{bg_red}<rect class=\"s\" fill=\"green\"/>"),
        GREEN,
        RED,
    ));
    v.push(p(
        "geometry-css-r",
        "CSS の `r`（circle）",
        format!("<style>.c {{ r: 80px }}</style>{bg_red}<circle class=\"c\" cx=\"50\" cy=\"50\" fill=\"green\"/>"),
        GREEN,
        RED,
    ));
    v.push(p(
        "geometry-css-d",
        "CSS の `d`（path）",
        format!("<style>.d {{ d: path(\"M0 0H100V100H0Z\") }}</style>{bg_red}<path class=\"d\" fill=\"green\"/>"),
        GREEN,
        RED,
    ));
    v.push(p(
        "marker-shorthand",
        "`marker` のまとめのプロパティ（`style` 属性）",
        format!(
            "<defs><marker id=\"m\" markerWidth=\"100\" markerHeight=\"100\" refX=\"50\" refY=\"50\" markerUnits=\"userSpaceOnUse\"><rect width=\"100\" height=\"100\" fill=\"green\"/></marker></defs>{bg_red}<path d=\"M50 50 L50.1 50\" stroke=\"none\" fill=\"none\" style=\"marker: url(#m)\"/>"
        ),
        GREEN,
        RED,
    ));
    v.push(p(
        "marker-attr",
        "`marker-start` 属性（SVG 1.1。比べの基準）",
        format!(
            "<defs><marker id=\"m\" markerWidth=\"100\" markerHeight=\"100\" refX=\"50\" refY=\"50\" markerUnits=\"userSpaceOnUse\"><rect width=\"100\" height=\"100\" fill=\"green\"/></marker></defs>{bg_red}<path d=\"M50 50 L50.1 50\" stroke=\"none\" fill=\"none\" marker-start=\"url(#m)\"/>"
        ),
        GREEN,
        RED,
    ));
    // 値の書き方（壊れた値は無視され、塗りの既定の黒になる）
    for (name, what, val) in [
        ("color-hex3", "`#rgb`", "#080"),
        ("color-hex6", "`#rrggbb`", "#008000"),
        ("color-hex4", "`#rgba`", "#080f"),
        ("color-hex8", "`#rrggbbaa`", "#008000ff"),
        ("color-hex-upper", "大文字の `#0A0`", "#0A0"),
        ("color-rgb", "`rgb(0, 128, 0)`", "rgb(0, 128, 0)"),
        ("color-rgb-percent", "`rgb(0%, 50%, 0%)`", "rgb(0%, 50%, 0%)"),
        ("color-rgb-space", "`rgb(0 128 0)`（CSS Color 4）", "rgb(0 128 0)"),
        ("color-rgb-slash", "`rgb(0 128 0 / 1)`", "rgb(0 128 0 / 1)"),
        ("color-rgba", "`rgba(0, 128, 0, 1)`", "rgba(0, 128, 0, 1)"),
        ("color-hsl", "`hsl(120, 100%, 25%)`", "hsl(120, 100%, 25%)"),
        ("color-hsla", "`hsla(120, 100%, 25%, 1)`", "hsla(120, 100%, 25%, 1)"),
        ("color-hwb", "`hwb(120 0% 50%)`", "hwb(120 0% 50%)"),
        ("color-named", "名前 `green`", "green"),
        ("color-named-upper", "大文字の名前 `GREEN`", "GREEN"),
        (
            "color-named-css4",
            "CSS 4 の名前 `rebeccapurple` の代わりに `darkgreen` ではなく緑の `green`（比べの基準）",
            "green",
        ),
        ("color-system", "システムの色 `ButtonText`", "ButtonText"),
    ] {
        v.push(Probe {
            group: "値の書き方",
            name,
            what,
            body: svg(&format!("<rect width=\"100\" height=\"100\" fill=\"{val}\"/>")),
            yes: GREEN,
            no: BLACK,
            point: (50, 50),
        });
    }
    v.retain(|p| p.name != "color-named-css4");
    for (name, what, w) in [
        ("unit-mm", "長さの `mm`", "26.458333mm"),
        ("unit-in", "長さの `in`", "1.0416667in"),
        ("unit-pt", "長さの `pt`", "75pt"),
        ("unit-pc", "長さの `pc`", "6.25pc"),
        ("unit-em", "長さの `em`（font-size 50）", "2em"),
        ("unit-ex", "長さの `ex`（font-size 50。usvg は em の半分）", "4ex"),
        ("unit-percent", "長さの `%`（表示の領域の幅）", "100%"),
        ("unit-vw", "長さの `vw`", "100vw"),
        ("unit-px-upper", "大文字の単位 `100PX`", "100PX"),
    ] {
        v.push(Probe {
            group: "値の書き方",
            name,
            what,
            body: svg(&format!(
                "<rect width=\"100\" height=\"100\" fill=\"red\"/><rect font-size=\"50\" width=\"{w}\" height=\"100\" fill=\"green\"/>"
            )),
            yes: GREEN,
            no: RED,
            point: (95, 50),
        });
    }
    // 文書の項目
    let d = |name: &'static str, what: &'static str, body: String, yes: [u8; 3], no: [u8; 3]| Probe {
        group: "文書",
        name,
        what,
        body,
        yes,
        no,
        point: (50, 50),
    };
    v.push(d(
        "duplicate-id",
        "同じ id の 2 つ（緑＝最初のもの・赤＝後のもの）",
        svg("<defs><linearGradient id=\"g\"><stop stop-color=\"green\"/></linearGradient><linearGradient id=\"g\"><stop stop-color=\"red\"/></linearGradient></defs><rect width=\"100\" height=\"100\" fill=\"url(#g)\"/>"),
        GREEN,
        RED,
    ));
    v.push(d(
        "required-features-bogus",
        "`requiredFeatures` の知らない機能（赤＝照らして外す・緑＝無視する）",
        svg("<switch><rect requiredFeatures=\"http://example.com/bogus\" width=\"100\" height=\"100\" fill=\"green\"/><rect width=\"100\" height=\"100\" fill=\"red\"/></switch>"),
        RED,
        GREEN,
    ));
    v.push(d(
        "required-features-known",
        "`requiredFeatures` の SVG 1.1 の機能（`#Shape`。緑＝通る）",
        svg("<switch><rect requiredFeatures=\"http://www.w3.org/TR/SVG11/feature#Shape\" width=\"100\" height=\"100\" fill=\"green\"/><rect width=\"100\" height=\"100\" fill=\"red\"/></switch>"),
        GREEN,
        RED,
    ));
    v.push(d(
        "required-features-empty",
        "空の `requiredFeatures`（赤＝偽とみなす・緑＝無視する）",
        svg("<switch><rect requiredFeatures=\"\" width=\"100\" height=\"100\" fill=\"green\"/><rect width=\"100\" height=\"100\" fill=\"red\"/></switch>"),
        RED,
        GREEN,
    ));
    v.push(d(
        "required-extensions",
        "`requiredExtensions` の知らない拡張（赤＝外す）",
        svg("<switch><rect requiredExtensions=\"http://example.com/x\" width=\"100\" height=\"100\" fill=\"green\"/><rect width=\"100\" height=\"100\" fill=\"red\"/></switch>"),
        RED,
        GREEN,
    ));
    v.push(d(
        "system-language",
        "`systemLanguage=\"ja\"`（言語の並びは en。緑＝照らして外す）",
        svg("<switch><rect systemLanguage=\"ja\" width=\"100\" height=\"100\" fill=\"red\"/><rect width=\"100\" height=\"100\" fill=\"green\"/></switch>"),
        GREEN,
        RED,
    ));
    v.push(d(
        "system-language-region",
        "`systemLanguage=\"en-US\"`（言語の並びは en。緑＝通る）",
        svg("<switch><rect systemLanguage=\"en-US\" width=\"100\" height=\"100\" fill=\"green\"/><rect width=\"100\" height=\"100\" fill=\"red\"/></switch>"),
        GREEN,
        RED,
    ));
    v.push(d(
        "use-self-cycle",
        "自分を指す use（緑＝ほかは描く）",
        svg("<rect width=\"100\" height=\"100\" fill=\"green\"/><use id=\"u\" xlink:href=\"#u\"/>"),
        GREEN,
        RED,
    ));
    v.push(d(
        "pattern-cycle",
        "中身が自分を塗る pattern（緑＝輪の外は描く）",
        svg("<defs><pattern id=\"p\" width=\"10\" height=\"10\" patternUnits=\"userSpaceOnUse\"><rect width=\"10\" height=\"10\" fill=\"url(#p)\"/></pattern></defs><rect width=\"100\" height=\"100\" fill=\"green\"/><rect width=\"50\" height=\"50\" fill=\"url(#p)\"/>"),
        GREEN,
        RED,
    ));
    v.push(d(
        "attlist-default",
        "ATTLIST の既定の属性値（緑＝当てる・黒＝当てない）",
        "<!DOCTYPE svg [<!ATTLIST rect fill CDATA \"green\">]>\n".to_string()
            + &svg("<rect width=\"100\" height=\"100\"/>"),
        GREEN,
        BLACK,
    ));
    v.push(d(
        "entity",
        "内部の一般の実体（比べの基準）",
        "<!DOCTYPE svg [<!ENTITY c \"green\">]>\n".to_string()
            + &svg("<rect width=\"100\" height=\"100\" fill=\"&c;\"/>"),
        GREEN,
        BLACK,
    ));
    v.push(d(
        "xml-space-preserve-attr",
        "属性の値の改行（属性の値の正規化。比べの基準）",
        svg("<rect width=\"100\"\nheight=\"100\" fill=\"\n green\n\"/>"),
        GREEN,
        BLACK,
    ));
    v.extend(css_details());
    v
}

/// CSS の細部（css の単位で決める字句・名前・照合の細かい扱い）。効けば緑、効かなければ赤。
fn css_details() -> Vec<Probe> {
    let c = |name: &'static str, what: &'static str, body: &str| Probe {
        group: "CSS の細部",
        name,
        what,
        body: svg(body),
        yes: GREEN,
        no: RED,
        point: (50, 50),
    };
    let r = "<rect id=\"r\" class=\"a\" width=\"100\" height=\"100\" fill=\"red\"/>";
    let st = |css: &str, content: &str| format!("<style>{css}</style>{content}");
    vec![
        c("prop-upper", "プロパティの名前の大文字 `FILL`（CSS は区別しない）", &st("rect { FILL: green }", r)),
        c(
            "prop-upper-attr",
            "style 属性のプロパティの名前の大文字",
            "<rect width=\"100\" height=\"100\" fill=\"red\" style=\"FILL: green\"/>",
        ),
        c("value-upper", "色の名前の大文字 `GREEN`", &st("rect { fill: GREEN }", r)),
        c("escape-class", "クラスの名前の escape `.\\61`", &st(".\\61 { fill: green }", r)),
        c("escape-type", "型の名前の escape `r\\65 ct`", &st("r\\65 ct { fill: green }", r)),
        c("escape-hex-space", "escape のあとの空白 `#\\72 `", &st("#\\72  { fill: green }", r)),
        c("id-upper", "`#R`（id は区別する。当たらないのが正しい）", &st("#R { fill: green }", r)),
        c("class-upper", "`.A`（class は区別する。当たらないのが正しい）", &st(".A { fill: green }", r)),
        c(
            "attr-name-upper",
            "`[DATA-X]`（XML の属性の名前は区別する。当たらないのが正しい）",
            &st("[DATA-X] { fill: green }", "<rect data-x=\"1\" width=\"100\" height=\"100\" fill=\"red\"/>"),
        ),
        c(
            "attr-unquoted",
            "`[data-x=foo]`（引用符なしの値）",
            &st("[data-x=foo] { fill: green }", "<rect data-x=\"foo\" width=\"100\" height=\"100\" fill=\"red\"/>"),
        ),
        c(
            "attr-xlink-href",
            "`[href]` が `xlink:href` に当たるか",
            &st("[href] { fill: green }", "<rect xlink:href=\"#x\" width=\"100\" height=\"100\" fill=\"red\"/>"),
        ),
        c(
            "first-child-foreign",
            "`:first-child` の前に別の名前空間の要素",
            &st(
                "rect:first-child { fill: green }",
                "<g><foo:x xmlns:foo=\"urn:x\"/><rect width=\"100\" height=\"100\" fill=\"red\"/></g>",
            ),
        ),
        c(
            "adjacent-foreign",
            "`desc + rect` の間に別の名前空間の要素",
            &st(
                "desc + rect { fill: green }",
                "<g><desc/><foo:x xmlns:foo=\"urn:x\"/><rect width=\"100\" height=\"100\" fill=\"red\"/></g>",
            ),
        ),
        c(
            "child-foreign",
            "`g > rect` の間に別の名前空間の要素（子でないので当たらないのが CSS）",
            &st(
                "g > rect { fill: green }",
                "<g><foo:x xmlns:foo=\"urn:x\"><rect width=\"100\" height=\"100\" fill=\"red\"/></foo:x></g>",
            ),
        ),
        c(
            "first-child-text",
            "`:first-child` の前に文字（文字は数えない）",
            &st("rect:first-child { fill: green }", "<g>text<rect width=\"100\" height=\"100\" fill=\"red\"/></g>"),
        ),
        c(
            "first-child-root",
            "根の要素の `:first-child`",
            "<style>svg:first-child rect { fill: green }</style><rect width=\"100\" height=\"100\" fill=\"red\"/>",
        ),
        c(
            "type-foreign",
            "別の名前空間の要素の型 `x rect`（`foo:x` に当たるか）",
            &st(
                "x rect { fill: green }",
                "<foo:x xmlns:foo=\"urn:x\"><rect width=\"100\" height=\"100\" fill=\"red\"/></foo:x>",
            ),
        ),
        c(
            "important-space",
            "`! important`（間に空白）",
            &st("rect { fill: green ! important }", "<rect width=\"100\" height=\"100\" style=\"fill: red\"/>"),
        ),
        c(
            "important-upper",
            "`!IMPORTANT`",
            &st("rect { fill: green !IMPORTANT }", "<rect width=\"100\" height=\"100\" style=\"fill: red\"/>"),
        ),
        c("decl-later-wins", "同じ規則の後の宣言が勝つ", &st("rect { fill: red; fill: green }", r)),
        c(
            "decl-invalid-later",
            "後の宣言が壊れた値（前の宣言が残るのが CSS）",
            &st("rect { fill: green; fill: nonsense }", r),
        ),
        c(
            "attr-invalid-value",
            "プレゼンテーション属性の壊れた値（受け継いだ値が残るか）",
            "<g fill=\"green\"><rect width=\"100\" height=\"100\" fill=\"nonsense\"/></g>",
        ),
        c(
            "cdo-cdc",
            "`<!--`・`-->` を CSS の中に書く",
            "<style>&lt;!-- rect { fill: green } --&gt;</style><rect width=\"100\" height=\"100\" fill=\"red\"/>",
        ),
        c("comment-in-selector", "セレクタの中の注釈 `rect/**/.a`", &st("rect/**/.a { fill: green }", r)),
        c("combinator-tight", "空白のない結合子 `g>rect`", &st("g>rect { fill: green }", &format!("<g>{r}</g>"))),
        c("unclosed-block", "閉じていない規則（文書の終わりで閉じる）", &st("rect { fill: green", r)),
        c("string-in-value", "宣言の値の中の `;` を含む文字列", &st("rect { font-family: \"a;b\"; fill: green }", r)),
        c(
            "bad-selector-block",
            "読めないセレクタの規則のあとの規則",
            &st("rect[ { fill: red } rect { fill: green }", r),
        ),
        c(
            "style-two",
            "2 つの `<style>`（後が勝つ）",
            "<style>rect { fill: red }</style><style>rect { fill: green }</style><rect width=\"100\" height=\"100\"/>",
        ),
        c(
            "style-media-attr",
            "`<style media=\"print\">`（media 属性を見るか。見なければ当たる）",
            "<style media=\"print\">rect { fill: green }</style><rect width=\"100\" height=\"100\" fill=\"red\"/>",
        ),
        c(
            "style-cdata-split",
            "文字と CDATA に分かれた `<style>`",
            "<style>rect { fill:<![CDATA[ green }]]></style><rect width=\"100\" height=\"100\" fill=\"red\"/>",
        ),
        c(
            "style-in-foreign",
            "別の名前空間の要素の中の `<style>`",
            "<foo:x xmlns:foo=\"urn:x\"><style>rect { fill: green }</style></foo:x><rect width=\"100\" height=\"100\" fill=\"red\"/>",
        ),
        c(
            "style-foreign-ns",
            "別の名前空間の `style` 要素（当たらないのが正しい）",
            "<foo:style xmlns:foo=\"urn:x\">rect { fill: green }</foo:style><rect width=\"100\" height=\"100\" fill=\"red\"/>",
        ),
    ]
}

/// 大きさの試し（resvg の大きさを並べる）。
fn size_probes() -> Vec<(&'static str, &'static str, String)> {
    let ns = "xmlns=\"http://www.w3.org/2000/svg\"";
    let shape =
        "<rect x=\"10\" y=\"10\" width=\"50\" height=\"40\" fill=\"green\" stroke=\"black\" stroke-width=\"20\"/>";
    vec![
        ("w-h", "幅と高さ（比べの基準）", format!("<svg {ns} width=\"100\" height=\"50\">{shape}</svg>")),
        ("viewbox-only", "viewBox だけ", format!("<svg {ns} viewBox=\"0 0 120 80\">{shape}</svg>")),
        ("none", "どちらもない（中身の外接矩形）", format!("<svg {ns}>{shape}</svg>")),
        ("width-only", "幅だけ", format!("<svg {ns} width=\"100\">{shape}</svg>")),
        ("height-only", "高さだけ", format!("<svg {ns} height=\"100\">{shape}</svg>")),
        (
            "width-only-viewbox",
            "幅だけと viewBox",
            format!("<svg {ns} width=\"240\" viewBox=\"0 0 120 80\">{shape}</svg>"),
        ),
        ("percent", "幅・高さが %（viewBox なし）", format!("<svg {ns} width=\"50%\" height=\"50%\">{shape}</svg>")),
        (
            "percent-viewbox",
            "幅・高さが % と viewBox",
            format!("<svg {ns} width=\"50%\" height=\"100%\" viewBox=\"0 0 120 80\">{shape}</svg>"),
        ),
        ("zero", "幅が 0", format!("<svg {ns} width=\"0\" height=\"50\">{shape}</svg>")),
        ("negative", "幅が負", format!("<svg {ns} width=\"-10\" height=\"50\">{shape}</svg>")),
        ("fraction", "幅が 1 に満たない", format!("<svg {ns} width=\"0.3\" height=\"50\">{shape}</svg>")),
        ("fraction-up", "幅が 10.5（切り上げ）", format!("<svg {ns} width=\"10.5\" height=\"50\">{shape}</svg>")),
        ("mm", "幅が 10mm", format!("<svg {ns} width=\"10mm\" height=\"50\">{shape}</svg>")),
        ("em", "幅が 10em", format!("<svg {ns} width=\"10em\" height=\"50\">{shape}</svg>")),
        ("auto", "幅が auto", format!("<svg {ns} width=\"auto\" height=\"50\">{shape}</svg>")),
        ("bogus", "幅が壊れた値", format!("<svg {ns} width=\"abc\" height=\"50\">{shape}</svg>")),
        ("viewbox-zero", "viewBox の幅が 0", format!("<svg {ns} viewBox=\"0 0 0 80\">{shape}</svg>")),
        ("viewbox-negative", "viewBox の幅が負", format!("<svg {ns} viewBox=\"0 0 -10 80\">{shape}</svg>")),
        (
            "none-negative-content",
            "どちらもない・中身が負の座標",
            format!("<svg {ns}><rect x=\"-30\" y=\"-20\" width=\"50\" height=\"40\"/></svg>"),
        ),
        ("none-empty", "どちらもない・中身もない", format!("<svg {ns}/>")),
        (
            "none-transform",
            "どちらもない・変形した中身",
            format!("<svg {ns}><g transform=\"translate(100 50)\"><rect width=\"10\" height=\"10\"/></g></svg>"),
        ),
        (
            "none-hidden",
            "どちらもない・見えない中身（visibility・display）",
            format!(
                "<svg {ns}><rect width=\"30\" height=\"30\"/><rect width=\"200\" height=\"200\" visibility=\"hidden\"/><rect width=\"300\" height=\"300\" display=\"none\"/></svg>"
            ),
        ),
    ]
}

/// 乗算済みの画素をストレートの色にする。
fn straight(px: &[u8]) -> ([u8; 3], u8) {
    let a = px[3];
    let d = |c: u8| crate::image::demultiply(c, a);
    ([d(px[0]), d(px[1]), d(px[2])], a)
}

fn dist(a: [u8; 3], b: [u8; 3]) -> u32 {
    (0..3).map(|i| u32::from(a[i].abs_diff(b[i]))).sum()
}

/// 点の色が yes・no のどちらに近いか。
fn judge(img: &crate::image::Rgba, p: &Probe) -> String {
    let (x, y) = p.point;
    let i = ((y * img.w + x) * 4) as usize;
    let Some(px) = img.px.get(i..i + 4) else { return "（範囲の外）".into() };
    let (c, a) = straight(px);
    if a < 128 {
        return "透明".into();
    }
    let (dy, dn) = (dist(c, p.yes), dist(c, p.no));
    if dy.min(dn) > 120 {
        return format!("どちらでもない（{},{},{}）", c[0], c[1], c[2]);
    }
    if dy < dn { "効く".into() } else { "効かない".into() }
}

pub fn run(args: &[String]) -> Result<(), String> {
    check_args(args, &["--out"], &[], 0)?;
    let root = workspace_root();
    let out = arg(args, "--out").map(PathBuf::from).unwrap_or_else(|| root.join("tests/svg/m1-probe"));
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let resvg = Resvg::new(&Fonts::None);
    let chrome = Chrome::find().ok();
    let work = root.join(format!("local/probe/work-{}", std::process::id()));
    let mut lines = String::new();
    let mut md = String::from(
        "# M1 の照らし（試しの SVG）\n\n`cargo run -p sumie-tools --release -- probe` が作って描いた結果（roadmap M1 の依頼文。範囲はこの結果を見て\n\
         決める）。「効く」は決めた点の色が効いたときの色に近いこと。resvg は Kura の設定の 0.48.1、Chrome はヘッドレス\n\
         （`--disable-gpu`）。試しの SVG はこのリポジトリのために書いたもの（MIT OR Apache-2.0）。\n",
    );
    let mut group = "";
    for p in probes() {
        let file = out.join(format!("{}.svg", p.name));
        std::fs::write(&file, &p.body).map_err(|e| e.to_string())?;
        let r = match resvg.render(p.body.as_bytes(), 1.0) {
            Ok(o) => judge(&o.img, &p),
            Err(e) => format!("誤り（{e}）"),
        };
        let c = match &chrome {
            Some(ch) => match ch.render(&file, 100, 100, (100.0, 100.0), &work.join(p.name)) {
                Ok((img, _)) => judge(&img, &p),
                Err(e) => format!("誤り（{e}）"),
            },
            None => "（Chrome がない）".into(),
        };
        if p.group != group {
            group = p.group;
            let _ = write!(md, "\n## {group}\n\n| 試し | 何を見るか | resvg | Chrome |\n|---|---|---|---|\n");
        }
        let _ = writeln!(md, "| `{}` | {} | {r} | {c} |", p.name, p.what.replace('|', "\\|"));
        let l =
            Line::new().str("group", p.group).str("name", p.name).str("resvg", r.as_str()).str("chrome", c.as_str());
        lines.push_str(&l.to_json());
        lines.push('\n');
    }
    let _ = write!(md, "\n## 大きさ\n\n| 試し | 何を見るか | resvg の大きさ |\n|---|---|---|\n");
    let size_dir = out.join("size");
    std::fs::create_dir_all(&size_dir).map_err(|e| e.to_string())?;
    for (name, what, body) in size_probes() {
        std::fs::write(size_dir.join(format!("{name}.svg")), &body).map_err(|e| e.to_string())?;
        let r = match resvg.tree(body.as_bytes()) {
            Ok(t) => {
                let (w, h) = Resvg::size(&t);
                let (cw, ch) = Resvg::css_size(&t);
                format!("{w} × {h}（{cw} × {ch}）")
            }
            Err(e) => format!("誤り（{e}）"),
        };
        let _ = writeln!(md, "| `size/{name}` | {what} | {r} |");
        let l = Line::new().str("group", "大きさ").str("name", name).str("resvg", r.as_str());
        lines.push_str(&l.to_json());
        lines.push('\n');
    }
    let _ = std::fs::remove_dir_all(&work);
    std::fs::write(out.join("README.md"), md).map_err(|e| e.to_string())?;
    let res = root.join("local/probe/result.jsonl");
    std::fs::create_dir_all(res.parent().ok_or("親がない")?).map_err(|e| e.to_string())?;
    std::fs::write(&res, lines).map_err(|e| e.to_string())?;
    println!("{} と {} を書いた", out.join("README.md").display(), res.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_have_unique_names_and_parse() {
        let ps = probes();
        let mut names: Vec<&str> = ps.iter().map(|p| p.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len(), "試しの名前が重なる");
        // どれも XML として読める（roxmltree）
        for p in &ps {
            let opt = resvg::usvg::roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
            assert!(resvg::usvg::roxmltree::Document::parse_with_options(&p.body, opt).is_ok(), "{}", p.name);
        }
    }
}
