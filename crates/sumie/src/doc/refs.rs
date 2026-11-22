//! 参照の辺・輪・展開の数・条件（architecture.md §4.3・FR-CO-05・FR-ST-05・FR-IN-08）。
//!
//! - 辺は保守的に張る（輪を見落とさない側に倒す）：要素 → 子（暗に。節の鎖）、`href`（use・feImage・グラデーション・
//!   pattern・filter・textPath・tref）、受け継がない `url(#)`（clip-path・mask・filter）、受け継ぐ `url(#)`（fill・stroke・
//!   marker-start・marker-mid・marker-end。行きがけの順に最も近い祖先の指定を持ち、塗る要素と use・feImage から張る）
//! - 輪は明示の積み上げの深さ優先（3 色）で見つける。戻りの辺が参照の辺ならそれを、子の辺なら積み上げの上でいちばん近い
//!   参照の辺を取り除き（警告）、取り除いた辺の上で輪がなくなるまでくり返す
//! - 展開の数：輪のない辺の上で、帰りがけの順に「展開したあとの要素の数」を飽和の足し算で求める（use・pattern・marker・
//!   clip-path・mask・filter の参照は、参照ごとに中身を数える）
//! - 条件（`requiredExtensions`・`systemLanguage`。`requiredFeatures` は無視する）が偽の要素と、`switch` で選ばれない子は
//!   描かない（その部分木ごと）。`switch` は条件の通る最初の子の要素を選ぶ
//! - 外の文書への参照（`#` で始まらない `href`・`url()`。`data:` を除く）は読まずに、読まなかった参照に載せる

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::{Kind, Lexer, decode_string, ident_is};
use crate::css::sheet::Sheet;
use crate::doc::ids::Ids;
use crate::error::Error;
use crate::mem::{Frozen, MVec, Plain};
use crate::style::cascade::{SRC_SHEET, SRC_STORE, Styles, WIDE_INHERIT, WIDE_INITIAL, WIDE_NONE, WIDE_UNSET};
use crate::style::props::lookup;
use crate::xml::lex::Span;
use crate::xml::{self, KIND_ELEMENT, NONE, NS_NONE, NS_SVG, NS_XLINK, Node, XmlDoc};

/// 辺の種類。
/// 中身を作らない `href`（グラデーション・textPath・tref）
pub(crate) const EDGE_HREF: u8 = 0;
/// 中身を参照ごとに作る `href`（use・feImage・pattern・filter）
pub(crate) const EDGE_USE: u8 = 9;
pub(crate) const EDGE_CLIP: u8 = 1;
pub(crate) const EDGE_MASK: u8 = 2;
pub(crate) const EDGE_FILTER: u8 = 3;
pub(crate) const EDGE_FILL: u8 = 4;
pub(crate) const EDGE_STROKE: u8 = 5;
pub(crate) const EDGE_MARKER_START: u8 = 6;
pub(crate) const EDGE_MARKER_MID: u8 = 7;
pub(crate) const EDGE_MARKER_END: u8 = 8;

/// 参照の辺。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Edge {
    pub(crate) target: u32,
    pub(crate) kind: u8,
    /// 輪になるので取り除いた
    pub(crate) removed: u8,
    pub(crate) pad: [u8; 2],
}

impl Plain for Edge {}

/// 覚え（警告か読まなかった参照）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RefNote {
    pub(crate) pos: u32,
    /// 読まなかった参照なら href の範囲（`src` の置き場の中）
    pub(crate) href: Span,
    pub(crate) src: u8,
    pub(crate) unread: u8,
    pub(crate) pad: [u8; 2],
}

impl Plain for RefNote {}

/// 参照の表。
#[derive(Debug)]
#[allow(dead_code, reason = "展開の数と辺は estimate_build・木を作る段・needs から使う")]
pub(crate) struct Refs {
    /// 節ごとの辺の始まり（節の数 ＋ 1）
    pub(crate) start: Frozen<u32>,
    pub(crate) edges: Frozen<Edge>,
    /// 節ごとの印：描かない（条件が偽・switch で選ばれない。祖先からも受け継ぐ）
    pub(crate) hidden: Frozen<u8>,
    /// 根から展開したあとの要素の数（飽和する）
    pub(crate) instances: u64,
    /// 輪の警告と、読まなかった参照（位置の順）
    pub(crate) notes: Frozen<RefNote>,
}

