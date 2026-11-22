//! 計算した値（CSS Cascade 4 §4.4・CSS Values 4・SVG 2 §13。architecture.md §4.2）。親の計算した値と要素の指定の値から、
//! 要素の計算した値を求める。木を作る段（M2）は行きがけの順に深さごとの計算した値を積み上げて使い、M1 は照合の道具
//! （`sumie-tools values`）と試験が文書の木をたどって使う（`doc::walk`）。
//!
//! - 受け継ぐプロパティは親の値から、受け継がないものは初期の値から始め、勝った宣言で上書きする。`inherit` は親の値、
//!   `initial` は初期の値、`unset` は受け継ぐなら親・ほかは初期の値
//! - 長さの文脈：`em`・`ex`・`ch`・`ic`・`cap`・`lh` は要素の文字の大きさと行の高さ（`font-size` と `line-height` 自身は親の）、
//!   `rem`・`rlh` は根の要素、`%` は最も近い表示の領域（viewBox があればその幅・高さ、なければ表示の領域の幅・高さ。
//!   `stroke-width` は対角を √2 で割ったもの）、`vw` などは文書の大きさ。解けない長さの宣言は書かなかったものとする
//! - `currentColor` は計算した値でも `currentColor` のまま受け継ぎ、使うときに要素の `color` で解く（CSS Color 4 §4.4。
//!   `color` の `currentColor` は `inherit`）
//! - 不透明度は 0〜1 に収める。`display: none` は要素ごとの印（部分木を描かないのは呼び手が決める）
//! - `transform`：属性は SVG の書き方、CSS の宣言は CSS の書き方で行列にし、`transform-origin` の点のまわりに掛ける。参照の
//!   箱は `transform-box`（初期の値は `view-box`：最も近い表示の領域の原点と幅・高さ）。`fill-box`・`stroke-box`（と、SVG の
//!   要素では同じ扱いの `content-box`・`border-box`）は外接矩形が要るので、単位の行列でない変形は「解けない」印を付ける
//!   （木を作る段で解く）

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::{Kind as Tk, ident_is};
use crate::error::Error;
use crate::math;
use crate::mem::Plain;
use crate::style::cascade::{
    ORIGIN_ATTR, SRC_SHEET, SRC_STORE, Specified, WIDE_INHERIT, WIDE_INITIAL, WIDE_NONE, WIDE_UNSET,
};
use crate::style::props::{PROPS, PropertyId, lookup};
use crate::values::{self, Color, ColorValue, Fallback, Length, Paint, Transform, Unit, Values};
use crate::xml::lex::Span;

/// 塗りの種類。
pub(crate) const PAINT_NONE: u8 = 0;
pub(crate) const PAINT_COLOR: u8 = 1;
pub(crate) const PAINT_URL: u8 = 2;
pub(crate) const PAINT_CONTEXT_FILL: u8 = 3;
pub(crate) const PAINT_CONTEXT_STROKE: u8 = 4;

/// url の代わり（書いていない・none・色）。
pub(crate) const FALLBACK_UNSET: u8 = 0;
pub(crate) const FALLBACK_NONE: u8 = 1;
pub(crate) const FALLBACK_COLOR: u8 = 2;

/// `visibility`。
pub(crate) const VISIBLE: u8 = 0;
pub(crate) const HIDDEN: u8 = 1;
pub(crate) const COLLAPSE: u8 = 2;

/// 計算した塗り。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CPaint {
    /// 色、または url の代わりの色（`current` なら使うときに要素の `color` で解く）
    pub(crate) color: Color,
    /// url の中身の範囲（`src` の置き場の中）
    pub(crate) href: Span,
    pub(crate) kind: u8,
    pub(crate) fallback: u8,
    pub(crate) src: u8,
    pub(crate) escaped: u8,
    pub(crate) current: u8,
    pub(crate) pad: [u8; 3],
}

impl Plain for CPaint {}

