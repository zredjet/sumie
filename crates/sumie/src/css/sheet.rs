//! 規則の並び（CSS Syntax Level 3 §5.4.1・CSS Cascade 5・Conditional Rules。FR-ST-06〜09・ADR-66 (2)(3)）。
//!
//! 文書のすべての `<style>` を文書の順に 1 つの [`Sheet`] に読む：
//!
//! - 規則：選択子の並び（[`crate::css::select`]）と宣言の範囲・条件（`@media`・`@supports`・`<style media>`）・層・順
//! - 条件は読むだけで照らさない（`@media` は文書の大きさが、`@supports` はプロパティの表が要るので、style の段で照らす）。
//!   条件は外の条件を `parent` で指す
//! - 層（`@layer`）は木にして、名前の初めての順と「層の中の規則はその子の層より強い」で順位を決める（[`Sheet::rank`]）。
//!   層のない規則は最も強い
//! - `@namespace` は `<style>` ごと。ほかの規則より前にだけ書ける
//! - `@import`・`@font-face` の `src` の URL は読まずに、読まなかった参照に載せる（FR-IN-08）。ほかの知らないアットルールは
//!   飛ばして警告する
//! - 壊れたセレクタの規則は、宣言の塊ごと捨てて警告する（CSS の仕様）
//!
//! 数える段（`estimate_parse`）は同じ読み方で、容量を伸ばしながら読んで量を測る。読む段は測った量で最初に容量を取る。

use crate::budget::Budget;
use crate::cost;
use crate::css::decl::{Decl, parse_declarations};
use crate::css::lex::{Kind, Lexer, Token, decode, decode_string, ident_is};
use crate::css::select::{NONE, NsDecl, Tables, ns_of_uri, parse_selector_list};
use crate::error::Error;
use crate::mem::{Frozen, MVec, Plain};
use crate::report::WarningKind;
use crate::xml::lex::Span;

/// 規則。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rule {
    /// 並びの最初の選択子
    pub(crate) selector: u32,
    /// 宣言の番号の範囲
    pub(crate) decls: Span,
    /// いちばん内の条件（なければ NONE）
    pub(crate) cond: u32,
    /// 層（なければ NONE）
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

/// 条件（`@media` の問いの並び・`@supports` の条件・`<style media>`）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cond {
    /// 条件の字句の範囲（`text` の中）
    pub(crate) text: Span,
    /// 外の条件
    pub(crate) parent: u32,
    pub(crate) kind: u8,
    pub(crate) pad: [u8; 3],
}

impl Plain for Cond {}

/// 層の木の節。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layer {
    /// 名前の 1 段（名前の置き場の中。名前のない層は空で、ほかと同じにならない）
    pub(crate) name: Span,
    pub(crate) parent: u32,
    pub(crate) anonymous: u32,
    /// 順位（小さいほど弱い）。`finish` で決める
    pub(crate) rank: u32,
}

impl Plain for Layer {}

/// 後で報告に移す覚え（警告か、読まなかった参照）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Note {
    /// 読まなかった参照なら href の範囲（`text` の中）。警告なら空
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
            self.notes.bytes(),
        ]
        .iter()
        .fold(0u64, |a, &b| a.saturating_add(b))
    }

    /// 規則の層の順位（層のない規則は u32::MAX で最も強い）。
    pub(crate) fn rank(&self, r: &Rule) -> u32 {
        if r.layer == NONE {
            return u32::MAX;
        }
        self.layers.as_slice().get(r.layer as usize).map_or(u32::MAX, |l| l.rank)
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
    pub(crate) notes: u64,
}