impl Refs {
    pub(crate) fn bytes(&self) -> u64 {
        [self.start.bytes(), self.edges.bytes(), self.hidden.bytes(), self.notes.bytes()]
            .iter()
            .fold(0u64, |a, &b| a.saturating_add(b))
    }

    /// 節 `n` の辺。
    #[allow(dead_code, reason = "木を作る段と needs から使う")]
    pub(crate) fn of(&self, n: u32) -> &[Edge] {
        let s = self.start.as_slice();
        let a = s.get(n as usize).copied().unwrap_or(0) as usize;
        let b = s.get((n as usize).saturating_add(1)).copied().unwrap_or(0) as usize;
        self.edges.as_slice().get(a..b).unwrap_or(&[])
    }
}

/// `href` で中身を参照する要素。
fn href_element(local: &[u8]) -> bool {
    matches!(
        local,
        b"use" | b"feImage" | b"linearGradient" | b"radialGradient" | b"pattern" | b"filter" | b"textPath" | b"tref"
    )
}

/// 塗りの辺を張る要素（図形・文字。use・feImage は中身が use の文脈で受け継ぐので塗る要素とみなす）。
fn paints(local: &[u8]) -> bool {
    matches!(
        local,
        b"rect"
            | b"circle"
            | b"ellipse"
            | b"line"
            | b"polyline"
            | b"polygon"
            | b"path"
            | b"text"
            | b"tspan"
            | b"textPath"
            | b"tref"
            | b"use"
            | b"feImage"
    )
}

/// marker を付ける要素。
fn markable(local: &[u8]) -> bool {
    matches!(local, b"line" | b"polyline" | b"polygon" | b"path" | b"use" | b"feImage")
}

/// 受け継ぐ参照のプロパティ（順は INH_KINDS と同じ）。
const INH_PROPS: [&[u8]; 5] = [b"fill", b"stroke", b"marker-start", b"marker-mid", b"marker-end"];
const INH_KINDS: [u8; 5] = [EDGE_FILL, EDGE_STROKE, EDGE_MARKER_START, EDGE_MARKER_MID, EDGE_MARKER_END];

/// 節ごとの受け継ぐ参照（参照先の節。なければ NONE）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Inherited {
    t: [u32; 5],
}

impl Plain for Inherited {}

/// 読む口の共通の部分。
struct Cx<'a, 'b> {
    doc: &'a XmlDoc,
    input: &'a [u8],
    ids: &'a Ids,
    budget: &'b Budget<'b>,
    scratch: MVec<'b, u8>,
}

impl Cx<'_, '_> {
    /// 参照の文字列（`raw` は置き場の中の生の範囲、`escaped` なら CSS の escape を解く）を引く。`#id` なら参照先の節、
    /// 外の文書なら読まなかった参照に載せる（`data:` と空は何もしない）。
    fn resolve(
        &mut self,
        buf: &[u8],
        raw: Span,
        escaped: bool,
        src: u8,
        pos: u32,
        notes: &mut MVec<'_, RefNote>,
    ) -> Result<Option<u32>, Error> {
        self.scratch.clear();
        if escaped {
            for c in decode_string(buf, raw) {
                let (b, n) = crate::css::select::utf8_pub(c);
                self.scratch.append_slice(b.get(..n).unwrap_or(&[]))?;
            }
        } else {
            self.scratch.append_slice(raw.of(buf))?;
        }
        let v = trim_ascii_ws(self.scratch.as_slice());
        if let Some(id) = v.strip_prefix(b"#") {
            return self.ids.find(self.doc, self.input, id, self.budget);
        }
        if v.is_empty() || v.get(..5).is_some_and(|p| p.eq_ignore_ascii_case(b"data:")) {
            return Ok(None);
        }
        // 外の文書：読まない（範囲は置き場の中の生のもの。報告は解いた形にする）
        notes.push(RefNote { pos, href: raw, src, unread: 1, pad: [0; 2] })?;
        Ok(None)
    }

