//! CSS の字句（CSS Syntax Level 3 §4。architecture.md §4.2）。バイト列（正しい UTF-8）を字句の並びにする。確保しない。
//!
//! - 前処理（§3.3）は読みながら行う：CR・FF・CRLF は改行、NUL は U+FFFD（解くとき）
//! - escape は字句の範囲を決めるときに飛ばし、中身は [`decode`] で解く（escape も NUL もない中身は入力のまま読める）
//! - 数は ASCII の部分を切り出して `str::parse::<f64>` で読む（正しく丸める。determinism.md §3）。有限でない数は
//!   `finite` が偽（使う側が壊れた値として扱う。FR-ST-15）
//! - 読むバイトは 1 バイト 1 単位で、読む前に塊ずつ払う（`ScanMeter`）。どの枝も 1 バイト以上進む（終わりでは `Eof`）

use crate::budget::Budget;
use crate::cost;
use crate::error::Error;
use crate::xml::chars::decode as utf8_at;
use crate::xml::lex::{ScanMeter, Span};

/// 字句の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Ident,
    Function,
    AtKeyword,
    Hash,
    Str,
    BadString,
    Url,
    BadUrl,
    Delim,
    Number,
    Percentage,
    Dimension,
    Whitespace,
    Cdo,
    Cdc,
    Colon,
    Semicolon,
    Comma,
    LBracket,
    RBracket,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Eof,
}

/// 字句。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Token {
    pub(crate) kind: Kind,
    /// 字句の全体の範囲
    pub(crate) span: Span,
    /// 中身の生の範囲：ident・function・at-keyword・hash の名前、string・url の中、dimension の単位
    pub(crate) inner: Span,
    /// 中身に escape か NUL がある（[`decode`] で解いて読む）
    pub(crate) escaped: bool,
    /// hash が識別子の形（id）か・数が整数の形か
    pub(crate) flag: bool,
    /// 数の値（Number・Percentage・Dimension）と、それが有限か
    pub(crate) value: f64,
    pub(crate) finite: bool,
    /// delim の符号位置
    pub(crate) delim: u32,
}

impl Token {
    const fn new(kind: Kind, start: usize, end: usize) -> Token {
        Token {
            kind,
            span: Span { start: start as u32, end: end as u32 },
            inner: Span { start: 0, end: 0 },
            escaped: false,
            flag: false,
            value: 0.0,
            finite: true,
            delim: 0,
        }
    }

    /// delim の `c` か。
    pub(crate) fn is_delim(&self, c: u8) -> bool {
        self.kind == Kind::Delim && self.delim == u32::from(c)
    }
}

const fn is_newline(b: u8) -> bool {
    matches!(b, b'\n' | b'\r' | 0x0C)
}

const fn is_ws(b: u8) -> bool {
    is_newline(b) || b == b' ' || b == b'\t'
}

/// 識別子の頭の符号位置（英字・`_`・U+0080 以上）。
const fn is_ident_start(c: u32) -> bool {
    matches!(c, 0x41..=0x5A | 0x61..=0x7A | 0x5F) || c >= 0x80
}

const fn is_ident_char(c: u32) -> bool {
    is_ident_start(c) || matches!(c, 0x30..=0x39 | 0x2D)
}

const fn is_non_printable(b: u8) -> bool {
    matches!(b, 0x00..=0x08 | 0x0B | 0x0E..=0x1F | 0x7F)
}

const fn hex_value(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some(b.saturating_sub(b'0') as u32),
        b'a'..=b'f' => Some((b.saturating_sub(b'a') as u32).saturating_add(10)),
        b'A'..=b'F' => Some((b.saturating_sub(b'A') as u32).saturating_add(10)),
        _ => None,
    }
}

pub(crate) struct Lexer<'a, 'b> {
    s: &'a [u8],
    i: usize,
    meter: ScanMeter,
    budget: &'b Budget<'b>,
}