impl CPaint {
    pub(crate) const NONE: CPaint = CPaint {
        color: Color::TRANSPARENT,
        href: Span { start: 0, end: 0 },
        kind: PAINT_NONE,
        fallback: FALLBACK_UNSET,
        src: 0,
        escaped: 0,
        current: 0,
        pad: [0; 3],
    };
    pub(crate) const BLACK: CPaint = CPaint { color: Color::BLACK, kind: PAINT_COLOR, ..CPaint::NONE };

    /// 色の値から（`currentColor` は印にする）。
    fn of_color(c: ColorValue) -> CPaint {
        match c {
            ColorValue::Color(c) => CPaint { color: c, kind: PAINT_COLOR, ..CPaint::NONE },
            ColorValue::CurrentColor => CPaint { color: Color::BLACK, kind: PAINT_COLOR, current: 1, ..CPaint::NONE },
        }
    }

    fn of(p: Paint, src: u8) -> CPaint {
        match p {
            Paint::None => CPaint::NONE,
            Paint::Color(c) => CPaint::of_color(c),
            Paint::ContextFill => CPaint { kind: PAINT_CONTEXT_FILL, ..CPaint::NONE },
            Paint::ContextStroke => CPaint { kind: PAINT_CONTEXT_STROKE, ..CPaint::NONE },
            Paint::Url { href, escaped, fallback } => {
                let (fb, color, current) = match fallback {
                    Fallback::Unset => (FALLBACK_UNSET, Color::TRANSPARENT, 0),
                    Fallback::None => (FALLBACK_NONE, Color::TRANSPARENT, 0),
                    Fallback::Color(ColorValue::Color(c)) => (FALLBACK_COLOR, c, 0),
                    Fallback::Color(ColorValue::CurrentColor) => (FALLBACK_COLOR, Color::BLACK, 1),
                };
                CPaint {
                    color,
                    href,
                    kind: PAINT_URL,
                    fallback: fb,
                    src,
                    escaped: u8::from(escaped),
                    current,
                    pad: [0; 3],
                }
            }
        }
    }

    /// 使う色（`currentColor` を `color` で解く）。
    pub(crate) fn used_color(&self, color: Color) -> Color {
        if self.current != 0 { color } else { self.color }
    }
}

/// 計算した値（木を作る段で深さごとに積む）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Computed {
    pub(crate) fill: CPaint,
    pub(crate) stroke: CPaint,
    pub(crate) color: Color,
    pub(crate) fill_opacity: f64,
    pub(crate) stroke_opacity: f64,
    /// ユーザー単位（% は表示の領域で解いたもの）
    pub(crate) stroke_width: f64,
    pub(crate) opacity: f64,
    pub(crate) font_size: f64,
    /// 行の高さ（px。`lh_number` なら `lh_factor` × 文字の大きさで、子は子の文字の大きさで求め直す）
    pub(crate) line_height: f64,
    pub(crate) lh_factor: f64,
    /// `stroke-width` の割合（`sw_is_pct` のとき。計算した値は割合のまま受け継ぎ、使う要素の表示の領域で解く）
    pub(crate) sw_pct: f64,
    /// 要素の変形（`transform-origin` を含む。受け継がない）
    pub(crate) transform: Transform,
    /// `transform` の計算した値の行列（原点を掛ける前。`transform: inherit` が受け継ぐ）
    pub(crate) transform_raw: Transform,
    /// `transform-origin` の点（ユーザー単位。受け継がない。`transform-origin: inherit` が受け継ぐ）
    pub(crate) origin_x: f64,
    pub(crate) origin_y: f64,
    pub(crate) visibility: u8,
    pub(crate) display_none: u8,
    /// 変形が解けた（0 なら外接矩形が要る：木を作る段で解く）
    pub(crate) transform_known: u8,
    pub(crate) lh_number: u8,
    pub(crate) sw_is_pct: u8,
    /// `transform-box` が view-box（初期の値。ほかは外接矩形が要る）
    pub(crate) box_vb: u8,
    pub(crate) pad: [u8; 2],
}

impl Plain for Computed {}

