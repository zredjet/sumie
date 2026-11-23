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
    /// `transform` の計算した値の行列（原点を掛ける前。割合の平行移動は 0 にしたもの。`transform: inherit` が受け継ぐ）
    pub(crate) transform_raw: Transform,
    /// 割合の平行移動の係数（参照の箱の幅・高さ 1 あたり：[e の幅, e の高さ, f の幅, f の高さ]。受け継いだ要素が自分の
    /// 参照の箱で解く。CSS Transforms 1・Chrome と同じ。文書の 6 回目のレビュー）
    pub(crate) raw_coef: [f64; 4],
    /// `transform-origin` の使う点（ユーザー単位。受け継がない）
    pub(crate) origin_x: f64,
    pub(crate) origin_y: f64,
    /// `transform-origin` の計算した値（[横の長さ, 横の割合, 縦の長さ, 縦の割合]。割合のまま持ち、`transform-origin:
    /// inherit` が受け継いで自分の参照の箱で解く。文書の 6 回目のレビュー）
    pub(crate) origin_spec: [f64; 4],
    pub(crate) visibility: u8,
    pub(crate) display_none: u8,
    /// 変形が解けた（0 なら外接矩形が要る：木を作る段で解く）
    pub(crate) transform_known: u8,
    pub(crate) lh_number: u8,
    pub(crate) sw_is_pct: u8,
    /// `transform-box` が view-box（初期の値。ほかは外接矩形が要る）
    pub(crate) box_vb: u8,
    /// 線と塗りの細部
    pub(crate) line: LineStyle,
    /// 幾何のプロパティ（受け継がない）
    pub(crate) geo: Geo,
    /// M2 でまだ描かない効果の印（受け継がない。[`FX_CLIP`] など）
    pub(crate) fx: u8,
    /// `transform-origin` の計算した値の長さが解けた（`transform-origin: inherit` が受け継ぐ。文書の 5 回目のレビュー）
    pub(crate) origin_known: u8,
    /// `transform_raw` の長さが解けた（割合を除く。`transform: inherit` が受け継ぐ）
    pub(crate) raw_known: u8,
    /// `transform_raw` に割合の平行移動がある（`raw_coef` を使う）
    pub(crate) raw_pct: u8,
    /// CSS の箱のプロパティの、初期の値と同じ絵にならない宣言の効く要素の印（受け継がない。`props::BOX_ROOT` など。
    /// 木を作る段が根・どの要素・rect で NotYet にする。ADR-103）
    pub(crate) css_box: u8,
    pub(crate) pad: [u8; 5],
}

/// 効果の印：`clip-path` が url でも none でもない（基本の形など）・`filter` が url でも none でもない（フィルタの関数）・
/// `mask` が url でも none でもない・`mix-blend-mode` が normal でない・`isolation: isolate`。url の効果は参照の辺で見る
/// （参照先が正しいときだけ効く）。
pub(crate) const FX_CLIP: u8 = 1;
pub(crate) const FX_FILTER: u8 = 2;
pub(crate) const FX_MASK: u8 = 4;
pub(crate) const FX_BLEND: u8 = 8;
pub(crate) const FX_ISOLATE: u8 = 16;
/// CSS の個別の変形（`translate`・`rotate`・`scale`）か `offset-path` が none でない
pub(crate) const FX_CSS_TRANSFORM: u8 = 32;
/// `display: contents`
pub(crate) const FX_CONTENTS: u8 = 64;
/// `content-visibility: hidden`（入れ物の中身を描かない。NotYet でない）
pub(crate) const FX_CV_HIDDEN: u8 = 128;

/// 幾何のプロパティの欄（[`Geo::v`] の番号）。
pub(crate) const G_X: usize = 0;
pub(crate) const G_Y: usize = 1;
pub(crate) const G_WIDTH: usize = 2;
pub(crate) const G_HEIGHT: usize = 3;
pub(crate) const G_RX: usize = 4;
pub(crate) const G_RY: usize = 5;
pub(crate) const G_R: usize = 6;
pub(crate) const G_CX: usize = 7;
pub(crate) const G_CY: usize = 8;
const GEO_SLOTS: usize = 9;

/// 幾何のプロパティの計算した値（SVG 2 §10・ADR-66 (4)。受け継がない）。長さは px、割合は割合のまま（`pct` の印。使う
/// 要素の表示の領域で解く）、`auto` は印。`d` は宣言の場所（置き場と範囲。木を作る段が宣言ごとに 1 度だけ読む）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Geo {
    pub(crate) v: [f64; GEO_SLOTS],
    pub(crate) d: Span,
    /// 欄ごとの印（bit k が欄 k）
    pub(crate) pct: u16,
    pub(crate) auto: u16,
    /// `d` の置き場（[`DASH_NONE`] と同じ 0xFF ならなし）・`d` が属性（パスのデータ）か CSS（`path("…")`）か
    pub(crate) d_src: u8,
    pub(crate) d_attr: u8,
    pub(crate) pad: [u8; 2],
}

impl Plain for Geo {}

