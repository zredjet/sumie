//! セレクタの照合（architecture.md §4.2・ADR-41 (1)）。文書を行きがけの順に 1 度流し、複合のセレクタごとの旗で決める。
//!
//! 要素 e と複合 k の旗 `M[e][k]` は「k が e に当たり、k の左の複合までが結合子のとおり当たる」。結合子ごとに：
//!
//! - 子孫：親の「祖先か自分のどれかで立った旗」（深さごとの `anc`）
//! - 子：親の旗（深さごとの `cur`）
//! - 直後の兄弟：直前の要素の兄弟の旗（同じ深さの `cur` が同じ親のもの）
//! - 後の兄弟：前の要素の兄弟のどれかで立った旗（深さごとの `later`）
//!
//! 複合は番号の順に決める（`:is()` などの中の選択子は外の複合より番号が小さいので、同じ要素で先に決まる）。要素ごとの
//! 手間は「複合の数 ＋ 単純な選択子を照らす手間」で、後戻りがない。複合ごと・単純な選択子ごとに払い、属性を探す走査は
//! 属性ごとに払う。要素の id・class・言語・`:link`・`:empty` は、使うときだけ要素ごとに 1 度だけ求める。後ろの兄弟を見る
//! 擬似クラス（`:last-child` など）は、照合の前に兄弟の数を数えておく。確保は（深さ ＋ 1）× 複合の数のビットの 3 組と、
//! 兄弟の数の表（使うときだけ）。
//!
//! 要素の名前空間は種類（SVG・XLink・XML・なし・そのほか）だけを持つので、`@namespace` の「そのほか」の URI は区別しない
//! （XML の側は ADR-65。CSS での帰結：`@namespace a "urn:a"; a|*` は urn:b の要素にも当たる。ADR-77 (3)）。どの名前空間の要素も兄弟と親子に数える（照らしの `first-child-foreign`・`adjacent-foreign` と同じ）。

use crate::budget::Budget;
use crate::cost;
use crate::css::select::{
    COMB_CHILD, COMB_DESC, COMB_LATER, COMB_NEXT, FLAG_CASE_I, FLAG_NO_TOKEN, NONE, NS_ANY, OP_DASH, OP_EQ, OP_EXISTS,
    OP_INCLUDES, OP_PREFIX, OP_SUBSTR, OP_SUFFIX, S_ATTR, S_CLASS, S_EMPTY, S_FIRST_CHILD, S_FIRST_OF_TYPE, S_ID, S_IS,
    S_LANG, S_LAST_CHILD, S_LAST_OF_TYPE, S_LINK, S_NEVER, S_NOT, S_NTH_CHILD, S_NTH_LAST_CHILD, S_NTH_LAST_OF_TYPE,
    S_NTH_OF_TYPE, S_ONLY_CHILD, S_ONLY_OF_TYPE, S_ROOT, S_TYPE, S_UNIVERSAL, Simple,
};
use crate::css::sheet::Sheet;
use crate::error::Error;
use crate::mem::{
    MVec, Plain, bytes_cmp, bytes_eq, bytes_eq_ignore_case, lower_bound_try, stable_sort, stable_sort_try,
};
use crate::xml::lex::{ScanMeter, Span};
use crate::xml::{KIND_ELEMENT, NS_NONE, NS_SVG, NS_XLINK, NS_XML, Node, XmlDoc};

/// 照合の結果を受ける側（要素ごとに `element` のあと、当たった規則を規則の順に `rule`）。
pub(crate) trait MatchSink {
    fn element(&mut self, node: u32) -> Result<(), Error>;
    /// 規則 `rule` が当たった（`spec` は当たった選択子のいちばん高い詳細度）
    fn rule(&mut self, rule: u32, spec: u32) -> Result<(), Error>;
}

/// 要素の兄弟の数（1 から数える位置と数。同じ型のものも）。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct Sib {
    index: u32,
    count: u32,
    type_index: u32,
    type_count: u32,
}

impl Plain for Sib {}

/// 照合に使う単純な選択子の種類。
#[derive(Default, Clone, Copy)]
struct Uses {
    sib: bool,
    ty: bool,
    id: bool,
    class: bool,
    lang: bool,
    link: bool,
    empty: bool,
}

/// どの単純な選択子を使うか（単純な選択子ごとに払う）。
fn uses(sheet: &Sheet, budget: &Budget<'_>) -> Result<Uses, Error> {
    let mut u = Uses::default();
    for s in sheet.simples.as_slice() {
        budget.spend(cost::OP_STEP)?;
        match s.kind {
            S_LAST_CHILD | S_ONLY_CHILD | S_NTH_LAST_CHILD | S_NTH_CHILD | S_FIRST_CHILD => u.sib = true,
            S_FIRST_OF_TYPE | S_LAST_OF_TYPE | S_ONLY_OF_TYPE | S_NTH_OF_TYPE | S_NTH_LAST_OF_TYPE => {
                u.sib = true;
                u.ty = true;
            }
            S_ID => u.id = true,
            S_CLASS => u.class = true,
            S_LANG => u.lang = true,
            S_LINK => u.link = true,
            S_EMPTY => u.empty = true,
            _ => {}
        }
    }
    Ok(u)
}

