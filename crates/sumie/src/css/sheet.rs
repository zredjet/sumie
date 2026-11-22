//! 規則の並び（CSS Syntax Level 3 §5.4.1・CSS Cascade 5・Conditional Rules。FR-ST-06〜09・ADR-66 (2)(3)・ADR-73）。
//!
//! 文書のすべての `<style>` を文書の順に 1 つの [`Sheet`] に読む：
//!
//! - 規則：選択子の並び（[`crate::css::select`]）と宣言の範囲・条件（`@media`・`@supports`・`<style media>`）・層・順
//! - 条件は読むだけで照らさない（`@media` は文書の大きさが、`@supports` はプロパティの表が要るので、style の段で照らす）。
//!   条件は外の条件を `parent` で指す
//! - 層（`@layer`）は、書いた所ごとの節（名前の 1 段・外の層・条件）として読み、同じ名前の層を [`Layer::canon`] でまとめる。
//!   順位は条件を照らしたあとに [`Sheet::layer_ranks`] で決める（偽の条件の中で初めて書いた名前は順に入らない）
//! - `@namespace` は `<style>` ごと。正しい規則（`@charset`・`@import`・`@layer` の文を除く）より前にだけ書ける。`@import`
//!   は、正しい規則（`@charset`・`@layer` の文を除く）より前にだけ書ける。壊れた規則と知らないアットルールは数えない（Chrome
//!   と同じ）
//! - `@import`・`@font-face` の `src` の URL は読まずに、読まなかった参照に載せる（FR-IN-08。escape を解いた形）。ほかの
//!   知らないアットルールは飛ばして警告する
//! - 壊れたセレクタの規則は、宣言の塊ごと捨てて警告する（CSS の仕様）。当てないセレクタ（`:has()` など）を捨てた規則と、
//!   規則の中の入れ子の規則も警告する
//!
//! 数える段（`estimate_parse`）は同じ読み方で、容量を伸ばしながら読んで量を測る。読む段は測った量で最初に容量を取る。

use crate::budget::Budget;
use crate::cost;
use crate::css::decl::{Decl, Item, parse_declarations};
use crate::css::lex::{Kind, Lexer, Nest, Token, decode, decode_string, ident_is};
use crate::css::select::{NONE, NsDecl, NsTable, Tables, ns_of_uri, parse_selector_list};
use crate::error::Error;
use crate::mem::{Frozen, MVec, Plain, bytes_cmp, stable_sort_try};
use crate::report::WarningKind;
use crate::xml::lex::Span;

/// 規則。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rule {
    /// 並びの最初の選択子（当てる選択子がなければ NONE）
    pub(crate) selector: u32,
    /// 宣言の番号の範囲
    pub(crate) decls: Span,
    /// いちばん内の条件（なければ NONE）
    pub(crate) cond: u32,
    /// いちばん内の層の節（[`Sheet::layers`] の番号。なければ NONE）
    pub(crate) layer: u32,
    /// 文書の中の順
    pub(crate) order: u32,
    /// `<style>` 要素の入力の位置
    pub(crate) pos: u32,
}

impl Plain for Rule {}

/// 条件の種類。
pub(crate) const COND_MEDIA: u8 = 0;
pub(crate) const COND_SUPPORTS: u8 = 1;
/// `@import` の `supports(…)`（`<supports-condition>` か 1 つの宣言。CSS Cascade 5 §2.1）
pub(crate) const COND_SUPPORTS_IMPORT: u8 = 2;

/// 条件（`@media` の問いの並び・`@supports` の条件・`<style media>`）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cond {
    /// 条件の字句の範囲（`text` の中）
    pub(crate) text: Span,
    /// 外の条件
    pub(crate) parent: u32,
    /// `@supports selector()` に使う `@namespace` の表（`ns` の中の範囲）
    pub(crate) ns: Span,
    pub(crate) kind: u8,
    pub(crate) pad: [u8; 3],
}

impl Plain for Cond {}

/// 層を書いた所（`@layer a.b` は a と b の 2 つの節）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layer {
    /// 名前の 1 段（`names` の中。名前のない層は空）
    pub(crate) name: Span,
    /// 外の層の節（なければ NONE）
    pub(crate) parent: u32,
    /// 書いた所のいちばん内の条件（なければ NONE）
    pub(crate) cond: u32,
    /// 同じ層（外の層と名前が同じ）の節に共通の番号（`finish` で決める。名前のない層はそれぞれ別）
    pub(crate) canon: u32,
    pub(crate) anonymous: u8,
    pub(crate) pad: [u8; 3],
}

impl Plain for Layer {}

/// 後で報告に移す覚え（警告か、読まなかった参照）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Note {
    /// 読まなかった参照なら href（escape を解いたもの。`names` の中）。警告なら空
    pub(crate) href: Span,
    pub(crate) pos: u32,
    pub(crate) unread: u8,
    pub(crate) pad: u8,
    pub(crate) kind: WarningKind,
}

impl Plain for Note {}

/// 読んだ規則の並び（文書のすべての `<style>`）。
#[derive(Debug)]
pub(crate) struct Sheet {
    pub(crate) text: Frozen<u8>,
    pub(crate) names: Frozen<u8>,
    pub(crate) rules: Frozen<Rule>,
    pub(crate) decls: Frozen<Decl>,
    pub(crate) selectors: Frozen<crate::css::select::Selector>,
    pub(crate) compounds: Frozen<crate::css::select::Compound>,
    pub(crate) simples: Frozen<crate::css::select::Simple>,
    pub(crate) conds: Frozen<Cond>,
    pub(crate) layers: Frozen<Layer>,
    /// まとめた層ごとの外の層（まとめた番号。なければ NONE）
    pub(crate) canon_parents: Frozen<u32>,
    /// `<style>` ごとの `@namespace` の表（接頭辞の順。接頭辞は `ns_names` の中）
    pub(crate) ns: Frozen<NsDecl>,
    pub(crate) ns_names: Frozen<u8>,
    pub(crate) notes: Frozen<Note>,
}

impl Sheet {
    /// 持ち続ける量。
    pub(crate) fn bytes(&self) -> u64 {
        [
            self.text.bytes(),
            self.names.bytes(),
            self.rules.bytes(),
            self.decls.bytes(),
            self.selectors.bytes(),
            self.compounds.bytes(),
            self.simples.bytes(),
            self.conds.bytes(),
            self.layers.bytes(),
            self.canon_parents.bytes(),
            self.ns.bytes(),
            self.ns_names.bytes(),
            self.notes.bytes(),
        ]
        .iter()
        .fold(0u64, |a, &b| a.saturating_add(b))
    }

