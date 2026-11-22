//! 段 2：値（architecture.md §4.2・requirements.md FR-ST-11・FR-SH-04・FR-PT-02・ADR-66 (4)）。プロパティの値を読む。
//!
//! - 値は CSS の字句（[`crate::css::lex`]）で読む（プレゼンテーション属性も CSS の値として読む。SVG 2）。`transform` 属性
//!   だけは SVG の書き方（単位のない数・省いてよい `,`・`rotate(a cx cy)`）も受ける
//! - 数は有限のものだけ（FR-ST-15。有限でない数は壊れた値）
//! - 長さは単位のまま持ち、em・ex・%・vw などは計算した値を求めるとき（文脈が要る）に解く
//! - 色は sRGB の 0〜255（丸めない）と α の 0〜1（塗る段で 8 ビットにする。M3）
//! - 壊れた値は None（呼び手は宣言・属性を無視して警告する）

#![allow(dead_code, reason = "M1 の style の単位で、プロパティの表から使う")]

pub(crate) mod names;

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::{Kind, Lexer, Token, ident_is};
use crate::error::Error;
use crate::math;
use crate::mem::{Plain, lower_bound};
use crate::xml::lex::Span;

/// 長さの単位。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unit {
    /// 単位なし（ユーザー単位）
    None,
    Px,
    Mm,
    Cm,
    In,
    Pt,
    Pc,
    Q,
    Em,
    Ex,
    Rem,
    Percent,
    Vw,
    Vh,
    Vmin,
    Vmax,
}

impl Plain for Unit {}

/// 長さ（単位のまま）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Length {
    pub(crate) value: f64,
    pub(crate) unit: Unit,
    pub(crate) pad: [u8; 7],
}

impl Plain for Length {}

impl Length {
    pub(crate) const fn new(value: f64, unit: Unit) -> Length {
        Length { value, unit, pad: [0; 7] }
    }

    /// 絶対の単位を px（ユーザー単位）にする（96 dpi）。文脈の要る単位は None。
    pub(crate) fn absolute_px(&self) -> Option<f64> {
        let k = match self.unit {
            Unit::None | Unit::Px => 1.0,
            Unit::In => 96.0,
            Unit::Cm => 96.0 / 2.54,
            Unit::Mm => 96.0 / 25.4,
            Unit::Q => 96.0 / 101.6,
            Unit::Pt => 96.0 / 72.0,
            Unit::Pc => 16.0,
            _ => return None,
        };
        Some(self.value * k)
    }
}

/// 色（sRGB。r・g・b は 0〜255、a は 0〜1。丸めない）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Color {
    pub(crate) r: f32,
    pub(crate) g: f32,
    pub(crate) b: f32,
    pub(crate) a: f32,
}

impl Plain for Color {}

impl Color {
    pub(crate) const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub(crate) const TRANSPARENT: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

    const fn rgb24(v: u32) -> Color {
        Color { r: ((v >> 16) & 0xFF) as f32, g: ((v >> 8) & 0xFF) as f32, b: (v & 0xFF) as f32, a: 1.0 }
    }
}

/// 色の値（`currentColor` は計算した値を求めるときに解く）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ColorValue {
    Color(Color),
    CurrentColor,
}

/// 塗り（`fill`・`stroke`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Paint {
    None,
    Color(ColorValue),
    /// `url(#id)`（`href` は値の中の url の中身の範囲）と、参照先が使えないときの代わり
    Url {
        href: Span,
        escaped: bool,
        fallback: Fallback,
    },
    ContextFill,
    ContextStroke,
}

/// url の塗りの代わり。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Fallback {
    /// 書いていない（参照先がなければ none）
    Unset,
    None,
    Color(ColorValue),
}

/// 2 × 3 の変形の行列（`[a c e; b d f]`）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Transform {
    pub(crate) a: f64,
    pub(crate) b: f64,
    pub(crate) c: f64,
    pub(crate) d: f64,
    pub(crate) e: f64,
    pub(crate) f: f64,
}

impl Plain for Transform {}

impl Transform {
    pub(crate) const IDENTITY: Transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// `self × o`（`o` を先に当てる）。
    pub(crate) fn then(&self, o: &Transform) -> Transform {
        Transform {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }

    fn finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f].iter().all(|v| v.is_finite())
    }
}