/// 要素ごとに 1 度だけ求めるもの（属性は `XmlDoc::attrs` の番号。なければ NONE）。
#[derive(Clone, Copy)]
struct Elem {
    id: u32,
    class: u32,
    /// 言語の属性（自分か、受け継いだもの）
    lang: u32,
    link: bool,
    empty: bool,
}

/// 要素の属性を 1 度だけ走査して（属性ごとに払う）、使うものを求める。`parent_lang` は親の言語の属性。
fn elem_info(
    doc: &XmlDoc,
    input: &[u8],
    n: &Node,
    u: Uses,
    parent_lang: u32,
    budget: &Budget<'_>,
) -> Result<Elem, Error> {
    let mut e = Elem { id: NONE, class: NONE, lang: NONE, link: false, empty: false };
    let mut xml_lang = NONE;
    let mut plain_lang = NONE;
    let mut href = false;
    if u.id || u.class || u.lang || u.link {
        for k in n.attrs.start..n.attrs.end {
            budget.spend(cost::OP_STEP)?;
            let Some(a) = doc.attrs.as_slice().get(k as usize) else { break };
            let local = a.local.of(input);
            match (a.ns, local) {
                (NS_NONE, b"id") if e.id == NONE => e.id = k,
                (NS_NONE, b"class") if e.class == NONE => e.class = k,
                (NS_XML, b"lang") if xml_lang == NONE => xml_lang = k,
                // 名前空間のない lang は SVG の要素のもの（SVG 2 §5.10。HTML と同じく、ほかの名前空間の要素は xml:lang だけ）
                (NS_NONE, b"lang") if plain_lang == NONE && n.ns == NS_SVG => plain_lang = k,
                (NS_NONE | NS_XLINK, b"href") => href = true,
                _ => {}
            }
        }
    }
    // 言語：xml:lang、なければ（SVG の要素の）名前空間のない lang、なければ親から
    e.lang = if xml_lang != NONE {
        xml_lang
    } else if plain_lang != NONE {
        plain_lang
    } else {
        parent_lang
    };
    e.link = u.link && href && n.ns == NS_SVG && n.local.of(input) == b"a";
    if u.empty {
        // 子の要素も、長さのある文字もない（注釈と処理命令は節にならない。空の CDATA は数えない。Chrome と同じ）
        let mut empty = true;
        let mut c = n.first_child;
        while c != NONE {
            budget.spend(cost::OP_STEP)?;
            let Some(ch) = doc.node(c) else { break };
            if ch.kind == KIND_ELEMENT || !doc.text(input, ch).is_empty() {
                empty = false;
                break;
            }
            c = ch.next_sibling;
        }
        e.empty = empty;
    }
    Ok(e)
}

/// 要素の兄弟の位置と数を数える（`with_type` なら同じ型（名前空間と局所名）の中の位置と数も）。
fn siblings<'b>(doc: &XmlDoc, input: &[u8], with_type: bool, budget: &'b Budget<'b>) -> Result<MVec<'b, Sib>, Error> {
    let nodes = doc.nodes.as_slice();
    let mut out: MVec<'b, Sib> = MVec::with_capacity(budget, nodes.len())?;
    out.resize_to(nodes.len(), Sib::default())?;
    let mut kids: MVec<'b, u32> = MVec::new(budget);
    // 親ごとに（根の要素は親がなく、兄弟もない）
    for pn in nodes {
        budget.spend(cost::OP_STEP)?;
        let first = if pn.kind == KIND_ELEMENT { pn.first_child } else { NONE };
        let mut c = first;
        kids.clear();
        while c != NONE {
            budget.spend(cost::OP_STEP)?;
            let Some(n) = nodes.get(c as usize) else { break };
            if n.kind == KIND_ELEMENT {
                kids.push(c)?;
            }
            c = n.next_sibling;
        }
        let count = kids.len() as u32;
        for (k, &id) in kids.as_slice().iter().enumerate() {
            budget.spend(cost::OP_STEP)?;
            if let Some(x) = out.get_mut(id as usize) {
                x.index = (k as u32).saturating_add(1);
                x.count = count;
            }
        }
        if with_type && kids.len() > 1 {
            // 名前で並べ（同じ名前は文書の順のまま）、続く同じ名前の並びで数える
            let key = |id: &u32| {
                let n = nodes.get(*id as usize);
                (n.map_or(0, |n| n.ns), n.map_or(&[][..], |n| n.local.of(input)))
            };
            // 名前のバイトは比べる中で塊ずつ払う
            stable_sort_try(budget, kids.as_mut_slice(), |x, y| {
                let (kx, ky) = (key(x), key(y));
                if kx.0 != ky.0 {
                    return Ok(kx.0 < ky.0);
                }
                Ok(bytes_cmp(budget, kx.1, ky.1)? == core::cmp::Ordering::Less)
            })?;
            let ks = kids.as_slice();
            let mut i = 0usize;
            while i < ks.len() {
                let mut j = i;
                while j < ks.len() {
                    budget.spend(cost::OP_STEP)?;
                    let (Some(x), Some(y)) = (ks.get(j), ks.get(i)) else { break };
                    let (kx, ky) = (key(x), key(y));
                    if kx.0 != ky.0 || !bytes_eq(budget, kx.1, ky.1)? {
                        break;
                    }
                    j = j.saturating_add(1);
                }
                let run = (j.saturating_sub(i)) as u32;
                for (k, id) in ks.get(i..j).unwrap_or(&[]).iter().enumerate() {
                    budget.spend(cost::OP_STEP)?;
                    if let Some(x) = out.get_mut(*id as usize) {
                        x.type_index = (k as u32).saturating_add(1);
                        x.type_count = run;
                    }
                }
                i = j;
            }
        } else if let Some(&only) = kids.as_slice().first().filter(|_| kids.len() == 1)
            && let Some(x) = out.get_mut(only as usize)
        {
            x.type_index = 1;
            x.type_count = 1;
        }
    }
    // 根の要素
    for (i, n) in nodes.iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        if n.kind == KIND_ELEMENT && n.parent == NONE {
            if let Some(x) = out.get_mut(i) {
                *x = Sib { index: 1, count: 1, type_index: 1, type_count: 1 };
            }
            break;
        }
    }
    Ok(out)
}

