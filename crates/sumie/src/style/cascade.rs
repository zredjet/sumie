//! カスケード（FR-ST-09・ADR-66 (3)・architecture.md §4.2）。要素ごとに、プロパティごとの指定の値（勝った宣言の値の
//! 字句の範囲）を決める。
//!
//! 弱い順：プレゼンテーション属性 ＜ 規則（層の順位・詳細度・規則の順・宣言の順） ＜ `style` 属性 ＜ 規則の `!important`
//! （層は逆の順。層のない規則がいちばん弱い） ＜ `style` 属性の `!important`。壊れた値の宣言は勝たない（無視して警告）。
//! 規則の宣言は、照合の前に 1 度だけ確かめる。値は計算した値を求めるとき（木を作る段）に文脈で読み直す。
//!
//! 幾何のプロパティ（x・y・width・height・r・rx・ry・cx・cy・d）のプレゼンテーション属性は、それを幾何に使う要素でだけ
//! 読む（`text` の `x` などは別の意味の属性）。SVG の名前空間の要素だけに指定の値を持つ。

use crate::budget::Budget;
use crate::cost;
use crate::css::decl::{Decl, Item, parse_declarations};
use crate::css::lex::{Lexer, decode};
use crate::css::matcher::{MatchSink, match_rules};
use crate::css::select::NONE;
use crate::css::sheet::{COND_MEDIA, COND_SUPPORTS_IMPORT, Sheet};
use crate::error::Error;
use crate::mem::{Frozen, MVec, Plain, stable_sort};
use crate::report::WarningKind;
use crate::style::cond::{Env, media, supports, supports_import};
use crate::style::props::{PROPS, PropertyId, lookup, lookup_css, prop};
use crate::style::valid::{Ctx, Wide, font_parts, valid, wide};
use crate::xml::lex::Span;
use crate::xml::{FLAG_IN_STORE, NS_NONE, NS_SVG, Node, XmlDoc};

/// 値の置き場。
pub(crate) const SRC_INPUT: u8 = 0;
pub(crate) const SRC_STORE: u8 = 1;
pub(crate) const SRC_SHEET: u8 = 2;

/// どこから来た宣言か（`transform` の書き方が違う）。
pub(crate) const ORIGIN_ATTR: u8 = 0;
pub(crate) const ORIGIN_RULE: u8 = 1;
pub(crate) const ORIGIN_STYLE: u8 = 2;

/// 指定の値（勝った宣言）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Specified {
    pub(crate) node: u32,
    /// 値の字句の範囲（`src` の置き場の中）
    pub(crate) value: Span,
    pub(crate) prop: u16,
    pub(crate) src: u8,
    pub(crate) origin: u8,
    /// CSS の全体の言葉（0 なし、1 inherit、2 initial、3 unset。`revert-layer` の宣言は勝たないので持たない）
    pub(crate) wide: u8,
    pub(crate) pad: [u8; 3],
}

impl Plain for Specified {}

/// カスケードの結果。
#[derive(Debug)]
pub(crate) struct Styles {
    /// 節の番号の順、同じ節はプロパティの番号の順
    pub(crate) specs: Frozen<Specified>,
    /// 節ごとの `specs` の始まり（節の数 ＋ 1）
    pub(crate) index: Frozen<u32>,
    /// 警告（種類と位置）
    pub(crate) warnings: Frozen<Warn>,
}

impl Styles {
    pub(crate) fn bytes(&self) -> u64 {
        self.specs.bytes().saturating_add(self.index.bytes()).saturating_add(self.warnings.bytes())
    }

    /// 節の指定の値。
    pub(crate) fn of(&self, node: u32) -> &[Specified] {
        let ix = self.index.as_slice();
        let a = ix.get(node as usize).copied().unwrap_or(0) as usize;
        let b = ix.get((node as usize).saturating_add(1)).copied().unwrap_or(0) as usize;
        self.specs.as_slice().get(a..b).unwrap_or(&[])
    }
}

/// 警告。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Warn {
    pub(crate) pos: u32,
    pub(crate) kind: WarningKind,
    pub(crate) pad: u16,
}

impl Plain for Warn {}

/// 規則の宣言を前に確かめた結果。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DeclInfo {
    prop: u16,
    wide: u8,
    ok: u8,
    /// `font` のまとめの長い名前の範囲・`background` のまとめの色の範囲（`fonts` の番号。ほかは NONE）
    font: u32,
}