    /// 条件 `c` の `@namespace` の表。
    pub(crate) fn ns_table(&self, c: &Cond) -> NsTable<'_> {
        NsTable {
            decls: self.ns.as_slice().get(c.ns.start as usize..c.ns.end as usize).unwrap_or(&[]),
            tail: &[],
            names: self.ns_names.as_slice(),
        }
    }

    /// まとめた層ごとの順位（小さいほど弱い）。`cond_ok[c]` は条件 `c` が外の条件まで含めて真か。層の順は、真の条件の
    /// 所で名前を初めて書いた順（CSS Cascade 5 §6.4.3）。子の層はその順に並び、層の中の規則（層そのもの）は子の層より強い
    /// ので、帰りがけの順に番号を振る。真の所で書いていない層の順位は 0（その層の規則はどれも当たらない）。
    pub(crate) fn layer_ranks<'b>(&self, cond_ok: &[u8], budget: &'b Budget<'b>) -> Result<MVec<'b, u32>, Error> {
        let layers = self.layers.as_slice();
        let parents = self.canon_parents.as_slice();
        let n = parents.len();
        let active = |c: u32| c == NONE || cond_ok.get(c as usize).copied() == Some(1);
        // まとめた層ごとに、真の所で初めて書いた節
        let mut first: MVec<'b, u32> = MVec::with_capacity(budget, n)?;
        first.resize_to(n, NONE)?;
        // 子の数（外の層ごと。いちばん外は n の所）と、子の並び（初めて書いた順）
        let mut count: MVec<'b, u32> = MVec::with_capacity(budget, n.saturating_add(2))?;
        count.resize_to(n.saturating_add(2), 0)?;
        for (i, l) in layers.iter().enumerate() {
            budget.spend(cost::OP_STEP)?;
            if !active(l.cond) {
                continue;
            }
            if let Some(f) = first.get_mut(l.canon as usize)
                && *f == NONE
            {
                *f = i as u32;
                let p = parents.get(l.canon as usize).copied().unwrap_or(NONE);
                let slot = if p == NONE { n } else { p as usize };
                if let Some(c) = count.get_mut(slot.saturating_add(1)) {
                    *c = c.saturating_add(1);
                }
            }
        }
        // 始まりの位置（count を足し上げる）
        for k in 1..count.len() {
            budget.spend(cost::OP_STEP)?;
            let prev = count.get(k.saturating_sub(1)).copied().unwrap_or(0);
            if let Some(c) = count.get_mut(k) {
                *c = c.saturating_add(prev);
            }
        }
        let mut kids: MVec<'b, u32> = MVec::with_capacity(budget, n)?;
        kids.resize_to(n, NONE)?;
        let mut fill: MVec<'b, u32> = MVec::with_capacity(budget, n.saturating_add(1))?;
        fill.append_slice(count.as_slice().get(..n.saturating_add(1)).unwrap_or(&[]))?;
        for (i, l) in layers.iter().enumerate() {
            budget.spend(cost::OP_STEP)?;
            if first.get(l.canon as usize).copied() != Some(i as u32) {
                continue;
            }
            let p = parents.get(l.canon as usize).copied().unwrap_or(NONE);
            let slot = if p == NONE { n } else { p as usize };
            if let Some(at) = fill.get_mut(slot) {
                if let Some(k) = kids.get_mut(*at as usize) {
                    *k = l.canon;
                }
                *at = at.saturating_add(1);
            }
        }
        // 帰りがけの順に番号を振る（明示の積み上げ：（層, 次に見る子の位置））
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Visit {
            node: u32,
            next: u32,
        }
        impl Plain for Visit {}
        let mut ranks: MVec<'b, u32> = MVec::with_capacity(budget, n)?;
        ranks.resize_to(n, 0)?;
        let range = |slot: usize| {
            let a = count.get(slot).copied().unwrap_or(0);
            let b = count.get(slot.saturating_add(1)).copied().unwrap_or(a);
            (a, b)
        };
        let mut stack: MVec<'_, Visit> = MVec::new(budget);
        stack.push(Visit { node: NONE, next: range(n).0 })?;
        let mut rank = 0u32;
        while let Some(top) = stack.last().copied() {
            budget.spend(cost::OP_STEP)?;
            let slot = if top.node == NONE { n } else { top.node as usize };
            let (_, end) = range(slot);
            if top.next < end {
                if let Some(t) = stack.last_mut() {
                    t.next = top.next.saturating_add(1);
                }
                let c = kids.get(top.next as usize).copied().unwrap_or(NONE);
                if c != NONE {
                    stack.push(Visit { node: c, next: range(c as usize).0 })?;
                }
                continue;
            }
            stack.pop();
            if top.node != NONE
                && let Some(r) = ranks.get_mut(top.node as usize)
            {
                *r = rank;
                rank = rank.saturating_add(1);
            }
        }
        Ok(ranks)
    }
}

/// 数える段で測る量（読む段の容量）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SheetCounts {
    pub(crate) text: u64,
    pub(crate) names: u64,
    pub(crate) rules: u64,
    pub(crate) decls: u64,
    pub(crate) selectors: u64,
    pub(crate) compounds: u64,
    pub(crate) simples: u64,
    pub(crate) conds: u64,
    pub(crate) layers: u64,
    pub(crate) ns: u64,
    pub(crate) ns_names: u64,
    pub(crate) notes: u64,
}

/// 規則の並びを作る口（`<style>` を文書の順に足す）。
pub(crate) struct SheetBuilder<'b> {
    budget: &'b Budget<'b>,
    text: MVec<'b, u8>,
    tables: Tables<'b>,
    rules: MVec<'b, Rule>,
    decls: MVec<'b, Decl>,
    conds: MVec<'b, Cond>,
    layers: MVec<'b, Layer>,
    ns: MVec<'b, NsDecl>,
    /// `ns` の長さの山（同じ接頭辞を除く前。読む段の容量にする：2 度目の読みで伸び直さない）
    ns_peak: usize,
    ns_names: MVec<'b, u8>,
    notes: MVec<'b, Note>,
    order: u32,
}

/// アットルールを読んだあと。
enum Step {
    Done,
    /// 中に規則を読む塊を開いた
    Open(Block),
    /// 入れ子の塊を閉じる `}` で止まった
    Close,
}

/// 入れ子の塊（`@media {…}`・`@supports {…}`・`@layer {…}`）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Block {
    cond: u32,
    layer: u32,
}

impl Plain for Block {}

/// 規則の並びの頭で、まだ書けるもの（`<style>` ごと）。
#[derive(Clone, Copy)]
struct Head {
    may_import: bool,
    may_namespace: bool,
}

