//! 宣言の並び。規則の `{}` の中と `style` 属性の値に使う。
//!
//! - 宣言は `名前 : 値 [! important]`。名前は ident（大文字小文字を問わずに比べるのは使う側。ADR-66 (3)）
//! - `!important` は値の終わりの `!` と ident `important`（大文字小文字を問わない。間に空白を許す）
//! - かっこは種類ごとに組で読む（`(`・`[`・`{`・関数。中の別の種類の閉じはただの字句。CSS Syntax §5.4.7）
//! - 規則の中（CSS Syntax の「consume a block's contents」と CSS Nesting）：`:` のない名前や、名前でない字句で始まるものは
//!   入れ子の規則として読み、最初の深さ 0 の `{…}` の塊までで終える。値に深さ 0 の `{…}` がある宣言（`--` で始まる名前の
//!   ほか）も、その塊までの入れ子の規則。アットルールは `;` か `{…}` まで。入れ子の規則とアットルールは読まずに、知らせて
//!   捨てる（ADR-66 (4)。[`Item::Nested`]）。深さ 0 の `;` だけで終わるものは壊れた宣言で、黙って捨てる
//! - `style` 属性（CSS Syntax L3 の「consume a list of declarations」。Chrome と同じ）：入れ子の規則を読まず、壊れた宣言は
//!   深さ 0 の `;` まで飛ばす。深さ 0 の `}` はただの字句。アットルールは `;` か `{…}` まで飛ばす
//! - 値は空白を除いた範囲で返す（値を読むのは values の段。同じ字句で読み直す）

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::{Kind, Lexer, Nest, Token, ident_is};
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

/// 宣言の並びから知らせるもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Item {
    Decl(Decl),
    /// 読まずに捨てた入れ子の規則かアットルール（規則の中だけ）
    Nested,
}

/// 深さ 0 の `;`（読む）か、`}`（`stop_at_brace` のとき。読まない字句として返す）・終わりまで飛ばす。`first` は読んだ字句。
fn skip_remnants(
    lex: &mut Lexer<'_, '_>,
    nest: &mut Nest<'_>,
    first: Token,
    stop_at_brace: bool,
) -> Result<Token, Error> {
    nest.clear();
    let mut t = first;
    loop {
        if nest.top() {
            match t.kind {
                Kind::Eof | Kind::Semicolon => return Ok(t),
                Kind::RBrace if stop_at_brace => return Ok(t),
                _ => {}
            }
        } else if t.kind == Kind::Eof {
            return Ok(t);
        }
        nest.feed(&t)?;
        t = lex.next()?;
    }
}

/// 入れ子の規則（`first` は読んだ字句）：深さ 0 の `{…}` の塊を読み終えたら None。深さ 0 の `;` で終われば（壊れた宣言）
/// None。深さ 0 の `}`・終わりならその字句（並びの終わり）。塊を読んだかを `nested` に書く。
fn skip_rule(
    lex: &mut Lexer<'_, '_>,
    nest: &mut Nest<'_>,
    first: Token,
    nested: &mut bool,
) -> Result<Option<Token>, Error> {
    nest.clear();
    let mut t = first;
    let mut in_block = false;
    loop {
        if t.kind == Kind::Eof {
            *nested |= in_block;
            return Ok(Some(t));
        }
        if nest.top() {
            match t.kind {
                Kind::Semicolon => return Ok(None),
                Kind::RBrace => return Ok(Some(t)),
                Kind::LBrace => in_block = true,
                _ => {}
            }
        }
        if nest.feed(&t)? && in_block {
            // 深さ 0 の塊を閉じた
            *nested = true;
            return Ok(None);
        }
        t = lex.next()?;
    }
}