/// `background-image` の画像の層の印を求めた印（`pad[0]` の上の位。下の位は画像の層の印 `valid::IMAGE_GRADIENT`・
/// `IMAGE_DATA`・`IMAGE_FUNCTION`。まとめから分けたものは値の範囲を置かない）。
pub(crate) const BG_IMAGE_FROM_SHORTHAND: u8 = 0x80;

/// CSS の箱のプロパティ（`Kind::CssBox`）の値が初期の値と同じ絵にならない印（`pad[0]`。宣言ごとに 1 度だけ求める。ADR-103）。
pub(crate) const BOX_ON: u8 = 1;

/// CSS の箱のプロパティの宣言の印（同じ絵にならなければ [`BOX_ON`]）。
fn box_flag(id: PropertyId, text: &[u8], value: Span, budget: &Budget<'_>) -> Result<Option<u8>, Error> {
    match prop(id).map(|p| p.kind) {
        Some(crate::style::props::Kind::CssBox(rule)) => {
            // 確かめは済んでいる（壊れた値は候補にならない）。同じ絵にならなければ BOX_ON
            let cx = crate::style::valid::Ctx { attr: false, unitless: false, viewport: true };
            let same = crate::style::valid::css_box(rule, text, value, cx, budget)?.unwrap_or(false);
            Ok(Some(if same { 0 } else { BOX_ON }))
        }
        _ => Ok(None),
    }
}

/// `font` のまとめを長い名前に分けた範囲（宣言ごとに 1 度だけ求める）。順は FONT_LONGHANDS の初めの 7 つ。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct FontRec {
    spans: [Span; 7],
    /// 書いた長い名前のビット
    present: u8,
    pad: [u8; 3],
}

impl Plain for FontRec {}

impl FontRec {
    /// `background` のまとめの最後の層の色の範囲（`spans[0]`。色があれば `present` の 1）と画像の層の印（`pad[0]`。
    /// `valid::IMAGE_GRADIENT`・`IMAGE_DATA`。ADR-99 (4)・ADR-101・ADR-102）。
    fn background(b: crate::style::valid::Background) -> FontRec {
        let mut r = FontRec::default();
        if let Some(c) = b.color {
            r.spans[0] = c;
            r.present = 1;
        }
        r.pad[0] = b.image;
        r
    }

    /// 長い名前の `background-image` の画像の層の印（`pad[0]`）。
    fn image(flags: u8) -> FontRec {
        let mut r = FontRec::default();
        r.pad[0] = flags;
        r
    }

    fn of(p: &crate::style::valid::FontParts) -> FontRec {
        let parts = [p.style, p.variant, p.weight, p.stretch, p.size, p.line_height, p.family];
        let mut r = FontRec::default();
        for (k, x) in parts.iter().enumerate() {
            if let (Some(sp), Some(slot)) = (x, r.spans.get_mut(k)) {
                *slot = *sp;
                r.present |= 1u8.checked_shl(k as u32).unwrap_or(0);
            }
        }
        r
    }
}

/// `font` が決める長い名前（初めの 7 つは FontRec の順。残りは `font` が初期の値にするもの）。
const FONT_LONGHANDS: [&[u8]; 11] = [
    b"font-style",
    b"font-variant",
    b"font-weight",
    b"font-stretch",
    b"font-size",
    b"line-height",
    b"font-family",
    b"font-size-adjust",
    b"font-kerning",
    b"font-feature-settings",
    b"font-variation-settings",
];

impl Plain for DeclInfo {}