impl Geo {
    /// 初期の値（width・height・rx・ry は auto、ほかは 0、d は none）。
    pub(crate) const INITIAL: Geo = Geo {
        v: [0.0; GEO_SLOTS],
        d: Span { start: 0, end: 0 },
        pct: 0,
        auto: (1 << G_WIDTH) | (1 << G_HEIGHT) | (1 << G_RX) | (1 << G_RY),
        d_src: DASH_NONE,
        d_attr: 0,
        pad: [0; 2],
    };

    fn copy_slot(&mut self, from: &Geo, k: usize) {
        if let (Some(d), Some(f)) = (self.v.get_mut(k), from.v.get(k)) {
            *d = *f;
        }
        let bit = 1u16.checked_shl(k as u32).unwrap_or(0);
        self.pct = (self.pct & !bit) | (from.pct & bit);
        self.auto = (self.auto & !bit) | (from.auto & bit);
    }

    /// 欄 `k` の使う値（割合は `basis` に対して。`auto` は None）。
    pub(crate) fn used(&self, k: usize, basis: Option<f64>) -> Option<f64> {
        let bit = 1u16.checked_shl(k as u32).unwrap_or(0);
        if self.auto & bit != 0 {
            return None;
        }
        let v = self.v.get(k).copied().unwrap_or(0.0);
        if self.pct & bit != 0 {
            return basis.map(|b| v * b / 100.0).filter(|x| x.is_finite());
        }
        Some(v)
    }
}

/// 線と塗りの細部（M2。受け継ぐ：塗りの規則・端・継ぎ目・miterlimit・破線・位相・paint-order・shape-rendering。受け継がない：
/// vector-effect）。破線の並びは宣言の場所（置き場と範囲）だけを持ち、木を作る段と計画が解く（同じ宣言を多くの要素が使っても
/// 写しが膨らまない）。破線の位相は長さのまま（使う要素の文脈で解く）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LineStyle {
    pub(crate) miter_limit: f64,
    pub(crate) dash_offset: Length,
    /// 破線の宣言の範囲（`dash_src` の置き場の中。`dash_src` が [`DASH_NONE`] なら実線）
    pub(crate) dash: Span,
    pub(crate) dash_src: u8,
    /// 破線の宣言は単位のない長さを受ける（属性か SVG のプロパティ）
    pub(crate) dash_unitless: u8,
    /// 0 非ゼロ・1 偶奇
    pub(crate) fill_rule: u8,
    /// 0 butt・1 round・2 square
    pub(crate) cap: u8,
    /// 0 miter・1 round・2 bevel
    pub(crate) join: u8,
    /// 線を塗りより先に描く（paint-order）
    pub(crate) stroke_first: u8,
    /// 反エイリアスなし（crispEdges・optimizeSpeed。ADR-91 (4)）
    pub(crate) crisp: u8,
    /// vector-effect: non-scaling-stroke（受け継がない）
    pub(crate) non_scaling: u8,
}

impl Plain for LineStyle {}

/// 実線（破線の宣言がない）。
pub(crate) const DASH_NONE: u8 = 0xFF;

impl LineStyle {
    pub(crate) const INITIAL: LineStyle = LineStyle {
        miter_limit: 4.0,
        dash_offset: Length::new(0.0, Unit::None),
        dash: Span { start: 0, end: 0 },
        dash_src: DASH_NONE,
        dash_unitless: 0,
        fill_rule: 0,
        cap: 0,
        join: 0,
        stroke_first: 0,
        crisp: 0,
        non_scaling: 0,
    };
}

/// `transform-origin` の 50% 50%（CSS の初期の値・根の svg の既定）。
const ORIGIN_CENTER: [f64; 4] = [0.0, 50.0, 0.0, 50.0];

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
            // 呼び手の既定の文字の大きさが大きくても有限に（文書の 4 回目のレビュー）
            line_height: if (NORMAL_LINE * font_size).is_finite() { NORMAL_LINE * font_size } else { font_size },
            lh_factor: NORMAL_LINE,
            sw_pct: 0.0,
            transform: Transform::IDENTITY,
            transform_raw: Transform::IDENTITY,
            raw_coef: [0.0; 4],
            origin_x: 0.0,
            origin_y: 0.0,
            // 根の `transform-origin: inherit` は CSS の初期の値
            origin_spec: ORIGIN_CENTER,
            visibility: VISIBLE,
            display_none: 0,
            transform_known: 1,
            lh_number: 1,
            sw_is_pct: 0,
            box_vb: 1,
            line: LineStyle::INITIAL,
            geo: Geo::INITIAL,
            fx: 0,
            origin_known: 1,
            raw_known: 1,
            raw_pct: 0,
            css_box: 0,
            pad: [0; 5],
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
    fill_rule: Option<PropertyId>,
    linecap: Option<PropertyId>,
    linejoin: Option<PropertyId>,
    miterlimit: Option<PropertyId>,
    dasharray: Option<PropertyId>,
    dashoffset: Option<PropertyId>,
    paint_order: Option<PropertyId>,
    shape_rendering: Option<PropertyId>,
    vector_effect: Option<PropertyId>,
    geo: [Option<PropertyId>; GEO_SLOTS],
    d: Option<PropertyId>,
    clip_path: Option<PropertyId>,
    filter: Option<PropertyId>,
    mask: Option<PropertyId>,
    blend: Option<PropertyId>,
    isolation: Option<PropertyId>,
    /// 接頭辞つきの効果と `mask-image`（none でなければまだ描かない。ADR-101）
    prefixed: [Option<PropertyId>; 5],
    /// CSS の個別の変形と offset-path・offset（none でなければまだ描かない）
    css_transform: [Option<PropertyId>; 5],
    content_visibility: Option<PropertyId>,
}

