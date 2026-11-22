//! 段 1：XML（architecture.md §4.1・requirements.md FR-IN）。
//!
//! 入力のバイト列を、行きがけの順の節の配列（[`XmlDoc`]）にする。2 つの段で読む：
//!
//! 1. 数える段（[`count`]）：整形式・名前空間・属性の重なり・実体を確かめ、節・属性・文字の置き場のバイト数と、作る段の
//!    手間を数える
//! 2. 作る段（[`build`]）：数えた量の容量を最初に取り（伸ばさない）、同じ歩き（[`walk`]）で節を作る
//!
//! 歩きの手間と確保は 2 つの段でぴったり同じ（同じ字句・同じ確かめ・同じ一時の入れ物）。作る段の手間は「数える段で
//! 測った歩きの手間 ＋ 数えた書き込みの手間」で、見積もりはそれを上から押さえる（見積もり ≥ 実際）。
//!
//! - 入力は UTF-8 として正しいものだけを読む（先頭の BOM は飛ばす。FR-IN-01・02）。svgz は `Unsupported`（FR-IN-03）
//! - 属性の値は XML 1.0 §3.3.3 のとおり空白を揃え（改行と字送りを空白に）、文字は改行を LF に揃える（§2.11）。参照を
//!   展開したものは文字の置き場に詰め、展開しない値は入力を借りる
//! - 名前は名前空間（SVG・XLink・XML・そのほか・なし）と局所名（入力の範囲）。`xmlns` の宣言は属性に入れない

pub(crate) mod chars;
mod dtd;
pub(crate) mod lex;

use std::cell::Cell;

use crate::budget::{Budget, chunk_len};
use crate::cost;
use crate::error::{Error, Feature, Invalid, InvalidReason, Resource, Stage, Unsupported};
use crate::limits::Limits;
use crate::mem::{Frozen, MVec, Plain, bytes_cmp, bytes_eq, stable_sort, stable_sort_try};
use crate::xml::dtd::{AttDecl, Entity};
use crate::xml::lex::{Event, Lexer, ScanMeter, Span};

/// 節がないことの印。
pub(crate) const NONE: u32 = u32::MAX;

/// 名前空間。
pub(crate) const NS_NONE: u8 = 0;
pub(crate) const NS_SVG: u8 = 1;
pub(crate) const NS_XLINK: u8 = 2;
pub(crate) const NS_XML: u8 = 3;
pub(crate) const NS_XMLNS: u8 = 4;
pub(crate) const NS_OTHER: u8 = 5;

const URI_SVG: &[u8] = b"http://www.w3.org/2000/svg";
const URI_XLINK: &[u8] = b"http://www.w3.org/1999/xlink";
const URI_XML: &[u8] = b"http://www.w3.org/XML/1998/namespace";
const URI_XMLNS: &[u8] = b"http://www.w3.org/2000/xmlns/";

/// 節の種類。
pub(crate) const KIND_ELEMENT: u8 = 0;
pub(crate) const KIND_TEXT: u8 = 1;

/// 節の印：文字・値が文字の置き場にある（なければ入力を借りる）。
pub(crate) const FLAG_IN_STORE: u8 = 1;
/// 節の印：`xml:space="preserve"`（受け継いだものを含む）。
pub(crate) const FLAG_PRESERVE: u8 = 2;
/// 節の印：CDATA の文字。
pub(crate) const FLAG_CDATA: u8 = 4;

/// 節（要素か文字）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) kind: u8,
    pub(crate) ns: u8,
    pub(crate) flags: u8,
    pub(crate) pad: u8,
    pub(crate) parent: u32,
    pub(crate) first_child: u32,
    pub(crate) next_sibling: u32,
    /// 要素の局所名（入力の範囲）
    pub(crate) local: Span,
    /// 属性の番号の範囲（`XmlDoc::attrs` の中）
    pub(crate) attrs: Span,
    /// 文字の範囲（`FLAG_IN_STORE` なら置き場の中、なければ入力の中）
    pub(crate) text: Span,
    /// 入力の中の位置（要素は `<`、文字は始まり）
    pub(crate) pos: u32,
}

impl Plain for Node {}

/// 属性。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Attr {
    pub(crate) ns: u8,
    pub(crate) flags: u8,
    pub(crate) pad: [u8; 2],
    pub(crate) local: Span,
    /// 値（`FLAG_IN_STORE` なら置き場の中、なければ入力の中）
    pub(crate) value: Span,
    pub(crate) pos: u32,
}

impl Plain for Attr {}

/// 読んだ XML。名前と借りた値は入力を指す（入力は `Document` が借りて持つ）。
#[derive(Debug)]
pub(crate) struct XmlDoc {
    pub(crate) nodes: Frozen<Node>,
    pub(crate) attrs: Frozen<Attr>,
    pub(crate) store: Frozen<u8>,
}

#[allow(dead_code, reason = "M1 の style・doc から使う（今は試験とファズだけ）")]
impl XmlDoc {
    /// 持ち続ける量。
    pub(crate) fn bytes(&self) -> u64 {
        self.nodes.bytes().saturating_add(self.attrs.bytes()).saturating_add(self.store.bytes())
    }

    pub(crate) fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.as_slice().get(id as usize)
    }

    /// 局所名。
    pub(crate) fn local<'a>(&self, input: &'a [u8], n: &Node) -> &'a [u8] {
        n.local.of(input)
    }

    /// 節の属性。
    pub(crate) fn attrs_of(&self, n: &Node) -> &[Attr] {
        self.attrs.as_slice().get(n.attrs.start as usize..n.attrs.end as usize).unwrap_or(&[])
    }

    /// 属性の値。
    pub(crate) fn value<'a>(&'a self, input: &'a [u8], a: &Attr) -> &'a [u8] {
        if a.flags & FLAG_IN_STORE != 0 { a.value.of(self.store.as_slice()) } else { a.value.of(input) }
    }

    /// 文字の節の中身。
    pub(crate) fn text<'a>(&'a self, input: &'a [u8], n: &Node) -> &'a [u8] {
        if n.flags & FLAG_IN_STORE != 0 { n.text.of(self.store.as_slice()) } else { n.text.of(input) }
    }

    /// 名前空間 `ns` と局所名 `name` の属性（見た属性ごとに払う）。
    pub(crate) fn find_attr<'a>(
        &'a self,
        input: &[u8],
        n: &Node,
        ns: u8,
        name: &[u8],
        budget: &Budget<'_>,
    ) -> Result<Option<&'a Attr>, Error> {
        for a in self.attrs_of(n) {
            budget.spend(cost::OP_STEP)?;
            // 名前は短い決まった名前なので、長さの違う名前はすぐに違うとわかる
            if a.ns == ns && a.local.of(input) == name {
                return Ok(Some(a));
            }
        }
        Ok(None)
    }

    /// 名前空間 `ns` と局所名 `name` の属性の値（見た属性ごとに払う）。
    pub(crate) fn attr<'a>(
        &'a self,
        input: &'a [u8],
        n: &Node,
        ns: u8,
        name: &[u8],
        budget: &Budget<'_>,
    ) -> Result<Option<&'a [u8]>, Error> {
        Ok(self.find_attr(input, n, ns, name, budget)?.map(|a| self.value(input, a)))
    }
}

/// 数える段の結果（作る段の容量と、作る段の手間の上限）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct XmlCounts {
    pub(crate) nodes: u64,
    pub(crate) attrs: u64,
    /// 文字の置き場のバイト数
    pub(crate) store: u64,
    /// 実体を展開したあとのバイト数（入力 ＋ 展開で増えた分）
    pub(crate) expanded: u64,
    /// 歩きの手間と確保の山（作る段も同じ）
    pub(crate) walk_work: u64,
    pub(crate) walk_peak: u64,
    /// 作る段が書き込む手間（節・属性・置き場）
    pub(crate) write_work: u64,
    /// 要素の数（後ろの段の見積もり）
    pub(crate) elements: u64,
}

impl XmlCounts {
    /// 作る段の手間の上限。
    pub(crate) fn build_work(&self) -> u64 {
        self.walk_work.saturating_add(self.write_work).saturating_add(cost::ALLOC_STEP.saturating_mul(3))
    }

