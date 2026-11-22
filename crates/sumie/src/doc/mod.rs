//! 段 3：文書（architecture.md §4.3）。読む段の全体（xml → css → style → doc）をまとめ、[`Document`] を作る。
//!
//! 読む段は 1 本の道（ADR-70）：XML を数える → XML の節を作る → `<style>` を規則の並びに読む → 条件を照らしてカスケード
//! する → 報告をまとめる。道の全体を `Limits::parse_work`・`parse_bytes` を上限にした 1 つの `Budget` で払い、段の中では
//! 「数えてから作る」（XML の節・規則の並びは数えた容量で取る）。`estimate_parse` は同じ道を同じ上限で走らせて、払った手間・
//! 確保の山・持ち続ける量を返し（見積もりは実際とぴったり同じ）、`parse` は同じ道を走らせて結果を持つ。同じ道なので、
//! `estimate_parse` と `parse` は同じ誤りを返し（D2）、`estimate_parse` が通れば `parse` も通る（確保器の失敗と中止を除く）。

pub(crate) mod ids;
pub(crate) mod refs;
pub(crate) mod size;

use crate::budget::{Budget, Estimate};
use crate::cancel::Cancel;
use crate::cost;
use crate::css::sheet::{Sheet, SheetBuilder};
use crate::error::{Error, Stage};
use crate::limits::Limits;
use crate::mem::MVec;
use crate::report::{Report, ReportBuilder};
use crate::style::ParseOptions;
use crate::style::cascade::{SRC_SHEET, SRC_STORE, WIDE_NONE};
use crate::style::cascade::{Styles, cascade};
use crate::style::cond::Env;
use crate::style::props::lookup;
use crate::values::{Unit, Values};
use crate::xml::{self, KIND_ELEMENT, KIND_TEXT, NS_NONE, NS_SVG, XmlDoc};
use ids::Ids;
use refs::Refs;
pub use size::{PixelSize, Size};
use size::{SizeRule, size_rule};

/// 読んだ文書（入力を借りる。名前と値は写さない）。
pub struct Document<'s> {
    input: &'s [u8],
    xml: XmlDoc,
    sheet: Sheet,
    styles: Styles,
    ids: Ids,
    refs: Refs,
    size: SizeRule,
    limits: Limits,
    report: Report,
}

impl core::fmt::Debug for Document<'_> {
    /// 大きさだけを出す（入力と節を全部出すと、大きな文書で記録が膨らむ）。
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Document")
            .field("input_bytes", &self.input.len())
            .field("nodes", &self.xml.nodes.len())
            .field("held_bytes", &self.held_bytes())
            .finish_non_exhaustive()
    }
}

impl<'s> Document<'s> {
    /// この段までの警告・読まなかった参照。
    pub fn report(&self) -> &Report {
        &self.report
    }

    /// ユーザー単位の大きさ（FR-OUT-01）。`None` は中身の外接矩形で決まる（木を作るまで決まらない。大きさは木から読む。
    /// ADR-63 (3)）。
    pub fn size(&self) -> Option<Size> {
        self.size.known()
    }

    /// 倍率 `scale` で描くときの画素の大きさ（切り上げる）。`None` は [`Document::size`] と同じ。倍率が有限で正でないか、
    /// 画素の数が u32 を超えれば `Invalid`（`Argument`）。
    pub fn pixel_size(&self, scale: f64) -> Result<Option<PixelSize>, Error> {
        match self.size.known() {
            Some(s) => Ok(Some(size::pixel_size(s, scale)?)),
            None => {
                // 倍率の誤りは大きさが決まらなくても返す
                size::pixel_size(Size { width: 1.0, height: 1.0 }, scale)?;
                Ok(None)
            }
        }
    }

    /// 持ち続ける量（`Estimate::result_bytes` と同じ）。
    pub(crate) fn held_bytes(&self) -> u64 {
        [
            self.xml.bytes(),
            self.sheet.bytes(),
            self.styles.bytes(),
            self.ids.bytes(),
            self.refs.bytes(),
            self.report.bytes(),
        ]
        .iter()
        .fold(0u64, |a, &b| a.saturating_add(b))
    }
}

#[allow(dead_code, reason = "M1 の doc の単位と、木を作る段（tree）から使う")]
impl<'s> Document<'s> {
    pub(crate) fn input(&self) -> &'s [u8] {
        self.input
    }

    pub(crate) fn xml(&self) -> &XmlDoc {
        &self.xml
    }

    pub(crate) fn sheet(&self) -> &Sheet {
        &self.sheet
    }

    pub(crate) fn styles(&self) -> &Styles {
        &self.styles
    }

    pub(crate) fn limits(&self) -> &Limits {
        &self.limits
    }

    pub(crate) fn ids(&self) -> &Ids {
        &self.ids
    }

    pub(crate) fn refs(&self) -> &Refs {
        &self.refs
    }
}