impl<'a, 'b> Lexer<'a, 'b> {
    pub(crate) fn new(s: &'a [u8], budget: &'b Budget<'b>) -> Lexer<'a, 'b> {
        Lexer { s, i: 0, meter: ScanMeter::new(0, s.len()), budget }
    }

    /// `s` の `range` の中だけを読む（字句の範囲は `s` の中の位置。範囲の外は終わりとして扱う）。
    pub(crate) fn over(s: &'a [u8], range: Span, budget: &'b Budget<'b>) -> Lexer<'a, 'b> {
        let end = (range.end as usize).min(s.len());
        let start = (range.start as usize).min(end);
        let s = s.get(..end).unwrap_or(&[]);
        Lexer { s, i: start, meter: ScanMeter::new(start, end), budget }
    }

    /// 今の位置。
    pub(crate) fn pos(&self) -> usize {
        self.i
    }

    /// `i` のバイト（字句を進めずに見る。払う）。
    pub(crate) fn byte_at(&mut self, i: usize) -> Result<Option<u8>, Error> {
        self.at(i)
    }

    /// `i` のバイト（払ってから読む）。
    fn at(&mut self, i: usize) -> Result<Option<u8>, Error> {
        self.meter.pay(self.budget, i)?;
        Ok(self.s.get(i).copied())
    }

    /// `i` の符号位置とバイト数（多バイトの文字は最後のバイトまで払う）。
    fn cp(&mut self, i: usize) -> Result<Option<(u32, usize)>, Error> {
        if self.at(i)?.is_none() {
            return Ok(None);
        }
        self.at(i.saturating_add(3))?;
        Ok(utf8_at(self.s, i))
    }

    /// `i` から 2 つが正しい escape か（§4.3.8）。
    fn valid_escape(&mut self, i: usize) -> Result<bool, Error> {
        Ok(self.at(i)? == Some(b'\\') && !self.at(i.saturating_add(1))?.is_some_and(is_newline))
    }

    /// `i` から 3 つが識別子を始めるか（§4.3.9）。
    fn starts_ident(&mut self, i: usize) -> Result<bool, Error> {
        match self.at(i)? {
            Some(b'-') => {
                let j = i.saturating_add(1);
                if self.at(j)? == Some(b'-') {
                    return Ok(true);
                }
                if self.cp(j)?.is_some_and(|(c, _)| is_ident_start(c)) {
                    return Ok(true);
                }
                self.valid_escape(j)
            }
            Some(b'\\') => self.valid_escape(i),
            Some(_) => Ok(self.cp(i)?.is_some_and(|(c, _)| is_ident_start(c))),
            None => Ok(false),
        }
    }

    /// `i` から 3 つが数を始めるか（§4.3.10）。
    fn starts_number(&mut self, i: usize) -> Result<bool, Error> {
        let digit = |b: Option<u8>| b.is_some_and(|b| b.is_ascii_digit());
        match self.at(i)? {
            Some(b'+' | b'-') => {
                let j = i.saturating_add(1);
                if digit(self.at(j)?) {
                    return Ok(true);
                }
                Ok(self.at(j)? == Some(b'.') && digit(self.at(j.saturating_add(1))?))
            }
            Some(b'.') => Ok(digit(self.at(i.saturating_add(1))?)),
            b => Ok(digit(b)),
        }
    }

    /// escape（`\` の次の位置 `i` から）を飛ばしたあとの位置（§4.3.7）。
    fn skip_escape(&mut self, i: usize) -> Result<usize, Error> {
        let mut j = i;
        let mut n = 0u32;
        while n < 6
            && let Some(b) = self.at(j)?
            && hex_value(b).is_some()
        {
            j = j.saturating_add(1);
            n = n.saturating_add(1);
        }
        if n > 0 {
            // 16 進のあとの空白 1 つ（CRLF は 1 つ）
            match self.at(j)? {
                Some(b'\r') if self.at(j.saturating_add(1))? == Some(b'\n') => return Ok(j.saturating_add(2)),
                Some(b) if is_ws(b) => return Ok(j.saturating_add(1)),
                _ => return Ok(j),
            }
        }
        match self.cp(j)? {
            Some((_, len)) => Ok(j.saturating_add(len)),
            None => Ok(j),
        }
    }