impl<'b> SheetBuilder<'b> {
    /// 容量を伸ばしながら読む（数える段）。
    pub(crate) fn new(budget: &'b Budget<'b>) -> SheetBuilder<'b> {
        SheetBuilder {
            budget,
            text: MVec::new(budget),
            tables: Tables::new(budget),
            rules: MVec::new(budget),
            decls: MVec::new(budget),
            conds: MVec::new(budget),
            layers: MVec::new(budget),
            ns: MVec::new(budget),
            ns_peak: 0,
            ns_names: MVec::new(budget),
            notes: MVec::new(budget),
            order: 0,
        }
    }

    /// 数える段で測った量で容量を取る（読む段）。
    pub(crate) fn with_counts(budget: &'b Budget<'b>, c: &SheetCounts) -> Result<SheetBuilder<'b>, Error> {
        let cap = |n: u64| usize::try_from(n).map_err(|_| budget.too_large());
        Ok(SheetBuilder {
            budget,
            text: MVec::with_capacity(budget, cap(c.text)?)?,
            tables: Tables {
                names: MVec::with_capacity(budget, cap(c.names)?)?,
                selectors: MVec::with_capacity(budget, cap(c.selectors)?)?,
                compounds: MVec::with_capacity(budget, cap(c.compounds)?)?,
                simples: MVec::with_capacity(budget, cap(c.simples)?)?,
            },
            rules: MVec::with_capacity(budget, cap(c.rules)?)?,
            decls: MVec::with_capacity(budget, cap(c.decls)?)?,
            conds: MVec::with_capacity(budget, cap(c.conds)?)?,
            layers: MVec::with_capacity(budget, cap(c.layers)?)?,
            ns: MVec::with_capacity(budget, cap(c.ns)?)?,
            ns_peak: 0,
            ns_names: MVec::with_capacity(budget, cap(c.ns_names)?)?,
            notes: MVec::with_capacity(budget, cap(c.notes)?)?,
            order: 0,
        })
    }

    /// ここまでの量。
    pub(crate) fn counts(&self) -> SheetCounts {
        let n = |x: usize| x as u64;
        SheetCounts {
            text: n(self.text.len()),
            names: n(self.tables.names.len()),
            rules: n(self.rules.len()),
            decls: n(self.decls.len()),
            selectors: n(self.tables.selectors.len()),
            compounds: n(self.tables.compounds.len()),
            simples: n(self.tables.simples.len()),
            conds: n(self.conds.len()),
            layers: n(self.layers.len()),
            ns: n(self.ns_peak.max(self.ns.len())),
            ns_names: n(self.ns_names.len()),
            notes: n(self.notes.len()),
        }
    }

    /// `<style>` 1 つ（`css` は揃えた中身、`media` は `media` 属性の値、`pos` は要素の位置）を足す。
    pub(crate) fn add(&mut self, css: &[u8], media: Option<&[u8]>, pos: u32) -> Result<(), Error> {
        // 置き場の中の範囲は u32 で持つ（上限を広げて 4 GiB を超えたら、範囲を切らずに断る）
        let total = self.text.len().saturating_add(css.len()).saturating_add(media.map_or(0, <[u8]>::len));
        if u32::try_from(total).is_err() || u32::try_from(self.tables.names.len().saturating_add(total)).is_err() {
            return Err(self.budget.too_large());
        }
        let base = self.text.len();
        self.text.append_slice(css)?;
        let range = Span::new(base, self.text.len());
        let ns_start = self.ns.len();
        let mut top = NONE;
        if let Some(m) = media {
            let mb = self.text.len();
            self.text.append_slice(m)?;
            top = self.conds.len() as u32;
            self.conds.push(Cond {
                text: Span::new(mb, self.text.len()),
                parent: NONE,
                ns: Span::new(ns_start, ns_start),
                kind: COND_MEDIA,
                pad: [0; 3],
            })?;
        }
        // 字句は text を借りるので、ほかの入れ物を分けて借りる
        let SheetBuilder { budget, text, tables, rules, decls, conds, layers, ns, ns_peak, ns_names, notes, order } =
            self;
        let s = text.as_slice();
        let budget: &'b Budget<'b> = budget;
        let mut lex = Lexer::over(s, range, budget);
        let mut blocks: MVec<'_, Block> = MVec::new(budget);
        blocks.push(Block { cond: top, layer: NONE })?;
        let mut head = Head { may_import: true, may_namespace: true };
        let mut ctx = Ctx {
            s,
            budget,
            tables,
            rules,
            decls,
            conds,
            layers,
            ns,
            ns_peak,
            ns_names,
            ns_start,
            ns_dirty: false,
            ns_sorted: ns_start,
            notes,
            order,
            pos,
            nest: Nest::new(budget),
        };
        loop {
            budget.spend(cost::OP_STEP)?;
            let t = lex.next()?;
            let Some(&blk) = blocks.last() else { break };
            let nested = blocks.len() > 1;
            match t.kind {
                Kind::Whitespace => {}
                Kind::Eof => break,
                Kind::Cdo | Kind::Cdc if !nested => {}
                Kind::RBrace if nested => {
                    blocks.pop();
                }
                Kind::AtKeyword => match ctx.at_rule(&mut lex, t, blk, nested, &mut head)? {
                    Step::Open(b) => blocks.push(b)?,
                    Step::Close => {
                        blocks.pop();
                    }
                    Step::Done => {}
                },
                _ => {
                    let (closed, valid) = ctx.qualified_rule(&mut lex, t, blk, nested)?;
                    // 窓を閉じるのはいちばん外の正しい規則だけ（正しい `@media`・`@supports` は塊を開くときに閉じた。形の壊れた
                    // `@supports` の中の規則は閉じない。CSS の 4 回目のレビュー）
                    if valid && !nested {
                        head = Head { may_import: false, may_namespace: false };
                    }
                    if closed {
                        blocks.pop();
                    }
                }
            }
        }
        ctx.seal_ns(true)
    }

    /// 同じ層をまとめて、読んだ規則の並びにする。
    pub(crate) fn finish(mut self) -> Result<Sheet, Error> {
        let canon_parents = canonicalize_layers(self.budget, &mut self.layers, self.tables.names.as_slice())?;
        Ok(Sheet {
            text: self.text.freeze(),
            names: self.tables.names.freeze(),
            rules: self.rules.freeze(),
            decls: self.decls.freeze(),
            selectors: self.tables.selectors.freeze(),
            compounds: self.tables.compounds.freeze(),
            simples: self.tables.simples.freeze(),
            conds: self.conds.freeze(),
            layers: self.layers.freeze(),
            canon_parents: canon_parents.freeze(),
            ns: self.ns.freeze(),
            ns_names: self.ns_names.freeze(),
            notes: self.notes.freeze(),
        })
    }
}

