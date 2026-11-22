//! 段 1：XML（architecture.md §4.1・requirements.md FR-IN）。
//!
//! 入力のバイト列を、行きがけの順の節の配列（[`XmlDoc`]）にする。2 つの段で読む：
//!
//! 1. 数える段（[`count`]）：整形式・名前空間・属性の重なり・実体を確かめ、節・属性・文字の置き場のバイト数と、作る段の
//!    手間を数える。`estimate_parse` はこの段だけを走らせる
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

use crate::budget::{Budget, chunk_len};
use crate::cost;
use crate::error::{Error, Feature, Invalid, InvalidReason, Resource, Stage, Unsupported};
use crate::limits::Limits;
use crate::mem::{Frozen, MVec, Plain, stable_sort};
use crate::xml::dtd::Entity;
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

    /// 名前空間 `ns` と局所名 `name` の属性の値。
    pub(crate) fn attr<'a>(&'a self, input: &'a [u8], n: &Node, ns: u8, name: &[u8]) -> Option<&'a [u8]> {
        self.attrs_of(n).iter().find(|a| a.ns == ns && a.local.of(input) == name).map(|a| self.value(input, a))
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
    /// 要素の数・文字の節の数（後ろの段の見積もり）
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
    let head = b.get(..b.len().min(200)).unwrap_or(&[]);
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
    /// 解いた名前空間と局所名
    ns: u8,
    pad: [u8; 3],
    local: Span,
}

impl Plain for RawAttr {}

/// 歩きの事象を受ける側（数える段と作る段）。
trait Sink {
    fn element(&mut self, ns: u8, local: Span, pos: u32, preserve: bool, parent: u32) -> Result<u32, Error>;
    fn attr(&mut self, w: &Walk<'_, '_>, ns: u8, local: Span, value: Span, pos: u32) -> Result<(), Error>;
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
}

/// 文字と値を展開して書き出す先（数えるか、置き場に書く）。
trait Out {
    fn write(&mut self, s: &[u8]) -> Result<(), Error>;
}

/// 数える側：バイト数と、作る段で置き場に書く手間（`cost::append` の伸ばさない形）。
struct CountOut {
    bytes: u64,
    work: u64,
}

impl Out for CountOut {
    fn write(&mut self, s: &[u8]) -> Result<(), Error> {
        self.bytes = self.bytes.saturating_add(s.len() as u64);
        self.work = self.work.saturating_add(cost::append(0, u64::MAX, s.len() as u64, 1).0);
        Ok(())
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
}

/// 展開の仕方。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Attr,
    Text,
    CData,
}

impl Walk<'_, '_> {
    /// `raw` を置き場に書く要るか（参照・改行の揃え・属性の空白の揃えがあるか）。
    fn needs_store(&self, raw: Span, mode: Mode) -> bool {
        raw.of(self.b).iter().any(|&c| match mode {
            Mode::Attr => c == b'&' || c == b'\t' || c == b'\n' || c == b'\r',
            Mode::Text => c == b'&' || c == b'\r',
            Mode::CData => c == b'\r',
        })
    }

