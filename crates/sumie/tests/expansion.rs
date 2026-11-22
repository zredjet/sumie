//! 展開の数（`estimate_build` が上限と比べる数）が、素朴に展開した要素の数を下回らないことを、種つきの乱数の文書で確かめる
//! （文書の段の 4 回目のレビュー：use の文脈の受け継ぎ・marker の context-fill・受け継がない参照の inherit の数え漏らしが
//! 回ごとに見つかったので、数え方の全体を素朴な展開と照らす）。
//!
//! 文書は、参照が前の定義だけを指す（輪のない）乱数の文書。素朴な展開は、要素を文脈（受け継いだ塗り・親の mask・marker の
//! context-fill）つきで、use の中身・塗りの pattern・mask・marker を参照ごとに展開して数える（SVG 2 の use の影の木と、
//! pattern・mask・marker の中身は参照ごとに作り、中身は自分の文書の祖先から受け継ぐ規則）。

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use sumie::{Error, Limits, Never, ParseOptions, Resource};

/// 種つきの xorshift。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DefKind {
    Pattern,
    Mask,
    Marker,
    Group,
}

#[derive(Clone, Copy, PartialEq)]
enum Fill {
    Unset,
    None,
    Url(usize),
    Context,
}

#[derive(Clone, Copy, PartialEq)]
enum MaskV {
    Unset,
    Url(usize),
    Inherit,
}

#[derive(Clone)]
enum Kind {
    G(Vec<Elem>),
    Rect,
    Path(Option<usize>),
    Use(usize),
}

#[derive(Clone)]
struct Elem {
    kind: Kind,
    fill: Fill,
    mask: MaskV,
}

struct Def {
    kind: DefKind,
    /// Group は g 自身（中身はその子）
    elem: Elem,
}

/// 定義 `below` より前のものだけを指す要素を作る。
fn gen_elem(r: &mut Rng, defs: &[Def], depth: u32, in_marker: bool) -> Elem {
    let pick = |r: &mut Rng, k: DefKind| -> Option<usize> {
        let cands: Vec<usize> = defs.iter().enumerate().filter(|(_, d)| d.kind == k).map(|(i, _)| i).collect();
        if cands.is_empty() { None } else { Some(cands[r.below(cands.len() as u64) as usize]) }
    };
    let fill = match r.below(5) {
        0 => pick(r, DefKind::Pattern).map_or(Fill::Unset, Fill::Url),
        1 => Fill::None,
        2 if in_marker => Fill::Context,
        _ => Fill::Unset,
    };
    let mask = match r.below(5) {
        0 => pick(r, DefKind::Mask).map_or(MaskV::Unset, MaskV::Url),
        1 => MaskV::Inherit,
        _ => MaskV::Unset,
    };
    let kind = match r.below(5) {
        0 | 1 if depth < 3 => {
            let n = 1 + r.below(3) as usize;
            Kind::G((0..n).map(|_| gen_elem(r, defs, depth + 1, in_marker)).collect())
        }
        2 => Kind::Path(pick(r, DefKind::Marker)),
        3 => pick(r, DefKind::Group).map_or(Kind::Rect, Kind::Use),
        _ => Kind::Rect,
    };
    Elem { kind, fill, mask }
}

fn write_elem(e: &Elem, out: &mut String, id: Option<usize>) {
    let mut attrs = String::new();
    if let Some(i) = id {
        attrs.push_str(&format!(" id=\"d{i}\""));
    }
    match e.fill {
        Fill::Unset => {}
        Fill::None => attrs.push_str(" fill=\"none\""),
        Fill::Url(p) => attrs.push_str(&format!(" fill=\"url(#d{p})\"")),
        Fill::Context => attrs.push_str(" fill=\"context-fill\""),
    }
    match e.mask {
        MaskV::Unset => {}
        MaskV::Url(m) => attrs.push_str(&format!(" mask=\"url(#d{m})\"")),
        MaskV::Inherit => attrs.push_str(" style=\"mask: inherit\""),
    }
    match &e.kind {
        Kind::G(ch) => {
            out.push_str(&format!("<g{attrs}>"));
            for c in ch {
                write_elem(c, out, None);
            }
            out.push_str("</g>");
        }
        Kind::Rect => out.push_str(&format!("<rect width=\"1\" height=\"1\"{attrs}/>")),
        Kind::Path(m) => {
            let mk = m.map_or(String::new(), |k| format!(" marker-end=\"url(#d{k})\""));
            out.push_str(&format!("<path d=\"M0 0L1 1\"{attrs}{mk}/>"));
        }
        Kind::Use(t) => out.push_str(&format!("<use href=\"#d{t}\"{attrs}/>")),
    }
}

/// 文脈：受け継いだ塗り（pattern の定義の番号）・親の計算した mask・marker の context-fill。
#[derive(Clone, Copy)]
struct Ctx {
    fill: Option<usize>,
    mask: Option<usize>,
    context: Option<usize>,
}

const NONE_CTX: Ctx = Ctx { fill: None, mask: None, context: None };

fn sat(a: u64, b: u64) -> u64 {
    a.saturating_add(b)
}

