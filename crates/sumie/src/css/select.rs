//! セレクタ（Selectors Level 3 と Level 4 の一部。FR-ST-07・ADR-66 (1)）を読み、選択子・複合・単純な選択子の表に足す。
//!
//! - 表は番号でつなぐ（複合は左の複合を `prev`、単純な選択子は `next`、並びの選択子は `next` で指す）。足す順は「中の並び
//!   が先」：`:is()` などの中の選択子と複合は、それを持つ外の複合より前に足すので、番号の順に照らせば中が先に決まる
//! - `:is()`・`:where()` の並びは寛容（壊れた選択子だけを捨てる）。`:not()` と規則の並びは、1 つでも壊れていれば全体が壊れる
//!   （壊れた中の並びは、外の寛容な並びまで上がって、その選択子を捨てる）
//! - 当てない擬似クラス（`:hover` など）・擬似要素・`:has()`・`:nth-child(… of S)` は正しいセレクタとして読み、当たらない
//!   印（`never`）を付ける
//! - 再帰しない（入れ子の並びは明示の積み上げ）

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::{Kind, Lexer, Token, decode, decode_string, ident_is};
use crate::error::Error;
use crate::mem::{MVec, Plain};
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
    pub(crate) pad: [u8; 3],
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

    fn push_simple(&mut self, s: Simple) -> Result<u32, Error> {
        let id = self.simples.len() as u32;
        self.simples.push(s)?;
        Ok(id)
    }
}

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

/// `@namespace` の表（規則の並びごと）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct NsDecl {
    /// 接頭辞（名前の置き場の中）。既定の名前空間は空
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

    fn unpack(p: u32) -> Spec {
        Spec { a: (p >> 20) & 1023, b: (p >> 10) & 1023, c: p & 1023 }
    }

    fn add(self, o: Spec) -> Spec {
        Spec { a: self.a.saturating_add(o.a), b: self.b.saturating_add(o.b), c: self.c.saturating_add(o.c) }
    }
}

/// 入れ子の並び（規則の並び・`:is()`・`:where()`・`:not()`）を読んでいる所。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Frame {
    /// この並びを持つ単純な選択子の種類（規則の並びは NONE の代わりに S_NEVER を使わず、`top` で見分ける）
    kind: u8,
    top: bool,
    forgiving: bool,
    /// `:where()` は詳細度 0
    zero: bool,
    first_sel: u32,
    last_sel: u32,
    max_spec: Spec,
    /// 読んでいる選択子
    prev_compound: u32,
    pending_comb: u8,
    spec: Spec,
    /// 読んでいる複合
    first_simple: u32,
    last_simple: u32,
    compound_empty: bool,
    /// 複合に擬似要素を読んだ（そのあとに単純な選択子は書けない）
    after_pseudo_element: bool,
    /// 読んでいる選択子が壊れた
    broken: bool,
}

impl Frame {
    fn new(kind: u8, top: bool, forgiving: bool, zero: bool) -> Frame {
        Frame {
            kind,
            top,
            forgiving,
            zero,
            first_sel: NONE,
            last_sel: NONE,
            max_spec: Spec::default(),
            prev_compound: NONE,
            pending_comb: COMB_NONE,
            spec: Spec::default(),
            first_simple: NONE,
            last_simple: NONE,
            compound_empty: true,
            after_pseudo_element: false,
            broken: false,
        }
    }
}