/// 勘定の見張りの値（ファズと試験が見る。`parse_watch`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Watch {
    pub(crate) work: u64,
    pub(crate) peak: u64,
    /// 1 回の払いの最大と、返しすぎ
    pub(crate) max_spend: u64,
    pub(crate) over_released: bool,
}

/// 読む段の道。
fn pipeline<'s>(svg: &'s [u8], opts: &ParseOptions<'_>, budget: &Budget<'_>) -> Result<Document<'s>, Error> {
    opts.style.check()?;
    let budget = budget_ref(budget);
    let limits = &opts.limits;
    let counts = xml::count(svg, limits, budget, Stage::Parse)?;
    let doc = xml::build(svg, &counts, limits, budget, Stage::Parse)?;
    let sheet = read_sheets(&doc, svg, budget)?;
    // `@media` と vw などに使う大きさは既定の文字の大きさで解く（根の文字の大きさは `@media` に依りうる。ADR-74 (7)）
    let media_size = size_rule(&doc, svg, opts.style.font_size, budget)?;
    let env = Env { color_scheme: opts.style.color_scheme, size: media_size.known().map(|s| (s.width, s.height)) };
    let styles = cascade(&doc, svg, &sheet, &env, budget)?;
    // 文書の大きさの em などは根の要素の文字の大きさで解く（usvg と同じ）
    let root_font = root_font_size(&doc, svg, &sheet, &styles, opts.style.font_size, budget)?;
    let size = if root_font == opts.style.font_size { media_size } else { size_rule(&doc, svg, root_font, budget)? };
    let ids = Ids::build(&doc, svg, budget)?;
    let refs = refs::build(&doc, svg, &sheet, &styles, &ids, opts.style.languages, budget)?;
    let report = report(&doc, svg, &sheet, &styles, &refs, budget)?;
    Ok(Document { input: svg, xml: doc, sheet, styles, ids, refs, size, limits: opts.limits, report })
}

