#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;

/// 表から、読んだセレクタを決まった形の文字列に戻す。
fn show(t: &Tables<'_>, first: u32) -> String {
    let names = t.names.as_slice();
    let name = |sp: Span| String::from_utf8_lossy(sp.of(names)).to_string();
    let ns = |n: u8| match n {
        NS_ANY => String::new(),
        x => format!("{x}|"),
    };
    let mut out = Vec::new();
    let mut sel = first;
    while sel != NONE {
        let s = t.selectors.as_slice()[sel as usize];
        let mut comps = Vec::new();
        let mut c = s.subject;
        while c != NONE {
            comps.push(c);
            c = t.compounds.as_slice()[c as usize].prev;
        }
        comps.reverse();
        let mut text = String::new();
        for (k, &c) in comps.iter().enumerate() {
            let comp = t.compounds.as_slice()[c as usize];
            if k > 0 {
                text.push_str(match comp.comb {
                    COMB_DESC => " ",
                    COMB_CHILD => " > ",
                    COMB_NEXT => " + ",
                    COMB_LATER => " ~ ",
                    _ => " ? ",
                });
            }
            if comp.ns != NS_ANY {
                text.push_str(&format!("{{{}}}", comp.ns));
            }
            let mut sp = comp.first;
            while sp != NONE {
                let x = t.simples.as_slice()[sp as usize];
                text.push_str(&match x.kind {
                    S_TYPE => format!("{}{}", ns(x.ns), name(x.a)),
                    S_UNIVERSAL => format!("{}*", ns(x.ns)),
                    S_ID => format!("#{}", name(x.a)),
                    S_CLASS => format!(".{}", name(x.a)),
                    S_ATTR => {
                        let op = ["", "=", "~=", "|=", "^=", "$=", "*="][x.op as usize];
                        let v = if x.op == OP_EXISTS { String::new() } else { format!("\"{}\"", name(x.b)) };
                        let i = if x.flags & FLAG_CASE_I != 0 { " i" } else { "" };
                        format!("[{}{}{op}{v}{i}]", ns(x.ns), name(x.a))
                    }
                    S_NTH_CHILD => format!(":nth-child({}n{:+})", x.n_a, x.n_b),
                    S_NTH_LAST_OF_TYPE => format!(":nth-last-of-type({}n{:+})", x.n_a, x.n_b),
                    S_FIRST_CHILD => ":first-child".into(),
                    S_LAST_CHILD => ":last-child".into(),
                    S_ROOT => ":root".into(),
                    S_LINK => ":link".into(),
                    S_LANG => format!(":lang({})", name(x.a)),
                    S_NOT => format!(":not({})", show(t, x.list)),
                    S_IS => format!(":is({})", show(t, x.list)),
                    S_NEVER => ":never".into(),
                    k => format!(":k{k}"),
                });
                sp = x.next;
            }
        }
        out.push(format!("{text}@{}", s.spec));
        sel = s.next;
    }
    out.join(", ")
}

fn parse_full(src: &str, ns: &[(&str, &str)]) -> Option<(String, bool)> {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut t = Tables::new(&budget);
    let mut names: MVec<'_, u8> = MVec::new(&budget);
    let mut decls: MVec<'_, NsDecl> = MVec::new(&budget);
    // 接頭辞の順に、同じ接頭辞は後の宣言（規則の並びが並べるのと同じ形）
    let mut last: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    for (p, u) in ns {
        last.insert(p, u);
    }
    for (p, u) in last {
        let start = names.len();
        names.append_slice(p.as_bytes()).unwrap();
        decls.push(NsDecl { prefix: Span::new(start, names.len()), ns: ns_of_uri(u.as_bytes()), pad: [0; 3] }).unwrap();
    }
    let table = NsTable { decls: decls.as_slice(), names: names.as_slice() };
    let s = src.as_bytes();
    let p = parse_selector_list(s, Span::new(0, s.len()), table, &mut t, &budget).unwrap()?;
    assert!(budget.max_spend() < crate::budget::CANCEL_INTERVAL);
    Some((if p.first == NONE { String::new() } else { show(&t, p.first) }, p.unsupported))
}

fn parse_with(src: &str, ns: &[(&str, &str)]) -> Option<String> {
    parse_full(src, ns).map(|x| x.0)
}

fn parse(src: &str) -> Option<String> {
    parse_with(src, &[])
}

/// 詳細度の詰めた値。
fn sp(a: u32, b: u32, c: u32) -> u32 {
    (a << 20) | (b << 10) | c
}

