//! 値の形の確かめ（カスケードで壊れた値を無視するため。FR-ST-15）。値そのものは、計算した値を求めるとき（木を作る段）に
//! 文脈（文字の大きさ・表示の領域）を使って読み直す。

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::{Kind as Tk, Lexer, ident_is};
use crate::error::Error;
use crate::style::props::{BoxRule, Kind};
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

/// 形を確かめた結果（`@supports` の 3 値のため。単位 3 の 8 回目のレビュー）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Check {
    /// 壊れた値
    Bad,
    /// 形を確かめて受けた
    Sure,
    /// 形を確かめずに受けた（`Kind::Raw`・CSS の箱のプロパティの読めない部品・`offset` の道。Chrome が受けるかは分からない）
    Unsure,
}

impl Check {
    const fn of(b: bool) -> Check {
        if b { Check::Sure } else { Check::Bad }
    }
}

/// 値が `kind` の形か。
pub(crate) fn valid(kind: Kind, s: &[u8], range: Span, cx: Ctx, budget: &Budget<'_>) -> Result<bool, Error> {
    Ok(check(kind, s, range, cx, budget)? != Check::Bad)
}

/// 値が `kind` の形かを、確かめて受けたか・確かめずに受けたかと分けて返す。
pub(crate) fn check(kind: Kind, s: &[u8], range: Span, cx: Ctx, budget: &Budget<'_>) -> Result<Check, Error> {
    let budget = budget_ref(budget);
    match kind {
        Kind::Transform if cx.attr => return Ok(Check::of(values::transform_attr(s, range, budget)?.is_some())),
        Kind::Transform => return Ok(Check::of(values::transform_css_valid(s, range, cx.viewport, budget)?)),
        // `d` 属性はパスの書き方（SVG 2 では壊れた所までを描くので、形はパスを作る段で確かめる）
        Kind::Path if cx.attr => return Ok(Check::Sure),
        Kind::Font => return Ok(Check::of(font_parts(s, range, cx, budget)?.is_some())),
        Kind::Background => return Ok(Check::of(background_color(s, range, budget)?.is_some())),
        Kind::BackgroundImage => return Ok(Check::of(image_list(s, range, budget)?.is_some())),
        Kind::CssBox(rule) => {
            return Ok(match css_box(rule, s, range, cx, budget)? {
                None => Check::Bad,
                Some(b) if b.unknown => Check::Unsure,
                Some(_) => Check::Sure,
            });
        }
        Kind::Offset => {
            return Ok(match offset_form(s, range, cx, budget)? {
                None => Check::Bad,
                Some(true) => Check::Unsure,
                Some(false) => Check::Sure,
            });
        }
        _ => {}
    }
    let mut v = Values::new(s, range, budget);
    if v.at_end()? {
        return Ok(Check::Bad);
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
        Kind::SizeAuto => {
            v.eat_ident(b"auto")?
                || (!cx.attr && eat_any(&mut v, crate::style::props::SIZE_KEYWORDS)?)
                || (!cx.attr && size_function(&mut v, cx)?)
                || len_of(&mut v, cx)?.is_some_and(|l| l.value >= 0.0)
        }
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
                // `shape()`（CSS Shapes 2）も受ける（まだ描かないので NotYet。単位 3 の 4 回目のレビュー）
                const FNS: &[&[u8]] =
                    &[b"inset", b"circle", b"ellipse", b"polygon", b"path", b"rect", b"xywh", b"shape"];
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
                    return Ok(Check::Bad);
                }
            }
            if v.at_end()? {
                return Ok(Check::Unsure);
            }
            false
        }
        Kind::Transform | Kind::Font | Kind::Background | Kind::BackgroundImage | Kind::CssBox(_) | Kind::Offset => {
            false
        }
    };
    Ok(Check::of(ok && v.at_end()?))
}