/// `line-height: normal` の代わりの数（フォントがない。CSS Inline 3 の多くの実装と同じ 1.2）。
const NORMAL_LINE: f64 = 1.2;

impl Computed {
    /// 根の要素の親に当たる初期の値（`font_size` は既定の文字の大きさ）。
    pub(crate) fn initial(font_size: f64) -> Computed {
        Computed {
            fill: CPaint::BLACK,
            stroke: CPaint::NONE,
            color: Color::BLACK,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            stroke_width: 1.0,
            opacity: 1.0,
            font_size,
            line_height: NORMAL_LINE * font_size,
            lh_factor: NORMAL_LINE,
            sw_pct: 0.0,
            transform: Transform::IDENTITY,
            transform_raw: Transform::IDENTITY,
            origin_x: 0.0,
            origin_y: 0.0,
            visibility: VISIBLE,
            display_none: 0,
            transform_known: 1,
            lh_number: 1,
            sw_is_pct: 0,
            box_vb: 1,
            pad: [0; 2],
        }
    }
}

/// 計算に使うプロパティの番号（表を 1 度だけ引く）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PropIds {
    color: Option<PropertyId>,
    display: Option<PropertyId>,
    fill: Option<PropertyId>,
    fill_opacity: Option<PropertyId>,
    font_size: Option<PropertyId>,
    line_height: Option<PropertyId>,
    opacity: Option<PropertyId>,
    stroke: Option<PropertyId>,
    stroke_opacity: Option<PropertyId>,
    stroke_width: Option<PropertyId>,
    transform: Option<PropertyId>,
    transform_box: Option<PropertyId>,
    transform_origin: Option<PropertyId>,
    visibility: Option<PropertyId>,
}

impl PropIds {
    pub(crate) fn new() -> PropIds {
        PropIds {
            color: lookup(b"color", false),
            display: lookup(b"display", false),
            fill: lookup(b"fill", false),
            fill_opacity: lookup(b"fill-opacity", false),
            font_size: lookup(b"font-size", false),
            line_height: lookup(b"line-height", false),
            opacity: lookup(b"opacity", false),
            stroke: lookup(b"stroke", false),
            stroke_opacity: lookup(b"stroke-opacity", false),
            stroke_width: lookup(b"stroke-width", false),
            transform: lookup(b"transform", false),
            transform_box: lookup(b"transform-box", false),
            transform_origin: lookup(b"transform-origin", false),
            visibility: lookup(b"visibility", false),
        }
    }
}

/// 要素の指定の値のうち、計算に使うもの（並びを 1 度だけ流して拾う。払いながら。文書の 1 回目のレビュー）。
#[derive(Debug, Clone, Copy, Default)]
struct Picked {
    color: Option<Specified>,
    display: Option<Specified>,
    fill: Option<Specified>,
    fill_opacity: Option<Specified>,
    font_size: Option<Specified>,
    line_height: Option<Specified>,
    opacity: Option<Specified>,
    stroke: Option<Specified>,
    stroke_opacity: Option<Specified>,
    stroke_width: Option<Specified>,
    transform: Option<Specified>,
    transform_box: Option<Specified>,
    transform_origin: Option<Specified>,
    visibility: Option<Specified>,
}

fn pick(specs: &[Specified], ids: &PropIds, budget: &Budget<'_>) -> Result<Picked, Error> {
    let mut p = Picked::default();
    for &sp in specs {
        budget.spend(cost::OP_STEP)?;
        let id = Some(sp.prop);
        let slot = if id == ids.color {
            &mut p.color
        } else if id == ids.display {
            &mut p.display
        } else if id == ids.fill {
            &mut p.fill
        } else if id == ids.fill_opacity {
            &mut p.fill_opacity
        } else if id == ids.font_size {
            &mut p.font_size
        } else if id == ids.line_height {
            &mut p.line_height
        } else if id == ids.opacity {
            &mut p.opacity
        } else if id == ids.stroke {
            &mut p.stroke
        } else if id == ids.stroke_opacity {
            &mut p.stroke_opacity
        } else if id == ids.stroke_width {
            &mut p.stroke_width
        } else if id == ids.transform {
            &mut p.transform
        } else if id == ids.transform_box {
            &mut p.transform_box
        } else if id == ids.transform_origin {
            &mut p.transform_origin
        } else if id == ids.visibility {
            &mut p.visibility
        } else {
            continue;
        };
        *slot = Some(sp);
    }
    Ok(p)
}

