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
//! 手間は「複合の数 ＋ 単純な選択子を照らす手間」で、後戻りがない。後ろの兄弟を見る擬似クラス（`:last-child` など）は、
//! 照合の前に兄弟の数を数えておく。確保は（深さ ＋ 1）× 複合の数のビットの 3 組と、兄弟の数の表（使うときだけ）。
//!
//! 要素の名前空間は種類（SVG・XLink・XML・なし・そのほか）だけを持つので、`@namespace` の「そのほか」の URI は区別しない
//! （ADR-66 (1)）。どの名前空間の要素も兄弟と親子に数える（照らしの `first-child-foreign`・`adjacent-foreign` と同じ）。

use crate::budget::Budget;
use crate::cost;
use crate::css::select::{
    COMB_CHILD, COMB_DESC, COMB_LATER, COMB_NEXT, FLAG_CASE_I, NONE, NS_ANY, OP_DASH, OP_EQ, OP_EXISTS, OP_INCLUDES,
    OP_PREFIX, OP_SUBSTR, OP_SUFFIX, S_ATTR, S_CLASS, S_EMPTY, S_FIRST_CHILD, S_FIRST_OF_TYPE, S_ID, S_IS, S_LANG,
    S_LAST_CHILD, S_LAST_OF_TYPE, S_LINK, S_NEVER, S_NOT, S_NTH_CHILD, S_NTH_LAST_CHILD, S_NTH_LAST_OF_TYPE,
    S_NTH_OF_TYPE, S_ONLY_CHILD, S_ONLY_OF_TYPE, S_ROOT, S_TYPE, S_UNIVERSAL, Simple,
};
use crate::css::sheet::Sheet;
use crate::error::Error;
use crate::mem::{MVec, Plain, stable_sort};
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