    /// 名前（§4.3.11）を飛ばし、終わりの位置と escape・NUL があったかを返す。
    fn skip_name(&mut self, mut i: usize) -> Result<(usize, bool), Error> {
        let mut escaped = false;
        loop {
            if self.valid_escape(i)? {
                escaped = true;
                i = self.skip_escape(i.saturating_add(1))?;
                continue;
            }
            match self.cp(i)? {
                Some((0, len)) => {
                    escaped = true;
                    i = i.saturating_add(len);
                }
                Some((c, len)) if is_ident_char(c) => i = i.saturating_add(len),
                _ => return Ok((i, escaped)),
            }
        }
    }

    /// 次の字句。
    pub(crate) fn next(&mut self) -> Result<Token, Error> {
        // 注釈を飛ばす
        loop {
            let i = self.i;
            if self.at(i)? == Some(b'/') && self.at(i.saturating_add(1))? == Some(b'*') {
                let mut j = i.saturating_add(2);
                loop {
                    match self.at(j)? {
                        None => break,
                        Some(b'*') if self.at(j.saturating_add(1))? == Some(b'/') => {
                            j = j.saturating_add(2);
                            break;
                        }
                        Some(_) => j = j.saturating_add(1),
                    }
                }
                self.i = j;
                continue;
            }
            break;
        }
        let i = self.i;
        let Some(b) = self.at(i)? else { return Ok(Token::new(Kind::Eof, i, i)) };
        let one = |k: Kind| Token::new(k, i, i.saturating_add(1));
        let t = match b {
            _ if is_ws(b) => {
                let mut j = i.saturating_add(1);
                while self.at(j)?.is_some_and(is_ws) {
                    j = j.saturating_add(1);
                }
                Token::new(Kind::Whitespace, i, j)
            }
            b'"' | b'\'' => self.string(i, b)?,
            b'#' => {
                let j = i.saturating_add(1);
                let name_start = self.cp(j)?.is_some_and(|(c, _)| is_ident_char(c)) || self.valid_escape(j)?;
                if name_start {
                    let id = self.starts_ident(j)?;
                    let (e, escaped) = self.skip_name(j)?;
                    let mut t = Token::new(Kind::Hash, i, e);
                    t.inner = Span::new(j, e);
                    t.escaped = escaped;
                    t.flag = id;
                    t
                } else {
                    self.delim(i)?
                }
            }
            b'(' => one(Kind::LParen),
            b')' => one(Kind::RParen),
            b',' => one(Kind::Comma),
            b':' => one(Kind::Colon),
            b';' => one(Kind::Semicolon),
            b'[' => one(Kind::LBracket),
            b']' => one(Kind::RBracket),
            b'{' => one(Kind::LBrace),
            b'}' => one(Kind::RBrace),
            b'+' | b'.' if self.starts_number(i)? => self.numeric(i)?,
            b'-' if self.starts_number(i)? => self.numeric(i)?,
            b'-' if self.at(i.saturating_add(1))? == Some(b'-') && self.at(i.saturating_add(2))? == Some(b'>') => {
                Token::new(Kind::Cdc, i, i.saturating_add(3))
            }
            b'-' if self.starts_ident(i)? => self.ident_like(i)?,
            b'<' if self.at(i.saturating_add(1))? == Some(b'!')
                && self.at(i.saturating_add(2))? == Some(b'-')
                && self.at(i.saturating_add(3))? == Some(b'-') =>
            {
                Token::new(Kind::Cdo, i, i.saturating_add(4))
            }
            b'@' if self.starts_ident(i.saturating_add(1))? => {
                let j = i.saturating_add(1);
                let (e, escaped) = self.skip_name(j)?;
                let mut t = Token::new(Kind::AtKeyword, i, e);
                t.inner = Span::new(j, e);
                t.escaped = escaped;
                t
            }
            b'\\' if self.valid_escape(i)? => self.ident_like(i)?,
            b'0'..=b'9' => self.numeric(i)?,
            _ if self.cp(i)?.is_some_and(|(c, _)| is_ident_start(c) || c == 0) => self.ident_like(i)?,
            _ => self.delim(i)?,
        };
        self.i = t.span.end as usize;
        Ok(t)
    }

