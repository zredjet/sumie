//! プロパティの表（FR-ST-09・ADR-66 (3)(4)）。名前・受け継ぐか・プレゼンテーション属性か・値の形。
//!
//! 名前のバイトの順に並べてあり、番号（[`PropertyId`]）は表の中の位置。CSS の宣言の名前は大文字小文字を問わずに引き、
//! プレゼンテーション属性の名前は区別して引く（XML の属性の名前）。値の形は、カスケードで「壊れた値は無視する」ために
//! 確かめる形（[`super::valid`]）。形を細かく確かめない（`Raw`）プロパティは、そのプロパティを描く段（M2〜M6）で
//! 確かめ方を足す。

use crate::mem::lower_bound;

/// プロパティの番号（表の中の位置）。
pub(crate) type PropertyId = u16;

/// 値の形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Paint,
    Color,
    /// 数か割合（不透明度。計算した値で 0〜1 に収める）
    Opacity,
    /// 長さか割合（負も）
    Length,
    /// 負でない長さか割合
    LengthNonNeg,
    /// `auto` か負でない長さか割合
    LengthAuto,
    /// 1 以上の数（`stroke-miterlimit`）
    MiterLimit,
    /// 決まった言葉のどれか
    Keywords(&'static [&'static [u8]]),
    /// `none` か `url(...)`
    UrlOrNone,
    /// `none` か、負でない長さか割合の並び
    Dasharray,
    /// 書体の名前の並び
    FontFamily,
    /// 文字の大きさ（言葉・負でない長さか割合）
    FontSize,
    /// 文字の太さ（言葉か 1〜1000 の数）
    FontWeight,
    /// `normal` か長さ（`letter-spacing`・`word-spacing`）
    Spacing,
    /// `line-height`（`normal`・数・長さか割合）
    LineHeight,
    /// `font-style`（`normal`・`italic`・`oblique <角度>?`）
    FontStyle,
    /// `font-stretch`（言葉か負でない割合）
    FontStretch,
    /// `display`（CSS Display 3 の 1 つ・2 つの言葉の書き方）
    Display,
    /// `overflow`（1 つか 2 つの言葉）
    Overflow,
    /// `font` のまとめ（カスケードで長い名前に分ける）
    Font,
    /// `baseline`・`sub`・`super` か長さか割合
    BaselineShift,
    /// CSS の `transform`
    Transform,
    /// `transform-origin`
    TransformOrigin,
    /// `paint-order`
    PaintOrder,
    /// `filter`（`none`・`url()`・フィルタの関数）
    Filter,
    /// `clip-path`（`none`・`url()`・基本の形）
    ClipPath,
    /// `d`（`none` か `path("…")`）
    Path,
    /// 細かく確かめない（空でなければ受ける）
    Raw,
}

/// プロパティ。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Prop {
    pub(crate) name: &'static [u8],
    pub(crate) inherited: bool,
    /// プレゼンテーション属性として読むか（`mix-blend-mode`・`transform`（属性は別に読む）・まとめのプロパティは読まない）
    pub(crate) attr: bool,
    /// CSS でも単位のない長さを受けるか（SVG のプロパティ。CSS のプロパティは属性でだけ受ける。Chrome と同じ。ADR-74）
    pub(crate) unitless: bool,
    pub(crate) kind: Kind,
}

const fn p(name: &'static [u8], inherited: bool, attr: bool, kind: Kind) -> Prop {
    Prop { name, inherited, attr, unitless: false, kind }
}

/// CSS でも単位のない長さを受けるプロパティ。
const fn pu(name: &'static [u8], inherited: bool, attr: bool, kind: Kind) -> Prop {
    Prop { name, inherited, attr, unitless: true, kind }
}