/// 根の要素の文字の大きさ（px）。指定の値が長さか割合ならそれ（em・ex・% は既定の文字の大きさに対して）、ほか（言葉・
/// 全体の言葉・指定なし）は既定の文字の大きさ。
fn root_font_size(
    doc: &XmlDoc,
    input: &[u8],
    sheet: &Sheet,
    styles: &Styles,
    default: f64,
    budget: &Budget<'_>,
) -> Result<f64, Error> {
    let budget = budget_ref(budget);
    let Some(id) = lookup(b"font-size", false) else { return Ok(default) };
    let Some(spec) = styles.of(0).iter().find(|s| s.prop == id) else { return Ok(default) };
    if spec.wide != WIDE_NONE {
        return Ok(default);
    }
    let buf = match spec.src {
        SRC_SHEET => sheet.text.as_slice(),
        SRC_STORE => doc.store.as_slice(),
        _ => input,
    };
    let mut v = Values::new(buf, spec.value, budget);
    let Some(l) = v.length(true)? else { return Ok(default) };
    let px = match l.unit {
        Unit::Em | Unit::Rem | Unit::Ic => Some(l.value * default),
        Unit::Ex | Unit::Ch => Some(l.value * default * 0.5),
        Unit::Cap => Some(l.value * default * 0.7),
        Unit::Percent => Some(l.value * default / 100.0),
        _ => l.absolute_px(),
    };
    Ok(px.filter(|x| x.is_finite() && *x >= 0.0).unwrap_or(default))
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 文書の `<style>`（SVG の名前空間・`type` がないか空か `text/css`）を文書の順に規則の並びに読む。中身は子の文字と
/// CDATA をつないだもの。数えてから、数えた容量で読む。
fn read_sheets(doc: &XmlDoc, input: &[u8], budget: &Budget<'_>) -> Result<Sheet, Error> {
    let budget = budget_ref(budget);
    let mut scratch: MVec<'_, u8> = MVec::new(budget);
    let counts = {
        let mut b = SheetBuilder::new(budget);
        each_style(doc, input, budget, &mut scratch, &mut |css, media, pos| b.add(css, media, pos))?;
        b.counts()
    };
    let mut b = SheetBuilder::with_counts(budget, &counts)?;
    each_style(doc, input, budget, &mut scratch, &mut |css, media, pos| b.add(css, media, pos))?;
    b.finish()
}

/// `<style>` を受ける口（中身・media 属性・位置）。
type StyleFn<'f> = &'f mut dyn FnMut(&[u8], Option<&[u8]>, u32) -> Result<(), Error>;

/// `<style>` ごとに（中身・media 属性・位置）を渡す。
fn each_style(
    doc: &XmlDoc,
    input: &[u8],
    budget: &Budget<'_>,
    scratch: &mut MVec<'_, u8>,
    f: StyleFn<'_>,
) -> Result<(), Error> {
    let nodes = doc.nodes.as_slice();
    for n in nodes {
        budget.spend(cost::OP_STEP)?;
        if n.kind != KIND_ELEMENT || n.ns != NS_SVG || n.local.of(input) != b"style" {
            continue;
        }
        // `type` は空か `text/css`（大文字小文字を問わない。前後の空白は捨てない。Chrome と同じ）
        if let Some(t) = doc.attr(input, n, NS_NONE, b"type", budget)?
            && !(t.is_empty() || t.eq_ignore_ascii_case(b"text/css"))
        {
            continue;
        }
        scratch.clear();
        let mut c = n.first_child;
        while c != xml::NONE {
            budget.spend(cost::OP_STEP)?;
            let Some(k) = doc.node(c) else { break };
            if k.kind == KIND_TEXT {
                scratch.append_slice(doc.text(input, k))?;
            }
            c = k.next_sibling;
        }
        let media = doc.attr(input, n, NS_NONE, b"media", budget)?;
        f(scratch.as_slice(), media, n.pos)?;
    }
    Ok(())
}

/// 規則の並びの覚え・カスケードの警告・参照の覚え（輪と読まなかった参照）を報告にする（数えた容量で）。
fn report(
    doc: &XmlDoc,
    input: &[u8],
    sheet: &Sheet,
    styles: &Styles,
    refs: &Refs,
    budget: &Budget<'_>,
) -> Result<Report, Error> {
    let budget = budget_ref(budget);
    let notes = sheet.notes.as_slice();
    let ref_notes = refs.notes.as_slice();
    // 数える（覚えごとに払う）
    let mut unread = 0usize;
    let mut href_bytes = 0usize;
    let mut warns = styles.warnings.len();
    for n in notes {
        budget.spend(cost::OP_STEP)?;
        if n.unread != 0 {
            unread = unread.saturating_add(1);
            href_bytes = href_bytes.saturating_add(n.href.len());
        } else {
            warns = warns.saturating_add(1);
        }
    }
    for n in ref_notes {
        budget.spend(cost::OP_STEP)?;
        if n.unread != 0 {
            unread = unread.saturating_add(1);
            href_bytes = href_bytes.saturating_add(n.href.len());
        } else {
            warns = warns.saturating_add(1);
        }
    }
    let mut r = ReportBuilder::with_capacity(budget, warns, unread, href_bytes)?;
    // 規則の並びの読まなかった参照の href は escape を解いて名前の置き場にある
    let names = sheet.names.as_slice();
    for n in notes {
        budget.spend(cost::OP_STEP)?;
        if n.unread != 0 {
            r.unread(n.href.of(names), u64::from(n.pos))?;
        } else {
            r.warn(n.kind, Some(u64::from(n.pos)))?;
        }
    }
    for n in ref_notes {
        budget.spend(cost::OP_STEP)?;
        if n.unread != 0 {
            let buf = match n.src {
                SRC_SHEET => sheet.text.as_slice(),
                SRC_STORE => doc.store.as_slice(),
                _ => input,
            };
            r.unread(n.href.of(buf), u64::from(n.pos))?;
        } else {
            r.warn(crate::report::WarningKind::ReferenceCycle, Some(u64::from(n.pos)))?;
        }
    }
    for w in styles.warnings.as_slice() {
        budget.spend(cost::OP_STEP)?;
        r.warn(w.kind, Some(u64::from(w.pos)))?;
    }
    r.finish()
}

/// 読む段の見積もり（architecture.md §3）。読む段の道を `parse` と同じ上限で走らせ、払った手間・確保の山・持ち続ける量を
/// 返す（ADR-70）。
pub(crate) fn estimate_parse(svg: &[u8], opts: &ParseOptions<'_>, cancel: &dyn Cancel) -> Result<Estimate, Error> {
    let budget = Budget::new(Stage::Parse, opts.limits.parse_work, opts.limits.parse_bytes, cancel);
    let doc = pipeline(svg, opts, &budget)?;
    Ok(Estimate { work: budget.work(), peak_bytes: budget.peak(), result_bytes: doc.held_bytes() })
}

/// 読む（architecture.md §3）。
pub(crate) fn parse<'s>(svg: &'s [u8], opts: &ParseOptions<'_>, cancel: &dyn Cancel) -> Result<Document<'s>, Error> {
    parse_watch(svg, opts, cancel, &mut Watch::default())
}

/// [`parse`] と同じで、勘定の見張りの値を `watch` に書く。
pub(crate) fn parse_watch<'s>(
    svg: &'s [u8],
    opts: &ParseOptions<'_>,
    cancel: &dyn Cancel,
    watch: &mut Watch,
) -> Result<Document<'s>, Error> {
    let budget = Budget::new(Stage::Parse, opts.limits.parse_work, opts.limits.parse_bytes, cancel);
    let r = pipeline(svg, opts, &budget);
    watch.work = budget.work();
    watch.peak = budget.peak();
    watch.max_spend = budget.max_spend();
    watch.over_released = budget.over_released();
    r
}

#[cfg(test)]
mod tests;