/// 読んでいる間の口（`text` を借りたまま、ほかの入れ物に書く）。
struct Ctx<'c, 's, 'b> {
    s: &'s [u8],
    budget: &'b Budget<'b>,
    tables: &'c mut Tables<'b>,
    rules: &'c mut MVec<'b, Rule>,
    decls: &'c mut MVec<'b, Decl>,
    conds: &'c mut MVec<'b, Cond>,
    layers: &'c mut MVec<'b, Layer>,
    ns: &'c mut MVec<'b, NsDecl>,
    ns_peak: &'c mut usize,
    ns_names: &'c mut MVec<'b, u8>,
    /// この `<style>` の `@namespace` の表の始まり（`ns` の中）
    ns_start: usize,
    /// 表に足してから、まだ並べていない
    ns_dirty: bool,
    /// 並べた部分の終わり（`ns` の中。これより後ろは書いた順の尾）
    ns_sorted: usize,
    notes: &'c mut MVec<'b, Note>,
    order: &'c mut u32,
    pos: u32,
    nest: Nest<'b>,
}

/// 規則のセレクタに使う `@namespace` の表（この `<style>` の並べた部分と、並べていない尾）。
fn rule_ns_table<'a>(ns: &'a [NsDecl], start: usize, sorted: usize, names: &'a [u8]) -> NsTable<'a> {
    let own = ns.get(start..).unwrap_or(&[]);
    let sorted = sorted.saturating_sub(start);
    NsTable { decls: own.get(..sorted).unwrap_or(&[]), tail: own.get(sorted..).unwrap_or(&[]), names }
}

/// `@namespace` の表の並べていない尾の長さの上限（[`Ctx::seal_ns`]）。
const NS_TAIL: usize = 64;

/// 飛ばすけれど正しいアットルール（それより後ろに `@import`・`@namespace` を書けない）。
const KNOWN_AT_RULES: &[&[u8]] = &[
    b"font-face",
    b"keyframes",
    b"-webkit-keyframes",
    b"page",
    b"counter-style",
    b"property",
    b"font-feature-values",
    b"font-palette-values",
    b"container",
    b"scope",
    b"starting-style",
    b"view-transition",
    b"position-try",
    b"color-profile",
];

/// 層の名前に使えない言葉（CSS の全体の言葉。CSS Cascade 5 §6.4.2）。
const RESERVED_LAYER_NAMES: &[&[u8]] = &[b"initial", b"inherit", b"unset", b"revert", b"revert-layer"];

impl<'b> Ctx<'_, '_, 'b> {
    fn note(&mut self, kind: WarningKind) -> Result<(), Error> {
        self.notes.push(Note { href: Span::default(), pos: self.pos, unread: 0, pad: 0, kind })
    }

    /// 読まなかった参照（文字列か url の字句の中身。escape を解いて名前の置き場に写す）。
    fn unread(&mut self, t: &Token) -> Result<(), Error> {
        // 空と `data:` は外を指さないので載せない（参照の辺の `resolve` と同じ。中身を写さない）
        let mut head = [0u8; 5];
        let mut k = 0usize;
        for c in decode_string(self.s, t.inner, self.budget) {
            let c = c?;
            if k == 0 && matches!(c, 0x20 | 0x09 | 0x0A | 0x0C | 0x0D) {
                continue;
            }
            let Some(slot) = head.get_mut(k) else { break };
            // ASCII の大文字は小文字に、ASCII でない字は比べに当たらない印にする
            *slot = match c {
                0x41..=0x5A => (c as u8) | 0x20,
                0..0x80 => c as u8,
                _ => 0xFF,
            };
            k = k.saturating_add(1);
        }
        if k == 0 || head == *b"data:" {
            return Ok(());
        }
        let start = self.tables.names.len();
        for c in decode_string(self.s, t.inner, self.budget) {
            let (buf, n) = crate::css::select::utf8_pub(c?);
            self.tables.names.append_slice(buf.get(..n).unwrap_or(&[]))?;
        }
        let href = Span::new(start, self.tables.names.len());
        self.notes.push(Note { href, pos: self.pos, unread: 1, pad: 0, kind: WarningKind::InvalidValue })
    }

    /// この `<style>` の `@namespace` の表。
    fn ns_range(&self) -> Span {
        Span::new(self.ns_start, self.ns.len())
    }

    /// この `<style>` の `@namespace` の表を接頭辞の順に並べ、同じ接頭辞は後の宣言だけを残す（足すのは書いた順で、並べ替え
    /// は安定なので、同じ接頭辞の並びの最後が後の宣言）。`full` でなければ、並べていない尾が `NS_TAIL` 個までは並べずに
    /// 残す（引く側が尾を後ろから見る。`@namespace` と窓を閉じないものを交互に書いた文書で、並べ直しが 2 乗に伸びない。
    /// CSS の 4 回目のレビュー）。条件の表と `<style>` の終わりは `full` で並べきる。
    fn seal_ns(&mut self, full: bool) -> Result<(), Error> {
        if !self.ns_dirty {
            return Ok(());
        }
        if !full && self.ns.len().saturating_sub(self.ns_sorted) <= NS_TAIL {
            return Ok(());
        }
        self.ns_dirty = false;
        *self.ns_peak = (*self.ns_peak).max(self.ns.len());
        let budget = self.budget;
        let names = self.ns_names.as_slice();
        let start = self.ns_start;
        let Some(own) = self.ns.as_mut_slice().get_mut(start..) else { return Ok(()) };
        stable_sort_try(budget, own, |a, b| Ok(bytes_cmp(budget, a.prefix.of(names), b.prefix.of(names))?.is_lt()))?;
        let mut keep = start;
        let n = self.ns.len();
        for k in start..n {
            budget.spend(cost::OP_STEP)?;
            let Some(&d) = self.ns.get(k) else { continue };
            // 次が同じ接頭辞なら、これは前の宣言
            let later_same = match self.ns.get(k.saturating_add(1)) {
                Some(x) if k.saturating_add(1) < n => {
                    bytes_cmp(budget, x.prefix.of(names), d.prefix.of(names))?.is_eq()
                }
                _ => false,
            };
            if later_same {
                continue;
            }
            if let Some(x) = self.ns.get_mut(keep) {
                *x = d;
            }
            keep = keep.saturating_add(1);
        }
        self.ns.truncate(keep);
        self.ns_sorted = self.ns.len();
        Ok(())
    }