/// 指定の値の置き場（入力・XML の置き場・CSS の置き場）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Bufs<'a> {
    pub(crate) input: &'a [u8],
    pub(crate) store: &'a [u8],
    pub(crate) sheet: &'a [u8],
}

impl<'a> Bufs<'a> {
    pub(crate) fn of(&self, src: u8) -> &'a [u8] {
        match src {
            SRC_SHEET => self.sheet,
            SRC_STORE => self.store,
            _ => self.input,
        }
    }
}

/// 長さの文脈の、要素の外から来るもの。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Base {
    /// 既定の文字の大きさ（`medium`。根の要素の親の文字の大きさ）
    pub(crate) default_font: f64,
    /// 根の要素の文字の大きさと行の高さ（`rem`・`rlh`。根の要素そのものでは既定の値）
    pub(crate) root_font: f64,
    pub(crate) root_line: f64,
    /// 最も近い表示の領域の幅・高さ（% の基準。決まらなければ None）
    pub(crate) viewport: Option<(f64, f64)>,
    /// 文書の大きさ（vw などの基準。決まらなければ None）
    pub(crate) doc: Option<(f64, f64)>,
}

/// % の向き。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis {
    X,
    Y,
    /// 対角を √2 で割ったもの（向きのない長さ）
    Diag,
}

/// 長さをユーザー単位に解く（`font`・`line` は文字の大きさと行の高さ）。解けなければ None。
pub(crate) fn resolve(l: Length, font: f64, line: f64, base: &Base, axis: Axis) -> Option<f64> {
    let pct = |p: f64| -> Option<f64> {
        let (w, h) = base.viewport?;
        let r = match axis {
            Axis::X => w,
            Axis::Y => h,
            Axis::Diag => math::hypot(w, h) / core::f64::consts::SQRT_2,
        };
        Some(p * r / 100.0)
    };
    let vw = |p: f64, pick: u8| -> Option<f64> {
        let (w, h) = base.doc?;
        let r = match pick {
            0 => w,
            1 => h,
            2 => math::min(w, h),
            _ => math::max(w, h),
        };
        Some(p * r / 100.0)
    };
    let v = l.value;
    let out = match l.unit {
        Unit::Em | Unit::Ic => Some(v * font),
        Unit::Ex | Unit::Ch => Some(v * font * 0.5),
        Unit::Cap => Some(v * font * 0.7),
        Unit::Lh => Some(v * line),
        Unit::Rlh => Some(v * base.root_line),
        Unit::Rem => Some(v * base.root_font),
        Unit::Percent => pct(v),
        Unit::Vw | Unit::Vi => vw(v, 0),
        Unit::Vh | Unit::Vb => vw(v, 1),
        Unit::Vmin => vw(v, 2),
        Unit::Vmax => vw(v, 3),
        _ => l.absolute_px(),
    };
    out.filter(|x| x.is_finite())
}

