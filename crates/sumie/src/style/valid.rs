//! 値の形の確かめ（カスケードで壊れた値を無視するため。FR-ST-15）。値そのものは、計算した値を求めるとき（木を作る段）に
//! 文脈（文字の大きさ・表示の領域）を使って読み直す。

use crate::budget::Budget;
use crate::css::lex::{Kind as Tk, ident_is};
use crate::error::Error;
use crate::style::props::Kind;
use crate::values::{self, Unit, Values};
use crate::xml::lex::Span;

/// CSS の全体の言葉（どのプロパティにも書ける）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wide {
    Inherit,
    Initial,
    Unset,
}

/// 値が CSS の全体の言葉だけか（`revert`・`revert-layer` は利用者の段の規則を持たないので `unset` と同じ）。
pub(crate) fn wide(s: &[u8], range: Span, budget: &Budget<'_>) -> Result<Option<Wide>, Error> {
    let mut v = Values::new(s, range, budget_ref(budget));
    let w = if v.eat_ident(b"inherit")? {
        Wide::Inherit
    } else if v.eat_ident(b"initial")? {
        Wide::Initial
    } else if v.eat_ident(b"unset")? || v.eat_ident(b"revert")? || v.eat_ident(b"revert-layer")? {
        Wide::Unset
    } else {
        return Ok(None);
    };
    Ok(v.at_end()?.then_some(w))
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 値が `kind` の形か。`attr` はプレゼンテーション属性か（`transform` 属性は SVG の書き方）。
pub(crate) fn valid(kind: Kind, s: &[u8], range: Span, attr: bool, budget: &Budget<'_>) -> Result<bool, Error> {
    let budget = budget_ref(budget);
    if kind == Kind::Transform {
        return Ok(values::transform(s, range, attr, 16.0, budget)?.is_some());
    }
    let mut v = Values::new(s, range, budget);
    if v.at_end()? {
        return Ok(false);
    }
    let ok = match kind {
        Kind::Paint => v.paint()?.is_some(),
        Kind::Color => v.color()?.is_some(),
        Kind::Opacity => {
            let t = v.next()?;
            t.finite && matches!(t.kind, Tk::Number | Tk::Percentage)
        }
        Kind::Length => v.length(true)?.is_some(),
        Kind::LengthNonNeg => v.length(true)?.is_some_and(|l| l.value >= 0.0),
        Kind::LengthAuto => v.eat_ident(b"auto")? || v.length(true)?.is_some_and(|l| l.value >= 0.0),
        Kind::MiterLimit => v.number()?.is_some_and(|n| n >= 1.0),
        Kind::Keywords(list) => {
            let t = v.next()?;
            t.kind == Tk::Ident && list.iter().any(|w| ident_is(s, t.inner, t.escaped, w))
        }
        Kind::UrlOrNone => v.eat_ident(b"none")? || url(&mut v)?,
        Kind::Dasharray => {
            if v.eat_ident(b"none")? {
                true
            } else {
                let mut ok = true;
                let mut any = false;
                while !v.at_end()? {
                    budget.spend(crate::cost::OP_STEP)?;
                    match v.length(true)? {
                        Some(l) if l.value >= 0.0 => any = true,
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                    if v.peek()?.kind == Tk::Comma {
                        v.next()?;
                        if v.at_end()? {
                            ok = false;
                            break;
                        }
                    }
                }
                ok && any
            }
        }
        Kind::FontFamily => font_family(&mut v)?,
        Kind::FontSize => {
            const WORDS: &[&[u8]] = &[
                b"xx-small",
                b"x-small",
                b"small",
                b"medium",
                b"large",
                b"x-large",
                b"xx-large",
                b"xxx-large",
                b"larger",
                b"smaller",
            ];
            let t = v.peek()?;
            if t.kind == Tk::Ident {
                v.next()?;
                WORDS.iter().any(|w| ident_is(s, t.inner, t.escaped, w))
            } else {
                v.length(true)?.is_some_and(|l| l.value >= 0.0)
            }
        }
        Kind::FontWeight => {
            let t = v.next()?;
            match t.kind {
                Tk::Ident => [b"normal".as_slice(), b"bold", b"bolder", b"lighter"]
                    .iter()
                    .any(|w| ident_is(s, t.inner, t.escaped, w)),
                Tk::Number => t.finite && (1.0..=1000.0).contains(&t.value),
                _ => false,
            }
        }
        Kind::Spacing => v.eat_ident(b"normal")? || v.length(true)?.is_some(),
        Kind::BaselineShift => {
            v.eat_ident(b"baseline")? || v.eat_ident(b"sub")? || v.eat_ident(b"super")? || v.length(true)?.is_some()
        }
        Kind::TransformOrigin => {
            let mut n = 0usize;
            let mut ok = true;
            while !v.at_end()? && n < 3 {
                budget.spend(crate::cost::OP_STEP)?;
                let keyword = v.eat_ident(b"left")?
                    || v.eat_ident(b"center")?
                    || v.eat_ident(b"right")?
                    || v.eat_ident(b"top")?
                    || v.eat_ident(b"bottom")?;
                // 3 つめは長さだけ（割合は書けない）
                if !keyword && !v.length(true)?.is_some_and(|l| n < 2 || l.unit != Unit::Percent) {
                    ok = false;
                    break;
                }
                n = n.saturating_add(1);
            }
            ok && n > 0
        }
        Kind::PaintOrder => {
            if v.eat_ident(b"normal")? {
                true
            } else {
                let mut seen = [false; 3];
                let mut ok = true;
                let mut any = false;
                while !v.at_end()? {
                    let k = if v.eat_ident(b"fill")? {
                        0
                    } else if v.eat_ident(b"stroke")? {
                        1
                    } else if v.eat_ident(b"markers")? {
                        2
                    } else {
                        ok = false;
                        break;
                    };
                    match seen.get_mut(k) {
                        Some(x) if !*x => *x = true,
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                    any = true;
                }
                ok && any
            }
        }
        Kind::Filter => {
            if v.eat_ident(b"none")? {
                true
            } else {
                const FNS: &[&[u8]] = &[
                    b"blur",
                    b"brightness",
                    b"contrast",
                    b"drop-shadow",
                    b"grayscale",
                    b"hue-rotate",
                    b"invert",
                    b"opacity",
                    b"saturate",
                    b"sepia",
                ];
                functions_or_urls(&mut v, FNS)?
            }
        }
        Kind::ClipPath => {
            if v.eat_ident(b"none")? {
                true
            } else {
                const FNS: &[&[u8]] = &[b"inset", b"circle", b"ellipse", b"polygon", b"path", b"rect", b"xywh"];
                const BOXES: &[&[u8]] = &[
                    b"fill-box",
                    b"stroke-box",
                    b"view-box",
                    b"margin-box",
                    b"border-box",
                    b"padding-box",
                    b"content-box",
                ];
                if url(&mut v)? {
                    true
                } else {
                    let mut ok = true;
                    let mut any = false;
                    while !v.at_end()? {
                        let t = v.next()?;
                        match t.kind {
                            Tk::Function if FNS.iter().any(|w| ident_is(s, t.inner, t.escaped, w)) => {
                                skip_args(&mut v)?;
                            }
                            Tk::Ident if BOXES.iter().any(|w| ident_is(s, t.inner, t.escaped, w)) => {}
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                        any = true;
                    }
                    ok && any
                }
            }
        }
        Kind::Path => {
            if v.eat_ident(b"none")? {
                true
            } else {
                let t = v.next()?;
                if t.kind == Tk::Function && ident_is(s, t.inner, t.escaped, b"path") {
                    let a = v.next()?;
                    a.kind == Tk::Str && v.next()?.kind == Tk::RParen
                } else {
                    false
                }
            }
        }
        Kind::Raw => {
            while !v.at_end()? {
                budget.spend(crate::cost::OP_STEP)?;
                let t = v.next()?;
                if matches!(t.kind, Tk::BadString | Tk::BadUrl) {
                    return Ok(false);
                }
            }
            true
        }
        Kind::Transform => false,
    };
    Ok(ok && v.at_end()?)
}

/// `url(...)` を 1 つ読む。
fn url(v: &mut Values<'_, '_>) -> Result<bool, Error> {
    let t = v.peek()?;
    match t.kind {
        Tk::Url => {
            v.next()?;
            Ok(true)
        }
        Tk::Function if ident_is(v.s, t.inner, t.escaped, b"url") => {
            v.next()?;
            let a = v.next()?;
            Ok(a.kind == Tk::Str && v.next()?.kind == Tk::RParen)
        }
        _ => Ok(false),
    }
}

/// 関数の引数を `)` まで飛ばす（かっこは組で）。
fn skip_args(v: &mut Values<'_, '_>) -> Result<(), Error> {
    let mut depth: u32 = 0;
    loop {
        let t = v.next()?;
        match t.kind {
            Tk::Eof => return Ok(()),
            Tk::RParen if depth == 0 => return Ok(()),
            Tk::Function | Tk::LParen => depth = depth.saturating_add(1),
            Tk::RParen => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
}

/// `url()` か、決まった名前の関数の並び（1 つ以上）。
fn functions_or_urls(v: &mut Values<'_, '_>, fns: &[&[u8]]) -> Result<bool, Error> {
    let mut any = false;
    while !v.at_end()? {
        if url(v)? {
            any = true;
            continue;
        }
        let t = v.next()?;
        if t.kind == Tk::Function && fns.iter().any(|w| ident_is(v.s, t.inner, t.escaped, w)) {
            skip_args(v)?;
            any = true;
        } else {
            return Ok(false);
        }
    }
    Ok(any)
}

/// 書体の名前の並び（文字列か ident の続き。`,` で区切る）。
fn font_family(v: &mut Values<'_, '_>) -> Result<bool, Error> {
    let mut want = true;
    let mut any = false;
    while !v.at_end()? {
        let t = v.next()?;
        match t.kind {
            Tk::Str if want => {
                want = false;
                any = true;
            }
            Tk::Ident => {
                // ident の続き（`Times New Roman`）は 1 つの名前
                any = true;
                want = false;
            }
            Tk::Comma if !want => want = true,
            _ => return Ok(false),
        }
    }
    Ok(any && !want)
}

#[cfg(test)]
mod tests;
