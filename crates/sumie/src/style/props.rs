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
    /// `width`・`height`：`auto` か負でない長さか割合。CSS では大きさの言葉（`stretch`・`min-content` など。[`SIZE_KEYWORDS`]）も
    /// 受け、`auto` と同じに扱う（Chrome と同じ。rect は描かない。CSS の洗い出し。ADR-103）
    SizeAuto,
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
    /// `background` のまとめ（カスケードで `background-color` に分ける）
    Background,
    /// `background-image`（層ごとに 1 つの画像。根の背景の画像の層を NotYet にするために読む）
    BackgroundImage,
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
    /// CSS の箱のプロパティ（根の border・padding・margin など、要素の outline、rect の論理の大きさ）。値は細かく確かめず、
    /// 初期の値と同じ絵になるかを宣言ごとに 1 度だけ求める（同じ絵にならなければ NotYet。ADR-103）
    CssBox(BoxRule),
}

/// CSS の箱のプロパティの書き方（Chrome と同じ形だけを受ける：壊れた値は宣言ごと捨てて前の宣言を生かす）と、初期の値と
/// 同じ絵になる値の決め方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoxRule {
    /// 線の太さ・形・色をそれぞれ 1 つまで（`border` など）。形が `none`・`hidden` か書いていなければ同じ（幅と色だけでは
    /// 描かない）
    BorderLine,
    /// 線の形の言葉を 1〜n（`border-style` は 4）。どれも `none`・`hidden` なら同じ
    Styles(u8),
    /// 長さか割合を 1〜max（`auto` を受けるか・負を受けるか）。どれも 0（か `auto`）なら同じ（padding・margin）
    Lengths { max: u8, auto: bool, neg: bool },
    /// 角の丸み：負でない長さか割合を 1〜max、`slash` なら `/` のあとにもう 1〜max。どれも 0 なら同じ
    Radius { max: u8, slash: bool },
    /// `none` だけなら同じ（値は細かく確かめない。none でなければ NotYet。box-shadow・border-image など）
    NoneOnly,
    /// outline の太さ・形（`auto` も）・色をそれぞれ 1 つまで。形が `none` か書いていなければ同じ
    OutlineLine,
    /// outline の形の言葉 1 つ。`none` なら同じ
    OutlineStyle,
    /// rect の論理の大きさ（`auto`・負でない長さか割合・大きさの言葉）。どの値も同じでない（`auto` も全体の言葉も幅・高さの
    /// 属性に勝つ）
    Logical,
    /// 言葉の 1 つ（`position`）。`neutral` なら同じ
    Keyword { words: &'static [&'static [u8]], neutral: &'static [u8] },
    /// `scrollbar-gutter`（`auto` か `stable` と `both-edges`）。`auto` なら同じ
    Gutter,
}

impl BoxRule {
    /// 効く要素：根だけ（[`BOX_ROOT`]）・どの要素も（[`BOX_OUTLINE`]）・rect（[`BOX_LOGICAL`]）。
    pub(crate) const fn scope(self) -> u8 {
        match self {
            BoxRule::OutlineLine | BoxRule::OutlineStyle => BOX_OUTLINE,
            BoxRule::Logical => BOX_LOGICAL,
            _ => BOX_ROOT,
        }
    }
}

/// `position` の言葉。
pub(crate) const POSITIONS: &[&[u8]] = &[b"static", b"relative", b"absolute", b"fixed", b"sticky"];

/// [`BoxRule::scope`] の印（計算した値の `css_box` の位）。
pub(crate) const BOX_ROOT: u8 = 1;
pub(crate) const BOX_OUTLINE: u8 = 2;
pub(crate) const BOX_LOGICAL: u8 = 4;

/// CSS の `width`・`height` で `auto` と同じに扱う大きさの言葉（Chrome が受けるもの）。
pub(crate) const SIZE_KEYWORDS: &[&[u8]] = &[
    b"stretch",
    b"-webkit-fill-available",
    b"min-content",
    b"max-content",
    b"fit-content",
    b"-webkit-min-content",
    b"-webkit-max-content",
    b"-webkit-fit-content",
];