/// 1 つ先を見られる字句の口。
struct Cursor<'l, 'a, 'b> {
    lex: &'l mut Lexer<'a, 'b>,
    peeked: Option<Token>,
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

    fn next(&mut self) -> Result<Token, Error> {
        if let Some(t) = self.peeked.take() {
            return Ok(t);
        }
        self.lex.next()
    }

    fn skip_ws(&mut self) -> Result<bool, Error> {
        let mut any = false;
        while self.peek()?.kind == Kind::Whitespace {
            self.next()?;
            any = true;
        }
        Ok(any)
    }

    /// 深さ 0 の `,` か `)`（読まない）・終わりまで飛ばす。
    fn skip_to_list_end(&mut self) -> Result<(), Error> {
        let mut depth: u32 = 0;
        loop {
            let t = self.peek()?;
            match t.kind {
                Kind::Eof => return Ok(()),
                Kind::Comma | Kind::RParen if depth == 0 => return Ok(()),
                Kind::LParen | Kind::LBracket | Kind::LBrace | Kind::Function => depth = depth.saturating_add(1),
                Kind::RParen | Kind::RBracket | Kind::RBrace => depth = depth.saturating_sub(1),
                _ => {}
            }
            self.next()?;
        }
    }

    /// 関数の中身を `)` まで読み飛ばし、中身の範囲（`)` の前まで）を返す。
    fn skip_function(&mut self) -> Result<Span, Error> {
        let start = self.peek()?.span.start;
        let mut end = start;
        let mut depth: u32 = 0;
        loop {
            let t = self.next()?;
            match t.kind {
                Kind::Eof => return Ok(Span { start, end: t.span.start }),
                Kind::RParen if depth == 0 => return Ok(Span { start, end }),
                Kind::LParen | Kind::LBracket | Kind::LBrace | Kind::Function => depth = depth.saturating_add(1),
                Kind::RParen | Kind::RBracket | Kind::RBrace => depth = depth.saturating_sub(1),
                _ => {}
            }
            end = t.span.end;
        }
    }
}

/// 既定の名前空間（型と `*` に当てる。宣言がなければどれでも）。後の宣言が勝つ。
fn default_ns(decls: &[NsDecl]) -> u8 {
    decls.iter().rev().find(|d| d.prefix.len() == 0).map_or(NS_ANY, |d| d.ns)
}

/// 接頭辞の名前空間（宣言のないものは None）。
fn ns_lookup(decls: &[NsDecl], names: &[u8], s: &[u8], t: &Token) -> Option<u8> {
    decls.iter().rev().find(|d| d.prefix.len() > 0 && name_eq(s, t, d.prefix.of(names))).map(|d| d.ns)
}

/// 字句の中身（解いたもの）が `want`（UTF-8）と同じか。
fn name_eq(s: &[u8], t: &Token, want: &[u8]) -> bool {
    if !t.escaped {
        return t.inner.of(s) == want;
    }
    let mut k = 0usize;
    for c in decode(s, t.inner) {
        let (buf, n) = utf8(c);
        let Some(part) = buf.get(..n) else { return false };
        let Some(w) = want.get(k..k.saturating_add(n)) else { return false };
        if w != part {
            return false;
        }
        k = k.saturating_add(n);
    }
    k == want.len()
}

/// 選択子の並び `range` を読み、最初の選択子の番号を返す（壊れていれば None。表に足した分は捨てずに残る）。並びの
/// 選択子は `next` でつなぐ。
pub(crate) fn parse_selector_list(
    s: &[u8],
    range: Span,
    ns: &[NsDecl],
    t: &mut Tables<'_>,
    budget: &Budget<'_>,
) -> Result<Option<u32>, Error> {
    match parse_list(s, range, ns, t, budget) {
        Ok(first) => Ok(Some(first)),
        Err(Fail::Bad) => Ok(None),
        Err(Fail::Stop(e)) => Err(e),
    }
}

