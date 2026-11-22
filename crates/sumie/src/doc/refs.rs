//! 参照の辺・輪・展開の数・条件（architecture.md §4.3・FR-CO-05・FR-ST-05・FR-IN-08）。
//!
//! - 辺は保守的に張る（輪を見落とさない側に倒す）：要素 → 子（暗に。節の鎖）、`href`（use・feImage・グラデーション・
//!   pattern・filter・textPath・tref）、受け継がない `url(#)`（clip-path・mask・filter）、受け継ぐ `url(#)`（fill・stroke・
//!   marker-start・marker-mid・marker-end。行きがけの順に最も近い祖先の指定を持ち、塗る要素と use・feImage から張る）
//! - 受け継がない参照は値ごとの並びの節（XML の節の数から後ろ）で共有する（ADR-81 (3)）。textPath・tref の `href` は
//!   文字の辺で、輪にも展開にも入れない
//! - 輪は強連結成分（Tarjan。明示の積み上げ）で見つけ、輪のある成分の中の、文書の順に後ろ向きの参照の辺と並びの節に入る辺を
//!   取り除く（警告。1 度の歩き。取り除く辺はどれももとの図の輪の上にある。ADR-82 (2)）
//! - 展開の数：輪のない辺の上で、帰りがけの順に「展開したあとの要素の数」を飽和の足し算で求める（use・pattern・marker・
//!   clip-path・mask・filter の参照は、参照ごとに中身を数える。use の中身と marker の中の塗る要素は、塗りの参照先を
//!   その数の倍で数える。ADR-81 (1)・ADR-82 (1)）。子を描かない入れ物（defs・symbol・pattern・marker・
//!   clipPath・mask・グラデーション・filter など）の中身は、参照の辺で入ったときだけ数える（defs の中の use の鎖を、
//!   描かないのに n² に数えない）
//! - 条件（`requiredExtensions`・`systemLanguage`。`requiredFeatures` は無視する）が偽の要素と、`switch` で選ばれない子は
//!   描かない（その部分木ごと）。`switch` は条件の通る最初の子の要素を選ぶ
//! - 外の文書への参照（`#` で始まらない `href`・`url()`。`data:` を除く）は読まずに、読まなかった参照に載せる

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::{Kind, Lexer, decode_string, ident_is};
use crate::css::sheet::Sheet;
use crate::doc::ids::Ids;
use crate::error::Error;
use crate::mem::{Frozen, MVec, Plain, stable_sort};
use crate::style::cascade::{SRC_SHEET, SRC_STORE, Styles, WIDE_INHERIT, WIDE_INITIAL, WIDE_NONE, WIDE_UNSET};
use crate::style::props::lookup;
use crate::xml::lex::Span;
use crate::xml::{self, KIND_ELEMENT, NONE, NS_NONE, NS_SVG, NS_XLINK, Node, XmlDoc};

/// 自分の条件（`requiredExtensions`・`systemLanguage`）が偽。
pub(crate) const HIDDEN_SELF: u8 = 1;
/// 親の `switch` が選ばない（参照の辺で入れば描く。Chrome・usvg と同じ）。
pub(crate) const HIDDEN_SWITCH: u8 = 2;

/// 辺の種類。
/// 中身を作らない `href`（グラデーション・textPath・tref）
pub(crate) const EDGE_HREF: u8 = 0;
/// 中身を参照ごとに作る `href`（use・feImage・pattern・filter）
pub(crate) const EDGE_USE: u8 = 9;
/// 文字の `href`（textPath の道・tref の文字。描き直さないので輪にも展開にも入れない。文書の 2 回目のレビュー）
pub(crate) const EDGE_TEXT: u8 = 10;
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
    /// 読まなかった参照なら href の範囲（`src` の置き場の中。`escaped` なら CSS の escape を解いて報告する）
    pub(crate) href: Span,
    pub(crate) src: u8,
    pub(crate) unread: u8,
    pub(crate) escaped: u8,
    pub(crate) pad: u8,
}

impl Plain for RefNote {}