/// 開いている要素の積み上げの 1 段。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Level {
    node: u32,
    /// この深さで最後に見た要素の親（直前の兄弟か見分ける）
    last_parent: u32,
    /// 言語の属性（`XmlDoc::attrs` の番号。受け継ぐ。なければ NONE）
    lang: u32,
    pad: u32,
}

impl Plain for Level {}

/// 複合の鍵の種類（その複合が当たる要素が必ず持つもの）。
const KEY_ID: u8 = 0;
const KEY_CLASS: u8 = 1;
const KEY_TYPE: u8 = 2;

/// 複合の索引の 1 つ（鍵の種類・名前と複合の番号）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct KeyEntry {
    name: Span,
    compound: u32,
    kind: u8,
    pad: [u8; 3],
}

impl Plain for KeyEntry {}

/// 規則の右端の複合ごとの（規則, 詳細度）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Subject {
    compound: u32,
    rule: u32,
    spec: u32,
}

impl Plain for Subject {}

/// 当たった規則（規則の番号と詳細度）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Hit {
    rule: u32,
    spec: u32,
}

impl Plain for Hit {}

/// 一度に払う語の数（語を埋める・書き換える手間を塊ずつ払う）。
const WORDS_CHUNK: usize = 2048;

/// 語の並びを 0 で埋める（塊ずつ払う）。
fn clear_words(budget: &Budget<'_>, w: &mut [u64]) -> Result<(), Error> {
    let mut a = 0usize;
    while a < w.len() {
        let b = a.saturating_add(WORDS_CHUNK).min(w.len());
        budget.spend(cost::fill((b.saturating_sub(a) as u64).saturating_mul(8)))?;
        if let Some(part) = w.get_mut(a..b) {
            part.fill(0);
        }
        a = b;
    }
    Ok(())
}

/// 照合する。`active[r]` が 0 の規則は報告しない（条件が偽）。
///
/// 複合ごとに、当たる要素が必ず持つ鍵（id・class・型。この順に選ぶ）を決めて索引にし、要素ごとに「鍵のない複合」と
/// 「要素の id・class・局所名を鍵にする複合」だけを番号の順に照らす（照らさない複合はその要素に当たらない）。当たった
/// 右端の複合から規則を引く（すべての規則を回らない）。
pub(crate) fn match_rules(
    doc: &XmlDoc,
    input: &[u8],
    sheet: &Sheet,
    active: &[u8],
    budget: &Budget<'_>,
    sink: &mut dyn MatchSink,
) -> Result<(), Error> {
    match_rules_with(doc, input, sheet, active, budget, sink, true)
}

/// 索引を使わずに照合する（すべての複合を鍵のないものとして要素ごとに照らす。索引の取りこぼしと当てすぎを見張る
/// 照らしの相手。試験とファズの `css` の対象が使う）。
pub(crate) fn match_rules_unindexed(
    doc: &XmlDoc,
    input: &[u8],
    sheet: &Sheet,
    active: &[u8],
    budget: &Budget<'_>,
    sink: &mut dyn MatchSink,
) -> Result<(), Error> {
    match_rules_with(doc, input, sheet, active, budget, sink, false)
}