/// 幾何の欄のプロパティの名前（[`G_X`] などの順）。
const GEO_NAMES: [&[u8]; GEO_SLOTS] = [b"x", b"y", b"width", b"height", b"rx", b"ry", b"r", b"cx", b"cy"];

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
            fill_rule: lookup(b"fill-rule", false),
            linecap: lookup(b"stroke-linecap", false),
            linejoin: lookup(b"stroke-linejoin", false),
            miterlimit: lookup(b"stroke-miterlimit", false),
            dasharray: lookup(b"stroke-dasharray", false),
            dashoffset: lookup(b"stroke-dashoffset", false),
            paint_order: lookup(b"paint-order", false),
            shape_rendering: lookup(b"shape-rendering", false),
            vector_effect: lookup(b"vector-effect", false),
            geo: GEO_NAMES.map(|n| lookup(n, false)),
            d: lookup(b"d", false),
            clip_path: lookup(b"clip-path", false),
            filter: lookup(b"filter", false),
            mask: lookup(b"mask", false),
            blend: lookup(b"mix-blend-mode", false),
            isolation: lookup(b"isolation", false),
            prefixed: [
                lookup(b"-webkit-clip-path", false),
                lookup(b"-webkit-filter", false),
                lookup(b"-webkit-mask", false),
                lookup(b"-webkit-mask-image", false),
                lookup(b"mask-image", false),
            ],
            css_transform: [
                lookup(b"translate", false),
                lookup(b"rotate", false),
                lookup(b"scale", false),
                lookup(b"offset-path", false),
                lookup(b"offset", false),
            ],
            content_visibility: lookup(b"content-visibility", false),
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
    fill_rule: Option<Specified>,
    linecap: Option<Specified>,
    linejoin: Option<Specified>,
    miterlimit: Option<Specified>,
    dasharray: Option<Specified>,
    dashoffset: Option<Specified>,
    paint_order: Option<Specified>,
    shape_rendering: Option<Specified>,
    vector_effect: Option<Specified>,
    geo: [Option<Specified>; GEO_SLOTS],
    d: Option<Specified>,
    clip_path: Option<Specified>,
    filter: Option<Specified>,
    mask: Option<Specified>,
    blend: Option<Specified>,
    isolation: Option<Specified>,
    prefixed: [Option<Specified>; 5],
    css_transform: [Option<Specified>; 5],
    content_visibility: Option<Specified>,
    /// CSS の箱のプロパティの、同じ絵にならない宣言の効く要素の印（`props::BOX_ROOT` など）と、`inherit` の印
    css_box: u8,
    css_box_inherit: u8,
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
        } else if id == ids.fill_rule {
            &mut p.fill_rule
        } else if id == ids.linecap {
            &mut p.linecap
        } else if id == ids.linejoin {
            &mut p.linejoin
        } else if id == ids.miterlimit {
            &mut p.miterlimit
        } else if id == ids.dasharray {
            &mut p.dasharray
        } else if id == ids.dashoffset {
            &mut p.dashoffset
        } else if id == ids.paint_order {
            &mut p.paint_order
        } else if id == ids.shape_rendering {
            &mut p.shape_rendering
        } else if id == ids.vector_effect {
            &mut p.vector_effect
        } else if id == ids.d {
            &mut p.d
        } else if id == ids.clip_path {
            &mut p.clip_path
        } else if id == ids.filter {
            &mut p.filter
        } else if id == ids.mask {
            &mut p.mask
        } else if id == ids.blend {
            &mut p.blend
        } else if id == ids.isolation {
            &mut p.isolation
        } else if let Some(k) = ids.prefixed.iter().position(|g| id == *g) {
            match p.prefixed.get_mut(k) {
                Some(x) => x,
                None => continue,
            }
        } else if id == ids.content_visibility {
            &mut p.content_visibility
        } else if let Some(k) = ids.css_transform.iter().position(|g| id == *g) {
            match p.css_transform.get_mut(k) {
                Some(x) => x,
                None => continue,
            }
        } else if let Some(k) = ids.geo.iter().position(|g| id == *g) {
            match p.geo.get_mut(k) {
                Some(x) => x,
                None => continue,
            }
        } else {
            // CSS の箱のプロパティ：宣言の印（カスケードが宣言ごとに 1 度だけ求めたもの）を効く要素ごとに集める
            if let Some(crate::style::props::Kind::CssBox(rule)) = PROPS.get(usize::from(sp.prop)).map(|q| q.kind) {
                if sp.pad[0] & crate::style::cascade::BOX_ON != 0 {
                    p.css_box |= rule.scope();
                } else if sp.wide == WIDE_INHERIT {
                    p.css_box_inherit |= rule.scope();
                }
            }
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
    /// 根の要素を求めるか。根の svg には UA の規則の `transform-origin: 0 0` が当たらず、既定は 50% 50%。参照の箱は
    /// `transform-box` に依らず表示の領域の箱（CSS の箱を持つ要素の border-box。Chrome と同じ。ADR-85 (1)）
    pub(crate) outermost: bool,
    /// 大きさを中身で決める文書の根の要素を求めるか。変形の原点の既定（書いていないか初期の値）は 0 0（usvg と同じ。大きさが
    /// 変形に依るので表示の領域の真ん中は決まらない。書いた % と言葉は表示の領域（書いた幅・高さ、なければ 100）で解く。
    /// ADR-101 (1)・ADR-102）
    pub(crate) content_sized: bool,
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
        raw_coef: [0.0; 4],
        origin_x: 0.0,
        origin_y: 0.0,
        origin_spec: [0.0; 4],
        origin_known: 1,
        raw_known: 1,
        raw_pct: 0,
        transform_known: initial.transform_known,
        box_vb: initial.box_vb,
        line: LineStyle { non_scaling: 0, ..parent.line },
        geo: Geo::INITIAL,
        fx: 0,
        css_box: 0,
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
        // 数の行の高さは文字の大きさに掛ける（あふれたら親の行の高さのまま）
        let lh = c.lh_factor * c.font_size;
        if lh.is_finite() {
            c.line_height = lh;
        } else {
            c.lh_number = 0;
            c.line_height = parent.line_height;
        }
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
    line_style(&mut c, parent, &p, bufs, &wide, budget)?;
    geometry(&mut c, parent, &p, bufs, base, &wide, budget)?;
    effects(&mut c, parent, &p, bufs, &wide, base.outermost, budget)?;
    // CSS の箱のプロパティ（受け継がない。`inherit` は親の印）
    c.css_box = p.css_box | (parent.css_box & p.css_box_inherit);
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
        // display: contents は要素の箱（変形・位置・効果）を捨てて子を描く（CSS Display 3 §2.8）。まだ描かない（NotYet。
        // 黙って箱を残して描かない。単位 3 の 4 回目のレビュー）
        let contents = match wide(s) {
            Some(true) => parent.fx & FX_CONTENTS != 0,
            Some(false) => false,
            None => Values::new(bufs.of(s.src), s.value, budget).eat_ident(b"contents")?,
        };
        if contents {
            c.fx |= FX_CONTENTS;
        }
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
            return Ok(Some(parent.font_size * FONT_SIZE_STEP).filter(|x| x.is_finite()));
        }
        if is(b"smaller") {
            return Ok(Some(parent.font_size / FONT_SIZE_STEP).filter(|x| x.is_finite()));
        }
        // math は数式の外では親の大きさ
        if is(b"math") {
            return Ok(Some(parent.font_size));
        }
        return Ok(FONT_SIZE_SCALE
            .iter()
            .find(|(w, _)| is(w))
            .map(|&(_, k)| k * base.default_font)
            .filter(|x| x.is_finite()));
    }
    let unitless = s.origin == ORIGIN_ATTR;
    let Some(l) = v.length(unitless)? else { return Ok(None) };
    // em・% などは親の文字の大きさと行の高さに対して
    let out = match l.unit {
        Unit::Percent => Some(l.value * parent.font_size / 100.0),
        _ => resolve(l, parent.font_size, parent.line_height, base, Axis::Diag),
    };
    // 有限でない大きさは書かなかったものとする（NaN・±∞ を値に入れない。文書の 3 回目のレビュー）
    Ok(out.filter(|&x| x >= 0.0 && x.is_finite()))
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
        Unit::Percent => Some(l.value * font / 100.0).filter(|x| x.is_finite()),
        // lh は親の行の高さ
        Unit::Lh => Some(l.value * parent.line_height).filter(|x| x.is_finite()),
        _ => resolve(l, font, parent.line_height, base, Axis::Diag),
    };
    Ok(out.filter(|&x| x >= 0.0).map(|x| (false, x)))
}