/// 参照の表。
#[derive(Debug)]
#[allow(dead_code, reason = "展開の数と辺は estimate_build・木を作る段・needs から使う")]
pub(crate) struct Refs {
    /// 節ごとの辺の始まり（節の数 ＋ 1）
    pub(crate) start: Frozen<u32>,
    pub(crate) edges: Frozen<Edge>,
    /// 節ごとの、自分の印（祖先からは受け継がない。たどる側が道に沿って受け継ぐ）：`HIDDEN_SELF`（自分の条件が偽。
    /// どの文脈でも描かない）・`HIDDEN_SWITCH`（親の switch が選ばない。参照の辺で入ったときは効かない。文書の 1 回目の
    /// レビュー）
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

/// 受け継がない参照（clip-path・mask・filter）の並びの節（値ごとに 1 つ。節の番号は XML の節の数 ＋ 並びの番号）。
/// 要素は並びの節へ 1 本の辺を張り、並びの節が参照先へ辺を張る（`inherit` は親の並びを指し、規則の値は当たる要素で
/// 共有する。辺の数を要素の数と値の長さの和に比例させる。文書の 2 回目のレビュー）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ListRec {
    /// 並びの辺（`ledges` の中の始まりと数）
    start: u32,
    len: u32,
    /// 最初に使った要素の位置（輪の警告の位置）
    pos: u32,
    resolved: u8,
    pad: [u8; 3],
}

impl Plain for ListRec {}

/// 節ごとの、受け継がない参照の並び（clip-path・mask・filter の順。なければ NONE）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Lists {
    t: [u32; 3],
}

