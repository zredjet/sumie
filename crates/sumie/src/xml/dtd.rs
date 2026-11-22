//! DTD の内部のサブセット（requirements.md FR-IN-07）。一般の実体の宣言と、ATTLIST の属性の宣言を読む。
//!
//! - パラメタ実体（宣言・`%名前;` の参照）・外部の実体（`SYSTEM`・`PUBLIC`）・値に `<`・`&`・`%` を含む実体は
//!   `Unsupported`（今の Kura の前段と同じ。値に参照を含まないので、展開は 1 段で、大きさは「参照の数 × 値の長さ」で決まる）
//! - ATTLIST は XML 1.0 §3.3 のとおりに読む（型・既定の宣言）。既定の値は、要素がその属性を書いていないときに当てる
//!   （§5.1。非検証の処理器にも求められる。ADR-66）。同じ要素の同じ属性は最初の宣言を使う（§3.3）。読めない ATTLIST は
//!   最初の `>` まで飛ばして当てない（roxmltree はどの ATTLIST も最初の `>` まで飛ばして受ける。NFR-10）
//! - ELEMENT・NOTATION の宣言は引用符を見ながら読み飛ばす。宣言の中の文字は確かめない（roxmltree と同じ）
//! - 注釈・処理命令・空白は飛ばす。宣言の外のそのほかの文字と、知らない宣言（`<!FOO`）は誤り（roxmltree と同じ）
//! - `<!ENTITY` のあと・名前のあとには空白が要る（XML 1.0 §4.2）
//! - 同じ名前の実体は最初の宣言を使う（XML 1.0 §4.2）
//!
//! サブセットは字句（[`crate::xml::lex`]）で一度読んだが、ここで読み直すバイトも 1 バイト 1 単位で、読む前に塊ずつ払う
//! （[`ScanMeter`]）。ほかに実体 1 つあたりの手間と、名前で引くための並べ替えの手間を払う。

use crate::budget::Budget;
use crate::cost;
use crate::error::{Error, Feature, Unsupported};
use crate::mem::{MVec, Plain, lower_bound, stable_sort};
use crate::xml::chars::{decode, is_name_char, is_name_start, is_space};
use crate::xml::lex::{ScanMeter, Span};

/// 一般の実体（名前と値の範囲。どちらも入力の中）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Entity {
    pub(crate) name: Span,
    pub(crate) value: Span,
    /// 宣言の順（同じ名前は最初のものを使う）
    pub(crate) order: u32,
    /// 文字の中で展開してよいか（値が文書の文字だけで、`]]>` を含まない。1 なら真）
    pub(crate) text_ok: u32,
    /// 揃えたあとのバイト数と、置き場に書く手間（属性の中・文字の中。XML の段が 1 度だけ求め、参照ごとには走査しない）
    pub(crate) attr_bytes: u64,
    pub(crate) attr_work: u64,
    pub(crate) text_bytes: u64,
    pub(crate) text_work: u64,
}

impl Plain for Entity {}

/// ATTLIST で宣言した属性（要素の名前と属性の名前は修飾つきの名前のまま。XML 1.0 の ATTLIST は名前空間の前の名前で
/// 引く）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AttDecl {
    pub(crate) element: Span,
    pub(crate) name: Span,
    /// 既定の値（引用符の間の生の中身。`has_default` が 1 のとき）
    pub(crate) value: Span,
    /// CDATA でない型なら 1（値の空白を、頭と終わりを捨て、続きを 1 つに揃える。XML 1.0 §3.3.3）
    pub(crate) tokens: u32,
    pub(crate) has_default: u32,
    /// 宣言の順（同じ要素の同じ属性は最初のものを使う）
    pub(crate) order: u32,
    /// 壊れた宣言（属性の名前の形・既定の値の参照。XML の段が DOCTYPE で確かめる）。ないものとして扱う
    pub(crate) broken: u32,
}

impl Plain for AttDecl {}

/// 内部のサブセットを読んだ結果（どちらも名前で引けるように並べてある）。
pub(crate) struct Subset<'b> {
    pub(crate) entities: MVec<'b, Entity>,
    pub(crate) attlists: MVec<'b, AttDecl>,
}

fn unsupported(feature: Feature, pos: usize) -> Error {
    Error::Unsupported(Unsupported { feature, position: Some(pos as u64) })
}

fn invalid(pos: usize) -> Error {
    Error::Invalid(crate::error::Invalid { reason: crate::error::InvalidReason::Xml, position: Some(pos as u64) })
}