/// 要素の計算した値を求める（`specs` は要素の指定の値。プロパティの番号の順）。
pub(crate) fn compute(
    parent: &Computed,
    specs: &[Specified],
    ids: &PropIds,
    bufs: &Bufs<'_>,
    base: &Base,
    budget: &Budget<'_>,
) -> Result<Computed, Error> {
    let budget = budget_ref(budget);
    let initial = Computed::initial(base.default_font);
    // 受け継ぐものは親から、受け継がないものは初期の値から
    let mut c = Computed {
        opacity: initial.opacity,
        display_none: initial.display_none,
        transform: initial.transform,
        transform_raw: initial.transform_raw,
        origin_x: 0.0,
        origin_y: 0.0,
        transform_known: initial.transform_known,
        box_vb: initial.box_vb,
        ..*parent
    };
    let p = pick(specs, ids, budget)?;
    // 全体の言葉：Some(true) は親の値、Some(false) は初期の値、None は値を読む
    let wide = |s: &Specified| -> Option<bool> {
        let inherited = PROPS.get(usize::from(s.prop)).is_some_and(|p| p.inherited);
        match s.wide {
            WIDE_NONE => None,
            WIDE_INHERIT => Some(true),
            WIDE_INITIAL => Some(false),
            WIDE_UNSET => Some(inherited),
            _ => None,
        }
    };
    // 文字の大きさと行の高さ（ほかの長さが使うので先に）
    if let Some(s) = p.font_size.as_ref() {
        budget.spend(cost::OP_STEP)?;
        c.font_size = match wide(s) {
            Some(true) => parent.font_size,
            Some(false) => base.default_font,
            None => font_size(s, parent, bufs, base, budget)?.unwrap_or(parent.font_size),
        };
    }
    if let Some(s) = p.line_height.as_ref() {
        budget.spend(cost::OP_STEP)?;
        match wide(s) {
            Some(true) => {
                c.lh_number = parent.lh_number;
                c.lh_factor = parent.lh_factor;
                c.line_height = parent.line_height;
            }
            Some(false) => {
                c.lh_number = 1;
                c.lh_factor = NORMAL_LINE;
            }
            None => {
                if let Some((num, v)) = line_height(s, c.font_size, parent, bufs, base, budget)? {
                    c.lh_number = u8::from(num);
                    if num {
                        c.lh_factor = v;
                    } else {
                        c.line_height = v;
                    }
                }
            }
        }
    }
    if c.lh_number != 0 {
        c.line_height = c.lh_factor * c.font_size;
    }
    let font = c.font_size;
    let line = c.line_height;
    if let Some(s) = p.color.as_ref() {
        budget.spend(cost::OP_STEP)?;
        c.color = match wide(s) {
            Some(true) => parent.color,
            Some(false) => Color::BLACK,
            None => match Values::new(bufs.of(s.src), s.value, budget).color()? {
                Some(ColorValue::Color(x)) => x,
                // color の currentColor は inherit
                _ => parent.color,
            },
        };
    }
    for (sp, is_fill) in [(p.fill, true), (p.stroke, false)] {
        let Some(s) = sp.as_ref() else { continue };
        budget.spend(cost::OP_STEP)?;
        let init = if is_fill { CPaint::BLACK } else { CPaint::NONE };
        let from_parent = if is_fill { parent.fill } else { parent.stroke };
        let p = match wide(s) {
            Some(true) => from_parent,
            Some(false) => init,
            None => match Values::new(bufs.of(s.src), s.value, budget).paint()? {
                Some(p) => CPaint::of(p, s.src),
                None => from_parent,
            },
        };
        if is_fill {
            c.fill = p;
        } else {
            c.stroke = p;
        }
    }
    for (sp, which) in [(p.fill_opacity, 0u8), (p.stroke_opacity, 1), (p.opacity, 2)] {
        let Some(s) = sp.as_ref() else { continue };
        budget.spend(cost::OP_STEP)?;
        let from_parent = match which {
            0 => parent.fill_opacity,
            1 => parent.stroke_opacity,
            _ => parent.opacity,
        };
        let v = match wide(s) {
            Some(true) => Some(from_parent),
            Some(false) => Some(1.0),
            None => opacity(s, bufs, budget)?,
        };
        let Some(v) = v else { continue };
        match which {
            0 => c.fill_opacity = v,
            1 => c.stroke_opacity = v,
            _ => c.opacity = v,
        }
    }
    // stroke-width：割合は計算した値で割合のまま受け継ぎ、この要素の表示の領域で解く（文書の 1 回目のレビュー）
    let pct_width = |v: f64| resolve(Length::new(v, Unit::Percent), font, line, base, Axis::Diag);
    let mut from_parent = true;
    if let Some(s) = p.stroke_width.as_ref() {
        budget.spend(cost::OP_STEP)?;
        match wide(s) {
            Some(true) => {}
            Some(false) => {
                from_parent = false;
                c.stroke_width = 1.0;
                c.sw_is_pct = 0;
            }
            None => {
                let unitless = PROPS.get(usize::from(s.prop)).is_some_and(|p| p.unitless) || s.origin == ORIGIN_ATTR;
                let l = Values::new(bufs.of(s.src), s.value, budget).length(unitless)?.filter(|l| l.value >= 0.0);
                match l {
                    Some(l) if l.unit == Unit::Percent => {
                        if let Some(v) = pct_width(l.value) {
                            from_parent = false;
                            c.stroke_width = v;
                            c.sw_is_pct = 1;
                            c.sw_pct = l.value;
                        }
                    }
                    Some(l) => {
                        if let Some(v) = resolve(l, font, line, base, Axis::Diag) {
                            from_parent = false;
                            c.stroke_width = v;
                            c.sw_is_pct = 0;
                        }
                    }
                    None => {}
                }
            }
        }
    }
    if from_parent && parent.sw_is_pct != 0 {
        // 受け継いだ割合をこの要素の表示の領域で解き直す（解けなければ親の使った値）
        c.stroke_width = pct_width(parent.sw_pct).unwrap_or(parent.stroke_width);
    }
    if let Some(s) = p.visibility.as_ref() {
        budget.spend(cost::OP_STEP)?;
        c.visibility = match wide(s) {
            Some(true) => parent.visibility,
            Some(false) => VISIBLE,
            None => {
                let buf = bufs.of(s.src);
                let t = Values::new(buf, s.value, budget).next()?;
                let is = |w: &[u8]| t.kind == Tk::Ident && ident_is(buf, t.inner, t.escaped, w);
                if is(b"hidden") {
                    HIDDEN
                } else if is(b"collapse") {
                    COLLAPSE
                } else if is(b"visible") {
                    VISIBLE
                } else {
                    parent.visibility
                }
            }
        };
    }
    if let Some(s) = p.display.as_ref() {
        budget.spend(cost::OP_STEP)?;
        c.display_none = match wide(s) {
            Some(true) => parent.display_none,
            Some(false) => 0,
            None => u8::from(Values::new(bufs.of(s.src), s.value, budget).eat_ident(b"none")?),
        };
    }
    transform(&mut c, parent, &p, bufs, base, budget)?;
    Ok(c)
}