    /// 前置き（`first` から）を、深さ 0 の `{`・`;`（`stop_semicolon`）・`}`（入れ子のとき）・終わりまで読む。前置きの範囲と
    /// 止まった字句。かっこは種類ごとに組で読む。
    fn prelude(
        &mut self,
        lex: &mut Lexer<'_, '_>,
        first: Token,
        stop_semicolon: bool,
        nested: bool,
    ) -> Result<(Span, Token), Error> {
        let start = first.span.start;
        let mut end = start;
        self.nest.clear();
        let mut t = first;
        loop {
            if t.kind == Kind::Eof {
                return Ok((Span { start, end }, t));
            }
            if self.nest.top() {
                let stop = match t.kind {
                    Kind::LBrace => true,
                    Kind::Semicolon => stop_semicolon,
                    Kind::RBrace => nested,
                    _ => false,
                };
                if stop {
                    return Ok((Span { start, end }, t));
                }
            }
            self.nest.feed(&t)?;
            if t.kind != Kind::Whitespace {
                end = t.span.end;
            }
            t = lex.next()?;
        }
    }

    /// `{` を読んだあとの塊を、組の `}`（か終わり）まで飛ばす。
    fn skip_block(&mut self, lex: &mut Lexer<'_, '_>) -> Result<(), Error> {
        self.nest.clear();
        self.nest.feed(&Token::open_brace())?;
        loop {
            let t = lex.next()?;
            if t.kind == Kind::Eof || self.nest.feed(&t)? {
                return Ok(());
            }
        }
    }

    /// 規則（`first` から）。入れ子の塊を閉じる `}` で止まったか、正しい規則（選択子が読めた）か。
    fn qualified_rule(
        &mut self,
        lex: &mut Lexer<'_, '_>,
        first: Token,
        blk: Block,
        nested: bool,
    ) -> Result<(bool, bool), Error> {
        let (prelude, stop) = self.prelude(lex, first, false, nested)?;
        match stop.kind {
            Kind::LBrace => {}
            Kind::RBrace => {
                // 塊の `{` がないまま入れ子の塊が閉じた：規則を捨てる
                self.note(WarningKind::InvalidSelector)?;
                return Ok((true, false));
            }
            _ => {
                self.note(WarningKind::InvalidSelector)?;
                return Ok((false, false));
            }
        }
        self.seal_ns(false)?;
        let table = rule_ns_table(self.ns.as_slice(), self.ns_start, self.ns_sorted, self.ns_names.as_slice());
        let sel = parse_selector_list(self.s, prelude, table, self.tables, self.budget)?;
        let Some(parsed) = sel else {
            self.note(WarningKind::InvalidSelector)?;
            self.skip_block(lex)?;
            return Ok((false, false));
        };
        if parsed.unsupported {
            self.note(WarningKind::UnsupportedSelector)?;
        }
        let d0 = self.decls.len();
        let decls = &mut *self.decls;
        let notes = &mut *self.notes;
        let pos = self.pos;
        parse_declarations(lex, self.s, true, self.budget, &mut |it| match it {
            Item::Decl(d) => decls.push(d),
            Item::Nested => {
                notes.push(Note { href: Span::default(), pos, unread: 0, pad: 0, kind: WarningKind::NestedRule })
            }
        })?;
        let order = *self.order;
        *self.order = order.saturating_add(1);
        self.rules.push(Rule {
            selector: parsed.first,
            decls: Span::new(d0, self.decls.len()),
            cond: blk.cond,
            layer: blk.layer,
            order,
            pos: self.pos,
        })?;
        Ok((false, true))
    }

    /// アットルール。中に規則を読む塊を開いたら、その塊を返す。
    fn at_rule(
        &mut self,
        lex: &mut Lexer<'_, '_>,
        at: Token,
        blk: Block,
        nested: bool,
        head: &mut Head,
    ) -> Result<Step, Error> {
        let s = self.s;
        // 名前は 1 度だけ解いて、言葉の表と比べる
        let nm = crate::css::lex::name_of(s, at.inner, at.escaped, self.budget)?;
        let is = |w: &[u8]| nm.is(w);
        let mut first = lex.next()?;
        while first.kind == Kind::Whitespace {
            first = lex.next()?;
        }
        let (prelude, stop) = self.prelude(lex, first, true, nested)?;
        let block = stop.kind == Kind::LBrace;
        if stop.kind == Kind::RBrace && nested {
            // `;` のないまま入れ子の塊が閉じた：文のアットルールはそこで終わる（CSS Syntax。`@layer a` は層を足す）
            if is(b"layer")
                && let Some(list) = self.layer_names(prelude)?
                && !list.segs.is_empty()
            {
                self.register(blk, list.segs.as_slice())?;
            } else {
                self.note(WarningKind::AtRule)?;
            }
            return Ok(Step::Close);
        }
        let closed = Head { may_import: false, may_namespace: false };
        if is(b"supports") && block && !self.supports_shape_ok(prelude)? {
            // 形の壊れた `@supports` は規則ごと捨て（中を読まない）、`@import`・`@namespace` を閉じない（Chrome と同じ）
            self.note(WarningKind::AtRule)?;
            self.skip_block(lex)?;
            return Ok(Step::Done);
        }
        if (is(b"media") || is(b"supports")) && block {
            // `@media` の壊れた問いは `not all` なので、規則は正しい
            if !nested {
                *head = closed;
            }
            let kind = if is(b"media") { COND_MEDIA } else { COND_SUPPORTS };
            let id = self.conds.len() as u32;
            self.seal_ns(true)?;
            let ns = self.ns_range();
            self.conds.push(Cond { text: prelude, parent: blk.cond, ns, kind, pad: [0; 3] })?;
            return Ok(Step::Open(Block { cond: id, layer: blk.layer }));
        }
        if is(b"layer") {
            let names = self.layer_names(prelude)?;
            match (names, block) {
                (Some(list), true) if list.names <= 1 => {
                    if !nested {
                        *head = closed;
                    }
                    let layer = if list.segs.is_empty() {
                        self.anonymous(blk)?
                    } else {
                        self.register(blk, list.segs.as_slice())?
                    };
                    return Ok(Step::Open(Block { cond: blk.cond, layer }));
                }
                (Some(list), false) if !list.segs.is_empty() => {
                    // `@layer` の文は `@import`・`@namespace` の前に書ける
                    self.register(blk, list.segs.as_slice())?;
                    return Ok(Step::Done);
                }
                _ => {
                    self.note(WarningKind::AtRule)?;
                    if block {
                        self.skip_block(lex)?;
                    }
                    return Ok(Step::Done);
                }
            }
        }
        if is(b"namespace") && !block {
            if head.may_namespace
                && !nested
                && let Some((prefix, uri)) = namespace_prelude(s, prelude, self.budget)?
            {
                let start = self.ns_names.len();
                if let Some(p) = prefix {
                    for c in decode(s, p.inner) {
                        let (buf, n) = crate::css::select::utf8_pub(c);
                        self.ns_names.append_slice(buf.get(..n).unwrap_or(&[]))?;
                    }
                }
                let u: MVec<'_, u8> = decode_uri(s, uri, self.budget)?;
                let d =
                    NsDecl { prefix: Span::new(start, self.ns_names.len()), ns: ns_of_uri(u.as_slice()), pad: [0; 3] };
                // 書いた順に足し、引く前に並べる（seal_ns）
                self.ns.push(d)?;
                self.ns_dirty = true;
                head.may_import = false;
                return Ok(Step::Done);
            }
            self.note(WarningKind::AtRule)?;
            return Ok(Step::Done);
        }
        if is(b"import") && !block {
            // 外の規則は読まない（読まなかった参照）。正しい規則より後ろの @import は捨てる。`layer`・`layer(名前)` は、
            // 読まなくても層の順に入れる（Chrome と同じ）
            match first_url(s, prelude, self.budget)? {
                Some(href) if head.may_import && !nested => {
                    self.unread(&href)?;
                    self.import_layer(blk, prelude)?;
                }
                _ => self.note(WarningKind::AtRule)?,
            }
            return Ok(Step::Done);
        }
        if is(b"charset") && !block {
            return Ok(Step::Done);
        }
        if KNOWN_AT_RULES.iter().any(|w| is(w)) && block && !nested {
            *head = closed;
        }
        if is(b"font-face") && block {
            // src の url は読まない（読まなかった参照）。塊の中の url を拾う
            self.font_face_urls(lex)?;
            self.note(WarningKind::AtRule)?;
            return Ok(Step::Done);
        }
        // 知らないもの・ここでは使わないもの（@keyframes・@page など）
        self.note(WarningKind::AtRule)?;
        if block {
            self.skip_block(lex)?;
        }
        Ok(Step::Done)
    }

