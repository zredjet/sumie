//! セレクタ（Selectors Level 3 と Level 4 の一部。FR-ST-07・ADR-66 (1)・ADR-73）を読み、選択子・複合・単純な選択子の表に足す。
//!
//! - 表は番号でつなぐ（複合は左の複合を `prev`、単純な選択子は `next`、並びの選択子は `next` で指す）。足す順は「中の並び
//!   が先」：`:is()` などの中の選択子と複合は、それを持つ外の複合より前に足すので、番号の順に照らせば中が先に決まる
//! - `:is()`・`:where()` の並びは寛容（壊れた選択子だけを捨てる）。`:not()`・`:has()`・`of S` と規則の並びは、1 つでも
//!   壊れていれば全体が壊れる（壊れた中の並びは、外の寛容な並びまで上がって、その選択子を捨てる）
//! - 当てない擬似クラス（`:hover` など）と擬似要素は正しいセレクタとして読み、当たらない印（`never`）を付ける。擬似要素は
//!   いちばん外の並びにだけ書け、そのあとに書ける擬似クラスと擬似要素は擬似要素ごとに決まる（Chrome 157 と照らした表。
//!   ADR-109・ADR-110）
//! - `:has()`・`:nth-child(… of S)` は中まで確かめて読むが当てず、それを含むいちばん外の選択子を捨てる（[`Parsed::unsupported`]。
//!   当てないものを `:not()` で裏返して当てすぎないため）
//! - 既定の名前空間（`@namespace` の接頭辞なし）は、型と `*` と、型のない複合（`:is()` などの中の右端の複合を除く）に効く
//! - 再帰しない（入れ子の並びは明示の積み上げ）

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::{Kind, Lexer, Nest, Token, decode, decode_string, ident_is};
use crate::error::Error;
use crate::mem::{MVec, Plain, bytes_cmp, lower_bound_try};
use crate::xml::lex::Span;
use crate::xml::{NS_NONE, NS_OTHER, NS_SVG, NS_XLINK, NS_XML};

/// 表の中で「ない」の印。
pub(crate) const NONE: u32 = u32::MAX;

/// 名前空間の制約で「どれでも」。
pub(crate) const NS_ANY: u8 = 255;

/// 結合子（左の複合とのつながり）。
pub(crate) const COMB_NONE: u8 = 0;
pub(crate) const COMB_DESC: u8 = 1;
pub(crate) const COMB_CHILD: u8 = 2;
pub(crate) const COMB_NEXT: u8 = 3;
pub(crate) const COMB_LATER: u8 = 4;

/// 単純な選択子の種類。
pub(crate) const S_TYPE: u8 = 0;
pub(crate) const S_UNIVERSAL: u8 = 1;
pub(crate) const S_ID: u8 = 2;
pub(crate) const S_CLASS: u8 = 3;
pub(crate) const S_ATTR: u8 = 4;
pub(crate) const S_FIRST_CHILD: u8 = 5;
pub(crate) const S_LAST_CHILD: u8 = 6;
pub(crate) const S_ONLY_CHILD: u8 = 7;
pub(crate) const S_NTH_CHILD: u8 = 8;
pub(crate) const S_NTH_LAST_CHILD: u8 = 9;
pub(crate) const S_FIRST_OF_TYPE: u8 = 10;
pub(crate) const S_LAST_OF_TYPE: u8 = 11;
pub(crate) const S_ONLY_OF_TYPE: u8 = 12;
pub(crate) const S_NTH_OF_TYPE: u8 = 13;
pub(crate) const S_NTH_LAST_OF_TYPE: u8 = 14;
pub(crate) const S_ROOT: u8 = 15;
pub(crate) const S_EMPTY: u8 = 16;
pub(crate) const S_LINK: u8 = 17;
pub(crate) const S_NOT: u8 = 18;
pub(crate) const S_IS: u8 = 19;
pub(crate) const S_LANG: u8 = 20;
pub(crate) const S_NEVER: u8 = 21;

/// 属性のセレクタの比べ方。
pub(crate) const OP_EXISTS: u8 = 0;
pub(crate) const OP_EQ: u8 = 1;
pub(crate) const OP_INCLUDES: u8 = 2;
pub(crate) const OP_DASH: u8 = 3;
pub(crate) const OP_PREFIX: u8 = 4;
pub(crate) const OP_SUFFIX: u8 = 5;
pub(crate) const OP_SUBSTR: u8 = 6;

/// 単純な選択子の印：属性の値を大文字小文字を問わずに比べる（`i`）。
pub(crate) const FLAG_CASE_I: u8 = 1;
/// 単純な選択子の印：空白で区切った語として比べる名前が空か空白を含む（`.class`・`[a~=v]` はどの要素にも当たらない）。
pub(crate) const FLAG_NO_TOKEN: u8 = 2;

/// 空白で区切った語として比べる名前が、当たりえないか（空か CSS の空白を含む。読むバイトを払う）。
fn no_token(name: &[u8], budget: &Budget<'_>) -> Result<bool, Error> {
    if name.is_empty() {
        return Ok(true);
    }
    let mut meter = crate::xml::lex::ScanMeter::new(0, name.len());
    for (k, &c) in name.iter().enumerate() {
        meter.pay(budget, k)?;
        if matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0C) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 選択子（複合を結合子でつないだもの）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Selector {
    /// 右端（当てる要素）の複合
    pub(crate) subject: u32,
    /// 並びの次の選択子
    pub(crate) next: u32,
    /// 詳細度（a << 20 | b << 10 | c。各 1023 で止める）
    pub(crate) spec: u32,
    pub(crate) pad: u32,
}

impl Plain for Selector {}

/// 複合のセレクタ。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Compound {
    /// 最初の単純な選択子
    pub(crate) first: u32,
    /// 左の複合（なければ NONE）と、そのつながり
    pub(crate) prev: u32,
    pub(crate) comb: u8,
    /// 型のない複合に効く既定の名前空間（NS_ANY ならどれでも）
    pub(crate) ns: u8,
    pub(crate) pad: [u8; 2],
}

impl Plain for Compound {}

/// 単純な選択子。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Simple {
    /// 同じ複合の次
    pub(crate) next: u32,
    /// `:not()`・`:is()` の中の並びの最初の選択子
    pub(crate) list: u32,
    /// 名前（型・id・class・属性の名前・`:lang` の範囲）と値（属性の値）。名前の置き場の中
    pub(crate) a: Span,
    pub(crate) b: Span,
    /// An+B
    pub(crate) n_a: i32,
    pub(crate) n_b: i32,
    pub(crate) kind: u8,
    /// 名前空間の制約（NS_ANY か xml の名前空間の種類）
    pub(crate) ns: u8,
    pub(crate) op: u8,
    pub(crate) flags: u8,
}

impl Plain for Simple {}

impl Simple {
    const fn new(kind: u8) -> Simple {
        Simple {
            next: NONE,
            list: NONE,
            a: Span { start: 0, end: 0 },
            b: Span { start: 0, end: 0 },
            n_a: 0,
            n_b: 0,
            kind,
            ns: NS_ANY,
            op: 0,
            flags: 0,
        }
    }
}

/// 選択子の表（規則の並び全体で 1 つ）。
pub(crate) struct Tables<'b> {
    pub(crate) names: MVec<'b, u8>,
    pub(crate) selectors: MVec<'b, Selector>,
    pub(crate) compounds: MVec<'b, Compound>,
    pub(crate) simples: MVec<'b, Simple>,
}

impl<'b> Tables<'b> {
    pub(crate) fn new(budget: &'b Budget<'b>) -> Tables<'b> {
        Tables {
            names: MVec::new(budget),
            selectors: MVec::new(budget),
            compounds: MVec::new(budget),
            simples: MVec::new(budget),
        }
    }

    /// 解いた符号位置を UTF-8 で名前の置き場に足す（`lower` なら ASCII を小文字に）。
    fn push_name(&mut self, chars: impl Iterator<Item = u32>, lower: bool) -> Result<Span, Error> {
        let start = self.names.len();
        for c in chars {
            let c = if lower && (0x41..=0x5A).contains(&c) { c.saturating_add(0x20) } else { c };
            let (buf, n) = utf8(c);
            self.names.append_slice(buf.get(..n).unwrap_or(&[]))?;
        }
        Ok(Span::new(start, self.names.len()))
    }

    /// 文字列の字句の中身を解いて（読む分を払う）名前の置き場に足す。
    fn push_name_str(&mut self, s: &[u8], span: Span, lower: bool) -> Result<Span, Error> {
        let start = self.names.len();
        let budget = self.names.budget();
        for c in decode_string(s, span, budget) {
            let c = c?;
            let c = if lower && (0x41..=0x5A).contains(&c) { c.saturating_add(0x20) } else { c };
            let (buf, n) = utf8(c);
            self.names.append_slice(buf.get(..n).unwrap_or(&[]))?;
        }
        Ok(Span::new(start, self.names.len()))
    }

    fn push_simple(&mut self, s: Simple) -> Result<u32, Error> {
        let id = self.simples.len() as u32;
        self.simples.push(s)?;
        Ok(id)
    }
}