/// 線と塗りの細部（[`LineStyle`]）を求める。
fn line_style(
    c: &mut Computed,
    parent: &Computed,
    p: &Picked,
    bufs: &Bufs<'_>,
    wide: &dyn Fn(&Specified) -> Option<bool>,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    let init = LineStyle::INITIAL;
    let pl = parent.line;
    // 言葉の並びから番号（知らない言葉・壊れた値は None で親の値）
    let keyword = |s: &Specified, words: &[&[u8]]| -> Result<Option<u8>, Error> {
        let buf = bufs.of(s.src);
        let mut v = Values::new(buf, s.value, budget);
        let t = v.next()?;
        if t.kind != Tk::Ident || !v.at_end()? {
            return Ok(None);
        }
        Ok(words.iter().position(|w| ident_is(buf, t.inner, t.escaped, w)).and_then(|i| u8::try_from(i).ok()))
    };
    let mut kw = |sp: Option<Specified>,
                  words: &[&[u8]],
                  get: fn(&LineStyle) -> u8,
                  set: fn(&mut LineStyle, u8)|
     -> Result<(), Error> {
        let Some(s) = sp else { return Ok(()) };
        budget.spend(cost::OP_STEP)?;
        let v = match wide(&s) {
            Some(true) => get(&pl),
            Some(false) => get(&init),
            None => keyword(&s, words)?.unwrap_or(get(&pl)),
        };
        set(&mut c.line, v);
        Ok(())
    };
    kw(p.fill_rule, &[b"nonzero", b"evenodd"], |l| l.fill_rule, |l, v| l.fill_rule = v)?;
    kw(p.linecap, &[b"butt", b"round", b"square"], |l| l.cap, |l, v| l.cap = v)?;
    kw(p.linejoin, &[b"miter", b"round", b"bevel"], |l| l.join, |l, v| l.join = v)?;
    // shape-rendering：crispEdges と optimizeSpeed は反エイリアスなし（ADR-91 (4)）
    kw(
        p.shape_rendering,
        &[b"auto", b"geometricPrecision", b"crispEdges", b"optimizeSpeed"],
        |l| l.crisp,
        |l, v| l.crisp = u8::from(v >= 2),
    )?;
    if let Some(s) = p.vector_effect.as_ref() {
        budget.spend(cost::OP_STEP)?;
        c.line.non_scaling = match wide(s) {
            Some(true) => pl.non_scaling,
            Some(false) => 0,
            None => u8::from(keyword(s, &[b"none", b"non-scaling-stroke"])? == Some(1)),
        };
    }
    if let Some(s) = p.miterlimit.as_ref() {
        budget.spend(cost::OP_STEP)?;
        c.line.miter_limit = match wide(s) {
            Some(true) => pl.miter_limit,
            Some(false) => init.miter_limit,
            None => {
                let t = Values::new(bufs.of(s.src), s.value, budget).next()?;
                if t.kind == Tk::Number && t.finite && t.value >= 0.0 { t.value } else { pl.miter_limit }
            }
        };
    }
    if let Some(s) = p.dasharray.as_ref() {
        budget.spend(cost::OP_STEP)?;
        match wide(s) {
            Some(true) => {
                c.line.dash = pl.dash;
                c.line.dash_src = pl.dash_src;
                c.line.dash_unitless = pl.dash_unitless;
            }
            Some(false) => c.line.dash_src = DASH_NONE,
            None => {
                // 値は読まずに宣言の場所だけを持つ（`none` も。木を作る段が宣言ごとに 1 度だけ読み、none と読めない値は
                // 実線にする。展開した要素ごとに字句を読まない：単位 3 のレビュー）
                c.line.dash = s.value;
                c.line.dash_src = s.src;
                let unitless = PROPS.get(usize::from(s.prop)).is_some_and(|p| p.unitless) || s.origin == ORIGIN_ATTR;
                c.line.dash_unitless = u8::from(unitless);
            }
        }
    }
    if let Some(s) = p.dashoffset.as_ref() {
        budget.spend(cost::OP_STEP)?;
        c.line.dash_offset = match wide(s) {
            Some(true) => pl.dash_offset,
            Some(false) => init.dash_offset,
            None => {
                let unitless = PROPS.get(usize::from(s.prop)).is_some_and(|p| p.unitless) || s.origin == ORIGIN_ATTR;
                Values::new(bufs.of(s.src), s.value, budget).length(unitless)?.unwrap_or(pl.dash_offset)
            }
        };
    }
    if let Some(s) = p.paint_order.as_ref() {
        budget.spend(cost::OP_STEP)?;
        c.line.stroke_first = match wide(s) {
            Some(true) => pl.stroke_first,
            Some(false) => 0,
            None => paint_order_stroke_first(s, bufs, budget)?.unwrap_or(pl.stroke_first),
        };
    }
    Ok(())
}