/// `lex` から宣言の並びを読み、宣言ごと（と、規則の中で捨てた入れ子の規則ごと）に `f` を呼ぶ。`in_block` なら深さ 0 の
/// `}` で止まり（読んだ `}` の字句を返す）、そうでなければ終わりまで読む（`style` 属性）。
pub(crate) fn parse_declarations(
    lex: &mut Lexer<'_, '_>,
    s: &[u8],
    in_block: bool,
    budget: &Budget<'_>,
    f: &mut dyn FnMut(Item) -> Result<(), Error>,
) -> Result<Token, Error> {
    let budget: &Budget<'_> = budget;
    let mut nest = Nest::new(budget_ref(budget));
    loop {
        budget.spend(cost::OP_STEP)?;
        let t = lex.next()?;
        match t.kind {
            Kind::Whitespace | Kind::Semicolon => {}
            Kind::Eof => return Ok(t),
            Kind::RBrace if in_block => return Ok(t),
            Kind::AtKeyword => {
                // アットルール：`;` か `{…}` まで（規則の中なら深さ 0 の `}` で並びが終わる）
                let mut nested = false;
                let u = lex.next()?;
                let end = if in_block {
                    skip_rule(lex, &mut nest, u, &mut nested)?
                } else {
                    skip_rule_attr(lex, &mut nest, u)?
                };
                if in_block {
                    f(Item::Nested)?;
                }
                if let Some(e) = end {
                    return Ok(e);
                }
            }
            Kind::Ident => {
                if let Some(e) = declaration(lex, s, t, in_block, &mut nest, f)? {
                    return Ok(e);
                }
            }
            _ if in_block => {
                let mut nested = false;
                let end = skip_rule(lex, &mut nest, t, &mut nested)?;
                if nested {
                    f(Item::Nested)?;
                }
                if let Some(e) = end {
                    return Ok(e);
                }
            }
            _ => {
                let e = skip_remnants(lex, &mut nest, t, false)?;
                if e.kind != Kind::Semicolon {
                    return Ok(e);
                }
            }
        }
    }
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// `style` 属性の中のアットルール（`first` は名前の次の字句）：深さ 0 の `;` か `{…}` の塊まで。終わりならその字句。
fn skip_rule_attr(lex: &mut Lexer<'_, '_>, nest: &mut Nest<'_>, first: Token) -> Result<Option<Token>, Error> {
    nest.clear();
    let mut t = first;
    let mut in_block = false;
    loop {
        if t.kind == Kind::Eof {
            return Ok(Some(t));
        }
        if nest.top() {
            match t.kind {
                Kind::Semicolon => return Ok(None),
                Kind::LBrace => in_block = true,
                _ => {}
            }
        }
        if nest.feed(&t)? && in_block {
            return Ok(None);
        }
        t = lex.next()?;
    }
}

/// 1 つの宣言（`name` は読んだ ident）。並びが終わった（`}` か終わり）ならその字句を返す。
fn declaration(
    lex: &mut Lexer<'_, '_>,
    s: &[u8],
    name: Token,
    in_block: bool,
    nest: &mut Nest<'_>,
    f: &mut dyn FnMut(Item) -> Result<(), Error>,
) -> Result<Option<Token>, Error> {
    let mut t = lex.next()?;
    while t.kind == Kind::Whitespace {
        t = lex.next()?;
    }
    if t.kind != Kind::Colon {
        if in_block {
            // `:` がない：入れ子の規則か壊れた宣言
            let mut nested = false;
            let end = skip_rule(lex, nest, t, &mut nested)?;
            if nested {
                f(Item::Nested)?;
            }
            return Ok(end);
        }
        let e = skip_remnants(lex, nest, t, false)?;
        return Ok(if e.kind == Kind::Semicolon { None } else { Some(e) });
    }
    // 値：深さ 0 の `;`・`}`（規則の中）・終わりまで。空白でない字句の終わりの位置を後ろから 3 つ覚える（`!` と
    // `important` の前の値の終わり）
    let custom = name.inner.of(s).starts_with(b"--");
    nest.clear();
    let mut first: Option<u32> = None;
    // ends[0] は最後、ends[1] はその前、ends[2] はさらに前の空白でない字句の終わり
    let mut ends: [u32; 3] = [0; 3];
    let mut count = 0u32;
    let mut prev: Option<Token> = None;
    let mut last: Option<Token> = None;
    let mut block = false;
    let stop = loop {
        let u = lex.next()?;
        if u.kind == Kind::Eof {
            break u;
        }
        if nest.top() {
            match u.kind {
                Kind::Semicolon => break u,
                Kind::RBrace if in_block => break u,
                Kind::LBrace if in_block && !custom => block = true,
                _ => {}
            }
        }
        let closed = nest.feed(&u)?;
        if block && closed {
            // 値に深さ 0 の `{…}` がある宣言は、その塊までの入れ子の規則（CSS Syntax「consume a declaration」と
            // 「consume a qualified rule」）
            f(Item::Nested)?;
            return Ok(None);
        }
        if u.kind != Kind::Whitespace {
            if first.is_none() {
                first = Some(u.span.start);
            }
            ends = [u.span.end, ends[0], ends[1]];
            count = count.saturating_add(1);
            prev = last;
            last = Some(u);
        }
    };
    if block {
        // 塊が閉じないまま終わった（入れ子の規則）
        f(Item::Nested)?;
        return Ok(Some(stop));
    }
    let start = first.unwrap_or(ends[0]);
    let mut value = Span { start, end: ends[0].max(start) };
    let mut important = 0u8;
    if let (Some(p), Some(l)) = (prev, last)
        && p.is_delim(b'!')
        && l.kind == Kind::Ident
        && ident_is(s, l.inner, l.escaped, b"important")
    {
        important = 1;
        // `!` の前の空白でない字句の終わりまで
        let end = if count >= 3 { ends[2] } else { start };
        value = Span { start, end: end.max(start) };
    }
    f(Item::Decl(Decl { name: name.inner, value, important, name_escaped: u8::from(name.escaped), pad: [0; 2] }))?;
    Ok(if stop.kind == Kind::Semicolon { None } else { Some(stop) })
}

#[cfg(test)]
mod tests;