#[test]
fn simple_selectors_and_combinators() {
    assert_eq!(parse("rect").unwrap(), format!("rect@{}", sp(0, 0, 1)));
    assert_eq!(parse(" g  >rect.a#b , *").unwrap(), format!("g > rect.a#b@{}, *@0", sp(1, 1, 2)));
    assert_eq!(parse("a b+c~d").unwrap(), format!("a b + c ~ d@{}", sp(0, 0, 4)));
    assert_eq!(
        parse("[x] [y=\"v\"][z~=w i][q|=a][p^=b][s$='c'][u*=d s]").unwrap(),
        format!("[0|x] [0|y=\"v\"][0|z~=\"w\" i][0|q|=\"a\"][0|p^=\"b\"][0|s$=\"c\"][0|u*=\"d\"]@{}", sp(0, 7, 0))
    );
    assert_eq!(
        parse("rect:first-child:last-child:root").unwrap(),
        format!("rect:first-child:last-child:root@{}", sp(0, 3, 1))
    );
    assert_eq!(parse(".\\61 b").unwrap(), format!(".ab@{}", sp(0, 1, 0)));
}

#[test]
fn nested_lists_and_specificity() {
    // :is は中のいちばん高い詳細度、:where は 0、:not は中のいちばん高い詳細度
    assert_eq!(
        parse("rect:is(.a, #b)").unwrap(),
        format!("rect:is(.a@{}, #b@{})@{}", sp(0, 1, 0), sp(1, 0, 0), sp(1, 0, 1))
    );
    assert_eq!(parse("rect:where(#b)").unwrap(), format!("rect:is(#b@{})@{}", sp(1, 0, 0), sp(0, 0, 1)));
    assert_eq!(
        parse(":not(g rect, .x)").unwrap(),
        format!(":not(g rect@{}, .x@{})@{}", sp(0, 0, 2), sp(0, 1, 0), sp(0, 1, 0))
    );
    assert_eq!(parse("a:is(b:not(c))").unwrap(), "a:is(b:not(c@1)@2)@3");
    // :is は寛容（壊れたものだけ捨てる）、:not は寛容でない
    assert_eq!(parse("rect:is(%, .a)").unwrap(), format!("rect:is(.a@{})@{}", sp(0, 1, 0), sp(0, 1, 1)));
    assert_eq!(parse("rect:is(%)").unwrap(), "rect:never@1");
    assert_eq!(parse("rect:not(%, .a)"), None);
    assert_eq!(parse("rect:is(a:not(%), b)").unwrap(), "rect:is(b@1)@2");
    // 閉じていない関数は終わりで閉じる
    assert_eq!(parse("rect:is(.a").unwrap(), format!("rect:is(.a@{})@{}", sp(0, 1, 0), sp(0, 1, 1)));
}

#[test]
fn pseudo_classes_and_an_b() {
    assert_eq!(parse(":nth-child(2n+1)").unwrap(), format!(":nth-child(2n+1)@{}", sp(0, 1, 0)));
    assert_eq!(parse(":nth-child( odd )").unwrap(), format!(":nth-child(2n+1)@{}", sp(0, 1, 0)));
    assert_eq!(parse(":nth-child(even)").unwrap(), format!(":nth-child(2n+0)@{}", sp(0, 1, 0)));
    assert_eq!(parse(":nth-child(-n + 3)").unwrap(), format!(":nth-child(-1n+3)@{}", sp(0, 1, 0)));
    assert_eq!(parse(":nth-child(5)").unwrap(), format!(":nth-child(0n+5)@{}", sp(0, 1, 0)));
    assert_eq!(parse(":nth-last-of-type(n- 2)").unwrap(), format!(":nth-last-of-type(1n-2)@{}", sp(0, 1, 0)));
    // `of S` は確かめて読むが当てず、選択子ごと捨てる
    assert_eq!(parse_full(":nth-child(2n+1 of .a)", &[]), Some((String::new(), true)));
    for bad in [":nth-child(2 n)", ":nth-child(+ 2n)", ":nth-child(n++1)", ":nth-child()", ":nth-of-type(n of a)"] {
        assert_eq!(parse(bad), None, "{bad}");
    }
    assert_eq!(parse(":lang(EN, \"fr-CA\")").unwrap(), format!(":lang(en,fr-ca)@{}", sp(0, 1, 0)));
    assert_eq!(parse("a:link:hover").unwrap(), format!("a:link:never@{}", sp(0, 2, 1)));
    assert_eq!(parse("rect::before").unwrap(), format!("rect:never@{}", sp(0, 0, 2)));
    assert_eq!(parse_full("rect:has(> g), g", &[]), Some((format!("g@{}", sp(0, 0, 1)), true)));
    for bad in [":bogus", "rect::before.a", ":first-child()", "::", ":"] {
        assert_eq!(parse(bad), None, "{bad}");
    }
}