/// `paint-order` の値で線が塗りより先か（`normal` は偽。書いていない言葉は塗り・線・marker の順で後ろに足す。壊れた値は None）。
fn paint_order_stroke_first(s: &Specified, bufs: &Bufs<'_>, budget: &Budget<'_>) -> Result<Option<u8>, Error> {
    let budget = budget_ref(budget);
    let buf = bufs.of(s.src);
    let mut v = Values::new(buf, s.value, budget);
    let mut first: Option<u8> = None;
    let mut any = false;
    while !v.at_end()? {
        budget.spend(cost::OP_STEP)?;
        let t = v.next()?;
        if t.kind != Tk::Ident {
            return Ok(None);
        }
        let is = |w: &[u8]| ident_is(buf, t.inner, t.escaped, w);
        any = true;
        if is(b"normal") {
            return Ok(Some(0));
        } else if is(b"stroke") {
            first.get_or_insert(1);
        } else if is(b"fill") {
            first.get_or_insert(0);
        } else if !is(b"markers") {
            return Ok(None);
        }
    }
    if !any {
        return Ok(None);
    }
    Ok(Some(first.unwrap_or(0)))
}

/// 幾何のプロパティ（[`Geo`]）を求める。壊れた値・負の大きさと半径・解けない長さの宣言は初期の値（受け継がないので
/// 書かなかったのと同じ。SVG 2 §10.2・ADR-90 (3)）。
fn geometry(
    c: &mut Computed,
    parent: &Computed,
    p: &Picked,
    bufs: &Bufs<'_>,
    base: &Base,
    wide: &dyn Fn(&Specified) -> Option<bool>,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    let (font, line) = (c.font_size, c.line_height);
    for (k, sp) in p.geo.iter().enumerate() {
        let Some(s) = sp.as_ref() else { continue };
        budget.spend(cost::OP_STEP)?;
        match wide(s) {
            Some(true) => c.geo.copy_slot(&parent.geo, k),
            Some(false) => {}
            None => {
                let buf = bufs.of(s.src);
                let bit = 1u16.checked_shl(k as u32).unwrap_or(0);
                let can_auto = matches!(k, G_WIDTH | G_HEIGHT | G_RX | G_RY);
                let mut v = Values::new(buf, s.value, budget);
                let sizing = matches!(k, G_WIDTH | G_HEIGHT) && s.origin != ORIGIN_ATTR;
                let mut auto = can_auto && v.eat_ident(b"auto")?;
                if !auto && sizing {
                    budget.spend(cost::OP_STEP)?;
                    let cx = crate::style::valid::Ctx { attr: false, unitless: false, viewport: true };
                    auto = crate::style::valid::size_function(&mut v, cx)?;
                }
                for w in crate::style::props::SIZE_KEYWORDS {
                    if auto || !sizing {
                        break;
                    }
                    budget.spend(cost::OP_STEP)?;
                    auto = v.eat_ident(w)?;
                }
                if auto {
                    if v.at_end()? {
                        c.geo.auto |= bit;
                    }
                    continue;
                }
                let unitless = PROPS.get(usize::from(s.prop)).is_some_and(|p| p.unitless) || s.origin == ORIGIN_ATTR;
                let Some(l) = v.length(unitless)? else { continue };
                if !v.at_end()? {
                    continue;
                }
                let non_neg = matches!(k, G_WIDTH | G_HEIGHT | G_RX | G_RY | G_R);
                if non_neg && l.value < 0.0 {
                    continue;
                }
                let val = if l.unit == Unit::Percent {
                    Some((l.value, true))
                } else {
                    resolve(l, font, line, base, Axis::X).map(|x| (x, false))
                };
                let Some((x, is_pct)) = val else { continue };
                if let Some(d) = c.geo.v.get_mut(k) {
                    *d = x;
                }
                c.geo.auto &= !bit;
                if is_pct {
                    c.geo.pct |= bit;
                } else {
                    c.geo.pct &= !bit;
                }
            }
        }
    }
    if let Some(s) = p.d.as_ref() {
        budget.spend(cost::OP_STEP)?;
        match wide(s) {
            Some(true) => {
                c.geo.d = parent.geo.d;
                c.geo.d_src = parent.geo.d_src;
                c.geo.d_attr = parent.geo.d_attr;
            }
            Some(false) => c.geo.d_src = DASH_NONE,
            None => {
                // 値は読まずに宣言の場所だけを持つ（CSS の `none` は木を作る段が path() でないとして何も描かない）
                c.geo.d = s.value;
                c.geo.d_src = s.src;
                c.geo.d_attr = u8::from(s.origin == ORIGIN_ATTR);
            }
        }
    }
    Ok(())
}