    /// 作る段が持ち続ける量（節・属性・置き場）。
    pub(crate) fn result_bytes(&self) -> u64 {
        self.nodes
            .saturating_mul(size_of::<Node>() as u64)
            .saturating_add(self.attrs.saturating_mul(size_of::<Attr>() as u64))
            .saturating_add(self.store)
    }

    /// 作る段の確保の山の上限（歩きの一時の入れ物 ＋ 結果）。
    pub(crate) fn build_peak(&self) -> u64 {
        self.walk_peak.saturating_add(self.result_bytes())
    }
}

fn invalid(reason: InvalidReason, pos: usize) -> Error {
    Error::Invalid(Invalid { reason, position: Some(pos as u64) })
}

fn xml_err(pos: usize) -> Error {
    invalid(InvalidReason::Xml, pos)
}

/// 入力の前置き：大きさ・svgz・BOM・UTF-8。本文の始まり（BOM のあと）を返す。
pub(crate) fn prelude(b: &[u8], limits: &Limits, budget: &Budget<'_>, stage: Stage) -> Result<usize, Error> {
    let len = b.len() as u64;
    // 位置は u32 で持つ
    let most = limits.input_bytes.min(u64::from(u32::MAX).saturating_sub(1));
    if len > most {
        return Err(Error::limit(Resource::Input, len, most, stage));
    }
    // 展開したあとの量は入力の長さから数えるので、上限が入力より小さければ値の有無によらず断る
    if len > limits.expanded_bytes {
        return Err(Error::limit(Resource::Expanded, len, limits.expanded_bytes, stage));
    }
    if b.starts_with(&[0x1f, 0x8b]) {
        return Err(Error::Unsupported(Unsupported { feature: Feature::Svgz, position: Some(0) }));
    }
    if b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) {
        return Err(Error::Unsupported(Unsupported { feature: Feature::Encoding, position: Some(0) }));
    }
    let start = if b.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    // UTF-8 を塊ごとに確かめる（塊の終わりで切れた文字は次の塊で読む）
    let chunk = chunk_len(1);
    let mut i = start;
    while i < b.len() {
        let end = i.saturating_add(chunk).min(b.len());
        budget.spend(cost::scan(end.saturating_sub(i) as u64))?;
        let part = b.get(i..end).unwrap_or(&[]);
        match core::str::from_utf8(part) {
            Ok(_) => i = end,
            Err(e) => {
                let bad = i.saturating_add(e.valid_up_to());
                if e.error_len().is_none() && end < b.len() && bad > i {
                    // 塊の終わりで切れた文字：そこから読み直す
                    i = bad;
                } else {
                    return Err(not_utf8(b, bad));
                }
            }
        }
    }
    Ok(start)
}

/// UTF-8 でないときの誤り：宣言が UTF-8 以外を名乗れば `Unsupported`、ほかは `Invalid`（FR-IN-02）。
fn not_utf8(b: &[u8], bad: usize) -> Error {
    let body = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    let head = body.get(..body.len().min(200)).unwrap_or(&[]);
    if head.starts_with(b"<?xml") {
        let end = (0..head.len()).find(|&k| head.get(k..).is_some_and(|r| r.starts_with(b"?>"))).unwrap_or(head.len());
        let decl = head.get(..end).unwrap_or(&[]);
        if let Some(enc) = declared_encoding(decl)
            && !enc.eq_ignore_ascii_case(b"utf-8")
            && !enc.eq_ignore_ascii_case(b"utf8")
        {
            return Error::Unsupported(Unsupported { feature: Feature::Encoding, position: Some(0) });
        }
    }
    invalid(InvalidReason::Utf8, bad)
}

/// XML の宣言の `encoding` の値。
fn declared_encoding(decl: &[u8]) -> Option<&[u8]> {
    let k = (0..decl.len()).find(|&k| decl.get(k..).is_some_and(|r| r.starts_with(b"encoding")))?;
    let mut j = k.saturating_add(8);
    while decl.get(j).is_some_and(|&c| chars::is_space(c) || c == b'=') {
        j = j.saturating_add(1);
    }
    let q = *decl.get(j)?;
    if q != b'"' && q != b'\'' {
        return None;
    }
    let vs = j.saturating_add(1);
    let ve = (vs..decl.len()).find(|&k| decl.get(k) == Some(&q))?;
    decl.get(vs..ve)
}

// ---- 歩き ----

/// 開いている要素。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Open {
    name: Span,
    /// この要素のあとに戻す名前空間の束ねの数
    bindings: u32,
    /// 作る段の節の番号と、最後の子
    node: u32,
    last_child: u32,
    preserve: bool,
    pad: [u8; 3],
}

impl Plain for Open {}

/// 名前空間の束ね（接頭辞と名前空間。接頭辞が空なら既定の名前空間）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Binding {
    prefix: Span,
    ns: u8,
    pad: [u8; 3],
}

impl Plain for Binding {}

/// 開きのタグの中の属性（集めてから名前空間を解く）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct RawAttr {
    name: Span,
    value: Span,
    pos: u32,
    /// 書いた順（ATTLIST の既定の値で足したものは後ろ）
    order: u32,
    /// 解いた名前空間と局所名
    local: Span,
    ns: u8,
    /// ATTLIST で CDATA でない型と宣言した属性（値の空白を 1 つに揃える）
    tokens: u8,
    /// ATTLIST の既定の値で足した属性（値は DTD の中。展開の量は値の全部を数える）
    from_decl: u8,
    /// 既定の値で足した属性が名前空間の確かめで通らず、捨てた（roxmltree は ATTLIST を読まないので、断らない。NFR-10）
    dropped: u8,
}

impl Plain for RawAttr {}

impl RawAttr {
    /// 値の揃え方。
    fn mode(&self) -> Mode {
        if self.tokens != 0 { Mode::Tokens } else { Mode::Attr }
    }

    /// 値のうち入力の量に入っているバイト数（既定の値は要素に書かれていないので 0）。
    fn in_input(&self) -> u64 {
        if self.from_decl != 0 { 0 } else { self.value.len() as u64 }
    }
}

/// 歩きの事象を受ける側（数える段と作る段）。
trait Sink {
    fn element(&mut self, ns: u8, local: Span, pos: u32, preserve: bool, parent: u32) -> Result<u32, Error>;
    fn attr(&mut self, w: &Walk<'_, '_>, a: &RawAttr) -> Result<(), Error>;
    fn element_done(&mut self, node: u32, attrs_start: u64) -> Result<(), Error>;
    fn text(&mut self, w: &Walk<'_, '_>, raw: Span, cdata: bool, parent: u32, preserve: bool) -> Result<u32, Error>;
    fn link(&mut self, parent: u32, prev: u32, child: u32);
    fn attrs_len(&self) -> u64;
}

/// 歩きの共有の状態（字句のほか）。
struct Walk<'a, 'b> {
    b: &'a [u8],
    budget: &'b Budget<'b>,
    entities: MVec<'b, Entity>,
    /// ATTLIST の属性の宣言（要素の名前・属性の名前で並べたもの）
    attlists: MVec<'b, AttDecl>,
    /// 展開で増えた分（入力 ＋ これが展開したあとのバイト数）と、その上限（`Limits::expanded_bytes`。置き場の位置は
    /// u32 なので u32 の内に抑える）
    grown: Cell<u64>,
    expanded_limit: u64,
    stage: Stage,
}

/// 文字と値を展開して書き出す先（数えるか、置き場に書く）。
trait Out {
    fn write(&mut self, s: &[u8]) -> Result<(), Error>;
    /// 実体の値を書く（数える側は前に求めた量を足すだけで、値を走査しない）。
    fn entity(&mut self, w: &Walk<'_, '_>, e: &Entity, mode: Mode) -> Result<(), Error>;
    /// 数える側なら、書いたものとして量を足す（真を返す）。書く側は何もしない（偽）。
    fn add(&mut self, _bytes: u64, _work: u64) -> Result<bool, Error> {
        Ok(false)
    }
}