#[test]
fn broken_selectors() {
    for bad in [
        "",
        " ",
        "a,",
        ",a",
        "a >",
        "> a",
        "a > > b",
        "#1",
        ". a",
        "rect.",
        "a)",
        "[x",
        "[x=]",
        "[x y]",
        "[x=a b]",
        "a|b",
        "rect.a, %",
    ] {
        assert_eq!(parse(bad), None, "{bad:?}");
    }
}

#[test]
fn namespaces() {
    let ns = [("svg", "http://www.w3.org/2000/svg"), ("x", "http://www.w3.org/1999/xlink")];
    assert_eq!(parse_with("svg|rect", &ns).unwrap(), format!("{NS_SVG}|rect@1"));
    assert_eq!(parse_with("*|rect", &ns).unwrap(), "rect@1");
    assert_eq!(parse_with("|rect", &ns).unwrap(), format!("{NS_NONE}|rect@1"));
    assert_eq!(parse_with("[x|href]", &ns).unwrap(), format!("[{NS_XLINK}|href]@{}", sp(0, 1, 0)));
    assert_eq!(parse_with("[*|href]", &ns).unwrap(), format!("[href]@{}", sp(0, 1, 0)));
    assert_eq!(parse_with("[x|=a]", &ns).unwrap(), format!("[{NS_NONE}|x|=\"a\"]@{}", sp(0, 1, 0)));
    // 接頭辞のない属性は名前空間なし
    assert_eq!(parse_with("[href]", &ns).unwrap(), format!("[{NS_NONE}|href]@{}", sp(0, 1, 0)));
    // 知らない接頭辞は壊れている
    assert_eq!(parse_with("q|rect", &ns), None);
    // 既定の名前空間は型と * に当てる
    let d = [("", "http://www.w3.org/2000/svg")];
    assert_eq!(parse_with("rect *", &d).unwrap(), format!("{NS_SVG}|rect {NS_SVG}|*@1"));
}

fn valid(sel: &str) -> bool {
    parse_full(&format!("{sel}, #r"), &[]).is_some()
}

/// 擬似クラス・擬似要素の名前と、擬似要素のあとに書けるもの（ADR-73。仕様と Chrome 157 で照らした）。
#[test]
fn pseudo_names_and_pseudo_elements() {
    for ok in [
        ":open",
        ":-webkit-autofill",
        ":active-view-transition",
        ":dir(foo)",
        ":dir( LTR )",
        ":popover-open",
        ":state(x)",
        ":-webkit-any(rect)",
        ":-webkit-any-link",
        ":host(rect)",
        ":playing",
        ":is(::before)",
        ":where(:not(::before))",
        "::marker",
        "::part(x)",
        "::-webkit-foo",
        "::Before",
        ":Before",
        "::before::marker",
        "::part(x)::before",
        "::part(x):hover",
        "::-webkit-foo:hover",
        "::before:hover",
        ":lang(\\*-CH)",
    ] {
        assert!(valid(ok), "{ok}");
    }
    for bad in [
        ":target-within",
        ":matches(rect)",
        ":not()",
        ":dir()",
        ":dir(ltr, rtl)",
        "::foo",
        "::-moz-selection",
        "::after:not(.a)",
        "rect::before.a",
        "::before rect",
        "::before:empty",
        "::-webkit-foo::before",
        "::marker:focus-within::before",
        ":not(::before)",
        ":not(:before)",
        "rect:not(::foo)",
    ] {
        assert!(!valid(bad), "{bad}");
    }
    assert_eq!(parse(":dir(ltr)").unwrap(), format!("*@{}", sp(0, 1, 0)));
    assert_eq!(parse(":dir(rtl)").unwrap(), format!(":never@{}", sp(0, 1, 0)));
}