/// 文字の大きさの言葉の倍率（CSS Fonts 4 §2.5 の表。`medium` は既定の文字の大きさ）。
const FONT_SIZE_SCALE: &[(&[u8], f64)] = &[
    (b"xx-small", 3.0 / 5.0),
    (b"x-small", 3.0 / 4.0),
    (b"small", 8.0 / 9.0),
    (b"medium", 1.0),
    (b"large", 6.0 / 5.0),
    (b"x-large", 3.0 / 2.0),
    (b"xx-large", 2.0),
    (b"xxx-large", 3.0),
];

/// `larger`・`smaller` の倍率（CSS Fonts 4 §2.5 が許す 1.2）。
const FONT_SIZE_STEP: f64 = 1.2;

fn font_size(
    s: &Specified,
    parent: &Computed,
    bufs: &Bufs<'_>,
    base: &Base,
    budget: &Budget<'_>,
) -> Result<Option<f64>, Error> {
    let buf = bufs.of(s.src);
    let mut v = Values::new(buf, s.value, budget_ref(budget));
    let t = v.peek()?;
    if t.kind == Tk::Ident {
        let is = |w: &[u8]| ident_is(buf, t.inner, t.escaped, w);
        if is(b"larger") {
            return Ok(Some(parent.font_size * FONT_SIZE_STEP));
        }
        if is(b"smaller") {
            return Ok(Some(parent.font_size / FONT_SIZE_STEP));
        }
        // math は数式の外では親の大きさ
        if is(b"math") {
            return Ok(Some(parent.font_size));
        }
        return Ok(FONT_SIZE_SCALE.iter().find(|(w, _)| is(w)).map(|&(_, k)| k * base.default_font));
    }
    let unitless = s.origin == ORIGIN_ATTR;
    let Some(l) = v.length(unitless)? else { return Ok(None) };
    // em・% などは親の文字の大きさと行の高さに対して
    let out = match l.unit {
        Unit::Percent => Some(l.value * parent.font_size / 100.0),
        _ => resolve(l, parent.font_size, parent.line_height, base, Axis::Diag),
    };
    Ok(out.filter(|&x| x >= 0.0))
}