/// M2 でまだ描かない効果の印（[`FX_CLIP`] など）を求める。
#[allow(clippy::too_many_arguments, reason = "根かを足した")]
fn effects(
    c: &mut Computed,
    parent: &Computed,
    p: &Picked,
    bufs: &Bufs<'_>,
    wide: &dyn Fn(&Specified) -> Option<bool>,
    root: bool,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    // 値が none だけか
    let none_only = |s: &Specified| -> Result<bool, Error> {
        let buf = bufs.of(s.src);
        let mut v = Values::new(buf, s.value, budget);
        let t = v.next()?;
        Ok(t.kind == Tk::Ident && ident_is(buf, t.inner, t.escaped, b"none") && v.at_end()?)
    };
    // 値が none（か normal・auto）でも url だけでもないか
    let other = |s: &Specified, keep: &[u8]| -> Result<bool, Error> {
        let buf = bufs.of(s.src);
        let mut v = Values::new(buf, s.value, budget);
        let t = v.next()?;
        if t.kind == Tk::Ident && ident_is(buf, t.inner, t.escaped, keep) && v.at_end()? {
            return Ok(false);
        }
        if t.kind == Tk::Url && v.at_end()? {
            return Ok(false);
        }
        if t.kind == Tk::Function && ident_is(buf, t.inner, t.escaped, b"url") {
            let a = v.next()?;
            let b = v.next()?;
            if a.kind == Tk::Str && b.kind == Tk::RParen && v.at_end()? {
                return Ok(false);
            }
        }
        Ok(true)
    };
    for (sp, bit, keep) in [
        (p.clip_path, FX_CLIP, b"none".as_slice()),
        (p.filter, FX_FILTER, b"none"),
        (p.mask, FX_MASK, b"none"),
        (p.blend, FX_BLEND, b"normal"),
        (p.isolation, FX_ISOLATE, b"auto"),
    ] {
        let Some(s) = sp.as_ref() else { continue };
        budget.spend(cost::OP_STEP)?;
        let on = match wide(s) {
            Some(true) => parent.fx & bit != 0,
            Some(false) => false,
            // 根の mask は CSS の箱の mask（url が mask の要素を指さなければ、Chrome は何も描かない）。none でなければ
            // まだ描かない（CSS の洗い出し。ADR-103）
            None if root && bit == FX_MASK => !none_only(s)?,
            None => other(s, keep)?,
        };
        if on {
            c.fx |= bit;
        }
    }
    for (k, sp) in p.prefixed.iter().enumerate() {
        let Some(s) = sp.as_ref() else { continue };
        budget.spend(cost::OP_STEP)?;
        let bit = match k {
            0 => FX_CLIP,
            1 => FX_FILTER,
            _ => FX_MASK,
        };
        let on = match wide(s) {
            Some(true) => parent.fx & bit != 0,
            Some(false) => false,
            None => !none_only(s)?,
        };
        if on {
            c.fx |= bit;
        }
    }
    // content-visibility: hidden は入れ物の中身を描かない（受け継がない。Chrome と同じ。単位 3 の 5 回目のレビュー）
    if let Some(s) = p.content_visibility.as_ref() {
        budget.spend(cost::OP_STEP)?;
        let hidden = match wide(s) {
            Some(true) => parent.fx & FX_CV_HIDDEN != 0,
            Some(false) => false,
            None => Values::new(bufs.of(s.src), s.value, budget).eat_ident(b"hidden")?,
        };
        if hidden {
            c.fx |= FX_CV_HIDDEN;
        }
    }
    // `offset` のまとめが道を持つか（関数・url・箱の言葉。位置・anchor・回転・距離だけなら道がなく、Chrome でも動かない。
    // 単位 3 の 6 回目のレビュー）
    let has_path = |s: &Specified| -> Result<bool, Error> {
        const BOXES: &[&[u8]] =
            &[b"content-box", b"padding-box", b"border-box", b"margin-box", b"fill-box", b"stroke-box", b"view-box"];
        let buf = bufs.of(s.src);
        let mut v = Values::new(buf, s.value, budget);
        loop {
            budget.spend(cost::OP_STEP)?;
            let t = v.next()?;
            match t.kind {
                Tk::Eof => return Ok(false),
                Tk::Function | Tk::Url => return Ok(true),
                Tk::Ident if BOXES.iter().any(|w| ident_is(buf, t.inner, t.escaped, w)) => return Ok(true),
                _ => {}
            }
        }
    };
    // CSS の個別の変形と offset-path・offset：none でなければ（offset は道があれば）まだ描かない（黙って変形なしで描かない。
    // 単位 3 の 4 回目のレビュー）
    for (k, sp) in p.css_transform.iter().enumerate() {
        let Some(sp) = sp.as_ref() else { continue };
        budget.spend(cost::OP_STEP)?;
        let on = match wide(sp) {
            Some(true) => parent.fx & FX_CSS_TRANSFORM != 0,
            Some(false) => false,
            None if k == 4 => has_path(sp)?,
            None => !none_only(sp)?,
        };
        if on {
            c.fx |= FX_CSS_TRANSFORM;
        }
    }
    Ok(())
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

/// `transform`・`transform-origin`・`transform-box` から要素の変形を求める。`transform: inherit` は親の計算した値（原点を
/// 掛ける前の行列と割合の平行移動の係数）を、`transform-origin: inherit` は親の計算した値（長さと割合）を受け継ぎ、割合は
/// この要素の参照の箱で解いてこの要素の原点を掛ける（ADR-87 (2)）。`transform-box: inherit` は親の箱（文書の 1 回目の
/// レビュー）。
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
    let view_box = c.box_vb != 0 || base.outermost;
    let font = c.font_size;
    let line = c.line_height;
    // 変形の原点の計算した値（長さと割合。受け継がない。inherit は親の計算した値で、割合はこの要素の参照の箱で解く。
    // CSS Transforms 1・Chrome と同じ。文書の 6 回目のレビュー）。既定は UA の規則の 0 0、根の svg は 50% 50%（ADR-85 (1)）
    let initial_origin = if base.content_sized { [0.0; 4] } else { ORIGIN_CENTER };
    let mut og = if base.outermost { initial_origin } else { [0.0; 4] };
    let mut og_known = true;
    if let Some(o) = p.transform_origin.as_ref() {
        budget.spend(cost::OP_STEP)?;
        match o.wide {
            // 根の inherit は初期の値（ADR-87 (2)。中身で大きさを決める文書の根は 0 0。単位 3 の 5 回目のレビュー）
            WIDE_INHERIT if base.outermost => og = initial_origin,
            WIDE_INHERIT => {
                og = parent.origin_spec;
                og_known = parent.origin_known != 0;
            }
            WIDE_NONE => match origin(o, bufs, font, line, base, budget)? {
                Some(v) => og = v,
                None => {
                    og = [0.0; 4];
                    og_known = false;
                }
            },
            // initial・unset は CSS の初期の値 50% 50%（SVG の要素の 0 0 は UA の規則の値で、initial はそれを上書きする。
            // Chrome と同じ。文書の 4 回目のレビュー）。大きさを中身で決める文書の根は 0 0（usvg と同じ）
            _ => og = initial_origin,
        }
    }
    c.origin_spec = og;
    c.origin_known = u8::from(og_known);
    // 使う点（割合は参照の箱で解く。view-box は最も近い表示の領域。解けなければ (0, 0) で、変形があれば「解けない」印）
    let (ox, oy, origin_known) = match place(og, base.viewport) {
        Some((x, y)) => (x, y, og_known),
        None => (0.0, 0.0, false),
    };
    c.origin_x = ox;
    c.origin_y = oy;
    let Some(s) = p.transform.as_ref() else { return Ok(()) };
    budget.spend(cost::OP_STEP)?;
    // 計算した値：原点を掛ける前の行列と、割合の平行移動の係数（受け継いだ要素が自分の参照の箱で解く）
    let mut raw = (Transform::IDENTITY, [0.0; 4], true, false);
    let (m, mut known) = match s.wide {
        WIDE_INHERIT => {
            raw = (parent.transform_raw, parent.raw_coef, parent.raw_known != 0, parent.raw_pct != 0);
            let (rm, coef, rk, pct) = raw;
            match base.viewport {
                _ if !pct => (rm, rk),
                Some((w, h)) if view_box => {
                    let e = rm.e + coef[0] * w + coef[1] * h;
                    let f = rm.f + coef[2] * w + coef[3] * h;
                    (Transform { e, f, ..rm }, rk)
                }
                _ => (rm, false),
            }
        }
        WIDE_INITIAL | WIDE_UNSET => (Transform::IDENTITY, true),
        _ if s.origin == ORIGIN_ATTR => {
            let m = values::transform_attr(bufs.of(s.src), s.value, budget)?.unwrap_or(Transform::IDENTITY);
            raw.0 = m;
            (m, true)
        }
        _ => {
            let mut saw_pct = false;
            let mut res = |l: Length, horizontal: bool| {
                let axis = if horizontal { Axis::X } else { Axis::Y };
                if l.unit == Unit::Percent {
                    saw_pct = true;
                    if !view_box {
                        return None;
                    }
                }
                resolve(l, font, line, base, axis)
            };
            let (m, ok) = values::transform_css(bufs.of(s.src), s.value, base.doc.is_some(), &mut res, budget)?
                .unwrap_or((Transform::IDENTITY, true));
            raw = (m, [0.0; 4], ok, false);
            if saw_pct {
                // 割合を 0・幅 1・高さ 1 で解いた行列から係数を求める
                let eval = |wx: f64, hy: f64| {
                    let mut r = |l: Length, horizontal: bool| {
                        if l.unit == Unit::Percent {
                            let k = if horizontal { wx } else { hy };
                            return Some(l.value * k / 100.0);
                        }
                        resolve(l, font, line, base, if horizontal { Axis::X } else { Axis::Y })
                    };
                    values::transform_css(bufs.of(s.src), s.value, base.doc.is_some(), &mut r, budget)
                };
                if let (Some((m0, ok0)), Some((mx, _)), Some((my, _))) =
                    (eval(0.0, 0.0)?, eval(1.0, 0.0)?, eval(0.0, 1.0)?)
                {
                    let coef = [mx.e - m0.e, my.e - m0.e, mx.f - m0.f, my.f - m0.f];
                    raw = if m0.finite() && coef.iter().all(|x| x.is_finite()) {
                        (m0, coef, ok0, true)
                    } else {
                        (Transform::IDENTITY, [0.0; 4], false, false)
                    };
                }
            }
            (m, ok)
        }
    };
    if !m.finite() {
        c.transform = Transform::IDENTITY;
        c.transform_raw = Transform::IDENTITY;
        c.raw_coef = [0.0; 4];
        c.raw_known = 0;
        c.raw_pct = 0;
        c.transform_known = 0;
        return Ok(());
    }
    c.transform_raw = raw.0;
    c.raw_coef = raw.1;
    c.raw_known = u8::from(raw.2);
    c.raw_pct = u8::from(raw.3);
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
    let to = Transform { e: ox, f: oy, ..Transform::IDENTITY };
    let back = Transform { e: -ox, f: -oy, ..Transform::IDENTITY };
    let t = to.then(&m).then(&back);
    c.transform = if t.finite() { t } else { Transform::IDENTITY };
    c.transform_known = u8::from(known && t.finite());
    Ok(())
}