/// 数える側：バイト数と、作る段で置き場に書く手間（`cost::append` の伸ばさない形）。`limit` があれば、書く量が展開の
/// 上限を超えた所で `Limit(Expanded)` を返す（展開したあとのバイト数は「`base` ＋ 書いた量」。全部を数える前に止まる）。
struct CountOut {
    bytes: u64,
    work: u64,
    limit: Option<(u64, u64, Stage)>,
}

impl CountOut {
    const fn new() -> CountOut {
        CountOut { bytes: 0, work: 0, limit: None }
    }

    fn check(&self) -> Result<(), Error> {
        if let Some((base, allowed, stage)) = self.limit {
            let needed = base.saturating_add(self.bytes);
            if needed > allowed {
                return Err(Error::limit(Resource::Expanded, needed, allowed, stage));
            }
        }
        Ok(())
    }
}

impl Out for CountOut {
    fn write(&mut self, s: &[u8]) -> Result<(), Error> {
        self.bytes = self.bytes.saturating_add(s.len() as u64);
        self.work = self.work.saturating_add(cost::append(0, u64::MAX, s.len() as u64, 1).0);
        self.check()
    }

    fn entity(&mut self, _w: &Walk<'_, '_>, e: &Entity, mode: Mode) -> Result<(), Error> {
        let (bytes, work) = if mode == Mode::Attr { (e.attr_bytes, e.attr_work) } else { (e.text_bytes, e.text_work) };
        self.bytes = self.bytes.saturating_add(bytes);
        // 作る段は値を読みながら書く（読む手間も作る段の手間に入れる）
        self.work = self.work.saturating_add(work).saturating_add(cost::scan(e.value.len() as u64));
        self.check()
    }

    fn add(&mut self, bytes: u64, work: u64) -> Result<bool, Error> {
        self.bytes = self.bytes.saturating_add(bytes);
        self.work = self.work.saturating_add(work);
        self.check()?;
        Ok(true)
    }
}

/// CDATA でない型の属性の値：揃えた値の頭と終わりの空白を捨て、続く空白を 1 つにして `inner` に書く（XML 1.0 §3.3.3）。
/// 実体の値もこれを通す（前に求めた長さは使えないので走査する）。通すバイトは塊ずつ払う。
struct Collapse<'o, 'c> {
    inner: &'o mut dyn Out,
    budget: &'c Budget<'c>,
    /// 前に空白があり、まだ書いていない
    pending: bool,
    /// 空白でないものを 1 つでも書いた
    started: bool,
}

impl Collapse<'_, '_> {
    fn emit(&mut self, part: &[u8]) -> Result<(), Error> {
        if part.is_empty() {
            return Ok(());
        }
        if self.pending {
            self.inner.write(b" ")?;
            self.pending = false;
        }
        self.started = true;
        self.inner.write(part)
    }
}

impl Out for Collapse<'_, '_> {
    fn write(&mut self, s: &[u8]) -> Result<(), Error> {
        let mut meter = ScanMeter::new(0, s.len());
        let mut run = 0usize;
        let mut i = 0usize;
        while let Some(&c) = s.get(i) {
            meter.pay(self.budget, i)?;
            if c == b' ' {
                self.emit(s.get(run..i).unwrap_or(&[]))?;
                self.pending = self.started;
                run = i.saturating_add(1);
            }
            i = i.saturating_add(1);
        }
        self.emit(s.get(run..).unwrap_or(&[]))
    }

    fn entity(&mut self, w: &Walk<'_, '_>, e: &Entity, mode: Mode) -> Result<(), Error> {
        // 数える側は、DOCTYPE で求めた形から量を足す（作る段が値を読んで揃える手間も入れる）。書く側は値を読んで揃える
        if e.tok_flags & 8 == 0 {
            return Ok(());
        }
        if e.tok_flags & 1 == 0 {
            // 空白だけ：空白を読んだのと同じ
            if self.inner.add(0, cost::scan(e.value.len() as u64).saturating_add(cost::scan(e.attr_bytes)))? {
                self.pending = self.started;
                return Ok(());
            }
            return normalize(w.b, w.budget, e.value, mode, self).map(|_| ());
        }
        let pend = if e.tok_flags & 2 != 0 { self.started } else { self.pending };
        let lead_work = if pend { cost::append(0, u64::MAX, 1, 1).0 } else { 0 };
        let scan_work = cost::scan(e.value.len() as u64).saturating_add(cost::scan(e.attr_bytes));
        let bytes = e.tok_bytes.saturating_add(u64::from(pend));
        if self.inner.add(bytes, e.tok_work.saturating_add(lead_work).saturating_add(scan_work))? {
            self.started = true;
            self.pending = e.tok_flags & 4 != 0;
            return Ok(());
        }
        normalize(w.b, w.budget, e.value, mode, self).map(|_| ())
    }
}

/// 作る側：置き場に書く。
struct StoreOut<'s, 'b> {
    store: &'s mut MVec<'b, u8>,
}

impl Out for StoreOut<'_, '_> {
    fn write(&mut self, s: &[u8]) -> Result<(), Error> {
        self.store.append_slice(s)
    }

    fn entity(&mut self, w: &Walk<'_, '_>, e: &Entity, mode: Mode) -> Result<(), Error> {
        normalize(w.b, w.budget, e.value, mode, self).map(|_| ())
    }
}

/// 展開の仕方。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Attr,
    /// CDATA でない型の属性（`Attr` に加えて空白を 1 つに揃える）
    Tokens,
    Text,
    CData,
}

