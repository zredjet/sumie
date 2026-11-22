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
    /// `unset` と `revert`（利用者の段の規則を持たないので、`revert` は `unset` と同じ）
    Unset,
    /// `revert-layer`（その宣言がないものとして、弱い宣言に戻す。ADR-74）
    RevertLayer,
}

/// 値が CSS の全体の言葉だけか。
pub(crate) fn wide(s: &[u8], range: Span, budget: &Budget<'_>) -> Result<Option<Wide>, Error> {
    let mut v = Values::new(s, range, budget_ref(budget));
    let w = if v.eat_ident(b"inherit")? {
        Wide::Inherit
    } else if v.eat_ident(b"initial")? {
        Wide::Initial
    } else if v.eat_ident(b"unset")? || v.eat_ident(b"revert")? {
        Wide::Unset
    } else if v.eat_ident(b"revert-layer")? {
        Wide::RevertLayer
    } else {
        return Ok(None);
    };
    Ok(v.at_end()?.then_some(w))
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 確かめる文脈。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ctx {
    /// プレゼンテーション属性か（`transform` 属性・`d` 属性は SVG の書き方）
    pub(crate) attr: bool,
    /// 単位のない長さを受けるか（属性はどれも受ける。CSS では SVG のプロパティだけ。ADR-74）
    pub(crate) unitless: bool,
    /// 表示の領域で決まる単位（vw など）を受けるか（文書の大きさが決まるとき）
    pub(crate) viewport: bool,
}

/// 長さか割合（単位のない数は `cx.unitless` のとき。表示の領域の単位は `cx.viewport` のとき）。
fn len_of(v: &mut Values<'_, '_>, cx: Ctx) -> Result<Option<values::Length>, Error> {
    Ok(v.length(cx.unitless)?.filter(|l| cx.viewport || !l.unit.viewport()))
}

/// 値が `kind` の形か。
pub(crate) fn valid(kind: Kind, s: &[u8], range: Span, cx: Ctx, budget: &Budget<'_>) -> Result<bool, Error> {
    let budget = budget_ref(budget);
    match kind {
        Kind::Transform if cx.attr => return Ok(values::transform_attr(s, range, budget)?.is_some()),
        Kind::Transform => return values::transform_css_valid(s, range, cx.viewport, budget),
        // `d` 属性はパスの書き方（SVG 2 では壊れた所までを描くので、形はパスを作る段で確かめる）
        Kind::Path if cx.attr => return Ok(true),
        Kind::Font => return Ok(font_parts(s, range, cx, budget)?.is_some()),
        _ => {}
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
        Kind::Length => len_of(&mut v, cx)?.is_some(),
        Kind::LengthNonNeg => len_of(&mut v, cx)?.is_some_and(|l| l.value >= 0.0),
        Kind::LengthAuto => v.eat_ident(b"auto")? || len_of(&mut v, cx)?.is_some_and(|l| l.value >= 0.0),
        // 負だけが壊れた値（0 以上 1 未満は継ぎ目がいつも bevel になる。SVG 2・Chrome と同じ）
        Kind::MiterLimit => v.number()?.is_some_and(|n| n >= 0.0),
        Kind::Keywords(list) => {
            let t = v.next()?;
            t.kind == Tk::Ident && list.iter().any(|w| ident_is(s, t.inner, t.escaped, w))
        }
        Kind::UrlOrNone => v.eat_ident(b"none")? || url(&mut v)?,
        Kind::Dasharray => {
            if v.eat_ident(b"none")? {
                true
            } else {
                // 数も書ける（SVG 2）
                let cx = Ctx { unitless: true, ..cx };
                let mut ok = true;
                let mut any = false;
                while !v.at_end()? {
                    budget.spend(crate::cost::OP_STEP)?;
                    match len_of(&mut v, cx)? {
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
        Kind::FontFamily => font_family(&mut v, budget)?,
        Kind::FontSize => font_size(&mut v, cx)?,
        Kind::FontWeight => font_weight(&mut v)?,
        Kind::LineHeight => line_height(&mut v, cx)?,
        Kind::FontStyle => font_style(&mut v)?,
        Kind::FontStretch => {
            let t = v.next()?;
            match t.kind {
                Tk::Ident => {
                    ident_is(s, t.inner, t.escaped, b"normal")
                        || crate::style::props::FONT_STRETCH.iter().any(|w| ident_is(s, t.inner, t.escaped, w))
                }
                Tk::Percentage => t.finite && t.value >= 0.0,
                _ => false,
            }
        }
        Kind::Display => display(&mut v, budget)?,
        Kind::Overflow => {
            let mut n = 0u32;
            let mut ok = true;
            while !v.at_end()? {
                let t = v.next()?;
                n = n.saturating_add(1);
                if n > 2
                    || t.kind != Tk::Ident
                    || !crate::style::props::OVERFLOW.iter().any(|w| ident_is(s, t.inner, t.escaped, w))
                {
                    ok = false;
                    break;
                }
            }
            ok && n > 0
        }
        Kind::Spacing => v.eat_ident(b"normal")? || len_of(&mut v, cx)?.is_some(),
        Kind::BaselineShift => {
            v.eat_ident(b"baseline")? || v.eat_ident(b"sub")? || v.eat_ident(b"super")? || len_of(&mut v, cx)?.is_some()
        }
        Kind::TransformOrigin => transform_origin(&mut v, cx, budget)?,
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
                functions_or_urls(&mut v, FNS, cx)?
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
                    // 形の関数と箱の言葉を、それぞれ高々 1 つ（順は問わない）
                    let (mut shape, mut boxed) = (0u32, 0u32);
                    let mut ok = true;
                    while !v.at_end()? {
                        let t = v.next()?;
                        match t.kind {
                            Tk::Function if FNS.iter().any(|w| ident_is(s, t.inner, t.escaped, w)) => {
                                shape = shape.saturating_add(1);
                                ok &= skip_args(&mut v, cx)?;
                            }
                            Tk::Ident if BOXES.iter().any(|w| ident_is(s, t.inner, t.escaped, w)) => {
                                boxed = boxed.saturating_add(1);
                            }
                            _ => ok = false,
                        }
                        if !ok || shape > 1 || boxed > 1 {
                            ok = false;
                            break;
                        }
                    }
                    ok && shape.saturating_add(boxed) > 0
                }
            }
        }
        Kind::Path => {
            // CSS の `d` は `none` か `path("…")`
            if v.eat_ident(b"none")? {
                true
            } else {
                let t = v.next()?;
                if t.kind == Tk::Function && ident_is(s, t.inner, t.escaped, b"path") {
                    // 値の終わりで閉じない関数は閉じたものとする（CSS Syntax）
                    let a = v.next()?;
                    a.kind == Tk::Str && matches!(v.next()?.kind, Tk::RParen | Tk::Eof)
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
        Kind::Transform | Kind::Font => false,
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
            Ok(a.kind == Tk::Str && matches!(v.next()?.kind, Tk::RParen | Tk::Eof))
        }
        _ => Ok(false),
    }
}

/// 関数の引数を組の `)` まで飛ばす（かっこは種類ごとに組で）。大きさの決まらない文書で表示の領域の単位があれば偽。
fn skip_args(v: &mut Values<'_, '_>, cx: Ctx) -> Result<bool, Error> {
    let mut nest = crate::css::lex::Nest::new(v.budget());
    let mut ok = true;
    loop {
        let t = v.next()?;
        match t.kind {
            Tk::Eof => return Ok(ok),
            Tk::RParen if nest.top() => return Ok(ok),
            Tk::Dimension if !cx.viewport => {
                let is = |w: &[u8]| ident_is(v.s, t.inner, t.escaped, w);
                if is(b"vw") || is(b"vh") || is(b"vmin") || is(b"vmax") || is(b"vi") || is(b"vb") {
                    ok = false;
                }
            }
            _ => {}
        }
        nest.feed(&t)?;
    }
}

/// `url()` か、決まった名前の関数の並び（1 つ以上）。
fn functions_or_urls(v: &mut Values<'_, '_>, fns: &[&[u8]], cx: Ctx) -> Result<bool, Error> {
    let mut any = false;
    while !v.at_end()? {
        if url(v)? {
            any = true;
            continue;
        }
        let t = v.next()?;
        if t.kind == Tk::Function && fns.iter().any(|w| ident_is(v.s, t.inner, t.escaped, w)) {
            if !skip_args(v, cx)? {
                return Ok(false);
            }
            any = true;
        } else {
            return Ok(false);
        }
    }
    Ok(any)
}

/// `font-style`：`normal`・`italic`・`oblique <角度 -90deg〜90deg>?`（CSS Fonts 4 §2.4）。
fn font_style(v: &mut Values<'_, '_>) -> Result<bool, Error> {
    if v.eat_ident(b"normal")? || v.eat_ident(b"italic")? {
        return Ok(true);
    }
    if !v.eat_ident(b"oblique")? {
        return Ok(false);
    }
    if v.peek()?.kind == Tk::Dimension {
        return Ok(v.angle(false)?.is_some_and(|a| (-90.0..=90.0).contains(&a)));
    }
    Ok(true)
}

/// `display`（CSS Display 3）：昔の 1 つの言葉か、外の言葉・中の言葉（・`list-item`）を 1 つずつ順を問わずに。
fn display(v: &mut Values<'_, '_>, budget: &Budget<'_>) -> Result<bool, Error> {
    const OUTSIDE: &[&[u8]] = &[b"block", b"inline", b"run-in"];
    const INSIDE: &[&[u8]] = &[b"flow", b"flow-root", b"table", b"flex", b"grid", b"ruby", b"math"];
    const SINGLE: &[&[u8]] =
        &[b"ruby", b"ruby-base", b"ruby-text", b"ruby-base-container", b"ruby-text-container", b"math", b"flow"];
    let s = v.s;
    let first = v.peek()?;
    if first.kind != Tk::Ident {
        return Ok(false);
    }
    // 1 つの言葉
    let one = |t: &crate::css::lex::Token| {
        crate::style::props::DISPLAY.iter().chain(SINGLE).any(|w| ident_is(s, t.inner, t.escaped, w))
    };
    let (mut outside, mut inside, mut item) = (false, None::<bool>, false);
    let mut n = 0u32;
    let mut single = false;
    while !v.at_end()? {
        budget.spend(crate::cost::OP_STEP)?;
        let t = v.next()?;
        if t.kind != Tk::Ident {
            return Ok(false);
        }
        n = n.saturating_add(1);
        let is = |w: &[u8]| ident_is(s, t.inner, t.escaped, w);
        if n == 1 && one(&t) {
            single = true;
        }
        if OUTSIDE.iter().any(|w| is(w)) && !outside {
            outside = true;
        } else if INSIDE.iter().any(|w| is(w)) && inside.is_none() {
            inside = Some(is(b"flow") || is(b"flow-root"));
        } else if is(b"list-item") && !item {
            item = true;
        } else if n > 1 || !single {
            return Ok(false);
        }
    }
    Ok(match n {
        1 => single || outside || inside.is_some() || item,
        // list-item と組む中の言葉は flow か flow-root だけ。list-item なしは 2 つまで
        2 | 3 => (item && inside.is_none_or(|f| f)) || (!item && n == 2 && outside && inside.is_some()),
        _ => false,
    })
}

/// 書体の名前に使えない言葉（CSS の全体の言葉と `default`。CSS Fonts 4 §2.1）。
const RESERVED_FAMILY: &[&[u8]] = &[b"inherit", b"initial", b"unset", b"revert", b"revert-layer", b"default"];

/// 書体の名前の並び（文字列か、ident の続き。`,` で区切る）。ident 1 つだけの名前に全体の言葉は書けない。
fn font_family(v: &mut Values<'_, '_>, budget: &Budget<'_>) -> Result<bool, Error> {
    // 0：名前を待つ、1：文字列のあと、2：ident の続きのあと
    let mut state = 0u8;
    let mut idents = 0u32;
    let mut reserved = false;
    let mut any = false;
    while !v.at_end()? {
        budget.spend(crate::cost::OP_STEP)?;
        let t = v.next()?;
        match (t.kind, state) {
            (Tk::Str, 0) => {
                state = 1;
                any = true;
            }
            (Tk::Ident, 0 | 2) => {
                if state == 0 {
                    idents = 0;
                    reserved = false;
                }
                idents = idents.saturating_add(1);
                reserved |= RESERVED_FAMILY.iter().any(|w| ident_is(v.s, t.inner, t.escaped, w));
                state = 2;
                any = true;
            }
            (Tk::Comma, 1 | 2) => {
                if state == 2 && idents == 1 && reserved {
                    return Ok(false);
                }
                state = 0;
            }
            _ => return Ok(false),
        }
    }
    if state == 2 && idents == 1 && reserved {
        return Ok(false);
    }
    Ok(any && state != 0)
}

/// 文字の大きさの言葉。
const FONT_SIZE_WORDS: &[&[u8]] = &[
    b"math",
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

fn font_size(v: &mut Values<'_, '_>, cx: Ctx) -> Result<bool, Error> {
    let t = v.peek()?;
    if t.kind == Tk::Ident {
        v.next()?;
        return Ok(FONT_SIZE_WORDS.iter().any(|w| ident_is(v.s, t.inner, t.escaped, w)));
    }
    Ok(len_of(v, cx)?.is_some_and(|l| l.value >= 0.0))
}

fn font_weight(v: &mut Values<'_, '_>) -> Result<bool, Error> {
    let t = v.next()?;
    Ok(match t.kind {
        Tk::Ident => {
            [b"normal".as_slice(), b"bold", b"bolder", b"lighter"].iter().any(|w| ident_is(v.s, t.inner, t.escaped, w))
        }
        Tk::Number => t.finite && (1.0..=1000.0).contains(&t.value),
        _ => false,
    })
}

/// `line-height`：`normal`・負でない数・負でない長さか割合。
fn line_height(v: &mut Values<'_, '_>, cx: Ctx) -> Result<bool, Error> {
    if v.eat_ident(b"normal")? {
        return Ok(true);
    }
    if let Some(n) = v.number()? {
        return Ok(n >= 0.0);
    }
    Ok(len_of(v, Ctx { unitless: false, ..cx })?.is_some_and(|l| l.value >= 0.0))
}

/// `transform-origin`（CSS Transforms 1 §6）：1 つ・2 つ（横・縦。言葉 2 つは順を問わない）・それに長さ。
fn transform_origin(v: &mut Values<'_, '_>, cx: Ctx, budget: &Budget<'_>) -> Result<bool, Error> {
    // 項の種類：0 横の言葉（left・right）、1 縦の言葉（top・bottom）、2 center、3 長さか割合
    let mut items = [0u8; 3];
    let mut n = 0usize;
    let mut third_is_length = false;
    while !v.at_end()? {
        budget.spend(crate::cost::OP_STEP)?;
        if n >= 3 {
            return Ok(false);
        }
        let item = if v.eat_ident(b"left")? || v.eat_ident(b"right")? {
            0
        } else if v.eat_ident(b"top")? || v.eat_ident(b"bottom")? {
            1
        } else if v.eat_ident(b"center")? {
            2
        } else {
            match len_of(v, cx)? {
                Some(l) => {
                    if n == 2 {
                        third_is_length = l.unit != Unit::Percent;
                    }
                    3
                }
                None => return Ok(false),
            }
        };
        if let Some(x) = items.get_mut(n) {
            *x = item;
        }
        n = n.saturating_add(1);
    }
    let [a, b, _] = items;
    let pair_ok = || match (a, b) {
        // 言葉 2 つは順を問わない（同じ向きの 2 つは書けない）
        (0, 0) | (1, 1) => false,
        (0..=2, 0..=2) => true,
        // 長さを含む形は 横、縦 の順
        (x, y) => matches!(x, 0 | 2 | 3) && matches!(y, 1..=3),
    };
    Ok(match n {
        1 => true,
        2 => pair_ok(),
        3 => pair_ok() && third_is_length,
        _ => false,
    })
}

/// `font` のまとめの書き方を長い名前ごとの範囲に分けたもの（書いていないものは None）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FontParts {
    pub(crate) style: Option<Span>,
    pub(crate) variant: Option<Span>,
    pub(crate) weight: Option<Span>,
    pub(crate) stretch: Option<Span>,
    pub(crate) size: Option<Span>,
    pub(crate) line_height: Option<Span>,
    pub(crate) family: Option<Span>,
    /// システムの書体の言葉（`caption` など。環境で決まるので、どの長い名前も初期の値にする。ADR-74）
    pub(crate) system: bool,
}

/// 文字の幅の言葉（`font-stretch`）。
const STRETCH: &[&[u8]] = &[
    b"ultra-condensed",
    b"extra-condensed",
    b"condensed",
    b"semi-condensed",
    b"semi-expanded",
    b"expanded",
    b"extra-expanded",
    b"ultra-expanded",
];

/// `font`（CSS Fonts 4 §2.8）：`[ 字形 || small-caps || 太さ || 幅 ]? 大きさ [ / 行の高さ ]? 書体の並び` か、システムの
/// 書体の言葉。長い名前ごとの範囲に分ける。
pub(crate) fn font_parts(s: &[u8], range: Span, cx: Ctx, budget: &Budget<'_>) -> Result<Option<FontParts>, Error> {
    let budget = budget_ref(budget);
    let cx = Ctx { unitless: false, ..cx };
    let mut v = Values::new(s, range, budget);
    let mut p = FontParts::default();
    let first = v.peek()?;
    if first.kind == Tk::Ident
        && [b"caption".as_slice(), b"icon", b"menu", b"message-box", b"small-caption", b"status-bar"]
            .iter()
            .any(|w| ident_is(s, first.inner, first.escaped, w))
    {
        v.next()?;
        p.system = true;
        return Ok(v.at_end()?.then_some(p));
    }
    // 頭の 4 つ（どれも 1 度だけ。`normal` はどれにも数える）
    let mut count = 0u32;
    while count < 4 {
        budget.spend(crate::cost::OP_STEP)?;
        let t = v.peek()?;
        let is = |w: &[u8]| t.kind == Tk::Ident && ident_is(s, t.inner, t.escaped, w);
        if is(b"normal") {
            v.next()?;
        } else if p.style.is_none() && (is(b"italic") || is(b"oblique")) {
            v.next()?;
            let mut span = t.span;
            if is(b"oblique") {
                let a = v.peek()?;
                if a.kind == Tk::Dimension {
                    match v.angle(false)? {
                        Some(d) if (-90.0..=90.0).contains(&d) => span.end = a.span.end,
                        _ => return Ok(None),
                    }
                }
            }
            p.style = Some(span);
        } else if p.variant.is_none() && is(b"small-caps") {
            v.next()?;
            p.variant = Some(t.span);
        } else if p.weight.is_none()
            && (is(b"bold")
                || is(b"bolder")
                || is(b"lighter")
                || (t.kind == Tk::Number && t.finite && (1.0..=1000.0).contains(&t.value)))
        {
            v.next()?;
            p.weight = Some(t.span);
        } else if p.stretch.is_none() && STRETCH.iter().any(|w| is(w)) {
            v.next()?;
            p.stretch = Some(t.span);
        } else {
            break;
        }
        count = count.saturating_add(1);
    }
    // 大きさ（書かなければ壊れている）
    let t = v.peek()?;
    let size_ok = if t.kind == Tk::Ident {
        v.next()?;
        FONT_SIZE_WORDS.iter().any(|w| ident_is(s, t.inner, t.escaped, w))
    } else {
        len_of(&mut v, cx)?.is_some_and(|l| l.value >= 0.0)
    };
    if !size_ok {
        return Ok(None);
    }
    p.size = Some(t.span);
    if v.peek()?.is_delim(b'/') {
        v.next()?;
        let lh = v.peek()?;
        if !line_height(&mut v, cx)? {
            return Ok(None);
        }
        p.line_height = Some(lh.span);
    }
    let f = v.peek()?;
    if f.kind == Tk::Eof {
        return Ok(None);
    }
    let family = Span { start: f.span.start, end: range.end.max(f.span.start) };
    let mut fv = Values::new(s, family, budget);
    if !font_family(&mut fv, budget)? {
        return Ok(None);
    }
    p.family = Some(family);
    Ok(Some(p))
}

#[cfg(test)]
mod tests;