const LINECAP: &[&[u8]] = &[b"butt", b"round", b"square"];
// `miter-clip`・`arcs` は SVG 2 にあるが、Chrome と同じく壊れた値にする（宣言を無視して親の値を受け継ぐ。ADR-91 (1)。
// ADR-74 の「仕様に合わせて読む」を改める）
const LINEJOIN: &[&[u8]] = &[b"miter", b"round", b"bevel"];
const RULE: &[&[u8]] = &[b"nonzero", b"evenodd"];
const VISIBILITY: &[&[u8]] = &[b"visible", b"hidden", b"collapse"];
pub(crate) const DISPLAY: &[&[u8]] = &[
    b"inline",
    b"block",
    b"none",
    b"inline-block",
    b"list-item",
    b"run-in",
    b"table",
    b"inline-table",
    b"table-row-group",
    b"table-header-group",
    b"table-footer-group",
    b"table-row",
    b"table-column-group",
    b"table-column",
    b"table-cell",
    b"table-caption",
    b"flex",
    b"inline-flex",
    b"grid",
    b"inline-grid",
    b"contents",
    b"flow-root",
];
pub(crate) const OVERFLOW: &[&[u8]] = &[b"visible", b"hidden", b"scroll", b"auto", b"clip", b"overlay"];
const RENDERING: &[&[u8]] = &[b"auto", b"optimizespeed", b"crispedges", b"geometricprecision"];
const COLOR_RENDERING: &[&[u8]] = &[b"auto", b"optimizespeed", b"optimizequality"];
const TEXT_RENDERING: &[&[u8]] = &[b"auto", b"optimizespeed", b"optimizelegibility", b"geometricprecision"];
const IMAGE_RENDERING: &[&[u8]] =
    &[b"auto", b"optimizespeed", b"optimizequality", b"smooth", b"high-quality", b"crisp-edges", b"pixelated"];
const INTERPOLATION: &[&[u8]] = &[b"auto", b"srgb", b"linearrgb"];
const ISOLATION: &[&[u8]] = &[b"auto", b"isolate"];
const BLEND: &[&[u8]] = &[
    b"normal",
    b"multiply",
    b"screen",
    b"overlay",
    b"darken",
    b"lighten",
    b"color-dodge",
    b"color-burn",
    b"hard-light",
    b"soft-light",
    b"difference",
    b"exclusion",
    b"hue",
    b"saturation",
    b"color",
    b"luminosity",
    b"plus-darker",
    b"plus-lighter",
];
const MASK_TYPE: &[&[u8]] = &[b"luminance", b"alpha"];
const VECTOR_EFFECT: &[&[u8]] =
    &[b"none", b"non-scaling-stroke", b"non-scaling-size", b"non-rotation", b"fixed-position"];
const TEXT_ANCHOR: &[&[u8]] = &[b"start", b"middle", b"end"];
const WRITING_MODE: &[&[u8]] = &[
    b"horizontal-tb",
    b"vertical-rl",
    b"vertical-lr",
    b"sideways-rl",
    b"sideways-lr",
    b"lr",
    b"lr-tb",
    b"rl",
    b"rl-tb",
    b"tb",
    b"tb-rl",
];
const DIRECTION: &[&[u8]] = &[b"ltr", b"rtl"];
pub(crate) const FONT_STRETCH: &[&[u8]] = &[
    b"normal",
    b"ultra-condensed",
    b"extra-condensed",
    b"condensed",
    b"semi-condensed",
    b"semi-expanded",
    b"expanded",
    b"extra-expanded",
    b"ultra-expanded",
];
const DOMINANT: &[&[u8]] = &[
    b"auto",
    b"text-bottom",
    b"alphabetic",
    b"ideographic",
    b"middle",
    b"central",
    b"mathematical",
    b"hanging",
    b"text-top",
    b"use-script",
    b"no-change",
    b"reset-size",
    b"text-after-edge",
    b"text-before-edge",
];
const ALIGNMENT: &[&[u8]] = &[
    b"auto",
    b"baseline",
    b"before-edge",
    b"text-before-edge",
    b"middle",
    b"central",
    b"after-edge",
    b"text-after-edge",
    b"ideographic",
    b"alphabetic",
    b"hanging",
    b"mathematical",
    b"text-top",
    b"text-bottom",
];
const UNICODE_BIDI: &[&[u8]] = &[b"normal", b"embed", b"isolate", b"bidi-override", b"isolate-override", b"plaintext"];
const WHITE_SPACE: &[&[u8]] = &[b"normal", b"pre", b"nowrap", b"pre-wrap", b"break-spaces", b"pre-line"];
const TRANSFORM_BOX: &[&[u8]] = &[b"content-box", b"border-box", b"fill-box", b"stroke-box", b"view-box"];
const FONT_KERNING: &[&[u8]] = &[b"auto", b"normal", b"none"];
const POINTER_EVENTS: &[&[u8]] = &[
    b"auto",
    b"bounding-box",
    b"visiblepainted",
    b"visiblefill",
    b"visiblestroke",
    b"visible",
    b"painted",
    b"fill",
    b"stroke",
    b"all",
    b"none",
];