fn parse_list(s: &[u8], range: Span, ns: &[NsDecl], t: &mut Tables<'_>, budget: &Budget<'_>) -> R<u32> {
    let mut lex = Lexer::over(s, range, budget_ref(budget));
    let mut cur = Cursor { lex: &mut lex, peeked: None };
    let mut frames: MVec<'_, Frame> = MVec::new(budget_ref(budget));
    frames.push(Frame::new(S_NEVER, true, false, false))?;
    cur.skip_ws()?;
    loop {
        budget.spend(cost::OP_STEP)?;
        let tok = cur.peek()?;
        let depth = frames.len();
        let Some(mut f) = frames.last().copied() else { return Err(invalid()) };
        match tok.kind {
            Kind::Eof | Kind::RParen | Kind::Comma => {
                if tok.kind != Kind::Eof {
                    cur.next()?;
                }
                // 選択子を終える
                if tok.kind == Kind::RParen && f.top {
                    f.broken = true;
                }
                end_selector(&mut f, t)?;
                if let Some(x) = frames.last_mut() {
                    *x = f;
                }
                if f.broken && !f.forgiving {
                    // 壊れた並び：外へ上げる
                    if f.top {
                        return Err(invalid());
                    }
                    if tok.kind == Kind::Comma {
                        // 並びの残りを飛ばして閉じのかっこまで
                        cur.skip_to_list_end()?;
                        while cur.peek()?.kind == Kind::Comma {
                            cur.next()?;
                            cur.skip_to_list_end()?;
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
                    if tok.kind != Kind::Eof || f.first_sel == NONE {
                        return Err(invalid());
                    }
                    return Ok(f.first_sel);
                }
                frames.pop();
                // 寛容な並びが空でも正しい（何にも当たらない）。外の複合に単純な選択子として足す
                let mut simple = Simple::new(f.kind);
                simple.list = f.first_sel;
                if f.first_sel == NONE {
                    simple.kind = if f.kind == S_NOT { S_IS } else { S_NEVER };
                    if f.kind == S_NOT {
                        // 空の :not() は壊れている
                        if let Some(p) = frames.last_mut() {
                            p.broken = true;
                        }
                        continue;
                    }
                }
                let add = if f.zero { Spec::default() } else { f.max_spec };
                if let Some(p) = frames.last_mut() {
                    add_simple(p, t, simple)?;
                    p.spec = p.spec.add(add);
                }
                continue;
            }
            _ => {}
        }
        if f.broken {
            // 壊れた選択子の残りを飛ばす（寛容な並びなら次の `,` まで）
            cur.skip_to_list_end()?;
            continue;
        }
        let r = step(&mut cur, s, &mut f, t, ns, &mut frames);
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

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 読んでいる選択子を終え、並びにつなぐ（壊れていれば捨てる）。
fn end_selector(f: &mut Frame, t: &mut Tables<'_>) -> R<()> {
    if f.broken {
        return Ok(());
    }
    if f.compound_empty {
        // 空の選択子・結合子で終わる選択子は壊れている
        f.broken = true;
        return Ok(());
    }
    let c = push_compound(f, t)?;
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
        f.first_simple = NONE;
        f.last_simple = NONE;
        f.compound_empty = true;
        f.after_pseudo_element = false;
        f.broken = false;
    }
}

/// 読んでいる複合を表に足す。
fn push_compound(f: &mut Frame, t: &mut Tables<'_>) -> R<u32> {
    let id = t.compounds.len() as u32;
    t.compounds.push(Compound { first: f.first_simple, prev: f.prev_compound, comb: f.pending_comb, pad: [0; 3] })?;
    f.prev_compound = id;
    f.first_simple = NONE;
    f.last_simple = NONE;
    f.compound_empty = true;
    f.after_pseudo_element = false;
    Ok(id)
}

fn add_simple(f: &mut Frame, t: &mut Tables<'_>, s: Simple) -> R<()> {
    if f.after_pseudo_element && s.kind != S_NEVER {
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
    ns: &[NsDecl],
    frames: &mut MVec<'_, Frame>,
) -> R<()> {
    let default_ns = default_ns(ns);
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
        if f.compound_empty {
            return Err(invalid());
        }
        push_compound(f, t)?;
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
                let c = if tok.is_delim(b'*') {
                    NS_ANY
                } else {
                    ns_lookup(ns, t.names.as_slice(), s, &tok).ok_or_else(invalid)?
                };
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
            add_simple(f, t, simple)
        }
        Kind::Hash if tok.flag => {
            let mut x = Simple::new(S_ID);
            x.a = t.push_name(decode(s, tok.inner), false)?;
            f.spec.a = f.spec.a.saturating_add(1);
            add_simple(f, t, x)
        }
        Kind::Delim if tok.is_delim(b'.') => {
            let n = cur.next()?;
            if n.kind != Kind::Ident {
                return Err(invalid());
            }
            let mut x = Simple::new(S_CLASS);
            x.a = t.push_name(decode(s, n.inner), false)?;
            f.spec.b = f.spec.b.saturating_add(1);
            add_simple(f, t, x)
        }
        Kind::LBracket => {
            let x = attribute(cur, s, t, ns)?;
            f.spec.b = f.spec.b.saturating_add(1);
            add_simple(f, t, x)
        }
        Kind::Colon => pseudo(cur, s, f, t, frames),
        _ => Err(invalid()),
    }
}

/// 属性のセレクタ（`[` を読んだあと）。
fn attribute(cur: &mut Cursor<'_, '_, '_>, s: &[u8], t: &mut Tables<'_>, ns: &[NsDecl]) -> R<Simple> {
    cur.skip_ws()?;
    let first = cur.next()?;
    let (ns_c, name_tok) = if first.is_delim(b'|') {
        (NS_NONE, cur.next()?)
    } else if cur.peek()?.is_delim(b'|') && !peek2_is_eq(cur)? {
        cur.next()?;
        let c = if first.is_delim(b'*') {
            NS_ANY
        } else if first.kind == Kind::Ident {
            ns_lookup(ns, t.names.as_slice(), s, &first).ok_or_else(invalid)?
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
        Kind::Str => t.push_name(decode_string(s, v.inner), false)?,
        _ => return Err(invalid()),
    };
    cur.skip_ws()?;
    let mut end = cur.next()?;
    if end.kind == Kind::Ident {
        if ident_is(s, end.inner, end.escaped, b"i") {
            x.flags = FLAG_CASE_I;
        } else if !ident_is(s, end.inner, end.escaped, b"s") {
            return Err(invalid());
        }
        cur.skip_ws()?;
        end = cur.next()?;
    }
    if end.kind != Kind::RBracket {
        return Err(invalid());
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
    let mut n = cur.next()?;
    let element = n.kind == Kind::Colon;
    if element {
        n = cur.next()?;
    }
    let is = |w: &[u8]| ident_is(s, n.inner, n.escaped, w);
    match n.kind {
        Kind::Ident => {
            // 擬似要素（`::x` と、古い 1 つの `:` の 4 つ）
            if element || is(b"before") || is(b"after") || is(b"first-line") || is(b"first-letter") {
                f.spec.c = f.spec.c.saturating_add(1);
                add_simple(f, t, Simple::new(S_NEVER))?;
                f.after_pseudo_element = true;
                return Ok(());
            }
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
            } else if is(b"link") || is(b"any-link") {
                S_LINK
            } else if is(b"defined") {
                S_UNIVERSAL
            } else if DYNAMIC.iter().any(|w| is(w)) {
                S_NEVER
            } else {
                return Err(invalid());
            };
            f.spec.b = f.spec.b.saturating_add(1);
            let mut x = Simple::new(kind);
            x.ns = NS_ANY;
            add_simple(f, t, x)
        }
        Kind::Function if !element => {
            if is(b"not") || is(b"is") || is(b"where") || is(b"matches") || is(b"-webkit-any") {
                let (kind, forgiving, zero) = if is(b"not") {
                    (S_NOT, false, false)
                } else if is(b"where") {
                    (S_IS, true, true)
                } else {
                    (S_IS, true, false)
                };
                // 入れ子の並びを始める（外の複合の状態 `f` は、呼び手が外の段に書き戻す）
                frames.push(Frame::new(kind, false, forgiving, zero))?;
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
                let arg = cur.skip_function()?;
                let (a, b, of) = parse_an_b(s, arg).ok_or_else(invalid)?;
                if of && !matches!(kind, S_NTH_CHILD | S_NTH_LAST_CHILD) {
                    return Err(invalid());
                }
                let mut x = Simple::new(if of { S_NEVER } else { kind });
                x.n_a = a;
                x.n_b = b;
                f.spec.b = f.spec.b.saturating_add(1);
                return add_simple(f, t, x);
            }
            if is(b"lang") {
                let arg = cur.skip_function()?;
                let ranges = lang_ranges(s, arg, t)?;
                let mut x = Simple::new(S_LANG);
                x.a = ranges;
                f.spec.b = f.spec.b.saturating_add(1);
                return add_simple(f, t, x);
            }
            if is(b"has") || is(b"host") || is(b"host-context") || is(b"state") || is(b"dir") {
                // 正しいが当てない（:has は後ろを見る）
                cur.skip_function()?;
                f.spec.b = f.spec.b.saturating_add(1);
                return add_simple(f, t, Simple::new(S_NEVER));
            }
            Err(invalid())
        }
        Kind::Function if element => {
            // `::part()`・`::slotted()` など
            cur.skip_function()?;
            f.spec.c = f.spec.c.saturating_add(1);
            add_simple(f, t, Simple::new(S_NEVER))?;
            f.after_pseudo_element = true;
            Ok(())
        }
        _ => Err(invalid()),
    }
}

/// 静止した絵では当たらない擬似クラス（正しい名前として受ける）。
const DYNAMIC: &[&[u8]] = &[
    b"hover",
    b"active",
    b"focus",
    b"focus-within",
    b"focus-visible",
    b"visited",
    b"target",
    b"target-within",
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
    b"playing",
    b"paused",
    b"current",
    b"past",
    b"future",
    b"fullscreen",
    b"modal",
    b"picture-in-picture",
    b"user-invalid",
    b"user-valid",
    b"popover-open",
    b"host",
];

/// An+B（CSS Syntax §6）と、`of S` が続くか。`arg` は関数の中身。
fn parse_an_b(s: &[u8], arg: Span) -> Option<(i32, i32, bool)> {
    let raw = arg.of(s);
    // `of` の前まで（`of` は空白で区切られた ident）
    let mut body = raw;
    let mut of = false;
    let mut k = 0usize;
    while k.saturating_add(2) <= raw.len() {
        let before_ws = k == 0 || raw.get(k.saturating_sub(1)).is_some_and(|b| is_ws(*b));
        let w = raw.get(k..k.saturating_add(2)).unwrap_or(&[]);
        let after = raw.get(k.saturating_add(2)).copied();
        if before_ws && w.eq_ignore_ascii_case(b"of") && after.is_none_or(is_ws) && k > 0 {
            body = raw.get(..k).unwrap_or(&[]);
            of = true;
            break;
        }
        k = k.saturating_add(1);
    }
    let body = trim_ws(body);
    if body.eq_ignore_ascii_case(b"odd") {
        return Some((2, 1, of));
    }
    if body.eq_ignore_ascii_case(b"even") {
        return Some((2, 0, of));
    }
    let n_at = body.iter().position(|&b| b == b'n' || b == b'N');
    let Some(n_at) = n_at else {
        return parse_int(body, true).map(|b| (0, b, of));
    };
    let pre = body.get(..n_at).unwrap_or(&[]);
    let a = match pre {
        b"" | b"+" => 1,
        b"-" => -1,
        _ => parse_int(pre, true)?,
    };
    let rest = trim_ws(body.get(n_at.saturating_add(1)..).unwrap_or(&[]));
    if rest.is_empty() {
        return Some((a, 0, of));
    }
    let (sign, digits) = match rest.first() {
        Some(b'+') => (1i64, trim_ws(rest.get(1..).unwrap_or(&[]))),
        Some(b'-') => (-1i64, trim_ws(rest.get(1..).unwrap_or(&[]))),
        _ => return None,
    };
    let v = parse_int(digits, false)?;
    let b = clamp_i32(sign.saturating_mul(i64::from(v)));
    Some((a, b, of))
}

const fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
}

fn trim_ws(mut s: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = s
        && is_ws(*first)
    {
        s = rest;
    }
    while let [rest @ .., last] = s
        && is_ws(*last)
    {
        s = rest;
    }
    s
}

/// 10 進の整数（`signed` なら頭の `+`・`-` を許す）。i32 で止める。
fn parse_int(s: &[u8], signed: bool) -> Option<i32> {
    let (neg, digits) = match s.first() {
        Some(b'+') if signed => (false, s.get(1..)?),
        Some(b'-') if signed => (true, s.get(1..)?),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut v: i64 = 0;
    for &d in digits {
        v = v.saturating_mul(10).saturating_add(i64::from(d.saturating_sub(b'0')));
    }
    Some(clamp_i32(if neg { v.saturating_neg() } else { v }))
}

fn clamp_i32(v: i64) -> i32 {
    i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX })
}

/// `:lang()` の言語の範囲（ident か文字列を `,` で区切ったもの）を、小文字にして `,` でつないで名前の置き場に足す。
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
                    t.names.push(b',')?;
                }
                if tok.kind == Kind::Ident {
                    t.push_name(decode(s, tok.inner), true)?;
                } else {
                    t.push_name(decode_string(s, tok.inner), true)?;
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