/// `viewBox`。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ViewBox {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

/// `preserveAspectRatio` の揃え方（`none` と 9 つ）。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    None,
    XMinYMin,
    XMidYMin,
    XMaxYMin,
    XMinYMid,
    XMidYMid,
    XMaxYMid,
    XMinYMax,
    XMidYMax,
    XMaxYMax,
}

/// `preserveAspectRatio`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Aspect {
    pub(crate) align: Align,
    pub(crate) slice: bool,
}

impl Aspect {
    pub(crate) const DEFAULT: Aspect = Aspect { align: Align::XMidYMid, slice: false };
}

/// 値の字句を、空白を飛ばしながら読む口。
pub(crate) struct Values<'s, 'b> {
    pub(crate) s: &'s [u8],
    lex: Lexer<'s, 'b>,
    peeked: Option<Token>,
    budget: &'b Budget<'b>,
}

impl<'s, 'b> Values<'s, 'b> {
    pub(crate) fn new(s: &'s [u8], range: Span, budget: &'b Budget<'b>) -> Values<'s, 'b> {
        Values { s, lex: Lexer::over(s, range, budget), peeked: None, budget }
    }

    /// 次の空白でない字句を見る。
    pub(crate) fn peek(&mut self) -> Result<Token, Error> {
        if let Some(t) = self.peeked {
            return Ok(t);
        }
        loop {
            let t = self.lex.next()?;
            if t.kind != Kind::Whitespace {
                self.peeked = Some(t);
                return Ok(t);
            }
        }
    }

    pub(crate) fn next(&mut self) -> Result<Token, Error> {
        let t = self.peek()?;
        self.peeked = None;
        Ok(t)
    }

    pub(crate) fn at_end(&mut self) -> Result<bool, Error> {
        Ok(self.peek()?.kind == Kind::Eof)
    }