    /// `raw` を展開して `out` に書く（参照・改行・属性の空白）。生のバイトは読む前に塊ずつ払う。
    fn expand(&self, raw: Span, mode: Mode, out: &mut dyn Out) -> Result<(), Error> {
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
            for &d in digits {
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
        // 実体の値：改行を揃え、属性の中なら空白も揃える（値は参照を含まない。DTD で確かめた）
        self.normalize(e.value, mode, out)
    }

    /// 参照を含まない `raw` を、改行（と属性なら空白）を揃えて書く（実体の値）。文字の中では、値の文字が文書の文字か
    /// を確かめる（属性の値の中では確かめない。roxmltree と同じ。DTD の中の文字は字句で確かめていない）。
    fn normalize(&self, raw: Span, mode: Mode, out: &mut dyn Out) -> Result<(), Error> {
        let s = raw.of(self.b);
        let mut meter = ScanMeter::new(0, s.len());
        let mut run = 0usize;
        let mut i = 0usize;
        while let Some(&c) = s.get(i) {
            meter.pay(self.budget, i.saturating_add(3))?;
            // 多バイトの文字は頭のバイトで確かめる（続きのバイトは飛ばす）
            if mode == Mode::Text
                && ((c < 0x20 && !chars::is_space(c))
                    || (c >= 0xC0 && !chars::decode(s, i).is_some_and(|(u, _)| chars::is_char(u))))
            {
                return Err(xml_err((raw.start as usize).saturating_add(i)));
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
        Ok(())
    }

    /// 書き出す量（数える）。
    fn expanded_len(&self, raw: Span, mode: Mode) -> Result<CountOut, Error> {
        let mut c = CountOut { bytes: 0, work: 0 };
        self.expand(raw, mode, &mut c)?;
        Ok(c)
    }
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
/// 名前 `a` として受ける（roxmltree と同じ）。名前の手間は字句で払ってある。
fn split_qname(b: &[u8], q: Span) -> Result<(Option<Span>, Span), Error> {
    let s = q.of(b);
    let Some(k) = s.iter().position(|&c| c == b':') else { return Ok((None, q)) };
    let rest = s.get(k.saturating_add(1)..).unwrap_or(&[]);
    if rest.is_empty() || rest.contains(&b':') {
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
) -> Result<(), Error> {
    let mut lex = Lexer::new(b, start, budget);
    let mut w = Walk { b, budget, entities: MVec::new(budget) };
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
                    w.entities = dtd::entities(b, s, budget)?;
                }
            }
            Event::Start { pos, name } => {
                raw.clear();
                pending = Some((name, pos));
            }
            Event::Attr { pos, name, value } => {
                raw.push(RawAttr { name, value, pos, ns: NS_NONE, pad: [0; 3], local: name })?;
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
                let mark = bindings.len() as u32;
                let mut default_bound = false;
                // 名前空間の宣言を束ねる
                for k in 0..raw.len() {
                    let Some(&a) = raw.get(k) else { continue };
                    let (prefix, local) = split_qname(b, a.name)?;
                    // `xmlns`（頭の `:` だけの `:xmlns` も。roxmltree と同じ）は既定の名前空間の宣言
                    let decl_prefix = if prefix.is_none() && local.of(b) == b"xmlns" {
                        Some(Span::new(a.name.end as usize, a.name.end as usize))
                    } else if prefix.is_some_and(|p| p.of(b) == b"xmlns") {
                        Some(local)
                    } else {
                        None
                    };
                    let Some(p) = decl_prefix else { continue };
                    uri.clear();
                    w.expand(a.value, Mode::Attr, &mut StoreOut { store: &mut uri })?;
                    let u = uri.as_slice();
                    let pb = p.of(b);
                    // xmlns は束ねられない。xml は XML の名前空間にだけ、XML の名前空間は xml にだけ束ねる
                    if pb == b"xmlns" || u == URI_XMLNS || ((pb == b"xml") != (u == URI_XML)) {
                        return Err(xml_err(a.pos as usize));
                    }
                    // 接頭辞つきの宣言を空にするのは名前空間 1.0 では誤りだが、roxmltree は空の URI の名前空間として受ける
                    // （SVG でも なし でもない「そのほか」）。既定の名前空間の宣言が重なれば最初のものを使う（roxmltree と同じ。
                    // 重なりの確かめでも誤りにしない）
                    let ns = if !pb.is_empty() && u.is_empty() { NS_OTHER } else { ns_of_uri(u) };
                    if !(pb.is_empty() && default_bound) {
                        bindings.push(Binding { prefix: p, ns, pad: [0; 3] })?;
                    }
                    default_bound |= pb.is_empty();
                    // 宣言の局所名は接頭辞（既定の名前空間なら空）
                    if let Some(r) = raw.get_mut(k) {
                        r.ns = NS_XMLNS;
                        r.local = p;
                    }
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
                        budget.spend(cost::copy(pb.len() as u64).saturating_add(1))?;
                        if let Some(bd) = bindings.get(k)
                            && bd.prefix.of(b) == pb
                        {
                            return Ok(Some(bd.ns));
                        }
                    }
                    Ok(if prefix.is_none() { Some(NS_NONE) } else { None })
                };
                // 要素の名前空間
                let (eprefix, elocal) = split_qname(b, qname)?;
                let Some(ens) = resolve(eprefix, true, &bindings)? else { return Err(xml_err(pos as usize)) };
                // 属性の名前空間
                for k in 0..raw.len() {
                    let Some(&a) = raw.get(k) else { continue };
                    if a.ns == NS_XMLNS {
                        continue;
                    }
                    let (prefix, local) = split_qname(b, a.name)?;
                    let Some(ns) = resolve(prefix, false, &bindings)? else { return Err(xml_err(a.pos as usize)) };
                    if let Some(r) = raw.get_mut(k) {
                        r.ns = ns;
                        r.local = local;
                    }
                }
                // 属性の重なり（名前空間と局所名で並べて隣を比べる。名前を比べる分を前払いする）
                check_duplicates(b, &mut raw, budget)?;
                // xml:space
                let parent = open.last().copied();
                let mut preserve = parent.is_some_and(|p| p.preserve);
                for a in raw.as_slice() {
                    if a.ns == NS_XML && a.local.of(b) == b"space" {
                        match a.value.of(b) {
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
                    let Some(&a) = raw.get(k) else { continue };
                    if a.ns == NS_XMLNS {
                        continue;
                    }
                    sink.attr(&w, a.ns, a.local, a.value, a.pos)?;
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
                budget.spend(cost::copy(name.len() as u64).saturating_add(1))?;
                // 接頭辞と局所名で比べる（`</:svg>` は `<svg>` を閉じる。roxmltree と同じ）
                let (op, ol) = split_qname(b, o.name)?;
                let (ep, el) = split_qname(b, name)?;
                let pre = |p: Option<Span>| p.map_or(&[][..], |p| p.of(b));
                if pre(op) != pre(ep) || ol.of(b) != el.of(b) {
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
            Event::Eof => return Ok(()),
        }
    }
}

/// 開きのタグの属性の重なりを確かめる（名前空間と局所名）。`raw` の並びは変える。
fn check_duplicates(b: &[u8], raw: &mut MVec<'_, RawAttr>, budget: &Budget<'_>) -> Result<(), Error> {
    let n = raw.len() as u64;
    if n < 2 {
        return Ok(());
    }
    // そのほかの名前空間は URI を持たないので、修飾つきの名前で比べる（同じ接頭辞の重なりを拾う）
    let key = |a: &RawAttr| -> (u8, &[u8]) { (a.ns, if a.ns == NS_OTHER { a.name.of(b) } else { a.local.of(b) }) };
    // 比べる名前のバイトの分（要素ごとに鍵の長さ × 段の数）を前払いする
    let names: u64 =
        raw.as_slice().iter().fold(0u64, |s, a| s.saturating_add((key(a).1.len() as u64).saturating_add(1)));
    budget.spend_large(cost::scan(names).saturating_mul(cost::ceil_log2(n).saturating_add(1)))?;
    stable_sort(budget, raw.as_mut_slice(), |x, y| key(x) < key(y))?;
    let s = raw.as_slice();
    for k in 1..s.len() {
        // 既定の名前空間の宣言（`xmlns`）の重なりは受ける（最初のものを使う）
        if let (Some(x), Some(y)) = (s.get(k.saturating_sub(1)), s.get(k))
            && key(x) == key(y)
            && !(x.ns == NS_XMLNS && x.local.len() == 0)
        {
            return Err(xml_err(y.pos as usize));
        }
    }
    // 元の順に戻す（作る段は属性を書いた順に持つ）
    stable_sort(budget, raw.as_mut_slice(), |x, y| x.pos < y.pos)?;
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
    /// 展開で増えた分（入力 ＋ これが展開したあとのバイト数）
    grown: u64,
}

impl CountSink {
    fn store_value(&mut self, w: &Walk<'_, '_>, raw: Span, mode: Mode) -> Result<(), Error> {
        if w.needs_store(raw, mode) {
            let c = w.expanded_len(raw, mode)?;
            self.store = self.store.saturating_add(c.bytes);
            self.write_work = self.write_work.saturating_add(c.work);
            self.grown = self.grown.saturating_add(c.bytes.saturating_sub(raw.len() as u64));
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

    fn attr(&mut self, w: &Walk<'_, '_>, _ns: u8, _local: Span, value: Span, _pos: u32) -> Result<(), Error> {
        self.attrs = self.attrs.saturating_add(1);
        self.write_work = self.write_work.saturating_add(cost::append(0, u64::MAX, 1, size_of::<Attr>() as u64).0);
        self.store_value(w, value, Mode::Attr)
    }

    fn element_done(&mut self, _node: u32, _attrs_start: u64) -> Result<(), Error> {
        Ok(())
    }

    fn text(&mut self, w: &Walk<'_, '_>, raw: Span, cdata: bool, _parent: u32, _preserve: bool) -> Result<u32, Error> {
        let id = self.nodes;
        self.nodes = self.nodes.saturating_add(1);
        self.write_work = self.write_work.saturating_add(cost::append(0, u64::MAX, 1, size_of::<Node>() as u64).0);
        self.store_value(w, raw, if cdata { Mode::CData } else { Mode::Text })?;
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
    /// 値を置き場に書くか借りるか。範囲と印。
    fn value(&mut self, w: &Walk<'_, '_>, raw: Span, mode: Mode) -> Result<(Span, u8), Error> {
        if !w.needs_store(raw, mode) {
            return Ok((raw, 0));
        }
        let start = self.store.len();
        w.expand(raw, mode, &mut StoreOut { store: &mut self.store })?;
        Ok((Span::new(start, self.store.len()), FLAG_IN_STORE))
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

    fn attr(&mut self, w: &Walk<'_, '_>, ns: u8, local: Span, value: Span, pos: u32) -> Result<(), Error> {
        let (v, flags) = self.value(w, value, Mode::Attr)?;
        self.attrs.push(Attr { ns, flags, pad: [0; 2], local, value: v, pos })
    }

    fn element_done(&mut self, node: u32, attrs_start: u64) -> Result<(), Error> {
        let end = self.attrs.len();
        if let Some(n) = self.nodes.get_mut(node as usize) {
            n.attrs = Span::new(attrs_start as usize, end);
        }
        Ok(())
    }

    fn text(&mut self, w: &Walk<'_, '_>, raw: Span, cdata: bool, parent: u32, preserve: bool) -> Result<u32, Error> {
        let (t, mut flags) = self.value(w, raw, if cdata { Mode::CData } else { Mode::Text })?;
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

/// 数える段（`estimate_parse` と `parse` の前半）。前置きから歩きの終わりまでを `budget` で払い、作る段の容量と手間の
/// 上限を返す。実体を展開したあとのバイト数が上限を超えれば `Limit`。
pub(crate) fn count(b: &[u8], limits: &Limits, budget: &Budget<'_>, stage: Stage) -> Result<XmlCounts, Error> {
    let start = prelude(b, limits, budget, stage)?;
    let mut sink = CountSink::default();
    walk(b, start, limits, budget_ref(budget), &mut sink, stage)?;
    let expanded = (b.len() as u64).saturating_add(sink.grown);
    if expanded > limits.expanded_bytes {
        return Err(Error::limit(Resource::Expanded, expanded, limits.expanded_bytes, stage));
    }
    Ok(XmlCounts {
        nodes: sink.nodes,
        attrs: sink.attrs,
        store: sink.store,
        expanded,
        walk_work: budget.work(),
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
