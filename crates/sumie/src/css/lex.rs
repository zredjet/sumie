//! CSS の字句（CSS Syntax Level 3 §4。architecture.md §4.2）。バイト列（正しい UTF-8）を字句の並びにする。確保しない。
//!
//! - 前処理（§3.3）は読みながら行う：CR・FF・CRLF は改行、NUL は U+FFFD（解くとき）
//! - escape は字句の範囲を決めるときに飛ばし、中身は [`decode`] で解く（escape も NUL もない中身は入力のまま読める）
//! - 数は読みながら短い形（有効数字の上限と、落とした桁が 0 でないかの印、指数）に揃え、`str::parse::<f64>` で読む
//!   （正しく丸める。determinism.md §3。長い数でも読む手間が字句の長さに比例しない）。有限でない数は `finite` が偽（使う側が
//!   壊れた値として扱う。FR-ST-15）
//! - 読むバイトは 1 バイト 1 単位で、読む前に塊ずつ払う（`ScanMeter`）。どの枝も 1 バイト以上進む（終わりでは `Eof`）

use crate::budget::Budget;
use crate::cost;
use crate::error::Error;
use crate::mem::MVec;
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

    /// 塊を飛ばす積み上げの頭に入れる `{`（位置は使わない）。
    pub(crate) const fn open_brace() -> Token {
        Token::new(Kind::LBrace, 0, 0)
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
    /// `s` の `range` の中だけを読む（字句の範囲は `s` の中の位置。範囲の外は終わりとして扱う）。
    pub(crate) fn over(s: &'a [u8], range: Span, budget: &'b Budget<'b>) -> Lexer<'a, 'b> {
        let end = (range.end as usize).min(s.len());
        let start = (range.start as usize).min(end);
        let s = s.get(..end).unwrap_or(&[]);
        Lexer { s, i: start, meter: ScanMeter::new(start, end), budget }
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

    /// 数の字句（§4.3.3）。数字は読みながら数え（[`NumScan`]）、長い数は短い形に揃えてから読む（`parse` の手間は決まった
    /// 上限の内）。
    fn numeric(&mut self, i: usize) -> Result<Token, Error> {
        let mut j = i;
        let mut integer = true;
        let mut d = NumScan::default();
        let neg = self.at(j)? == Some(b'-');
        if matches!(self.at(j)?, Some(b'+' | b'-')) {
            j = j.saturating_add(1);
        }
        while let Some(b) = self.at(j)?.filter(u8::is_ascii_digit) {
            d.digit(b, j, false);
            j = j.saturating_add(1);
        }
        if self.at(j)? == Some(b'.') && self.at(j.saturating_add(1))?.is_some_and(|b| b.is_ascii_digit()) {
            integer = false;
            j = j.saturating_add(1);
            while let Some(b) = self.at(j)?.filter(u8::is_ascii_digit) {
                d.digit(b, j, true);
                j = j.saturating_add(1);
            }
        }
        if matches!(self.at(j)?, Some(b'e' | b'E')) {
            let k = j.saturating_add(1);
            let sign = self.at(k)?;
            let digits_at = if matches!(sign, Some(b'+' | b'-')) { k.saturating_add(1) } else { k };
            if self.at(digits_at)?.is_some_and(|b| b.is_ascii_digit()) {
                integer = false;
                d.exp_neg = sign == Some(b'-');
                j = digits_at;
                while let Some(b) = self.at(j)?.filter(u8::is_ascii_digit) {
                    d.exp_digit(b);
                    j = j.saturating_add(1);
                }
            }
        }
        let text = self.s.get(i..j).unwrap_or(&[]);
        let value = if text.len() <= SHORT_NUMBER {
            self.budget.spend(cost::number(text.len() as u64))?;
            parse_f64(text)
        } else {
            self.budget.spend(cost::number(NUM_BUF as u64))?;
            let v = d.long_value(self.s);
            if neg { -v } else { v }
        };
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

/// そのまま `str::parse::<f64>` に渡す数の字句の長さの上限（これより長い数は短い形に揃える）。
const SHORT_NUMBER: usize = 64;
/// 揃えた数で持つ有効数字の数。f64 の丸めの境目（隣り合う 2 つの f64 のちょうど中）は 10 進で 767 桁の内に収まるので、
/// 800 桁と「落とした桁に 0 でないものがある」の印の 1 桁で、もとの数と同じに丸まる。
const SIG: u64 = 800;
/// 揃えた数の指数の上限（有効数字が 801 桁なので、これを越えればどれも 0 か ∞）。
const EXP_CAP: i64 = 100_000;
/// 読む指数の上限（小数の桁の数は入力の長さの内なので、これより大きい指数は、桁を合わせても `EXP_CAP` を越える）。
const EXP_READ_CAP: i64 = 1 << 40;
/// 揃えた数の置き場（数字・`e`・指数の符号と数字）。
const NUM_BUF: usize = 832;

/// 指数を ±`EXP_CAP` に収める（`Ord::clamp` は落ちうる口として禁じている）。
const fn clamp_exp(e: i64) -> i64 {
    if e > EXP_CAP {
        EXP_CAP
    } else if e < -EXP_CAP {
        -EXP_CAP
    } else {
        e
    }
}

fn parse_f64(text: &[u8]) -> f64 {
    core::str::from_utf8(text).ok().and_then(|t| t.parse::<f64>().ok()).unwrap_or(f64::NAN)
}

/// 数の字句を読みながら数えたもの（値は `digits × 10^(exp - frac)`）。
#[derive(Default)]
struct NumScan {
    /// 有効数字（頭の 0 を除いた桁）の数と、最後の 0 でない桁が何桁目か
    n_sig: u64,
    last_nonzero: u64,
    /// 最初の有効数字の入力の位置
    first_sig: usize,
    /// 小数の部分の桁の数（頭の 0 も数える）
    frac: u64,
    /// 指数（`EXP_READ_CAP` で止める）
    exp: i64,
    exp_neg: bool,
}

impl NumScan {
    fn digit(&mut self, b: u8, at: usize, frac: bool) {
        if frac {
            self.frac = self.frac.saturating_add(1);
        }
        if self.n_sig == 0 && b == b'0' {
            return;
        }
        if self.n_sig == 0 {
            self.first_sig = at;
        }
        self.n_sig = self.n_sig.saturating_add(1);
        if b != b'0' {
            self.last_nonzero = self.n_sig;
        }
    }

    fn exp_digit(&mut self, b: u8) {
        let v = self.exp.saturating_mul(10).saturating_add(i64::from(b.saturating_sub(b'0')));
        self.exp = if v > EXP_READ_CAP { EXP_READ_CAP } else { v };
    }

    /// 長い数の値（符号は呼び手が付ける）：有効数字を `SIG` 桁まで写し、落とした桁に 0 でないものがあれば 1 桁足して、
    /// 指数を合わせる。写すのは決まった桁数だけ（頭の 0 は飛ばして写さない）。
    fn long_value(&self, s: &[u8]) -> f64 {
        if self.n_sig == 0 {
            return 0.0;
        }
        let mut out = [0u8; NUM_BUF];
        let keep = if self.n_sig < SIG { self.n_sig } else { SIG };
        let mut n = 0usize;
        let mut k = self.first_sig;
        // 有効数字の間には `.` が高々 1 つある
        while (n as u64) < keep {
            let Some(&b) = s.get(k) else { break };
            k = k.saturating_add(1);
            if b == b'.' {
                continue;
            }
            if let Some(x) = out.get_mut(n) {
                *x = b;
            }
            n = n.saturating_add(1);
        }
        let dropped = self.n_sig.saturating_sub(keep);
        let sticky = self.last_nonzero > keep;
        let mut shift: i64 = 0;
        if sticky {
            if let Some(x) = out.get_mut(n) {
                *x = b'1';
            }
            n = n.saturating_add(1);
            shift = -1;
        }
        let e = if self.exp_neg { self.exp.saturating_neg() } else { self.exp };
        let frac = i64::try_from(self.frac).unwrap_or(i64::MAX);
        let dropped = i64::try_from(dropped).unwrap_or(i64::MAX);
        let e = e.saturating_sub(frac).saturating_add(dropped).saturating_add(shift);
        let e = clamp_exp(e);
        if let Some(x) = out.get_mut(n) {
            *x = b'e';
        }
        n = n.saturating_add(1);
        if e < 0 {
            if let Some(x) = out.get_mut(n) {
                *x = b'-';
            }
            n = n.saturating_add(1);
        }
        // 指数の 10 進（上の桁から。|e| ≤ EXP_CAP なので 6 桁まで）
        let mag = e.unsigned_abs();
        let mut started = false;
        let mut p: u64 = 100_000;
        while p > 0 {
            let dgt = mag.checked_div(p).unwrap_or(0).checked_rem(10).unwrap_or(0);
            if dgt != 0 || started || p == 1 {
                started = true;
                if let Some(x) = out.get_mut(n) {
                    *x = b'0'.saturating_add(dgt as u8);
                }
                n = n.saturating_add(1);
            }
            p = p.checked_div(10).unwrap_or(0);
        }
        parse_f64(out.get(..n).unwrap_or(&[]))
    }
}

/// 開きのかっこに対する閉じのかっこ（`{`・`[`・`(`・関数）。
pub(crate) const fn closer(k: Kind) -> Option<Kind> {
    match k {
        Kind::LBrace => Some(Kind::RBrace),
        Kind::LBracket => Some(Kind::RBracket),
        Kind::LParen | Kind::Function => Some(Kind::RParen),
        _ => None,
    }
}

/// かっこの積み上げ（CSS Syntax §5.4.7・§5.4.8：成分の値は、開きのかっこから組の閉じのかっこまで。中の別の種類の閉じは
/// ただの字句）。
pub(crate) struct Nest<'b> {
    stack: MVec<'b, u8>,
}

impl<'b> Nest<'b> {
    pub(crate) fn new(budget: &'b Budget<'b>) -> Nest<'b> {
        Nest { stack: MVec::new(budget) }
    }

    /// どのかっこの中でもないか。
    pub(crate) fn top(&self) -> bool {
        self.stack.is_empty()
    }

    /// 字句 `t` を積み上げに入れる（開きは積み、いちばん内の開きと組の閉じは降ろす。ほかの閉じはただの字句）。降ろして
    /// どのかっこの中でもなくなったら真。
    pub(crate) fn feed(&mut self, t: &Token) -> Result<bool, Error> {
        if let Some(c) = closer(t.kind) {
            self.stack.push(c as u8)?;
        } else if self.stack.last().is_some_and(|&c| c == t.kind as u8) {
            self.stack.pop();
            return Ok(self.stack.is_empty());
        }
        Ok(false)
    }

    pub(crate) fn clear(&mut self) {
        self.stack.clear();
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