    fn delim(&mut self, i: usize) -> Result<Token, Error> {
        let (c, len) = self.cp(i)?.unwrap_or((0xFFFD, 1));
        let mut t = Token::new(Kind::Delim, i, i.saturating_add(len.max(1)));
        t.delim = c;
        Ok(t)
    }

    /// 文字列（§4.3.5）。`i` は引用符。
    fn string(&mut self, i: usize, q: u8) -> Result<Token, Error> {
        let start = i.saturating_add(1);
        let mut j = start;
        let mut escaped = false;
        loop {
            match self.at(j)? {
                None => {
                    let mut t = Token::new(Kind::Str, i, j);
                    t.inner = Span::new(start, j);
                    t.escaped = escaped;
                    return Ok(t);
                }
                Some(b) if b == q => {
                    let mut t = Token::new(Kind::Str, i, j.saturating_add(1));
                    t.inner = Span::new(start, j);
                    t.escaped = escaped;
                    return Ok(t);
                }
                // 改行は壊れた文字列（改行は読まずに残す）
                Some(b) if is_newline(b) => return Ok(Token::new(Kind::BadString, i, j)),
                Some(b'\\') => {
                    escaped = true;
                    let k = j.saturating_add(1);
                    match self.at(k)? {
                        None => j = k,
                        Some(b'\r') if self.at(k.saturating_add(1))? == Some(b'\n') => j = k.saturating_add(2),
                        Some(b) if is_newline(b) => j = k.saturating_add(1),
                        Some(_) => j = self.skip_escape(k)?,
                    }
                }
                Some(0) => {
                    escaped = true;
                    j = j.saturating_add(1);
                }
                Some(_) => j = j.saturating_add(1),
            }
        }
    }

    /// 数の字句（§4.3.3）。
    fn numeric(&mut self, i: usize) -> Result<Token, Error> {
        let mut j = i;
        let mut integer = true;
        if matches!(self.at(j)?, Some(b'+' | b'-')) {
            j = j.saturating_add(1);
        }
        while self.at(j)?.is_some_and(|b| b.is_ascii_digit()) {
            j = j.saturating_add(1);
        }
        if self.at(j)? == Some(b'.') && self.at(j.saturating_add(1))?.is_some_and(|b| b.is_ascii_digit()) {
            integer = false;
            j = j.saturating_add(2);
            while self.at(j)?.is_some_and(|b| b.is_ascii_digit()) {
                j = j.saturating_add(1);
            }
        }
        if matches!(self.at(j)?, Some(b'e' | b'E')) {
            let k = j.saturating_add(1);
            let digits_at = if matches!(self.at(k)?, Some(b'+' | b'-')) { k.saturating_add(1) } else { k };
            if self.at(digits_at)?.is_some_and(|b| b.is_ascii_digit()) {
                integer = false;
                j = digits_at;
                while self.at(j)?.is_some_and(|b| b.is_ascii_digit()) {
                    j = j.saturating_add(1);
                }
            }
        }
        let text = self.s.get(i..j).unwrap_or(&[]);
        self.budget.spend_large(cost::number(text.len() as u64))?;
        let value = core::str::from_utf8(text).ok().and_then(|t| t.parse::<f64>().ok()).unwrap_or(f64::NAN);
        let finite = value.is_finite();
        let mut t = if self.starts_ident(j)? {
            let (e, escaped) = self.skip_name(j)?;
            let mut t = Token::new(Kind::Dimension, i, e);
            t.inner = Span::new(j, e);
            t.escaped = escaped;
            t
        } else if self.at(j)? == Some(b'%') {
            Token::new(Kind::Percentage, i, j.saturating_add(1))
        } else {
            Token::new(Kind::Number, i, j)
        };
        t.value = if finite { value } else { 0.0 };
        t.finite = finite;
        t.flag = integer;
        Ok(t)
    }