    /// `@supports` の前置きが `<supports-condition>` の形か（CSS Conditional 3 §2.1。上の段だけを見る：`not` の 1 つの項か、
    /// `and` か `or` の一方だけでつないだ項の並び。項はかっこか関数で、中は問わない（一般の囲み））。
    fn supports_shape_ok(&self, prelude: Span) -> Result<bool, Error> {
        let s = self.s;
        let mut lex = Lexer::over(s, prelude, self.budget);
        let mut items = 0u32;
        // つなぎ：0 まだ、1 and、2 or
        let mut op = 0u8;
        let mut negated = false;
        let mut want_item = true;
        loop {
            let t = lex.next()?;
            match t.kind {
                Kind::Whitespace => {}
                Kind::Eof => return Ok(items > 0 && !want_item),
                Kind::LParen | Kind::Function if want_item => {
                    let mut nest = Nest::new(self.budget);
                    nest.feed(&t)?;
                    loop {
                        let u = lex.next()?;
                        // 閉じないまま終わったかっこは閉じたものとする（CSS Syntax）
                        if u.kind == Kind::Eof || nest.feed(&u)? {
                            break;
                        }
                    }
                    items = items.saturating_add(1);
                    want_item = false;
                }
                Kind::Ident if want_item && items == 0 && !negated && ident_is(s, t.inner, t.escaped, b"not") => {
                    negated = true;
                }
                Kind::Ident if !want_item && !negated => {
                    let o = if ident_is(s, t.inner, t.escaped, b"and") {
                        1
                    } else if ident_is(s, t.inner, t.escaped, b"or") {
                        2
                    } else {
                        return Ok(false);
                    };
                    if op != 0 && op != o {
                        return Ok(false);
                    }
                    op = o;
                    want_item = true;
                }
                _ => return Ok(false),
            }
        }
    }

    /// `@import` の前置きの `layer`（名前のない層）か `layer(名前)` を層の順に入れる。層のあとの `supports(…)` と媒体の
    /// 問いの並びは、層を書いた所の条件にする（偽なら層の順に入らない。CSS Cascade 5 §2.1。Chrome と同じ）。
    fn import_layer(&mut self, blk: Block, prelude: Span) -> Result<(), Error> {
        let s = self.s;
        let mut lex = Lexer::over(s, prelude, self.budget);
        // 層は url のすぐあと（CSS Cascade 5 §2：`@import <url> [layer | layer()]? <import-conditions>`。条件のあとの
        // `layer(…)` は媒体の問いの一部）。層：None（書いていない）・Some(None)（名前のない層）・Some(Some(名前の範囲))
        let mut t = lex.next()?;
        while t.kind == Kind::Whitespace {
            t = lex.next()?;
        }
        if t.kind == Kind::Function && ident_is(s, t.inner, t.escaped, b"url") {
            // `url("…")` の閉じまで
            let mut nest = Nest::new(self.budget);
            nest.feed(&t)?;
            loop {
                let u = lex.next()?;
                if u.kind == Kind::Eof || nest.feed(&u)? {
                    break;
                }
            }
        } else if !matches!(t.kind, Kind::Url | Kind::Str) {
            return Ok(());
        }
        let mut t = lex.next()?;
        while t.kind == Kind::Whitespace {
            t = lex.next()?;
        }
        let layer: Option<Option<Span>> = match t.kind {
            Kind::Ident if ident_is(s, t.inner, t.escaped, b"layer") => Some(None),
            Kind::Function if ident_is(s, t.inner, t.escaped, b"layer") => {
                let start = t.span.end;
                let mut end = start;
                loop {
                    let u = lex.next()?;
                    if matches!(u.kind, Kind::Eof | Kind::RParen) {
                        break;
                    }
                    end = u.span.end;
                }
                Some(Some(Span { start, end }))
            }
            _ => None,
        };
        let Some(name) = layer else { return Ok(()) };
        // 層のあとの条件
        let mut cond = blk.cond;
        let mut rest = lex.next()?;
        while rest.kind == Kind::Whitespace {
            rest = lex.next()?;
        }
        if rest.kind == Kind::Function && ident_is(s, rest.inner, rest.escaped, b"supports") {
            let start = rest.span.end;
            let mut end = start;
            let mut nest = Nest::new(self.budget);
            nest.feed(&rest)?;
            loop {
                let u = lex.next()?;
                if u.kind == Kind::Eof || nest.feed(&u)? {
                    break;
                }
                end = u.span.end;
            }
            self.seal_ns(true)?;
            let ns = self.ns_range();
            let id = self.conds.len() as u32;
            self.conds.push(Cond {
                text: Span { start, end },
                parent: cond,
                ns,
                kind: COND_SUPPORTS_IMPORT,
                pad: [0; 3],
            })?;
            cond = id;
            rest = lex.next()?;
        }
        // 残りは媒体の問いの並び（空なら条件なし）
        while rest.kind == Kind::Whitespace {
            rest = lex.next()?;
        }
        if rest.kind != Kind::Eof {
            let media = Span { start: rest.span.start, end: prelude.end };
            let id = self.conds.len() as u32;
            self.conds.push(Cond { text: media, parent: cond, ns: self.ns_range(), kind: COND_MEDIA, pad: [0; 3] })?;
            cond = id;
        }
        let at = Block { cond, layer: blk.layer };
        match name {
            None => {
                self.anonymous(at)?;
            }
            Some(span) => {
                if let Some(list) = self.layer_names(span)?
                    && list.names == 1
                {
                    self.register(at, list.segs.as_slice())?;
                }
            }
        }
        Ok(())
    }