fn match_rules_with(
    doc: &XmlDoc,
    input: &[u8],
    sheet: &Sheet,
    active: &[u8],
    budget: &Budget<'_>,
    sink: &mut dyn MatchSink,
    index: bool,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    if sheet.rules.is_empty() {
        // 規則がなければ要素だけを知らせる
        for (i, n) in doc.nodes.as_slice().iter().enumerate() {
            budget.spend(cost::OP_STEP)?;
            if n.kind == KIND_ELEMENT {
                sink.element(i as u32)?;
            }
        }
        return Ok(());
    }
    let nodes = doc.nodes.as_slice();
    let names = sheet.names.as_slice();
    let ncomp = sheet.compounds.len();
    let words = ncomp.saturating_add(63).checked_div(64).unwrap_or(0);
    let mut u = uses(sheet, budget)?;
    // 鍵の索引：id・class の索引を引くので、要素の id と class を求める
    u.id = true;
    u.class = true;
    let sib = if u.sib { Some(siblings(doc, input, u.ty, budget)?) } else { None };
    // 生きている複合（当てる規則の選択子から、左の複合と `:is()`・`:not()` などの中の並びをたどって届くもの）。条件が偽の
    // 規則と、捨てた規則の残りの複合は照らさない（CSS の 4 回目のレビュー）
    let live = live_compounds(sheet, active, budget)?;
    // 複合の鍵
    let mut keyed: MVec<'_, KeyEntry> = MVec::new(budget);
    let mut keyless: MVec<'_, u32> = MVec::new(budget);
    for (k, c) in sheet.compounds.as_slice().iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        if live.get(k).copied() != Some(1) {
            continue;
        }
        let mut best: Option<KeyEntry> = None;
        let mut dead = false;
        let mut sp = c.first;
        while sp != NONE {
            budget.spend(cost::OP_STEP)?;
            let Some(x) = sheet.simples.as_slice().get(sp as usize) else { break };
            let cand = match x.kind {
                S_ID => Some(KEY_ID),
                S_CLASS if x.flags & FLAG_NO_TOKEN != 0 => {
                    dead = true;
                    None
                }
                S_CLASS => Some(KEY_CLASS),
                S_TYPE => Some(KEY_TYPE),
                _ => None,
            };
            if let Some(kind) = cand
                && best.is_none_or(|b| kind < b.kind)
            {
                best = Some(KeyEntry { name: x.a, compound: k as u32, kind, pad: [0; 3] });
            }
            sp = x.next;
        }
        if !index {
            keyless.push(k as u32)?;
            continue;
        }
        if dead {
            continue;
        }
        match best {
            Some(e) => keyed.push(e)?,
            None => keyless.push(k as u32)?,
        }
    }
    let key_less = |x: &KeyEntry, y: &KeyEntry| -> Result<bool, Error> {
        if x.kind != y.kind {
            return Ok(x.kind < y.kind);
        }
        match bytes_cmp(budget, x.name.of(names), y.name.of(names))? {
            core::cmp::Ordering::Equal => Ok(x.compound < y.compound),
            o => Ok(o.is_lt()),
        }
    };
    stable_sort_try(budget, keyed.as_mut_slice(), key_less)?;
    // 規則の右端の複合
    let mut subjects: MVec<'_, Subject> = MVec::new(budget);
    for (r, rule) in sheet.rules.as_slice().iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        if active.get(r).copied().unwrap_or(0) == 0 {
            continue;
        }
        let mut sel = rule.selector;
        while sel != NONE {
            budget.spend(cost::OP_STEP)?;
            let Some(s) = sheet.selectors.as_slice().get(sel as usize) else { break };
            subjects.push(Subject { compound: s.subject, rule: r as u32, spec: s.spec })?;
            sel = s.next;
        }
    }
    stable_sort(budget, subjects.as_mut_slice(), |a, b| a.compound < b.compound)?;
    // 複合ごとの subjects の始まり（当たるたびに 2 分で探さない。CSS の 3 回目のレビュー）
    let mut subj_start: MVec<'_, u32> = MVec::with_capacity(budget, ncomp.saturating_add(1))?;
    {
        let list = subjects.as_slice();
        let mut j = 0usize;
        for k in 0..=ncomp {
            budget.spend(cost::OP_STEP)?;
            while list.get(j).is_some_and(|x| (x.compound as usize) < k) {
                budget.spend(1)?;
                j = j.saturating_add(1);
            }
            subj_start.push(j as u32)?;
        }
    }
    // 深さの上限（行きがけの順に親の深さを積む）
    let mut max_depth = 0usize;
    {
        let mut stack: MVec<'_, u32> = MVec::new(budget);
        for (i, n) in nodes.iter().enumerate() {
            budget.spend(cost::OP_STEP)?;
            if n.kind != KIND_ELEMENT {
                continue;
            }
            while stack.last().is_some_and(|&t| t != n.parent) {
                budget.spend(1)?;
                stack.pop();
            }
            stack.push(i as u32)?;
            max_depth = max_depth.max(stack.len());
        }
    }
    let size = max_depth.saturating_add(1).saturating_mul(words);
    let mut cur: MVec<'_, u64> = MVec::with_capacity(budget, size)?;
    cur.resize_to(size, 0)?;
    let mut anc: MVec<'_, u64> = MVec::with_capacity(budget, size)?;
    anc.resize_to(size, 0)?;
    let mut later: MVec<'_, u64> = MVec::with_capacity(budget, size)?;
    later.resize_to(size, 0)?;
    let mut newm: MVec<'_, u64> = MVec::with_capacity(budget, words)?;
    newm.resize_to(words, 0)?;
    let mut levels: MVec<'_, Level> = MVec::with_capacity(budget, max_depth.saturating_add(1))?;
    levels.resize_to(max_depth.saturating_add(1), Level { node: NONE, last_parent: NONE, lang: NONE, pad: 0 })?;
    let mut path: MVec<'_, u32> = MVec::with_capacity(budget, max_depth.saturating_add(1))?;
    let mut cands: MVec<'_, u32> = MVec::new(budget);
    let mut keyed_c: MVec<'_, u32> = MVec::new(budget);
    // 鍵の範囲（同じ鍵の並びの頭）ごとに、最後に候補に足した要素（同じ class の語が重なっても 1 度だけ足す。CSS の 4 回目の
    // レビュー）
    let mut range_seen: MVec<'_, u32> = MVec::with_capacity(budget, keyed.len())?;
    range_seen.resize_to(keyed.len(), NONE)?;
    let mut hits: MVec<'_, Hit> = MVec::new(budget);
    let ctx = Ctx { doc, input, sheet, sib: sib.as_ref().map(MVec::as_slice), budget };
    for (i, n) in nodes.iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        if n.kind != KIND_ELEMENT {
            continue;
        }
        let e = i as u32;
        while path.last().is_some_and(|&t| t != n.parent) {
            budget.spend(1)?;
            path.pop();
        }
        let d = path.len();
        path.push(e)?;
        let same_parent = levels.get(d).is_some_and(|l| l.last_parent == n.parent && l.node != NONE);
        let row = d.saturating_mul(words);
        let prow = d.saturating_sub(1).saturating_mul(words);
        if !same_parent && let Some(w) = later.as_mut_slice().get_mut(row..row.saturating_add(words)) {
            clear_words(budget, w)?;
        }
        // 要素ごとに 1 度だけ求めるもの（言語は親から受け継ぐ）
        let parent_lang = if d > 0 { levels.get(d.saturating_sub(1)).map_or(NONE, |l| l.lang) } else { NONE };
        let info = elem_info(doc, input, n, u, parent_lang, budget)?;
        // 照らす複合：鍵のないものと、要素の id・class・局所名を鍵にするもの（番号の順）
        keyed_c.clear();
        let add_key =
            |kind: u8, name: &[u8], cands: &mut MVec<'_, u32>, seen: &mut MVec<'_, u32>| -> Result<(), Error> {
                let list = keyed.as_slice();
                let lo = lower_bound_try(budget, list, |x| {
                    if x.kind != kind {
                        return Ok(x.kind < kind);
                    }
                    Ok(bytes_cmp(budget, x.name.of(names), name)?.is_lt())
                })?;
                // この要素で足した範囲なら飛ばす（範囲の頭が同じ鍵のときだけ印を使う）
                match list.get(lo) {
                    Some(x) if x.kind == kind && bytes_eq(budget, x.name.of(names), name)? => {
                        if seen.get(lo).copied() == Some(e) {
                            return Ok(());
                        }
                        if let Some(s) = seen.get_mut(lo) {
                            *s = e;
                        }
                    }
                    _ => return Ok(()),
                }
                for x in list.get(lo..).unwrap_or(&[]) {
                    budget.spend(cost::OP_STEP)?;
                    if x.kind != kind || !bytes_eq(budget, x.name.of(names), name)? {
                        break;
                    }
                    cands.push(x.compound)?;
                }
                Ok(())
            };
        add_key(KEY_TYPE, n.local.of(input), &mut keyed_c, &mut range_seen)?;
        if let Some(v) = ctx.attr_value(info.id) {
            add_key(KEY_ID, v, &mut keyed_c, &mut range_seen)?;
        }
        if let Some(v) = ctx.attr_value(info.class) {
            // 空白で区切った語ごと（読むバイトを払う）
            let mut meter = crate::xml::lex::ScanMeter::new(0, v.len());
            let mut a = 0usize;
            for k in 0..=v.len() {
                meter.pay(budget, k)?;
                if v.get(k).is_some_and(|&c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)) {
                    continue;
                }
                if k > a {
                    add_key(KEY_CLASS, v.get(a..k).unwrap_or(&[]), &mut keyed_c, &mut range_seen)?;
                }
                a = k.saturating_add(1);
            }
        }
        // 鍵で引いたもの（短い）を並べ、鍵のないもの（並んでいる）と合わせて番号の順にする
        stable_sort(budget, keyed_c.as_mut_slice(), |a, b| a < b)?;
        cands.clear();
        {
            let (xs, ys) = (keyless.as_slice(), keyed_c.as_slice());
            let (mut p, mut q) = (0usize, 0usize);
            loop {
                budget.spend(cost::OP_STEP)?;
                let next = match (xs.get(p), ys.get(q)) {
                    (Some(&x), Some(&y)) if x <= y => {
                        p = p.saturating_add(1);
                        x
                    }
                    (_, Some(&y)) => {
                        q = q.saturating_add(1);
                        y
                    }
                    (Some(&x), None) => {
                        p = p.saturating_add(1);
                        x
                    }
                    (None, None) => break,
                };
                if cands.last() != Some(&next) {
                    cands.push(next)?;
                }
            }
        }
        // 旗を番号の順に決める（照らす複合ごとに払う。同じ番号は 1 度だけ）
        clear_words(budget, newm.as_mut_slice())?;
        for &k in cands.as_slice() {
            budget.spend(cost::OP_STEP)?;
            let Some(c) = sheet.compounds.as_slice().get(k as usize) else { continue };
            // 型のない複合に効く既定の名前空間
            if c.ns != NS_ANY && c.ns != n.ns {
                continue;
            }
            if !ctx.compound(e, n, c.first, &info, newm.as_slice())? {
                continue;
            }
            let prev_ok = if c.prev == NONE {
                true
            } else {
                let p = c.prev as usize;
                let get = |v: &MVec<'_, u64>, base: usize| bit(v.as_slice(), base, p);
                match c.comb {
                    COMB_DESC => d > 0 && get(&anc, prow),
                    COMB_CHILD => d > 0 && get(&cur, prow),
                    COMB_NEXT => same_parent && get(&cur, row),
                    COMB_LATER => same_parent && get(&later, row),
                    _ => false,
                }
            };
            let (wi, m) = word_bit(k as usize);
            if prev_ok && let Some(w) = newm.as_mut_slice().get_mut(wi) {
                *w |= m;
            }
        }
        // 深さ d の旗を書き換える（塊ずつ払う）
        let nm = newm.as_slice();
        let mut j = 0usize;
        while j < words {
            let end = j.saturating_add(WORDS_CHUNK).min(words);
            budget.spend(cost::fill((end.saturating_sub(j) as u64).saturating_mul(8)).saturating_mul(3))?;
            while j < end {
                let m = nm.get(j).copied().unwrap_or(0);
                let pa = if d > 0 { anc.as_slice().get(prow.saturating_add(j)).copied().unwrap_or(0) } else { 0 };
                if let Some(x) = cur.as_mut_slice().get_mut(row.saturating_add(j)) {
                    *x = m;
                }
                if let Some(x) = anc.as_mut_slice().get_mut(row.saturating_add(j)) {
                    *x = pa | m;
                }
                if let Some(x) = later.as_mut_slice().get_mut(row.saturating_add(j)) {
                    *x |= m;
                }
                j = j.saturating_add(1);
            }
        }
        if let Some(l) = levels.get_mut(d) {
            *l = Level { node: e, last_parent: n.parent, lang: info.lang, pad: 0 };
        }
        // 当たった右端の複合から規則を引き、規則の順に知らせる（同じ規則はいちばん高い詳細度）
        sink.element(e)?;
        hits.clear();
        for &k in cands.as_slice() {
            budget.spend(cost::OP_STEP)?;
            if !bit(nm, 0, k as usize) {
                continue;
            }
            let lo = subj_start.get(k as usize).copied().unwrap_or(0) as usize;
            let hi = subj_start.get((k as usize).saturating_add(1)).copied().unwrap_or(0) as usize;
            for x in subjects.as_slice().get(lo..hi).unwrap_or(&[]) {
                budget.spend(cost::OP_STEP)?;
                hits.push(Hit { rule: x.rule, spec: x.spec })?;
            }
        }
        stable_sort(budget, hits.as_mut_slice(), |a, b| a.rule < b.rule)?;
        let mut k = 0usize;
        let hs = hits.as_slice();
        while let Some(h) = hs.get(k) {
            budget.spend(cost::OP_STEP)?;
            let mut best = h.spec;
            let mut j = k.saturating_add(1);
            while let Some(x) = hs.get(j).filter(|x| x.rule == h.rule) {
                best = best.max(x.spec);
                j = j.saturating_add(1);
            }
            sink.rule(h.rule, best)?;
            k = j;
        }
    }
    Ok(())
}