    /// 指定の値の中の url（url の字句と `url("…")`）ごとに `f` を呼ぶ（字句を払いながら読む）。
    fn urls(&self, buf: &[u8], value: Span, f: &mut dyn FnMut(Span, bool) -> Result<(), Error>) -> Result<(), Error> {
        let mut lex = Lexer::over(buf, value, self.budget);
        let mut in_url_fn = false;
        loop {
            let t = lex.next()?;
            match t.kind {
                Kind::Eof => return Ok(()),
                Kind::Url => f(t.inner, t.escaped)?,
                Kind::Function => in_url_fn = ident_is(buf, t.inner, t.escaped, b"url"),
                Kind::Str if in_url_fn => {
                    f(t.inner, t.escaped)?;
                    in_url_fn = false;
                }
                Kind::Whitespace => {}
                _ => in_url_fn = false,
            }
        }
    }
}

fn trim_ascii_ws(mut s: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = s
        && matches!(first, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
    {
        s = rest;
    }
    while let [rest @ .., last] = s
        && matches!(last, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
    {
        s = rest;
    }
    s
}

/// 参照の表を作る。`languages` は `systemLanguage` を照らす言語の並び。
pub(crate) fn build(
    doc: &XmlDoc,
    input: &[u8],
    sheet: &Sheet,
    styles: &Styles,
    ids: &Ids,
    languages: &[&str],
    budget: &Budget<'_>,
) -> Result<Refs, Error> {
    let budget = budget_ref(budget);
    let nodes = doc.nodes.as_slice();
    let n = nodes.len();
    let mut cx = Cx { doc, input, ids, budget, scratch: MVec::new(budget) };
    let mut notes: MVec<'_, RefNote> = MVec::new(budget);
    let mut start: MVec<'_, u32> = MVec::with_capacity(budget, n.saturating_add(1))?;
    let mut edges: MVec<'_, Edge> = MVec::new(budget);
    let mut inh: MVec<'_, Inherited> = MVec::with_capacity(budget, n)?;
    let mut hidden: MVec<'_, u8> = MVec::with_capacity(budget, n)?;
    // 受け継がない参照のプロパティの番号
    let clip = lookup(b"clip-path", false);
    let mask = lookup(b"mask", false);
    let filter = lookup(b"filter", false);
    let inh_ids: [Option<u16>; 5] = [
        lookup(INH_PROPS[0], false),
        lookup(INH_PROPS[1], false),
        lookup(INH_PROPS[2], false),
        lookup(INH_PROPS[3], false),
        lookup(INH_PROPS[4], false),
    ];
    // switch の子：選んだか（親ごとに 1 つ目の通る子）
    let mut switch_chosen: MVec<'_, u8> = MVec::with_capacity(budget, n)?;
    switch_chosen.resize_to(n, 0)?;
    for (i, node) in nodes.iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        start.push(edges.len() as u32)?;
        let parent = node.parent;
        let parent_inh = if parent == NONE {
            Inherited { t: [NONE; 5] }
        } else {
            inh.get(parent as usize).copied().unwrap_or(Inherited { t: [NONE; 5] })
        };
        let parent_hidden = if parent == NONE { 0 } else { hidden.get(parent as usize).copied().unwrap_or(0) };
        if node.kind != KIND_ELEMENT {
            inh.push(parent_inh)?;
            hidden.push(parent_hidden)?;
            continue;
        }
        let pos = node.pos;
        let svg = node.ns == NS_SVG;
        let local = node.local.of(input);
        // 条件と switch
        let mut hide = parent_hidden != 0 || (svg && !conditions_pass(doc, input, node, languages, budget)?);
        if parent != NONE
            && let Some(p) = nodes.get(parent as usize)
            && p.ns == NS_SVG
            && p.local.of(input) == b"switch"
            && svg
            && switch_candidate(local)
        {
            // 通る最初の子の要素だけを描く
            let chosen = switch_chosen.get(parent as usize).copied().unwrap_or(0) != 0;
            if !hide && !chosen {
                if let Some(c) = switch_chosen.get_mut(parent as usize) {
                    *c = 1;
                }
            } else {
                hide = true;
            }
        }
        hidden.push(u8::from(hide))?;
        if !svg {
            inh.push(parent_inh)?;
            continue;
        }
        // href
        if href_element(local) {
            let a = match doc.find_attr(input, node, NS_NONE, b"href", budget)? {
                Some(a) => Some(a),
                None => doc.find_attr(input, node, NS_XLINK, b"href", budget)?,
            };
            if let Some(a) = a {
                let (buf, span, src) = attr_buf(doc, input, a);
                if let Some(t) = cx.resolve(buf, span, false, src, pos, &mut notes)? {
                    let kind = if matches!(local, b"use" | b"feImage" | b"pattern" | b"filter") {
                        EDGE_USE
                    } else {
                        EDGE_HREF
                    };
                    edges.push(Edge { target: t, kind, removed: 0, pad: [0; 2] })?;
                }
            }
        }
        // 指定の値
        let specs = styles.of(i as u32);
        let mut own = parent_inh;
        for s in specs {
            budget.spend(cost::OP_STEP)?;
            let buf = value_buf(doc, input, sheet, s.src);
            if let Some(k) = inh_ids.iter().position(|&x| x == Some(s.prop)) {
                // 受け継ぐ参照：inherit・unset は親のまま、initial は なし、値なら最初の url を引く
                let target = match s.wide {
                    WIDE_INHERIT | WIDE_UNSET => continue,
                    WIDE_INITIAL => NONE,
                    _ => {
                        let mut found: Option<(Span, bool)> = None;
                        cx.urls(buf, s.value, &mut |sp, esc| {
                            if found.is_none() {
                                found = Some((sp, esc));
                            }
                            Ok(())
                        })?;
                        match found {
                            Some((sp, esc)) => cx.resolve(buf, sp, esc, s.src, pos, &mut notes)?.unwrap_or(NONE),
                            None => NONE,
                        }
                    }
                };
                if let Some(x) = own.t.get_mut(k) {
                    *x = target;
                }
                continue;
            }
            let kind = if Some(s.prop) == clip {
                EDGE_CLIP
            } else if Some(s.prop) == mask {
                EDGE_MASK
            } else if Some(s.prop) == filter {
                EDGE_FILTER
            } else {
                continue;
            };
            if s.wide != WIDE_NONE {
                continue;
            }
            let mut found: MVec<'_, Span> = MVec::new(budget);
            let mut escaped: MVec<'_, u8> = MVec::new(budget);
            cx.urls(buf, s.value, &mut |sp, esc| {
                found.push(sp)?;
                escaped.push(u8::from(esc))
            })?;
            for (k, &sp) in found.as_slice().iter().enumerate() {
                budget.spend(cost::OP_STEP)?;
                let esc = escaped.get(k).copied().unwrap_or(0) != 0;
                if let Some(t) = cx.resolve(buf, sp, esc, s.src, pos, &mut notes)? {
                    edges.push(Edge { target: t, kind, removed: 0, pad: [0; 2] })?;
                }
            }
        }
        inh.push(own)?;
        // 受け継いだ参照の辺（塗る要素・marker を付ける要素）
        if paints(local) {
            for k in 0..INH_KINDS.len() {
                let marker = k >= 2;
                if marker && !markable(local) {
                    continue;
                }
                let (Some(&t), Some(&kind)) = (own.t.get(k), INH_KINDS.get(k)) else { continue };
                if t != NONE {
                    edges.push(Edge { target: t, kind, removed: 0, pad: [0; 2] })?;
                }
            }
        }
    }
    start.push(edges.len() as u32)?;
    // 輪を取り除き、展開の数を求める
    let start = start.freeze();
    let mut edges = edges;
    remove_cycles(doc, start.as_slice(), &mut edges, &mut notes, budget)?;
    let instances = count_instances(doc, input, start.as_slice(), edges.as_slice(), budget)?;
    // 覚えは位置の順（足した順は節の順なので、位置の順になっている）
    let mut exact_notes: MVec<'_, RefNote> = MVec::with_capacity(budget, notes.len())?;
    exact_notes.append_slice(notes.as_slice())?;
    let mut exact_edges: MVec<'_, Edge> = MVec::with_capacity(budget, edges.len())?;
    exact_edges.append_slice(edges.as_slice())?;
    Ok(Refs { start, edges: exact_edges.freeze(), hidden: hidden.freeze(), instances, notes: exact_notes.freeze() })
}

/// 指定の値の置き場。
fn value_buf<'a>(doc: &'a XmlDoc, input: &'a [u8], sheet: &'a Sheet, src: u8) -> &'a [u8] {
    match src {
        SRC_SHEET => sheet.text.as_slice(),
        SRC_STORE => doc.store.as_slice(),
        _ => input,
    }
}