    /// `@font-face {…}` の中の url を読まなかった参照に載せ、塊を閉じまで読む。
    fn font_face_urls(&mut self, lex: &mut Lexer<'_, '_>) -> Result<(), Error> {
        let mut nest = Nest::new(self.budget);
        nest.feed(&Token::open_brace())?;
        let mut url_fn = false;
        loop {
            let t = lex.next()?;
            match t.kind {
                Kind::Eof => return Ok(()),
                Kind::Url => self.unread(&t)?,
                Kind::Str if url_fn => {
                    self.unread(&t)?;
                    url_fn = false;
                }
                _ => {}
            }
            if t.kind == Kind::Function {
                url_fn = ident_is(self.s, t.inner, t.escaped, b"url");
            } else if t.kind != Kind::Whitespace && t.kind != Kind::Str {
                url_fn = false;
            }
            if nest.feed(&t)? {
                return Ok(());
            }
        }
    }

    /// 層の名前を書いた所を足す（`a.b.c` の段ごとに節。名前ごとに外の層から始める）。最後の名前の最後の節の番号。
    fn register(&mut self, blk: Block, segs: &[Seg]) -> Result<u32, Error> {
        let mut at = blk.layer;
        let mut last = NONE;
        for g in segs {
            self.budget.spend(cost::OP_STEP)?;
            let id = self.layers.len() as u32;
            self.layers.push(Layer {
                name: g.name,
                parent: at,
                cond: blk.cond,
                canon: NONE,
                anonymous: 0,
                pad: [0; 3],
            })?;
            last = id;
            at = if g.last != 0 { blk.layer } else { id };
        }
        Ok(last)
    }

    fn anonymous(&mut self, blk: Block) -> Result<u32, Error> {
        let id = self.layers.len() as u32;
        self.layers.push(Layer {
            name: Span::default(),
            parent: blk.layer,
            cond: blk.cond,
            canon: NONE,
            anonymous: 1,
            pad: [0; 3],
        })?;
        Ok(id)
    }

    /// `@layer` の前置きの名前の並び（`a.b, c`）。名前の段は escape を解いて名前の置き場に写す。空の前置きは空の並び、
    /// 形が違えば None（空の項目・`.` の前後の空白・全体の言葉の名前も）。
    fn layer_names(&mut self, prelude: Span) -> Result<Option<LayerNames<'b>>, Error> {
        let s = self.s;
        let budget = self.budget;
        let mut segs: MVec<'b, Seg> = MVec::new(budget);
        let mut lex = Lexer::over(s, prelude, budget);
        // 0：名前を待つ（並びの頭か `,` のあと）、1：段を読んだ、2：`.` のあと、3：名前のあとの空白
        let mut state = 0u8;
        let mut any = false;
        let mut names = 0usize;
        loop {
            let t = lex.next()?;
            match (t.kind, state) {
                (Kind::Whitespace, 0 | 3) => {}
                (Kind::Whitespace, 1) => state = 3,
                (Kind::Ident, 0 | 2) => {
                    if RESERVED_LAYER_NAMES.iter().any(|w| ident_is(s, t.inner, t.escaped, w)) {
                        return Ok(None);
                    }
                    let start = self.tables.names.len();
                    for c in decode(s, t.inner) {
                        let (buf, n) = crate::css::select::utf8_pub(c);
                        self.tables.names.append_slice(buf.get(..n).unwrap_or(&[]))?;
                    }
                    segs.push(Seg { name: Span::new(start, self.tables.names.len()), last: 1, pad: [0; 3] })?;
                    if state == 0 {
                        names = names.saturating_add(1);
                    }
                    state = 1;
                    any = true;
                }
                (Kind::Delim, 1) if t.is_delim(b'.') => {
                    if let Some(g) = segs.last_mut() {
                        g.last = 0;
                    }
                    state = 2;
                }
                (Kind::Comma, 1 | 3) => state = 0,
                (Kind::Eof, 1 | 3) => return Ok(Some(LayerNames { segs, names })),
                (Kind::Eof, 0) if !any => return Ok(Some(LayerNames { segs, names })),
                _ => return Ok(None),
            }
        }
    }
}

/// 層の名前の 1 段。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Seg {
    name: Span,
    /// 名前の最後の段
    last: u8,
    pad: [u8; 3],
}

impl Plain for Seg {}

/// `@layer` の前置きの名前（段の並び）と名前の数。
struct LayerNames<'b> {
    segs: MVec<'b, Seg>,
    names: usize,
}

