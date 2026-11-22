//! Kura の `crates/kura-engine/src/codec/svg.rs` の試験にある攻撃の例（roadmap M1 の完了条件）を、読む段の全体
//! （`estimate_parse`・`parse`）と木を作る段の見積もり（`estimate_build`）に通す。どの例も 128 KiB のスタックで回し、
//! `fuzzing::document_parse` の不変の条件（見積もりと実際が同じ・上限のきわ・D2）と、ヒープの山 ≤ 数えた山を確かめる。
//!
//! 期待は Kura と同じでなく、この描画器の仕様に合わせる：Kura が usvg の都合で断るもの（参照の輪・marker の中の marker・
//! 深さ 64・CSS の結合子の数など）は、断らずに上限の内で読む（FR-CO-05・FR-CO-06・NFR-10 の判定の範囲）。展開の爆弾は
//! 木を作る段の見積もりで断る。描く段の上限で断るもの（頂点の多いパス × marker・巨大なキャンバス）は M2・M4 で確かめる
//! （ここでは読めることだけを確かめる）。

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use sumie::fuzzing::{Check, document_parse, values_parse};
use sumie::{Error, Estimate, Feature, Never, ParseOptions, Resource, WarningKind};
use sumie_testkit::alloc::{assert_heap_within, measure};
use sumie_testkit::stack::on_small_stack;

/// 読んだ結果。
#[derive(Debug)]
struct Seen {
    /// 読む段の誤り
    error: Option<Error>,
    /// 警告の種類（位置の順）
    warnings: Vec<WarningKind>,
    /// ユーザー単位の大きさ
    size: Option<(f64, f64)>,
    /// 木を作る段の見積もり（読めたとき）
    build: Option<Result<Estimate, Error>>,
}

impl Seen {
    fn ok(&self) -> bool {
        self.error.is_none()
    }

    fn cycles(&self) -> usize {
        self.warnings.iter().filter(|&&k| k == WarningKind::ReferenceCycle).count()
    }

    fn limit(&self) -> Option<Resource> {
        match self.error {
            Some(Error::Limit(l)) => Some(l.resource),
            _ => None,
        }
    }

    fn unsupported(&self) -> Option<Feature> {
        match self.error {
            Some(Error::Unsupported(u)) => Some(u.feature),
            _ => None,
        }
    }

    fn build_ok(&self) -> bool {
        matches!(self.build, Some(Ok(_)))
    }

    fn build_limit(&self) -> Option<Resource> {
        match self.build {
            Some(Err(Error::Limit(l))) => Some(l.resource),
            _ => None,
        }
    }
}

/// 128 KiB のスタックで読む段の全体を通し、不変の条件とヒープを確かめて結果を返す。
#[track_caller]
fn run(src: impl Into<Vec<u8>>) -> Seen {
    let src = src.into();
    let head: String = String::from_utf8_lossy(&src).chars().take(160).collect();
    let (check, heap, peak, seen) = on_small_stack(move || {
        let (out, heap) = measure(|| document_parse(&src));
        // 読めなかったものは読み直さない（上限で断る重い例を 2 度回さない）
        let parsed = match out.error {
            Some(e) => Err(e),
            None => sumie::parse(&src, &ParseOptions::DEFAULT, &Never),
        };
        let seen = match parsed {
            Ok(d) => Seen {
                error: None,
                warnings: d.report().warnings().map(|w| w.kind).collect(),
                size: d.size().map(|s| (s.width, s.height)),
                build: Some(d.estimate_build(&Never)),
            },
            Err(e) => Seen { error: Some(e), warnings: Vec::new(), size: None, build: None },
        };
        (out.check, heap, out.peak, seen)
    });
    assert_eq!(check, Check::Ok, "{head}");
    assert_heap_within(&heap, peak, 0, &head);
    assert_eq!(heap.current, 0, "{head}：返していない確保");
    seen
}

const X: &str = r#"xmlns="http://www.w3.org/2000/svg""#;
const XL: &str = r#"xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink""#;

// ---- 実体の爆弾・外の実体・展開の爆弾（Kura：expansion_bombs_are_refused） ----