/// 生きている複合の印（複合の数の長さ。1 が生きている）。中の並びの複合と左の複合は外の複合より番号が小さいので、番号の
/// 大きい方から 1 度流せば足りる。
fn live_compounds<'b>(sheet: &Sheet, active: &[u8], budget: &'b Budget<'b>) -> Result<MVec<'b, u8>, Error> {
    let comps = sheet.compounds.as_slice();
    let sels = sheet.selectors.as_slice();
    let simples = sheet.simples.as_slice();
    let mut live: MVec<'b, u8> = MVec::with_capacity(budget, comps.len())?;
    live.resize_to(comps.len(), 0)?;
    let mark_list = |live: &mut MVec<'b, u8>, mut sel: u32| -> Result<(), Error> {
        while sel != NONE {
            budget.spend(cost::OP_STEP)?;
            let Some(s) = sels.get(sel as usize) else { break };
            if let Some(x) = live.get_mut(s.subject as usize) {
                *x = 1;
            }
            sel = s.next;
        }
        Ok(())
    };
    for (r, rule) in sheet.rules.as_slice().iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        if active.get(r).copied().unwrap_or(0) != 0 {
            mark_list(&mut live, rule.selector)?;
        }
    }
    for k in (0..comps.len()).rev() {
        budget.spend(cost::OP_STEP)?;
        if live.get(k).copied() != Some(1) {
            continue;
        }
        let Some(c) = comps.get(k) else { continue };
        if let Some(x) = live.get_mut(c.prev as usize) {
            *x = 1;
        }
        let mut sp = c.first;
        while sp != NONE {
            budget.spend(cost::OP_STEP)?;
            let Some(x) = simples.get(sp as usize) else { break };
            if x.list != NONE {
                mark_list(&mut live, x.list)?;
            }
            sp = x.next;
        }
    }
    Ok(live)
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 旗 `k` の語の番号と、語の中のビット。
fn word_bit(k: usize) -> (usize, u64) {
    let w = k.checked_div(64).unwrap_or(0);
    let r = k.checked_rem(64).unwrap_or(0) as u32;
    (w, 1u64.checked_shl(r).unwrap_or(0))
}