/// `line-height`：（数か, 値）。数は子が受け継いで子の文字の大きさに掛ける。
fn line_height(
    s: &Specified,
    font: f64,
    parent: &Computed,
    bufs: &Bufs<'_>,
    base: &Base,
    budget: &Budget<'_>,
) -> Result<Option<(bool, f64)>, Error> {
    let mut v = Values::new(bufs.of(s.src), s.value, budget_ref(budget));
    if v.eat_ident(b"normal")? {
        return Ok(Some((true, NORMAL_LINE)));
    }
    if let Some(n) = v.number()? {
        return Ok((n >= 0.0).then_some((true, n)));
    }
    let Some(l) = v.length(false)? else { return Ok(None) };
    let out = match l.unit {
        Unit::Percent => Some(l.value * font / 100.0),
        // lh は親の行の高さ
        Unit::Lh => Some(l.value * parent.line_height),
        _ => resolve(l, font, parent.line_height, base, Axis::Diag),
    };
    Ok(out.filter(|&x| x >= 0.0).map(|x| (false, x)))
}

/// 数か割合を 0〜1 に収める。
fn opacity(s: &Specified, bufs: &Bufs<'_>, budget: &Budget<'_>) -> Result<Option<f64>, Error> {
    let t = Values::new(bufs.of(s.src), s.value, budget_ref(budget)).next()?;
    let v = match t.kind {
        Tk::Number if t.finite => t.value,
        Tk::Percentage if t.finite => t.value / 100.0,
        _ => return Ok(None),
    };
    Ok(Some(math::clamp(v, 0.0, 1.0)))
}