/// 勝ち負けを決める鍵（大きい方が勝つ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
struct Key {
    tier: u8,
    layer: u32,
    spec: u32,
    order: u32,
    idx: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Slot {
    set: u8,
    pad: [u8; 3],
    key_tier: u32,
    key_layer: u32,
    key_spec: u32,
    key_order: u32,
    key_idx: u32,
    spec: Specified,
}

impl Plain for Slot {}

impl Slot {
    fn key(&self) -> Key {
        Key {
            tier: self.key_tier as u8,
            layer: self.key_layer,
            spec: self.key_spec,
            order: self.key_order,
            idx: self.key_idx,
        }
    }
}

/// `Specified::wide` の値。
pub(crate) const WIDE_NONE: u8 = 0;
pub(crate) const WIDE_INHERIT: u8 = 1;
pub(crate) const WIDE_INITIAL: u8 = 2;
pub(crate) const WIDE_UNSET: u8 = 3;
/// `revert-layer`（候補にしない。弱い宣言に戻す）
const WIDE_REVERT_LAYER: u8 = 4;

const fn wide_code(w: Option<Wide>) -> u8 {
    match w {
        None => WIDE_NONE,
        Some(Wide::Inherit) => WIDE_INHERIT,
        Some(Wide::Initial) => WIDE_INITIAL,
        Some(Wide::Unset) => WIDE_UNSET,
        Some(Wide::RevertLayer) => WIDE_REVERT_LAYER,
    }
}

/// アニメーションの要素（`fill` 属性は時の扱いで、塗りではない）。
fn animation(local: &[u8]) -> bool {
    matches!(local, b"animate" | b"animateMotion" | b"animateTransform" | b"animateColor" | b"set" | b"discard")
}

/// 幾何のプロパティのプレゼンテーション属性を、この要素で読むか（`outer` は外側の svg か）。use・symbol・入れ子の svg の
/// width・height は XML の属性で、CSS のプロパティにしない（SVG 2 の geometry・Chrome と同じ。木を作る段が属性として読む）。
fn geometry_ok(local: &[u8], name: &[u8], outer: bool) -> bool {
    match name {
        b"x" | b"y" => matches!(local, b"rect" | b"image" | b"foreignObject" | b"svg" | b"use" | b"symbol"),
        b"width" | b"height" => matches!(local, b"rect" | b"image" | b"foreignObject") || (outer && local == b"svg"),
        b"r" => local == b"circle",
        b"cx" | b"cy" => matches!(local, b"circle" | b"ellipse"),
        b"rx" | b"ry" => matches!(local, b"rect" | b"ellipse"),
        b"d" => local == b"path",
        _ => true,
    }
}

/// カスケードする。`env` は条件を照らす環境（明暗と文書の大きさ）。
pub(crate) fn cascade(
    doc: &XmlDoc,
    input: &[u8],
    sheet: &Sheet,
    env: &Env,
    budget: &Budget<'_>,
) -> Result<Styles, Error> {
    let budget = budget_ref(budget);
    let text = sheet.text.as_slice();
    // 条件を照らす（外の条件まで含めて真か。外の条件は中の条件より前にあるので、順に 1 度ずつ）
    let nconds = sheet.conds.len();
    let mut cond_ok: MVec<'_, u8> = MVec::with_capacity(budget, nconds)?;
    for c in sheet.conds.as_slice() {
        budget.spend(cost::OP_STEP)?;
        let outer = c.parent == NONE || cond_ok.get(c.parent as usize).copied() == Some(1);
        let ok = outer
            && match c.kind {
                COND_MEDIA => media(text, c.text, env, budget)?,
                COND_SUPPORTS_IMPORT => supports_import(text, c.text, sheet.ns_table(c), budget)?,
                _ => supports(text, c.text, sheet.ns_table(c), budget)?,
            };
        cond_ok.push(u8::from(ok))?;
    }
    // 規則ごとに当てるか
    let mut active: MVec<'_, u8> = MVec::with_capacity(budget, sheet.rules.len())?;
    for r in sheet.rules.as_slice() {
        budget.spend(cost::OP_STEP)?;
        let ok = r.cond == NONE || cond_ok.get(r.cond as usize).copied() == Some(1);
        active.push(u8::from(ok))?;
    }
    // 層の順位（真の条件の所で名前を書いた順）
    let ranks = sheet.layer_ranks(cond_ok.as_slice(), budget)?;
    // 表示の領域の単位（vw など）は文書の大きさが決まるときだけ読める
    let viewport = env.size.is_some();
    // 規則の宣言を 1 度だけ確かめる（宣言は規則の順に続いて並ぶ。規則の外の宣言はない）
    let mut warns: MVec<'_, Warn> = MVec::new(budget);
    let mut infos: MVec<'_, DeclInfo> = MVec::with_capacity(budget, sheet.decls.len())?;
    infos.resize_to(sheet.decls.len(), DeclInfo { prop: u16::MAX, wide: 0, ok: 0, font: NONE })?;
    let mut fonts: MVec<'_, FontRec> = MVec::new(budget);
    let font_id = lookup(b"font", false);
    let background_id = lookup(b"background", false);
    let background_image_id = lookup(b"background-image", false);
    for (ri, r) in sheet.rules.as_slice().iter().enumerate() {
        // 当たらない規則（条件が偽）の宣言は確かめない（警告も出さない）
        if active.get(ri).copied().unwrap_or(0) == 0 {
            budget.spend(cost::OP_STEP)?;
            continue;
        }
        for k in r.decls.start..r.decls.end {
            budget.spend(cost::OP_STEP)?;
            let Some(d) = sheet.decls.as_slice().get(k as usize) else { continue };
            let mut info = check_decl(text, d, viewport, budget)?;
            if info.prop != u16::MAX && info.ok == 0 {
                warns.push(Warn { pos: r.pos, kind: WarningKind::InvalidValue, pad: 0 })?;
            }
            // `font` のまとめは長い名前の範囲を、`background` のまとめは色の範囲を、宣言ごとに 1 度だけ求める（当たった要素
            // ごとに求め直すと、要素の数 × 値の長さの手間になる。単位 3 の 3 回目のレビュー）
            if Some(info.prop) == font_id && info.ok != 0 && info.wide == WIDE_NONE {
                let cx = Ctx { attr: false, unitless: false, viewport };
                if let Some(p) = font_parts(text, d.value, cx, budget)? {
                    info.font = fonts.len() as u32;
                    fonts.push(FontRec::of(&p))?;
                }
            }
            if Some(info.prop) == background_id
                && info.ok != 0
                && info.wide == WIDE_NONE
                && let Some(c) = crate::style::valid::background_color(text, d.value, budget)?
            {
                info.font = fonts.len() as u32;
                fonts.push(FontRec::background(c))?;
            }
            // 長い名前の `background-image` の画像の層の印も宣言ごとに 1 度だけ求める（木を作る段で読み直さない：読み直すと
            // 木を作る段の見積もりに入らない。単位 3 の 5 回目のレビュー）
            if Some(info.prop) == background_image_id
                && info.ok != 0
                && info.wide == WIDE_NONE
                && let Some(flags) = crate::style::valid::image_list(text, d.value, budget)?
            {
                info.font = fonts.len() as u32;
                fonts.push(FontRec::image(flags))?;
            }
            // CSS の箱のプロパティの印も宣言ごとに 1 度だけ
            if info.ok != 0
                && info.wide == WIDE_NONE
                && let Some(flag) = box_flag(info.prop, text, d.value, budget)?
            {
                info.font = fonts.len() as u32;
                fonts.push(FontRec::image(flag))?;
            }
            if let Some(x) = infos.get_mut(k as usize) {
                *x = info;
            }
        }
    }
    // 照合しながら要素ごとに決める
    let nodes = doc.nodes.len();
    let mut out = Out {
        short: Shorthands::new(),
        doc,
        input,
        sheet,
        infos: infos.as_slice(),
        fonts: fonts.as_slice(),
        ranks: ranks.as_slice(),
        viewport,
        budget,
        slots: MVec::with_capacity(budget, PROPS.len())?,
        touched: MVec::with_capacity(budget, PROPS.len())?,
        cur: NONE,
        specs: MVec::new(budget),
        index: MVec::with_capacity(budget, nodes.saturating_add(1))?,
        warns,
    };
    out.slots.resize_to(PROPS.len(), Slot::empty())?;
    match_rules(doc, input, sheet, active.as_slice(), budget, &mut out)?;
    out.finish_element()?;
    // 残りの節の始まり（終わりの印を含めて節の数 ＋ 1）
    let n = out.specs.len() as u32;
    while out.index.len() <= nodes {
        budget.spend(cost::OP_STEP)?;
        out.index.push(n)?;
    }
    // 指定の値をぴったりの容量に写す（伸ばしながら集めたので）
    let mut exact: MVec<'_, Specified> = MVec::with_capacity(budget, out.specs.len())?;
    exact.append_slice(out.specs.as_slice())?;
    let mut w: MVec<'_, Warn> = MVec::with_capacity(budget, out.warns.len())?;
    w.append_slice(out.warns.as_slice())?;
    let index = core::mem::replace(&mut out.index, MVec::new(budget));
    Ok(Styles { specs: exact.freeze(), index: index.freeze(), warnings: w.freeze() })
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 宣言の名前（escape を解く）でプロパティを引く。
pub(crate) fn decl_prop(text: &[u8], d: &Decl, budget: &Budget<'_>) -> Result<Option<PropertyId>, Error> {
    budget.spend(cost::LOOKUP)?;
    if d.name_escaped == 0 {
        return Ok(lookup_css(d.name.of(text)));
    }
    // 名前は NAME_MAX バイトの内の ASCII（表のどの名前もそう）
    let mut buf = [0u8; crate::style::props::NAME_MAX];
    let mut n = 0usize;
    for c in decode(text, d.name) {
        let Some(b) = u8::try_from(c).ok().filter(u8::is_ascii) else { return Ok(None) };
        let Some(x) = buf.get_mut(n) else { return Ok(None) };
        *x = b;
        n = n.saturating_add(1);
    }
    Ok(lookup_css(buf.get(..n).unwrap_or(&[])))
}

/// 宣言の名前と値を確かめる（知らない名前は prop が u16::MAX）。
fn check_decl(text: &[u8], d: &Decl, viewport: bool, budget: &Budget<'_>) -> Result<DeclInfo, Error> {
    let Some(id) = decl_prop(text, d, budget)? else {
        return Ok(DeclInfo { prop: u16::MAX, wide: 0, ok: 0, font: NONE });
    };
    let w = wide(text, d.value, budget)?;
    let ok = match (w, prop(id)) {
        (Some(_), _) => true,
        (None, Some(p)) => {
            let cx = Ctx { attr: false, unitless: p.unitless, viewport };
            valid(p.kind, text, d.value, cx, budget)?
        }
        (None, None) => false,
    };
    Ok(DeclInfo { prop: id, wide: wide_code(w), ok: u8::from(ok), font: NONE })
}

impl Slot {
    const fn empty() -> Slot {
        Slot {
            set: 0,
            pad: [0; 3],
            key_tier: 0,
            key_layer: 0,
            key_spec: 0,
            key_order: 0,
            key_idx: 0,
            spec: Specified {
                node: 0,
                value: Span { start: 0, end: 0 },
                prop: 0,
                src: 0,
                origin: 0,
                wide: 0,
                pad: [0; 3],
            },
        }
    }
}

/// まとめのプロパティの長い名前の番号（表を 1 度だけ引く。CSS の 4 回目のレビュー）。
#[derive(Debug, Clone, Copy)]
struct Shorthands {
    font: Option<PropertyId>,
    font_longhands: [Option<PropertyId>; 11],
    markers: [Option<PropertyId>; 3],
    background: Option<PropertyId>,
    background_color: Option<PropertyId>,
    background_image: Option<PropertyId>,
}

impl Shorthands {
    fn new() -> Shorthands {
        Shorthands {
            font: lookup(b"font", false),
            font_longhands: FONT_LONGHANDS.map(|n| lookup(n, false)),
            markers: [b"marker-start".as_slice(), b"marker-mid", b"marker-end"].map(|n| lookup(n, false)),
            background: lookup(b"background", false),
            background_color: lookup(b"background-color", false),
            background_image: lookup(b"background-image", false),
        }
    }
}

struct Out<'a, 'b> {
    /// まとめのプロパティの長い名前の番号
    short: Shorthands,
    doc: &'a XmlDoc,
    input: &'a [u8],
    sheet: &'a Sheet,
    infos: &'a [DeclInfo],
    /// `font` の宣言の長い名前の範囲
    fonts: &'a [FontRec],
    /// まとめた層ごとの順位
    ranks: &'a [u32],
    /// 表示の領域の単位を読めるか
    viewport: bool,
    budget: &'b Budget<'b>,
    slots: MVec<'b, Slot>,
    /// この要素で候補を置いたプロパティ（書き出すときに並べる）
    touched: MVec<'b, u16>,
    /// 決めている要素（なければ NONE）
    cur: u32,
    specs: MVec<'b, Specified>,
    index: MVec<'b, u32>,
    warns: MVec<'b, Warn>,
}

impl Out<'_, '_> {
    fn node(&self) -> Option<&Node> {
        self.doc.node(self.cur)
    }

    /// 候補を置く（強ければ入れ替える）。まとめのプロパティ（`all`・`marker`・`font`・`background`）は長い名前に分ける。
    /// `revert-layer` は候補にしない（弱い宣言に戻す）。
    fn offer(&mut self, prop: PropertyId, key: Key, spec: Specified, font: Option<FontRec>) -> Result<(), Error> {
        self.budget.spend(cost::OP_STEP)?;
        if spec.wide == WIDE_REVERT_LAYER {
            return Ok(());
        }
        // `all`（全体の言葉だけを受ける）は、direction・unicode-bidi と長い名前に分けるまとめを除くどの名前にも同じ言葉を
        // 置く（Chrome と同じ。分けるまとめの長い名前はそれぞれの名前で置く。分けずに持つまとめ（`offset`・`mask`・
        // `overflow` など）はそのまま置く。ユーザーの判断。単位 3 の 5 回目のレビュー）
        if PROPS.get(usize::from(prop)).is_some_and(|p| p.name == b"all") {
            for (i, p) in PROPS.iter().enumerate() {
                self.budget.spend(cost::OP_STEP)?;
                if matches!(p.name, b"all" | b"direction" | b"unicode-bidi" | b"font" | b"background" | b"marker") {
                    continue;
                }
                let Ok(id) = PropertyId::try_from(i) else { continue };
                // CSS の箱のプロパティは全体の言葉をそのまま置く（`all` の分けた言葉は論理の大きさでも幅の属性に勝たない。
                // Chrome と同じ）
                if matches!(p.kind, crate::style::props::Kind::CssBox(_)) {
                    self.put(id, key, Specified { prop: id, ..spec })?;
                } else {
                    self.offer_one(id, key, Specified { prop: id, ..spec }, None)?;
                }
            }
            return Ok(());
        }
        self.offer_one(prop, key, spec, font)
    }

    /// `all` のほかの名前の候補を置く（`offer` の続き）。
    fn offer_one(&mut self, prop: PropertyId, key: Key, spec: Specified, font: Option<FontRec>) -> Result<(), Error> {
        let name = PROPS.get(usize::from(prop)).map_or(&[][..], |p| p.name);
        if name == b"marker" {
            for id in self.short.markers.into_iter().flatten() {
                self.put(id, key, Specified { prop: id, ..spec })?;
            }
            return Ok(());
        }
        if name == b"font" {
            return self.offer_font(key, spec, font);
        }
        if name == b"background" {
            return self.offer_background(key, spec, font);
        }
        // CSS の箱のプロパティ：同じ絵にならない印を置く（求めていなければ同じ絵にならないとみなす。値は読み直さない）。
        // rect の論理の大きさは全体の言葉（initial・unset・revert・inherit）でも幅・高さの属性に勝つので印を置く（Chrome と
        // 同じ。単位 3 の 6 回目のレビュー）
        if let Some(crate::style::props::Kind::CssBox(box_rule)) = PROPS.get(usize::from(prop)).map(|p| p.kind) {
            let flag = if spec.wide == WIDE_NONE {
                font.map_or(BOX_ON, |r| r.pad[0])
            } else if box_rule == crate::style::props::BoxRule::Logical {
                BOX_ON
            } else {
                0
            };
            return self.put(prop, key, Specified { pad: [flag, 0, 0], ..spec });
        }
        // 長い名前の `background-image`：画像の層の印を置く（まとめから分けたものと同じ形。値は読み直さない）
        if name == b"background-image" && spec.wide == WIDE_NONE {
            let flags = font.map_or(0, |r| r.pad[0]);
            return self.put(prop, key, Specified { pad: [BG_IMAGE_FROM_SHORTHAND | flags, 0, 0], ..spec });
        }
        // use・symbol・入れ子の svg の width・height は CSS のプロパティでない（木を作る段が属性として読む）
        if (name == b"width" || name == b"height")
            && let Some(n) = self.node()
            && let local = n.local.of(self.input)
            && (local == b"use" || local == b"symbol" || (local == b"svg" && n.parent != NONE))
        {
            return Ok(());
        }
        self.put(prop, key, spec)
    }

    /// `font` を長い名前に分ける（書いていない長い名前と、`font` が初期の値にするほかの長い名前は `initial`）。長い名前の
    /// 範囲は宣言ごとに 1 度だけ求めたもの（`font`）。全体の言葉（`font: inherit` など）は、長い名前もみな同じ言葉。
    fn offer_font(&mut self, key: Key, spec: Specified, font: Option<FontRec>) -> Result<(), Error> {
        let rec = if spec.wide == WIDE_NONE {
            match font {
                Some(r) => Some(r),
                None => return Ok(()),
            }
        } else {
            None
        };
        for (k, id) in self.short.font_longhands.into_iter().enumerate() {
            self.budget.spend(cost::OP_STEP)?;
            let Some(id) = id else { continue };
            let s = match rec {
                None => Specified { prop: id, ..spec },
                Some(r) => {
                    match r.spans.get(k).filter(|_| k < 7 && r.present & 1u8.checked_shl(k as u32).unwrap_or(0) != 0) {
                        Some(&range) => Specified { prop: id, value: range, wide: WIDE_NONE, ..spec },
                        None => Specified { prop: id, value: Span::default(), wide: WIDE_INITIAL, ..spec },
                    }
                }
            };
            self.put(id, key, s)?;
        }
        Ok(())
    }

    /// `background` を `background-color` と `background-image` に分ける（色は最後の層の色の範囲。色がなければ `initial`。
    /// 画像は値の範囲を置かず、画像の層の印を `pad[0]` に `BG_IMAGE_FROM_SHORTHAND` と合わせて置く。全体の言葉はそのまま。
    /// ほかの長い名前は描かないので置かない。範囲と印は宣言ごとに 1 度だけ求めたもの（`rec`）。ADR-99 (4)・ADR-102）。
    fn offer_background(&mut self, key: Key, spec: Specified, rec: Option<FontRec>) -> Result<(), Error> {
        let (Some(color_id), Some(image_id)) = (self.short.background_color, self.short.background_image) else {
            return Ok(());
        };
        if spec.wide != WIDE_NONE {
            self.put(color_id, key, Specified { prop: color_id, ..spec })?;
            return self.put(image_id, key, Specified { prop: image_id, ..spec });
        }
        let Some(r) = rec else { return Ok(()) };
        let color = if r.present & 1 != 0 {
            Specified { prop: color_id, value: r.spans[0], ..spec }
        } else {
            Specified { prop: color_id, value: Span::default(), wide: WIDE_INITIAL, ..spec }
        };
        self.put(color_id, key, color)?;
        // 画像の層の印（グラデーション・data: の画像は木を作る段で NotYet。単位 3 の 4 回目のレビュー：印を色の宣言に
        // 乗せると、後ろの background-color や長い名前の background-image で消えた）
        self.put(
            image_id,
            key,
            Specified {
                prop: image_id,
                value: Span::default(),
                pad: [BG_IMAGE_FROM_SHORTHAND | r.pad[0], 0, 0],
                ..spec
            },
        )
    }

    fn put(&mut self, prop: PropertyId, key: Key, spec: Specified) -> Result<(), Error> {
        let Some(s) = self.slots.get_mut(usize::from(prop)) else { return Ok(()) };
        if s.set != 0 && key < s.key() {
            return Ok(());
        }
        let first = s.set == 0;
        *s = Slot {
            set: 1,
            pad: [0; 3],
            key_tier: u32::from(key.tier),
            key_layer: key.layer,
            key_spec: key.spec,
            key_order: key.order,
            key_idx: key.idx,
            spec,
        };
        if first {
            self.touched.push(prop)?;
        }
        Ok(())
    }

    /// 要素の決めた指定の値を書き出し、次の要素のために空にする。プレゼンテーション属性と style 属性をここで足す。
    fn finish_element(&mut self) -> Result<(), Error> {
        if self.cur == NONE {
            return Ok(());
        }
        let e = self.cur;
        let Some(&n) = self.node() else { return Ok(()) };
        if n.ns == NS_SVG {
            self.attributes(e, &n)?;
        }
        // 節の始まりを書く（飛ばした文字の節を含めて、この要素まで）
        let start = self.specs.len() as u32;
        while self.index.len() <= e as usize {
            self.budget.spend(cost::OP_STEP)?;
            self.index.push(start)?;
        }
        // 候補を置いたプロパティだけを、番号の順に書き出す
        stable_sort(self.budget, self.touched.as_mut_slice(), |a, b| a < b)?;
        for k in 0..self.touched.len() {
            self.budget.spend(cost::OP_STEP)?;
            let Some(&id) = self.touched.get(k) else { continue };
            let Some(s) = self.slots.get_mut(usize::from(id)) else { continue };
            s.set = 0;
            let spec = Specified { node: e, ..s.spec };
            if n.ns == NS_SVG {
                self.specs.push(spec)?;
            }
        }
        self.touched.clear();
        self.cur = NONE;
        Ok(())
    }

    /// プレゼンテーション属性と style 属性を候補にする。
    fn attributes(&mut self, e: u32, n: &Node) -> Result<(), Error> {
        let doc = self.doc;
        let input = self.input;
        let local = n.local.of(input);
        for (k, a) in doc.attrs_of(n).iter().enumerate() {
            // 名前でプロパティを引く手間も払う
            self.budget.spend(cost::LOOKUP)?;
            if a.ns != NS_NONE {
                continue;
            }
            let name = a.local.of(input);
            let src = if a.flags & FLAG_IN_STORE != 0 { SRC_STORE } else { SRC_INPUT };
            let buf = if src == SRC_STORE { doc.store.as_slice() } else { input };
            if name == b"style" {
                self.style_attribute(e, n, buf, a.value, src)?;
                continue;
            }
            // transform 属性は transform プロパティのプレゼンテーション属性（SVG の書き方）
            let id = if name == b"transform" { lookup(b"transform", false) } else { lookup(name, false) };
            let Some(id) = id else { continue };
            let Some(p) = prop(id) else { continue };
            let outer = n.parent == NONE;
            if !(p.attr || name == b"transform")
                || !geometry_ok(local, name, outer)
                || (name == b"fill" && animation(local))
            {
                continue;
            }
            let w = wide(buf, a.value, self.budget)?;
            let cx = Ctx { attr: true, unitless: true, viewport: self.viewport };
            if w.is_none() && !valid(p.kind, buf, a.value, cx, self.budget)? {
                self.warns.push(Warn { pos: a.pos, kind: WarningKind::InvalidValue, pad: 0 })?;
                continue;
            }
            let spec = Specified {
                node: e,
                value: a.value,
                prop: id,
                src,
                origin: ORIGIN_ATTR,
                wide: wide_code(w),
                pad: [0; 3],
            };
            self.offer(id, Key { tier: 0, idx: k as u32, ..Key::default() }, spec, None)?;
        }
        Ok(())
    }

    fn style_attribute(&mut self, e: u32, n: &Node, buf: &[u8], value: Span, src: u8) -> Result<(), Error> {
        let budget = self.budget;
        let mut lex = Lexer::over(buf, value, budget);
        let mut list: MVec<'_, Decl> = MVec::new(budget);
        parse_declarations(&mut lex, buf, false, budget, &mut |it| match it {
            Item::Decl(d) => list.push(d),
            Item::Nested => Ok(()),
        })?;
        for (k, d) in list.as_slice().iter().enumerate() {
            budget.spend(cost::OP_STEP)?;
            let info = check_decl(buf, d, self.viewport, budget)?;
            if info.prop == u16::MAX {
                continue;
            }
            if info.ok == 0 {
                self.warns.push(Warn { pos: n.pos, kind: WarningKind::InvalidValue, pad: 0 })?;
                continue;
            }
            let tier = if d.important != 0 { 4 } else { 2 };
            let spec = Specified {
                node: e,
                value: d.value,
                prop: info.prop,
                src,
                origin: ORIGIN_STYLE,
                wide: info.wide,
                pad: [0; 3],
            };
            // `style` 属性の `font` は長い名前の範囲を、`background` は色の範囲を、ここで（宣言ごとに 1 度）求める
            let font = if self.short.font == Some(info.prop) && info.wide == WIDE_NONE {
                let cx = Ctx { attr: false, unitless: false, viewport: self.viewport };
                font_parts(buf, d.value, cx, budget)?.map(|p| FontRec::of(&p))
            } else if self.short.background == Some(info.prop) && info.wide == WIDE_NONE {
                crate::style::valid::background_color(buf, d.value, budget)?.map(FontRec::background)
            } else if self.short.background_image == Some(info.prop) && info.wide == WIDE_NONE {
                crate::style::valid::image_list(buf, d.value, budget)?.map(FontRec::image)
            } else if info.wide == WIDE_NONE {
                box_flag(info.prop, buf, d.value, budget)?.map(FontRec::image)
            } else {
                None
            };
            self.offer(info.prop, Key { tier, idx: k as u32, ..Key::default() }, spec, font)?;
        }
        Ok(())
    }
}

impl MatchSink for Out<'_, '_> {
    fn element(&mut self, node: u32) -> Result<(), Error> {
        self.finish_element()?;
        self.cur = node;
        Ok(())
    }

    fn rule(&mut self, rule: u32, spec: u32) -> Result<(), Error> {
        let sheet = self.sheet;
        let Some(r) = sheet.rules.as_slice().get(rule as usize) else { return Ok(()) };
        // 層のない規則は最も強い
        let rank = if r.layer == NONE {
            u32::MAX
        } else {
            let canon = sheet.layers.as_slice().get(r.layer as usize).map_or(NONE, |l| l.canon);
            self.ranks.get(canon as usize).copied().unwrap_or(0)
        };
        for k in r.decls.start..r.decls.end {
            self.budget.spend(cost::OP_STEP)?;
            let Some(info) = self.infos.get(k as usize).copied() else { continue };
            let Some(d) = sheet.decls.as_slice().get(k as usize) else { continue };
            if info.prop == u16::MAX || info.ok == 0 {
                continue;
            }
            let important = d.important != 0;
            let key = Key {
                tier: if important { 3 } else { 1 },
                layer: if important { u32::MAX.saturating_sub(rank) } else { rank },
                spec,
                order: r.order,
                idx: k,
            };
            let s = Specified {
                node: self.cur,
                value: d.value,
                prop: info.prop,
                src: SRC_SHEET,
                origin: ORIGIN_RULE,
                wide: info.wide,
                pad: [0; 3],
            };
            let font = self.fonts.get(info.font as usize).copied();
            self.offer(info.prop, key, s, font)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