impl<'a> Walk<'a, '_> {
    /// `raw` を置き場に書く要るか（参照・改行の揃え・属性の空白の揃えがあるか）。読むバイトは塊ずつ払う。
    fn needs_store(&self, raw: Span, mode: Mode) -> Result<bool, Error> {
        let s = raw.of(self.b);
        let mut meter = ScanMeter::new(0, s.len());
        if mode == Mode::Tokens && (s.first() == Some(&b' ') || s.last() == Some(&b' ')) {
            return Ok(true);
        }
        let mut prev = 0u8;
        for (i, &c) in s.iter().enumerate() {
            meter.pay(self.budget, i)?;
            let hit = match mode {
                Mode::Attr => c == b'&' || c == b'\t' || c == b'\n' || c == b'\r',
                Mode::Tokens => c == b'&' || c == b'\t' || c == b'\n' || c == b'\r' || (c == b' ' && prev == b' '),
                Mode::Text => c == b'&' || c == b'\r',
                Mode::CData => c == b'\r',
            };
            if hit {
                return Ok(true);
            }
            prev = c;
        }
        Ok(false)
    }

    /// `raw` を展開して `out` に書く（参照・改行・属性の空白。`Tokens` は空白も揃える）。
    fn expand(&self, raw: Span, mode: Mode, out: &mut dyn Out) -> Result<(), Error> {
        if mode == Mode::Tokens {
            let mut c = Collapse { inner: out, budget: self.budget, pending: false, started: false };
            return self.expand_raw(raw, Mode::Attr, &mut c);
        }
        self.expand_raw(raw, mode, out)
    }

    /// `raw` を展開して `out` に書く（参照・改行・属性の空白）。生のバイトは読む前に塊ずつ払う。
    fn expand_raw(&self, raw: Span, mode: Mode, out: &mut dyn Out) -> Result<(), Error> {
        let s = raw.of(self.b);
        let mut meter = ScanMeter::new(0, s.len());
        let base = raw.start as usize;
        let mut run = 0usize;
        let mut i = 0usize;
        let flush = |out: &mut dyn Out, from: usize, to: usize| -> Result<(), Error> {
            if to > from { out.write(s.get(from..to).unwrap_or(&[])) } else { Ok(()) }
        };
        while let Some(&c) = s.get(i) {
            meter.pay(self.budget, i.saturating_add(1))?;
            match c {
                b'\r' => {
                    flush(out, run, i)?;
                    out.write(if mode == Mode::Attr { b" " } else { b"\n" })?;
                    i = i.saturating_add(if s.get(i.saturating_add(1)) == Some(&b'\n') { 2 } else { 1 });
                    run = i;
                }
                b'\n' | b'\t' if mode == Mode::Attr => {
                    flush(out, run, i)?;
                    out.write(b" ")?;
                    i = i.saturating_add(1);
                    run = i;
                }
                b'&' if mode != Mode::CData => {
                    flush(out, run, i)?;
                    let mut end = i;
                    loop {
                        meter.pay(self.budget, end)?;
                        match s.get(end) {
                            None => return Err(xml_err(base.saturating_add(i))),
                            Some(b';') => break,
                            Some(_) => end = end.saturating_add(1),
                        }
                    }
                    let name = s.get(i.saturating_add(1)..end).unwrap_or(&[]);
                    self.reference(name, base.saturating_add(i), mode, out)?;
                    i = end.saturating_add(1);
                    run = i;
                }
                _ => i = i.saturating_add(1),
            }
        }
        flush(out, run, s.len())
    }

    /// 参照 `&name;` を書く。
    fn reference(&self, name: &[u8], pos: usize, mode: Mode, out: &mut dyn Out) -> Result<(), Error> {
        if let Some(num) = name.strip_prefix(b"#") {
            let (digits, radix) = match num.strip_prefix(b"x") {
                Some(h) => (h, 16u32),
                None => (num, 10u32),
            };
            if digits.is_empty() {
                return Err(xml_err(pos));
            }
            // 値は u32 に入らなければ誤り。サロゲートと U+10FFFF を超える値は U+FFFD、ほかの文書の文字でない値は誤り
            // （roxmltree と同じ）
            let mut v: u32 = 0;
            let mut meter = ScanMeter::new(0, digits.len());
            for (i, &d) in digits.iter().enumerate() {
                meter.pay(self.budget, i)?;
                let x = match d {
                    b'0'..=b'9' => u32::from(d.saturating_sub(b'0')),
                    b'a'..=b'f' if radix == 16 => u32::from(d.saturating_sub(b'a')).saturating_add(10),
                    b'A'..=b'F' if radix == 16 => u32::from(d.saturating_sub(b'A')).saturating_add(10),
                    _ => return Err(xml_err(pos)),
                };
                v = v.checked_mul(radix).and_then(|v| v.checked_add(x)).ok_or_else(|| xml_err(pos))?;
            }
            if (0xD800..=0xDFFF).contains(&v) || v > 0x10_FFFF {
                v = 0xFFFD;
            }
            if !chars::is_char(v) {
                return Err(xml_err(pos));
            }
            let (buf, n) = utf8(v);
            return out.write(buf.get(..n).unwrap_or(&[]));
        }
        let pre: &[u8] = match name {
            b"lt" => b"<",
            b"gt" => b">",
            b"amp" => b"&",
            b"apos" => b"'",
            b"quot" => b"\"",
            _ => b"",
        };
        if !pre.is_empty() {
            return out.write(pre);
        }
        let Some(e) = dtd::lookup(self.b, self.entities.as_slice(), name, self.budget)? else {
            return Err(xml_err(pos));
        };
        // 実体の値：改行を揃え、属性の中なら空白も揃える（値は参照を含まない。DTD で確かめた）。文字の中では、値が
        // 文書の文字だけで `]]>` を含まないこと（DOCTYPE を読んだときに 1 度だけ求めた）
        if mode == Mode::Text && e.text_ok == 0 {
            return Err(xml_err(pos));
        }
        out.entity(self, &e, mode)
    }

    /// 展開で増えた分を足し、上限と比べる（`out` は展開したあとのバイト数、`raw` は元のバイト数）。2 つの段で同じに呼ぶ。
    fn grow(&self, out: u64, raw: u64) -> Result<(), Error> {
        let g = self.grown.get().saturating_add(out.saturating_sub(raw));
        self.grown.set(g);
        let needed = (self.b.len() as u64).saturating_add(g);
        if needed > self.expanded_limit {
            return Err(Error::limit(Resource::Expanded, needed, self.expanded_limit, self.stage));
        }
        Ok(())
    }

    /// 属性の値を揃えた形で見る。揃える要がなければ入力のまま、要れば `scratch` に書く。`count` なら展開で増えた分を
    /// 数えて上限と比べる（属性に入れない xmlns の宣言。属性に入れるものは置き場に書くときに数える）。
    fn attr_value<'x>(&self, a: &RawAttr, scratch: &'x mut MVec<'_, u8>, count: bool) -> Result<&'x [u8], Error>
    where
        'a: 'x,
    {
        let mode = a.mode();
        let in_input = a.in_input();
        if !self.needs_store(a.value, mode)? {
            if count && in_input == 0 {
                self.grow(a.value.len() as u64, 0)?;
            }
            return Ok(a.value.of(self.b));
        }
        let c = self.expanded_len(a.value, mode, in_input)?;
        if count {
            self.grow(c.bytes, in_input)?;
        }
        scratch.clear();
        self.expand(a.value, mode, &mut StoreOut { store: scratch })?;
        Ok(scratch.as_slice())
    }

    /// 書き出す量（数える）。`in_input` は、展開する前の値のうち入力の量に入っているバイト数（要素に書いた値は値の
    /// 長さ、ATTLIST の既定の値は 0）。
    fn expanded_len(&self, raw: Span, mode: Mode, in_input: u64) -> Result<CountOut, Error> {
        let base = (self.b.len() as u64).saturating_add(self.grown.get()).saturating_sub(in_input);
        let mut c = CountOut { limit: Some((base, self.expanded_limit, self.stage)), ..CountOut::new() };
        self.expand(raw, mode, &mut c)?;
        c.limit = None;
        Ok(c)
    }
}

/// 参照を含まない実体の値 `raw` を、改行（と属性なら空白）を揃えて `out` に書く。文字の中（`Mode::Text`）では、値が
/// 文書の文字だけで `]]>` を含まないかも確かめ、外れたら書くのをやめて偽を返す（属性の値の中では確かめない。roxmltree と
/// 同じ。DTD の中の文字は字句で確かめていない）。読むバイトは塊ずつ払う。
fn normalize(b: &[u8], budget: &Budget<'_>, raw: Span, mode: Mode, out: &mut dyn Out) -> Result<bool, Error> {
    let s = raw.of(b);
    let mut meter = ScanMeter::new(0, s.len());
    let mut run = 0usize;
    let mut i = 0usize;
    while let Some(&c) = s.get(i) {
        meter.pay(budget, i.saturating_add(3))?;
        // 多バイトの文字は頭のバイトで確かめる（続きのバイトは飛ばす）
        if mode == Mode::Text
            && ((c < 0x20 && !chars::is_space(c))
                || (c >= 0xC0 && !chars::decode(s, i).is_some_and(|(u, _)| chars::is_char(u)))
                || (c == b']' && s.get(i.saturating_add(1)..).is_some_and(|r| r.starts_with(b"]>"))))
        {
            return Ok(false);
        }
        let attr_space = mode == Mode::Attr && (c == b'\n' || c == b'\t');
        if c == b'\r' || attr_space {
            if i > run {
                out.write(s.get(run..i).unwrap_or(&[]))?;
            }
            out.write(if mode == Mode::Attr { b" " } else { b"\n" })?;
            let crlf = c == b'\r' && s.get(i.saturating_add(1)) == Some(&b'\n');
            i = i.saturating_add(if crlf { 2 } else { 1 });
            run = i;
        } else {
            i = i.saturating_add(1);
        }
    }
    if s.len() > run {
        out.write(s.get(run..).unwrap_or(&[]))?;
    }
    Ok(true)
}