    /// 識別子の形の字句（§4.3.4）。
    fn ident_like(&mut self, i: usize) -> Result<Token, Error> {
        let (e, escaped) = self.skip_name(i)?;
        let inner = Span::new(i, e);
        if self.at(e)? == Some(b'(') {
            if ident_is(self.s, inner, escaped, b"url") {
                // `url(` のあとの空白を飛ばし、引用符が来れば関数、来なければ url の字句
                let mut j = e.saturating_add(1);
                while self.at(j)?.is_some_and(is_ws) && self.at(j.saturating_add(1))?.is_some_and(is_ws) {
                    j = j.saturating_add(1);
                }
                let k = if self.at(j)?.is_some_and(is_ws) { j.saturating_add(1) } else { j };
                if !matches!(self.at(k)?, Some(b'"' | b'\'')) {
                    return self.url(i, e.saturating_add(1));
                }
            }
            let mut t = Token::new(Kind::Function, i, e.saturating_add(1));
            t.inner = inner;
            t.escaped = escaped;
            return Ok(t);
        }
        let mut t = Token::new(Kind::Ident, i, e);
        t.inner = inner;
        t.escaped = escaped;
        Ok(t)
    }

    /// url の字句（§4.3.6）。`j` は `(` のあと。
    fn url(&mut self, i: usize, mut j: usize) -> Result<Token, Error> {
        while self.at(j)?.is_some_and(is_ws) {
            j = j.saturating_add(1);
        }
        let start = j;
        let mut escaped = false;
        loop {
            match self.at(j)? {
                None | Some(b')') => {
                    let end = j;
                    let close = if self.at(j)? == Some(b')') { j.saturating_add(1) } else { j };
                    let mut t = Token::new(Kind::Url, i, close);
                    t.inner = Span::new(start, end);
                    t.escaped = escaped;
                    return Ok(t);
                }
                Some(b) if is_ws(b) => {
                    let end = j;
                    while self.at(j)?.is_some_and(is_ws) {
                        j = j.saturating_add(1);
                    }
                    if matches!(self.at(j)?, None | Some(b')')) {
                        let close = if self.at(j)? == Some(b')') { j.saturating_add(1) } else { j };
                        let mut t = Token::new(Kind::Url, i, close);
                        t.inner = Span::new(start, end);
                        t.escaped = escaped;
                        return Ok(t);
                    }
                    return self.bad_url(i, j);
                }
                Some(b'"' | b'\'' | b'(') => return self.bad_url(i, j),
                Some(b) if is_non_printable(b) => return self.bad_url(i, j),
                Some(b'\\') => {
                    if self.valid_escape(j)? {
                        escaped = true;
                        j = self.skip_escape(j.saturating_add(1))?;
                    } else {
                        return self.bad_url(i, j);
                    }
                }
                Some(_) => j = j.saturating_add(1),
            }
        }
    }

    /// 壊れた url の残り（§4.3.14）を `)` まで飛ばす。
    fn bad_url(&mut self, i: usize, mut j: usize) -> Result<Token, Error> {
        loop {
            match self.at(j)? {
                None => return Ok(Token::new(Kind::BadUrl, i, j)),
                Some(b')') => return Ok(Token::new(Kind::BadUrl, i, j.saturating_add(1))),
                Some(b'\\') if self.valid_escape(j)? => j = self.skip_escape(j.saturating_add(1))?,
                Some(_) => j = j.saturating_add(1),
            }
        }
    }
}