    /// 次が ident `w`（小文字）なら読む。
    pub(crate) fn eat_ident(&mut self, w: &[u8]) -> Result<bool, Error> {
        let t = self.peek()?;
        if t.kind == Kind::Ident && ident_is(self.s, t.inner, t.escaped, w) {
            self.next()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn eat(&mut self, k: Kind) -> Result<bool, Error> {
        if self.peek()?.kind == k {
            self.next()?;
            return Ok(true);
        }
        Ok(false)
    }

    /// 単位の名前（dimension の単位。大文字小文字を問わない）。
    fn unit(&self, t: &Token) -> Option<Unit> {
        let is = |w: &[u8]| ident_is(self.s, t.inner, t.escaped, w);
        Some(if is(b"px") {
            Unit::Px
        } else if is(b"mm") {
            Unit::Mm
        } else if is(b"cm") {
            Unit::Cm
        } else if is(b"in") {
            Unit::In
        } else if is(b"pt") {
            Unit::Pt
        } else if is(b"pc") {
            Unit::Pc
        } else if is(b"q") {
            Unit::Q
        } else if is(b"em") {
            Unit::Em
        } else if is(b"ex") {
            Unit::Ex
        } else if is(b"rem") {
            Unit::Rem
        } else if is(b"vw") {
            Unit::Vw
        } else if is(b"vh") {
            Unit::Vh
        } else if is(b"vmin") {
            Unit::Vmin
        } else if is(b"vmax") {
            Unit::Vmax
        } else {
            return None;
        })
    }

    /// 数（有限のものだけ）。
    pub(crate) fn number(&mut self) -> Result<Option<f64>, Error> {
        let t = self.peek()?;
        if t.kind == Kind::Number && t.finite {
            self.next()?;
            return Ok(Some(t.value));
        }
        Ok(None)
    }

    /// 長さか割合（`unitless` なら単位のない数も受ける）。
    pub(crate) fn length(&mut self, unitless: bool) -> Result<Option<Length>, Error> {
        let t = self.peek()?;
        if !t.finite {
            return Ok(None);
        }
        let l = match t.kind {
            Kind::Number if unitless || t.value == 0.0 => Length::new(t.value, Unit::None),
            Kind::Percentage => Length::new(t.value, Unit::Percent),
            Kind::Dimension => match self.unit(&t) {
                Some(u) => Length::new(t.value, u),
                None => return Ok(None),
            },
            _ => return Ok(None),
        };
        self.next()?;
        Ok(Some(l))
    }

    /// 角度を度で（`unitless` なら単位のない数も度として受ける）。
    pub(crate) fn angle(&mut self, unitless: bool) -> Result<Option<f64>, Error> {
        let t = self.peek()?;
        if !t.finite {
            return Ok(None);
        }
        let deg = match t.kind {
            Kind::Number if unitless || t.value == 0.0 => t.value,
            Kind::Dimension => {
                let is = |w: &[u8]| ident_is(self.s, t.inner, t.escaped, w);
                if is(b"deg") {
                    t.value
                } else if is(b"rad") {
                    t.value * math::RAD_TO_DEG
                } else if is(b"grad") {
                    t.value * 0.9
                } else if is(b"turn") {
                    t.value * 360.0
                } else {
                    return Ok(None);
                }
            }
            _ => return Ok(None),
        };
        self.next()?;
        Ok(Some(deg))
    }

    /// 色（`currentColor` を含む）。
    pub(crate) fn color(&mut self) -> Result<Option<ColorValue>, Error> {
        self.budget.spend(cost::OP_STEP)?;
        let t = self.peek()?;
        match t.kind {
            Kind::Hash => {
                self.next()?;
                Ok(hex(t.inner.of(self.s)).filter(|_| !t.escaped).map(ColorValue::Color))
            }
            Kind::Ident => {
                self.next()?;
                if ident_is(self.s, t.inner, t.escaped, b"currentcolor") {
                    return Ok(Some(ColorValue::CurrentColor));
                }
                if ident_is(self.s, t.inner, t.escaped, b"transparent") {
                    return Ok(Some(ColorValue::Color(Color::TRANSPARENT)));
                }
                if t.escaped {
                    return Ok(None);
                }
                Ok(named(t.inner.of(self.s)).map(ColorValue::Color))
            }
            Kind::Function => {
                self.next()?;
                let is = |w: &[u8]| ident_is(self.s, t.inner, t.escaped, w);
                let c = if is(b"rgb") || is(b"rgba") {
                    self.rgb()?
                } else if is(b"hsl") || is(b"hsla") {
                    self.hsl()?
                } else if is(b"hwb") {
                    self.hwb()?
                } else {
                    None
                };
                Ok(c.map(ColorValue::Color))
            }
            _ => Ok(None),
        }
    }

    /// 色の関数の引数：昔の書き方（`,` で区切る。3 つか 4 つ）と新しい書き方（空白で区切り、`/` のあとに α）。
    /// 返すのは成分と α と、昔の書き方だったか。
    fn components(&mut self, legacy_ok: bool) -> Result<Option<([Comp; 3], f64, bool)>, Error> {
        let mut comps = [Comp::None; 3];
        let first = self.comp()?;
        let Some(c0) = first else { return Ok(None) };
        comps[0] = c0;
        let legacy = legacy_ok && self.eat(Kind::Comma)?;
        for k in 1..3 {
            let Some(c) = self.comp()? else { return Ok(None) };
            if let Some(x) = comps.get_mut(k) {
                *x = c;
            }
            if legacy && k < 2 && !self.eat(Kind::Comma)? {
                return Ok(None);
            }
        }
        let mut alpha = 1.0;
        let sep = if legacy {
            self.eat(Kind::Comma)?
        } else if self.peek()?.is_delim(b'/') {
            self.next()?;
            true
        } else {
            false
        };
        if sep {
            let t = self.peek()?;
            alpha = match t.kind {
                Kind::Number if t.finite => t.value,
                Kind::Percentage if t.finite => t.value / 100.0,
                Kind::Ident if !legacy && ident_is(self.s, t.inner, t.escaped, b"none") => 0.0,
                _ => return Ok(None),
            };
            self.next()?;
        }
        if !self.eat(Kind::RParen)? && !self.at_end()? {
            return Ok(None);
        }
        // 昔の書き方は `none` を書けない
        if legacy && comps.iter().any(|c| matches!(c, Comp::NoneKw)) {
            return Ok(None);
        }
        Ok(Some((comps, math::clamp(alpha, 0.0, 1.0), legacy)))
    }

    fn comp(&mut self) -> Result<Option<Comp>, Error> {
        let t = self.peek()?;
        if !t.finite {
            return Ok(None);
        }
        let c = match t.kind {
            Kind::Number => Comp::Num(t.value),
            Kind::Percentage => Comp::Pct(t.value),
            Kind::Dimension => match self.angle(false)? {
                Some(d) => return Ok(Some(Comp::Num(d))),
                None => return Ok(None),
            },
            Kind::Ident if ident_is(self.s, t.inner, t.escaped, b"none") => Comp::NoneKw,
            _ => return Ok(None),
        };
        self.next()?;
        Ok(Some(c))
    }

    fn rgb(&mut self) -> Result<Option<Color>, Error> {
        let Some((c, a, legacy)) = self.components(true)? else { return Ok(None) };
        let ch = |x: Comp| match x {
            Comp::Num(v) => math::clamp(v, 0.0, 255.0),
            Comp::Pct(p) => math::clamp(p * 2.55, 0.0, 255.0),
            Comp::NoneKw | Comp::None => 0.0,
        };
        // 昔の書き方は、3 つとも数か 3 つとも割合（CSS Color 4 §5.1）。角度は rgb() に書けない
        let pct = c.iter().filter(|x| matches!(x, Comp::Pct(_))).count();
        if legacy && pct != 0 && pct != 3 {
            return Ok(None);
        }
        Ok(Some(Color { r: ch(c[0]) as f32, g: ch(c[1]) as f32, b: ch(c[2]) as f32, a: a as f32 }))
    }

    fn hsl(&mut self) -> Result<Option<Color>, Error> {
        let Some((c, a, legacy)) = self.components(true)? else { return Ok(None) };
        // 昔の書き方の彩度と明度は割合だけ
        if legacy && !(matches!(c[1], Comp::Pct(_)) && matches!(c[2], Comp::Pct(_))) {
            return Ok(None);
        }
        let h = match c[0] {
            Comp::Num(v) => v,
            Comp::NoneKw => 0.0,
            _ => return Ok(None),
        };
        let pct = |x: Comp| match x {
            Comp::Pct(p) => Some(math::clamp(p, 0.0, 100.0) / 100.0),
            Comp::Num(n) => Some(math::clamp(n, 0.0, 100.0) / 100.0),
            Comp::NoneKw => Some(0.0),
            Comp::None => None,
        };
        let (Some(s), Some(l)) = (pct(c[1]), pct(c[2])) else { return Ok(None) };
        let (r, g, b) = hsl_to_rgb(h, s, l);
        Ok(Some(Color { r: (r * 255.0) as f32, g: (g * 255.0) as f32, b: (b * 255.0) as f32, a: a as f32 }))
    }

    fn hwb(&mut self) -> Result<Option<Color>, Error> {
        let Some((c, a, _)) = self.components(false)? else { return Ok(None) };
        let h = match c[0] {
            Comp::Num(v) => v,
            Comp::NoneKw => 0.0,
            _ => return Ok(None),
        };
        let pct = |x: Comp| match x {
            Comp::Pct(p) => Some(math::clamp(p, 0.0, 100.0) / 100.0),
            Comp::Num(n) => Some(math::clamp(n, 0.0, 100.0) / 100.0),
            Comp::NoneKw => Some(0.0),
            Comp::None => None,
        };
        let (Some(w), Some(bk)) = (pct(c[1]), pct(c[2])) else { return Ok(None) };
        let (r, g, b) = if w + bk >= 1.0 {
            let gray = w / (w + bk);
            (gray, gray, gray)
        } else {
            let (r, g, b) = hsl_to_rgb(h, 1.0, 0.5);
            let k = 1.0 - w - bk;
            (r * k + w, g * k + w, b * k + w)
        };
        Ok(Some(Color { r: (r * 255.0) as f32, g: (g * 255.0) as f32, b: (b * 255.0) as f32, a: a as f32 }))
    }

    /// 塗り（`none`・色・`url(...)`［代わり］・`context-fill`・`context-stroke`）。
    pub(crate) fn paint(&mut self) -> Result<Option<Paint>, Error> {
        if self.eat_ident(b"none")? {
            return Ok(Some(Paint::None));
        }
        if self.eat_ident(b"context-fill")? {
            return Ok(Some(Paint::ContextFill));
        }
        if self.eat_ident(b"context-stroke")? {
            return Ok(Some(Paint::ContextStroke));
        }
        let t = self.peek()?;
        let url = match t.kind {
            Kind::Url => {
                self.next()?;
                Some((t.inner, t.escaped))
            }
            Kind::Function if ident_is(self.s, t.inner, t.escaped, b"url") => {
                self.next()?;
                let v = self.next()?;
                if v.kind != Kind::Str || !self.eat(Kind::RParen)? {
                    return Ok(None);
                }
                Some((v.inner, v.escaped))
            }
            _ => None,
        };
        if let Some((href, escaped)) = url {
            if self.at_end()? {
                return Ok(Some(Paint::Url { href, escaped, fallback: Fallback::Unset }));
            }
            let fb = if self.eat_ident(b"none")? {
                Fallback::None
            } else {
                match self.color()? {
                    Some(c) => Fallback::Color(c),
                    None => return Ok(None),
                }
            };
            return Ok(Some(Paint::Url { href, escaped, fallback: fb }));
        }
        Ok(self.color()?.map(Paint::Color))
    }

    /// 数の並び（`,` か空白で区切る。`viewBox`・`stroke-dasharray` など）。`f` に順に渡し、並べ終えた数を返す。
    pub(crate) fn number_list(&mut self, f: &mut dyn FnMut(f64) -> Result<(), Error>) -> Result<Option<usize>, Error> {
        let mut n = 0usize;
        loop {
            self.budget.spend(cost::OP_STEP)?;
            match self.number()? {
                Some(v) => {
                    f(v)?;
                    n = n.saturating_add(1);
                }
                None => return Ok(if self.at_end()? && n > 0 { Some(n) } else { None }),
            }
            if self.at_end()? {
                return Ok(Some(n));
            }
            let comma = self.eat(Kind::Comma)?;
            if comma && self.at_end()? {
                return Ok(None);
            }
        }
    }
}

/// 色の成分。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Comp {
    None,
    Num(f64),
    Pct(f64),
    NoneKw,
}

/// `#rgb`・`#rgba`・`#rrggbb`・`#rrggbbaa`（16 進は大文字小文字を問わない）。
fn hex(h: &[u8]) -> Option<Color> {
    let d = |c: u8| -> Option<u32> {
        match c {
            b'0'..=b'9' => Some(u32::from(c.saturating_sub(b'0'))),
            b'a'..=b'f' => Some(u32::from(c.saturating_sub(b'a')).saturating_add(10)),
            b'A'..=b'F' => Some(u32::from(c.saturating_sub(b'A')).saturating_add(10)),
            _ => None,
        }
    };
    let mut v = [0u32; 8];
    for (k, &c) in h.iter().enumerate() {
        *v.get_mut(k)? = d(c)?;
    }
    let two = |hi: u32, lo: u32| (hi.saturating_mul(16).saturating_add(lo)) as f32;
    let one = |x: u32| x.saturating_mul(17) as f32;
    Some(match h.len() {
        3 => Color { r: one(v[0]), g: one(v[1]), b: one(v[2]), a: 1.0 },
        4 => Color { r: one(v[0]), g: one(v[1]), b: one(v[2]), a: one(v[3]) / 255.0 },
        6 => Color { r: two(v[0], v[1]), g: two(v[2], v[3]), b: two(v[4], v[5]), a: 1.0 },
        8 => Color { r: two(v[0], v[1]), g: two(v[2], v[3]), b: two(v[4], v[5]), a: two(v[6], v[7]) / 255.0 },
        _ => return None,
    })
}

/// 名前の色（大文字小文字を問わない）。
fn named(name: &[u8]) -> Option<Color> {
    if name.len() > 24 || !name.is_ascii() {
        return None;
    }
    let mut buf = [0u8; 24];
    let lower = buf.get_mut(..name.len())?;
    for (d, &s) in lower.iter_mut().zip(name) {
        *d = if s.is_ascii_uppercase() { s.saturating_add(32) } else { s };
    }
    let lower: &[u8] = lower;
    let table = names::NAMED;
    let i = lower_bound(table, |(n, _)| *n < lower);
    table.get(i).filter(|(n, _)| *n == lower).map(|&(_, v)| Color::rgb24(v))
}

/// HSL（h は度、s・l は 0〜1）を sRGB の 0〜1 に（CSS Color 4 §7.1 の式）。
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let h = math::rem_euclid(h, 360.0);
    let f = |n: f64| {
        let k = math::rem_euclid(n + h / 30.0, 12.0);
        let a = s * math::min(l, 1.0 - l);
        l - a * math::max(-1.0, math::min(math::min(k - 3.0, 9.0 - k), 1.0))
    };
    (f(0.0), f(8.0), f(4.0))
}