/// `@namespace` の前置き：`[接頭辞]? (文字列 | url)`。
fn namespace_prelude(s: &[u8], prelude: Span, budget: &Budget<'_>) -> Result<Option<(Option<Token>, Token)>, Error> {
    let budget: &Budget<'_> = budget;
    let mut lex = Lexer::over(s, prelude, budget_ref(budget));
    let mut prefix = None;
    let mut uri = None;
    loop {
        let t = lex.next()?;
        match t.kind {
            Kind::Whitespace => {}
            Kind::Ident if prefix.is_none() && uri.is_none() => prefix = Some(t),
            Kind::Str | Kind::Url if uri.is_none() => uri = Some(t),
            Kind::Function if uri.is_none() && ident_is(s, t.inner, t.escaped, b"url") => {
                let mut v = lex.next()?;
                while v.kind == Kind::Whitespace {
                    v = lex.next()?;
                }
                if v.kind != Kind::Str {
                    return Ok(None);
                }
                uri = Some(v);
                let mut c = lex.next()?;
                while c.kind == Kind::Whitespace {
                    c = lex.next()?;
                }
                if c.kind != Kind::RParen {
                    return Ok(None);
                }
            }
            Kind::Eof => return Ok(uri.map(|u| (prefix, u))),
            _ => return Ok(None),
        }
    }
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 文字列か url の字句の中身を解いて UTF-8 にする（短い一時の入れ物）。
fn decode_uri<'b>(s: &[u8], t: Token, budget: &'b Budget<'b>) -> Result<MVec<'b, u8>, Error> {
    let mut out: MVec<'b, u8> = MVec::new(budget);
    for c in decode_string(s, t.inner, budget) {
        let (buf, n) = crate::css::select::utf8_pub(c?);
        out.append_slice(buf.get(..n).unwrap_or(&[]))?;
    }
    Ok(out)
}

/// 前置きの最初の url（url の字句・`url("…")`・文字列）の字句。
fn first_url(s: &[u8], prelude: Span, budget: &Budget<'_>) -> Result<Option<Token>, Error> {
    let mut lex = Lexer::over(s, prelude, budget_ref(budget));
    loop {
        let t = lex.next()?;
        match t.kind {
            Kind::Whitespace => {}
            Kind::Url | Kind::Str => return Ok(Some(t)),
            Kind::Function if ident_is(s, t.inner, t.escaped, b"url") => {
                let mut v = lex.next()?;
                while v.kind == Kind::Whitespace {
                    v = lex.next()?;
                }
                return Ok((v.kind == Kind::Str).then_some(v));
            }
            _ => return Ok(None),
        }
    }
}

/// 同じ層（まとめた外の層と名前が同じ。名前のない層はそれぞれ別）の節に共通の番号を振り、まとめた層ごとの外の層を返す。
/// 外の層の節は子の節より前にあるので、深さの順に、同じ深さの節を（外の層, 名前）で並べて続く同じものをまとめる
/// （手間は節の数 × log。名前のバイトは比べる中で払う）。
fn canonicalize_layers<'b>(
    budget: &'b Budget<'b>,
    layers: &mut MVec<'b, Layer>,
    names: &[u8],
) -> Result<MVec<'b, u32>, Error> {
    let n = layers.len();
    // まとめた層の数は節の数の内（容量は節の数で取り、持ち続ける量を数える段の量で決める）
    let mut parents: MVec<'b, u32> = MVec::with_capacity(budget, n)?;
    if n == 0 {
        return Ok(parents);
    }
    // 深さと、深さの順（同じ深さは書いた順）
    let mut depth: MVec<'_, u32> = MVec::with_capacity(budget, n)?;
    let mut max_depth = 0u32;
    for l in layers.as_slice() {
        budget.spend(cost::OP_STEP)?;
        let d = if l.parent == NONE { 0 } else { depth.get(l.parent as usize).copied().unwrap_or(0).saturating_add(1) };
        max_depth = max_depth.max(d);
        depth.push(d)?;
    }
    let levels = (max_depth as usize).saturating_add(2);
    let mut start: MVec<'_, u32> = MVec::with_capacity(budget, levels)?;
    start.resize_to(levels, 0)?;
    for &d in depth.as_slice() {
        budget.spend(cost::OP_STEP)?;
        if let Some(c) = start.get_mut((d as usize).saturating_add(1)) {
            *c = c.saturating_add(1);
        }
    }
    for k in 1..levels {
        budget.spend(cost::OP_STEP)?;
        let prev = start.get(k.saturating_sub(1)).copied().unwrap_or(0);
        if let Some(c) = start.get_mut(k) {
            *c = c.saturating_add(prev);
        }
    }
    let mut order: MVec<'_, u32> = MVec::with_capacity(budget, n)?;
    order.resize_to(n, 0)?;
    let mut fill: MVec<'_, u32> = MVec::with_capacity(budget, levels)?;
    fill.append_slice(start.as_slice())?;
    for (i, &d) in depth.as_slice().iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        if let Some(at) = fill.get_mut(d as usize) {
            if let Some(o) = order.get_mut(*at as usize) {
                *o = i as u32;
            }
            *at = at.saturating_add(1);
        }
    }
    let mut canon: MVec<'_, u32> = MVec::with_capacity(budget, n)?;
    canon.resize_to(n, NONE)?;
    let mut seg: MVec<'_, u32> = MVec::new(budget);
    for lv in 0..levels.saturating_sub(1) {
        let a = start.get(lv).copied().unwrap_or(0) as usize;
        let b = start.get(lv.saturating_add(1)).copied().unwrap_or(0) as usize;
        if a >= b {
            continue;
        }
        seg.clear();
        seg.append_slice(order.as_slice().get(a..b).unwrap_or(&[]))?;
        let ls = layers.as_slice();
        let cs = canon.as_slice();
        // 鍵：（外の層のまとめた番号, 名前のない層か, 名前）。名前のない層は節の番号で別にする
        let key = |x: u32| {
            let l = ls.get(x as usize);
            let p = l.map_or(NONE, |l| l.parent);
            let pc = if p == NONE { NONE } else { cs.get(p as usize).copied().unwrap_or(NONE) };
            (pc, l.map_or(0, |l| l.anonymous), l.map_or(&[][..], |l| l.name.of(names)))
        };
        let cmp = |x: u32, y: u32| -> Result<core::cmp::Ordering, Error> {
            let (kx, ky) = (key(x), key(y));
            let o = kx.0.cmp(&ky.0).then(kx.1.cmp(&ky.1));
            if o != core::cmp::Ordering::Equal {
                return Ok(o);
            }
            if kx.1 != 0 {
                return Ok(x.cmp(&y));
            }
            bytes_cmp(budget, kx.2, ky.2)
        };
        stable_sort_try(budget, seg.as_mut_slice(), |x, y| Ok(cmp(*x, *y)? == core::cmp::Ordering::Less))?;
        let mut prev: Option<u32> = None;
        let mut assign: MVec<'_, u32> = MVec::with_capacity(budget, seg.len())?;
        for &x in seg.as_slice() {
            budget.spend(cost::OP_STEP)?;
            let same = match prev {
                Some(p) => cmp(p, x)? == core::cmp::Ordering::Equal,
                None => false,
            };
            if !same {
                parents.push(key(x).0)?;
            }
            assign.push((parents.len() as u32).saturating_sub(1))?;
            prev = Some(x);
        }
        for (k, &x) in seg.as_slice().iter().enumerate() {
            budget.spend(cost::OP_STEP)?;
            if let (Some(c), Some(&v)) = (canon.get_mut(x as usize), assign.get(k)) {
                *c = v;
            }
        }
    }
    for (l, &c) in layers.as_mut_slice().iter_mut().zip(canon.as_slice()) {
        budget.spend(cost::OP_STEP)?;
        l.canon = c;
    }
    Ok(parents)
}

#[cfg(test)]
mod tests;