/// 言葉の並びのどれか 1 つを読む。
fn eat_any(v: &mut Values<'_, '_>, words: &[&[u8]]) -> Result<bool, Error> {
    for w in words {
        if v.eat_ident(w)? {
            return Ok(true);
        }
    }
    Ok(false)
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

/// `calc-size()` の第 2 引数の頭を読む（Chrome：`size` か長さ・割合で、演算子が続かないものは受け、続く字句は見ない。数・
/// ほかの次元・`size` 以外の言葉・`{`・`[`・空は壊れた値。演算子の続く式と、かっこ・関数の頭は、`calc()` と同じく M7 まで
/// 壊れた値（ADR-66・ADR-83。単位 3 の 8 回目のレビュー））。
fn calc_size_head(v: &mut Values<'_, '_>) -> Result<bool, Error> {
    let t = v.peek()?;
    let head = match t.kind {
        Tk::Ident => {
            let ok = ident_is(v.s, t.inner, t.escaped, b"size");
            if ok {
                v.next()?;
            }
            ok
        }
        Tk::Percentage | Tk::Dimension => v.length(false)?.is_some_and(|l| l.unit != Unit::None),
        _ => false,
    };
    if !head {
        return Ok(false);
    }
    let n = v.peek()?;
    Ok(!(n.is_delim(b'+') || n.is_delim(b'-') || n.is_delim(b'*') || n.is_delim(b'/')))
}

/// width・height の大きさの関数で、Chrome が SVG の rect で `auto` と同じに扱うもの（単位 3 の 6・7 回目のレビュー）を 1 つ
/// 読む：基準が大きさの言葉の `calc-size()`（入れ子の `calc-size()` の基準も同じ。続く式は頭を確かめる：[`calc_size_head`]）と、fallback の
/// ない `anchor-size()`（anchor の名前と大きさの言葉をそれぞれ 1 つまで）。基準が `any` か長さの `calc-size()` と fallback の
/// ある `anchor-size()` は長さになり、`calc()` と同じく M7 まで壊れた値（偽を返す。字句は読み進めてよい：値は壊れたもの）。
pub(crate) fn size_function(v: &mut Values<'_, '_>, cx: Ctx) -> Result<bool, Error> {
    let t = v.peek()?;
    if t.kind != Tk::Function {
        return Ok(false);
    }
    if ident_is(v.s, t.inner, t.escaped, b"calc-size") {
        v.next()?;
        // 入れ子の calc-size( を数える（再帰しない）
        let mut depth = 1u32;
        loop {
            let t = v.peek()?;
            if t.kind == Tk::Function && ident_is(v.s, t.inner, t.escaped, b"calc-size") {
                v.next()?;
                depth = depth.saturating_add(1);
            } else {
                break;
            }
        }
        if !eat_any(v, crate::style::props::SIZE_BASIS)? {
            return Ok(false);
        }
        for _ in 0..depth {
            if !v.eat(Tk::Comma)? || !calc_size_head(v)? {
                return Ok(false);
            }
            skip_args(v, cx)?;
        }
        return Ok(true);
    }
    if ident_is(v.s, t.inner, t.escaped, b"anchor-size") {
        v.next()?;
        let (mut name, mut size) = (false, false);
        loop {
            let t = v.next()?;
            match t.kind {
                Tk::RParen | Tk::Eof => return Ok(true),
                Tk::Ident
                    if !size
                        && crate::style::props::ANCHOR_SIZES.iter().any(|w| ident_is(v.s, t.inner, t.escaped, w)) =>
                {
                    size = true;
                }
                Tk::Ident if !name && v.s.get(t.inner.start as usize..).is_some_and(|b| b.starts_with(b"--")) => {
                    name = true;
                }
                // fallback（長さになる）と、ほかの字句（壊れた値）
                _ => return Ok(false),
            }
        }
    }
    Ok(false)
}

/// `offset` のまとめの形（CSS Motion Path 1 §2.6：`[ 位置? [ 道 [ 距離 || 回転 ]? ]? ]! [ / anchor ]?`）を確かめ、要素を
/// 動かしうるかを返す（壊れた値は None）。道のない形（位置・`auto`・`normal`・`none`・箱の言葉と距離）は Chrome の文法で
/// 確かめて Some(false)（Chrome でも SVG の要素は動かない）。道の関数（`path()`・`ray()`・`url()`・基本の形）か、道のあとの距離・回転が
/// あれば、中身を確かめずに Some(true)（効けば NotYet。`@supports` では分からない）。読めない字句は、道の関数があれば
/// Some(true)、なければ壊れた値（Chrome は壊れた値を捨てて前の宣言を生かす。単位 3 の 6〜8 回目のレビュー）。字句ごとに払う。
pub(crate) fn offset_form(s: &[u8], range: Span, cx: Ctx, budget: &Budget<'_>) -> Result<Option<bool>, Error> {
    const BOXES: &[&[u8]] =
        &[b"content-box", b"padding-box", b"border-box", b"margin-box", b"fill-box", b"stroke-box", b"view-box"];
    let budget = budget_ref(budget);
    let cx = Ctx { unitless: false, ..cx };
    let mut v = Values::new(s, range, budget);
    if v.at_end()? {
        return Ok(None);
    }
    // 位置（`auto`・`normal` か 1〜4 つの項）。3 つの項は形を確かめない（Some(None)）
    let position = |v: &mut Values<'_, '_>, words: bool| -> Result<Option<Option<bool>>, Error> {
        if words && (v.eat_ident(b"auto")? || v.eat_ident(b"normal")?) {
            return Ok(Some(Some(true)));
        }
        // 項の種類：0 横の言葉（left・right）、1 縦の言葉（top・bottom）、2 center、3 長さか割合
        let mut items = [0u8; 4];
        let mut n = 0usize;
        while n < 4 {
            budget.spend(cost::OP_STEP)?;
            let item = if v.eat_ident(b"left")? || v.eat_ident(b"right")? {
                0
            } else if v.eat_ident(b"top")? || v.eat_ident(b"bottom")? {
                1
            } else if v.eat_ident(b"center")? {
                2
            } else if len_of(v, cx)?.is_some() {
                3
            } else {
                break;
            };
            if let Some(x) = items.get_mut(n) {
                *x = item;
            }
            n = n.saturating_add(1);
        }
        let [a, b, c, d] = items;
        Ok(match n {
            0 => None,
            1 => Some(Some(true)),
            // 言葉 2 つは順を問わない（同じ向きの 2 つは書けない）。長さを含む形は 横、縦 の順
            2 => Some(Some(match (a, b) {
                (0, 0) | (1, 1) => false,
                (0..=2, 0..=2) => true,
                (x, y) => matches!(x, 0 | 2 | 3) && matches!(y, 1..=3),
            })),
            3 => Some(None),
            // 言葉 長さ 言葉 長さ（横と縦が 1 つずつ）
            _ => Some(Some(a <= 1 && c <= 1 && a != c && b == 3 && d == 3)),
        })
    };
    let mut unsure = false;
    // 位置
    match position(&mut v, true)? {
        Some(Some(false)) => return Ok(None),
        Some(None) => unsure = true,
        _ => {}
    }
    // 道（関数・url・`none`）と箱の言葉（どちらも 1 つまで、順を問わない）
    let (mut func, mut none, mut boxed) = (false, false, false);
    loop {
        budget.spend(cost::OP_STEP)?;
        let t = v.peek()?;
        if !func && !none && matches!(t.kind, Tk::Function | Tk::Url) {
            v.next()?;
            if t.kind == Tk::Function {
                skip_args(&mut v, cx)?;
            }
            func = true;
        } else if !func && !none && !boxed && v.eat_ident(b"none")? {
            none = true;
        } else if !boxed && !none && eat_any(&mut v, BOXES)? {
            boxed = true;
        } else {
            break;
        }
    }
    // 距離と回転（道のあとだけ。どちらも 1 つまで、順を問わない）
    let (mut distance, mut rotate) = (false, false);
    if func || none || boxed {
        loop {
            budget.spend(cost::OP_STEP)?;
            if !distance && len_of(&mut v, cx)?.is_some() {
                distance = true;
            } else if !rotate && (v.eat_ident(b"auto")? || v.eat_ident(b"reverse")?) {
                rotate = true;
                v.angle(false)?;
            } else if !rotate && v.angle(false)?.is_some() {
                rotate = true;
                let _ = v.eat_ident(b"auto")? || v.eat_ident(b"reverse")?;
            } else {
                break;
            }
        }
    }
    // anchor
    if v.peek()?.is_delim(b'/') {
        v.next()?;
        match position(&mut v, true)? {
            None | Some(Some(false)) => return Ok(if func { Some(true) } else { None }),
            Some(None) => unsure = true,
            Some(Some(true)) => {}
        }
    }
    if !v.at_end()? {
        return Ok(if func { Some(true) } else { None });
    }
    // 箱の言葉と距離だけなら Chrome でも動かない（単位 3 の 7 回目のレビュー）。`none` のあとの距離・回転と、箱の言葉のあとの
    // 回転は確かめていないので、動かしうるとみなす
    Ok(Some(func || unsure || (none && (distance || rotate)) || (boxed && rotate)))
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

/// CSS の箱のプロパティの値を確かめた結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoxCheck {
    /// 読めた部品が初期の値と同じ絵になる
    pub(crate) same: bool,
    /// 読めない部品がある（Chrome は受けうる。同じ絵になるかは分からない）
    pub(crate) unknown: bool,
}

impl BoxCheck {
    /// 初期の値と同じ絵になると分かるか。
    pub(crate) const fn plain(self) -> bool {
        self.same && !self.unknown
    }
}

/// CSS の箱のプロパティの値を Chrome と同じ形で確かめ、初期の値と同じ絵になるかを返す（壊れた値は None：宣言ごと捨てる。
/// [`BoxRule`]。宣言ごとに 1 度だけ求める。ADR-103）。字句ごとに払う。
pub(crate) fn css_box(
    rule: BoxRule,
    s: &[u8],
    range: Span,
    cx: Ctx,
    budget: &Budget<'_>,
) -> Result<Option<BoxCheck>, Error> {
    let budget = budget_ref(budget);
    const LINE: &[&[u8]] =
        &[b"none", b"hidden", b"dotted", b"dashed", b"solid", b"double", b"groove", b"ridge", b"inset", b"outset"];
    const WIDTHS: &[&[u8]] = &[b"thin", b"medium", b"thick"];
    let mut v = Values::new(s, range, budget);
    if v.at_end()? {
        return Ok(None);
    }
    // 線の形の言葉を 1 つ読む（読めたら Some(同じ絵か)）。outline は auto も受け、hidden は受けない
    let style = |v: &mut Values<'_, '_>, outline: bool| -> Result<Option<bool>, Error> {
        let t = v.peek()?;
        if t.kind != Tk::Ident {
            return Ok(None);
        }
        let is = |w: &[u8]| ident_is(s, t.inner, t.escaped, w);
        if outline && is(b"auto") {
            v.next()?;
            return Ok(Some(false));
        }
        for w in LINE {
            if is(w) && !(outline && *w == b"hidden") {
                v.next()?;
                return Ok(Some(*w == b"none" || *w == b"hidden"));
            }
        }
        Ok(None)
    };
    // 読めない部品（sumie が読まない色の関数・ほかの関数・知らない単位の次元・知らない言葉）を 1 つ読み飛ばす。Chrome は
    // 受けうるので壊れた値にせず、同じ絵にならないとみなす（NotYet。単位 3 の 7 回目のレビュー）。形の誤り（負の値・
    // 2 つ目の太さなど）だけを壊れた値にする
    let unknown_piece = |v: &mut Values<'_, '_>, idents: bool| -> Result<bool, Error> {
        let t = v.peek()?;
        match t.kind {
            Tk::Function => {
                v.next()?;
                skip_args(v, cx)?;
                Ok(true)
            }
            Tk::Dimension => {
                v.next()?;
                Ok(true)
            }
            Tk::Ident if idents => {
                v.next()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    };
    let mut unknown = false;
    let out = match rule {
        BoxRule::BorderLine | BoxRule::OutlineLine => {
            let outline = rule == BoxRule::OutlineLine;
            let (mut width, mut line, mut color) = (false, None::<bool>, false);
            // 2 つ目の太さ・形・色と、負か割合の太さは壊れた値（Chrome と同じ。読めない部品として読み飛ばさない。単位 3 の
            // 8 回目のレビュー）
            while !v.at_end()? {
                budget.spend(crate::cost::OP_STEP)?;
                if let Some(n) = style(&mut v, outline)? {
                    if line.is_some() {
                        return Ok(None);
                    }
                    line = Some(n);
                    continue;
                }
                if eat_any(&mut v, WIDTHS)? {
                    if width {
                        return Ok(None);
                    }
                    width = true;
                    continue;
                }
                if let Some(l) = len_of(&mut v, cx)? {
                    if width || l.value < 0.0 || l.unit == Unit::Percent {
                        return Ok(None);
                    }
                    width = true;
                    continue;
                }
                let mut probe = v;
                if probe.color()?.is_some() {
                    if color {
                        return Ok(None);
                    }
                    v = probe;
                    color = true;
                    continue;
                }
                if unknown_piece(&mut v, true)? {
                    unknown = true;
                    continue;
                }
                return Ok(None);
            }
            Some(line.unwrap_or(true))
        }
        BoxRule::Styles(_) | BoxRule::OutlineStyle => {
            let (max, outline) = match rule {
                BoxRule::Styles(max) => (max, false),
                _ => (1, true),
            };
            let mut n = 0u8;
            let mut same = true;
            while !v.at_end()? {
                budget.spend(crate::cost::OP_STEP)?;
                let Some(x) = style(&mut v, outline)? else { return Ok(None) };
                n = n.saturating_add(1);
                same &= x;
            }
            (n >= 1 && n <= max).then_some(same)
        }
        BoxRule::Lengths { max, auto, neg } => {
            let mut n = 0u8;
            let mut same = true;
            while !v.at_end()? {
                budget.spend(crate::cost::OP_STEP)?;
                if auto && v.eat_ident(b"auto")? {
                } else if let Some(l) = len_of(&mut v, cx)? {
                    if !neg && l.value < 0.0 {
                        return Ok(None);
                    }
                    same &= l.value == 0.0;
                } else if unknown_piece(&mut v, false)? {
                    unknown = true;
                } else {
                    return Ok(None);
                }
                n = n.saturating_add(1);
            }
            (n >= 1 && n <= max).then_some(same)
        }
        BoxRule::Radius { max, slash } => {
            let (mut n, mut m, mut after) = (0u8, 0u8, false);
            let mut same = true;
            while !v.at_end()? {
                budget.spend(crate::cost::OP_STEP)?;
                let t = v.peek()?;
                if slash && !after && n >= 1 && t.is_delim(b'/') {
                    v.next()?;
                    after = true;
                    continue;
                }
                if let Some(l) = len_of(&mut v, cx)? {
                    if l.value < 0.0 {
                        return Ok(None);
                    }
                    same &= l.value == 0.0;
                } else if unknown_piece(&mut v, false)? {
                    unknown = true;
                } else {
                    return Ok(None);
                }
                if after {
                    m = m.saturating_add(1);
                } else {
                    n = n.saturating_add(1);
                }
            }
            (n >= 1 && n <= max && (!after || (m >= 1 && m <= max))).then_some(same)
        }
        BoxRule::NoneOnly => {
            let none = v.eat_ident(b"none")? && v.at_end()?;
            while !v.at_end()? {
                budget.spend(crate::cost::OP_STEP)?;
                if matches!(v.next()?.kind, Tk::BadString | Tk::BadUrl) {
                    return Ok(None);
                }
            }
            Some(none)
        }
        BoxRule::Logical => {
            // 負の長さは読めない部品を試す前に壊れた値（単位 3 の 8 回目のレビュー）
            if v.eat_ident(b"auto")? || eat_any(&mut v, crate::style::props::SIZE_KEYWORDS)? {
                Some(false)
            } else if let Some(l) = len_of(&mut v, cx)? {
                (l.value >= 0.0).then_some(false)
            } else if unknown_piece(&mut v, false)? {
                unknown = true;
                Some(false)
            } else {
                None
            }
        }
        BoxRule::Keyword { words, neutral } => {
            let t = v.next()?;
            let ok = t.kind == Tk::Ident && words.iter().any(|w| ident_is(s, t.inner, t.escaped, w));
            ok.then(|| ident_is(s, t.inner, t.escaped, neutral))
        }
        BoxRule::Gutter => {
            if v.eat_ident(b"auto")? {
                Some(true)
            } else {
                let (mut stable, mut both) = (false, false);
                while !v.at_end()? {
                    budget.spend(crate::cost::OP_STEP)?;
                    if !stable && v.eat_ident(b"stable")? {
                        stable = true;
                    } else if !both && v.eat_ident(b"both-edges")? {
                        both = true;
                    } else {
                        return Ok(None);
                    }
                }
                stable.then_some(false)
            }
        }
    };
    Ok(if v.at_end()? { out.map(|same| BoxCheck { same, unknown }) } else { None })
}

#[cfg(test)]
mod tests;

/// `background` のまとめの読み（最後の層の色の範囲と、グラデーションの層があるか）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Background {
    pub(crate) color: Option<Span>,
    /// 画像の層の印（[`IMAGE_GRADIENT`]・[`IMAGE_DATA`]）
    pub(crate) image: u8,
}

/// 画像の層にグラデーションがある（関数の引数の中も）。
pub(crate) const IMAGE_GRADIENT: u8 = 1;
/// 画像の層に `data:` の画像がある（Chrome は img の中でも描く）。
pub(crate) const IMAGE_DATA: u8 = 2;
/// 画像の層に `image()`（色の画像など。Chrome は描く）がある（単位 3 の 5 回目のレビュー）。
pub(crate) const IMAGE_FUNCTION: u8 = 4;

/// グラデーションの関数の名前。
const GRADIENTS: &[&[u8]] = &[
    b"linear-gradient",
    b"radial-gradient",
    b"conic-gradient",
    b"repeating-linear-gradient",
    b"repeating-radial-gradient",
    b"repeating-conic-gradient",
    b"-webkit-linear-gradient",
    b"-webkit-radial-gradient",
    b"-webkit-repeating-linear-gradient",
    b"-webkit-repeating-radial-gradient",
    b"-webkit-gradient",
];

/// 画像を包む関数の名前（引数の中のグラデーションと url を見る）。接頭辞のない `cross-fade()` と `element()` は Chrome では
/// 壊れた値（単位 3 の 4 回目のレビュー）。
const IMAGE_WRAPPERS: &[&[u8]] = &[b"image", b"image-set", b"-webkit-image-set", b"-webkit-cross-fade"];

/// URL が `data:` で始まるか（Chrome の URL の読みと同じく、頭の C0 の制御の字と空白を除き、どこにある ASCII の tab・LF・CR も
/// 除いてから見る。単位 3 の 5 回目・6 回目のレビュー）。
fn starts_data(b: &[u8], budget: &Budget<'_>) -> Result<bool, Error> {
    let mut head = [0u8; 5];
    let mut n = 0usize;
    let mut leading = true;
    let mut meter = crate::xml::lex::ScanMeter::new(0, b.len());
    for (i, &c) in b.iter().enumerate() {
        meter.pay(budget, i)?;
        if leading && c <= 0x20 {
            continue;
        }
        leading = false;
        if matches!(c, b'\t' | b'\n' | b'\r') {
            continue;
        }
        let Some(x) = head.get_mut(n) else { break };
        *x = c;
        n = n.saturating_add(1);
    }
    Ok(n == 5 && head.eq_ignore_ascii_case(b"data:"))
}

/// 部分が画像（`none`・url・グラデーション・画像を包む関数）なら、その印（[`IMAGE_GRADIENT`]・[`IMAGE_DATA`]）。画像でなければ
/// None。包む関数は引数の中のグラデーションの関数と `data:` の url を探す。
fn image_flags(s: &[u8], p: &Part, budget: &Budget<'_>) -> Result<Option<u8>, Error> {
    let budget = budget_ref(budget);
    let is_fn = |t: &crate::css::lex::Token, names: &[&[u8]]| {
        t.kind == Tk::Function && names.iter().any(|w| ident_is(s, t.inner, t.escaped, w))
    };
    // url（字句の url か、文字列の url() 関数）が data: か
    let data_url = |t: &crate::css::lex::Token, lex: &mut Lexer<'_, '_>| -> Result<bool, Error> {
        let text = |sp: Span| s.get(sp.start as usize..sp.end as usize).unwrap_or(&[]);
        // escape を含む url は中身を読み直さず、data: とみなす（まだ描かない側に倒す）
        if t.kind == Tk::Url {
            return Ok(t.escaped || starts_data(text(t.inner), budget)?);
        }
        if is_fn(t, &[b"url"]) {
            loop {
                let a = lex.next()?;
                match a.kind {
                    Tk::Whitespace => continue,
                    Tk::Str => return Ok(a.escaped || starts_data(text(a.inner), budget)?),
                    _ => return Ok(false),
                }
            }
        }
        // 画像を包む関数（image-set など）の引数の文字列も URL（単位 3 の 6 回目のレビュー）
        if t.kind == Tk::Str {
            return Ok(t.escaped || starts_data(text(t.inner), budget)?);
        }
        Ok(false)
    };
    if p.t.kind == Tk::Ident && ident_is(s, p.t.inner, p.t.escaped, b"none") {
        return Ok(Some(0));
    }
    if is_fn(&p.t, GRADIENTS) {
        return Ok(Some(IMAGE_GRADIENT));
    }
    if p.t.kind == Tk::Url || is_fn(&p.t, &[b"url"]) {
        let mut lex = Lexer::over(s, p.span, budget);
        let t = lex.next()?;
        return Ok(Some(if data_url(&t, &mut lex)? { IMAGE_DATA } else { 0 }));
    }
    if !is_fn(&p.t, IMAGE_WRAPPERS) {
        return Ok(None);
    }
    // 包む関数：引数の中を字句ごとに見る（どの字句でも 1 つ進む）。`image()` はどこにあっても印にする（色の画像も描く）
    let mut lex = Lexer::over(s, p.span, budget);
    let head = lex.next()?;
    let mut flags = if is_fn(&head, &[b"image"]) { IMAGE_FUNCTION } else { 0 };
    loop {
        budget.spend(cost::OP_STEP)?;
        let t = lex.next()?;
        if t.kind == Tk::Eof {
            break;
        }
        if is_fn(&t, GRADIENTS) {
            flags |= IMAGE_GRADIENT;
        } else if is_fn(&t, &[b"image"]) {
            flags |= IMAGE_FUNCTION;
        } else if data_url(&t, &mut lex)? {
            flags |= IMAGE_DATA;
        }
    }
    Ok(Some(flags))
}

/// `background-image` の値（`,` で分けた層ごとに画像 1 つ）を確かめ、画像の印（[`IMAGE_GRADIENT`]・[`IMAGE_DATA`]）を返す
/// （壊れていれば None）。
pub(crate) fn image_list(s: &[u8], range: Span, budget: &Budget<'_>) -> Result<Option<u8>, Error> {
    let budget = budget_ref(budget);
    let mut lex = Lexer::over(s, range, budget);
    let mut flags = 0u8;
    let mut in_layer = false;
    loop {
        budget.spend(cost::OP_STEP)?;
        let t = lex.next()?;
        match t.kind {
            Tk::Eof => return Ok(in_layer.then_some(flags)),
            Tk::Whitespace => continue,
            Tk::Comma => {
                if !in_layer {
                    return Ok(None);
                }
                in_layer = false;
                continue;
            }
            Tk::BadString | Tk::BadUrl => return Ok(None),
            _ => {}
        }
        if in_layer {
            return Ok(None);
        }
        // 部分（関数なら対応する `)` まで）
        let mut end = t.span.end;
        if t.kind == Tk::Function {
            let mut depth = 1u32;
            loop {
                let u = lex.next()?;
                match u.kind {
                    Tk::Eof => {
                        end = range.end;
                        break;
                    }
                    Tk::Function | Tk::LParen | Tk::LBracket | Tk::LBrace => depth = depth.saturating_add(1),
                    Tk::RParen | Tk::RBracket | Tk::RBrace => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            end = u.span.end;
                            break;
                        }
                    }
                    Tk::BadString | Tk::BadUrl => return Ok(None),
                    _ => {}
                }
            }
        }
        let p = Part::of(s, Span { start: t.span.start, end }, budget)?;
        match image_flags(s, &p, budget)? {
            Some(f) => flags |= f,
            None => return Ok(None),
        }
        in_layer = true;
    }
}