/// `:has()` と `of S` は中まで確かめて読み、それを含むいちばん外の選択子を捨てる（`:not()` で裏返さない）。
#[test]
fn has_and_of_drop_the_selector() {
    // `:is()` は寛容なので、中の壊れた `:has()` は捨てるだけ
    for ok in [
        ":has(> rect)",
        ":has(+ a, ~ b c)",
        ":nth-child(1 of.a)",
        ":nth-last-child(1 of a, b)",
        ":nth-child(1 of a b)",
        ":is(:has(:has(a)))",
    ] {
        assert!(valid(ok), "{ok}");
    }
    for bad in [
        ":has()",
        ":has(:has(a))",
        ":has(:not(:has(a)))",
        ":has(::before)",
        ":has(> > a)",
        ":nth-child(1 of !!)",
        ":nth-child(1 of)",
        ":nth-child(1 of ::before)",
        ":nth-of-type(1 of a)",
        ":nth-child(2nof .a)",
    ] {
        assert!(!valid(bad), "{bad}");
    }
    assert_eq!(parse_full("g:not(:has(rect))", &[]), Some((String::new(), true)));
    assert_eq!(parse_full(":is(:has(a), .b)", &[]), Some((String::new(), true)));
    assert_eq!(parse_full("rect, g:has(a)", &[]), Some((format!("rect@{}", sp(0, 0, 1)), true)));
}

/// An+B は字句の並びで読む（CSS Syntax §6.2。注釈は字句を分けるだけ）。
#[test]
fn an_b_by_tokens() {
    for (src, a, b) in [
        ("2n/**/+1", 2, 1),
        ("+n", 1, 0),
        ("-n- 1", -1, -1),
        ("n- 1", 1, -1),
        ("2n- 1", 2, -1),
        ("2n -1", 2, -1),
        ("2n - 1", 2, -1),
        ("2N+1", 2, 1),
        ("2\\6e+1", 2, 1),
        ("EVEN", 2, 0),
        ("+5", 0, 5),
        (" -n+3 ", -1, 3),
        ("-n-3", -1, -3),
        ("n-3", 1, -3),
        ("+n-3", 1, -3),
        ("-2n+1", -2, 1),
        ("99999999999n", i32::MAX, 0),
    ] {
        assert_eq!(
            parse(&format!(":nth-child({src})")).unwrap(),
            format!(":nth-child({a}n{b:+})@{}", sp(0, 1, 0)),
            "{src}"
        );
    }
    for bad in ["+ n", "- n", "2n + -1", "1.0", "1e1", "2n+", "n-", "--n", "2 n", "n++1", "+-n", "3n 4", "n-a"] {
        assert_eq!(parse(&format!(":nth-child({bad})")), None, "{bad}");
    }
    // 長い数字の名前も払いながら読む（parse_full が 1 回の払いの上限を確かめる）
    let long = format!(":nth-child(n-{})", "1".repeat(1 << 20));
    assert_eq!(parse(&long).unwrap(), format!(":nth-child(1n{:+})@{}", -i32::MAX, sp(0, 1, 0)));
}

/// 既定の名前空間は型のない複合にも効く（`:is()` などの中の右端の複合を除く。Chrome と同じ）。
#[test]
fn default_namespace_on_compounds() {
    let d = [("", "urn:x")];
    let o = NS_OTHER;
    assert_eq!(parse_with(".a + rect", &d).unwrap(), format!("{{{o}}}.a + {o}|rect@{}", sp(0, 1, 1)));
    assert_eq!(parse_with(":is(#r)", &d).unwrap(), format!("{{{o}}}:is(#r@{})@{}", sp(1, 0, 0), sp(1, 0, 0)));
    assert_eq!(
        parse_with("*|*:is(.a > *|*)", &d).unwrap(),
        format!("*:is({{{o}}}.a > *@{})@{}", sp(0, 1, 0), sp(0, 1, 0))
    );
}

/// 接頭辞は並べた表で引き、後の宣言が勝つ（escape も解いて比べる）。
#[test]
fn namespace_table_lookup() {
    let ns = [("b", "http://www.w3.org/2000/svg"), ("a", "urn:a"), ("b", "http://www.w3.org/1999/xlink")];
    assert_eq!(parse_with("b|rect", &ns).unwrap(), format!("{NS_XLINK}|rect@1"));
    assert_eq!(parse_with("\\62|rect", &ns).unwrap(), format!("{NS_XLINK}|rect@1"));
    assert_eq!(parse_with("a|rect", &ns).unwrap(), format!("{NS_OTHER}|rect@1"));
    assert_eq!(parse_with("c|rect", &ns), None);
}

/// 壊れた選択子の中で開いたかっこは組の閉じまで読んでから、寛容な並びの次の選択子へ進む（Chrome と同じ）。
#[test]
fn forgiving_recovery_skips_open_brackets() {
    for src in [":is(x(y), rect)", ":is((a), rect)", ":is([a=b(c)], rect)", ":is(:not(x(y)), rect)", ":is(:(a), rect)"]
    {
        assert_eq!(parse(src).unwrap(), ":is(rect@1)@1", "{src}");
    }
}
