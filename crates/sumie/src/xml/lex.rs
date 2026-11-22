//! XML の字句（architecture.md §4.1）。入力のバイト列を事象（開きのタグ・属性・閉じのタグ・文字・CDATA・DOCTYPE）の
//! 並びにする。節を作らず、確保もしない（数える段と作る段の両方が同じ字句を使う）。
//!
//! - 読むバイトは 1 バイト 1 単位の手間で、`SCAN_CHUNK` バイトずつ前払いする（読む前に払う。どの事象の中の長い走査も
//!   塊ごとに払うので、中止を見る間隔を守る）
//! - どの枝も必ず 1 バイト以上進む（終わりでは `Eof` を返し続ける）
//! - 注釈と処理命令は読み飛ばす。文字の参照と実体の参照は、事象の生の範囲のまま返す（展開は使う側）
//! - タグの名前の対応（開きと閉じ）・属性の重なり・名前空間は使う側で確かめる。字句は深さの数だけを持つ（根の要素の
//!   終わりを知るため）

use crate::budget::Budget;
use crate::cost;
use crate::error::{Error, Invalid, InvalidReason};
use crate::mem::Plain;
use crate::xml::chars::{decode, is_char, is_name_char, is_name_start, is_space};

/// 入力の中の範囲（バイトの位置。始まりと終わり）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Span {
    pub(crate) start: u32,
    pub(crate) end: u32,
}

impl Plain for Span {}

impl Span {
    pub(crate) fn new(start: usize, end: usize) -> Span {
        Span { start: start as u32, end: end as u32 }
    }

    pub(crate) fn len(self) -> usize {
        self.end.saturating_sub(self.start) as usize
    }

    /// `b` の中の中身（範囲の外なら空）。
    pub(crate) fn of(self, b: &[u8]) -> &[u8] {
        b.get(self.start as usize..self.end as usize).unwrap_or(&[])
    }
}

/// 字句の事象。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// DOCTYPE（内部のサブセットがあればその範囲。`[` と `]` の間）
    Doctype { subset: Option<Span> },
    /// 開きのタグの始まり（`pos` は `<` の位置、`name` は修飾つきの名前）
    Start { pos: u32, name: Span },
    /// 属性（`value` は引用符の間の生の中身）
    Attr { pos: u32, name: Span, value: Span },
    /// 開きのタグの終わり（`/>` なら空の要素）
    StartEnd { empty: bool },
    /// 閉じのタグ
    End { pos: u32, name: Span },
    /// 文字（参照を含む生の中身。改行はまだ揃えていない）
    Text { raw: Span },
    /// CDATA の中身
    CData { raw: Span },
    /// 入力の終わり（根の要素のあと）
    Eof,
}

/// 字句の段階。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// 根の要素の前（`doctype` は DOCTYPE を読んだか）
    Prolog {
        doctype: bool,
    },
    /// 根の要素の中
    Content,
    /// 開きのタグの属性を読んでいる
    InTag,
    /// 根の要素のあと
    Epilog,
    Done,
}

/// 一度に前払いするバイトの数（1 バイト 1 単位。`CANCEL_INTERVAL` 未満）。
pub(crate) const SCAN_CHUNK: usize = 1 << 15;

/// バイトを読む前に、`SCAN_CHUNK` バイトの塊ずつ払う印（1 バイト 1 単位）。`start..end` の外は払わない。どのバイトも
/// 1 回だけ払い、払う 1 回は `CANCEL_INTERVAL` 未満なので、長い走査の中でも決まった間隔で中止を見る。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanMeter {
    /// ここより前は払った
    paid: usize,
    end: usize,
}

impl ScanMeter {
    pub(crate) fn new(start: usize, end: usize) -> ScanMeter {
        ScanMeter { paid: start, end }
    }

    /// `i` のバイトを読む前に呼ぶ（`i` までを塊ずつ払う）。
    pub(crate) fn pay(&mut self, budget: &Budget<'_>, i: usize) -> Result<(), Error> {
        while i >= self.paid && self.paid < self.end {
            let to = self.paid.saturating_add(SCAN_CHUNK).min(self.end);
            budget.spend(cost::scan(to.saturating_sub(self.paid) as u64))?;
            self.paid = to;
        }
        Ok(())
    }
}