/// 実体の表に、揃えたあとの量と文字の中で使えるかを 1 度だけ求めて書く（参照ごとに値を走査しないため）。
fn shape_entities(b: &[u8], budget: &Budget<'_>, table: &mut MVec<'_, Entity>) -> Result<(), Error> {
    for k in 0..table.len() {
        budget.spend(cost::OP_STEP)?;
        let Some(&e) = table.get(k) else { continue };
        let mut attr = CountOut::new();
        normalize(b, budget, e.value, Mode::Attr, &mut attr)?;
        let mut text = CountOut::new();
        let ok = normalize(b, budget, e.value, Mode::Text, &mut text)?;
        // CDATA でない型の属性の中の形：頭の空白を無視する状態（始まっていない）で揃えた中身
        let mut core = CountOut::new();
        let trail = {
            let mut c = Collapse { inner: &mut core, budget, pending: false, started: false };
            normalize(b, budget, e.value, Mode::Attr, &mut c)?;
            c.pending
        };
        let lead = e.value.of(b).first().is_some_and(|f| matches!(f, b' ' | b'\t' | b'\n' | b'\r'));
        let has = core.bytes > 0;
        // 終わりの空白：中身のあとに空白があるか（始まっていない状態では、中身がなければ空白は残らない）
        let trail = trail || (has && e.value.of(b).last().is_some_and(|f| matches!(f, b' ' | b'\t' | b'\n' | b'\r')));
        let flags = u32::from(has)
            | (u32::from(lead && has) << 1)
            | (u32::from(trail) << 2)
            | (u32::from(!e.value.of(b).is_empty()) << 3);
        if let Some(x) = table.get_mut(k) {
            x.attr_bytes = attr.bytes;
            x.attr_work = attr.work;
            x.text_bytes = text.bytes;
            x.text_work = text.work;
            x.text_ok = u32::from(ok);
            x.tok_bytes = core.bytes;
            x.tok_work = core.work;
            x.tok_flags = flags;
        }
    }
    Ok(())
}

/// 符号位置を UTF-8 にする（`is_char` で確かめた値。std の `encode_utf8` は短い領域で panic するので自前で）。
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

/// 名前 `q` を接頭辞と局所名に分ける。`:` が 2 つ以上・空の局所名は誤り。頭の `:` だけのもの（`:a`）は接頭辞のない
/// 名前 `a` として受ける（roxmltree と同じ）。読むバイトは塊ずつ払う。
fn split_qname(b: &[u8], q: Span, budget: &Budget<'_>) -> Result<(Option<Span>, Span), Error> {
    let s = q.of(b);
    let mut meter = ScanMeter::new(0, s.len());
    let mut colon: Option<usize> = None;
    for (i, &c) in s.iter().enumerate() {
        meter.pay(budget, i)?;
        if c == b':' {
            if colon.is_some() {
                return Err(xml_err(q.start as usize));
            }
            colon = Some(i);
        }
    }
    let Some(k) = colon else { return Ok((None, q)) };
    let rest = s.get(k.saturating_add(1)..).unwrap_or(&[]);
    if rest.is_empty() {
        return Err(xml_err(q.start as usize));
    }
    let start = q.start as usize;
    let mid = start.saturating_add(k);
    let local = Span::new(mid.saturating_add(1), q.end as usize);
    Ok((if k == 0 { None } else { Some(Span::new(start, mid)) }, local))
}

/// 名前空間の URI から種類を決める（XML の名前空間を別の接頭辞に束ねるのは誤り。呼び手が見る）。
fn ns_of_uri(uri: &[u8]) -> u8 {
    if uri == URI_SVG {
        NS_SVG
    } else if uri == URI_XLINK {
        NS_XLINK
    } else if uri == URI_XML {
        NS_XML
    } else if uri.is_empty() {
        NS_NONE
    } else {
        NS_OTHER
    }
}