/// `transform`・`transform-origin`・`transform-box` から要素の変形を求める。`transform: inherit` は親の原点を掛ける前の
/// 行列を受け継ぎ、この要素の原点を掛ける。`transform-box: inherit` は親の箱（文書の 1 回目のレビュー）。
fn transform(
    c: &mut Computed,
    parent: &Computed,
    p: &Picked,
    bufs: &Bufs<'_>,
    base: &Base,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    // 参照の箱（view-box のときだけ解ける。受け継がない）
    if let Some(b) = p.transform_box.as_ref() {
        budget.spend(cost::OP_STEP)?;
        c.box_vb = match b.wide {
            WIDE_INHERIT => parent.box_vb,
            WIDE_NONE => {
                let buf = bufs.of(b.src);
                let t = Values::new(buf, b.value, budget).next()?;
                u8::from(t.kind == Tk::Ident && ident_is(buf, t.inner, t.escaped, b"view-box"))
            }
            _ => 1,
        };
    }
    let view_box = c.box_vb != 0;
    let font = c.font_size;
    let line = c.line_height;
    // 変形の原点（受け継がない。inherit は親の点。解けなければ (0, 0) で、変形があれば「解けない」印）
    let mut origin_known = true;
    if let Some(o) = p.transform_origin.as_ref() {
        budget.spend(cost::OP_STEP)?;
        let (x, y) = match o.wide {
            WIDE_INHERIT => (parent.origin_x, parent.origin_y),
            WIDE_NONE => match origin(o, bufs, font, line, base, budget)? {
                Some(pt) => pt,
                None => {
                    origin_known = false;
                    (0.0, 0.0)
                }
            },
            _ => (0.0, 0.0),
        };
        c.origin_x = x;
        c.origin_y = y;
    }
    let Some(s) = p.transform.as_ref() else { return Ok(()) };
    budget.spend(cost::OP_STEP)?;
    let (m, mut known) = match s.wide {
        WIDE_INHERIT => (parent.transform_raw, parent.transform_known != 0),
        WIDE_INITIAL | WIDE_UNSET => (Transform::IDENTITY, true),
        _ if s.origin == ORIGIN_ATTR => match values::transform_attr(bufs.of(s.src), s.value, budget)? {
            Some(m) => (m, true),
            None => (Transform::IDENTITY, true),
        },
        _ => {
            let mut res = |l: Length, horizontal: bool| {
                let axis = if horizontal { Axis::X } else { Axis::Y };
                if l.unit == Unit::Percent && !view_box {
                    return None;
                }
                resolve(l, font, line, base, axis)
            };
            match values::transform_css(bufs.of(s.src), s.value, base.doc.is_some(), &mut res, budget)? {
                Some((m, ok)) => (m, ok),
                None => (Transform::IDENTITY, true),
            }
        }
    };
    if !m.finite() {
        c.transform = Transform::IDENTITY;
        c.transform_raw = Transform::IDENTITY;
        c.transform_known = 0;
        return Ok(());
    }
    c.transform_raw = m;
    if m == Transform::IDENTITY {
        c.transform = m;
        c.transform_known = u8::from(known);
        return Ok(());
    }
    if !view_box {
        known = false;
    }
    // 変形の原点
    if !origin_known {
        known = false;
    }
    let (ox, oy) = (c.origin_x, c.origin_y);
    let to = Transform { e: ox, f: oy, ..Transform::IDENTITY };
    let back = Transform { e: -ox, f: -oy, ..Transform::IDENTITY };
    let t = to.then(&m).then(&back);
    c.transform = if t.finite() { t } else { Transform::IDENTITY };
    c.transform_known = u8::from(known && t.finite());
    Ok(())
}

/// `transform-origin` の点（参照の箱は view-box：原点は 0, 0、大きさは最も近い表示の領域）。3 つめ（z）は使わない。
fn origin(
    s: &Specified,
    bufs: &Bufs<'_>,
    font: f64,
    line: f64,
    base: &Base,
    budget: &Budget<'_>,
) -> Result<Option<(f64, f64)>, Error> {
    let budget = budget_ref(budget);
    let mut v = Values::new(bufs.of(s.src), s.value, budget);
    let pct = |p: f64| Length::new(p, Unit::Percent);
    let unitless = s.origin == ORIGIN_ATTR;
    // 項：（向き：0 横の言葉・1 縦の言葉・2 center か長さ, 長さ）
    let mut items = [(2u8, pct(50.0)); 2];
    let mut n = 0usize;
    while n < 3 && !v.at_end()? {
        budget.spend(cost::OP_STEP)?;
        let item = if v.eat_ident(b"left")? {
            (0, pct(0.0))
        } else if v.eat_ident(b"right")? {
            (0, pct(100.0))
        } else if v.eat_ident(b"top")? {
            (1, pct(0.0))
        } else if v.eat_ident(b"bottom")? {
            (1, pct(100.0))
        } else if v.eat_ident(b"center")? {
            (2, pct(50.0))
        } else {
            match v.length(unitless)? {
                Some(l) => (2, l),
                None => return Ok(None),
            }
        };
        if let Some(x) = items.get_mut(n) {
            *x = item;
        }
        n = n.saturating_add(1);
    }
    let [a, b] = items;
    let (x, y) = match n {
        0 => return Ok(None),
        // 1 つなら、もう片方は center
        1 if a.0 == 1 => (pct(50.0), a.1),
        1 => (a.1, pct(50.0)),
        // 縦の言葉が先か、横の言葉が後なら入れ替える
        _ if a.0 == 1 || b.0 == 0 => (b.1, a.1),
        _ => (a.1, b.1),
    };
    Ok(resolve(x, font, line, base, Axis::X).zip(resolve(y, font, line, base, Axis::Y)))
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