impl Plain for Lists {}

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
            for c in decode_string(buf, raw, self.budget) {
                let (b, n) = crate::css::select::utf8_pub(c?);
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
        notes.push(RefNote { pos, href: raw, src, unread: 1, escaped: u8::from(escaped), pad: 0 })?;
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
    // 規則の並びの値（置き場の位置）を集めて並べる：規則の値の並びの節の番号は、ここでの位置
    let mut sheet_keys: MVec<'_, u32> = MVec::new(budget);
    for i in 0..n {
        // 指定のない節（文字の節など）も 1 つずつ払う（文書の 3 回目のレビュー）
        budget.spend(cost::OP_STEP)?;
        for sp in styles.of(i as u32) {
            budget.spend(cost::OP_STEP)?;
            let p = Some(sp.prop);
            if sp.src == SRC_SHEET && sp.wide == WIDE_NONE && (p == clip || p == mask || p == filter) {
                sheet_keys.push(sp.value.start)?;
            }
        }
    }
    stable_sort(budget, sheet_keys.as_mut_slice(), |a, b| a < b)?;
    let mut keep = 0usize;
    for k in 0..sheet_keys.len() {
        budget.spend(cost::OP_STEP)?;
        let v = sheet_keys.get(k).copied().unwrap_or(0);
        if keep > 0 && sheet_keys.get(keep.saturating_sub(1)).copied() == Some(v) {
            continue;
        }
        if let Some(x) = sheet_keys.get_mut(keep) {
            *x = v;
        }
        keep = keep.saturating_add(1);
    }
    sheet_keys.truncate(keep);
    let mut lists: MVec<'_, ListRec> = MVec::with_capacity(budget, keep)?;
    lists.resize_to(keep, ListRec { start: 0, len: 0, pos: 0, resolved: 0, pad: [0; 3] })?;
    let mut ledges: MVec<'_, Edge> = MVec::new(budget);
    let mut own_lists: MVec<'_, Lists> = MVec::with_capacity(budget, n)?;
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
        if node.kind != KIND_ELEMENT {
            own_lists.push(Lists { t: [NONE; 3] })?;
            inh.push(parent_inh)?;
            hidden.push(0)?;
            continue;
        }
        let pos = node.pos;
        let svg = node.ns == NS_SVG;
        let local = node.local.of(input);
        // 条件と switch
        // 条件と switch（自分の印。switch の選び方は switch が描かれるかに依らない：use から参照されても同じ子を選ぶ）
        let own_false = svg && !conditions_pass(doc, input, node, languages, budget)?;
        let mut bits = if own_false { HIDDEN_SELF } else { 0 };
        if parent != NONE
            && let Some(p) = nodes.get(parent as usize)
            && p.ns == NS_SVG
            && p.local.of(input) == b"switch"
            && svg
            && switch_candidate(local)
            && !own_false
        {
            // 通る最初の子の要素だけを描く
            if switch_chosen.get(parent as usize).copied().unwrap_or(0) == 0 {
                if let Some(c) = switch_chosen.get_mut(parent as usize) {
                    *c = 1;
                }
            } else {
                bits |= HIDDEN_SWITCH;
            }
        }
        hidden.push(bits)?;
        if !svg {
            own_lists.push(Lists { t: [NONE; 3] })?;
            inh.push(parent_inh)?;
            continue;
        }
        // href
        if href_element(local) {
            let a = match doc.find_attr(input, node, NS_NONE, b"href", budget)? {
                Some(a) => Some(a),
                None => doc.find_attr(input, node, NS_XLINK, b"href", budget)?,
            };
            // feImage の外の画像は、<image> と同じに読まなかった参照でなく要るもの（呼び手の画像のデコーダ）にする
            // （文書の 1 回目のレビュー）
            let a = match a {
                Some(a)
                    if local == b"feImage"
                        && !starts_with_hash(attr_buf(doc, input, a).1.of(attr_buf(doc, input, a).0), budget)? =>
                {
                    None
                }
                x => x,
            };
            if let Some(a) = a {
                let (buf, span, src) = attr_buf(doc, input, a);
                if let Some(t) = cx.resolve(buf, span, false, src, pos, &mut notes)?
                    && target_kind_ok(nodes, input, t, href_targets(local))
                {
                    let kind = if matches!(local, b"use" | b"feImage" | b"pattern" | b"filter") {
                        EDGE_USE
                    } else if matches!(local, b"textPath" | b"tref") {
                        EDGE_TEXT
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
        let mut mine = Lists { t: [NONE; 3] };
        let parent_lists = if parent == NONE {
            Lists { t: [NONE; 3] }
        } else {
            own_lists.get(parent as usize).copied().unwrap_or(Lists { t: [NONE; 3] })
        };
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
                        let want: &[&[u8]] = if k < 2 { PAINT_SERVERS } else { &[b"marker"] };
                        match found {
                            Some((sp, esc)) => cx
                                .resolve(buf, sp, esc, s.src, pos, &mut notes)?
                                .filter(|&t| target_kind_ok(nodes, input, t, want))
                                .unwrap_or(NONE),
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
            let slot = match kind {
                EDGE_CLIP => 0usize,
                EDGE_MASK => 1,
                _ => 2,
            };
            let list = match s.wide {
                // 受け継がない参照の `inherit` は親の計算した値：親の並びを指す（輪を見落とさず、展開を数え漏らさない）
                WIDE_INHERIT => parent_lists.t.get(slot).copied().unwrap_or(NONE),
                WIDE_NONE => {
                    // 規則の値は当たる要素で共有する。属性と style 属性の値は要素ごと
                    let id = if s.src == SRC_SHEET {
                        crate::mem::lower_bound(sheet_keys.as_slice(), |&k| k < s.value.start)
                    } else {
                        let id = lists.len();
                        lists.push(ListRec { start: 0, len: 0, pos: 0, resolved: 0, pad: [0; 3] })?;
                        id
                    };
                    if lists.get(id).is_some_and(|l| l.resolved == 0) {
                        let at = ledges.len();
                        let mut found: MVec<'_, Span> = MVec::new(budget);
                        let mut escaped: MVec<'_, u8> = MVec::new(budget);
                        cx.urls(buf, s.value, &mut |sp, esc| {
                            found.push(sp)?;
                            escaped.push(u8::from(esc))
                        })?;
                        let want: &[&[u8]] = match kind {
                            EDGE_CLIP => &[b"clipPath"],
                            EDGE_MASK => &[b"mask"],
                            _ => &[b"filter"],
                        };
                        for (k, &sp) in found.as_slice().iter().enumerate() {
                            budget.spend(cost::OP_STEP)?;
                            let esc = escaped.get(k).copied().unwrap_or(0) != 0;
                            if let Some(t) = cx.resolve(buf, sp, esc, s.src, pos, &mut notes)?
                                && target_kind_ok(nodes, input, t, want)
                            {
                                ledges.push(Edge { target: t, kind, removed: 0, pad: [0; 2] })?;
                            }
                        }
                        if let Some(l) = lists.get_mut(id) {
                            *l = ListRec {
                                start: at as u32,
                                len: ledges.len().saturating_sub(at) as u32,
                                pos,
                                resolved: 1,
                                pad: [0; 3],
                            };
                        }
                    }
                    u32::try_from(id).unwrap_or(NONE)
                }
                _ => NONE,
            };
            if let Some(x) = mine.t.get_mut(slot) {
                *x = list;
            }
            if list != NONE {
                let target = u32::try_from(n).unwrap_or(NONE).saturating_add(list);
                edges.push(Edge { target, kind, removed: 0, pad: [0; 2] })?;
            }
        }
        own_lists.push(mine)?;
        inh.push(own)?;
        // 受け継いだ参照の辺（塗る要素・marker を付ける要素）。marker の辺を塗りの辺より先に張る（marker の中の
        // context-fill・context-stroke が要素の塗りを使うので、展開の数で塗りの倍率を決めるときに marker を数え終えている。
        // 文書の 3 回目のレビュー）
        if paints(local) {
            for k in [2usize, 3, 4, 0, 1] {
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
    // 並びの節の辺（並びの番号の順）
    let mut list_pos: MVec<'_, u32> = MVec::with_capacity(budget, lists.len())?;
    for l in lists.as_slice() {
        budget.spend(cost::OP_STEP)?;
        let a = l.start as usize;
        let b = a.saturating_add(l.len as usize);
        edges.append_slice(ledges.as_slice().get(a..b).unwrap_or(&[]))?;
        start.push(edges.len() as u32)?;
        list_pos.push(l.pos)?;
    }
    // 輪を取り除き、展開の数を求める
    let start = start.freeze();
    let mut edges = edges;
    remove_cycles(doc, start.as_slice(), &mut edges, &mut notes, list_pos.as_slice(), budget)?;
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

/// `switch` が選ぶ子（描く要素。`desc`・`title` などは選ばず、隠しもしない）。`foreignObject` は描かないので選ばない
/// （draw.io の書き出しの代わりの中身を描く。resvg と同じ。ADR-78 (6)）。
fn switch_candidate(local: &[u8]) -> bool {
    matches!(
        local,
        b"a" | b"g"
            | b"svg"
            | b"use"
            | b"image"
            | b"text"
            | b"switch"
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

/// 強連結成分を求める歩きの表（Tarjan）。
struct Tarjan<'a, 'b> {
    index: &'a mut MVec<'b, u32>,
    low: &'a mut MVec<'b, u32>,
    on: &'a mut MVec<'b, u8>,
    sstack: &'a mut MVec<'b, u32>,
    next: &'a mut u32,
}

impl Tarjan<'_, '_> {
    /// 節 `v` に入る（番号を振り、積み上げに積む）。
    fn enter(&mut self, v: u32, call: &mut MVec<'_, SccFrame>, child: u32, edge: u32) -> Result<(), Error> {
        let i = *self.next;
        if let Some(x) = self.index.get_mut(v as usize) {
            *x = i;
        }
        if let Some(x) = self.low.get_mut(v as usize) {
            *x = i;
        }
        *self.next = i.saturating_add(1);
        if let Some(x) = self.on.get_mut(v as usize) {
            *x = 1;
        }
        self.sstack.push(v)?;
        call.push(SccFrame { node: v, child, edge, pad: 0 })
    }
}

/// 強連結成分を求める歩きの段。
#[repr(C)]
#[derive(Clone, Copy)]
struct SccFrame {
    node: u32,
    /// 次に見る子（なければ NONE）
    child: u32,
    /// 次に見る辺の位置（`start` の中の位置）
    edge: u32,
    pad: u32,
}

impl Plain for SccFrame {}

/// 輪を取り除く（取り除いた辺に印を付け、輪の警告を足す）。1 度の歩きで済み、手間は節と辺の数に比例する（文書の 3 回目の
/// レビュー：取り残しの歩きは輪の数だけ歩き直す形と、輪でない辺を取り除く形があった）。
///
/// - 図は、子の辺（要素の子）と参照の辺（文字の辺を除く）。節は XML の節と並びの節（`list_pos` の数。XML の節の数から後ろ）
/// - 強連結成分（Tarjan。明示の積み上げ）を求め、輪のある成分（2 つ以上の節か、自分への辺）の中で、文書の順に後ろ向きの
///   参照の辺（行き先の番号 ≦ 元の番号）と、並びの節に入る辺を取り除く。子の辺はいつも前向き（節の番号は行きがけの順）、
///   並びの節は成分の中から入れなくなるので、残りに輪はない
/// - 取り除く辺はどれも、もとの図の輪の上にある（輪の参照だけを除く。FR-CO-05）。どの辺を除くかは文書の順で決まり、少ない
///   数を選ぶとは限らない（M2 の木を作る段が use の文脈で確かめる。ADR-78 (8)）
/// - 並びの節から出る辺は取り除かない（並びは要素で共有するので、輪の上の要素の、並びに入る辺を取り除く）
fn remove_cycles(
    doc: &XmlDoc,
    start: &[u32],
    edges: &mut MVec<'_, Edge>,
    notes: &mut MVec<'_, RefNote>,
    list_pos: &[u32],
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    let nodes = doc.nodes.as_slice();
    let n = nodes.len();
    let total = start.len().saturating_sub(1);
    if n == 0 {
        return Ok(());
    }
    let first_child = |i: u32| nodes.get(i as usize).map_or(NONE, |x| x.first_child);
    let edge_start = |i: u32| start.get(i as usize).copied().unwrap_or(0);
    let mut index: MVec<'_, u32> = MVec::with_capacity(budget, total)?;
    index.resize_to(total, NONE)?;
    let mut low: MVec<'_, u32> = MVec::with_capacity(budget, total)?;
    low.resize_to(total, 0)?;
    let mut on: MVec<'_, u8> = MVec::with_capacity(budget, total)?;
    on.resize_to(total, 0)?;
    let mut comp: MVec<'_, u32> = MVec::with_capacity(budget, total)?;
    comp.resize_to(total, NONE)?;
    let mut sizes: MVec<'_, u32> = MVec::new(budget);
    let mut sstack: MVec<'_, u32> = MVec::new(budget);
    let mut call: MVec<'_, SccFrame> = MVec::new(budget);
    let mut next_index = 0u32;
    for root in 0..total {
        budget.spend(cost::OP_STEP)?;
        let root = root as u32;
        if index.get(root as usize).copied() != Some(NONE) {
            continue;
        }
        let mut t =
            Tarjan { index: &mut index, low: &mut low, on: &mut on, sstack: &mut sstack, next: &mut next_index };
        t.enter(root, &mut call, first_child(root), edge_start(root))?;
        while let Some(top) = call.last().copied() {
            budget.spend(cost::OP_STEP)?;
            // 次の行き先：子（要素だけ）、次に参照の辺（飛ばす子と辺も 1 つずつ払う）
            let mut frame = top;
            let mut next = NONE;
            while frame.child != NONE {
                budget.spend(cost::OP_STEP)?;
                let c = frame.child;
                frame.child = nodes.get(c as usize).map_or(NONE, |x| x.next_sibling);
                if nodes.get(c as usize).is_some_and(|x| x.kind == KIND_ELEMENT) {
                    next = c;
                    break;
                }
            }
            if next == NONE {
                let end = edge_start(top.node.saturating_add(1));
                while frame.edge < end {
                    budget.spend(cost::OP_STEP)?;
                    let k = frame.edge;
                    frame.edge = frame.edge.saturating_add(1);
                    if let Some(e) = edges.get(k as usize)
                        && e.removed == 0
                        && e.kind != EDGE_TEXT
                    {
                        next = e.target;
                        break;
                    }
                }
            }
            if let Some(x) = call.last_mut() {
                *x = frame;
            }
            if next != NONE {
                match index.get(next as usize).copied() {
                    Some(NONE) => {
                        let mut t = Tarjan {
                            index: &mut index,
                            low: &mut low,
                            on: &mut on,
                            sstack: &mut sstack,
                            next: &mut next_index,
                        };
                        t.enter(next, &mut call, first_child(next), edge_start(next))?;
                    }
                    Some(ix) if on.get(next as usize).copied() == Some(1) => {
                        if let Some(l) = low.get_mut(top.node as usize) {
                            *l = (*l).min(ix);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            // 見終えた：根なら成分を取り出し、親の low を下げる
            call.pop();
            let v = top.node;
            let (lv, iv) = (low.get(v as usize).copied().unwrap_or(0), index.get(v as usize).copied().unwrap_or(0));
            if lv == iv {
                let id = sizes.len() as u32;
                let mut size = 0u32;
                while let Some(w) = sstack.pop() {
                    budget.spend(cost::OP_STEP)?;
                    if let Some(x) = on.get_mut(w as usize) {
                        *x = 0;
                    }
                    if let Some(x) = comp.get_mut(w as usize) {
                        *x = id;
                    }
                    size = size.saturating_add(1);
                    if w == v {
                        break;
                    }
                }
                sizes.push(size)?;
            }
            if let Some(p) = call.last() {
                let pn = p.node as usize;
                if let Some(l) = low.get_mut(pn) {
                    *l = (*l).min(lv);
                }
            }
        }
    }
    // 輪のある成分の中の、後ろ向きの参照の辺と並びの節に入る辺を取り除く（元の節の順に。警告は位置の順になる）
    for src in 0..total {
        budget.spend(cost::OP_STEP)?;
        let a = edge_start(src as u32) as usize;
        let b = edge_start((src as u32).saturating_add(1)) as usize;
        let cs = comp.get(src).copied().unwrap_or(NONE);
        for k in a..b {
            budget.spend(cost::OP_STEP)?;
            let Some(e) = edges.get(k).copied() else { continue };
            if e.removed != 0 || e.kind == EDGE_TEXT || comp.get(e.target as usize).copied() != Some(cs) {
                continue;
            }
            let t = e.target as usize;
            let cyclic = t == src || sizes.get(cs as usize).copied().unwrap_or(0) > 1;
            let cut = cyclic && src < n && (t >= n || t <= src);
            if !cut {
                continue;
            }
            if let Some(x) = edges.get_mut(k) {
                x.removed = 1;
            }
            let pos =
                nodes.get(src).map_or_else(|| list_pos.get(src.saturating_sub(n)).copied().unwrap_or(0), |x| x.pos);
            notes.push(RefNote { pos, href: Span::default(), src: 0, unread: 0, escaped: 0, pad: 0 })?;
        }
    }
    Ok(())
}

/// 数えた量（展開したあとの要素の数・その中の塗る要素の数・marker を付ける要素の数）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tally {
    count: u64,
    paints: u64,
    marks: u64,
}

impl Plain for Tally {}

/// 数え終えていない印（数は `CAP` で止めるので重ならない。文書の 2 回目のレビュー）。
const NOT_YET: u64 = u64::MAX;
const CAP: u64 = u64::MAX - 1;

fn cap_add(a: u64, b: u64) -> u64 {
    a.saturating_add(b).min(CAP)
}

fn cap_mul(a: u64, b: u64) -> u64 {
    a.saturating_mul(b).min(CAP)
}

/// 展開の数の積み上げの段。
#[repr(C)]
#[derive(Clone, Copy)]
struct CountFrame {
    node: u32,
    child: u32,
    edge: u32,
    /// use・feImage の中身（中身を作る href の行き先。なければ NONE）
    content: u32,
    /// 親へ足すときの倍率と、つなぎ（0 子、1 use の中身、2 そのほかの中身を作る参照）
    mult: u64,
    link: u8,
    pad: [u8; 7],
}

impl Plain for CountFrame {}

/// 展開したあとの要素の数（輪のない辺の上で、帰りがけの順に飽和の足し算。中身を作る参照は参照ごとに中身を数える）。
///
/// use・feImage の中身は use から塗りと marker を受け継ぐので、use の受け継いだ塗りの参照先は「中身の塗る要素の数」、
/// marker は「中身の marker を付ける要素の数」の倍で数える（中身の要素ごとに参照先の中身を作る。文書の 2 回目の
/// レビュー）。数は `CAP` で止める。並びの節（XML の節の数から後ろ）は自分を数えず、参照先の和。
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
    let total = start.len().saturating_sub(1);
    if n == 0 {
        return Ok(0);
    }
    let mut done: MVec<'_, Tally> = MVec::with_capacity(budget, total)?;
    done.resize_to(total, Tally { count: NOT_YET, paints: 0, marks: 0 })?;
    let mut on_stack: MVec<'_, u8> = MVec::with_capacity(budget, total)?;
    on_stack.resize_to(total, 0)?;
    let mut stack: MVec<'_, CountFrame> = MVec::new(budget);
    let mut acc: MVec<'_, Tally> = MVec::new(budget);
    let first_child = |i: u32| nodes.get(i as usize).map_or(NONE, |x| x.first_child);
    let edge_start = |i: u32| start.get(i as usize).copied().unwrap_or(0);
    let local_of = |i: u32| nodes.get(i as usize).filter(|x| x.ns == NS_SVG).map(|x| x.local.of(input));
    let is_use = |i: u32| local_of(i).is_some_and(|l| matches!(l, b"use" | b"feImage"));
    // 節を数え始めるときの自分の分
    let own = |i: u32| -> Tally {
        match nodes.get(i as usize) {
            None => Tally { count: 0, paints: 0, marks: 0 },
            Some(_) => {
                let l = local_of(i).unwrap_or(&[]);
                let shape = paints(l) && !matches!(l, b"use" | b"feImage");
                let mark = markable(l) && !matches!(l, b"use" | b"feImage");
                Tally { count: 1, paints: u64::from(shape), marks: u64::from(mark) }
            }
        }
    };
    // 子の分（中身の塗る要素と marker を付ける要素も足す）・use の中身の分（同じ）・ほかの参照の分（数だけを倍で）
    // つなぎ：0 子・1 use の中身（数と、塗りを使う要素・marker を付ける要素の数を足す）、2 ほかの中身を作る参照（数だけを倍で）、
    // 3 marker（数を倍で足し、marker の中の塗る要素を「この要素の塗りを使う要素」に倍で足す：context-fill・context-stroke）
    let add = |a: &mut Tally, t: Tally, link: u8, mult: u64| match link {
        0 | 1 => {
            a.count = cap_add(a.count, t.count);
            a.paints = cap_add(a.paints, t.paints);
            a.marks = cap_add(a.marks, t.marks);
        }
        3 => {
            a.count = cap_add(a.count, cap_mul(t.count, mult));
            a.paints = cap_add(a.paints, cap_mul(t.paints, mult));
        }
        _ => a.count = cap_add(a.count, cap_mul(t.count, mult)),
    };
    stack.push(CountFrame {
        node: 0,
        child: first_child(0),
        edge: edge_start(0),
        content: NONE,
        mult: 1,
        link: 0,
        pad: [0; 7],
    })?;
    acc.push(own(0))?;
    if let Some(x) = on_stack.get_mut(0) {
        *x = 1;
    }
    while let Some(top) = stack.last().copied() {
        budget.spend(cost::OP_STEP)?;
        // 次の行き先：（節, つなぎ, 倍率）
        let mut next: Option<(u32, u8, u64)> = None;
        let mut frame = top;
        while frame.child != NONE {
            budget.spend(cost::OP_STEP)?;
            let c = frame.child;
            let Some(x) = nodes.get(c as usize) else { break };
            frame.child = x.next_sibling;
            if x.kind != KIND_ELEMENT {
                continue;
            }
            // 子を描かない入れ物（defs・symbol・pattern など）は、子の辺では自分の 1 つだけを数え、中身は参照の辺で
            // 入ったときに数える（木を作る段も参照されたものだけを展開する）
            if x.ns == NS_SVG && not_rendered(x.local.of(input)) {
                if let Some(a) = acc.last_mut() {
                    a.count = cap_add(a.count, 1);
                }
                continue;
            }
            next = Some((c, 0, 1));
            break;
        }
        if next.is_none() {
            let end = edge_start(top.node.saturating_add(1));
            while frame.edge < end {
                budget.spend(cost::OP_STEP)?;
                let k = frame.edge;
                frame.edge = frame.edge.saturating_add(1);
                let Some(e) = edges.get(k as usize) else { continue };
                if e.removed != 0 || !instancing(e, nodes, input) {
                    continue;
                }
                let from_use = is_use(top.node);
                if e.kind == EDGE_USE && from_use {
                    frame.content = e.target;
                    next = Some((e.target, 1, 1));
                    break;
                }
                // 塗りの参照先は「この要素の塗りを使う要素」の数の倍（自分・use の中身の塗る要素・marker の中の塗る要素。
                // 辺は use の中身、marker、塗りの順に張るので、ここまでに数え終えている）。use の marker は中身の marker を
                // 付ける要素の数の倍
                let content = done.get(frame.content as usize).copied().filter(|t| t.count != NOT_YET);
                let users = acc.last().map_or(1, |a| a.paints);
                let (link, mult) = match e.kind {
                    EDGE_FILL | EDGE_STROKE => (2, users),
                    EDGE_MARKER_START | EDGE_MARKER_MID | EDGE_MARKER_END => {
                        (3, if from_use { content.map_or(0, |t| t.marks) } else { 1 })
                    }
                    _ => (2, 1),
                };
                next = Some((e.target, link, mult));
                break;
            }
        }
        if let Some(x) = stack.last_mut() {
            *x = frame;
        }
        match next {
            Some((t, link, mult)) => {
                let d = done.get(t as usize).copied().unwrap_or(Tally { count: 0, paints: 0, marks: 0 });
                if d.count != NOT_YET {
                    if let Some(a) = acc.last_mut() {
                        add(a, d, link, mult);
                    }
                } else if on_stack.get(t as usize).copied() == Some(0) {
                    if let Some(x) = on_stack.get_mut(t as usize) {
                        *x = 1;
                    }
                    stack.push(CountFrame {
                        node: t,
                        child: first_child(t),
                        edge: edge_start(t),
                        content: NONE,
                        mult,
                        link,
                        pad: [0; 7],
                    })?;
                    acc.push(own(t))?;
                }
            }
            None => {
                stack.pop();
                let mine = acc.pop().unwrap_or(Tally { count: 0, paints: 0, marks: 0 });
                if let Some(c) = done.get_mut(top.node as usize) {
                    *c = mine;
                }
                if let Some(x) = on_stack.get_mut(top.node as usize) {
                    *x = 0;
                }
                if let Some(a) = acc.last_mut() {
                    add(a, mine, top.link, top.mult);
                }
            }
        }
    }
    Ok(done.get(0).map_or(0, |t| t.count))
}

/// 値の前の空白を除いた頭が `#` か（読むバイトを塊ずつ払う）。
fn starts_with_hash(v: &[u8], budget: &Budget<'_>) -> Result<bool, Error> {
    let mut meter = crate::xml::lex::ScanMeter::new(0, v.len());
    for (k, &c) in v.iter().enumerate() {
        meter.pay(budget, k)?;
        if !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0C) {
            return Ok(c == b'#');
        }
    }
    Ok(false)
}

/// 塗りの参照先になる要素。
const PAINT_SERVERS: &[&[u8]] = &[b"linearGradient", b"radialGradient", b"pattern"];
const GRADIENTS: &[&[u8]] = &[b"linearGradient", b"radialGradient"];

/// 要素 `local` の `href` が指せる要素（空ならどれでも）。グラデーションはグラデーション、pattern は pattern、filter は
/// filter から属性と中身を受け継ぐ（SVG 2 §14.2・§14.3・FE 1 §5）。
fn href_targets(local: &[u8]) -> &'static [&'static [u8]] {
    match local {
        b"linearGradient" | b"radialGradient" => GRADIENTS,
        b"pattern" => &[b"pattern"],
        b"filter" => &[b"filter"],
        _ => &[],
    }
}

/// 参照先 `t` が使える種類の要素か（`want` が空ならどれでも）。種類の合わない参照は辺にしない（使わない参照で輪を作らない。
/// 文書の 1 回目のレビュー。ADR-78 (8)）。
fn target_kind_ok(nodes: &[Node], input: &[u8], t: u32, want: &[&[u8]]) -> bool {
    want.is_empty()
        || nodes.get(t as usize).is_some_and(|x| x.ns == NS_SVG && want.iter().any(|w| x.local.of(input) == *w))
}

/// 子を描く入れ物か（図形・use・image の子は描かない。文字の要素の子は文字の中身）。
pub(crate) fn renders_children(local: &[u8]) -> bool {
    matches!(local, b"svg" | b"g" | b"a" | b"switch" | b"text" | b"tspan" | b"textPath")
}

/// 要素が親の文脈で描かれうるか（SVG 2 の内容のモデルの近いもの）：文字の中（`in_text`）では文字の中身の要素（tspan・
/// textPath・tref）と `a` だけ、文字の外では文字の中身の要素と `stop`・フィルタの要素（fe…）を描かない（文書の 1 回目の
/// レビュー。Chrome・usvg と同じ）。
pub(crate) fn fits(local: &[u8], in_text: bool) -> bool {
    let text_content = matches!(local, b"tspan" | b"textPath" | b"tref");
    if in_text { text_content || local == b"a" } else { !text_content && local != b"stop" && !local.starts_with(b"fe") }
}

/// 子を描かない入れ物（参照の辺で入ったときだけ中をたどる）。
pub(crate) fn not_rendered(local: &[u8]) -> bool {
    matches!(
        local,
        b"defs"
            | b"symbol"
            | b"clipPath"
            | b"mask"
            | b"pattern"
            | b"marker"
            | b"linearGradient"
            | b"radialGradient"
            | b"filter"
            | b"title"
            | b"desc"
            | b"metadata"
            | b"style"
            | b"script"
            | b"foreignObject"
    )
}

/// 参照ごとに中身を作る辺か（中身を作らない href と、グラデーションへの塗り（stop だけで、描く要素を作らない）は数えない）。
fn instancing(e: &Edge, nodes: &[Node], input: &[u8]) -> bool {
    match e.kind {
        EDGE_HREF | EDGE_TEXT => false,
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