/// まとめの部分（字句 1 つか、関数なら対応する `)` まで）と、その頭の字句（範囲から読み直す）。
#[derive(Debug, Clone, Copy)]
struct Part {
    t: crate::css::lex::Token,
    span: Span,
}

impl Part {
    fn of(s: &[u8], span: Span, budget: &Budget<'_>) -> Result<Part, Error> {
        let mut v = Values::new(s, span, budget_ref(budget));
        Ok(Part { t: v.peek()?, span })
    }
}

/// `background` のまとめ（CSS Backgrounds 3 §3.10）の形を確かめ、最後の層の色の範囲とグラデーションの層があるかを返す（形が
/// 壊れていれば None）。層は `,` で分け、層ごとに 画像（`none`・`url()`・グラデーション・`image-set()` など）・位置（続いた
/// `<bg-position>`）と、その直後の `/` と大きさ（負でない長さか割合か `auto` を 2 つまで、か `cover`・`contain`）・繰り返し
/// （`repeat-x`・`repeat-y` か、続いた 2 つまでの言葉）・付き方・箱 2 つまでを、それぞれ 1 度まで。色は最後の層に 1 つまで
/// （Chrome と同じ。ADR-99 (4)・ADR-101）。
pub(crate) fn background_color(s: &[u8], range: Span, budget: &Budget<'_>) -> Result<Option<Background>, Error> {
    let budget = budget_ref(budget);
    let mut lex = Lexer::over(s, range, budget);
    let mut out = Background { color: None, image: 0 };
    let mut layer: crate::mem::MVec<'_, Span> = crate::mem::MVec::new(budget);
    let mut ended = false;
    let mut last_layer = false;
    while !ended {
        // 層を 1 つ集める
        layer.clear();
        loop {
            let t = lex.next()?;
            match t.kind {
                Tk::Eof => {
                    ended = true;
                    last_layer = true;
                    break;
                }
                Tk::Comma => break,
                Tk::Whitespace => continue,
                Tk::BadString | Tk::BadUrl => return Ok(None),
                _ => {}
            }
            let mut end = t.span.end;
            if t.kind == Tk::Function {
                let mut depth = 1u32;
                loop {
                    let u = lex.next()?;
                    match u.kind {
                        Tk::Eof => {
                            end = range.end;
                            break;
                        }
                        Tk::Function | Tk::LParen | Tk::LBracket | Tk::LBrace => depth = depth.saturating_add(1),
                        Tk::RParen | Tk::RBracket | Tk::RBrace => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                end = u.span.end;
                                break;
                            }
                        }
                        Tk::BadString | Tk::BadUrl => return Ok(None),
                        _ => {}
                    }
                }
            } else if matches!(t.kind, Tk::LParen | Tk::LBracket | Tk::LBrace) {
                return Ok(None);
            }
            layer.push(Span { start: t.span.start, end })?;
        }
        match layer_of(s, layer.as_slice(), last_layer, budget)? {
            Some((color, image)) => {
                if last_layer {
                    out.color = color;
                }
                out.image |= image;
            }
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

/// 1 つの層を確かめる（色の範囲と画像の印。壊れていれば None）。部分は範囲で受け、頭の字句をその都度読み直す。
fn layer_of(s: &[u8], spans: &[Span], last: bool, budget: &Budget<'_>) -> Result<Option<(Option<Span>, u8)>, Error> {
    let budget = budget_ref(budget);
    if spans.is_empty() {
        return Ok(None);
    }
    let part = |i: usize| -> Result<Option<Part>, Error> {
        match spans.get(i) {
            Some(&sp) => Ok(Some(Part::of(s, sp, budget)?)),
            None => Ok(None),
        }
    };
    let is = |p: &Part, w: &[u8]| p.t.kind == Tk::Ident && ident_is(s, p.t.inner, p.t.escaped, w);
    let fname = |p: &Part, w: &[u8]| p.t.kind == Tk::Function && ident_is(s, p.t.inner, p.t.escaped, w);
    // 長さか割合（負でもよい：位置）か、長さか割合の calc()・min()・max()・clamp()（中に長さでない次元があれば壊れた値。
    // 単位 3 の 4 回目のレビュー）
    let lp = |p: &Part| -> Result<bool, Error> {
        if [b"calc".as_slice(), b"min", b"max", b"clamp"].iter().any(|w| fname(p, w)) {
            let mut lex = Lexer::over(s, p.span, budget);
            let _ = lex.next()?;
            let mut length = false;
            loop {
                budget.spend(cost::OP_STEP)?;
                let t = lex.next()?;
                match t.kind {
                    Tk::Eof => return Ok(length),
                    Tk::Percentage => length = true,
                    Tk::Dimension => {
                        let mut v = Values::new(s, t.span, budget);
                        if v.length(false)?.is_none() {
                            return Ok(false);
                        }
                        length = true;
                    }
                    _ => {}
                }
            }
        }
        match p.t.kind {
            Tk::Percentage => Ok(true),
            Tk::Number => Ok(p.t.value == 0.0),
            Tk::Dimension => {
                let mut v = Values::new(s, p.span, budget);
                Ok(v.length(false)?.is_some() && v.at_end()?)
            }
            _ => Ok(false),
        }
    };
    let h = |p: &Part| is(p, b"left") || is(p, b"right");
    let vt = |p: &Part| is(p, b"top") || is(p, b"bottom");
    let c = |p: &Part| is(p, b"center");
    let pos_token = |p: &Part| -> Result<bool, Error> { Ok(h(p) || vt(p) || c(p) || lp(p)?) };
    let mut color: Option<Span> = None;
    let (mut image, mut position, mut repeat, mut attach, mut boxes) = (false, false, false, false, 0u8);
    let mut flags = 0u8;
    let mut i = 0usize;
    while let Some(p) = part(i)? {
        budget.spend(cost::OP_STEP)?;
        // 色（最後の層だけ。1 つまで）
        if matches!(p.t.kind, Tk::Hash | Tk::Ident | Tk::Function) && {
            let mut v = Values::new(s, p.span, budget);
            v.color()?.is_some() && v.at_end()?
        } {
            if !last || color.is_some() {
                return Ok(None);
            }
            color = Some(p.span);
            i = i.saturating_add(1);
            continue;
        }
        // 画像（包む関数の引数の中のグラデーションと data: の url も印にする）
        if let Some(f) = image_flags(s, &p, budget)? {
            if image {
                return Ok(None);
            }
            image = true;
            flags |= f;
            i = i.saturating_add(1);
            continue;
        }
        // 位置（続いた 4 つまでの字句）と、直後の `/` と大きさ
        if pos_token(&p)? {
            if position {
                return Ok(None);
            }
            position = true;
            let mut run: [Option<Part>; 4] = [None; 4];
            let mut n = 0usize;
            while n < 4 {
                match part(i)? {
                    Some(q) if pos_token(&q)? => {
                        if let Some(slot) = run.get_mut(n) {
                            *slot = Some(q);
                        }
                        n = n.saturating_add(1);
                        i = i.saturating_add(1);
                    }
                    _ => break,
                }
            }
            if !position_ok(run.get(..n).unwrap_or(&[]), &h, &vt, &c, &lp)? {
                return Ok(None);
            }
            if part(i)?.is_some_and(|q| q.t.kind == Tk::Delim && q.t.is_delim(b'/')) {
                i = i.saturating_add(1);
                if part(i)?.is_some_and(|q| is(&q, b"cover") || is(&q, b"contain")) {
                    i = i.saturating_add(1);
                    continue;
                }
                let mut k = 0u32;
                while k < 2 {
                    match part(i)? {
                        Some(q) if is(&q, b"auto") || (lp(&q)? && q.t.value >= 0.0) => {
                            k = k.saturating_add(1);
                            i = i.saturating_add(1);
                        }
                        _ => break,
                    }
                }
                if k == 0 {
                    return Ok(None);
                }
            }
            continue;
        }
        // 繰り返し（repeat-x・repeat-y は 1 つで、ほかは続いた 2 つまで）
        if is(&p, b"repeat-x") || is(&p, b"repeat-y") {
            if repeat {
                return Ok(None);
            }
            repeat = true;
            i = i.saturating_add(1);
            continue;
        }
        let rep = |q: &Part| is(q, b"repeat") || is(q, b"space") || is(q, b"round") || is(q, b"no-repeat");
        if rep(&p) {
            if repeat {
                return Ok(None);
            }
            repeat = true;
            i = i.saturating_add(1);
            if part(i)?.is_some_and(|q| rep(&q)) {
                i = i.saturating_add(1);
            }
            continue;
        }
        if is(&p, b"scroll") || is(&p, b"fixed") || is(&p, b"local") {
            if attach {
                return Ok(None);
            }
            attach = true;
            i = i.saturating_add(1);
            continue;
        }
        // 箱（background-origin と background-clip。clip は text と border-area も。Chrome と同じ）
        if is(&p, b"border-box")
            || is(&p, b"padding-box")
            || is(&p, b"content-box")
            || is(&p, b"text")
            || is(&p, b"border-area")
        {
            if boxes >= 2 {
                return Ok(None);
            }
            boxes = boxes.saturating_add(1);
            i = i.saturating_add(1);
            continue;
        }
        return Ok(None);
    }
    Ok(Some((color, flags)))
}

/// 続いた位置の字句が `<bg-position>` の形か（1 つ：どれでも。2 つ：言葉どうしは横 2 つ・縦 2 つでなければ順を問わず、長さを
/// 含めば 1 つ目が横（left・center・right・長さ）で 2 つ目が縦（top・center・bottom・長さ）。3・4 つ：`center` か「言葉
/// 長さ?」の 2 つの組で、横と縦が 1 つずつ）。
fn position_ok(
    run: &[Option<Part>],
    h: &dyn Fn(&Part) -> bool,
    v: &dyn Fn(&Part) -> bool,
    c: &dyn Fn(&Part) -> bool,
    lp: &dyn Fn(&Part) -> Result<bool, Error>,
) -> Result<bool, Error> {
    let kw = |p: &Part| h(p) || v(p) || c(p);
    match run {
        [Some(_)] => Ok(true),
        [Some(a), Some(b)] => {
            if kw(a) && kw(b) {
                return Ok(!(h(a) && h(b)) && !(v(a) && v(b)));
            }
            Ok((h(a) || c(a) || lp(a)?) && (v(b) || c(b) || lp(b)?))
        }
        _ => {
            // 組の種類：1 横・2 縦・3 center
            let mut kinds = [0u8; 2];
            let mut count = 0usize;
            let mut i = 0usize;
            while let Some(Some(p)) = run.get(i) {
                let kind = if c(p) {
                    3
                } else if h(p) {
                    1
                } else if v(p) {
                    2
                } else {
                    return Ok(false);
                };
                i = i.saturating_add(1);
                if kind != 3
                    && let Some(Some(q)) = run.get(i)
                    && !kw(q)
                    && lp(q)?
                {
                    i = i.saturating_add(1);
                }
                match kinds.get_mut(count) {
                    Some(k) => *k = kind,
                    None => return Ok(false),
                }
                count = count.saturating_add(1);
            }
            let [a, b] = kinds;
            Ok(count == 2 && (a != b || a == 3))
        }
    }
}