/// サブセットを読む口（`end` より前だけを読み、読む前に払う）。
struct Cur<'a, 'b> {
    b: &'a [u8],
    end: usize,
    meter: ScanMeter,
    budget: &'b Budget<'b>,
}

impl Cur<'_, '_> {
    fn at(&mut self, i: usize) -> Result<Option<u8>, Error> {
        if i >= self.end {
            return Ok(None);
        }
        self.meter.pay(self.budget, i)?;
        Ok(self.b.get(i).copied())
    }

    fn starts(&mut self, i: usize, pat: &[u8]) -> Result<bool, Error> {
        for (k, &p) in pat.iter().enumerate() {
            if self.at(i.saturating_add(k))? != Some(p) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn skip_space(&mut self, mut i: usize) -> Result<usize, Error> {
        while self.at(i)?.is_some_and(is_space) {
            i = i.saturating_add(1);
        }
        Ok(i)
    }

    /// `from` から `pat` を探す（見つかった位置）。
    fn find(&mut self, mut i: usize, pat: &[u8]) -> Result<Option<usize>, Error> {
        while self.at(i)?.is_some() {
            if self.starts(i, pat)? {
                return Ok(Some(i));
            }
            i = i.saturating_add(1);
        }
        Ok(None)
    }

    /// `i` の 1 文字（多バイトの文字は最後のバイトまで払う）。
    fn char_at(&mut self, i: usize) -> Result<Option<(u32, usize)>, Error> {
        if self.at(i)?.is_none() {
            return Ok(None);
        }
        self.at(i.saturating_add(3).min(self.end.saturating_sub(1)))?;
        Ok(decode(self.b, i).filter(|&(_, n)| i.saturating_add(n) <= self.end))
    }

    /// 名前の終わり（名前でなければ None）。
    fn name_end(&mut self, i: usize) -> Result<Option<usize>, Error> {
        let Some((c, n)) = self.char_at(i)? else { return Ok(None) };
        if !is_name_start(c) {
            return Ok(None);
        }
        let mut j = i.saturating_add(n);
        while let Some((c, n)) = self.char_at(j)? {
            if !is_name_char(c) {
                break;
            }
            j = j.saturating_add(n);
        }
        Ok(Some(j))
    }
}

/// 内部のサブセット `subset` を読み、名前で並べた実体の表（名前のバイトの順、同じ名前は宣言の順）と、要素の名前と属性の
/// 名前で並べた ATTLIST の表を返す。
pub(crate) fn parse_subset<'b>(b: &[u8], subset: Span, budget: &'b Budget<'b>) -> Result<Subset<'b>, Error> {
    let end = (subset.end as usize).min(b.len());
    let start = subset.start as usize;
    let mut c = Cur { b, end, meter: ScanMeter::new(start, end), budget };
    let mut out: MVec<'b, Entity> = MVec::new(budget);
    let mut atts: MVec<'b, AttDecl> = MVec::new(budget);
    let mut k = start;
    let mut order = 0u32;
    let mut att_order = 0u32;
    while let Some(ch) = c.at(k)? {
        if is_space(ch) {
            k = k.saturating_add(1);
        } else if c.starts(k, b"<!--")? {
            k = c.find(k.saturating_add(4), b"-->")?.ok_or_else(|| invalid(k))?.saturating_add(3);
        } else if c.starts(k, b"<?")? {
            k = c.find(k.saturating_add(2), b"?>")?.ok_or_else(|| invalid(k))?.saturating_add(2);
        } else if c.starts(k, b"<!ENTITY")? {
            let (e, next) = entity(&mut c, k, order)?;
            budget.spend(cost::OP_STEP)?;
            if let Some(e) = e {
                out.push(e)?;
                order = order.saturating_add(1);
            }
            k = next;
        } else if c.starts(k, b"<!ATTLIST")? {
            let mark = atts.len();
            k = match attlist(&mut c, k, &mut att_order, &mut atts) {
                Ok(next) => next,
                // 読めない ATTLIST は、最初の `>` まで飛ばして当てない（roxmltree が受けるものは受ける。NFR-10・ADR-66）
                Err(Error::Invalid(_)) => {
                    atts.truncate(mark);
                    c.find(k, b">")?.ok_or_else(|| invalid(k))?.saturating_add(1)
                }
                Err(e) => return Err(e),
            };
        } else if c.starts(k, b"<!ELEMENT")? || c.starts(k, b"<!NOTATION")? {
            // 引用符を見ながら '>' まで
            let mut e = k.saturating_add(2);
            let mut quote: Option<u8> = None;
            loop {
                let Some(x) = c.at(e)? else { return Err(invalid(k)) };
                match (quote, x) {
                    (Some(q), x) if x == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(x),
                    (None, b'>') => break,
                    _ => {}
                }
                e = e.saturating_add(1);
            }
            k = e.saturating_add(1);
        } else if ch == b'%' {
            return Err(unsupported(Feature::ParameterEntity, k));
        } else {
            return Err(invalid(k));
        }
    }
    // 名前で引けるように並べる（名前のバイトを比べる分は、要素ごとに名前の長さ × 段の数を前払いする）
    let n = out.len() as u64;
    let name_bytes: u64 =
        out.as_slice().iter().fold(0u64, |a, e| a.saturating_add((e.name.len() as u64).saturating_add(1)));
    budget.spend_large(cost::scan(name_bytes).saturating_mul(cost::ceil_log2(n)))?;
    stable_sort(budget, out.as_mut_slice(), |x, y| (x.name.of(b), x.order) < (y.name.of(b), y.order))?;
    let n = atts.len() as u64;
    let att_bytes: u64 = atts.as_slice().iter().fold(0u64, |a, d| {
        a.saturating_add((d.element.len() as u64).saturating_add(d.name.len() as u64).saturating_add(1))
    });
    budget.spend_large(cost::scan(att_bytes).saturating_mul(cost::ceil_log2(n)))?;
    let key = |d: &AttDecl| (d.element.of(b), d.name.of(b), d.order);
    stable_sort(budget, atts.as_mut_slice(), |x, y| key(x) < key(y))?;
    Ok(Subset { entities: out, attlists: atts })
}

/// `<!ATTLIST`（`k` の位置）の宣言を読み、属性の宣言を `out` に足す。宣言の終わりのあとの位置。
fn attlist(c: &mut Cur<'_, '_>, k: usize, order: &mut u32, out: &mut MVec<'_, AttDecl>) -> Result<usize, Error> {
    let mut t = c.skip_space(k.saturating_add(9))?;
    if t == k.saturating_add(9) {
        return Err(invalid(t));
    }
    let ee = c.name_end(t)?.ok_or_else(|| invalid(t))?;
    let element = Span::new(t, ee);
    t = ee;
    loop {
        c.budget.spend(cost::OP_STEP)?;
        let s = c.skip_space(t)?;
        if c.at(s)? == Some(b'>') {
            return Ok(s.saturating_add(1));
        }
        // AttDef ::= S Name S AttType S DefaultDecl
        if s == t {
            return Err(invalid(s));
        }
        let ne = c.name_end(s)?.ok_or_else(|| invalid(s))?;
        let name = Span::new(s, ne);
        let u = c.skip_space(ne)?;
        if u == ne {
            return Err(invalid(u));
        }
        let (tokens, after_type) = att_type(c, u)?;
        let v = c.skip_space(after_type)?;
        if v == after_type {
            return Err(invalid(v));
        }
        let mut decl = AttDecl { element, name, tokens: u32::from(tokens), order: *order, ..AttDecl::default() };
        if c.starts(v, b"#REQUIRED")? {
            t = v.saturating_add(9);
        } else if c.starts(v, b"#IMPLIED")? {
            t = v.saturating_add(8);
        } else {
            let mut w = v;
            if c.starts(v, b"#FIXED")? {
                w = c.skip_space(v.saturating_add(6))?;
                if w == v.saturating_add(6) {
                    return Err(invalid(w));
                }
            }
            // AttValue：引用符の中。`<` は誤り（参照は当てるときに展開する）
            let q = c.at(w)?.ok_or_else(|| invalid(w))?;
            if q != b'"' && q != b'\'' {
                return Err(invalid(w));
            }
            let vs = w.saturating_add(1);
            let mut ve = vs;
            loop {
                match c.at(ve)? {
                    None | Some(b'<') => return Err(invalid(ve)),
                    Some(x) if x == q => break,
                    Some(_) => ve = ve.saturating_add(1),
                }
            }
            decl.value = Span::new(vs, ve);
            decl.has_default = 1;
            t = ve.saturating_add(1);
        }
        out.push(decl)?;
        *order = order.saturating_add(1);
    }
}

/// ATTLIST の型（`i` の位置から）。CDATA でない型か、と型の終わりの位置。
fn att_type(c: &mut Cur<'_, '_>, i: usize) -> Result<(bool, usize), Error> {
    if c.starts(i, b"CDATA")? {
        return Ok((false, i.saturating_add(5)));
    }
    // 長いものから
    for kw in [b"IDREFS".as_slice(), b"IDREF", b"ID", b"ENTITIES", b"ENTITY", b"NMTOKENS", b"NMTOKEN"] {
        if c.starts(i, kw)? {
            return Ok((true, i.saturating_add(kw.len())));
        }
    }
    // NOTATION S ( … ) と列挙 ( … )：閉じのかっこまで（中に `>`・`(` は書けない）
    let open = if c.starts(i, b"NOTATION")? {
        let o = c.skip_space(i.saturating_add(8))?;
        if o == i.saturating_add(8) {
            return Err(invalid(o));
        }
        o
    } else {
        i
    };
    if c.at(open)? != Some(b'(') {
        return Err(invalid(open));
    }
    let mut e = open.saturating_add(1);
    loop {
        match c.at(e)? {
            None | Some(b'>' | b'(') => return Err(invalid(e)),
            Some(b')') => return Ok((true, e.saturating_add(1))),
            Some(_) => e = e.saturating_add(1),
        }
    }
}

/// 要素の修飾つきの名前 `element` の ATTLIST の宣言の範囲（並べた表の中）。比べる分を払う。
pub(crate) fn att_range(
    b: &[u8],
    table: &[AttDecl],
    element: &[u8],
    budget: &Budget<'_>,
) -> Result<(usize, usize), Error> {
    if table.is_empty() {
        return Ok((0, 0));
    }
    let steps = cost::search(table.len() as u64);
    budget.spend_large(steps.saturating_mul(2).saturating_mul(cost::copy(element.len() as u64).saturating_add(1)))?;
    let lo = lower_bound(table, |d| d.element.of(b) < element);
    let hi = lower_bound(table, |d| d.element.of(b) <= element);
    Ok((lo, hi.max(lo)))
}

/// `<!ENTITY`（`k` の位置）の宣言を読む。実体と、宣言の終わりのあとの位置。定義済みの 5 つの名前（`lt` など）の宣言は、
/// 値を見ずに飛ばす（定義済みの値が勝つ。XML 1.0 §4.6 が勧める `<!ENTITY lt "&#38;#60;">` も読める。roxmltree と同じ）。
fn entity(c: &mut Cur<'_, '_>, k: usize, order: u32) -> Result<(Option<Entity>, usize), Error> {
    let mut t = c.skip_space(k.saturating_add(8))?;
    if t == k.saturating_add(8) {
        return Err(invalid(t));
    }
    if c.at(t)? == Some(b'%') {
        return Err(unsupported(Feature::ParameterEntity, k));
    }
    let ne = c.name_end(t)?.ok_or_else(|| invalid(t))?;
    let name = Span::new(t, ne);
    let predefined = matches!(name.of(c.b), b"lt" | b"gt" | b"amp" | b"apos" | b"quot");
    t = c.skip_space(ne)?;
    if t == ne {
        return Err(invalid(t));
    }
    let q = c.at(t)?.ok_or_else(|| invalid(t))?;
    if q != b'"' && q != b'\'' {
        if c.starts(t, b"SYSTEM")? || c.starts(t, b"PUBLIC")? {
            return Err(unsupported(Feature::ExternalEntity, k));
        }
        return Err(invalid(t));
    }
    // 値（`<`・`&`・`%` を含めば Unsupported）
    let vs = t.saturating_add(1);
    let mut ve = vs;
    loop {
        match c.at(ve)? {
            None => return Err(invalid(t)),
            Some(x) if x == q => break,
            Some(b'<' | b'&' | b'%') if !predefined => return Err(unsupported(Feature::MarkupEntity, k)),
            Some(_) => ve = ve.saturating_add(1),
        }
    }
    let close = c.skip_space(ve.saturating_add(1))?;
    if c.at(close)? != Some(b'>') {
        return Err(invalid(close));
    }
    let e = (!predefined).then_some(Entity { name, value: Span::new(vs, ve), order, ..Entity::default() });
    Ok((e, close.saturating_add(1)))
}

/// 名前で引く（並べた表。同じ名前は最初の宣言）。名前のバイトを比べる分（名前の長さ × 歩数）を払う（長い名前は
/// 塊に分けて払う）。
pub(crate) fn lookup(b: &[u8], table: &[Entity], name: &[u8], budget: &Budget<'_>) -> Result<Option<Entity>, Error> {
    let steps = cost::search(table.len() as u64);
    budget.spend_large(steps.saturating_mul(cost::copy(name.len() as u64).saturating_add(1)))?;
    let i = lower_bound(table, |e| e.name.of(b) < name);
    Ok(table.get(i).copied().filter(|e| e.name.of(b) == name))
}

#[cfg(test)]
mod tests;
