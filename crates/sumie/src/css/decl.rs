//! 宣言の並び（CSS Syntax Level 3 §5.4.5「consume a list of declarations」）。規則の `{}` の中と `style` 属性の値に使う。
//!
//! - 宣言は `名前 : 値 [! important]`。名前は ident（大文字小文字を問わずに比べるのは使う側。ADR-66 (3)）
//! - `!important` は値の終わりの `!` と ident `important`（大文字小文字を問わない。間に空白を許す）
//! - 壊れた宣言は、深さ 0 の `;` まで飛ばす（かっこの中の `;` は区切りにしない）。アットルールは飛ばす
//! - 値は空白を除いた範囲で返す（値を読むのは values の段。同じ字句で読み直す）

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::{Kind, Lexer, Token, ident_is};
use crate::error::Error;
use crate::xml::lex::Span;

/// 宣言。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Decl {
    /// 名前（生の範囲。`name_escaped` なら escape を解いて読む）
    pub(crate) name: Span,
    /// 値（前後の空白と `!important` を除いた生の範囲）
    pub(crate) value: Span,
    pub(crate) important: u8,
    pub(crate) name_escaped: u8,
    pub(crate) pad: [u8; 2],
}

impl crate::mem::Plain for Decl {}

impl Decl {
    /// 名前が ASCII の `want`（小文字）と大文字小文字を問わず同じか。
    pub(crate) fn is(&self, s: &[u8], want: &[u8]) -> bool {
        ident_is(s, self.name, self.name_escaped != 0, want)
    }
}

/// 深さ 0 の `;` か `}`（`stop_at_brace` のとき）・終わりまで飛ばす。かっこは組で飛ばす。止まった字句を返す（`;` は読む）。
pub(crate) fn skip_to_semicolon(lex: &mut Lexer<'_, '_>, stop_at_brace: bool) -> Result<Token, Error> {
    let mut depth: u32 = 0;
    loop {
        let t = lex.next()?;
        match t.kind {
            Kind::Eof => return Ok(t),
            Kind::Semicolon if depth == 0 => return Ok(t),
            Kind::RBrace if depth == 0 && stop_at_brace => return Ok(t),
            Kind::LBrace | Kind::LBracket | Kind::LParen | Kind::Function => depth = depth.saturating_add(1),
            Kind::RBrace | Kind::RBracket | Kind::RParen => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
}

/// `lex` から宣言の並びを読み、宣言ごとに `f` を呼ぶ。`in_block` なら深さ 0 の `}` で止まり（読んだ `}` の字句を返す）、
/// そうでなければ終わりまで読む（`style` 属性）。
pub(crate) fn parse_declarations(
    lex: &mut Lexer<'_, '_>,
    s: &[u8],
    in_block: bool,
    budget: &Budget<'_>,
    f: &mut dyn FnMut(Decl) -> Result<(), Error>,
) -> Result<Token, Error> {
    loop {
        budget.spend(cost::OP_STEP)?;
        let t = lex.next()?;
        match t.kind {
            Kind::Whitespace | Kind::Semicolon => {}
            Kind::Eof => return Ok(t),
            Kind::RBrace if in_block => return Ok(t),
            Kind::AtKeyword => {
                // アットルール（宣言の中には書けない）：`;` か `{…}` まで飛ばす
                let mut depth: u32 = 0;
                loop {
                    let u = lex.next()?;
                    match u.kind {
                        Kind::Eof => return Ok(u),
                        Kind::Semicolon if depth == 0 => break,
                        Kind::RBrace if depth == 0 && in_block => return Ok(u),
                        Kind::LBrace | Kind::LBracket | Kind::LParen | Kind::Function => {
                            depth = depth.saturating_add(1);
                        }
                        Kind::RBrace | Kind::RBracket | Kind::RParen => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 && u.kind == Kind::RBrace {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Kind::Ident => {
                let end = declaration(lex, s, t, in_block, f)?;
                if let Some(e) = end {
                    return Ok(e);
                }
            }
            _ => {
                let e = skip_to_semicolon(lex, in_block)?;
                if e.kind != Kind::Semicolon {
                    return Ok(e);
                }
            }
        }
    }
}

/// 1 つの宣言（`name` は読んだ ident）。並びが終わった（`}` か終わり）ならその字句を返す。
fn declaration(
    lex: &mut Lexer<'_, '_>,
    s: &[u8],
    name: Token,
    in_block: bool,
    f: &mut dyn FnMut(Decl) -> Result<(), Error>,
) -> Result<Option<Token>, Error> {
    let mut t = lex.next()?;
    while t.kind == Kind::Whitespace {
        t = lex.next()?;
    }
    if t.kind != Kind::Colon {
        // `:` がない：壊れた宣言
        if matches!(t.kind, Kind::Eof) || (in_block && t.kind == Kind::RBrace) {
            return Ok(Some(t));
        }
        if t.kind == Kind::Semicolon {
            return Ok(None);
        }
        let e = skip_to_semicolon(lex, in_block)?;
        return Ok(if e.kind == Kind::Semicolon { None } else { Some(e) });
    }
    // 値：深さ 0 の `;`・`}`・終わりまで。空白でない最後の 2 つの字句（`!` と `important`）を覚える
    let mut depth: u32 = 0;
    let mut first: Option<u32> = None;
    // 空白でない字句の終わりの位置（最後・その前）と、最後の 2 つの字句
    let mut last_end: u32 = 0;
    let mut prev: Option<Token> = None;
    let mut last: Option<Token> = None;
    let stop = loop {
        let u = lex.next()?;
        let end_here = match u.kind {
            Kind::Eof => true,
            Kind::Semicolon | Kind::RBrace if depth == 0 => u.kind == Kind::Semicolon || in_block,
            _ => false,
        };
        if end_here {
            break u;
        }
        match u.kind {
            Kind::LBrace | Kind::LBracket | Kind::LParen | Kind::Function => depth = depth.saturating_add(1),
            Kind::RBrace | Kind::RBracket | Kind::RParen => depth = depth.saturating_sub(1),
            _ => {}
        }
        if u.kind != Kind::Whitespace {
            if first.is_none() {
                first = Some(u.span.start);
            }
            last_end = u.span.end;
            prev = last;
            last = Some(u);
        }
    };
    let start = first.unwrap_or(last_end);
    let mut value = Span { start, end: last_end.max(start) };
    let mut important = 0u8;
    if let (Some(p), Some(l)) = (prev, last)
        && p.is_delim(b'!')
        && l.kind == Kind::Ident
        && ident_is(s, l.inner, l.escaped, b"important")
    {
        important = 1;
        // `!` の前の空白でない所まで
        let mut e = p.span.start as usize;
        while e > start as usize
            && s.get(e.saturating_sub(1)).is_some_and(|&b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0C))
        {
            e = e.saturating_sub(1);
        }
        value = Span { start, end: (e as u32).max(start) };
    }
    f(Decl { name: name.inner, value, important, name_escaped: u8::from(name.escaped), pad: [0; 2] })?;
    Ok(if stop.kind == Kind::Semicolon { None } else { Some(stop) })
}

#[cfg(test)]
mod tests;