/// 定義の中身を 1 度作る数（定義の要素 1 つ ＋ 中身。中身は自分の文書の祖先から受け継ぐ：根は塗りも mask もない）。
fn expand_def(defs: &[Def], d: usize, context: Option<usize>) -> u64 {
    let ctx = Ctx { context, ..NONE_CTX };
    let mut n = 1u64;
    if let Kind::G(ch) = &defs[d].elem.kind {
        for c in ch {
            n = sat(n, count(defs, c, ctx));
        }
    }
    n
}

/// 要素を文脈 `ctx` で素朴に展開した要素の数。
fn count(defs: &[Def], e: &Elem, ctx: Ctx) -> u64 {
    let fill = match e.fill {
        Fill::Unset => ctx.fill,
        Fill::None => None,
        Fill::Url(p) => Some(p),
        Fill::Context => ctx.context,
    };
    let mask = match e.mask {
        MaskV::Unset => None,
        MaskV::Url(m) => Some(m),
        MaskV::Inherit => ctx.mask,
    };
    let mut n = 1u64;
    if let Some(m) = mask {
        n = sat(n, expand_def(defs, m, None));
    }
    let inner = Ctx { fill, mask, context: ctx.context };
    match &e.kind {
        Kind::G(ch) => {
            for c in ch {
                n = sat(n, count(defs, c, inner));
            }
        }
        Kind::Rect => {
            if let Some(p) = fill {
                n = sat(n, expand_def(defs, p, None));
            }
        }
        Kind::Path(marker) => {
            if let Some(p) = fill {
                n = sat(n, expand_def(defs, p, None));
            }
            // marker の中身の context-fill はこの要素の塗り
            if let Some(k) = marker {
                n = sat(n, expand_def(defs, *k, fill));
            }
        }
        // use の中身（g の定義の要素）は use の文脈で受け継ぐ
        Kind::Use(t) => n = sat(n, count(defs, &defs[*t].elem, inner)),
    }
    n
}

/// 定義の中身を書く（pattern・mask・marker は入れ物、Group は g そのもの）。
fn write_def(d: &Def, i: usize, out: &mut String) {
    let ch = match &d.elem.kind {
        Kind::G(ch) => ch.clone(),
        _ => Vec::new(),
    };
    let tag = match d.kind {
        DefKind::Pattern => "pattern",
        DefKind::Mask => "mask",
        DefKind::Marker => "marker",
        DefKind::Group => {
            write_elem(&d.elem, out, Some(i));
            return;
        }
    };
    out.push_str(&format!("<{tag} id=\"d{i}\">"));
    for c in &ch {
        write_elem(c, out, None);
    }
    out.push_str(&format!("</{tag}>"));
}

/// sumie の展開の数（上限を 0 にして estimate_build の Limit の要る量を読む）。
fn sumie_instances(svg: &str) -> u64 {
    let opts = ParseOptions { limits: Limits { instances: 0, ..Limits::DEFAULT }, ..ParseOptions::DEFAULT };
    let d = sumie::parse(svg.as_bytes(), &opts, &Never).unwrap();
    match d.estimate_build(&Never) {
        Err(Error::Limit(l)) if l.resource == Resource::Instances => l.needed,
        other => panic!("{other:?}"),
    }
}

#[test]
fn counted_instances_cover_naive_expansion() {
    let mut r = Rng(0x5eed_e4a1_9000_0001);
    let mut nontrivial = 0usize;
    for case in 0..3_000 {
        let mut defs: Vec<Def> = Vec::new();
        for _ in 0..(2 + r.below(6)) {
            let kind = match r.below(4) {
                0 => DefKind::Pattern,
                1 => DefKind::Mask,
                2 => DefKind::Marker,
                _ => DefKind::Group,
            };
            let in_marker = kind == DefKind::Marker;
            let n = 1 + r.below(3) as usize;
            let ch: Vec<Elem> = (0..n).map(|_| gen_elem(&mut r, &defs, 1, in_marker)).collect();
            let mut elem = Elem { kind: Kind::G(ch), fill: Fill::Unset, mask: MaskV::Unset };
            if kind == DefKind::Group {
                // use で使う g は、自分の塗りと mask も持てる
                let e = gen_elem(&mut r, &defs, 3, false);
                elem.fill = e.fill;
                elem.mask = e.mask;
            }
            defs.push(Def { kind, elem });
        }
        let top: Vec<Elem> = (0..(1 + r.below(4))).map(|_| gen_elem(&mut r, &defs, 0, false)).collect();
        let mut svg = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><defs>");
        for (i, d) in defs.iter().enumerate() {
            write_def(d, i, &mut svg);
        }
        svg.push_str("</defs>");
        for e in &top {
            write_elem(e, &mut svg, None);
        }
        svg.push_str("</svg>");
        // 素朴な展開：根・defs・描く木の要素（定義は参照されたときに数える）
        let mut naive = 2u64;
        for e in &top {
            naive = sat(naive, count(&defs, e, NONE_CTX));
        }
        let counted = sumie_instances(&svg);
        if naive > 50 {
            nontrivial += 1;
        }
        assert!(counted >= naive, "{case} 件目：数えた {counted} ＜ 素朴な展開 {naive}\n{svg}");
    }
    assert!(nontrivial > 100, "{nontrivial}");
}