/// 歩く（両方の段で同じ）。`sink` が数えるか作る。
fn walk<'b>(
    b: &[u8],
    start: usize,
    limits: &Limits,
    budget: &'b Budget<'b>,
    sink: &mut dyn Sink,
    stage: Stage,
) -> Result<u64, Error> {
    let mut lex = Lexer::new(b, start, budget);
    let expanded_limit = limits.expanded_bytes.min(u64::from(u32::MAX).saturating_sub(1));
    let mut w = Walk {
        b,
        budget,
        entities: MVec::new(budget),
        attlists: MVec::new(budget),
        grown: Cell::new(0),
        expanded_limit,
        stage,
    };
    let mut open: MVec<'b, Open> = MVec::new(budget);
    let mut bindings: MVec<'b, Binding> = MVec::new(budget);
    let mut raw: MVec<'b, RawAttr> = MVec::new(budget);
    let mut uri: MVec<'b, u8> = MVec::new(budget);
    let mut nodes: u64 = 0;
    // 開きのタグを読んでいる間の要素（名前・位置）
    let mut pending: Option<(Span, u32)> = None;
    loop {
        match lex.next()? {
            Event::Doctype { subset } => {
                if let Some(s) = subset {
                    let sub = dtd::parse_subset(b, s, budget)?;
                    let mut table = sub.entities;
                    shape_entities(b, budget, &mut table)?;
                    w.entities = table;
                    w.attlists = sub.attlists;
                    // 既定の値の参照と名前を確かめ、壊れた宣言（知らない実体・文書の文字でない文字の参照・名前の形）は、
                    // 宣言がないものとして当てない（同じ属性のあとの宣言が効く。roxmltree は ATTLIST を読まずに受ける。
                    // NFR-10・ADR-66）。ここでは展開の上限で比べない（量は当てるときに数える）
                    for k in 0..w.attlists.len() {
                        budget.spend(cost::OP_STEP)?;
                        let Some(&d) = w.attlists.get(k) else { continue };
                        let name_ok = match split_qname(b, d.name, budget) {
                            Ok(_) => true,
                            Err(Error::Invalid(_)) => false,
                            Err(e) => return Err(e),
                        };
                        let value_ok = d.has_default == 0
                            || (chars_ok(b, d.value, budget)?
                                && match w.expand(d.value, Mode::Attr, &mut CountOut::new()) {
                                    Ok(()) => true,
                                    Err(Error::Invalid(_)) => false,
                                    Err(e) => return Err(e),
                                });
                        if !(name_ok && value_ok)
                            && let Some(x) = w.attlists.get_mut(k)
                        {
                            x.broken = 1;
                        }
                    }
                    // 壊れた宣言を表から除く（並びは保つ。要素ごとに当てるときに回らない）
                    let mut keep = 0usize;
                    for k in 0..w.attlists.len() {
                        budget.spend(cost::OP_STEP)?;
                        let Some(&d) = w.attlists.get(k) else { continue };
                        if d.broken != 0 {
                            continue;
                        }
                        if let Some(x) = w.attlists.get_mut(keep) {
                            *x = d;
                        }
                        keep = keep.saturating_add(1);
                    }
                    w.attlists.truncate(keep);
                }
            }
            Event::Start { pos, name } => {
                raw.clear();
                pending = Some((name, pos));
            }
            Event::Attr { pos, name, value } => {
                let order = raw.len() as u32;
                raw.push(RawAttr {
                    name,
                    value,
                    pos,
                    order,
                    local: name,
                    ns: NS_NONE,
                    tokens: 0,
                    from_decl: 0,
                    dropped: 0,
                })?;
            }
            Event::StartEnd { empty } => {
                let Some((qname, pos)) = pending.take() else { return Err(xml_err(lex.pos())) };
                let depth = open.len() as u64;
                if depth.saturating_add(1) > u64::from(limits.depth) {
                    return Err(Error::limit(Resource::Depth, depth.saturating_add(1), u64::from(limits.depth), stage));
                }
                nodes = nodes.saturating_add(1);
                if nodes > limits.nodes {
                    return Err(Error::limit(Resource::Nodes, nodes, limits.nodes, stage));
                }
                // ATTLIST：書いた属性の型を知り、書いていない属性に既定の値を足す（名前空間を解く前。xmlns の既定も効く）
                apply_attlists(&w, qname, &mut raw)?;
                let mark = bindings.len() as u32;
                // 既定の名前空間を束ねる宣言（文書の順に最初のもの。`xmlns` と `b:xmlns` のどちらでも。roxmltree と同じ）
                let mut default_decl: Option<(usize, u8)> = None;
                // 名前空間の宣言を束ねる（既定の値で足した属性の名前は、DOCTYPE で確かめてある）
                for k in 0..raw.len() {
                    budget.spend(cost::OP_STEP)?;
                    let Some(&a) = raw.get(k) else { continue };
                    let (prefix, local) = split_qname(b, a.name, budget)?;
                    // `xmlns`（頭の `:` だけの `:xmlns` も。roxmltree と同じ）は既定の名前空間の宣言
                    let decl_prefix = if prefix.is_none() && local.of(b) == b"xmlns" {
                        Some(Span::new(a.name.end as usize, a.name.end as usize))
                    } else if prefix.is_some_and(|p| p.of(b) == b"xmlns") {
                        Some(local)
                    } else {
                        None
                    };
                    let Some(p) = decl_prefix else { continue };
                    let u = w.attr_value(&a, &mut uri, true)?;
                    let pb = p.of(b);
                    // XMLNS の名前空間は宣言できない。xml は XML の名前空間にだけ、XML の名前空間は xml にだけ束ねる。
                    // 既定の値で足した宣言なら、その宣言だけを捨てる
                    if u == URI_XMLNS || ((pb == b"xml") != (u == URI_XML)) {
                        if a.from_decl == 0 {
                            return Err(xml_err(a.pos as usize));
                        }
                        if let Some(r) = raw.get_mut(k) {
                            r.dropped = 1;
                            r.ns = NS_XMLNS;
                        }
                        continue;
                    }
                    // 宣言の局所名は接頭辞（既定の名前空間なら空）
                    if let Some(r) = raw.get_mut(k) {
                        r.ns = NS_XMLNS;
                        r.local = p;
                    }
                    // `xmlns:xmlns` の宣言は受けるが束ねない（その接頭辞を名前に使えば束ねていない接頭辞の誤り。roxmltree と同じ）
                    if pb == b"xmlns" {
                        continue;
                    }
                    // 接頭辞つきの宣言を空にするのは名前空間 1.0 では誤りだが、roxmltree は空の URI の名前空間として受ける
                    // （SVG でも なし でもない「そのほか」）。既定の名前空間の宣言が重なれば最初のものを使う（roxmltree と同じ。
                    // 重なりの確かめでも誤りにしない）
                    let ns = if !pb.is_empty() && u.is_empty() { NS_OTHER } else { ns_of_uri(u) };
                    if pb.is_empty() {
                        if default_decl.is_none() {
                            default_decl = Some((k, ns));
                        }
                    } else {
                        bindings.push(Binding { prefix: p, ns, pad: [0; 3] })?;
                    }
                }
                // 局所名が xmlns で接頭辞を束ねていない属性（`b:xmlns`）は、roxmltree と同じに既定の名前空間の宣言として
                // 読む（名前空間 1.0 では束ねていない接頭辞の誤りだが、断らない。束ねた接頭辞なら仕様のとおりふつうの属性）。
                // ATTLIST の既定の値で足したものは、束ねていない接頭辞の属性として捨てる（ADR-69 (3)）
                for k in 0..raw.len() {
                    budget.spend(cost::OP_STEP)?;
                    let Some(&a) = raw.get(k) else { continue };
                    if a.ns == NS_XMLNS || a.dropped != 0 || a.from_decl != 0 {
                        continue;
                    }
                    let (prefix, local) = split_qname(b, a.name, budget)?;
                    let Some(p) = prefix else { continue };
                    let pb = p.of(b);
                    if local.of(b) != b"xmlns" || pb == b"xml" || pb == b"xmlns" {
                        continue;
                    }
                    let mut bound = false;
                    for j in (0..bindings.len()).rev() {
                        budget.spend(cost::OP_STEP)?;
                        if let Some(bd) = bindings.get(j)
                            && bytes_eq(budget, bd.prefix.of(b), pb)?
                        {
                            bound = true;
                            break;
                        }
                    }
                    if bound {
                        continue;
                    }
                    let u = w.attr_value(&a, &mut uri, true)?;
                    if u == URI_XMLNS || u == URI_XML {
                        return Err(xml_err(a.pos as usize));
                    }
                    if default_decl.is_none_or(|(at, _)| k < at) {
                        default_decl = Some((k, ns_of_uri(u)));
                    }
                    if let Some(r) = raw.get_mut(k) {
                        r.ns = NS_XMLNS;
                        r.local = Span::new(a.name.end as usize, a.name.end as usize);
                    }
                }
                if let Some((at, ns)) = default_decl {
                    let end = raw.get(at).map_or(0, |a| a.name.end as usize);
                    bindings.push(Binding { prefix: Span::new(end, end), ns, pad: [0; 3] })?;
                }
                let resolve = |prefix: Option<Span>,
                               default_ns: bool,
                               bindings: &MVec<'b, Binding>|
                 -> Result<Option<u8>, Error> {
                    let pb: &[u8] = prefix.map_or(&[], |p| p.of(b));
                    if prefix.is_none() && !default_ns {
                        return Ok(Some(NS_NONE));
                    }
                    if pb == b"xml" {
                        return Ok(Some(NS_XML));
                    }
                    for k in (0..bindings.len()).rev() {
                        budget.spend(cost::OP_STEP)?;
                        if let Some(bd) = bindings.get(k)
                            && bytes_eq(budget, bd.prefix.of(b), pb)?
                        {
                            return Ok(Some(bd.ns));
                        }
                    }
                    Ok(if prefix.is_none() { Some(NS_NONE) } else { None })
                };
                // 要素の名前空間
                let (eprefix, elocal) = split_qname(b, qname, budget)?;
                let Some(ens) = resolve(eprefix, true, &bindings)? else { return Err(xml_err(pos as usize)) };
                // 属性の名前空間（既定の値で足した属性の接頭辞が束ねていなければ、その属性だけを捨てる）
                for k in 0..raw.len() {
                    budget.spend(cost::OP_STEP)?;
                    let Some(&a) = raw.get(k) else { continue };
                    if a.ns == NS_XMLNS {
                        continue;
                    }
                    let (prefix, local) = split_qname(b, a.name, budget)?;
                    let ns = resolve(prefix, false, &bindings)?;
                    if let Some(r) = raw.get_mut(k) {
                        match ns {
                            Some(ns) => {
                                r.ns = ns;
                                r.local = local;
                            }
                            None if a.from_decl != 0 => {
                                r.dropped = 1;
                                r.ns = NS_XMLNS;
                            }
                            None => return Err(xml_err(a.pos as usize)),
                        }
                    }
                }
                // 属性の重なり（名前空間と局所名で並べて隣を比べる。名前のバイトは比べる中で払う。既定の値で足した属性が
                // 重なれば、その属性を捨てる）
                check_duplicates(b, &mut raw, budget)?;
                // xml:space（揃えたあとの値で見る。`preserve`・`default` のほかの値は無いものとして親から受け継ぐ）
                let parent = open.last().copied();
                let mut preserve = parent.is_some_and(|p| p.preserve);
                for k in 0..raw.len() {
                    budget.spend(cost::OP_STEP)?;
                    let Some(&a) = raw.get(k) else { continue };
                    if a.dropped == 0 && a.ns == NS_XML && a.local.of(b) == b"space" {
                        match w.attr_value(&a, &mut uri, false)? {
                            b"preserve" => preserve = true,
                            b"default" => preserve = false,
                            _ => {}
                        }
                    }
                }
                let parent_node = parent.map_or(NONE, |p| p.node);
                let node = sink.element(ens, elocal, pos, preserve, parent_node)?;
                if let Some(p) = open.last_mut() {
                    sink.link(p.node, p.last_child, node);
                    p.last_child = node;
                }
                let attrs_start = sink.attrs_len();
                for k in 0..raw.len() {
                    budget.spend(cost::OP_STEP)?;
                    let Some(&a) = raw.get(k) else { continue };
                    if a.ns == NS_XMLNS || a.dropped != 0 {
                        continue;
                    }
                    sink.attr(&w, &a)?;
                }
                sink.element_done(node, attrs_start)?;
                if empty {
                    bindings.truncate(mark as usize);
                } else {
                    open.push(Open { name: qname, bindings: mark, node, last_child: NONE, preserve, pad: [0; 3] })?;
                }
            }
            Event::End { pos, name } => {
                let Some(o) = open.pop() else { return Err(xml_err(pos as usize)) };
                // 接頭辞と局所名で比べる（`</:svg>` は `<svg>` を閉じる。roxmltree と同じ。比べるバイトは塊ずつ払う）
                let (op, ol) = split_qname(b, o.name, budget)?;
                let (ep, el) = split_qname(b, name, budget)?;
                let pre = |p: Option<Span>| p.map_or(&[][..], |p| p.of(b));
                if !bytes_eq(budget, pre(op), pre(ep))? || !bytes_eq(budget, ol.of(b), el.of(b))? {
                    return Err(xml_err(pos as usize));
                }
                bindings.truncate(o.bindings as usize);
            }
            ev @ (Event::Text { .. } | Event::CData { .. }) => {
                let (t, is_cdata) = match ev {
                    Event::CData { raw } => (raw, true),
                    Event::Text { raw } => (raw, false),
                    _ => continue,
                };
                let Some(p) = open.last().copied() else { return Err(xml_err(t.start as usize)) };
                nodes = nodes.saturating_add(1);
                if nodes > limits.nodes {
                    return Err(Error::limit(Resource::Nodes, nodes, limits.nodes, stage));
                }
                let node = sink.text(&w, t, is_cdata, p.node, p.preserve)?;
                if let Some(p) = open.last_mut() {
                    sink.link(p.node, p.last_child, node);
                    p.last_child = node;
                }
            }
            Event::Eof => return Ok(w.grown.get()),
        }
    }
}