fn bit(v: &[u64], base: usize, k: usize) -> bool {
    let (w, m) = word_bit(k);
    v.get(base.saturating_add(w)).is_some_and(|x| x & m != 0)
}

struct Ctx<'a, 'b> {
    doc: &'a XmlDoc,
    input: &'a [u8],
    sheet: &'a Sheet,
    sib: Option<&'a [Sib]>,
    budget: &'b Budget<'b>,
}

impl Ctx<'_, '_> {
    /// 複合の単純な選択子がすべて当たるか（`first` から `next` でたどる。単純な選択子ごとに払う）。
    fn compound(&self, e: u32, n: &Node, first: u32, info: &Elem, newm: &[u64]) -> Result<bool, Error> {
        let mut sp = first;
        while sp != NONE {
            self.budget.spend(cost::OP_STEP)?;
            let Some(s) = self.sheet.simples.as_slice().get(sp as usize) else { return Ok(false) };
            if !self.simple(e, n, s, info, newm)? {
                return Ok(false);
            }
            sp = s.next;
        }
        Ok(true)
    }

    fn name(&self, s: &Simple) -> &[u8] {
        s.a.of(self.sheet.names.as_slice())
    }

    /// 属性の番号 `k` の値（NONE なら None）。
    fn attr_value(&self, k: u32) -> Option<&[u8]> {
        let a = self.doc.attrs.as_slice().get(k as usize)?;
        Some(self.doc.value(self.input, a))
    }

