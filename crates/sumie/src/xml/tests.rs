#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use super::*;
use crate::budget::Budget;
use crate::cancel::Never;
use crate::error::{Error, Feature, InvalidReason, Resource, Stage};
use crate::fuzzing::{Check, xml_parse_with};

const SVG: &str = "xmlns=\"http://www.w3.org/2000/svg\"";

/// 数える段と作る段を通し、見積もりの約束と鎖を確かめてから、読んだ文書を返す。
fn parse_with(src: &[u8], limits: &Limits) -> Result<(XmlDoc, XmlCounts), Error> {
    let o = xml_parse_with(src, limits);
    assert_eq!(o.check, Check::Ok, "{o:?}");
    assert!(o.build_work <= o.build_estimate && o.build_peak <= o.build_peak_estimate, "{o:?}");
    let eb = Budget::new(Stage::EstimateParse, limits.estimate_work, limits.estimate_bytes, &Never);
    let counts = count(src, limits, &eb, Stage::EstimateParse)?;
    let b = Budget::new(Stage::Parse, counts.build_work(), counts.build_peak(), &Never);
    let doc = build(src, &counts, limits, &b, Stage::Parse)?;
    // 結果の量は数えた結果の量と同じ
    assert_eq!(doc.bytes(), counts.result_bytes());
    Ok((doc, counts))
}

fn parse(src: &str) -> Result<XmlDoc, Error> {
    parse_with(src.as_bytes(), &Limits::DEFAULT).map(|(d, _)| d)
}

fn reason(r: Result<XmlDoc, Error>) -> Option<InvalidReason> {
    match r {
        Err(Error::Invalid(i)) => Some(i.reason),
        _ => None,
    }
}

fn feature(r: Result<XmlDoc, Error>) -> Option<Feature> {
    match r {
        Err(Error::Unsupported(u)) => Some(u.feature),
        _ => None,
    }
}

/// 要素の局所名の並び（行きがけの順）。
fn names(doc: &XmlDoc, src: &str) -> Vec<String> {
    doc.nodes
        .as_slice()
        .iter()
        .filter(|n| n.kind == KIND_ELEMENT)
        .map(|n| String::from_utf8_lossy(doc.local(src.as_bytes(), n)).to_string())
        .collect()
}

#[test]
fn nodes_attrs_and_links() {
    let src = format!("<svg {SVG} width=\"10\"><g id=\"a\"><rect/>text<circle r='1'/></g><!-- c --><?pi x?></svg>");
    let doc = parse(&src).unwrap();
    assert_eq!(names(&doc, &src), ["svg", "g", "rect", "circle"]);
    let b = src.as_bytes();
    let root = doc.node(0).unwrap();
    assert_eq!(root.ns, NS_SVG);
    assert_eq!(doc.attr(b, root, NS_NONE, b"width"), Some(b"10".as_slice()));
    // xmlns は属性に入れない
    assert_eq!(doc.attrs_of(root).len(), 1);
    let g = doc.node(root.first_child).unwrap();
    assert_eq!(doc.attr(b, g, NS_NONE, b"id"), Some(b"a".as_slice()));
    let rect = doc.node(g.first_child).unwrap();
    let text = doc.node(rect.next_sibling).unwrap();
    assert_eq!(text.kind, KIND_TEXT);
    assert_eq!(doc.text(b, text), b"text");
    let circle = doc.node(text.next_sibling).unwrap();
    assert_eq!(doc.attr(b, circle, NS_NONE, b"r"), Some(b"1".as_slice()));
    assert_eq!(circle.next_sibling, NONE);
}