/// `:lang()` の範囲の区切り（[`lang_ranges`]）。
pub(crate) const LANG_SEP: u8 = 0;

/// 符号位置を UTF-8 にする（ほかの css の部品も使う）。
pub(crate) fn utf8_pub(c: u32) -> ([u8; 4], usize) {
    utf8(c)
}

/// 符号位置を UTF-8 にする。
fn utf8(c: u32) -> ([u8; 4], usize) {
    let b = |x: u32| (x & 0xFF) as u8;
    if c < 0x80 {
        ([b(c), 0, 0, 0], 1)
    } else if c < 0x800 {
        ([b(0xC0 | (c >> 6)), b(0x80 | (c & 0x3F)), 0, 0], 2)
    } else if c < 0x10000 {
        ([b(0xE0 | (c >> 12)), b(0x80 | ((c >> 6) & 0x3F)), b(0x80 | (c & 0x3F)), 0], 3)
    } else {
        ([b(0xF0 | (c >> 18)), b(0x80 | ((c >> 12) & 0x3F)), b(0x80 | ((c >> 6) & 0x3F)), b(0x80 | (c & 0x3F))], 4)
    }
}

/// `@namespace` の宣言（接頭辞は名前空間の名前の置き場の中。既定の名前空間は空）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct NsDecl {
    pub(crate) prefix: Span,
    pub(crate) ns: u8,
    pub(crate) pad: [u8; 3],
}

impl Plain for NsDecl {}

/// 名前空間の URI から種類を決める（「そのほか」は URI を区別しない。ADR-66 (1)）。
pub(crate) fn ns_of_uri(uri: &[u8]) -> u8 {
    match uri {
        b"http://www.w3.org/2000/svg" => NS_SVG,
        b"http://www.w3.org/1999/xlink" => NS_XLINK,
        b"http://www.w3.org/XML/1998/namespace" => NS_XML,
        b"" => NS_NONE,
        _ => NS_OTHER,
    }
}

/// `@namespace` の表（`<style>` ごと）。宣言は接頭辞のバイトの順に並べ、同じ接頭辞は後の宣言だけを残す（規則の並びが
/// 引く前に並べる）。
#[derive(Clone, Copy)]
pub(crate) struct NsTable<'a> {
    /// 接頭辞の順に並べ、同じ接頭辞を除いたもの
    pub(crate) decls: &'a [NsDecl],
    /// 並べたあとに足した宣言（書いた順。後の宣言が勝つ。並べた部分の宣言にも勝つ）
    pub(crate) tail: &'a [NsDecl],
    pub(crate) names: &'a [u8],
}

impl NsTable<'_> {
    pub(crate) const EMPTY: NsTable<'static> = NsTable { decls: &[], tail: &[], names: &[] };

    /// 既定の名前空間（なければどれでも）。尾は後ろから、並べた部分では空の接頭辞がいちばん前に並ぶ。
    fn default_ns(&self) -> u8 {
        if let Some(d) = self.tail.iter().rev().find(|d| d.prefix.len() == 0) {
            return d.ns;
        }
        self.decls.first().filter(|d| d.prefix.len() == 0).map_or(NS_ANY, |d| d.ns)
    }

    /// 接頭辞の字句 `t`（ident。escape を解いて比べる）の名前空間。宣言がなければ None。尾を後ろから見てから、並べた部分を
    /// 2 分で探す。
    fn lookup(&self, s: &[u8], t: &Token, budget: &Budget<'_>) -> Result<Option<u8>, Error> {
        for d in self.tail.iter().rev() {
            budget.spend(cost::OP_STEP)?;
            if cmp_name(budget, d.prefix.of(self.names), s, t)? == core::cmp::Ordering::Equal {
                return Ok(Some(d.ns));
            }
        }
        let i = lower_bound_try(budget, self.decls, |d| Ok(cmp_name(budget, d.prefix.of(self.names), s, t)? == Less))?;
        match self.decls.get(i) {
            Some(d) if cmp_name(budget, d.prefix.of(self.names), s, t)? == core::cmp::Ordering::Equal => Ok(Some(d.ns)),
            _ => Ok(None),
        }
    }
}

use core::cmp::Ordering::Less;

/// 名前 `a`（UTF-8）と字句 `t` の中身（escape を解いたもの）を比べる。解く符号位置は 64 字の塊ごとに、解く前に払う
/// （1 字は生のバイトで 9 まで。`cost::decode`）。
fn cmp_name(budget: &Budget<'_>, a: &[u8], s: &[u8], t: &Token) -> Result<core::cmp::Ordering, Error> {
    if !t.escaped {
        return bytes_cmp(budget, a, t.inner.of(s));
    }
    let mut k = 0usize;
    let mut n = 0u32;
    let mut it = decode(s, t.inner);
    loop {
        if n.trailing_zeros() >= 6 {
            budget.spend(cost::decode(64).saturating_add(1))?;
        }
        n = n.wrapping_add(1);
        let Some(c) = it.next() else { break };
        let (buf, len) = utf8(c);
        let part = buf.get(..len).unwrap_or(&[]);
        let w = a.get(k..k.saturating_add(len).min(a.len())).unwrap_or(&[]);
        let o = w.cmp(part);
        if o != core::cmp::Ordering::Equal {
            return Ok(o);
        }
        k = k.saturating_add(len);
    }
    budget.spend(1)?;
    Ok(a.len().cmp(&k))
}

/// 読みの失敗：セレクタが壊れた（規則・選択子を捨てる）か、勘定の誤り（そのまま返す）。
pub(crate) enum Fail {
    Bad,
    Stop(Error),
}

impl From<Error> for Fail {
    fn from(e: Error) -> Fail {
        Fail::Stop(e)
    }
}

type R<T> = Result<T, Fail>;

const fn invalid() -> Fail {
    Fail::Bad
}

/// 詳細度の 3 つ組。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Spec {
    a: u32,
    b: u32,
    c: u32,
}

impl Plain for Spec {}
impl Plain for Frame {}

impl Spec {
    fn pack(self) -> u32 {
        (self.a.min(1023) << 20) | (self.b.min(1023) << 10) | self.c.min(1023)
    }

    fn add(self, o: Spec) -> Spec {
        Spec { a: self.a.saturating_add(o.a), b: self.b.saturating_add(o.b), c: self.c.saturating_add(o.c) }
    }
}

/// 擬似要素を読んだあと（そのあとに書けるもの。Chrome 157 と照らした。ADR-109・ADR-110）。
const PE_NONE: u8 = 0;
/// `::before`・`::after`（`::marker` と論理の擬似クラス（中は空））
const PE_BEFORE_AFTER: u8 = 1;
/// 要素に支えられた擬似要素 `::part()`・`::details-content`・`::picker()`（`::part()`・`::slotted()` でない擬似要素と、構造・
/// スクロールバーでない擬似クラス（`:state()`・`:dir()`・`:lang()`・`:active-view-transition-type()` も））
const PE_PART: u8 = 2;
/// ほかの擬似要素（論理の擬似クラス（中は空））
const PE_OTHER: u8 = 3;
/// `::selection`（`:window-inactive`）
const PE_SELECTION: u8 = 4;
/// `::slotted()`（木に沿う擬似要素。CSS Scoping 1 §3.2.2）
const PE_SLOTTED: u8 = 5;
/// `::column`（`::scroll-marker` だけ。論理の擬似クラスも書けない。CSS Overflow 5）
const PE_COLUMN: u8 = 6;
/// `::-webkit-scrollbar*`（`:hover`・`:active`・スクロールバーの擬似クラス・`:window-inactive`・`:enabled`・`:disabled`）
const PE_SCROLLBAR: u8 = 7;
/// 表にないほかの `::-webkit-*`（利用者の動きの擬似クラス）
const PE_WEBKIT: u8 = 8;
/// `::file-selector-button`・`::cue`（関数でない形）（利用者の動きの擬似クラス）
const PE_USER: u8 = 9;
/// `::scroll-marker`（利用者の動きの擬似クラスと `:target-current`）
const PE_SCROLL_MARKER: u8 = 10;
/// `::scroll-button()`（利用者の動きの擬似クラスと `:enabled`・`:disabled`）
const PE_SCROLL_BUTTON: u8 = 11;
/// `::scroll-marker-group`（`:hover`・`:focus-within`）
const PE_MARKER_GROUP: u8 = 13;
/// `::view-transition-*()`（`:only-child`）
const PE_VIEW_TRANSITION: u8 = 14;
/// `::search-text`（`:current`）
const PE_SEARCH_TEXT: u8 = 15;