/// 表（名前のバイトの順）。
pub(crate) const PROPS: &[Prop] = &[
    p(b"alignment-baseline", false, true, Kind::Keywords(ALIGNMENT)),
    // 根の svg の背景（CSS のプロパティとしてだけ。属性は読まない。resvg・Chrome と同じく描く。ADR-97 (4)）
    p(b"background-color", false, false, Kind::Color),
    pu(b"baseline-shift", false, true, Kind::BaselineShift),
    p(b"clip", false, true, Kind::Raw),
    p(b"clip-path", false, true, Kind::ClipPath),
    p(b"clip-rule", true, true, Kind::Keywords(RULE)),
    p(b"color", true, true, Kind::Color),
    p(b"color-interpolation", true, true, Kind::Keywords(INTERPOLATION)),
    p(b"color-interpolation-filters", true, true, Kind::Keywords(INTERPOLATION)),
    p(b"color-rendering", true, true, Kind::Keywords(COLOR_RENDERING)),
    p(b"cursor", true, true, Kind::Raw),
    pu(b"cx", false, true, Kind::Length),
    pu(b"cy", false, true, Kind::Length),
    p(b"d", false, true, Kind::Path),
    p(b"direction", true, true, Kind::Keywords(DIRECTION)),
    p(b"display", false, true, Kind::Display),
    p(b"dominant-baseline", true, true, Kind::Keywords(DOMINANT)),
    p(b"enable-background", false, true, Kind::Raw),
    p(b"fill", true, true, Kind::Paint),
    p(b"fill-opacity", true, true, Kind::Opacity),
    p(b"fill-rule", true, true, Kind::Keywords(RULE)),
    p(b"filter", false, true, Kind::Filter),
    p(b"flood-color", false, true, Kind::Color),
    p(b"flood-opacity", false, true, Kind::Opacity),
    p(b"font", true, false, Kind::Font),
    p(b"font-family", true, true, Kind::FontFamily),
    p(b"font-feature-settings", true, false, Kind::Raw),
    p(b"font-kerning", true, false, Kind::Keywords(FONT_KERNING)),
    p(b"font-size", true, true, Kind::FontSize),
    p(b"font-size-adjust", true, true, Kind::Raw),
    p(b"font-stretch", true, true, Kind::FontStretch),
    p(b"font-style", true, true, Kind::FontStyle),
    p(b"font-variant", true, true, Kind::Raw),
    p(b"font-variation-settings", true, false, Kind::Raw),
    p(b"font-weight", true, true, Kind::FontWeight),
    p(b"glyph-orientation-horizontal", true, true, Kind::Raw),
    p(b"glyph-orientation-vertical", true, true, Kind::Raw),
    p(b"height", false, true, Kind::LengthAuto),
    p(b"image-rendering", true, true, Kind::Keywords(IMAGE_RENDERING)),
    p(b"isolation", false, true, Kind::Keywords(ISOLATION)),
    p(b"kerning", true, true, Kind::Raw),
    p(b"letter-spacing", true, true, Kind::Spacing),
    p(b"lighting-color", false, true, Kind::Color),
    p(b"line-height", true, false, Kind::LineHeight),
    p(b"marker", true, false, Kind::UrlOrNone),
    p(b"marker-end", true, true, Kind::UrlOrNone),
    p(b"marker-mid", true, true, Kind::UrlOrNone),
    p(b"marker-start", true, true, Kind::UrlOrNone),
    p(b"mask", false, true, Kind::UrlOrNone),
    p(b"mask-type", false, true, Kind::Keywords(MASK_TYPE)),
    p(b"mix-blend-mode", false, false, Kind::Keywords(BLEND)),
    p(b"opacity", false, true, Kind::Opacity),
    p(b"overflow", false, true, Kind::Overflow),
    p(b"paint-order", true, true, Kind::PaintOrder),
    p(b"pointer-events", true, true, Kind::Keywords(POINTER_EVENTS)),
    pu(b"r", false, true, Kind::LengthNonNeg),
    pu(b"rx", false, true, Kind::LengthAuto),
    pu(b"ry", false, true, Kind::LengthAuto),
    p(b"shape-rendering", true, true, Kind::Keywords(RENDERING)),
    p(b"stop-color", false, true, Kind::Color),
    p(b"stop-opacity", false, true, Kind::Opacity),
    p(b"stroke", true, true, Kind::Paint),
    p(b"stroke-dasharray", true, true, Kind::Dasharray),
    pu(b"stroke-dashoffset", true, true, Kind::Length),
    p(b"stroke-linecap", true, true, Kind::Keywords(LINECAP)),
    p(b"stroke-linejoin", true, true, Kind::Keywords(LINEJOIN)),
    p(b"stroke-miterlimit", true, true, Kind::MiterLimit),
    p(b"stroke-opacity", true, true, Kind::Opacity),
    pu(b"stroke-width", true, true, Kind::LengthNonNeg),
    p(b"text-anchor", true, true, Kind::Keywords(TEXT_ANCHOR)),
    p(b"text-decoration", false, true, Kind::Raw),
    p(b"text-rendering", true, true, Kind::Keywords(TEXT_RENDERING)),
    p(b"transform", false, false, Kind::Transform),
    p(b"transform-box", false, false, Kind::Keywords(TRANSFORM_BOX)),
    p(b"transform-origin", false, true, Kind::TransformOrigin),
    p(b"unicode-bidi", false, true, Kind::Keywords(UNICODE_BIDI)),
    p(b"vector-effect", false, true, Kind::Keywords(VECTOR_EFFECT)),
    p(b"visibility", true, true, Kind::Keywords(VISIBILITY)),
    p(b"white-space", true, true, Kind::Keywords(WHITE_SPACE)),
    p(b"width", false, true, Kind::LengthAuto),
    p(b"word-spacing", true, true, Kind::Spacing),
    p(b"writing-mode", true, true, Kind::Keywords(WRITING_MODE)),
    pu(b"x", false, true, Kind::Length),
    pu(b"y", false, true, Kind::Length),
];

/// 名前（`lower` なら ASCII を小文字にして比べる。CSS の宣言）でプロパティを引く。
pub(crate) fn lookup(name: &[u8], lower: bool) -> Option<PropertyId> {
    if name.len() > 32 || !name.is_ascii() {
        return None;
    }
    let mut buf = [0u8; 32];
    let key = buf.get_mut(..name.len())?;
    for (d, &s) in key.iter_mut().zip(name) {
        *d = if lower && s.is_ascii_uppercase() { s.saturating_add(32) } else { s };
    }
    let key: &[u8] = key;
    let i = lower_bound(PROPS, |pr| pr.name < key);
    PROPS.get(i).filter(|pr| pr.name == key).and_then(|_| u16::try_from(i).ok())
}

/// 番号のプロパティ。
pub(crate) fn prop(id: PropertyId) -> Option<&'static Prop> {
    PROPS.get(usize::from(id))
}

#[cfg(test)]
mod tests;