/// 原点の計算した値を参照の箱（幅・高さ）で解く（割合のない成分は箱が要らない）。有限でなければ None。
fn place(o: [f64; 4], viewport: Option<(f64, f64)>) -> Option<(f64, f64)> {
    let [ax, px, ay, py] = o;
    let comp = |a: f64, p: f64, r: Option<f64>| -> Option<f64> {
        if p == 0.0 {
            return Some(a);
        }
        let v = p * r? / 100.0;
        Some(if a == 0.0 { v } else { a + v })
    };
    let x = comp(ax, px, viewport.map(|v| v.0))?;
    let y = comp(ay, py, viewport.map(|v| v.1))?;
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

/// `transform-origin` の計算した値（[横の長さ, 横の割合, 縦の長さ, 縦の割合]。割合は参照の箱で解く。3 つめ（z）は使わない）。
/// 形が壊れているか長さが解けなければ None。
fn origin(
    s: &Specified,
    bufs: &Bufs<'_>,
    font: f64,
    line: f64,
    base: &Base,
    budget: &Budget<'_>,
) -> Result<Option<[f64; 4]>, Error> {
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
    let split = |l: Length, axis: Axis| -> Option<(f64, f64)> {
        if l.unit == Unit::Percent {
            return Some((0.0, l.value));
        }
        resolve(l, font, line, base, axis).map(|a| (a, 0.0))
    };
    Ok(split(x, Axis::X).zip(split(y, Axis::Y)).map(|((ax, px), (ay, py))| [ax, px, ay, py]))
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