    fn simple(&self, e: u32, n: &Node, s: &Simple, info: &Elem, newm: &[u64]) -> Result<bool, Error> {
        let input = self.input;
        let ns_ok = |ns: u8| s.ns == NS_ANY || s.ns == ns;
        let sib = || self.sib.and_then(|v| v.get(e as usize)).copied().unwrap_or_default();
        Ok(match s.kind {
            S_TYPE => ns_ok(n.ns) && bytes_eq(self.budget, n.local.of(input), self.name(s))?,
            S_UNIVERSAL => ns_ok(n.ns),
            S_ID => match self.attr_value(info.id) {
                Some(v) => bytes_eq(self.budget, v, self.name(s))?,
                None => false,
            },
            S_CLASS => {
                let v = self.attr_value(info.class).unwrap_or(&[]);
                s.flags & FLAG_NO_TOKEN == 0 && has_token(self.budget, v, self.name(s), false)?
            }
            S_ATTR => self.attr(n, s)?,
            S_FIRST_CHILD => sib().index == 1,
            S_LAST_CHILD => {
                let x = sib();
                x.index == x.count
            }
            S_ONLY_CHILD => sib().count == 1,
            S_NTH_CHILD => nth(s.n_a, s.n_b, sib().index),
            S_NTH_LAST_CHILD => {
                let x = sib();
                nth(s.n_a, s.n_b, x.count.saturating_sub(x.index).saturating_add(1))
            }
            S_FIRST_OF_TYPE => sib().type_index == 1,
            S_LAST_OF_TYPE => {
                let x = sib();
                x.type_index == x.type_count
            }
            S_ONLY_OF_TYPE => sib().type_count == 1,
            S_NTH_OF_TYPE => nth(s.n_a, s.n_b, sib().type_index),
            S_NTH_LAST_OF_TYPE => {
                let x = sib();
                nth(s.n_a, s.n_b, x.type_count.saturating_sub(x.type_index).saturating_add(1))
            }
            S_ROOT => n.parent == NONE,
            S_EMPTY => info.empty,
            S_LINK => info.link,
            S_NOT | S_IS => {
                let mut any = false;
                let mut sel = s.list;
                while sel != NONE {
                    self.budget.spend(cost::OP_STEP)?;
                    let Some(x) = self.sheet.selectors.as_slice().get(sel as usize) else { break };
                    if bit(newm, 0, x.subject as usize) {
                        any = true;
                        break;
                    }
                    sel = x.next;
                }
                if s.kind == S_NOT { !any } else { any }
            }
            S_LANG => {
                let lang = self.attr_value(info.lang).unwrap_or(&[]);
                !lang.is_empty() && lang_in_ranges(self.budget, lang, self.name(s))?
            }
            S_NEVER => false,
            _ => false,
        })
    }