#[test]
fn entity_bombs_are_refused() {
    let a = "a".repeat(1_000);
    let refs = |n: usize| "&a;".repeat(n);
    // 参照を含む実体はそれ自体を断る（FR-IN-07）
    let nested = format!(
        r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a "{a}"><!ENTITY b "&a;&a;">]><svg {X} width="1" height="1"><desc>&b;</desc></svg>"#
    );
    assert_eq!(run(nested).unsupported(), Some(Feature::MarkupEntity));
    // 値に '>' を含む大きな実体を大量に参照（引用符の中の '>' で値を短く数えない）：展開のバイト数の上限
    let big = "x".repeat(100_000);
    let gt = format!(
        r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a ">{big}">]><svg {X} width="1" height="1"><desc>{}</desc></svg>"#,
        refs(1_000)
    );
    assert_eq!(run(gt).limit(), Some(Resource::Expanded));
    // 注釈の中の偽の DOCTYPE に惑わされない
    let fake = format!(
        r#"<?xml version="1.0"?><!-- <!DOCTYPE x []> --><!DOCTYPE svg [<!ENTITY a "{big}">]><svg {X} width="1" height="1"><desc>{}</desc></svg>"#,
        refs(1_000)
    );
    assert_eq!(run(fake).limit(), Some(Resource::Expanded));
    // 上限の内なら読める（100 KB × 100 = 10 MB）
    let within = format!(
        r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a "{big}">]><svg {X} width="1" height="1"><desc>{}</desc></svg>"#,
        refs(100)
    );
    assert!(run(within).ok());
    // パラメタ実体・外部の実体は読まない
    let param = format!(r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY % p "x">]><svg {X} width="1" height="1"/>"#);
    assert_eq!(run(param).unsupported(), Some(Feature::ParameterEntity));
    let external = format!(
        r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY e SYSTEM "file:///etc/hosts">]><svg {X} width="1" height="1"><desc>&e;</desc></svg>"#
    );
    assert_eq!(run(external).unsupported(), Some(Feature::ExternalEntity));
    // Illustrator 風の単純な実体（名前空間の実体）は読める
    let ai = r##"<?xml version="1.0"?><!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd" [<!ENTITY ns_svg "http://www.w3.org/2000/svg"><!ENTITY ns_flows "http://ns.adobe.com/Flows/1.0/">]><svg xmlns="&ns_svg;" width="2" height="2"><rect width="2" height="2" fill="#00ff00"/></svg>"##;
    let s = run(ai);
    assert!(s.ok() && s.warnings.is_empty() && s.build_ok(), "{s:?}");
    assert_eq!(s.size, Some((2.0, 2.0)));
}