/// `::slotted()` のあとに書ける木に沿う擬似要素（Chrome 157 と照らした。`::first-line`・`::selection` は書けない）。
const AFTER_SLOTTED: &[&[u8]] =
    &[b"before", b"after", b"marker", b"placeholder", b"file-selector-button", b"details-content", b"scroll-marker"];

/// 入れ子の並び（規則の並び・`:is()`・`:where()`・`:not()`・`:has()`・`of S`・関数の擬似の複合の引数）を読んでいる所。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Frame {
    /// この並びを閉じたときに外の複合に足す単純な選択子の種類
    kind: u8,
    /// 規則の並び（いちばん外）
    top: bool,
    forgiving: bool,
    /// `:where()` は詳細度 0
    zero: bool,
    /// 読むが当てない並び（`:has()`・`of S`）
    opaque: bool,
    /// 頭に結合子を書ける（`:has()`）
    relative: bool,
    /// `:has()` の中（`:has()` は入れ子にできない）
    in_has: bool,
    first_sel: u32,
    last_sel: u32,
    /// 読み終えた選択子の数（当てないものも数える）
    count: u32,
    max_spec: Spec,
    /// 並びの中に当てない選択子がある
    any_unsupported: bool,
    /// 並びの中に表にない `::-webkit-*` の擬似要素がある（[`Parsed::webkit`]）
    any_webkit: bool,
    /// 読んでいる選択子
    prev_compound: u32,
    pending_comb: u8,
    spec: Spec,
    /// 読んでいる選択子が当てないものを含む
    unsupported: bool,
    /// 読んでいる複合
    first_simple: u32,
    last_simple: u32,
    compound_empty: bool,
    /// 複合に型か `*` がある
    has_type: bool,
    /// 擬似要素を読んだあと（PE_*）
    pe: u8,
    /// 読んでいる選択子が壊れた
    broken: bool,
    /// この並びの中の、開いているかっこの数（並びの終わりの `,`・`)` を見分ける）
    base: u32,
    /// 擬似要素のあとに開いた論理の擬似クラスの並び（中は、その擬似要素のあとに書ける擬似クラスだけ。PE_*）
    outer_pe: u8,
    /// `:-webkit-any()`（寛容でない複合の並び。詳細度は擬似クラス 1 つ。Chrome と同じ）
    webkit_any: bool,
    /// 結合子を書けない並び（`:host()`・`:host-context()`・`::slotted()`・`::cue()` の引数と、その中の論理の擬似クラス）
    compound: bool,
    /// 関数の擬似の引数の並び（閉じても単純な選択子を足さない）：0 でない、1 ちょうど 1 つ、2 は 1 つ以上
    arg: u8,
}

impl Frame {
    fn new(kind: u8, top: bool, forgiving: bool, zero: bool, in_has: bool) -> Frame {
        Frame {
            kind,
            top,
            forgiving,
            zero,
            opaque: false,
            relative: false,
            in_has,
            first_sel: NONE,
            last_sel: NONE,
            count: 0,
            max_spec: Spec::default(),
            any_unsupported: false,
            any_webkit: false,
            prev_compound: NONE,
            pending_comb: COMB_NONE,
            spec: Spec::default(),
            unsupported: false,
            first_simple: NONE,
            last_simple: NONE,
            compound_empty: true,
            has_type: false,
            pe: PE_NONE,
            broken: false,
            base: 0,
            outer_pe: PE_NONE,
            webkit_any: false,
            compound: false,
            arg: 0,
        }
    }

    /// 書けるものを決める擬似要素（読んだ擬似要素か、この並びを開いた外の擬似要素）。
    fn pe_ctx(&self) -> u8 {
        if self.pe != PE_NONE { self.pe } else { self.outer_pe }
    }

    /// 開いているかっこの数が `base` の所から始まる並び。
    fn at(mut self, base: usize) -> Frame {
        self.base = u32::try_from(base).unwrap_or(u32::MAX);
        self
    }
}

/// 1 つ先を見られる字句の口。
struct Cursor<'l, 'a, 'b> {
    lex: &'l mut Lexer<'a, 'b>,
    peeked: Option<Token>,
    nest: Nest<'b>,
}

impl Cursor<'_, '_, '_> {
    fn peek(&mut self) -> Result<Token, Error> {
        if let Some(t) = self.peeked {
            return Ok(t);
        }
        let t = self.lex.next()?;
        self.peeked = Some(t);
        Ok(t)
    }

    /// 次の字句を読む（読んだ字句はかっこの積み上げに入れる。開いているかっこの数で、並びの終わりを見分ける）。
    fn next(&mut self) -> Result<Token, Error> {
        let t = match self.peeked.take() {
            Some(t) => t,
            None => self.lex.next()?,
        };
        self.nest.feed(&t)?;
        Ok(t)
    }

    fn skip_ws(&mut self) -> Result<bool, Error> {
        let mut any = false;
        while self.peek()?.kind == Kind::Whitespace {
            self.next()?;
            any = true;
        }
        Ok(any)
    }

    /// 開いているかっこの数が `base` の所の `,` か `)`（読まない）・終わりまで飛ばす（壊れた選択子の中で開いたかっこは、
    /// 組の閉じまで読む）。
    fn skip_to_list_end(&mut self, base: usize) -> Result<(), Error> {
        loop {
            let t = self.peek()?;
            match t.kind {
                Kind::Eof => return Ok(()),
                Kind::Comma | Kind::RParen if self.nest.depth() <= base => return Ok(()),
                _ => {}
            }
            self.next()?;
        }
    }

    /// 関数（開きの字句は読んだあと）の中身を組の `)` まで読み飛ばし、中身の範囲（`)` の前まで）を返す。
    fn skip_function(&mut self) -> Result<Span, Error> {
        let start = self.peek()?.span.start;
        let mut end = start;
        let target = self.nest.depth().saturating_sub(1);
        loop {
            let t = self.next()?;
            match t.kind {
                Kind::Eof => return Ok(Span { start, end: t.span.start }),
                Kind::RParen if self.nest.depth() <= target => return Ok(Span { start, end }),
                _ => {}
            }
            end = t.span.end;
        }
    }

    /// 開いているかっこの数。
    fn depth(&self) -> usize {
        self.nest.depth()
    }
}

/// 読んだ選択子の並び。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Parsed {
    /// 当てる最初の選択子（当てるものがなければ NONE）
    pub(crate) first: u32,
    /// 当てない選択子（`:has()`・`of S` を含む）を捨てた
    pub(crate) unsupported: bool,
    /// 寛容な並び（`:is()`・`:where()`）の中の壊れた選択子を捨てた（`@supports selector()` は偽。Chrome と同じ。単位 3 の
    /// 10 回目のレビュー）
    pub(crate) dropped: bool,
    /// 表にない `::-webkit-*` の擬似要素を受けた（規則では Chrome も受けるが、`@supports selector()` では Chrome の知るものだけ
    /// 真なので分からない。単位 3 の 11 回目のレビュー）
    pub(crate) webkit: bool,
}

/// 選択子の並び `range` を読む（壊れていれば None。表に足した分は捨てずに残る）。並びの選択子は `next` でつなぐ。
pub(crate) fn parse_selector_list(
    s: &[u8],
    range: Span,
    ns: NsTable<'_>,
    t: &mut Tables<'_>,
    budget: &Budget<'_>,
) -> Result<Option<Parsed>, Error> {
    match parse_list(s, range, ns, t, budget) {
        Ok(p) => Ok(Some(p)),
        Err(Fail::Bad) => Ok(None),
        Err(Fail::Stop(e)) => Err(e),
    }
}