pub(crate) struct Lexer<'a, 'b> {
    b: &'a [u8],
    i: usize,
    meter: ScanMeter,
    budget: &'b Budget<'b>,
    phase: Phase,
    /// 開いている要素の数（根の要素の終わりを知るため）
    depth: u64,
}

fn invalid(pos: usize) -> Error {
    Error::Invalid(Invalid { reason: InvalidReason::Xml, position: Some(pos as u64) })
}

impl<'a, 'b> Lexer<'a, 'b> {
    /// `b` の `start` から読む（`start` は BOM のあと）。
    pub(crate) fn new(b: &'a [u8], start: usize, budget: &'b Budget<'b>) -> Lexer<'a, 'b> {
        Lexer {
            b,
            i: start,
            meter: ScanMeter::new(start, b.len()),
            budget,
            phase: Phase::Prolog { doctype: false },
            depth: 0,
        }
    }

    /// 今の位置。
    pub(crate) fn pos(&self) -> usize {
        self.i
    }

    /// `i` のバイトを読む（払っていなければ塊を前払いする）。
    fn at(&mut self, i: usize) -> Result<Option<u8>, Error> {
        self.meter.pay(self.budget, i)?;
        Ok(self.b.get(i).copied())
    }

    /// `i` から `pat` が続くか。
    fn starts(&mut self, i: usize, pat: &[u8]) -> Result<bool, Error> {
        for (k, &p) in pat.iter().enumerate() {
            if self.at(i.saturating_add(k))? != Some(p) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `i` から空白を飛ばした位置。
    fn skip_space(&mut self, mut i: usize) -> Result<usize, Error> {
        while let Some(c) = self.at(i)? {
            if !is_space(c) {
                break;
            }
            i = i.saturating_add(1);
        }
        Ok(i)
    }

    /// `i` の 1 文字（払ってから読む）。符号位置とバイト数。
    fn char_at(&mut self, i: usize) -> Result<Option<(u32, usize)>, Error> {
        // 多バイトの文字の最後のバイトまで払う
        let Some(c0) = self.at(i)? else { return Ok(None) };
        let n = match c0 {
            0xF0..=0xFF => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        };
        self.at(i.saturating_add(n).saturating_sub(1))?;
        Ok(decode(self.b, i))
    }

    /// `i` から名前を読み、終わりの位置を返す（名前でなければ誤り）。
    fn name(&mut self, i: usize) -> Result<usize, Error> {
        let Some((c, n)) = self.char_at(i)? else { return Err(invalid(i)) };
        if !is_name_start(c) {
            return Err(invalid(i));
        }
        let mut j = i.saturating_add(n);
        while let Some((c, n)) = self.char_at(j)? {
            if !is_name_char(c) {
                break;
            }
            j = j.saturating_add(n);
        }
        Ok(j)
    }

    /// `i` から `pat` を探す（文字を確かめながら）。見つかった位置（`pat` の始まり）。
    fn find(&mut self, mut i: usize, pat: &[u8]) -> Result<Option<usize>, Error> {
        while let Some((c, n)) = self.char_at(i)? {
            if self.starts(i, pat)? {
                return Ok(Some(i));
            }
            if !is_char(c) {
                return Err(invalid(i));
            }
            i = i.saturating_add(n);
        }
        Ok(None)
    }

    /// 注釈（`<!--` の位置から）を飛ばし、終わりのあとの位置を返す。中の `--` は誤り（XML 1.0 §2.5）。
    fn comment(&mut self, i: usize) -> Result<usize, Error> {
        let body = i.saturating_add(4);
        let Some(e) = self.find(body, b"--")? else { return Err(invalid(i)) };
        if self.at(e.saturating_add(2))? != Some(b'>') {
            return Err(invalid(e));
        }
        Ok(e.saturating_add(3))
    }

    /// 処理命令（`<?` の位置から）を飛ばし、終わりのあとの位置を返す。`<?xml` に #x20 が続くものは XML の宣言で、文書の
    /// 頭のほかでは誤り。ほかの名前は `xml`・`XML`（`<?xml?>`・`<?xml\tx?>` など）も処理命令として受ける（roxmltree と同じ。
    /// XML 1.0 は `xml` を大文字小文字を問わず処理命令の名前に使えないとするが、roxmltree が受けるものは受ける。NFR-10）。
    fn pi(&mut self, i: usize) -> Result<usize, Error> {
        if self.starts(i, b"<?xml ")? {
            return Err(invalid(i));
        }
        let t = i.saturating_add(2);
        let te = self.name(t)?;
        let Some(e) = self.find(te, b"?>")? else { return Err(invalid(i)) };
        Ok(e.saturating_add(2))
    }

    /// XML の宣言（文書の頭の `<?xml` のあとに #x20）を読む。形は XML 1.0 §2.8 の XMLDecl（`version`、続けて空白を挟んで
    /// `encoding`・`standalone` をこの順に。値は引用符の中を何でも受ける）。
    fn xml_decl(&mut self, i: usize) -> Result<usize, Error> {
        let mut j = self.skip_space(i.saturating_add(5))?;
        j = self.decl_pair(j, b"version")?.ok_or_else(|| invalid(j))?;
        for key in [b"encoding".as_slice(), b"standalone".as_slice()] {
            let s = self.skip_space(j)?;
            if s > j
                && let Some(e) = self.decl_pair(s, key)?
            {
                j = e;
            }
        }
        j = self.skip_space(j)?;
        if !self.starts(j, b"?>")? {
            return Err(invalid(j));
        }
        Ok(j.saturating_add(2))
    }

    /// 宣言の `key = "値"`（`i` が `key` の始まりでなければ None）。値の閉じの引用符のあとの位置。
    fn decl_pair(&mut self, i: usize, key: &[u8]) -> Result<Option<usize>, Error> {
        if !self.starts(i, key)? {
            return Ok(None);
        }
        let mut j = self.skip_space(i.saturating_add(key.len()))?;
        if self.at(j)? != Some(b'=') {
            return Err(invalid(j));
        }
        j = self.skip_space(j.saturating_add(1))?;
        Ok(Some(self.quoted(j)?))
    }

    /// 引用された文字列（`i` は引用符）を飛ばし、閉じの引用符のあとの位置。
    fn quoted(&mut self, i: usize) -> Result<usize, Error> {
        let Some(q) = self.at(i)? else { return Err(invalid(i)) };
        if q != b'"' && q != b'\'' {
            return Err(invalid(i));
        }
        let Some(e) = self.find(i.saturating_add(1), &[q])? else { return Err(invalid(i)) };
        Ok(e.saturating_add(1))
    }

    /// DOCTYPE の中の引用された文字列（文字を確かめない。roxmltree と同じ）。閉じの引用符のあとの位置。
    fn quoted_raw(&mut self, i: usize) -> Result<usize, Error> {
        let Some(q) = self.at(i)? else { return Err(invalid(i)) };
        if q != b'"' && q != b'\'' {
            return Err(invalid(i));
        }
        let mut k = i.saturating_add(1);
        loop {
            match self.at(k)? {
                None => return Err(invalid(i)),
                Some(c) if c == q => return Ok(k.saturating_add(1)),
                Some(_) => k = k.saturating_add(1),
            }
        }
    }

    /// `i` から 1 つ以上の空白を飛ばした位置（空白がなければ誤り）。
    fn space_then(&mut self, i: usize) -> Result<usize, Error> {
        let j = self.skip_space(i)?;
        if j == i {
            return Err(invalid(i));
        }
        Ok(j)
    }

    /// DOCTYPE（`<!DOCTYPE` の位置から）。内部のサブセットの範囲と、終わりのあとの位置。
    fn doctype(&mut self, i: usize) -> Result<(Option<Span>, usize), Error> {
        let mut j = i.saturating_add(9);
        let s = self.skip_space(j)?;
        if s == j {
            return Err(invalid(j));
        }
        j = self.name(s)?;
        j = self.skip_space(j)?;
        // 外部の識別子（読まない）
        if self.starts(j, b"SYSTEM")? {
            j = self.space_then(j.saturating_add(6))?;
            j = self.quoted_raw(j)?;
            j = self.skip_space(j)?;
        } else if self.starts(j, b"PUBLIC")? {
            j = self.space_then(j.saturating_add(6))?;
            j = self.quoted_raw(j)?;
            j = self.space_then(j)?;
            j = self.quoted_raw(j)?;
            j = self.skip_space(j)?;
        }
        let mut subset = None;
        if self.at(j)? == Some(b'[') {
            let start = j.saturating_add(1);
            let mut k = start;
            loop {
                match self.at(k)? {
                    None => return Err(invalid(i)),
                    Some(b']') => break,
                    Some(b'"' | b'\'') => k = self.quoted_raw(k)?,
                    Some(b'<') if self.starts(k, b"<!--")? => k = self.comment(k)?,
                    Some(b'<') if self.starts(k, b"<?")? => k = self.pi(k)?,
                    // 宣言の中の文字は確かめない（roxmltree と同じ。宣言の外の文字は DTD の段で誤りにする）
                    Some(_) => k = k.saturating_add(1),
                }
            }
            subset = Some(Span::new(start, k));
            j = self.skip_space(k.saturating_add(1))?;
        }
        if self.at(j)? != Some(b'>') {
            return Err(invalid(j));
        }
        Ok((subset, j.saturating_add(1)))
    }

    /// 文字（`<` の前まで）。`]]>` と文書の文字でないものは誤り。
    fn text(&mut self, i: usize) -> Result<usize, Error> {
        let mut j = i;
        while let Some(c) = self.at(j)? {
            if c == b'<' {
                break;
            }
            if c == b']' && self.starts(j, b"]]>")? {
                return Err(invalid(j));
            }
            let Some((u, n)) = self.char_at(j)? else { return Err(invalid(j)) };
            if !is_char(u) {
                return Err(invalid(j));
            }
            j = j.saturating_add(n);
        }
        Ok(j)
    }

    /// 次の事象。
    pub(crate) fn next(&mut self) -> Result<Event, Error> {
        loop {
            let i = self.i;
            match self.phase {
                Phase::Done => return Ok(Event::Eof),
                Phase::Prolog { doctype } => {
                    // 文書の頭の `<?xml` ＋ #x20 は XML の宣言（ほかの空白が続くものは処理命令。roxmltree と同じ）
                    if i == self.start_of_doc() && self.starts(i, b"<?xml ")? {
                        self.i = self.xml_decl(i)?;
                        continue;
                    }
                    let j = self.skip_space(i)?;
                    if j != i {
                        self.i = j;
                        continue;
                    }
                    if self.starts(i, b"<!--")? {
                        self.i = self.comment(i)?;
                    } else if self.starts(i, b"<?")? {
                        self.i = self.pi(i)?;
                    } else if self.starts(i, b"<!DOCTYPE")? {
                        if doctype {
                            return Err(invalid(i));
                        }
                        let (subset, e) = self.doctype(i)?;
                        self.i = e;
                        self.phase = Phase::Prolog { doctype: true };
                        return Ok(Event::Doctype { subset });
                    } else if self.at(i)? == Some(b'<') {
                        return self.start_tag(i);
                    } else {
                        // 根の要素がない・前に文字がある
                        return Err(invalid(i));
                    }
                }
                Phase::InTag => return self.attr_or_end(i),
                Phase::Content => {
                    let Some(c) = self.at(i)? else { return Err(invalid(i)) };
                    if c != b'<' {
                        let e = self.text(i)?;
                        self.i = e;
                        return Ok(Event::Text { raw: Span::new(i, e) });
                    }
                    if self.starts(i, b"</")? {
                        let n = i.saturating_add(2);
                        let ne = self.name(n)?;
                        let e = self.skip_space(ne)?;
                        if self.at(e)? != Some(b'>') {
                            return Err(invalid(e));
                        }
                        self.i = e.saturating_add(1);
                        self.depth = self.depth.saturating_sub(1);
                        if self.depth == 0 {
                            self.phase = Phase::Epilog;
                        }
                        return Ok(Event::End { pos: i as u32, name: Span::new(n, ne) });
                    }
                    if self.starts(i, b"<!--")? {
                        self.i = self.comment(i)?;
                    } else if self.starts(i, b"<![CDATA[")? {
                        let body = i.saturating_add(9);
                        let Some(e) = self.find(body, b"]]>")? else { return Err(invalid(i)) };
                        self.i = e.saturating_add(3);
                        return Ok(Event::CData { raw: Span::new(body, e) });
                    } else if self.starts(i, b"<?")? {
                        self.i = self.pi(i)?;
                    } else if self.starts(i, b"<!")? {
                        return Err(invalid(i));
                    } else {
                        return self.start_tag(i);
                    }
                }
                Phase::Epilog => {
                    let j = self.skip_space(i)?;
                    if j != i {
                        self.i = j;
                        continue;
                    }
                    if self.at(i)?.is_none() {
                        self.phase = Phase::Done;
                        return Ok(Event::Eof);
                    }
                    if self.starts(i, b"<!--")? {
                        self.i = self.comment(i)?;
                    } else if self.starts(i, b"<?")? {
                        self.i = self.pi(i)?;
                    } else {
                        // 根の要素のあとの文字・2 つめの根の要素
                        return Err(invalid(i));
                    }
                }
            }
        }
    }

    /// 文書の頭（BOM のあと。XML の宣言はここにだけ書ける）。
    fn start_of_doc(&self) -> usize {
        if self.b.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 }
    }

    /// 開きのタグの始まり（`i` は `<`）。
    fn start_tag(&mut self, i: usize) -> Result<Event, Error> {
        let n = i.saturating_add(1);
        let ne = self.name(n)?;
        self.i = ne;
        self.phase = Phase::InTag;
        Ok(Event::Start { pos: i as u32, name: Span::new(n, ne) })
    }

    /// 開きのタグの中（属性か、タグの終わり）。
    fn attr_or_end(&mut self, i: usize) -> Result<Event, Error> {
        let j = self.skip_space(i)?;
        match self.at(j)? {
            Some(b'>') => {
                self.i = j.saturating_add(1);
                self.depth = self.depth.saturating_add(1);
                self.phase = Phase::Content;
                Ok(Event::StartEnd { empty: false })
            }
            Some(b'/') => {
                if self.at(j.saturating_add(1))? != Some(b'>') {
                    return Err(invalid(j));
                }
                self.i = j.saturating_add(2);
                self.phase = if self.depth == 0 { Phase::Epilog } else { Phase::Content };
                Ok(Event::StartEnd { empty: true })
            }
            Some(_) => {
                // 属性の前には空白が要る
                if j == i {
                    return Err(invalid(j));
                }
                let ne = self.name(j)?;
                let e = self.skip_space(ne)?;
                if self.at(e)? != Some(b'=') {
                    return Err(invalid(e));
                }
                let q = self.skip_space(e.saturating_add(1))?;
                let Some(quote) = self.at(q)? else { return Err(invalid(q)) };
                if quote != b'"' && quote != b'\'' {
                    return Err(invalid(q));
                }
                let vs = q.saturating_add(1);
                let mut k = vs;
                loop {
                    let Some(c) = self.at(k)? else { return Err(invalid(q)) };
                    if c == quote {
                        break;
                    }
                    if c == b'<' {
                        return Err(invalid(k));
                    }
                    let Some((u, n)) = self.char_at(k)? else { return Err(invalid(k)) };
                    if !is_char(u) {
                        return Err(invalid(k));
                    }
                    k = k.saturating_add(n);
                }
                self.i = k.saturating_add(1);
                // 次の属性の前の空白（属性と属性の間に空白がないのは誤り）
                if let Some(c) = self.at(self.i)?
                    && !is_space(c)
                    && c != b'>'
                    && c != b'/'
                {
                    return Err(invalid(self.i));
                }
                Ok(Event::Attr { pos: j as u32, name: Span::new(j, ne), value: Span::new(vs, k) })
            }
            None => Err(invalid(j)),
        }
    }
}

#[cfg(test)]
mod tests;