/// どの構造の擬似クラスを使うか。
fn uses(sheet: &Sheet) -> (bool, bool) {
    let mut sib = false;
    let mut ty = false;
    for s in sheet.simples.as_slice() {
        match s.kind {
            S_LAST_CHILD | S_ONLY_CHILD | S_NTH_LAST_CHILD | S_NTH_CHILD | S_FIRST_CHILD => sib = true,
            S_FIRST_OF_TYPE | S_LAST_OF_TYPE | S_ONLY_OF_TYPE | S_NTH_OF_TYPE | S_NTH_LAST_OF_TYPE => {
                sib = true;
                ty = true;
            }
            _ => {}
        }
    }
    (sib, ty)
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
            let bytes: u64 =
                kids.as_slice().iter().fold(0u64, |a, id| a.saturating_add((key(id).1.len() as u64).saturating_add(1)));
            budget.spend_large(cost::scan(bytes).saturating_mul(cost::ceil_log2(kids.len() as u64)))?;
            stable_sort(budget, kids.as_mut_slice(), |x, y| key(x) < key(y))?;
            let ks = kids.as_slice();
            let mut i = 0usize;
            while i < ks.len() {
                let mut j = i;
                while j < ks.len() && ks.get(j).map(key) == ks.get(i).map(key) {
                    budget.spend(cost::OP_STEP)?;
                    j = j.saturating_add(1);
                }
                let run = (j.saturating_sub(i)) as u32;
                for (k, id) in ks.get(i..j).unwrap_or(&[]).iter().enumerate() {
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
    /// 言語（xml:lang か lang の値の範囲。受け継ぐ。なければ空で lang_set 0）
    lang_node: u32,
    pad: u32,
}

impl Plain for Level {}

/// 照合する。`active[r]` が 0 の規則は報告しない（条件が偽）。
pub(crate) fn match_rules(
    doc: &XmlDoc,
    input: &[u8],
    sheet: &Sheet,
    active: &[u8],
    budget: &Budget<'_>,
    sink: &mut dyn MatchSink,
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
    let ncomp = sheet.compounds.len();
    let words = ncomp.saturating_add(63).checked_div(64).unwrap_or(0);
    let (use_sib, use_type) = uses(sheet);
    let sib = if use_sib { Some(siblings(doc, input, use_type, budget)?) } else { None };
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
    levels.resize_to(max_depth.saturating_add(1), Level { node: NONE, last_parent: NONE, lang_node: NONE, pad: 0 })?;
    let mut path: MVec<'_, u32> = MVec::with_capacity(budget, max_depth.saturating_add(1))?;
    let ctx = Ctx { doc, input, sheet, sib: sib.as_ref().map(MVec::as_slice), budget };
    for (i, n) in nodes.iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        if n.kind != KIND_ELEMENT {
            continue;
        }
        let e = i as u32;
        while path.last().is_some_and(|&t| t != n.parent) {
            path.pop();
        }
        let d = path.len();
        path.push(e)?;
        let same_parent = levels.get(d).is_some_and(|l| l.last_parent == n.parent && l.node != NONE);
        let row = d.saturating_mul(words);
        let prow = d.saturating_sub(1).saturating_mul(words);
        if !same_parent && let Some(w) = later.as_mut_slice().get_mut(row..row.saturating_add(words)) {
            w.fill(0);
        }
        budget.spend(cost::fill((words as u64).saturating_mul(8)).saturating_mul(4))?;
        // 言語：自分の xml:lang・lang か、親から受け継ぐ
        let parent_lang = if d > 0 { levels.get(d.saturating_sub(1)).map_or(NONE, |l| l.lang_node) } else { NONE };
        let lang_node = if lang_of(doc, input, n).is_some() { e } else { parent_lang };
        // 旗を番号の順に決める
        if let Some(w) = newm.as_mut_slice().get_mut(..) {
            w.fill(0);
        }
        for k in 0..ncomp {
            let Some(c) = sheet.compounds.as_slice().get(k) else { continue };
            let ok = ctx.compound(e, n, c.first, lang_node, newm.as_slice())?;
            if !ok {
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
            let (wi, m) = word_bit(k);
            if prev_ok && let Some(w) = newm.as_mut_slice().get_mut(wi) {
                *w |= m;
            }
        }
        // 深さ d の旗を書き換える
        let nm = newm.as_slice();
        for j in 0..words {
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
        }
        if let Some(l) = levels.get_mut(d) {
            *l = Level { node: e, last_parent: n.parent, lang_node, pad: 0 };
        }
        // 当たった規則を知らせる
        sink.element(e)?;
        for (r, rule) in sheet.rules.as_slice().iter().enumerate() {
            budget.spend(cost::OP_STEP)?;
            if active.get(r).copied().unwrap_or(0) == 0 {
                continue;
            }
            let mut best: Option<u32> = None;
            let mut sel = rule.selector;
            while sel != NONE {
                budget.spend(cost::OP_STEP)?;
                let Some(s) = sheet.selectors.as_slice().get(sel as usize) else { break };
                if bit(nm, 0, s.subject as usize) {
                    best = Some(best.map_or(s.spec, |b| b.max(s.spec)));
                }
                sel = s.next;
            }
            if let Some(spec) = best {
                sink.rule(r as u32, spec)?;
            }
        }
    }
    Ok(())
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

/// 要素の言語の属性（xml:lang、なければ名前空間のない lang）の値。
fn lang_of<'a>(doc: &'a XmlDoc, input: &'a [u8], n: &Node) -> Option<&'a [u8]> {
    doc.attr(input, n, NS_XML, b"lang").or_else(|| doc.attr(input, n, NS_NONE, b"lang"))
}

struct Ctx<'a, 'b> {
    doc: &'a XmlDoc,
    input: &'a [u8],
    sheet: &'a Sheet,
    sib: Option<&'a [Sib]>,
    budget: &'b Budget<'b>,
}

impl Ctx<'_, '_> {
    /// 名前のバイトを比べる手間。
    fn pay(&self, bytes: usize) -> Result<(), Error> {
        self.budget.spend_large(cost::copy(bytes as u64).saturating_add(cost::OP_STEP))
    }

    /// 複合の単純な選択子がすべて当たるか（`first` から `next` でたどる）。
    fn compound(&self, e: u32, n: &Node, first: u32, lang_node: u32, newm: &[u64]) -> Result<bool, Error> {
        let mut sp = first;
        while sp != NONE {
            let Some(s) = self.sheet.simples.as_slice().get(sp as usize) else { return Ok(false) };
            if !self.simple(e, n, s, lang_node, newm)? {
                return Ok(false);
            }
            sp = s.next;
        }
        Ok(true)
    }

    fn name(&self, s: &Simple) -> &[u8] {
        s.a.of(self.sheet.names.as_slice())
    }

    fn simple(&self, e: u32, n: &Node, s: &Simple, lang_node: u32, newm: &[u64]) -> Result<bool, Error> {
        let doc = self.doc;
        let input = self.input;
        let ns_ok = |ns: u8| s.ns == NS_ANY || s.ns == ns;
        let sib = || self.sib.and_then(|v| v.get(e as usize)).copied().unwrap_or_default();
        Ok(match s.kind {
            S_TYPE => {
                let a = self.name(s);
                self.pay(a.len())?;
                ns_ok(n.ns) && n.local.of(input) == a
            }
            S_UNIVERSAL => ns_ok(n.ns),
            S_ID => {
                let a = self.name(s);
                self.pay(a.len().saturating_add(doc.attrs_of(n).len()))?;
                doc.attr(input, n, NS_NONE, b"id") == Some(a)
            }
            S_CLASS => {
                let a = self.name(s);
                let v = doc.attr(input, n, NS_NONE, b"class").unwrap_or(&[]);
                self.pay(v.len().saturating_add(doc.attrs_of(n).len()))?;
                has_token(v, a, false)
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
            S_EMPTY => n.first_child == NONE,
            S_LINK => {
                n.ns == NS_SVG
                    && n.local.of(input) == b"a"
                    && (doc.attr(input, n, NS_NONE, b"href").is_some()
                        || doc.attr(input, n, NS_XLINK, b"href").is_some())
            }
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
                let lang = doc.node(lang_node).and_then(|ln| lang_of(doc, input, ln)).unwrap_or(&[]);
                let ranges = self.name(s);
                self.pay(ranges.len().saturating_add(lang.len()))?;
                !lang.is_empty() && ranges.split(|&c| c == b',').any(|r| lang_matches(lang, r))
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
        for at in doc.attrs_of(n) {
            self.pay(a.len())?;
            if !(s.ns == NS_ANY || s.ns == at.ns) || at.local.of(input) != a {
                continue;
            }
            let v = doc.value(input, at);
            self.pay(v.len().saturating_add(want.len()))?;
            let eq = |x: &[u8], y: &[u8]| if ci { x.eq_ignore_ascii_case(y) } else { x == y };
            let ok = match s.op {
                OP_EXISTS => true,
                OP_EQ => eq(v, want),
                OP_INCLUDES => has_token(v, want, ci),
                OP_DASH => {
                    eq(v, want)
                        || (v.len() > want.len()
                            && v.get(want.len()) == Some(&b'-')
                            && eq(v.get(..want.len()).unwrap_or(&[]), want))
                }
                OP_PREFIX => !want.is_empty() && v.len() >= want.len() && eq(v.get(..want.len()).unwrap_or(&[]), want),
                OP_SUFFIX => {
                    !want.is_empty()
                        && v.len() >= want.len()
                        && eq(v.get(v.len().saturating_sub(want.len())..).unwrap_or(&[]), want)
                }
                OP_SUBSTR => {
                    !want.is_empty()
                        && (0..=v.len().saturating_sub(want.len()))
                            .any(|k| v.get(k..k.saturating_add(want.len())).is_some_and(|w| eq(w, want)))
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

/// 空白で区切った語に `want` があるか（空白は CSS の空白。`want` が空か空白を含めば当たらない）。
fn has_token(v: &[u8], want: &[u8], ci: bool) -> bool {
    if want.is_empty() || want.iter().any(|&c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)) {
        return false;
    }
    v.split(|&c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0C))
        .any(|w| if ci { w.eq_ignore_ascii_case(want) } else { w == want })
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

/// 言語 `lang` が範囲 `range`（小文字）に当たるか（同じか、`range` のあとが `-`。大文字小文字を問わない）。
fn lang_matches(lang: &[u8], range: &[u8]) -> bool {
    if range.is_empty() {
        return false;
    }
    if range == b"*" {
        return true;
    }
    lang.len() >= range.len()
        && lang.get(..range.len()).is_some_and(|p| p.eq_ignore_ascii_case(range))
        && (lang.len() == range.len() || lang.get(range.len()) == Some(&b'-'))
}

#[cfg(test)]
mod tests;