/// 中身 `span` を解いた符号位置の並び（escape・NUL・文字列の中の改行の続き）。
pub(crate) struct Decode<'a> {
    s: &'a [u8],
    i: usize,
    end: usize,
    /// 文字列の中か（終わりの `\` は、文字列の中では何もせず、名前の中では U+FFFD。§4.3.5・§4.3.7）
    string: bool,
}

/// 名前（ident・function・at-keyword・hash・dimension の単位）の中身を解いて読む。
pub(crate) fn decode(s: &[u8], span: Span) -> Decode<'_> {
    Decode { s, i: span.start as usize, end: (span.end as usize).min(s.len()), string: false }
}

/// 文字列・url の中身を解いて読む。
pub(crate) fn decode_string(s: &[u8], span: Span) -> Decode<'_> {
    Decode { s, i: span.start as usize, end: (span.end as usize).min(s.len()), string: true }
}

impl Iterator for Decode<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        loop {
            if self.i >= self.end {
                return None;
            }
            let b = *self.s.get(self.i)?;
            if b == b'\\' {
                let k = self.i.saturating_add(1);
                let Some(&n) = self.s.get(k).filter(|_| k < self.end) else {
                    // 終わりの `\`：文字列の中なら何もしない、名前の中なら U+FFFD
                    self.i = k;
                    return if self.string { None } else { Some(0xFFFD) };
                };
                if is_newline(n) {
                    // 文字列の中の改行の続き
                    self.i = if n == b'\r' && self.s.get(k.saturating_add(1)) == Some(&b'\n') {
                        k.saturating_add(2)
                    } else {
                        k.saturating_add(1)
                    };
                    continue;
                }
                let mut j = k;
                let mut v: u32 = 0;
                let mut n_hex = 0u32;
                while n_hex < 6
                    && j < self.end
                    && let Some(h) = self.s.get(j).copied().and_then(hex_value)
                {
                    v = v.saturating_mul(16).saturating_add(h);
                    j = j.saturating_add(1);
                    n_hex = n_hex.saturating_add(1);
                }
                if n_hex > 0 {
                    if j < self.end
                        && let Some(&w) = self.s.get(j)
                    {
                        if w == b'\r' && self.s.get(j.saturating_add(1)) == Some(&b'\n') {
                            j = j.saturating_add(2);
                        } else if is_ws(w) {
                            j = j.saturating_add(1);
                        }
                    }
                    self.i = j.min(self.end);
                    return Some(if v == 0 || (0xD800..=0xDFFF).contains(&v) || v > 0x10_FFFF { 0xFFFD } else { v });
                }
                let (c, len) = utf8_at(self.s, k).unwrap_or((0xFFFD, 1));
                self.i = k.saturating_add(len);
                return Some(c);
            }
            let (c, len) = utf8_at(self.s, self.i).unwrap_or((0xFFFD, 1));
            self.i = self.i.saturating_add(len.max(1));
            return Some(if c == 0 { 0xFFFD } else { c });
        }
    }
}

/// 中身が ASCII の `want` と大文字小文字を問わず同じか（`want` は小文字の ASCII）。
pub(crate) fn ident_is(s: &[u8], span: Span, escaped: bool, want: &[u8]) -> bool {
    if !escaped {
        return span.of(s).eq_ignore_ascii_case(want);
    }
    let mut d = decode(s, span);
    for &w in want {
        match d.next() {
            Some(c) if c < 0x80 && (c as u8).eq_ignore_ascii_case(&w) => {}
            _ => return false,
        }
    }
    d.next().is_none()
}

#[cfg(test)]
mod tests;