fn parse_list(s: &[u8], range: Span, ns: NsTable<'_>, t: &mut Tables<'_>, budget: &Budget<'_>) -> R<Parsed> {
    let budget = budget_ref(budget);
    let default_ns = ns.default_ns();
    let mut lex = Lexer::over(s, range, budget);
    let mut cur = Cursor { lex: &mut lex, peeked: None, nest: Nest::new(budget) };
    let mut frames: MVec<'_, Frame> = MVec::new(budget);
    frames.push(Frame::new(S_NEVER, true, false, false, false))?;
    cur.skip_ws()?;
    let mut dropped = false;
    loop {
        budget.spend(cost::OP_STEP)?;
        let tok = cur.peek()?;
        let depth = frames.len();
        let Some(mut f) = frames.last().copied() else { return Err(invalid()) };
        let base = f.base as usize;
        let list_end = tok.kind == Kind::Eof || (matches!(tok.kind, Kind::RParen | Kind::Comma) && cur.depth() <= base);
        if f.broken && !list_end {
            // 壊れた選択子の残りを飛ばす（中で開いたかっこは組の閉じまで。寛容な並びなら次の `,` まで）
            cur.skip_to_list_end(base)?;
            continue;
        }
        if list_end {
            if tok.kind != Kind::Eof {
                cur.next()?;
            }
            // 選択子を終える
            if tok.kind == Kind::RParen && f.top {
                f.broken = true;
            }
            end_selector(&mut f, t, default_ns)?;
            if let Some(x) = frames.last_mut() {
                *x = f;
            }
            dropped |= f.broken && f.forgiving;
            if f.broken && !f.forgiving {
                // 壊れた並び：外へ上げる
                if f.top {
                    return Err(invalid());
                }
                if tok.kind == Kind::Comma {
                    // 並びの残りを飛ばして閉じのかっこまで
                    cur.skip_to_list_end(base)?;
                    while cur.peek()?.kind == Kind::Comma {
                        cur.next()?;
                        cur.skip_to_list_end(base)?;
                    }
                    if cur.peek()?.kind == Kind::RParen {
                        cur.next()?;
                    }
                }
                frames.pop();
                if let Some(p) = frames.last_mut() {
                    p.broken = true;
                }
                continue;
            }
            if tok.kind == Kind::Comma {
                reset_selector(frames.last_mut());
                cur.skip_ws()?;
                continue;
            }
            // 並びの終わり
            if f.top {
                if tok.kind != Kind::Eof || f.count == 0 {
                    return Err(invalid());
                }
                return Ok(Parsed {
                    first: f.first_sel,
                    unsupported: f.any_unsupported,
                    dropped,
                    webkit: f.any_webkit,
                });
            }
            frames.pop();
            close_list(&f, t, frames.last_mut())?;
            continue;
        }
        let r = step(&mut cur, s, &mut f, t, ns, default_ns, &mut frames);
        if let Some(x) = frames.get_mut(depth.saturating_sub(1)) {
            *x = f;
        }
        match r {
            Ok(()) => {}
            Err(Fail::Bad) => {
                if let Some(x) = frames.get_mut(depth.saturating_sub(1)) {
                    x.broken = true;
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// 閉じた入れ子の並び `f` を、外の複合 `p` の単純な選択子にする。
fn close_list(f: &Frame, t: &mut Tables<'_>, p: Option<&mut Frame>) -> R<()> {
    let Some(p) = p else { return Err(invalid()) };
    // 関数の擬似の引数：選択子の数だけ確かめる（単純な選択子は始めに足した）
    if f.arg != 0 {
        if f.count == 0 || (f.arg == 1 && f.count != 1) {
            p.broken = true;
        }
        return Ok(());
    }
    if f.opaque || f.kind == S_NOT {
        // `:has()`・`of S`・`:not()` の空の並びは壊れている
        if f.count == 0 {
            p.broken = true;
            return Ok(());
        }
    }
    let mut simple = Simple::new(f.kind);
    simple.list = f.first_sel;
    if f.opaque {
        // 読むが当てない：外の選択子ごと捨てる
        simple = Simple::new(S_NEVER);
        p.unsupported = true;
    } else if f.first_sel == NONE {
        // 寛容な並びが空でも正しい（何にも当たらない）
        simple.kind = S_NEVER;
    }
    if f.any_unsupported {
        p.unsupported = true;
    }
    if f.webkit_any && f.count == 0 {
        p.broken = true;
        return Ok(());
    }
    let add = if f.zero || f.opaque {
        Spec::default()
    } else if f.webkit_any {
        Spec { a: 0, b: 1, c: 0 }
    } else {
        f.max_spec
    };
    // 擬似要素のあとの論理の擬似クラスは、中を確かめてある
    add_simple(p, t, simple, f.outer_pe != PE_NONE)?;
    p.spec = p.spec.add(add);
    Ok(())
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 読んでいる選択子を終え、並びにつなぐ（壊れていれば捨てる。いちばん外の当てない選択子はつながない）。
fn end_selector(f: &mut Frame, t: &mut Tables<'_>, default_ns: u8) -> R<()> {
    if f.broken {
        return Ok(());
    }
    if f.compound_empty {
        // 空の選択子・結合子で終わる選択子は壊れている
        f.broken = true;
        return Ok(());
    }
    // 右端の複合：入れ子の並びの中では既定の名前空間が効かない（Selectors 4 の `:is()` などの決まり。ADR-73 (8)）
    let c = push_compound(f, t, if f.top { default_ns } else { NS_ANY })?;
    f.count = f.count.saturating_add(1);
    if f.unsupported {
        f.any_unsupported = true;
        if f.top {
            return Ok(());
        }
    }
    let id = t.selectors.len() as u32;
    t.selectors.push(Selector { subject: c, next: NONE, spec: f.spec.pack(), pad: 0 })?;
    if f.last_sel == NONE {
        f.first_sel = id;
    } else if let Some(p) = t.selectors.get_mut(f.last_sel as usize) {
        p.next = id;
    }
    f.last_sel = id;
    if f.spec > f.max_spec {
        f.max_spec = f.spec;
    }
    Ok(())
}

fn reset_selector(f: Option<&mut Frame>) {
    if let Some(f) = f {
        f.prev_compound = NONE;
        f.pending_comb = COMB_NONE;
        f.spec = Spec::default();
        f.unsupported = false;
        f.first_simple = NONE;
        f.last_simple = NONE;
        f.compound_empty = true;
        f.has_type = false;
        f.pe = PE_NONE;
        f.broken = false;
    }
}

/// 読んでいる複合を表に足す（型も `*` もない複合には `implied` の名前空間が効く）。
fn push_compound(f: &mut Frame, t: &mut Tables<'_>, implied: u8) -> R<u32> {
    let id = t.compounds.len() as u32;
    let ns = if f.has_type { NS_ANY } else { implied };
    t.compounds.push(Compound {
        first: f.first_simple,
        prev: f.prev_compound,
        comb: f.pending_comb,
        ns,
        pad: [0; 2],
    })?;
    f.prev_compound = id;
    f.first_simple = NONE;
    f.last_simple = NONE;
    f.compound_empty = true;
    f.has_type = false;
    f.pe = PE_NONE;
    Ok(id)
}

/// 単純な選択子を複合に足す（擬似要素のあとは `after_pe` のものだけ）。
fn add_simple(f: &mut Frame, t: &mut Tables<'_>, s: Simple, after_pe: bool) -> R<()> {
    if f.pe_ctx() != PE_NONE && !after_pe {
        return Err(invalid());
    }
    let id = t.push_simple(s)?;
    if f.last_simple == NONE {
        f.first_simple = id;
    } else if let Some(p) = t.simples.get_mut(f.last_simple as usize) {
        p.next = id;
    }
    f.last_simple = id;
    f.compound_empty = false;
    Ok(())
}

/// 選択子の 1 歩（単純な選択子・結合子・入れ子の並びの始まり）。
fn step(
    cur: &mut Cursor<'_, '_, '_>,
    s: &[u8],
    f: &mut Frame,
    t: &mut Tables<'_>,
    ns: NsTable<'_>,
    default_ns: u8,
    frames: &mut MVec<'_, Frame>,
) -> R<()> {
    let budget = t.names.budget();
    let tok = cur.peek()?;
    // 結合子
    if tok.kind == Kind::Whitespace || tok.is_delim(b'>') || tok.is_delim(b'+') || tok.is_delim(b'~') {
        cur.skip_ws()?;
        let n = cur.peek()?;
        let comb = if n.is_delim(b'>') {
            COMB_CHILD
        } else if n.is_delim(b'+') {
            COMB_NEXT
        } else if n.is_delim(b'~') {
            COMB_LATER
        } else {
            // 空白だけ：並びの終わりなら結合子でない
            if matches!(n.kind, Kind::Eof | Kind::Comma | Kind::RParen) {
                return Ok(());
            }
            COMB_DESC
        };
        if comb != COMB_DESC {
            cur.next()?;
            cur.skip_ws()?;
        }
        if f.pe_ctx() != PE_NONE || f.webkit_any || f.compound {
            // 擬似要素のあと（とその論理の擬似クラスの中）と `:-webkit-any()`・複合の引数の中に結合子は書けない
            return Err(invalid());
        }
        if f.compound_empty {
            // `:has()` の頭の結合子（`> a`）だけ書ける
            if f.relative && f.prev_compound == NONE && f.pending_comb == COMB_NONE && comb != COMB_DESC {
                f.pending_comb = comb;
                return Ok(());
            }
            return Err(invalid());
        }
        push_compound(f, t, default_ns)?;
        f.pending_comb = comb;
        return Ok(());
    }
    cur.next()?;
    match tok.kind {
        // 型・`*`・名前空間の接頭辞
        Kind::Ident | Kind::Delim if tok.kind == Kind::Ident || tok.is_delim(b'*') || tok.is_delim(b'|') => {
            if !f.compound_empty {
                return Err(invalid());
            }
            let (ns_c, name_tok) = if tok.is_delim(b'|') {
                // `|E`：名前空間なし
                (NS_NONE, cur.next()?)
            } else if cur.peek()?.is_delim(b'|') {
                // `ns|E`・`*|E`（`|` のあとに空白は書けない）
                cur.next()?;
                let c = if tok.is_delim(b'*') { NS_ANY } else { ns.lookup(s, &tok, budget)?.ok_or_else(invalid)? };
                (c, cur.next()?)
            } else {
                (default_ns, tok)
            };
            let simple = if name_tok.is_delim(b'*') {
                let mut x = Simple::new(S_UNIVERSAL);
                x.ns = ns_c;
                x
            } else if name_tok.kind == Kind::Ident {
                let mut x = Simple::new(S_TYPE);
                x.ns = ns_c;
                x.a = t.push_name(decode(s, name_tok.inner), false)?;
                f.spec.c = f.spec.c.saturating_add(1);
                x
            } else {
                return Err(invalid());
            };
            add_simple(f, t, simple, false)?;
            f.has_type = true;
            Ok(())
        }
        Kind::Hash if tok.flag => {
            let mut x = Simple::new(S_ID);
            x.a = t.push_name(decode(s, tok.inner), false)?;
            f.spec.a = f.spec.a.saturating_add(1);
            add_simple(f, t, x, false)
        }
        Kind::Delim if tok.is_delim(b'.') => {
            let n = cur.next()?;
            if n.kind != Kind::Ident {
                return Err(invalid());
            }
            let mut x = Simple::new(S_CLASS);
            x.a = t.push_name(decode(s, n.inner), false)?;
            if no_token(x.a.of(t.names.as_slice()), t.names.budget())? {
                x.flags |= FLAG_NO_TOKEN;
            }
            f.spec.b = f.spec.b.saturating_add(1);
            add_simple(f, t, x, false)
        }
        Kind::LBracket => {
            let x = attribute(cur, s, t, ns)?;
            f.spec.b = f.spec.b.saturating_add(1);
            add_simple(f, t, x, false)
        }
        Kind::Colon => pseudo(cur, s, f, t, frames),
        _ => Err(invalid()),
    }
}

/// 属性のセレクタ（`[` を読んだあと）。
fn attribute(cur: &mut Cursor<'_, '_, '_>, s: &[u8], t: &mut Tables<'_>, ns: NsTable<'_>) -> R<Simple> {
    let budget = t.names.budget();
    cur.skip_ws()?;
    let first = cur.next()?;
    let (ns_c, name_tok) = if first.is_delim(b'|') {
        (NS_NONE, cur.next()?)
    } else if cur.peek()?.is_delim(b'|') && !peek2_is_eq(cur)? {
        cur.next()?;
        let c = if first.is_delim(b'*') {
            NS_ANY
        } else if first.kind == Kind::Ident {
            ns.lookup(s, &first, budget)?.ok_or_else(invalid)?
        } else {
            return Err(invalid());
        };
        (c, cur.next()?)
    } else {
        // 接頭辞のない属性は名前空間のない属性だけに当たる
        (NS_NONE, first)
    };
    if name_tok.kind != Kind::Ident {
        return Err(invalid());
    }
    let mut x = Simple::new(S_ATTR);
    x.ns = ns_c;
    x.a = t.push_name(decode(s, name_tok.inner), false)?;
    cur.skip_ws()?;
    let op_tok = cur.next()?;
    if op_tok.kind == Kind::RBracket {
        x.op = OP_EXISTS;
        return Ok(x);
    }
    x.op = if op_tok.is_delim(b'=') {
        OP_EQ
    } else {
        let op = match op_tok.delim {
            0x7E if op_tok.kind == Kind::Delim => OP_INCLUDES,
            0x7C if op_tok.kind == Kind::Delim => OP_DASH,
            0x5E if op_tok.kind == Kind::Delim => OP_PREFIX,
            0x24 if op_tok.kind == Kind::Delim => OP_SUFFIX,
            0x2A if op_tok.kind == Kind::Delim => OP_SUBSTR,
            _ => return Err(invalid()),
        };
        if !cur.next()?.is_delim(b'=') {
            return Err(invalid());
        }
        op
    };
    cur.skip_ws()?;
    let v = cur.next()?;
    x.b = match v.kind {
        Kind::Ident => t.push_name(decode(s, v.inner), false)?,
        Kind::Str => t.push_name_str(s, v.inner, false)?,
        _ => return Err(invalid()),
    };
    cur.skip_ws()?;
    let mut end = cur.next()?;
    if end.kind == Kind::Ident {
        // `s` の印は Chrome が捨てる（単位 3 の 11 回目のレビュー）
        if ident_is(s, end.inner, end.escaped, b"i") {
            x.flags = FLAG_CASE_I;
        } else {
            return Err(invalid());
        }
        cur.skip_ws()?;
        end = cur.next()?;
    }
    if end.kind != Kind::RBracket {
        return Err(invalid());
    }
    if x.op == OP_INCLUDES && no_token(x.b.of(t.names.as_slice()), budget)? {
        x.flags |= FLAG_NO_TOKEN;
    }
    Ok(x)
}

/// 先に見た `|` のすぐ次が `=` か（`[a|=v]` の比べ方で、接頭辞の `|` でない）。
fn peek2_is_eq(cur: &mut Cursor<'_, '_, '_>) -> Result<bool, Error> {
    let p = cur.peek()?;
    Ok(cur.lex.byte_at(p.span.end as usize)? == Some(b'='))
}

/// 擬似クラス・擬似要素（`:` を読んだあと）。
fn pseudo(
    cur: &mut Cursor<'_, '_, '_>,
    s: &[u8],
    f: &mut Frame,
    t: &mut Tables<'_>,
    frames: &mut MVec<'_, Frame>,
) -> R<()> {
    let budget = t.names.budget();
    let mut n = cur.next()?;
    let element = n.kind == Kind::Colon;
    if element {
        n = cur.next()?;
    }
    // 名前は 1 度だけ解いて、言葉の表と比べる（escape の名前を言葉ごとに解き直さない）
    let nm = crate::css::lex::name_of(s, n.inner, n.escaped, budget)?;
    let is = |w: &[u8]| nm.is(w);
    let any = |list: &[&[u8]]| list.iter().any(|w| nm.is(w));
    let webkit = || n.inner.of(s).get(..8).is_some_and(|p| p.eq_ignore_ascii_case(b"-webkit-")) && !n.escaped;
    let legacy = n.kind == Kind::Ident && any(LEGACY_PSEUDO_ELEMENTS);
    if element || legacy {
        // 擬似要素：いちばん外の並びにだけ書ける
        // 関数の形は引数を文法で確かめる（Chrome と仕様が捨てる形を受けない。単位 3 の 11 回目のレビュー）
        // 複合の引数（`::slotted()`・`::cue()`）は入れ子の並びとして読む（再帰しない）
        let mut compound_arg = 0u8;
        let scrollbar = n.kind == Kind::Ident && !n.escaped && any(SCROLLBAR_ELEMENTS);
        let (known, listed) = match n.kind {
            Kind::Ident => (any(PSEUDO_ELEMENTS) || webkit(), any(PSEUDO_ELEMENTS) || scrollbar),
            Kind::Function => match pseudo_element_form(&nm) {
                Some(ArgForm::Compound) => {
                    compound_arg = 1;
                    (true, true)
                }
                Some(ArgForm::CompoundList) => {
                    compound_arg = 2;
                    (true, true)
                }
                Some(form) => {
                    let arg = cur.skip_function()?;
                    (args_ok(s, arg, form, budget)?, true)
                }
                None => {
                    cur.skip_function()?;
                    (webkit(), false)
                }
            },
            _ => (false, false),
        };
        if !known || !f.top {
            return Err(invalid());
        }
        if !listed {
            f.any_webkit = true;
        }
        let follows = match f.pe {
            PE_NONE => true,
            PE_BEFORE_AFTER => n.kind == Kind::Ident && is(b"marker"),
            // 要素に支えられた擬似要素のあとは `::part()`・`::slotted()` でない擬似要素（単位 3 の 13 回目のレビュー）
            // （関数の形の `::cue()` と表にない関数の形の `::-webkit-*()` も書けない。単位 3 の 14 回目のレビュー）
            PE_PART => !(n.kind == Kind::Function && (is(b"part") || is(b"slotted") || is(b"cue") || !listed)),
            PE_SLOTTED => n.kind == Kind::Ident && any(AFTER_SLOTTED),
            PE_COLUMN => n.kind == Kind::Ident && is(b"scroll-marker"),
            _ => false,
        };
        if !follows {
            return Err(invalid());
        }
        // 擬似要素ごとに、あとに書ける擬似クラス（Chrome 157 と照らした。単位 3 の 12 回目のレビュー）
        let ident = n.kind == Kind::Ident;
        let func = n.kind == Kind::Function;
        let next_pe = if ident && (is(b"before") || is(b"after")) {
            PE_BEFORE_AFTER
        } else if scrollbar {
            PE_SCROLLBAR
        } else if !listed {
            PE_WEBKIT
        } else if (func && (is(b"part") || is(b"picker"))) || (ident && is(b"details-content")) {
            PE_PART
        } else if ident && is(b"selection") {
            PE_SELECTION
        } else if func && is(b"slotted") {
            PE_SLOTTED
        } else if ident && is(b"column") {
            PE_COLUMN
        } else if ident && (is(b"file-selector-button") || is(b"cue")) {
            PE_USER
        } else if ident && is(b"scroll-marker") {
            PE_SCROLL_MARKER
        } else if ident && is(b"scroll-marker-group") {
            PE_MARKER_GROUP
        } else if ident && is(b"search-text") {
            PE_SEARCH_TEXT
        } else if func && is(b"scroll-button") {
            PE_SCROLL_BUTTON
        } else if func && pseudo_element_form(&nm) == Some(ArgForm::ViewTransition) {
            PE_VIEW_TRANSITION
        } else {
            PE_OTHER
        };
        f.spec.c = f.spec.c.saturating_add(1);
        add_simple(f, t, Simple::new(S_NEVER), true)?;
        f.pe = next_pe;
        if compound_arg != 0 {
            let mut g = Frame::new(S_NEVER, false, false, false, true).at(cur.depth());
            g.compound = true;
            g.arg = compound_arg;
            frames.push(g)?;
            cur.skip_ws()?;
        }
        return Ok(());
    }
    match n.kind {
        Kind::Ident => {
            let kind = if is(b"first-child") {
                S_FIRST_CHILD
            } else if is(b"last-child") {
                S_LAST_CHILD
            } else if is(b"only-child") {
                S_ONLY_CHILD
            } else if is(b"first-of-type") {
                S_FIRST_OF_TYPE
            } else if is(b"last-of-type") {
                S_LAST_OF_TYPE
            } else if is(b"only-of-type") {
                S_ONLY_OF_TYPE
            } else if is(b"root") || is(b"scope") {
                S_ROOT
            } else if is(b"empty") {
                S_EMPTY
            } else if is(b"link") || is(b"any-link") || is(b"-webkit-any-link") {
                S_LINK
            } else if is(b"defined") {
                S_UNIVERSAL
            } else if any(DYNAMIC) || any(SCROLLBAR_CLASSES) || any(OTHER_NEVER) {
                S_NEVER
            } else {
                return Err(invalid());
            };
            // 擬似要素のあとに書ける擬似クラス（擬似要素ごと。Chrome 157 と照らした。単位 3 の 11・12 回目のレビュー）
            let pe = f.pe_ctx();
            let user = any(USER_ACTION);
            let after_pe = match pe {
                PE_PART => {
                    matches!(kind, S_NEVER | S_LINK | S_UNIVERSAL)
                        && !any(SCROLLBAR_CLASSES)
                        && !is(b"current")
                        && !is(b"host")
                }
                PE_SCROLLBAR => {
                    is(b"hover")
                        || is(b"active")
                        || any(SCROLLBAR_CLASSES)
                        || is(b"window-inactive")
                        || is(b"enabled")
                        || is(b"disabled")
                }
                PE_USER | PE_WEBKIT => user,
                PE_SCROLL_MARKER => user || is(b"target-current"),
                PE_SCROLL_BUTTON => user || is(b"enabled") || is(b"disabled"),
                PE_MARKER_GROUP => is(b"hover") || is(b"focus-within"),
                PE_VIEW_TRANSITION => is(b"only-child"),
                PE_SEARCH_TEXT => is(b"current"),
                PE_SELECTION => is(b"window-inactive"),
                _ => false,
            };
            if pe != PE_NONE && !after_pe {
                return Err(invalid());
            }
            f.spec.b = f.spec.b.saturating_add(1);
            let mut x = Simple::new(kind);
            x.ns = NS_ANY;
            add_simple(f, t, x, true)
        }
        Kind::Function => {
            let pe = f.pe_ctx();
            // 擬似要素のあとに書ける関数は論理の擬似クラスだけ（中もそのあとに書ける擬似クラスだけ。Selectors 4 §3.6.3）。
            // 要素に支えられた擬似要素のあとは `:state()`・`:dir()`・`:lang()`・`:active-view-transition-type()` も、`::column`
            // のあとは論理の擬似クラスも書けない（Chrome と同じ。単位 3 の 13 回目のレビュー）
            let backed = pe == PE_PART;
            let backed_fn = is(b"state") || is(b"dir") || is(b"lang") || is(b"active-view-transition-type");
            let logical = is(b"not") || is(b"is") || is(b"where");
            if pe != PE_NONE && !((logical && pe != PE_COLUMN) || (backed && backed_fn)) {
                cur.skip_function()?;
                return Err(invalid());
            }
            if is(b"not") || is(b"is") || is(b"where") || is(b"-webkit-any") {
                let webkit_any = is(b"-webkit-any");
                let (kind, forgiving, zero) = if is(b"not") {
                    (S_NOT, false, false)
                } else if is(b"where") {
                    (S_IS, true, true)
                } else if webkit_any {
                    (S_IS, false, false)
                } else {
                    (S_IS, true, false)
                };
                // 入れ子の並びを始める（外の複合の状態 `f` は、呼び手が外の段に書き戻す）。`:-webkit-any()` の中の並びにも
                // 結合子は書けない（`:is()`・`:where()` は寛容なまま。Chrome と同じ。単位 3 の 13・14 回目のレビュー）
                let mut g = Frame::new(kind, false, forgiving, zero, f.in_has).at(cur.depth());
                g.outer_pe = pe;
                g.webkit_any = webkit_any;
                g.compound = f.compound || webkit_any;
                frames.push(g)?;
                cur.skip_ws()?;
                return Ok(());
            }
            if is(b"has") {
                // `:has()` は入れ子にできない
                if f.in_has {
                    cur.skip_function()?;
                    return Err(invalid());
                }
                let mut g = Frame::new(S_NEVER, false, false, false, true).at(cur.depth());
                g.opaque = true;
                g.relative = true;
                frames.push(g)?;
                cur.skip_ws()?;
                return Ok(());
            }
            let nth = if is(b"nth-child") {
                Some(S_NTH_CHILD)
            } else if is(b"nth-last-child") {
                Some(S_NTH_LAST_CHILD)
            } else if is(b"nth-of-type") {
                Some(S_NTH_OF_TYPE)
            } else if is(b"nth-last-of-type") {
                Some(S_NTH_LAST_OF_TYPE)
            } else {
                None
            };
            if let Some(kind) = nth {
                let Some((a, b)) = an_b(cur, s, budget)? else {
                    cur.skip_function()?;
                    return Err(invalid());
                };
                f.spec.b = f.spec.b.saturating_add(1);
                cur.skip_ws()?;
                let p = cur.peek()?;
                if p.kind == Kind::RParen {
                    cur.next()?;
                    let mut x = Simple::new(kind);
                    x.n_a = a;
                    x.n_b = b;
                    return add_simple(f, t, x, false);
                }
                if p.kind == Kind::Ident
                    && ident_is(s, p.inner, p.escaped, b"of")
                    && matches!(kind, S_NTH_CHILD | S_NTH_LAST_CHILD)
                {
                    // `of S`：S を確かめて読むが当てない
                    cur.next()?;
                    let mut g = Frame::new(S_NEVER, false, false, false, f.in_has).at(cur.depth());
                    g.opaque = true;
                    frames.push(g)?;
                    cur.skip_ws()?;
                    return Ok(());
                }
                cur.skip_function()?;
                return Err(invalid());
            }
            if is(b"lang") {
                // 言語の範囲は 1 つの名前だけ（文字列と並びは Chrome が捨てる。単位 3 の 11 回目のレビュー）
                let arg = cur.skip_function()?;
                if !args_ok(s, arg, ArgForm::Ident, budget)? {
                    return Err(invalid());
                }
                let ranges = lang_ranges(s, arg, t)?;
                let mut x = Simple::new(S_LANG);
                x.a = ranges;
                f.spec.b = f.spec.b.saturating_add(1);
                return add_simple(f, t, x, backed);
            }
            if is(b"dir") {
                // SVG の要素の向きはいつも ltr（HTML の `dir` 属性を持たない）
                cur.skip_ws()?;
                let a = cur.peek()?;
                let mut kind = S_NEVER;
                if a.kind == Kind::Ident {
                    cur.next()?;
                    cur.skip_ws()?;
                    if cur.peek()?.kind == Kind::RParen {
                        cur.next()?;
                        if ident_is(s, a.inner, a.escaped, b"ltr") {
                            kind = S_UNIVERSAL;
                        }
                        f.spec.b = f.spec.b.saturating_add(1);
                        return add_simple(f, t, Simple::new(kind), backed);
                    }
                }
                cur.skip_function()?;
                return Err(invalid());
            }
            if let Some(form) = never_function_form(&nm) {
                // 正しいが当てない（引数は文法で確かめる。複合の引数は入れ子の並びとして読む。単位 3 の 11・12 回目の
                // レビュー）
                f.spec.b = f.spec.b.saturating_add(1);
                if form == ArgForm::Compound {
                    add_simple(f, t, Simple::new(S_NEVER), false)?;
                    let mut g = Frame::new(S_NEVER, false, false, false, true).at(cur.depth());
                    g.compound = true;
                    g.arg = 1;
                    frames.push(g)?;
                    cur.skip_ws()?;
                    return Ok(());
                }
                let arg = cur.skip_function()?;
                if !args_ok(s, arg, form, budget)? {
                    return Err(invalid());
                }
                return add_simple(f, t, Simple::new(S_NEVER), backed);
            }
            cur.skip_function()?;
            Err(invalid())
        }
        _ => Err(invalid()),
    }
}

/// 1 つの `:` でも書ける昔の擬似要素。
const LEGACY_PSEUDO_ELEMENTS: &[&[u8]] = &[b"before", b"after", b"first-line", b"first-letter"];

/// 擬似要素（CSS Pseudo 4 などの仕様にあるもの。`::-webkit-*` も受ける。Chrome と同じ）。
const PSEUDO_ELEMENTS: &[&[u8]] = &[
    b"before",
    b"after",
    b"first-line",
    b"first-letter",
    b"marker",
    b"placeholder",
    b"selection",
    b"target-text",
    b"spelling-error",
    b"grammar-error",
    b"search-text",
    b"file-selector-button",
    b"backdrop",
    b"cue",
    b"details-content",
    b"view-transition",
    b"scroll-marker",
    b"scroll-marker-group",
    b"column",
    b"checkmark",
    b"picker-icon",
];

/// 利用者の動きの擬似クラス（擬似要素のあとにも書ける。Selectors 4 §3.6.3）。
const USER_ACTION: &[&[u8]] = &[b"hover", b"active", b"focus", b"focus-visible", b"focus-within"];

/// 静止した絵では当たらない擬似クラス（正しい名前として受ける。Selectors 4 と HTML などの仕様にあるもの、Chrome が受ける
/// `-webkit-` のもの。Chrome の知らない `:playing`・`:paused`・`:seeking`・`:buffering`・`:stalled`・`:muted`・`:local-link`
/// は除く。単位 3 の 13 回目のレビュー）。
const DYNAMIC: &[&[u8]] = &[
    b"hover",
    b"active",
    b"focus",
    b"focus-within",
    b"focus-visible",
    b"visited",
    b"target",
    b"target-current",
    b"checked",
    b"disabled",
    b"enabled",
    b"default",
    b"indeterminate",
    b"invalid",
    b"valid",
    b"in-range",
    b"out-of-range",
    b"required",
    b"optional",
    b"read-only",
    b"read-write",
    b"placeholder-shown",
    b"autofill",
    b"-webkit-autofill",
    b"user-invalid",
    b"user-valid",
    b"current",
    b"past",
    b"future",
    b"fullscreen",
    b"-webkit-full-screen",
    b"modal",
    b"picture-in-picture",
    b"popover-open",
    b"open",
    b"active-view-transition",
    b"xr-overlay",
    b"host",
];

/// Chrome が受けるスクロールバーの擬似クラス（静止した絵では当たらない。単独でも書け、スクロールバーの擬似要素のあとに書ける。
/// `::part()` のあとには書けない。単位 3 の 11・12 回目のレビュー。ADR-73 (6)）。
const SCROLLBAR_CLASSES: &[&[u8]] = &[
    b"horizontal",
    b"vertical",
    b"increment",
    b"decrement",
    b"start",
    b"end",
    b"double-button",
    b"single-button",
    b"no-button",
    b"corner-present",
];

/// Chrome が受けるほかの当てない擬似クラス（`:window-inactive` は `::selection` とスクロールバーのあとにも書ける）。
const OTHER_NEVER: &[&[u8]] =
    &[b"window-inactive", b"-webkit-drag", b"-webkit-full-page-media", b"-webkit-full-screen-ancestor"];

/// スクロールバーの擬似要素（Chrome の知る `::-webkit-*`。名前全体で比べる）。
const SCROLLBAR_ELEMENTS: &[&[u8]] = &[
    b"-webkit-scrollbar",
    b"-webkit-scrollbar-button",
    b"-webkit-scrollbar-corner",
    b"-webkit-scrollbar-thumb",
    b"-webkit-scrollbar-track",
    b"-webkit-scrollbar-track-piece",
    b"-webkit-resizer",
];

/// 関数の擬似クラス・擬似要素の引数の形（仕様の文法と Chrome。単位 3 の 11 回目のレビュー）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgForm {
    /// 名前 1 つ以上（空白で区切る）
    Idents,
    /// 名前 1 つ
    Ident,
    /// 名前の `,` の並び
    IdentList,
    /// `select` だけ
    Select,
    /// `*` か向きの言葉
    ScrollButton,
    /// `*` か名前と、`.名前` の続き
    ViewTransition,
    /// 複合の選択子 1 つ
    Compound,
    /// 複合の選択子の `,` の並び
    CompoundList,
}

/// 正しいが当てない関数の擬似クラスと引数の形（`:current()`・`:nth-col()`・`:nth-last-col()` は Chrome が捨てる）。
fn never_function_form(nm: &crate::css::lex::Name) -> Option<ArgForm> {
    if nm.is(b"host") || nm.is(b"host-context") {
        Some(ArgForm::Compound)
    } else if nm.is(b"state") {
        Some(ArgForm::Ident)
    } else if nm.is(b"active-view-transition-type") {
        Some(ArgForm::IdentList)
    } else {
        None
    }
}

/// 関数の形の擬似要素の引数の形。
fn pseudo_element_form(nm: &crate::css::lex::Name) -> Option<ArgForm> {
    if nm.is(b"cue") {
        Some(ArgForm::CompoundList)
    } else if nm.is(b"part") {
        Some(ArgForm::Idents)
    } else if nm.is(b"slotted") {
        Some(ArgForm::Compound)
    } else if nm.is(b"highlight") {
        Some(ArgForm::Ident)
    } else if nm.is(b"view-transition-group")
        || nm.is(b"view-transition-image-pair")
        || nm.is(b"view-transition-old")
        || nm.is(b"view-transition-new")
    {
        Some(ArgForm::ViewTransition)
    } else if nm.is(b"scroll-button") {
        Some(ArgForm::ScrollButton)
    } else if nm.is(b"picker") {
        Some(ArgForm::Select)
    } else {
        None
    }
}

/// `::scroll-button()` の向きの言葉。
const SCROLL_DIRECTIONS: &[&[u8]] =
    &[b"up", b"down", b"left", b"right", b"block-start", b"block-end", b"inline-start", b"inline-end"];

/// 関数の引数 `arg` が `form` の形か（入れ子のかっこの中は 1 つの字句として見る）。字句ごとに払う。
fn args_ok(s: &[u8], arg: Span, form: ArgForm, budget: &Budget<'_>) -> Result<bool, Error> {
    let mut lex = Lexer::over(s, arg, budget);
    let mut nest = Nest::new(budget);
    let list = matches!(form, ArgForm::IdentList | ArgForm::CompoundList);
    // 今の項の字句の数・項の中の空白・前の字句が `.` か・前の字句が名前か
    let (mut items, mut ws, mut dot, mut name) = (0u32, false, false, false);
    loop {
        budget.spend(cost::OP_STEP)?;
        let t = lex.next()?;
        if t.kind == Kind::Eof {
            break;
        }
        if !nest.top() {
            nest.feed(&t)?;
            continue;
        }
        match t.kind {
            Kind::Whitespace => {
                ws |= items > 0;
                continue;
            }
            Kind::Comma => {
                if !list || items == 0 || dot {
                    return Ok(false);
                }
                (items, ws, dot, name) = (0, false, false, false);
                continue;
            }
            _ => {}
        }
        let is = |w: &[u8]| t.kind == Kind::Ident && ident_is(s, t.inner, t.escaped, w);
        let ok = match form {
            ArgForm::Idents => t.kind == Kind::Ident,
            ArgForm::Ident | ArgForm::IdentList => items == 0 && t.kind == Kind::Ident,
            ArgForm::Select => items == 0 && is(b"select"),
            ArgForm::ScrollButton => items == 0 && (t.is_delim(b'*') || SCROLL_DIRECTIONS.iter().any(|w| is(w))),
            // 名前の直後の空白だけ書ける（`*` と `.` の直後は書けない。Chrome と同じ）
            ArgForm::ViewTransition => {
                (!ws || name)
                    && if dot {
                        t.kind == Kind::Ident
                    } else if items == 0 {
                        t.kind == Kind::Ident || t.is_delim(b'*') || t.is_delim(b'.')
                    } else {
                        t.is_delim(b'.')
                    }
            }
            // 複合の引数は入れ子の並びとして読む（ここへは来ない）
            ArgForm::Compound | ArgForm::CompoundList => false,
        };
        if !ok {
            return Ok(false);
        }
        dot = t.is_delim(b'.');
        name = t.kind == Kind::Ident;
        ws = false;
        items = items.saturating_add(1);
        nest.feed(&t)?;
    }
    Ok(items > 0 && !dot)
}

/// An+B の名前の形（`n`・`-n`・`n-`・`-n-`・`n-<数字>`・`-n-<数字>`）。
struct NForm {
    neg: bool,
    dash: bool,
    digits: Option<i32>,
}

/// 名前（ident か dimension の単位。escape を解く）を An+B の形として読む（`allow_neg` なら頭の `-` を許す）。
fn n_form(s: &[u8], span: Span, allow_neg: bool, budget: &Budget<'_>) -> Result<Option<NForm>, Error> {
    let mut it = decode(s, span);
    let mut paid = 0u32;
    // 64 字の塊ごとに、解く前に払う（1 字は生のバイトで 9 まで）
    let mut next = |budget: &Budget<'_>| -> Result<Option<u32>, Error> {
        if paid.trailing_zeros() >= 6 {
            budget.spend(cost::decode(64))?;
        }
        paid = paid.wrapping_add(1);
        Ok(it.next())
    };
    budget.spend(cost::OP_STEP)?;
    let mut c = next(budget)?;
    let mut neg = false;
    if allow_neg && c == Some(u32::from(b'-')) {
        neg = true;
        c = next(budget)?;
    }
    if !matches!(c, Some(0x6E | 0x4E)) {
        return Ok(None);
    }
    c = next(budget)?;
    let Some(dash) = c else { return Ok(Some(NForm { neg, dash: false, digits: None })) };
    if dash != u32::from(b'-') {
        return Ok(None);
    }
    c = next(budget)?;
    if c.is_none() {
        return Ok(Some(NForm { neg, dash: true, digits: None }));
    }
    let mut v: i64 = 0;
    while let Some(d) = c {
        if !(0x30..=0x39).contains(&d) {
            return Ok(None);
        }
        v = v.saturating_mul(10).saturating_add(i64::from(d.saturating_sub(0x30)));
        c = next(budget)?;
    }
    Ok(Some(NForm { neg, dash: true, digits: Some(clamp_i32(v)) }))
}

/// 整数の数の字句の値（i32 で止める）。
fn int_value(t: &Token) -> i32 {
    let v = t.value;
    if v >= 2_147_483_647.0 {
        i32::MAX
    } else if v <= -2_147_483_648.0 {
        i32::MIN
    } else {
        v as i32
    }
}

/// 整数の数の字句で、符号があるか（`signed`）・ないか。
fn integer_with_sign(s: &[u8], t: &Token, signed: bool) -> bool {
    t.kind == Kind::Number
        && t.flag
        && t.finite
        && s.get(t.span.start as usize).is_some_and(|&b| matches!(b, b'+' | b'-')) == signed
}

/// An+B（CSS Syntax §6.2。字句の並びで読む。`n` の後ろの `-` は名前の一部）。読めなければ None（読めた所までは読んで
/// いて、残りは呼び手が関数の終わりまで飛ばす。閉じのかっこは読まない）。
fn an_b(cur: &mut Cursor<'_, '_, '_>, s: &[u8], budget: &Budget<'_>) -> Result<Option<(i32, i32)>, Error> {
    cur.skip_ws()?;
    let t = cur.peek()?;
    let (a, form) = match t.kind {
        Kind::Ident if ident_is(s, t.inner, t.escaped, b"odd") => {
            cur.next()?;
            return Ok(Some((2, 1)));
        }
        Kind::Ident if ident_is(s, t.inner, t.escaped, b"even") => {
            cur.next()?;
            return Ok(Some((2, 0)));
        }
        Kind::Ident => {
            let Some(form) = n_form(s, t.inner, true, budget)? else { return Ok(None) };
            cur.next()?;
            (if form.neg { -1 } else { 1 }, form)
        }
        Kind::Delim if t.is_delim(b'+') => {
            // `+n…`（`+` と名前の間に空白は書けない）
            cur.next()?;
            let u = cur.peek()?;
            if u.kind != Kind::Ident {
                return Ok(None);
            }
            let Some(form) = n_form(s, u.inner, false, budget)? else { return Ok(None) };
            cur.next()?;
            (1, form)
        }
        Kind::Number if t.flag && t.finite => {
            cur.next()?;
            return Ok(Some((0, int_value(&t))));
        }
        Kind::Dimension if t.flag && t.finite => {
            let Some(form) = n_form(s, t.inner, false, budget)? else { return Ok(None) };
            cur.next()?;
            (int_value(&t), form)
        }
        _ => return Ok(None),
    };
    if let Some(d) = form.digits {
        // `n-<数字>`
        return Ok(Some((a, d.saturating_neg())));
    }
    if form.dash {
        // `n- <符号のない整数>`
        cur.skip_ws()?;
        let u = cur.peek()?;
        if !integer_with_sign(s, &u, false) {
            return Ok(None);
        }
        cur.next()?;
        return Ok(Some((a, int_value(&u).saturating_neg())));
    }
    // `n`、続けて `<符号つきの整数>` か `['+' | '-'] <符号のない整数>`
    cur.skip_ws()?;
    let p = cur.peek()?;
    if integer_with_sign(s, &p, true) {
        cur.next()?;
        return Ok(Some((a, int_value(&p))));
    }
    if p.is_delim(b'+') || p.is_delim(b'-') {
        cur.next()?;
        cur.skip_ws()?;
        let u = cur.peek()?;
        if !integer_with_sign(s, &u, false) {
            return Ok(None);
        }
        cur.next()?;
        let b = int_value(&u);
        return Ok(Some((a, if p.is_delim(b'-') { b.saturating_neg() } else { b })));
    }
    Ok(Some((a, 0)))
}

fn clamp_i32(v: i64) -> i32 {
    i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX })
}

/// `:lang()` の言語の範囲（ident か文字列を `,` で区切ったもの）を、小文字にして NUL でつないで名前の置き場に足す
/// （解いた名前と文字列に NUL は現れない：CSS Syntax の前処理と escape の `\0` は U+FFFD。文字列の範囲の中の `,` で
/// 範囲を分けない）。
fn lang_ranges(s: &[u8], arg: Span, t: &mut Tables<'_>) -> R<Span> {
    let budget = t.names.budget();
    let mut lex = Lexer::over(s, arg, budget);
    let start = t.names.len();
    let mut any = false;
    let mut want_item = true;
    loop {
        let tok = lex.next()?;
        match tok.kind {
            Kind::Whitespace => {}
            Kind::Eof => break,
            Kind::Ident | Kind::Str if want_item => {
                if any {
                    t.names.push(LANG_SEP)?;
                }
                if tok.kind == Kind::Ident {
                    t.push_name(decode(s, tok.inner), true)?;
                } else {
                    t.push_name_str(s, tok.inner, true)?;
                }
                any = true;
                want_item = false;
            }
            Kind::Comma if !want_item => want_item = true,
            _ => return Err(invalid()),
        }
    }
    if !any || want_item {
        return Err(invalid());
    }
    Ok(Span::new(start, t.names.len()))
}

#[cfg(test)]
mod tests;