/// CSS の `width`・`height` で `auto` と同じに扱う大きさの関数（Chrome が受けるもの）。
pub(crate) const SIZE_FUNCTIONS: &[&[u8]] = &[b"calc-size", b"anchor-size", b"fit-content", b"-webkit-fit-content"];

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
const CONTENT_VISIBILITY: &[&[u8]] = &[b"visible", b"auto", b"hidden"];
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
    p(b"-webkit-border-after", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"-webkit-border-after-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"-webkit-border-before", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"-webkit-border-before-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"-webkit-border-bottom-left-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"-webkit-border-bottom-right-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"-webkit-border-end", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"-webkit-border-end-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"-webkit-border-image", false, false, Kind::CssBox(BoxRule::NoneOnly)),
    p(b"-webkit-border-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 4, slash: true })),
    p(b"-webkit-border-start", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"-webkit-border-start-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"-webkit-border-top-left-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"-webkit-border-top-right-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"-webkit-box-shadow", false, false, Kind::CssBox(BoxRule::NoneOnly)),
    // Chrome が SVG の要素に効かせる接頭辞つきの効果（CSS のプロパティとしてだけ。まだ描かないので、none でなければ
    // NotYet。ADR-101）
    p(b"-webkit-clip-path", false, false, Kind::ClipPath),
    p(b"-webkit-filter", false, false, Kind::Filter),
    p(b"-webkit-logical-height", false, false, Kind::CssBox(BoxRule::Logical)),
    p(b"-webkit-logical-width", false, false, Kind::CssBox(BoxRule::Logical)),
    p(b"-webkit-margin-after", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"-webkit-margin-before", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"-webkit-margin-end", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"-webkit-margin-start", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"-webkit-mask", false, false, Kind::Raw),
    p(b"-webkit-mask-box-image", false, false, Kind::CssBox(BoxRule::NoneOnly)),
    p(b"-webkit-mask-box-image-source", false, false, Kind::CssBox(BoxRule::NoneOnly)),
    p(b"-webkit-mask-image", false, false, Kind::Raw),
    p(b"-webkit-padding-after", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"-webkit-padding-before", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"-webkit-padding-end", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"-webkit-padding-start", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"alignment-baseline", false, true, Kind::Keywords(ALIGNMENT)),
    // 全体の言葉だけを受け、カスケードで長い名前（direction・unicode-bidi を除く）に分ける（Chrome と同じ。ユーザーの判断）
    p(b"all", false, false, Kind::Keywords(&[])),
    // 根の svg の背景（CSS のプロパティとしてだけ。属性は読まない。resvg・Chrome と同じく描く。ADR-97 (4)）。まとめは
    // カスケードで `background-color` に分ける（最後の層の色。画像は描かない。Chrome と同じ。ADR-99 (4)）
    p(b"background", false, false, Kind::Background),
    p(b"background-color", false, false, Kind::Color),
    // 画像の層（グラデーションと data: の画像は Chrome が描くので、まだ描かない：NotYet。外の url は Chrome も img では
    // 読まない。単位 3 の 4 回目のレビュー）
    p(b"background-image", false, false, Kind::BackgroundImage),
    pu(b"baseline-shift", false, true, Kind::BaselineShift),
    p(b"block-size", false, false, Kind::CssBox(BoxRule::Logical)),
    p(b"border", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"border-block", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"border-block-end", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"border-block-end-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"border-block-start", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"border-block-start-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"border-block-style", false, false, Kind::CssBox(BoxRule::Styles(2))),
    p(b"border-bottom", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"border-bottom-left-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"border-bottom-right-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"border-bottom-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"border-end-end-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"border-end-start-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"border-image", false, false, Kind::CssBox(BoxRule::NoneOnly)),
    p(b"border-image-source", false, false, Kind::CssBox(BoxRule::NoneOnly)),
    p(b"border-inline", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"border-inline-end", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"border-inline-end-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"border-inline-start", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"border-inline-start-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"border-inline-style", false, false, Kind::CssBox(BoxRule::Styles(2))),
    p(b"border-left", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"border-left-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"border-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 4, slash: true })),
    p(b"border-right", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"border-right-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"border-shape", false, false, Kind::CssBox(BoxRule::NoneOnly)),
    p(b"border-start-end-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"border-start-start-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"border-style", false, false, Kind::CssBox(BoxRule::Styles(4))),
    p(b"border-top", false, false, Kind::CssBox(BoxRule::BorderLine)),
    p(b"border-top-left-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"border-top-right-radius", false, false, Kind::CssBox(BoxRule::Radius { max: 2, slash: false })),
    p(b"border-top-style", false, false, Kind::CssBox(BoxRule::Styles(1))),
    p(b"box-shadow", false, false, Kind::CssBox(BoxRule::NoneOnly)),
    p(b"clip", false, true, Kind::Raw),
    p(b"clip-path", false, true, Kind::ClipPath),
    p(b"clip-rule", true, true, Kind::Keywords(RULE)),
    p(b"color", true, true, Kind::Color),
    p(b"color-interpolation", true, true, Kind::Keywords(INTERPOLATION)),
    p(b"color-interpolation-filters", true, true, Kind::Keywords(INTERPOLATION)),
    p(b"color-rendering", true, true, Kind::Keywords(COLOR_RENDERING)),
    // 入れ物の中身を描かない（hidden。Chrome と同じ。ユーザーの判断。単位 3 の 5 回目のレビュー）
    p(b"content-visibility", false, false, Kind::Keywords(CONTENT_VISIBILITY)),
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
    p(b"height", false, true, Kind::SizeAuto),
    p(b"image-rendering", true, true, Kind::Keywords(IMAGE_RENDERING)),
    p(b"inline-size", false, false, Kind::CssBox(BoxRule::Logical)),
    p(b"isolation", false, true, Kind::Keywords(ISOLATION)),
    p(b"kerning", true, true, Kind::Raw),
    p(b"letter-spacing", true, true, Kind::Spacing),
    p(b"lighting-color", false, true, Kind::Color),
    p(b"line-height", true, false, Kind::LineHeight),
    p(b"margin", false, false, Kind::CssBox(BoxRule::Lengths { max: 4, auto: true, neg: true })),
    p(b"margin-block", false, false, Kind::CssBox(BoxRule::Lengths { max: 2, auto: true, neg: true })),
    p(b"margin-block-end", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"margin-block-start", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"margin-bottom", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"margin-inline", false, false, Kind::CssBox(BoxRule::Lengths { max: 2, auto: true, neg: true })),
    p(b"margin-inline-end", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"margin-inline-start", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"margin-left", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"margin-right", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"margin-top", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: true, neg: true })),
    p(b"marker", true, false, Kind::UrlOrNone),
    p(b"marker-end", true, true, Kind::UrlOrNone),
    p(b"marker-mid", true, true, Kind::UrlOrNone),
    p(b"marker-start", true, true, Kind::UrlOrNone),
    // CSS のまとめ（url のあとの言葉・画像・層）も受ける。none と url ただ 1 つのほかはまだ描かない（NotYet。単位 3 の
    // 4 回目のレビュー）
    p(b"mask", false, true, Kind::Raw),
    p(b"mask-image", false, false, Kind::Raw),
    p(b"mask-type", false, true, Kind::Keywords(MASK_TYPE)),
    p(b"mix-blend-mode", false, false, Kind::Keywords(BLEND)),
    // CSS の個別の変形と offset-path（CSS のプロパティとしてだけ。まだ描かないので、none でなければ NotYet。単位 3 の
    // 4 回目のレビュー）
    p(b"offset", false, false, Kind::Raw),
    p(b"offset-path", false, false, Kind::Raw),
    p(b"opacity", false, true, Kind::Opacity),
    p(b"outline", false, false, Kind::CssBox(BoxRule::OutlineLine)),
    p(b"outline-style", false, false, Kind::CssBox(BoxRule::OutlineStyle)),
    p(b"overflow", false, true, Kind::Overflow),
    // 長い名前（CSS のプロパティとしてだけ）。visible・auto のほかは切る側（NotYet）に倒す（単位 3 の 5 回目のレビュー）
    p(b"overflow-x", false, false, Kind::Keywords(OVERFLOW)),
    p(b"overflow-y", false, false, Kind::Keywords(OVERFLOW)),
    p(b"padding", false, false, Kind::CssBox(BoxRule::Lengths { max: 4, auto: false, neg: false })),
    p(b"padding-block", false, false, Kind::CssBox(BoxRule::Lengths { max: 2, auto: false, neg: false })),
    p(b"padding-block-end", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"padding-block-start", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"padding-bottom", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"padding-inline", false, false, Kind::CssBox(BoxRule::Lengths { max: 2, auto: false, neg: false })),
    p(b"padding-inline-end", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"padding-inline-start", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"padding-left", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"padding-right", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"padding-top", false, false, Kind::CssBox(BoxRule::Lengths { max: 1, auto: false, neg: false })),
    p(b"paint-order", true, true, Kind::PaintOrder),
    p(b"pointer-events", true, true, Kind::Keywords(POINTER_EVENTS)),
    p(b"position", false, false, Kind::CssBox(BoxRule::Keyword { words: POSITIONS, neutral: b"static" })),
    pu(b"r", false, true, Kind::LengthNonNeg),
    p(b"rotate", false, false, Kind::Raw),
    pu(b"rx", false, true, Kind::LengthAuto),
    pu(b"ry", false, true, Kind::LengthAuto),
    p(b"scale", false, false, Kind::Raw),
    p(b"scrollbar-gutter", false, false, Kind::CssBox(BoxRule::Gutter)),
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
    p(b"translate", false, false, Kind::Raw),
    p(b"unicode-bidi", false, true, Kind::Keywords(UNICODE_BIDI)),
    p(b"vector-effect", false, true, Kind::Keywords(VECTOR_EFFECT)),
    p(b"visibility", true, true, Kind::Keywords(VISIBILITY)),
    p(b"white-space", true, true, Kind::Keywords(WHITE_SPACE)),
    p(b"width", false, true, Kind::SizeAuto),
    p(b"word-spacing", true, true, Kind::Spacing),
    p(b"writing-mode", true, true, Kind::Keywords(WRITING_MODE)),
    pu(b"x", false, true, Kind::Length),
    pu(b"y", false, true, Kind::Length),
];

/// 名前（`lower` なら ASCII を小文字にして比べる。CSS の宣言）でプロパティを引く。
pub(crate) fn lookup(name: &[u8], lower: bool) -> Option<PropertyId> {
    lookup_in(name, lower, false)
}

/// CSS の宣言の名前でプロパティを引く（Chrome の別名も引く。属性の名前は別名を引かない）。
pub(crate) fn lookup_css(name: &[u8]) -> Option<PropertyId> {
    lookup_in(name, true, true)
}

fn lookup_in(name: &[u8], lower: bool, aliases: bool) -> Option<PropertyId> {
    if name.len() > NAME_MAX || !name.is_ascii() {
        return None;
    }
    let mut buf = [0u8; NAME_MAX];
    let key = buf.get_mut(..name.len())?;
    for (d, &s) in key.iter_mut().zip(name) {
        *d = if lower && s.is_ascii_uppercase() { s.saturating_add(32) } else { s };
    }
    let key: &[u8] = key;
    // Chrome の別名は元の名前と同じプロパティ（カスケードの勝ち負けも同じ。ADR-103）
    let key = if aliases { ALIASES.iter().find(|(a, _)| *a == key).map_or(key, |(_, to)| *to) } else { key };
    let i = lower_bound(PROPS, |pr| pr.name < key);
    PROPS.get(i).filter(|pr| pr.name == key).and_then(|_| u16::try_from(i).ok())
}

/// プロパティの名前の長さの上限（表のいちばん長い名前 `-webkit-border-bottom-right-radius` が入る）。
pub(crate) const NAME_MAX: usize = 40;

/// Chrome が同じプロパティとして読む別名（CSS の洗い出し。ADR-103）。
const ALIASES: &[(&[u8], &[u8])] = &[
    (b"-webkit-opacity", b"opacity"),
    (b"-webkit-transform", b"transform"),
    (b"-webkit-transform-origin", b"transform-origin"),
];

/// 番号のプロパティ。
pub(crate) fn prop(id: PropertyId) -> Option<&'static Prop> {
    PROPS.get(usize::from(id))
}

#[cfg(test)]
mod tests;
