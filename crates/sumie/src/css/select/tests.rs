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
        out.push(format!("{text}@{}", Spec::unpack(s.spec).pack()));
        sel = s.next;
    }
    out.join(", ")
}

fn parse_with(src: &str, ns: &[(&str, &str)]) -> Option<String> {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut t = Tables::new(&budget);
    let mut decls = Vec::new();
    for (p, u) in ns {
        let prefix = t.push_name(p.chars().map(|c| c as u32), false).unwrap();
        decls.push(NsDecl { prefix, ns: ns_of_uri(u.as_bytes()), pad: [0; 3] });
    }
    let s = src.as_bytes();
    let first = parse_selector_list(s, Span::new(0, s.len()), &decls, &mut t, &budget).unwrap()?;
    assert!(budget.max_spend() < crate::budget::CANCEL_INTERVAL);
    Some(show(&t, first))
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
    assert_eq!(parse(":nth-child(2n+1 of .a)").unwrap(), format!(":never@{}", sp(0, 1, 0)));
    for bad in [":nth-child(2 n)", ":nth-child(+ 2n)", ":nth-child(n++1)", ":nth-child()", ":nth-of-type(n of a)"] {
        assert_eq!(parse(bad), None, "{bad}");
    }
    assert_eq!(parse(":lang(EN, \"fr-CA\")").unwrap(), format!(":lang(en,fr-ca)@{}", sp(0, 1, 0)));
    assert_eq!(parse("a:link:hover").unwrap(), format!("a:link:never@{}", sp(0, 2, 1)));
    assert_eq!(parse("rect::before").unwrap(), format!("rect:never@{}", sp(0, 0, 2)));
    assert_eq!(parse("rect:has(> g)").unwrap(), format!("rect:never@{}", sp(0, 1, 1)));
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