#[test]
fn instance_bombs_are_refused_before_building() {
    // use の倍々（2²⁹ 個を超える）：読む段は通り、木を作る段の見積もりで断る
    let mut uses = format!(r##"<svg {XL} width="1" height="1"><defs><rect id="l0" width="1" height="1"/>"##);
    for i in 1..30 {
        uses.push_str(&format!(r##"<g id="l{i}"><use xlink:href="#l{p}"/><use xlink:href="#l{p}"/></g>"##, p = i - 1));
    }
    uses.push_str(r##"</defs><use xlink:href="#l29"/></svg>"##);
    let s = run(uses);
    assert!(s.ok(), "{s:?}");
    assert_eq!(s.build_limit(), Some(Resource::Instances), "{s:?}");
    // 倍々でも浅ければ読める（2¹⁰）
    let mut small = format!(r##"<svg {XL} width="1" height="1"><defs><rect id="l0" width="1" height="1"/>"##);
    for i in 1..11 {
        small.push_str(&format!(r##"<g id="l{i}"><use xlink:href="#l{p}"/><use xlink:href="#l{p}"/></g>"##, p = i - 1));
    }
    small.push_str(r##"</defs><use xlink:href="#l10"/></svg>"##);
    assert!(run(small).build_ok());
    // 自分を指す use：輪の参照だけを除き、警告する（Kura は断る。FR-CO-05・FR-CO-06）
    let selfref = format!(r##"<svg {XL} width="1" height="1"><g id="a"><use xlink:href="#a"/></g></svg>"##);
    let s = run(selfref);
    assert!(s.ok() && s.cycles() == 1 && s.build_ok(), "{s:?}");
    // marker の中の marker（Kura は断る）：marker の中の marker は描かない（FR-PT-07）ので、輪でも爆弾でもない
    let chain = format!(
        r##"<svg {X} width="10" height="10"><defs><marker id="m1"><path d="M0 0 L1 1 L2 2" marker-mid="url(#m2)"/></marker><marker id="m2"><rect width="1" height="1"/></marker></defs><path d="M0 0 L5 5 L9 9" marker-mid="url(#m1)"/></svg>"##
    );
    let s = run(chain);
    assert!(s.ok() && s.cycles() == 0 && s.build_ok(), "{s:?}");
    // marker が自分を指す：輪
    let selfm = format!(
        r##"<svg {X} width="10" height="10"><defs><marker id="m"><path d="M0 0 L1 1 L2 2" marker-mid="url(#m)"/></marker></defs><path d="M0 0 L5 5 L9 9" marker-mid="url(#m)"/></svg>"##
    );
    let s = run(selfm);
    assert!(s.ok() && s.cycles() == 1 && s.build_ok(), "{s:?}");
    // 頂点の多いパス × 大きな marker（Kura は断る）：頂点の数はパスを読む木を作る段で数える（M2 で断ることを確かめる）。
    // 読む段は読める
    let mut big_marker = format!(r##"<svg {X} width="10" height="10"><defs><marker id="m">"##);
    for _ in 0..4_000 {
        big_marker.push_str(r#"<rect width="1" height="1"/>"#);
    }
    big_marker.push_str("</marker></defs><path d=\"M0 0");
    for i in 0..2_000 {
        big_marker.push_str(&format!(" L{} {}", i % 10, i % 7));
    }
    big_marker.push_str("\" marker-mid=\"url(#m)\"/></svg>");
    assert!(run(big_marker).ok());
    // 巨大なキャンバス（Kura は断る）：キャンバスの上限は描く前に見る（M4）。読む段は大きさを返す
    let huge = format!(r#"<svg {X} width="100000" height="100000"/>"#);
    let s = run(huge);
    assert!(s.ok(), "{s:?}");
    assert_eq!(s.size, Some((100_000.0, 100_000.0)));
    // 小さな marker は読める
    let small = format!(
        r##"<svg {X} width="10" height="10"><defs><marker id="m" markerWidth="2" markerHeight="2"><rect width="1" height="1"/></marker></defs><path d="M0 0 L5 5 L9 9" stroke="black" marker-mid="url(#m)"/></svg>"##
    );
    let s = run(small);
    assert!(s.ok() && s.warnings.is_empty() && s.build_ok(), "{s:?}");
}

// ---- 深い入れ子（Kura：deep_nesting_is_refused_before_parsing） ----

fn nested(depth: usize) -> String {
    format!(
        r#"<svg {X} width="4" height="4">{}<rect width="2" height="2"/>{}</svg>"#,
        "<g>".repeat(depth),
        "</g>".repeat(depth)
    )
}

#[test]
fn deep_nesting_is_bounded() {
    // Kura は 64 段で断る。この描画器は再帰しないので 1,024 段まで読む（根の svg と葉の rect を含む）
    for d in [62, 63, 1_000, 1_022] {
        let s = run(nested(d));
        assert!(s.ok() && s.build_ok(), "{d}：{s:?}");
    }
    assert_eq!(run(nested(1_023)).limit(), Some(Resource::Depth));
    // 10 万段も落ちずに断る
    assert_eq!(run(nested(100_000)).limit(), Some(Resource::Depth));
    // 注釈・CDATA・引用符の中の '<'・'>' は数えない
    let tricky = format!(
        r#"<svg {X} width="4" height="4"><!-- {} --><desc><![CDATA[{}]]></desc><g id="a>b"/></svg>"#,
        "<g>".repeat(1_000),
        "<g>".repeat(1_000)
    );
    assert!(run(tricky).ok());
    // use の 40 段の鎖（Kura は usvg の展開の高さで断る）：明示の積み上げなので読む
    let mut chain = format!(r##"<svg {XL} width="4" height="4"><defs><g id="u0"><rect width="1" height="1"/></g>"##);
    for i in 1..40 {
        chain.push_str(&format!(r##"<g id="u{i}"><use xlink:href="#u{}"/></g>"##, i - 1));
    }
    chain.push_str(r##"</defs><use xlink:href="#u39"/></svg>"##);
    let s = run(chain);
    assert!(s.ok() && s.warnings.is_empty() && s.build_ok(), "{s:?}");
}

// ---- 参照の輪（Kura：reference_cycles_are_refused・reference_resolution_covers_what_usvg_follows） ----

#[test]
fn reference_cycles_are_cut_and_warned() {
    let x = r#"xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="6""#;
    // （文書, 輪の警告の数）。Kura はどれも断る。この描画器は輪の参照だけを除いて読む（FR-CO-05・ADR-64 (9)）
    let cycles = [
        // 根の fill を pattern の中身が受け継いで自分を描く
        (
            format!(
                r##"<svg {x} fill="url(#p)"><defs><pattern id="p" width="2" height="2" patternUnits="userSpaceOnUse"><rect width="1" height="1"/></pattern></defs><rect width="8" height="6"/></svg>"##
            ),
            1,
        ),
        (
            format!(
                r##"<svg {x}><g stroke="url(#p)"><pattern id="p" width="2" height="2"><rect width="1" height="1"/></pattern><rect width="8" height="6"/></g></svg>"##
            ),
            1,
        ),
        // CSS で受け継ぐ
        (
            format!(
                r##"<svg {x}><style>.a {{ fill: url(#p) }}</style><g class="a"><pattern id="p" width="2" height="2"><rect width="1" height="1"/></pattern></g></svg>"##
            ),
            1,
        ),
        // CSS の clip-path が clipPath の中の rect にも当たる
        (
            format!(
                r##"<svg {x}><style>g > rect {{ clip-path: url(#c) }}</style><clipPath id="c"><g><rect width="1" height="1"/></g></clipPath><g><rect width="8" height="6"/></g></svg>"##
            ),
            1,
        ),
        // 2 つの pattern が互いを塗る
        (
            format!(
                r##"<svg {x}><defs><pattern id="p" width="2" height="2"><rect width="1" height="1" fill="url(#q)"/></pattern><pattern id="q" width="2" height="2"><rect width="1" height="1" fill="url(#p)"/></pattern></defs><rect width="8" height="6" fill="url(#p)"/></svg>"##
            ),
            1,
        ),
        // mask の中身が自分を mask にする
        (
            format!(
                r##"<svg {x}><defs><mask id="m"><rect width="8" height="6" mask="url(#m)"/></mask></defs><rect width="8" height="6" mask="url(#m)"/></svg>"##
            ),
            1,
        ),
        // 複雑なセレクタの参照先が、中身を href の先から借りる
        (
            format!(
                r##"<svg {x}><style>svg rect{{fill:url(#p)}}</style><defs><pattern id="p" xlink:href="#q"/><pattern id="q" width="2" height="2" patternUnits="userSpaceOnUse"><rect width="1" height="1"/></pattern></defs><rect width="8" height="6"/></svg>"##
            ),
            1,
        ),
        // href の前の空白（URL の前後の空白は除く）
        (format!(r##"<svg {x}><g id="a"><use href=" #a"/></g></svg>"##), 1),
        // href と xlink:href を並べると href が勝つ（SVG 2）
        (format!(r##"<svg {x}><g id="a"><use xlink:href="#b" href="#a"/></g><rect id="b"/></svg>"##), 1),
        // 注釈の中の '}' のあとの受け継ぐ参照
        (
            format!(
                r##"<svg {x}><style>svg {{ /* }} */ fill: url(#p) }}</style><pattern id="p" width="2" height="2"><rect width="1" height="1"/></pattern></svg>"##
            ),
            1,
        ),
        // style 属性の中の注釈
        (
            format!(
                r##"<svg {x} style="/**/fill: url(#p)"><pattern id="p" width="2" height="2"><rect width="1" height="1"/></pattern></svg>"##
            ),
            1,
        ),
        // @media の中の規則（screen は真）
        (
            format!(
                r##"<svg {x}><style>@media screen {{ g {{ fill: url(#p) }} }}</style><g><pattern id="p" width="2" height="2"><rect width="1" height="1"/></pattern></g></svg>"##
            ),
            1,
        ),
    ];
    for (doc, n) in cycles {
        let s = run(doc.as_str());
        assert!(s.ok() && s.build_ok(), "{doc}：{s:?}");
        assert_eq!(s.cycles(), n, "{doc}：{s:?}");
    }
    // 輪にならないもの（Kura が保守的に輪とみなしたものを含む）
    let fine = [
        // Illustrator の書き出し
        format!(
            r##"<svg {x}><style>.st0{{clip-path:url(#c);}} .st1{{fill:url(#g)}}</style><defs><clipPath id="c"><rect width="6" height="4"/></clipPath><linearGradient id="g"><stop offset="0" stop-color="#f00"/></linearGradient></defs><g class="st0"><rect class="st1" width="8" height="6"/></g></svg>"##
        ),
        format!(
            r##"<svg {x}><defs><pattern id="p" width="2" height="2" patternUnits="userSpaceOnUse"><rect width="1" height="1" fill="#000"/></pattern></defs><rect width="8" height="6" fill="url(#p)"/></svg>"##
        ),
        // 古い書き方の性質の名前（`*fill`）は CSS の宣言として壊れているので捨てる（輪にならない）
        format!(
            r##"<svg {x} style="*fill:url(#p)"><defs><pattern id="p" width="2" height="2" patternUnits="userSpaceOnUse"><rect width="1" height="1"/></pattern></defs><rect width="8" height="6"/></svg>"##
        ),
        format!(
            r##"<svg {x}><style>svg{{*fill:url(#p)}}</style><defs><pattern id="p" width="2" height="2" patternUnits="userSpaceOnUse"><rect width="1" height="1"/></pattern></defs><rect width="8" height="6"/></svg>"##
        ),
        // 同じ id は最初の要素を指す（ADR-66 (5)）：後の p の中身は前の p を塗るだけ
        format!(
            r##"<svg {x}><defs><pattern id="p" width="2" height="2"><rect width="1" height="1"/></pattern><pattern id="p" width="2" height="2"><rect width="1" height="1" fill="url(#p)"/></pattern></defs><rect width="8" height="6" fill="url(#p)"/></svg>"##
        ),
        // 引用符の中の '}'・注釈のある CSS
        format!(
            r##"<svg {x}><style>/* a }} b */ .st0 {{ fill: url("#g") }} .st1{{font-family:"a}}b"}}</style><defs><linearGradient id="g"><stop offset="0" stop-color="#f00"/></linearGradient></defs><rect class="st0 st1" width="8" height="6"/></svg>"##
        ),
        // symbol を use で使う・gradient の href の鎖
        format!(
            r##"<svg {x}><defs><symbol id="s"><rect width="2" height="2"/></symbol></defs><use href="#s"/><use xlink:href="#s" x="3"/></svg>"##
        ),
        format!(
            r##"<svg {x}><defs><linearGradient id="a"><stop offset="0" stop-color="#f00"/></linearGradient><linearGradient id="b" href="#a"/></defs><rect width="8" height="6" fill="url(#b)"/></svg>"##
        ),
    ];
    for doc in fine {
        let s = run(doc.as_str());
        assert!(s.ok() && s.cycles() == 0 && s.build_ok(), "{doc}：{s:?}");
    }
    // xml:id：id として読むかは照らしてから決める（M3）。どちらでも上限の内で読む
    let xml_id = format!(
        r##"<svg {x}><defs><pattern xml:id="q" width="2" height="2"><rect width="1" height="1" fill="url(#q)"/></pattern></defs><rect width="8" height="6" fill="url(#q)"/></svg>"##
    );
    assert!(run(xml_id).ok());
    // 参照を含む CSS の規則が多い（Kura は 5,000 で断る）：上限の内で読む
    let mut many = format!(r#"<svg {x}><style>"#);
    for i in 0..5_000 {
        many.push_str(&format!(".c{i}{{fill:url(#g)}}"));
    }
    many.push_str("</style></svg>");
    assert!(run(many).ok());
}

// ---- CSS と参照の手間（Kura：css_and_reference_work_is_bounded・css_costs_are_bounded） ----

#[test]
fn css_and_reference_work_is_bounded() {
    let x = r#"xmlns="http://www.w3.org/2000/svg" width="4" height="4""#;
    // 12,000 の規則 × 12,000 の要素（Kura は断る）：鍵の索引で当たる規則だけを照らすので読む
    let rules: String = (0..12_000).map(|i| format!(".c{i}{{}}")).collect();
    assert!(run(format!("<svg {x}><style>{rules}</style>{}</svg>", "<g/>".repeat(12_000))).ok());
    // クラスの当たる 12,000 × 12,000
    let rules: String = (0..12_000).map(|i| format!(".c{i}{{fill:red}}")).collect();
    let els: String = (0..12_000).map(|i| format!(r#"<g class="c{i}"/>"#)).collect();
    assert!(run(format!("<svg {x}><style>{rules}</style>{els}</svg>")).ok());
    // 深い入れ子のどの段にも参照の多い規則が当たり、その中に図形が並ぶ（受け継ぐ参照は勝った 1 つだけ）
    let rules: String = (0..4_000).map(|i| format!("g{{fill:url(#p{i})}}")).collect();
    let patterns: String = (0..4_000).map(|i| format!(r#"<pattern id="p{i}"/>"#)).collect();
    let deep = format!(
        "<svg {x}><style>{rules}</style><defs>{patterns}</defs>{}{}{}</svg>",
        "<g>".repeat(60),
        "<rect/>".repeat(5_000),
        "</g>".repeat(60)
    );
    assert!(run(deep).ok());
    let dangling = format!(
        "<svg {x}><style>{rules}</style>{}{}{}</svg>",
        "<g>".repeat(60),
        "<clipPath/>".repeat(120),
        "</g>".repeat(60)
    );
    assert!(run(dangling).ok());
    // ふつうの大きさ
    let rules: String = (0..300).map(|i| format!(".c{i}{{fill:#000}}")).collect();
    let fine =
        format!("<svg {x}><style>{rules}</style>{}</svg>", r#"<rect class="c1" width="1" height="1"/>"#.repeat(3_000));
    let s = run(fine);
    assert!(s.ok() && s.warnings.is_empty() && s.build_ok(), "{s:?}");
}

#[test]
fn css_costs_are_bounded() {
    let x = r#"xmlns="http://www.w3.org/2000/svg" width="8" height="8""#;
    // 隣接の結合子の長い連鎖（simplecss は段の数だけ再帰する）：再帰しないのでスタックは使わないが、照合は要素ごとに
    // 鎖の複合をすべて照らす（11,000 × 11,001 回）ので、読む段の手間の上限で断る（release で 0.8 秒ほど。`sumie-tools latency`）
    let chain = format!("<svg {x}><style>{}g{{fill:red}}</style>{}</svg>", "g+".repeat(11_000), "<g/>".repeat(11_000));
    assert_eq!(run(chain).limit(), Some(Resource::Work));
    // 鎖が長くても要素が少なければ読む
    let short = format!("<svg {x}><style>{}g{{fill:red}}</style>{}</svg>", "g+".repeat(11_000), "<g/>".repeat(500));
    assert!(run(short).ok());
    // 深い入れ子での子孫の結合子の後戻り（組み合わせの数 C(61, 6) で増える形）：照合は要素と複合ごとの旗で決めるので
    // 後戻りがない（css/matcher.rs）
    let deep = format!(
        "<svg {x}><style>y g g g g g g rect{{fill:red}}</style>{}<rect/>{}</svg>",
        "<g>".repeat(61),
        "</g>".repeat(61)
    );
    let s = run(deep);
    assert!(s.ok() && s.warnings.is_empty(), "{s:?}");
    // 同じ id の要素が多く、1 つの要素がそれを何度も参照する
    let dup = format!(
        r#"<svg {x}><defs>{}</defs><rect width="4" height="4" style="{}"/></svg>"#,
        r#"<clipPath id="p"/>"#.repeat(3_000),
        "fill:url(#p);".repeat(3_000)
    );
    assert!(run(dup).ok());
    // 1 つの規則・style 属性の宣言が多い（usvg は宣言の数の 2 乗。Kura は断る）：線形なので読む
    assert!(run(format!("<svg {x}><style>g{{{}}}</style><g/></svg>", "opacity:1;".repeat(80_000))).ok());
    assert!(run(format!(r#"<svg {x}><g style="{}"/></svg>"#, "opacity:1;".repeat(300_000))).ok());
    // ふつうの CSS：クラスの規則 300 個 × 要素 5,000、子孫の結合子 2 つ
    let rules: String = (0..300).map(|i| format!(".st{i}{{fill:#123456;stroke:#000;stroke-width:1}}")).collect();
    let shapes: String = (0..5_000).map(|i| format!(r#"<rect class="st{}" width="1" height="1"/>"#, i % 300)).collect();
    let fine = format!(
        "<svg {x}><style>{rules} svg g rect{{opacity:.9}}</style>{}{shapes}{}</svg>",
        "<g>".repeat(10),
        "</g>".repeat(10)
    );
    let s = run(fine);
    assert!(s.ok() && s.warnings.is_empty() && s.build_ok(), "{s:?}");
}

// ---- 巨大な数・数の走査が終わること（Kura：huge_numbers_are_refused_before_usvg） ----

#[test]
fn huge_numbers_are_checked() {
    let x = r#"xmlns="http://www.w3.org/2000/svg" width="8" height="8""#;
    let warned = |attr: &str| {
        let s = run(format!(r#"<svg {x}><rect width="4" height="4" {attr}/></svg>"#));
        assert!(s.ok(), "{attr}：{s:?}");
        s.warnings.contains(&WarningKind::InvalidValue)
    };
    // 有限でない数は壊れた値（FR-ST-15）
    for a in [
        r#"stroke-width="1e999""#,
        r#"opacity="-1e999""#,
        r#"transform="scale(1e999)""#,
        r#"style="stroke-width:1e999""#,
    ] {
        assert!(warned(a), "{a}");
    }
    // 有限の大きな数は読む（Kura は usvg の都合で断る。NFR-10 の判定の範囲）
    for a in [
        r#"transform="scale(2e9)""#,
        r#"transform="matrix(1 0 0 1 1e8 -1e8)""#,
        r#"stroke-width="1e30""#,
        r#"style="stroke-width:1e37""#,
        r#"opacity="0.5e-30""#,
        r##"fill="#1e30ff""##,
    ] {
        assert!(!warned(a), "{a}");
    }
    // 数に見える名前
    let names = format!(
        r##"<svg {x}><defs><linearGradient id="g1e30"><stop stop-color="#1e30ff"/></linearGradient></defs><style>.c1e30 {{ fill: url(#g1e30) }}</style><rect id="r2e40" class="c1e30" width="4" height="4"/></svg>"##
    );
    let s = run(names);
    assert!(s.ok() && s.warnings.is_empty() && s.build_ok(), "{s:?}");
}

/// 英数字と記号の 15 字のすべての 3 文字の並び（＋ `1`）で、値の読みと読む段の全体が終わる（Kura がファズで見つけた
/// 数字のあとの `+` の無限ループの形）。
#[test]
fn every_three_character_sequence_terminates() {
    let alphabet = b"0a.+-e#u(lr)E ,";
    let mut n = 0u32;
    for &a in alphabet {
        for &b in alphabet {
            for &c in alphabet {
                let v = [a, b, c, b'1'];
                // 頭のバイトでプロパティと属性かを選ぶ（並びごとに回す）
                let head = (n.wrapping_mul(37) % 256) as u8;
                n += 1;
                let mut data = vec![head];
                data.extend_from_slice(&v);
                assert_eq!(values_parse(&data), Check::Ok, "{:?}", String::from_utf8_lossy(&v));
                let v = core::str::from_utf8(&v).unwrap();
                let doc = format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8" viewBox="{v}"><rect transform="{v}" stroke-width="{v}" fill="{v}" style="opacity:{v};transform:{v}"/></svg>"#
                );
                assert_eq!(document_parse(doc.as_bytes()).check, Check::Ok, "{doc}");
            }
        }
    }
}

// ---- DTD のまわりの多バイト文字（Kura：multibyte_text_around_the_dtd） ----

#[test]
fn multibyte_text_around_the_dtd() {
    let svg = r#"xmlns="http://www.w3.org/2000/svg" width="4" height="4""#;
    // 落ちずに読むか、断る
    for doc in [
        format!(r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY ns "鞍"><!-- 注釈 -->]><svg {svg}/>"#),
        format!(r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY ns "x">]>鞍<svg {svg}/>"#),
        r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY ns "x"><circle/></svg>ü鞍"#.to_string(),
        format!(r#"<?xml version="1.0"?><!DOCTYPE svg [ü鞍 'あ' "い" <?pi う?>]><svg {svg}/>"#),
        format!(r#"<!-- 前置きの注釈 --><?pi え?><svg {svg}/>"#),
    ] {
        let _ = run(doc);
    }
    assert!(run(format!(r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY ns "鞍">]><svg {svg}/>"#)).ok());
    assert!(run(format!(r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY ns "鞍"><!-- 注釈 -->]><svg {svg}/>"#)).ok());
    assert!(run(format!(r#"<!-- 前置きの注釈 --><?pi え?><svg {svg}/>"#)).ok());
}