/// 値の生のバイトが文書の文字だけか（ATTLIST の既定の値。字そのものとして書いた文書の文字でない文字の宣言は、文字の参照で
/// 書いたものと同じに壊れた宣言とする）。読むバイトを払う。
fn chars_ok(b: &[u8], raw: Span, budget: &Budget<'_>) -> Result<bool, Error> {
    let mut meter = ScanMeter::new(raw.start as usize, raw.end as usize);
    let mut i = raw.start as usize;
    while i < raw.end as usize {
        meter.pay(budget, i.saturating_add(3))?;
        let Some((c, n)) = chars::decode(b, i) else { return Ok(false) };
        if !chars::is_char(c) {
            return Ok(false);
        }
        i = i.saturating_add(n.max(1));
    }
    Ok(true)
}

/// ATTLIST の宣言を開きのタグに当てる：書いた属性は CDATA でない型なら印を付け、書いていない属性で既定の値のあるものを
/// 後ろに足す（XML 1.0 §3.3・§5.1）。同じ属性の 2 つめからの宣言は飛ばす。比べる名前の分を払う。
fn apply_attlists(w: &Walk<'_, '_>, qname: Span, raw: &mut MVec<'_, RawAttr>) -> Result<(), Error> {
    if w.attlists.is_empty() {
        return Ok(());
    }
    let b = w.b;
    let (lo, hi) = dtd::att_range(b, w.attlists.as_slice(), qname.of(b), w.budget)?;
    let written = raw.len();
    let mut last: Option<Span> = None;
    for k in lo..hi {
        w.budget.spend(cost::OP_STEP)?;
        let Some(&d) = w.attlists.get(k) else { continue };
        if d.broken != 0 {
            continue;
        }
        let name = d.name.of(b);
        if let Some(l) = last
            && bytes_eq(w.budget, l.of(b), name)?
        {
            continue;
        }
        last = Some(d.name);
        let mut found = false;
        for j in 0..written {
            w.budget.spend(cost::OP_STEP)?;
            let same = match raw.get(j) {
                Some(r) => bytes_eq(w.budget, r.name.of(b), name)?,
                None => false,
            };
            if same && let Some(r) = raw.get_mut(j) {
                r.tokens = u8::from(d.tokens != 0);
                found = true;
                break;
            }
        }
        if !found && d.has_default != 0 {
            let order = raw.len() as u32;
            raw.push(RawAttr {
                name: d.name,
                value: d.value,
                pos: d.name.start,
                order,
                local: d.name,
                ns: NS_NONE,
                tokens: u8::from(d.tokens != 0),
                from_decl: 1,
                dropped: 0,
            })?;
        }
    }
    Ok(())
}

/// 開きのタグの属性の重なりを確かめる（名前空間と局所名）。`raw` の並びは変える。
fn check_duplicates(b: &[u8], raw: &mut MVec<'_, RawAttr>, budget: &Budget<'_>) -> Result<(), Error> {
    if raw.len() < 2 {
        return Ok(());
    }
    // そのほかの名前空間は URI を持たないので、修飾つきの名前で比べる（同じ接頭辞の重なりを拾う）
    let key = |a: &RawAttr| -> (u8, &[u8]) { (a.ns, if a.ns == NS_OTHER { a.name.of(b) } else { a.local.of(b) }) };
    // 比べる名前のバイトは比べる中で塊ずつ払う
    let less = |x: &RawAttr, y: &RawAttr| -> Result<bool, Error> {
        let (kx, ky) = (key(x), key(y));
        if kx.0 != ky.0 {
            return Ok(kx.0 < ky.0);
        }
        Ok(bytes_cmp(budget, kx.1, ky.1)? == core::cmp::Ordering::Less)
    };
    stable_sort_try(budget, raw.as_mut_slice(), less)?;
    let same = |x: &RawAttr, y: &RawAttr| -> Result<bool, Error> {
        let (kx, ky) = (key(x), key(y));
        Ok(kx.0 == ky.0 && bytes_eq(budget, kx.1, ky.1)?)
    };
    // 同じ鍵の並びごとに：書いた属性が 2 つあれば誤り。既定の値で足した属性は、書いた属性があれば捨て、なければ最初の
    // ものだけを残す（並べ替えは安定なので、並びの中は書いた順）。既定の名前空間の宣言（`xmlns`）の重なりは受ける
    // （束ねるのは最初のもの）。捨てた属性は比べない
    let len = raw.len();
    let mut i = 0usize;
    while i < len {
        budget.spend(cost::OP_STEP)?;
        let Some(&first) = raw.get(i) else { break };
        let mut j = i.saturating_add(1);
        while j < len {
            budget.spend(cost::OP_STEP)?;
            let Some(y) = raw.get(j) else { break };
            if !same(y, &first)? {
                break;
            }
            j = j.saturating_add(1);
        }
        // 既定の名前空間の宣言（`xmlns`）と `xmlns:xml` の重なりは受ける（どちらも束ねるのは最初のもの。roxmltree と同じ）
        let lenient = first.ns == NS_XMLNS && (first.local.len() == 0 || first.local.of(b) == b"xml");
        if !lenient {
            let mut written = 0u32;
            let mut kept_default = false;
            for k in i..j {
                let Some(&a) = raw.get(k) else { continue };
                if a.dropped != 0 {
                    continue;
                }
                if a.from_decl == 0 {
                    written = written.saturating_add(1);
                    if written > 1 {
                        return Err(xml_err(a.pos as usize));
                    }
                }
            }
            for k in i..j {
                let Some(a) = raw.get_mut(k) else { continue };
                if a.dropped != 0 || a.from_decl == 0 {
                    continue;
                }
                if written > 0 || kept_default {
                    a.dropped = 1;
                } else {
                    kept_default = true;
                }
            }
        }
        i = j;
    }
    // 元の順に戻す（作る段は属性を書いた順に持つ）
    stable_sort(budget, raw.as_mut_slice(), |x, y| x.order < y.order)?;
    Ok(())
}

// ---- 数える段と作る段 ----

/// 数える側。
#[derive(Default)]
struct CountSink {
    nodes: u64,
    attrs: u64,
    elements: u64,
    store: u64,
    write_work: u64,
}

impl CountSink {
    /// 値を数える。`in_input` は値のうち入力の量に入っているバイト数（既定の値は 0 で、借りる値も展開の量に数える）。
    fn store_value(&mut self, w: &Walk<'_, '_>, raw: Span, mode: Mode, in_input: u64) -> Result<(), Error> {
        if w.needs_store(raw, mode)? {
            let c = w.expanded_len(raw, mode, in_input)?;
            w.grow(c.bytes, in_input)?;
            self.store = self.store.saturating_add(c.bytes);
            self.write_work = self.write_work.saturating_add(c.work);
        } else {
            w.grow(raw.len() as u64, in_input)?;
        }
        Ok(())
    }
}