impl SheetCounts {
    /// 持ち続ける量（`Sheet::bytes` と同じ式）。
    pub(crate) fn bytes(&self) -> u64 {
        let m = |n: u64, size: usize| n.saturating_mul(size as u64);
        [
            self.text,
            self.names,
            m(self.rules, size_of::<Rule>()),
            m(self.decls, size_of::<Decl>()),
            m(self.selectors, size_of::<crate::css::select::Selector>()),
            m(self.compounds, size_of::<crate::css::select::Compound>()),
            m(self.simples, size_of::<crate::css::select::Simple>()),
            m(self.conds, size_of::<Cond>()),
            m(self.layers, size_of::<Layer>()),
            m(self.notes, size_of::<Note>()),
        ]
        .iter()
        .fold(0u64, |a, &b| a.saturating_add(b))
    }
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
            notes: n(self.notes.len()),
        }
    }

    fn note(&mut self, kind: WarningKind, pos: u32) -> Result<(), Error> {
        self.notes.push(Note { href: Span::default(), pos, unread: 0, pad: 0, kind })
    }

    /// `<style>` 1 つ（`css` は揃えた中身、`media` は `media` 属性の値、`pos` は要素の位置）を足す。
    pub(crate) fn add(&mut self, css: &[u8], media: Option<&[u8]>, pos: u32) -> Result<(), Error> {
        let base = self.text.len();
        self.text.append_slice(css)?;
        let range = Span::new(base, self.text.len());
        let mut top = NONE;
        if let Some(m) = media {
            let mb = self.text.len();
            self.text.append_slice(m)?;
            top = self.conds.len() as u32;
            self.conds.push(Cond {
                text: Span::new(mb, self.text.len()),
                parent: NONE,
                kind: COND_MEDIA,
                pad: [0; 3],
            })?;
        }
        // 字句は text を借りるので、ほかの入れ物を分けて借りる
        let SheetBuilder { budget, text, tables, rules, decls, conds, layers, notes, order } = self;
        let s = text.as_slice();
        let budget: &'b Budget<'b> = budget;
        let mut lex = Lexer::over(s, range, budget);
        let mut ns: MVec<'_, NsDecl> = MVec::new(budget);
        let mut blocks: MVec<'_, Block> = MVec::new(budget);
        blocks.push(Block { cond: top, layer: NONE })?;
        // `@namespace` は、ほかの規則（`@charset`・`@import`・`@layer` の文を除く）より前にだけ書ける
        let mut may_namespace = true;
        let mut ctx = Ctx { s, budget, tables, rules, decls, conds, layers, notes, order, pos };
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
                Kind::AtKeyword => match ctx.at_rule(&mut lex, t, blk, nested, &mut ns, &mut may_namespace)? {
                    Step::Open(b) => blocks.push(b)?,
                    Step::Close => {
                        blocks.pop();
                    }
                    Step::Done => {}
                },
                _ => {
                    may_namespace = false;
                    let closed = ctx.qualified_rule(&mut lex, t, blk, nested, ns.as_slice())?;
                    if closed {
                        blocks.pop();
                    }
                }
            }
        }
        Ok(())
    }

    /// 層の順位を決めて、読んだ規則の並びにする。
    pub(crate) fn finish(mut self) -> Result<Sheet, Error> {
        rank_layers(self.budget, &mut self.layers)?;
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
    notes: &'c mut MVec<'b, Note>,
    order: &'c mut u32,
    pos: u32,
}

/// 開きのかっこに対する閉じのかっこ。
const fn closer(k: Kind) -> Option<Kind> {
    match k {
        Kind::LBrace => Some(Kind::RBrace),
        Kind::LBracket => Some(Kind::RBracket),
        Kind::LParen | Kind::Function => Some(Kind::RParen),
        _ => None,
    }
}