    fn attr(&self, n: &Node, s: &Simple) -> Result<bool, Error> {
        let doc = self.doc;
        let input = self.input;
        let a = self.name(s);
        let want = s.b.of(self.sheet.names.as_slice());
        let ci = s.flags & FLAG_CASE_I != 0;
        let budget = self.budget;
        // 比べるバイトは塊ずつ払う
        let eq = |x: &[u8], y: &[u8]| if ci { bytes_eq_ignore_case(budget, x, y) } else { bytes_eq(budget, x, y) };
        for at in doc.attrs_of(n) {
            budget.spend(cost::OP_STEP)?;
            if !(s.ns == NS_ANY || s.ns == at.ns) || !bytes_eq(budget, at.local.of(input), a)? {
                continue;
            }
            let v = doc.value(input, at);
            let ok = match s.op {
                OP_EXISTS => true,
                OP_EQ => eq(v, want)?,
                OP_INCLUDES => s.flags & FLAG_NO_TOKEN == 0 && has_token(budget, v, want, ci)?,
                OP_DASH => {
                    eq(v, want)?
                        || (v.len() > want.len()
                            && v.get(want.len()) == Some(&b'-')
                            && eq(v.get(..want.len()).unwrap_or(&[]), want)?)
                }
                OP_PREFIX => !want.is_empty() && v.len() >= want.len() && eq(v.get(..want.len()).unwrap_or(&[]), want)?,
                OP_SUFFIX => {
                    !want.is_empty()
                        && v.len() >= want.len()
                        && eq(v.get(v.len().saturating_sub(want.len())..).unwrap_or(&[]), want)?
                }
                OP_SUBSTR => {
                    let mut found = false;
                    if !want.is_empty() && v.len() >= want.len() {
                        for k in 0..=v.len().saturating_sub(want.len()) {
                            if eq(v.get(k..k.saturating_add(want.len())).unwrap_or(&[]), want)? {
                                found = true;
                                break;
                            }
                        }
                    }
                    found
                }
                _ => false,
            };
            if ok {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// 空白で区切った語に `want` があるか（空白は CSS の空白。`want` が空か空白を含むものは、呼び手が `FLAG_NO_TOKEN` で
/// 先に除く）。読むバイトは塊ずつ払う。
fn has_token(budget: &Budget<'_>, v: &[u8], want: &[u8], ci: bool) -> Result<bool, Error> {
    let ws = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0C);
    if want.is_empty() || want.len() > v.len() {
        budget.spend(1)?;
        return Ok(false);
    }
    let mut meter = ScanMeter::new(0, v.len());
    let mut start = 0usize;
    for i in 0..=v.len() {
        meter.pay(budget, i)?;
        if v.get(i).is_none_or(|&c| ws(c)) {
            let w = v.get(start..i).unwrap_or(&[]);
            let hit = if ci { bytes_eq_ignore_case(budget, w, want)? } else { bytes_eq(budget, w, want)? };
            if hit {
                return Ok(true);
            }
            start = i.saturating_add(1);
        }
    }
    Ok(false)
}

/// An+B に 1 から数えた位置 `i` が当たるか（ある n ≥ 0 で a·n + b = i）。
fn nth(a: i32, b: i32, i: u32) -> bool {
    let i = i64::from(i);
    let (a, b) = (i64::from(a), i64::from(b));
    if a == 0 {
        return i == b;
    }
    let diff = i.saturating_sub(b);
    // n = diff / a が 0 以上の整数
    diff.checked_rem(a) == Some(0) && diff.checked_div(a).is_some_and(|q| q >= 0)
}

/// 言語 `lang` が NUL で区切った範囲（小文字。`LANG_SEP`）のどれかに当たるか（RFC 4647 §3.3.2 の拡張の照らし：範囲の頭の部分が `*` か
/// 言語の頭と同じで、範囲の残りの部分がそれぞれ、言語のあとの部分に順に現れる。1 文字の部分（`x` など）は飛び越えない。
/// 大文字小文字を問わない）。範囲のバイトは塊ずつ払って読み、比べるバイトも払う。
fn lang_in_ranges(budget: &Budget<'_>, lang: &[u8], ranges: &[u8]) -> Result<bool, Error> {
    let mut meter = ScanMeter::new(0, ranges.len());
    let mut start = 0usize;
    for i in 0..=ranges.len() {
        meter.pay(budget, i)?;
        if ranges.get(i).is_some_and(|&c| c != crate::css::select::LANG_SEP) {
            continue;
        }
        let r = ranges.get(start..i).unwrap_or(&[]);
        start = i.saturating_add(1);
        if !r.is_empty() && extended_match(budget, lang, r)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 照らす言語の長さの上限（BCP 47 の言語の札はこれよりずっと短い。長いものは当たらないものとする）。
const MAX_LANG: usize = 4096;

/// RFC 4647 §3.3.2 の拡張の照らし（1 つの範囲）。部分に分けて読む言語と範囲のバイトを払う。
fn extended_match(budget: &Budget<'_>, lang: &[u8], range: &[u8]) -> Result<bool, Error> {
    if lang.len() > MAX_LANG || range.len() > MAX_LANG {
        budget.spend(1)?;
        return Ok(false);
    }
    budget.spend(cost::scan(lang.len().saturating_add(range.len()) as u64))?;
    let mut rs = range.split(|&c| c == b'-');
    let mut ls = lang.split(|&c| c == b'-');
    let (Some(r0), Some(l0)) = (rs.next(), ls.next()) else { return Ok(false) };
    if r0 != b"*" && !bytes_eq_ignore_case(budget, r0, l0)? {
        return Ok(false);
    }
    let mut lnext = ls.next();
    for r in rs {
        budget.spend(cost::OP_STEP)?;
        if r == b"*" {
            continue;
        }
        loop {
            budget.spend(cost::OP_STEP)?;
            let Some(l) = lnext else { return Ok(false) };
            if bytes_eq_ignore_case(budget, r, l)? {
                lnext = ls.next();
                break;
            }
            // 1 文字の部分は飛び越えない
            if l.len() == 1 {
                return Ok(false);
            }
            lnext = ls.next();
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