impl Sink for CountSink {
    fn element(&mut self, _ns: u8, _local: Span, _pos: u32, _preserve: bool, _parent: u32) -> Result<u32, Error> {
        let id = self.nodes;
        self.nodes = self.nodes.saturating_add(1);
        self.elements = self.elements.saturating_add(1);
        self.write_work = self.write_work.saturating_add(cost::append(0, u64::MAX, 1, size_of::<Node>() as u64).0);
        Ok(u32::try_from(id).unwrap_or(NONE))
    }

    fn attr(&mut self, w: &Walk<'_, '_>, a: &RawAttr) -> Result<(), Error> {
        self.attrs = self.attrs.saturating_add(1);
        self.write_work = self.write_work.saturating_add(cost::append(0, u64::MAX, 1, size_of::<Attr>() as u64).0);
        self.store_value(w, a.value, a.mode(), a.in_input())
    }

    fn element_done(&mut self, _node: u32, _attrs_start: u64) -> Result<(), Error> {
        Ok(())
    }

    fn text(&mut self, w: &Walk<'_, '_>, raw: Span, cdata: bool, _parent: u32, _preserve: bool) -> Result<u32, Error> {
        let id = self.nodes;
        self.nodes = self.nodes.saturating_add(1);
        self.write_work = self.write_work.saturating_add(cost::append(0, u64::MAX, 1, size_of::<Node>() as u64).0);
        self.store_value(w, raw, if cdata { Mode::CData } else { Mode::Text }, raw.len() as u64)?;
        Ok(u32::try_from(id).unwrap_or(NONE))
    }

    fn link(&mut self, _parent: u32, _prev: u32, _child: u32) {}

    fn attrs_len(&self) -> u64 {
        self.attrs
    }
}

/// 作る側。
struct BuildSink<'b> {
    nodes: MVec<'b, Node>,
    attrs: MVec<'b, Attr>,
    store: MVec<'b, u8>,
}

impl BuildSink<'_> {
    /// 値を置き場に書くか借りるか。範囲と印。増えた分は数える段と同じに足す（数える段で上限の内と確かめてある）。
    fn value(&mut self, w: &Walk<'_, '_>, raw: Span, mode: Mode, in_input: u64) -> Result<(Span, u8), Error> {
        if !w.needs_store(raw, mode)? {
            w.grow(raw.len() as u64, in_input)?;
            return Ok((raw, 0));
        }
        let start = self.store.len();
        w.expand(raw, mode, &mut StoreOut { store: &mut self.store })?;
        let end = self.store.len();
        w.grow(end.saturating_sub(start) as u64, in_input)?;
        Ok((Span::new(start, end), FLAG_IN_STORE))
    }
}

impl Sink for BuildSink<'_> {
    fn element(&mut self, ns: u8, local: Span, pos: u32, preserve: bool, parent: u32) -> Result<u32, Error> {
        let id = u32::try_from(self.nodes.len()).unwrap_or(NONE);
        self.nodes.push(Node {
            kind: KIND_ELEMENT,
            ns,
            flags: if preserve { FLAG_PRESERVE } else { 0 },
            pad: 0,
            parent,
            first_child: NONE,
            next_sibling: NONE,
            local,
            attrs: Span::default(),
            text: Span::default(),
            pos,
        })?;
        Ok(id)
    }

    fn attr(&mut self, w: &Walk<'_, '_>, a: &RawAttr) -> Result<(), Error> {
        let (v, flags) = self.value(w, a.value, a.mode(), a.in_input())?;
        self.attrs.push(Attr { ns: a.ns, flags, pad: [0; 2], local: a.local, value: v, pos: a.pos })
    }

    fn element_done(&mut self, node: u32, attrs_start: u64) -> Result<(), Error> {
        let end = self.attrs.len();
        if let Some(n) = self.nodes.get_mut(node as usize) {
            n.attrs = Span::new(attrs_start as usize, end);
        }
        Ok(())
    }

    fn text(&mut self, w: &Walk<'_, '_>, raw: Span, cdata: bool, parent: u32, preserve: bool) -> Result<u32, Error> {
        let (t, mut flags) = self.value(w, raw, if cdata { Mode::CData } else { Mode::Text }, raw.len() as u64)?;
        if preserve {
            flags |= FLAG_PRESERVE;
        }
        if cdata {
            flags |= FLAG_CDATA;
        }
        let id = u32::try_from(self.nodes.len()).unwrap_or(NONE);
        self.nodes.push(Node {
            kind: KIND_TEXT,
            ns: NS_NONE,
            flags,
            pad: 0,
            parent,
            first_child: NONE,
            next_sibling: NONE,
            local: Span::default(),
            attrs: Span::default(),
            text: t,
            pos: raw.start,
        })?;
        Ok(id)
    }

    fn link(&mut self, parent: u32, prev: u32, child: u32) {
        if prev == NONE {
            if let Some(p) = self.nodes.get_mut(parent as usize) {
                p.first_child = child;
            }
        } else if let Some(p) = self.nodes.get_mut(prev as usize) {
            p.next_sibling = child;
        }
    }

    fn attrs_len(&self) -> u64 {
        self.attrs.len() as u64
    }
}

/// 数える段（読む段の道の頭。ADR-70）。前置きから歩きの終わりまでを `budget` で払い、作る段の容量と手間の
/// 上限を返す。実体を展開したあとのバイト数が上限を超えれば、超えた所で `Limit`（展開する前に、実体ごとに求めた長さで
/// 数える）。歩きの手間は払った差から読む。確保の山は `budget` の通算から読むので、まだ何も確保していない `budget` を
/// 渡す（確保の山は差で読めない）。
pub(crate) fn count(b: &[u8], limits: &Limits, budget: &Budget<'_>, stage: Stage) -> Result<XmlCounts, Error> {
    let work_before = budget.work();
    let start = prelude(b, limits, budget, stage)?;
    let mut sink = CountSink::default();
    let grown = walk(b, start, limits, budget_ref(budget), &mut sink, stage)?;
    let expanded = (b.len() as u64).saturating_add(grown);
    // 節・属性の番号は u32 で持つ（置き場は展開の上限で u32 の内）
    let most = u64::from(u32::MAX).saturating_sub(1);
    if sink.nodes > most {
        return Err(Error::limit(Resource::Nodes, sink.nodes, most, stage));
    }
    if sink.attrs > most {
        // 属性の表のバイト数で表す（上限は番号の上限の分の表）
        let size = size_of::<Attr>() as u64;
        return Err(Error::limit(Resource::Memory, sink.attrs.saturating_mul(size), most.saturating_mul(size), stage));
    }
    Ok(XmlCounts {
        nodes: sink.nodes,
        attrs: sink.attrs,
        store: sink.store,
        expanded,
        walk_work: budget.work().saturating_sub(work_before),
        walk_peak: budget.peak(),
        write_work: sink.write_work,
        elements: sink.elements,
    })
}

/// 作る段。`counts` は同じ入力・同じ上限の数える段の結果（容量を最初に取り、伸ばさない）。
pub(crate) fn build<'b>(
    b: &[u8],
    counts: &XmlCounts,
    limits: &Limits,
    budget: &'b Budget<'b>,
    stage: Stage,
) -> Result<XmlDoc, Error> {
    let start = prelude(b, limits, budget, stage)?;
    let cap = |n: u64| usize::try_from(n).map_err(|_| budget.too_large());
    let mut sink = BuildSink {
        nodes: MVec::with_capacity(budget, cap(counts.nodes)?)?,
        attrs: MVec::with_capacity(budget, cap(counts.attrs)?)?,
        store: MVec::with_capacity(budget, cap(counts.store)?)?,
    };
    walk(b, start, limits, budget, &mut sink, stage)?;
    Ok(XmlDoc { nodes: sink.nodes.freeze(), attrs: sink.attrs.freeze(), store: sink.store.freeze() })
}

/// `&Budget<'_>` の寿命を揃える（数える段は結果を残さないので、借りの寿命を短くしてよい）。
fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