/// `transform` を読む（`attr` なら SVG の属性の書き方も受ける。CSS のプロパティは CSS Transforms の書き方だけ）。em・ex・
/// % の長さは解けないので、文脈の要らない値だけを行列にする（`font_size` は em の大きさ。% は None）。
pub(crate) fn transform(
    s: &[u8],
    range: Span,
    attr: bool,
    font_size: f64,
    budget: &Budget<'_>,
) -> Result<Option<Transform>, Error> {
    let budget = budget_ref(budget);
    let mut v = Values::new(s, range, budget);
    let mut m = Transform::IDENTITY;
    if v.eat_ident(b"none")? {
        return Ok(if v.at_end()? { Some(m) } else { None });
    }
    let mut first = true;
    loop {
        budget.spend(cost::OP_STEP)?;
        if v.at_end()? {
            return Ok(if first { if attr { Some(m) } else { None } } else { Some(m) });
        }
        if !first && attr {
            v.eat(Kind::Comma)?;
        }
        let f = v.next()?;
        if f.kind != Kind::Function {
            return Ok(None);
        }
        let name = |w: &[u8]| ident_is(s, f.inner, f.escaped, w);
        let Some(t) = transform_fn(&mut v, &name, attr, font_size)? else { return Ok(None) };
        m = m.then(&t);
        if !m.finite() {
            return Ok(None);
        }
        first = false;
    }
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 変形の関数の引数（`(` を読んだあと。`)` まで）。
fn transform_fn(
    v: &mut Values<'_, '_>,
    name: &dyn Fn(&[u8]) -> bool,
    attr: bool,
    font_size: f64,
) -> Result<Option<Transform>, Error> {
    // 引数：数・長さ・角度を `,` で区切る（属性では `,` を省いてよい）
    #[derive(Clone, Copy)]
    enum Arg {
        Len,
        Ang,
        Num,
    }
    let (kinds, min, max): (&[Arg], usize, usize) = if name(b"matrix") {
        (&[Arg::Num; 6], 6, 6)
    } else if name(b"translate") {
        (&[Arg::Len, Arg::Len], 1, 2)
    } else if name(b"translatex") || name(b"translatey") {
        (&[Arg::Len], 1, 1)
    } else if name(b"scale") {
        (&[Arg::Num, Arg::Num], 1, 2)
    } else if name(b"scalex") || name(b"scaley") {
        (&[Arg::Num], 1, 1)
    } else if name(b"rotate") {
        if attr { (&[Arg::Ang, Arg::Num, Arg::Num], 1, 3) } else { (&[Arg::Ang], 1, 1) }
    } else if name(b"skew") {
        (&[Arg::Ang, Arg::Ang], 1, 2)
    } else if name(b"skewx") || name(b"skewy") {
        (&[Arg::Ang], 1, 1)
    } else {
        return Ok(None);
    };
    let mut vals = [0.0f64; 6];
    let mut n = 0usize;
    loop {
        if v.eat(Kind::RParen)? {
            break;
        }
        if n >= max {
            return Ok(None);
        }
        if n > 0 && !v.eat(Kind::Comma)? && !attr {
            return Ok(None);
        }
        let Some(&k) = kinds.get(n) else { return Ok(None) };
        let x = match k {
            Arg::Num => v.number()?,
            Arg::Ang => v.angle(attr)?,
            Arg::Len => match v.length(attr)? {
                Some(l) if l.unit == Unit::Em => Some(l.value * font_size),
                Some(l) => l.absolute_px(),
                None => None,
            },
        };
        let Some(x) = x else { return Ok(None) };
        if let Some(slot) = vals.get_mut(n) {
            *slot = x;
        }
        n = n.saturating_add(1);
    }
    if n < min || (name(b"rotate") && attr && n == 2) {
        return Ok(None);
    }
    let [a0, a1, a2, a3, a4, a5] = vals;
    let rad = |d: f64| d * math::DEG_TO_RAD;
    Ok(Some(if name(b"matrix") {
        Transform { a: a0, b: a1, c: a2, d: a3, e: a4, f: a5 }
    } else if name(b"translate") {
        Transform { e: a0, f: if n > 1 { a1 } else { 0.0 }, ..Transform::IDENTITY }
    } else if name(b"translatex") {
        Transform { e: a0, ..Transform::IDENTITY }
    } else if name(b"translatey") {
        Transform { f: a0, ..Transform::IDENTITY }
    } else if name(b"scale") {
        Transform { a: a0, d: if n > 1 { a1 } else { a0 }, ..Transform::IDENTITY }
    } else if name(b"scalex") {
        Transform { a: a0, ..Transform::IDENTITY }
    } else if name(b"scaley") {
        Transform { d: a0, ..Transform::IDENTITY }
    } else if name(b"rotate") {
        let (sn, cs) = (math::sin(rad(a0)), math::cos(rad(a0)));
        let r = Transform { a: cs, b: sn, c: -sn, d: cs, e: 0.0, f: 0.0 };
        if n == 3 {
            let to = Transform { e: a1, f: a2, ..Transform::IDENTITY };
            let back = Transform { e: -a1, f: -a2, ..Transform::IDENTITY };
            to.then(&r).then(&back)
        } else {
            r
        }
    } else if name(b"skew") {
        Transform { c: math::tan(rad(a0)), b: if n > 1 { math::tan(rad(a1)) } else { 0.0 }, ..Transform::IDENTITY }
    } else if name(b"skewx") {
        Transform { c: math::tan(rad(a0)), ..Transform::IDENTITY }
    } else {
        Transform { b: math::tan(rad(a0)), ..Transform::IDENTITY }
    }))
}

/// `viewBox`（4 つの数。幅と高さは負でない。0 は呼び手が扱う）。
pub(crate) fn view_box(s: &[u8], range: Span, budget: &Budget<'_>) -> Result<Option<ViewBox>, Error> {
    let mut v = Values::new(s, range, budget_ref(budget));
    let mut xs = [0.0f64; 4];
    let mut k = 0usize;
    let n = v.number_list(&mut |x| {
        if let Some(slot) = xs.get_mut(k) {
            *slot = x;
        }
        k = k.saturating_add(1);
        Ok(())
    })?;
    if n != Some(4) {
        return Ok(None);
    }
    let [x, y, w, h] = xs;
    Ok((w >= 0.0 && h >= 0.0).then_some(ViewBox { x, y, w, h }))
}

/// `preserveAspectRatio`（`defer` は読み飛ばす）。
pub(crate) fn aspect(s: &[u8], range: Span, budget: &Budget<'_>) -> Result<Option<Aspect>, Error> {
    let mut v = Values::new(s, range, budget_ref(budget));
    v.eat_ident(b"defer")?;
    let t = v.next()?;
    if t.kind != Kind::Ident || t.escaped {
        return Ok(None);
    }
    let align = match t.inner.of(s) {
        b"none" => Align::None,
        b"xMinYMin" => Align::XMinYMin,
        b"xMidYMin" => Align::XMidYMin,
        b"xMaxYMin" => Align::XMaxYMin,
        b"xMinYMid" => Align::XMinYMid,
        b"xMidYMid" => Align::XMidYMid,
        b"xMaxYMid" => Align::XMaxYMid,
        b"xMinYMax" => Align::XMinYMax,
        b"xMidYMax" => Align::XMidYMax,
        b"xMaxYMax" => Align::XMaxYMax,
        _ => return Ok(None),
    };
    let slice = if v.eat_ident(b"slice")? {
        true
    } else {
        v.eat_ident(b"meet")?;
        false
    };
    Ok(v.at_end()?.then_some(Aspect { align, slice }))
}

#[cfg(test)]
mod tests;