#[test]
fn namespaces() {
    let b = |s: &str| parse(s).map(|d| d.nodes.as_slice().iter().map(|n| n.ns).collect::<Vec<_>>());
    // 接頭辞つきの SVG・既定の名前空間の入れ子・XLink
    let src = "<s:svg xmlns:s=\"http://www.w3.org/2000/svg\"><s:rect/><x xmlns=\"urn:x\"><y/></x></s:svg>";
    assert_eq!(b(src).unwrap(), [NS_SVG, NS_SVG, NS_OTHER, NS_OTHER]);
    let src =
        format!("<svg {SVG} xmlns:xlink=\"http://www.w3.org/1999/xlink\"><use xlink:href=\"#a\" href=\"#b\"/></svg>");
    let doc = parse(&src).unwrap();
    let u = doc.node(1).unwrap();
    assert_eq!(doc.attr(src.as_bytes(), u, NS_XLINK, b"href"), Some(b"#a".as_slice()));
    assert_eq!(doc.attr(src.as_bytes(), u, NS_NONE, b"href"), Some(b"#b".as_slice()));
    // 名前空間のない文書・xml の接頭辞
    assert_eq!(b("<svg><rect xml:space=\"preserve\"/></svg>").unwrap(), [NS_NONE, NS_NONE]);
    // 束ねていない接頭辞・xmlns の誤用
    assert_eq!(reason(parse("<a:svg/>")), Some(InvalidReason::Xml));
    assert_eq!(reason(parse("<svg b:x=\"1\"/>")), Some(InvalidReason::Xml));
    assert_eq!(reason(parse("<svg xmlns:xml=\"urn:x\"/>")), Some(InvalidReason::Xml));
    for bad in [
        "<svg xmlns:a=\"http://www.w3.org/XML/1998/namespace\"/>",
        "<svg xmlns=\"http://www.w3.org/XML/1998/namespace\"/>",
        "<svg xmlns:a=\"http://www.w3.org/2000/xmlns/\"/>",
        "<svg xmlns:xmlns=\"urn:a\"/>",
        "<svg xmlns:p=\"urn:a\" xmlns:p=\"urn:a\"/>",
        "<svg xmlns:=\"urn:a\"/>",
    ] {
        assert_eq!(reason(parse(bad)), Some(InvalidReason::Xml), "{bad}");
    }
    assert_eq!(b("<svg xmlns:xml=\"http://www.w3.org/XML/1998/namespace\"/>").unwrap(), [NS_NONE]);
    // 空の URI に束ねた接頭辞は「そのほか」（roxmltree と同じ。名前空間のない属性とは重ならない）
    let src = "<svg xmlns:p=\"\" p:a=\"1\" a=\"2\"><p:x/></svg>";
    let doc = parse(src).unwrap();
    assert_eq!(doc.nodes.as_slice().iter().map(|n| n.ns).collect::<Vec<_>>(), [NS_NONE, NS_OTHER]);
    assert_eq!(doc.attrs_of(doc.node(0).unwrap()).iter().map(|a| a.ns).collect::<Vec<_>>(), [NS_OTHER, NS_NONE]);
    // 既定の名前空間の宣言の重なりは最初のもの
    assert_eq!(b(&format!("<svg {SVG} xmlns=\"urn:a\"><g/></svg>")).unwrap(), [NS_SVG, NS_SVG]);
    assert_eq!(b(&format!("<svg xmlns=\"urn:a\" {SVG}/>")).unwrap(), [NS_OTHER]);
    // 頭の `:` だけの名前は接頭辞のない名前
    let src = format!("<:svg {SVG}><rect :width=\"1\"/></:svg>");
    let doc = parse(&src).unwrap();
    assert_eq!(names(&doc, &src), ["svg", "rect"]);
    assert_eq!(doc.attr(src.as_bytes(), doc.node(1).unwrap(), NS_NONE, b"width"), Some(b"1".as_slice()));
    assert_eq!(reason(parse("<svg :a=\"1\" a=\"2\"/>")), Some(InvalidReason::Xml));
    // 閉じのタグも接頭辞と局所名で比べる・`:xmlns` は既定の名前空間の宣言
    for good in ["<svg></:svg>", "<:svg></svg>", "<:svg></:svg>"] {
        assert!(parse(good).is_ok(), "{good}");
    }
    assert_eq!(b("<svg :xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap(), [NS_SVG]);
    for bad in ["<a:svg xmlns:a=\"urn:a\"></:svg>", "<svg></::svg>", "<svg></svg:>"] {
        assert_eq!(reason(parse(bad)), Some(InvalidReason::Xml), "{bad}");
    }
    for bad in ["<a:/>", "<:/>", "<::a/>", "<a:b:c/>", "<svg a:=\"1\"/>", "<svg ::a=\"1\"/>", "<svg a:b:c=\"1\"/>"] {
        assert_eq!(reason(parse(bad)), Some(InvalidReason::Xml), "{bad}");
    }
    // 名前空間の束ねは要素の中だけ
    assert_eq!(reason(parse("<svg><g xmlns:p=\"urn:p\"/><p:x/></svg>")), Some(InvalidReason::Xml));
    // 実体で書いた名前空間（Illustrator の書き方）
    let ai = "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\" [<!ENTITY ns_svg \"http://www.w3.org/2000/svg\">]><svg xmlns=\"&ns_svg;\"/>";
    assert_eq!(b(ai).unwrap(), [NS_SVG]);
}

#[test]
fn entities_and_references() {
    let src = format!(
        "<!DOCTYPE svg [<!ENTITY c \"#ff0000\"><!ENTITY c \"blue\">]><svg {SVG} fill=\"&c;\">&lt;&#65;&#x42;&c;</svg>"
    );
    let doc = parse(&src).unwrap();
    let b = src.as_bytes();
    let svg = doc.node(0).unwrap();
    // 同じ名前は最初の宣言
    assert_eq!(doc.attr(b, svg, NS_NONE, b"fill"), Some(b"#ff0000".as_slice()));
    assert_eq!(doc.text(b, doc.node(svg.first_child).unwrap()), b"<AB#ff0000");
    // 知らない実体・壊れた文字の参照・文書の文字でない参照
    assert_eq!(reason(parse(&format!("<svg {SVG}>&nope;</svg>"))), Some(InvalidReason::Xml));
    assert_eq!(reason(parse(&format!("<svg {SVG}>&#xZZ;</svg>"))), Some(InvalidReason::Xml));
    assert_eq!(reason(parse(&format!("<svg {SVG}>&#0;</svg>"))), Some(InvalidReason::Xml));
    assert_eq!(reason(parse(&format!("<svg {SVG}>&amp</svg>"))), Some(InvalidReason::Xml));
}

#[test]
fn dtd_unsupported_like_kura() {
    let s = |d: &str| format!("<!DOCTYPE svg [{d}]><svg {SVG}/>");
    assert_eq!(feature(parse(&s("<!ENTITY % p \"x\">"))), Some(Feature::ParameterEntity));
    assert_eq!(feature(parse(&s("%p;"))), Some(Feature::ParameterEntity));
    assert_eq!(feature(parse(&s("<!ENTITY e SYSTEM \"file:///etc/hosts\">"))), Some(Feature::ExternalEntity));
    for v in ["<g/>", "&amp;", "%p;"] {
        assert_eq!(feature(parse(&s(&format!("<!ENTITY e \"{v}\">")))), Some(Feature::MarkupEntity), "{v}");
    }
    // ELEMENT・ATTLIST・注釈・処理命令は読み飛ばす（ATTLIST の既定の値は当てない）
    let ok = s("<!ELEMENT svg ANY><!ATTLIST svg fill CDATA \"green\" x CDATA '>'><!-- ] --><?pi ]?>");
    let doc = parse(&ok).unwrap();
    assert!(doc.attrs_of(doc.node(0).unwrap()).is_empty());
}

#[test]
fn normalization() {
    let src = format!("<svg {SVG} a=\"x\ty\r\nz\nw\" b=\"&#10;\">a\r\nb\rc<![CDATA[d\r\ne]]></svg>");
    let doc = parse(&src).unwrap();
    let b = src.as_bytes();
    let svg = doc.node(0).unwrap();
    assert_eq!(doc.attr(b, svg, NS_NONE, b"a"), Some(b"x y z w".as_slice()));
    // 文字の参照の改行は揃えない
    assert_eq!(doc.attr(b, svg, NS_NONE, b"b"), Some(b"\n".as_slice()));
    let t = doc.node(svg.first_child).unwrap();
    assert_eq!(doc.text(b, t), b"a\nb\nc");
    let cd = doc.node(t.next_sibling).unwrap();
    assert_eq!(doc.text(b, cd), b"d\ne");
    assert!(cd.flags & FLAG_CDATA != 0);
    // 展開しない値は入力を借りる
    let plain = format!("<svg {SVG} a=\"plain\">t</svg>");
    let (doc, counts) = parse_with(plain.as_bytes(), &Limits::DEFAULT).unwrap();
    assert_eq!(counts.store, 0);
    assert_eq!(doc.attrs_of(doc.node(0).unwrap())[0].flags & FLAG_IN_STORE, 0);
}

#[test]
fn xml_space_is_inherited() {
    let src = format!("<svg {SVG} xml:space=\"preserve\"><g><t>x</t></g><g xml:space=\"default\"><t>y</t></g></svg>");
    let doc = parse(&src).unwrap();
    let pres: Vec<bool> = doc.nodes.as_slice().iter().map(|n| n.flags & FLAG_PRESERVE != 0).collect();
    assert_eq!(pres, [true, true, true, true, false, false, false]);
}

#[test]
fn well_formedness() {
    for bad in [
        "<svg><g></svg>",
        "<svg>",
        "<svg/>x",
        "<svg/><svg/>",
        "<svg a=\"<\"/>",
        "<svg a=\"1\"b=\"2\"/>",
        "<svg a=\"1\" a=\"2\"/>",
        "<svg xmlns:p=\"urn:p\" p:a=\"1\" p:a=\"2\"/>",
        "<svg><!-- a -- b --></svg>",
        "<svg>]]></svg>",
        "<svg><1/></svg>",
        "<svg/><?xml version=\"1.0\"?>",
        " <?xml version=\"1.0\"?><svg/>",
        "<svg a=1/>",
        "<svg>\u{1}</svg>",
        "<svg><!DOCTYPE x></svg>",
        "<!DOCTYPE a><!DOCTYPE b><svg/>",
        "x<svg/>",
        "",
        "<svg></svg >trailing",
        // XML の宣言（文書の頭だけ・形は XMLDecl）
        "<?xml version=\"1.0\"?><?xml version=\"1.0\"?><svg/>",
        "<svg><?xml x?></svg>",
        "<?xml version=\"1.0\" foo=\"bar\"?><svg/>",
        "<?xml encoding=\"UTF-8\"?><svg/>",
        "<?xml ?><svg/>",
        "<?xml version=\"1.0\"encoding=\"UTF-8\"?><svg/>",
        "<?xml version=\"1.0\" standalone=\"no\" encoding=\"UTF-8\"?><svg/>",
        "<?xml version=1.0?><svg/>",
        "<?xml\u{1}?><svg/>",
        "<?xml version=\"1.0\"?>",
        // 文字の参照
        "<svg>&#xFFFE;</svg>",
        "<svg>&#0;</svg>",
        "<svg>&#x1F;</svg>",
        "<svg>&#x100000000;</svg>",
        "<svg>&#4294967296;</svg>",
        "<svg>&#+65;</svg>",
        "<svg>&#X41;</svg>",
        "<svg>&#x;</svg>",
        "<svg a=\"&#xFFFF;\"/>",
        // DTD
        "<!DOCTYPE svg [ x ]><svg/>",
        "<!DOCTYPE svg [<!FOO bar>]><svg/>",
        "<!DOCTYPE svg [<!ENTITYe \"a\">]><svg/>",
        "<!DOCTYPE svg [<!ENTITY e\"a\">]><svg/>",
        "<!DOCTYPE svg [<!ENTITY e \"a\" x>]><svg/>",
        "<!DOCTYPE svg [<!ENTITY e \"a\"]><svg/>",
        "<!DOCTYPE svg [<!-- \u{1} -->]><svg/>",
        "<!DOCTYPE svg [<?pi \u{1}?>]><svg/>",
        "<!DOCTYPE svg [<?xml x?>]><svg/>",
        "<!DOCTYPE svg PUBLIC \"a\"><svg/>",
        "<!DOCTYPE svg SYSTEM><svg/>",
        "<!DOCTYPE svg [<!ENTITY e \"a\u{1}b\">]><svg>&e;</svg>",
        "<!DOCTYPE svg [<!ENTITY e \"a\u{FFFE}b\">]><svg>&e;</svg>",
    ] {
        assert_eq!(reason(parse(bad)), Some(InvalidReason::Xml), "{bad:?}");
    }
    for good in [
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<!-- c --><svg/>\n<!-- after -->",
        "<svg></svg >",
        "<svg a = '1' />",
        "\u{feff}<svg/>",
        "<!DOCTYPE svg><svg/>",
        "<svg><![CDATA[<>&]]></svg>",
        "<svg>a > b</svg>",
        // XML の宣言：頭で空白が続かなければ `?>` まで飛ばす。値は何でも受ける（roxmltree と同じ）
        "<?xml?><svg/>",
        "<?xml$version=\"1.0\"?><svg/>",
        "<?xmlfoo?><svg/>",
        "<?xml-stylesheet href=\"a\"?><svg/>",
        "<?xml version='2.0'?><svg/>",
        "<?xml version = \"1.0\" ?><svg/>",
        "<?xml\tversion=\"1.0\"?><svg/>",
        "<?xml version=\"1.0\" encoding=\"latin1\" standalone=\"maybe\"?><svg/>",
        "\u{feff}<?xml version=\"1.0\"?><svg/>",
        "<svg><?XML x?><?xml-stylesheet a?></svg>",
        // DTD の宣言の中の文字は確かめない・実体の値の文字は属性の中では確かめない（roxmltree と同じ）
        "<!DOCTYPE svg [<!ATTLIST svg x CDATA \"1\u{1}\">]><svg/>",
        "<!DOCTYPE svg SYSTEM \"a\u{1}\"><svg/>",
        "<!DOCTYPE svg [<!ENTITY e \"a\u{1}b\">]><svg a=\"&e;\"/>",
        "<!DOCTYPE svg [<!ENTITY e \"a\u{1}b\">]><svg/>",
        "<!DOCTYPE svg [<!ELEMENTsvg ANY>]><svg/>",
        "<!DOCTYPE svg [<!NOTATION n SYSTEM \"x\">]><svg/>",
        "<!DOCTYPE svg [ ] ><svg/>",
        "<!DOCTYPE svg [<!ENTITY  e   \"a\" >]><svg>&e;</svg>",
        "<!DOCTYPE svg PUBLIC \"a\" \"b\"><svg/>",
    ] {
        assert!(parse(good).is_ok(), "{good:?}");
    }
    // 文字の参照：サロゲートと U+10FFFF を超える値は U+FFFD
    for (src, want) in [
        ("<svg>&#xD800;</svg>", "\u{FFFD}"),
        ("<svg>&#55296;</svg>", "\u{FFFD}"),
        ("<svg>&#x110000;</svg>", "\u{FFFD}"),
        ("<svg>&#xFFFFFFFF;</svg>", "\u{FFFD}"),
        ("<svg>&#4294967295;</svg>", "\u{FFFD}"),
        ("<svg>&#x10FFFF;</svg>", "\u{10FFFF}"),
        ("<svg>&#x000000000000000000041;</svg>", "A"),
    ] {
        let doc = parse(src).unwrap();
        assert_eq!(doc.text(src.as_bytes(), doc.node(1).unwrap()), want.as_bytes(), "{src}");
    }
}

#[test]
fn encodings() {
    assert_eq!(feature(parse_with(&[0x1f, 0x8b, 8, 0], &Limits::DEFAULT).map(|x| x.0)), Some(Feature::Svgz));
    assert_eq!(feature(parse_with(&[0xFF, 0xFE, b'<', 0], &Limits::DEFAULT).map(|x| x.0)), Some(Feature::Encoding));
    let latin = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><svg>\xe9</svg>";
    assert_eq!(feature(parse_with(latin, &Limits::DEFAULT).map(|x| x.0)), Some(Feature::Encoding));
    // ASCII だけなら宣言に依らず読む
    assert!(parse("<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><svg/>").is_ok());
    let bad = b"<svg>\xe9</svg>";
    assert_eq!(reason(parse_with(bad, &Limits::DEFAULT).map(|x| x.0)), Some(InvalidReason::Utf8));
    // 塊の境目をまたぐ多バイトの文字
    let mut long = String::from("<svg>");
    long.push_str(&"a".repeat(65534));
    long.push_str(&"鞍".repeat(10));
    long.push_str("</svg>");
    assert!(parse(&long).is_ok());
}

#[test]
fn limits_are_kept() {
    let lim = |f: &dyn Fn(&mut Limits)| {
        let mut l = Limits::DEFAULT;
        f(&mut l);
        l
    };
    let deep = format!("{}{}", "<g>".repeat(2000), "</g>".repeat(2000));
    match parse_with(deep.as_bytes(), &Limits::DEFAULT) {
        Err(Error::Limit(l)) => assert_eq!(l.resource, Resource::Depth),
        r => panic!("{:?}", r.map(|_| ())),
    }
    let many = format!("<svg>{}</svg>", "<g/>".repeat(100));
    match parse_with(many.as_bytes(), &lim(&|l| l.nodes = 50)) {
        Err(Error::Limit(l)) => assert_eq!(l.resource, Resource::Nodes),
        r => panic!("{:?}", r.map(|_| ())),
    }
    match parse_with(many.as_bytes(), &lim(&|l| l.input_bytes = 10)) {
        Err(Error::Limit(l)) => assert_eq!(l.resource, Resource::Input),
        r => panic!("{:?}", r.map(|_| ())),
    }
}

/// Kura の `svg.rs` の試験の攻撃の例（実体の爆弾・DTD のまわりの多バイト文字）。
#[test]
fn kura_attacks() {
    // 実体の爆弾：64 KiB の値を 1,100 回参照（展開すると 64 MiB を超える）。展開せずに数えて断る
    let a = "x".repeat(1 << 16);
    let r = "&a;".repeat(1100);
    let bomb = format!("<?xml version=\"1.0\"?><!DOCTYPE svg [<!ENTITY a \"{a}\">]><svg {SVG}><desc>{r}</desc></svg>");
    match parse_with(bomb.as_bytes(), &Limits::DEFAULT) {
        Err(Error::Limit(l)) => assert_eq!(l.resource, Resource::Expanded),
        r => panic!("{:?}", r.map(|_| ())),
    }
    // 入れ子の実体（値に参照）は断る
    let nested = format!("<!DOCTYPE svg [<!ENTITY a \"x\"><!ENTITY b \"&a;&a;\">]><svg {SVG}><desc>&b;</desc></svg>");
    assert_eq!(feature(parse(&nested)), Some(Feature::MarkupEntity));
    // 注釈の中の偽の DOCTYPE に惑わされない
    let fake = format!(
        "<?xml version=\"1.0\"?><!-- <!DOCTYPE x []> --><!DOCTYPE svg [<!ENTITY a \"y\">]><svg {SVG}><desc>&a;</desc></svg>"
    );
    assert!(parse(&fake).is_ok());
    // DTD のまわりの多バイト文字
    for (src, ok) in [
        (format!("<?xml version=\"1.0\"?><!DOCTYPE svg [<!ENTITY ns \"鞍\"><!-- 注釈 -->]><svg {SVG}/>"), true),
        (format!("<?xml version=\"1.0\"?><!DOCTYPE svg [<!ENTITY ns \"x\">]>鞍<svg {SVG}/>"), false),
        ("<?xml version=\"1.0\"?><!DOCTYPE svg [<!ENTITY ns \"x\"><circle/></svg>ü鞍".to_string(), false),
        (format!("<?xml version=\"1.0\"?><!DOCTYPE svg [<!ATTLIST svg a CDATA 'あ'> <?pi う?>]><svg {SVG}/>"), true),
        // 宣言の外の文字は誤り（roxmltree と同じ）
        (format!("<?xml version=\"1.0\"?><!DOCTYPE svg [ü鞍 'あ' \"い\" <?pi う?>]><svg {SVG}/>"), false),
    ] {
        let r = parse(&src);
        assert_eq!(r.is_ok(), ok, "{src}: {:?}", r.map(|_| ()));
    }
}

/// 入力のすべての 3 バイトの並び（記号と英数の 15 字）で、どの段も止まらず落ちない（字句がどの枝でも進む）。
#[test]
fn all_three_byte_sequences() {
    let alphabet = b"<>/=\"'&;![]-?a ";
    for &x in alphabet {
        for &y in alphabet {
            for &z in alphabet {
                let o = xml_parse_with(&[x, y, z], &Limits::DEFAULT);
                assert_eq!(o.check, Check::Ok, "{:?}", [x, y, z]);
                let body = [b'<', b's', b'>', x, y, z, b'<', b'/', b's', b'>'];
                let o = xml_parse_with(&body, &Limits::DEFAULT);
                assert_eq!(o.check, Check::Ok, "{:?}", body);
            }
        }
    }
}