/// 属性の値の置き場・範囲・置き場の種類。
fn attr_buf<'a>(doc: &'a XmlDoc, input: &'a [u8], a: &xml::Attr) -> (&'a [u8], Span, u8) {
    if a.flags & xml::FLAG_IN_STORE != 0 {
        (doc.store.as_slice(), a.value, SRC_STORE)
    } else {
        (input, a.value, crate::style::cascade::SRC_INPUT)
    }
}

/// `switch` が選ぶ子（描く要素。`desc`・`title` などは選ばず、隠しもしない）。
fn switch_candidate(local: &[u8]) -> bool {
    matches!(
        local,
        b"a" | b"g"
            | b"svg"
            | b"use"
            | b"image"
            | b"text"
            | b"switch"
            | b"foreignObject"
            | b"rect"
            | b"circle"
            | b"ellipse"
            | b"line"
            | b"polyline"
            | b"polygon"
            | b"path"
    )
}

/// 条件の属性（`requiredExtensions`・`systemLanguage`）が通るか（FR-ST-05。`requiredFeatures` は無視する）。
fn conditions_pass(
    doc: &XmlDoc,
    input: &[u8],
    n: &Node,
    languages: &[&str],
    budget: &Budget<'_>,
) -> Result<bool, Error> {
    // 知っている拡張はないので、属性があれば偽（空も偽）
    if doc.find_attr(input, n, NS_NONE, b"requiredExtensions", budget)?.is_some() {
        return Ok(false);
    }
    let Some(v) = doc.attr(input, n, NS_NONE, b"systemLanguage", budget)? else { return Ok(true) };
    // `,` で区切った言語のどれかが、呼び手の言語のどれかと頭の部分（`-` の前）で同じ（大文字小文字を問わない）
    let mut meter = crate::xml::lex::ScanMeter::new(0, v.len());
    let mut a = 0usize;
    for i in 0..=v.len() {
        meter.pay(budget, i)?;
        if v.get(i).is_some_and(|&c| c != b',') {
            continue;
        }
        let tag = trim_ascii_ws(v.get(a..i).unwrap_or(&[]));
        a = i.saturating_add(1);
        let head = tag.split(|&c| c == b'-').next().unwrap_or(&[]);
        if head.is_empty() {
            continue;
        }
        for l in languages {
            budget.spend(cost::OP_STEP)?;
            let lh = l.as_bytes().split(|&c| c == b'-').next().unwrap_or(&[]);
            if crate::mem::bytes_eq_ignore_case(budget, head, lh)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// 深さ優先の積み上げの段。
#[repr(C)]
#[derive(Clone, Copy)]
struct Frame {
    node: u32,
    /// 次に見る子（なければ NONE）
    child: u32,
    /// 次に見る辺の位置（`start` の中の位置）
    edge: u32,
    /// この段に入った参照の辺（`edges` の番号。子の辺で入ったなら NONE）
    via: u32,
}

impl Plain for Frame {}

const WHITE: u8 = 0;
const GRAY: u8 = 1;
const BLACK: u8 = 2;

/// 輪を取り除く（取り除いた辺に印を付け、輪の警告を足す）。輪がなくなるまでくり返す。
fn remove_cycles(
    doc: &XmlDoc,
    start: &[u32],
    edges: &mut MVec<'_, Edge>,
    notes: &mut MVec<'_, RefNote>,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    let nodes = doc.nodes.as_slice();
    let n = nodes.len();
    if n == 0 {
        return Ok(());
    }
    let mut color: MVec<'_, u8> = MVec::with_capacity(budget, n)?;
    let mut stack: MVec<'_, Frame> = MVec::new(budget);
    loop {
        let mut removed_any = false;
        color.clear();
        color.resize_to(n, WHITE)?;
        stack.clear();
        // 根から（どの節も子の鎖で根から届く）
        let first_child = |i: u32| nodes.get(i as usize).map_or(NONE, |x| x.first_child);
        let edge_start = |i: u32| start.get(i as usize).copied().unwrap_or(0);
        if let Some(c) = color.get_mut(0) {
            *c = GRAY;
        }
        stack.push(Frame { node: 0, child: first_child(0), edge: edge_start(0), via: NONE })?;
        while let Some(top) = stack.last().copied() {
            budget.spend(cost::OP_STEP)?;
            // 次の行き先：子（要素だけ）、次に参照の辺
            let mut next: Option<(u32, u32)> = None;
            let mut frame = top;
            while frame.child != NONE {
                let c = frame.child;
                frame.child = nodes.get(c as usize).map_or(NONE, |x| x.next_sibling);
                if nodes.get(c as usize).is_some_and(|x| x.kind == KIND_ELEMENT) {
                    next = Some((c, NONE));
                    break;
                }
            }
            if next.is_none() {
                let end = edge_start(top.node.saturating_add(1));
                while frame.edge < end {
                    let k = frame.edge;
                    frame.edge = frame.edge.saturating_add(1);
                    if let Some(e) = edges.get(k as usize)
                        && e.removed == 0
                    {
                        next = Some((e.target, k));
                        break;
                    }
                }
            }
            if let Some(x) = stack.last_mut() {
                *x = frame;
            }
            let Some((target, via)) = next else {
                stack.pop();
                if let Some(c) = color.get_mut(top.node as usize) {
                    *c = BLACK;
                }
                continue;
            };
            match color.get(target as usize).copied() {
                Some(WHITE) => {
                    if let Some(c) = color.get_mut(target as usize) {
                        *c = GRAY;
                    }
                    stack.push(Frame { node: target, child: first_child(target), edge: edge_start(target), via })?;
                }
                Some(GRAY) => {
                    // 輪：戻りの辺が参照の辺ならそれを、子の辺なら積み上げの上でいちばん近い参照の辺を取り除く
                    let cut = if via != NONE {
                        Some(via)
                    } else {
                        let mut found = None;
                        for f in stack.as_slice().iter().rev() {
                            budget.spend(cost::OP_STEP)?;
                            // 行き先の段に入った辺は輪の外
                            if f.node == target {
                                break;
                            }
                            if f.via != NONE {
                                found = Some(f.via);
                                break;
                            }
                        }
                        found
                    };
                    if let Some(k) = cut
                        && let Some(e) = edges.get_mut(k as usize)
                        && e.removed == 0
                    {
                        e.removed = 1;
                        removed_any = true;
                        // 警告の位置は辺の元の要素
                        let src_node = source_of(start, k);
                        let pos = nodes.get(src_node as usize).map_or(0, |x| x.pos);
                        notes.push(RefNote { pos, href: Span::default(), src: 0, unread: 0, pad: [0; 2] })?;
                    }
                }
                _ => {}
            }
        }
        if !removed_any {
            return Ok(());
        }
    }
}

/// 辺 `k` の元の節（`start` は昇順なので二分探索）。
fn source_of(start: &[u32], k: u32) -> u32 {
    let i = crate::mem::lower_bound(start, |&s| s <= k);
    (i as u32).saturating_sub(1)
}

/// 展開したあとの要素の数（輪のない辺の上で、帰りがけの順に飽和の足し算。中身を作る参照は参照ごとに中身を数える）。
fn count_instances(
    doc: &XmlDoc,
    input: &[u8],
    start: &[u32],
    edges: &[Edge],
    budget: &Budget<'_>,
) -> Result<u64, Error> {
    let budget = budget_ref(budget);
    let nodes = doc.nodes.as_slice();
    let n = nodes.len();
    if n == 0 {
        return Ok(0);
    }
    // 数え終えた数（u64::MAX はまだ）と、積み上げの上にあるか（輪を取り除いたので来ないが、来ても回らない）
    let mut count: MVec<'_, u64> = MVec::with_capacity(budget, n)?;
    count.resize_to(n, u64::MAX)?;
    let mut on_stack: MVec<'_, u8> = MVec::with_capacity(budget, n)?;
    on_stack.resize_to(n, 0)?;
    let mut stack: MVec<'_, Frame> = MVec::new(budget);
    let mut acc: MVec<'_, u64> = MVec::new(budget);
    let first_child = |i: u32| nodes.get(i as usize).map_or(NONE, |x| x.first_child);
    let edge_start = |i: u32| start.get(i as usize).copied().unwrap_or(0);
    stack.push(Frame { node: 0, child: first_child(0), edge: edge_start(0), via: NONE })?;
    acc.push(1)?;
    if let Some(x) = on_stack.get_mut(0) {
        *x = 1;
    }
    while let Some(top) = stack.last().copied() {
        budget.spend(cost::OP_STEP)?;
        let mut next: Option<u32> = None;
        let mut frame = top;
        while frame.child != NONE {
            let c = frame.child;
            frame.child = nodes.get(c as usize).map_or(NONE, |x| x.next_sibling);
            if nodes.get(c as usize).is_some_and(|x| x.kind == KIND_ELEMENT) {
                next = Some(c);
                break;
            }
        }
        if next.is_none() {
            let end = edge_start(top.node.saturating_add(1));
            while frame.edge < end {
                let k = frame.edge;
                frame.edge = frame.edge.saturating_add(1);
                if let Some(e) = edges.get(k as usize)
                    && e.removed == 0
                    && instancing(e, nodes, input)
                {
                    next = Some(e.target);
                    break;
                }
            }
        }
        if let Some(x) = stack.last_mut() {
            *x = frame;
        }
        match next {
            Some(t) => {
                let done = count.get(t as usize).copied().unwrap_or(0);
                if done != u64::MAX {
                    if let Some(a) = acc.last_mut() {
                        *a = a.saturating_add(done);
                    }
                } else if on_stack.get(t as usize).copied() == Some(0) {
                    if let Some(x) = on_stack.get_mut(t as usize) {
                        *x = 1;
                    }
                    stack.push(Frame { node: t, child: first_child(t), edge: edge_start(t), via: NONE })?;
                    acc.push(1)?;
                }
            }
            None => {
                stack.pop();
                let mine = acc.pop().unwrap_or(1);
                if let Some(c) = count.get_mut(top.node as usize) {
                    *c = mine;
                }
                if let Some(x) = on_stack.get_mut(top.node as usize) {
                    *x = 0;
                }
                if let Some(a) = acc.last_mut() {
                    *a = a.saturating_add(mine);
                }
            }
        }
    }
    Ok(count.get(0).copied().unwrap_or(0))
}

/// 参照ごとに中身を作る辺か（中身を作らない href と、グラデーションへの塗り（stop だけで、描く要素を作らない）は数えない）。
fn instancing(e: &Edge, nodes: &[Node], input: &[u8]) -> bool {
    match e.kind {
        EDGE_HREF => false,
        EDGE_FILL | EDGE_STROKE => !nodes
            .get(e.target as usize)
            .is_some_and(|t| matches!(t.local.of(input), b"linearGradient" | b"radialGradient")),
        _ => true,
    }
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
