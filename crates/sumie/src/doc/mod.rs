//! 段 3：文書（architecture.md §4.3）。読む段の全体（xml → css → style → doc）をまとめ、[`Document`] を作る。
//!
//! 読む段は 1 本の道（ADR-70）：XML を数える → XML の節を作る → `<style>` を規則の並びに読む → 条件を照らしてカスケード
//! する → 報告をまとめる。道の全体を `Limits::parse_work`・`parse_bytes` を上限にした 1 つの `Budget` で払い、段の中では
//! 「数えてから作る」（XML の節・規則の並びは数えた容量で取る）。`estimate_parse` は同じ道を同じ上限で走らせて、払った手間・
//! 確保の山・持ち続ける量を返し（見積もりは実際とぴったり同じ）、`parse` は同じ道を走らせて結果を持つ。同じ道なので、
//! `estimate_parse` と `parse` は同じ誤りを返し（D2）、`estimate_parse` が通れば `parse` も通る（確保器の失敗と中止を除く）。

use crate::budget::{Budget, Estimate};
use crate::cancel::Cancel;
use crate::cost;
use crate::css::sheet::{Sheet, SheetBuilder};
use crate::error::{Error, Stage};
use crate::limits::Limits;
use crate::mem::MVec;
use crate::report::{Report, ReportBuilder};
use crate::style::ParseOptions;
use crate::style::cascade::{Styles, cascade};
use crate::style::cond::Env;
use crate::values::{Length, Unit, Values, view_box};
use crate::xml::lex::Span;
use crate::xml::{self, KIND_ELEMENT, KIND_TEXT, NS_NONE, NS_SVG, XmlDoc};

/// 読んだ文書（入力を借りる。名前と値は写さない）。
pub struct Document<'s> {
    input: &'s [u8],
    xml: XmlDoc,
    sheet: Sheet,
    styles: Styles,
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

    /// 持ち続ける量（`Estimate::result_bytes` と同じ）。
    pub(crate) fn held_bytes(&self) -> u64 {
        [self.xml.bytes(), self.sheet.bytes(), self.styles.bytes(), self.report.bytes()]
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
    let env =
        Env { color_scheme: opts.style.color_scheme, size: attribute_size(&doc, svg, opts.style.font_size, budget)? };
    let styles = cascade(&doc, svg, &sheet, &env, budget)?;
    let report = report(&sheet, &styles, budget)?;
    Ok(Document { input: svg, xml: doc, sheet, styles, limits: opts.limits, report })
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

/// `@media` と表示の領域の単位（vw など）に使う文書の大きさ（FR-OUT-01 のうち、属性で決まる所。ADR-74）：外側の `svg` の
/// `width`・`height`（絶対の長さか単位のない数。em・ex・ch・ic・cap・rem は `font_size` で解く）。`viewBox` があれば、% は
/// viewBox の大きさに対する割合、片方だけ（か壊れた値）なら viewBox の縦横の比で補い、どちらもなければ viewBox の大きさ。
/// viewBox がなく、どちらかが欠ける・%・壊れた値なら、大きさは中身で決まる（None）。0 か負も None（木を作る段で断る）。
fn attribute_size(
    doc: &XmlDoc,
    input: &[u8],
    font_size: f64,
    budget: &Budget<'_>,
) -> Result<Option<(f64, f64)>, Error> {
    let budget = budget_ref(budget);
    let Some(root) = doc.nodes.as_slice().first().filter(|n| n.kind == KIND_ELEMENT) else {
        return Ok(None);
    };
    if root.ns != NS_SVG || root.local.of(input) != b"svg" {
        return Ok(None);
    }
    /// 幅か高さの属性：絶対の大きさ、割合、なしか壊れた値。
    enum Dim {
        Abs(f64),
        Pct(f64),
        No,
    }
    let len = |name: &[u8]| -> Result<Dim, Error> {
        let Some(a) = doc.find_attr(input, root, NS_NONE, name, budget)? else { return Ok(Dim::No) };
        let (buf, span) = attr_buf(doc, input, a);
        let mut v = Values::new(buf, span, budget);
        let l: Option<Length> = v.length(true)?;
        if !v.at_end()? {
            return Ok(Dim::No);
        }
        let Some(l) = l else { return Ok(Dim::No) };
        let px = match l.unit {
            Unit::Percent => return Ok(Dim::Pct(l.value)),
            Unit::Em | Unit::Rem | Unit::Ic => Some(l.value * font_size),
            Unit::Ex | Unit::Ch => Some(l.value * font_size * 0.5),
            Unit::Cap => Some(l.value * font_size * 0.7),
            _ => l.absolute_px(),
        };
        Ok(px.filter(|x| x.is_finite()).map_or(Dim::No, Dim::Abs))
    };
    let w = len(b"width")?;
    let h = len(b"height")?;
    let vb = match doc.find_attr(input, root, NS_NONE, b"viewBox", budget)? {
        Some(a) => {
            let (buf, span) = attr_buf(doc, input, a);
            view_box(buf, span, budget)?.filter(|v| v.w > 0.0 && v.h > 0.0)
        }
        None => None,
    };
    let size = match (w, h, vb) {
        (Dim::Abs(w), Dim::Abs(h), _) => Some((w, h)),
        (w, h, Some(v)) => {
            let rw = match w {
                Dim::Abs(x) => Some(x),
                Dim::Pct(p) => Some(v.w * p / 100.0),
                Dim::No => None,
            };
            let rh = match h {
                Dim::Abs(x) => Some(x),
                Dim::Pct(p) => Some(v.h * p / 100.0),
                Dim::No => None,
            };
            match (rw, rh) {
                (Some(w), Some(h)) => Some((w, h)),
                (Some(w), None) => Some((w, w * v.h / v.w)),
                (None, Some(h)) => Some((h * v.w / v.h, h)),
                (None, None) => Some((v.w, v.h)),
            }
        }
        _ => None,
    };
    Ok(size.filter(|&(w, h)| w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0))
}

/// 属性の値の置き場と範囲。
fn attr_buf<'a>(doc: &'a XmlDoc, input: &'a [u8], a: &xml::Attr) -> (&'a [u8], Span) {
    if a.flags & xml::FLAG_IN_STORE != 0 { (doc.store.as_slice(), a.value) } else { (input, a.value) }
}

/// 規則の並びの覚えとカスケードの警告を報告にする（数えた容量で）。
fn report(sheet: &Sheet, styles: &Styles, budget: &Budget<'_>) -> Result<Report, Error> {
    let budget = budget_ref(budget);
    let notes = sheet.notes.as_slice();
    // 数える（覚えごとに払う）
    let mut unread = 0usize;
    let mut href_bytes = 0usize;
    for n in notes {
        budget.spend(cost::OP_STEP)?;
        if n.unread != 0 {
            unread = unread.saturating_add(1);
            href_bytes = href_bytes.saturating_add(n.href.len());
        }
    }
    let warns = notes.len().saturating_sub(unread).saturating_add(styles.warnings.len());
    let mut r = ReportBuilder::with_capacity(budget, warns, unread, href_bytes)?;
    // 読まなかった参照の href は escape を解いて名前の置き場にある
    let text = sheet.names.as_slice();
    for n in notes {
        budget.spend(cost::OP_STEP)?;
        if n.unread != 0 {
            r.unread(n.href.of(text), u64::from(n.pos))?;
        } else {
            r.warn(n.kind, Some(u64::from(n.pos)))?;
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