impl Ctx<'_, '_, '_> {
    fn note(&mut self, kind: WarningKind) -> Result<(), Error> {
        self.notes.push(Note { href: Span::default(), pos: self.pos, unread: 0, pad: 0, kind })
    }

    fn unread(&mut self, href: Span) -> Result<(), Error> {
        self.notes.push(Note { href, pos: self.pos, unread: 1, pad: 0, kind: WarningKind::InvalidValue })
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
        let mut stack: MVec<'_, u8> = MVec::new(self.budget);
        let mut t = first;
        loop {
            if stack.is_empty() {
                let stop = match t.kind {
                    Kind::LBrace | Kind::Eof => true,
                    Kind::Semicolon => stop_semicolon,
                    Kind::RBrace => nested,
                    _ => false,
                };
                if stop {
                    return Ok((Span { start, end }, t));
                }
            }
            if let Some(c) = closer(t.kind) {
                stack.push(c as u8)?;
            } else if stack.last().is_some_and(|&c| c == t.kind as u8) {
                stack.pop();
            } else if t.kind == Kind::Eof {
                return Ok((Span { start, end }, t));
            }
            if t.kind != Kind::Whitespace {
                end = t.span.end;
            }
            t = lex.next()?;
        }
    }

    /// `{` を読んだあとの塊を、組の `}`（か終わり）まで飛ばす。
    fn skip_block(&mut self, lex: &mut Lexer<'_, '_>) -> Result<(), Error> {
        let mut stack: MVec<'_, u8> = MVec::new(self.budget);
        stack.push(Kind::RBrace as u8)?;
        loop {
            let t = lex.next()?;
            if t.kind == Kind::Eof {
                return Ok(());
            }
            if let Some(c) = closer(t.kind) {
                stack.push(c as u8)?;
            } else if stack.last().is_some_and(|&c| c == t.kind as u8) {
                stack.pop();
                if stack.is_empty() {
                    return Ok(());
                }
            }
        }
    }

    /// 規則（`first` から）。入れ子の塊を閉じる `}` で止まったら真。
    fn qualified_rule(
        &mut self,
        lex: &mut Lexer<'_, '_>,
        first: Token,
        blk: Block,
        nested: bool,
        ns: &[NsDecl],
    ) -> Result<bool, Error> {
        let (prelude, stop) = self.prelude(lex, first, false, nested)?;
        match stop.kind {
            Kind::LBrace => {}
            Kind::RBrace => {
                // 塊の `{` がないまま入れ子の塊が閉じた：規則を捨てる
                self.note(WarningKind::InvalidSelector)?;
                return Ok(true);
            }
            _ => {
                self.note(WarningKind::InvalidSelector)?;
                return Ok(false);
            }
        }
        let sel = parse_selector_list(self.s, prelude, ns, self.tables, self.budget)?;
        let Some(first_sel) = sel else {
            self.note(WarningKind::InvalidSelector)?;
            self.skip_block(lex)?;
            return Ok(false);
        };
        let d0 = self.decls.len();
        let decls = &mut *self.decls;
        parse_declarations(lex, self.s, true, self.budget, &mut |d| decls.push(d))?;
        let order = *self.order;
        *self.order = order.saturating_add(1);
        self.rules.push(Rule {
            selector: first_sel,
            decls: Span::new(d0, self.decls.len()),
            cond: blk.cond,
            layer: blk.layer,
            order,
            pos: self.pos,
        })?;
        Ok(false)
    }

    /// アットルール。中に規則を読む塊を開いたら、その塊を返す。
    fn at_rule(
        &mut self,
        lex: &mut Lexer<'_, '_>,
        at: Token,
        blk: Block,
        nested: bool,
        ns: &mut MVec<'_, NsDecl>,
        may_namespace: &mut bool,
    ) -> Result<Step, Error> {
        let s = self.s;
        let is = |w: &[u8]| ident_is(s, at.inner, at.escaped, w);
        let mut first = lex.next()?;
        while first.kind == Kind::Whitespace {
            first = lex.next()?;
        }
        let (prelude, stop) = self.prelude(lex, first, true, nested)?;
        let block = stop.kind == Kind::LBrace;
        if stop.kind == Kind::RBrace && nested {
            // `;` も塊もないまま入れ子の塊が閉じた（このアットルールは壊れている）
            self.note(WarningKind::AtRule)?;
            return Ok(Step::Close);
        }
        let keep_namespace = is(b"charset") || is(b"import") || is(b"namespace") || (is(b"layer") && !block);
        if !keep_namespace {
            *may_namespace = false;
        }
        if (is(b"media") || is(b"supports")) && block {
            let kind = if is(b"media") { COND_MEDIA } else { COND_SUPPORTS };
            let id = self.conds.len() as u32;
            self.conds.push(Cond { text: prelude, parent: blk.cond, kind, pad: [0; 3] })?;
            return Ok(Step::Open(Block { cond: id, layer: blk.layer }));
        }
        if is(b"layer") {
            let names = layer_names(s, prelude, self.budget)?;
            match (names, block) {
                (Some(list), true) if list.len() <= 1 => {
                    let layer = match list.as_slice().first() {
                        Some(&n) => self.register(blk.layer, n)?,
                        None => self.anonymous(blk.layer)?,
                    };
                    return Ok(Step::Open(Block { cond: blk.cond, layer }));
                }
                (Some(list), false) if !list.is_empty() => {
                    for &n in list.as_slice() {
                        self.register(blk.layer, n)?;
                    }
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
            if *may_namespace
                && !nested
                && let Some((prefix, uri)) = namespace_prelude(s, prelude, self.budget)?
            {
                let p = self.tables_name(prefix)?;
                let u: MVec<'_, u8> = decode_uri(s, uri, self.budget)?;
                ns.push(NsDecl { prefix: p, ns: ns_of_uri(u.as_slice()), pad: [0; 3] })?;
                return Ok(Step::Done);
            }
            self.note(WarningKind::AtRule)?;
            return Ok(Step::Done);
        }
        if is(b"import") && !block {
            // 外の規則は読まない（読まなかった参照）
            if let Some(href) = first_url(s, prelude, self.budget)? {
                self.unread(href)?;
            }
            return Ok(Step::Done);
        }
        if is(b"charset") && !block {
            return Ok(Step::Done);
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

    /// `@font-face {…}` の中の url を読まなかった参照に載せ、塊を閉じまで読む。
    fn font_face_urls(&mut self, lex: &mut Lexer<'_, '_>) -> Result<(), Error> {
        let mut stack: MVec<'_, u8> = MVec::new(self.budget);
        stack.push(Kind::RBrace as u8)?;
        let mut url_fn = false;
        loop {
            let t = lex.next()?;
            match t.kind {
                Kind::Eof => return Ok(()),
                Kind::Url => self.unread(t.inner)?,
                Kind::Str if url_fn => {
                    self.unread(t.inner)?;
                    url_fn = false;
                }
                _ => {}
            }
            if t.kind == Kind::Function {
                url_fn = ident_is(self.s, t.inner, t.escaped, b"url");
            } else if t.kind != Kind::Whitespace && t.kind != Kind::Str {
                url_fn = false;
            }
            if let Some(c) = closer(t.kind) {
                stack.push(c as u8)?;
            } else if stack.last().is_some_and(|&c| c == t.kind as u8) {
                stack.pop();
                if stack.is_empty() {
                    return Ok(());
                }
            }
        }
    }

    fn tables_name(&mut self, t: Option<Token>) -> Result<Span, Error> {
        let start = self.tables.names.len();
        if let Some(t) = t {
            for c in decode(self.s, t.inner) {
                let (buf, n) = crate::css::select::utf8_pub(c);
                self.tables.names.append_slice(buf.get(..n).unwrap_or(&[]))?;
            }
        }
        Ok(Span::new(start, self.tables.names.len()))
    }

    /// 層を名前（`a.b.c` の字句の並び）で引き、なければ足す。最後の段の番号。
    fn register(&mut self, parent: u32, name: Span) -> Result<u32, Error> {
        let mut at = parent;
        let mut k = name.start as usize;
        let end = name.end as usize;
        // 名前は ident を `.` でつないだもの（layer_names で確かめてある）。段ごとに引く
        while k < end {
            let mut j = k;
            while j < end && self.s.get(j) != Some(&b'.') {
                j = j.saturating_add(1);
            }
            let seg = Span::new(k, j);
            at = self.child(at, seg)?;
            k = j.saturating_add(1);
        }
        Ok(at)
    }

    /// `parent` の子で名前が `seg` の層（なければ足す）。
    fn child(&mut self, parent: u32, seg: Span) -> Result<u32, Error> {
        let want = seg.of(self.s);
        for (i, l) in self.layers.as_slice().iter().enumerate() {
            self.budget.spend(cost::copy(want.len() as u64).saturating_add(1))?;
            if l.parent == parent && l.anonymous == 0 && l.name.of(self.tables.names.as_slice()) == want {
                return Ok(i as u32);
            }
        }
        let start = self.tables.names.len();
        self.tables.names.append_slice(want)?;
        let id = self.layers.len() as u32;
        self.layers.push(Layer { name: Span::new(start, self.tables.names.len()), parent, anonymous: 0, rank: 0 })?;
        Ok(id)
    }

    fn anonymous(&mut self, parent: u32) -> Result<u32, Error> {
        let id = self.layers.len() as u32;
        self.layers.push(Layer { name: Span::default(), parent, anonymous: 1, rank: 0 })?;
        Ok(id)
    }
}

/// `@layer` の前置きの名前の並び（`a.b, c`）。各名前は入力の中の範囲（escape のない ident を `.` でつないだもの）。
/// 空の前置きは空の並び、形が違えば None。
fn layer_names<'b>(s: &[u8], prelude: Span, budget: &'b Budget<'b>) -> Result<Option<MVec<'b, Span>>, Error> {
    let mut out: MVec<'b, Span> = MVec::new(budget);
    let mut lex = Lexer::over(s, prelude, budget);
    let mut cur: Option<Span> = None;
    let mut want_ident = true;
    loop {
        let t = lex.next()?;
        match t.kind {
            Kind::Whitespace if cur.is_none() || !want_ident => {
                if let Some(c) = cur.take() {
                    out.push(c)?;
                }
            }
            Kind::Ident if want_ident && !t.escaped => {
                cur = Some(match cur {
                    Some(c) => Span { start: c.start, end: t.span.end },
                    None => t.span,
                });
                want_ident = false;
            }
            Kind::Delim if t.is_delim(b'.') && !want_ident && cur.is_some() => want_ident = true,
            Kind::Comma if !want_ident || cur.is_none() => {
                if let Some(c) = cur.take() {
                    out.push(c)?;
                } else if out.is_empty() {
                    return Ok(None);
                }
                want_ident = true;
            }
            Kind::Eof => {
                if let Some(c) = cur.take() {
                    out.push(c)?;
                } else if want_ident && !out.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(out));
            }
            _ => return Ok(None),
        }
    }
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
    for c in decode_string(s, t.inner) {
        let (buf, n) = crate::css::select::utf8_pub(c);
        out.append_slice(buf.get(..n).unwrap_or(&[]))?;
    }
    Ok(out)
}

/// 前置きの最初の url（url の字句・`url("…")`・文字列）の中身の範囲。
fn first_url(s: &[u8], prelude: Span, budget: &Budget<'_>) -> Result<Option<Span>, Error> {
    let mut lex = Lexer::over(s, prelude, budget_ref(budget));
    loop {
        let t = lex.next()?;
        match t.kind {
            Kind::Whitespace => {}
            Kind::Url | Kind::Str => return Ok(Some(t.inner)),
            Kind::Function if ident_is(s, t.inner, t.escaped, b"url") => {
                let mut v = lex.next()?;
                while v.kind == Kind::Whitespace {
                    v = lex.next()?;
                }
                return Ok((v.kind == Kind::Str).then_some(v.inner));
            }
            _ => return Ok(None),
        }
    }
}

/// 層の順位：子の層は名前の初めての順に並び、層の中の規則（層そのもの）はその子の層より強い。帰りがけの順に番号を振る。
fn rank_layers(budget: &Budget<'_>, layers: &mut MVec<'_, Layer>) -> Result<(), Error> {
    let n = layers.len();
    if n == 0 {
        return Ok(());
    }
    // 明示の積み上げ：（節, 次に見る子の候補の番号）。親は子より前に足されるので、子の候補は親より後ろだけを見る
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Visit {
        node: u32,
        next: u32,
    }
    impl Plain for Visit {}
    let mut stack: MVec<'_, Visit> = MVec::new(budget_ref(budget));
    stack.push(Visit { node: NONE, next: 0 })?;
    let mut rank = 0u32;
    while let Some(top) = stack.last().copied() {
        budget.spend(cost::OP_STEP)?;
        // 次の子を探す
        let mut k = top.next as usize;
        let mut found = None;
        while k < n {
            budget.spend(1)?;
            if layers.get(k).is_some_and(|l| l.parent == top.node) {
                found = Some(k);
                break;
            }
            k = k.saturating_add(1);
        }
        match found {
            Some(c) => {
                if let Some(t) = stack.last_mut() {
                    t.next = (c as u32).saturating_add(1);
                }
                stack.push(Visit { node: c as u32, next: (c as u32).saturating_add(1) })?;
            }
            None => {
                stack.pop();
                if top.node != NONE
                    && let Some(l) = layers.get_mut(top.node as usize)
                {
                    l.rank = rank;
                    rank = rank.saturating_add(1);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
