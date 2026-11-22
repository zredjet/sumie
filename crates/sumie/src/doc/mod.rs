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
    let env = Env { color_scheme: opts.style.color_scheme, size: attribute_size(&doc, svg, budget)? };
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
        if let Some(t) = doc.attr(input, n, NS_NONE, b"type") {
            let t = trim(t);
            if !(t.is_empty() || t.eq_ignore_ascii_case(b"text/css")) {
                continue;
            }
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
        let media = doc.attr(input, n, NS_NONE, b"media");
        f(scratch.as_slice(), media, n.pos)?;
    }
    Ok(())
}

fn trim(mut s: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = s
        && matches!(first, b' ' | b'\t' | b'\n' | b'\r')
    {
        s = rest;
    }
    while let [rest @ .., last] = s
        && matches!(last, b' ' | b'\t' | b'\n' | b'\r')
    {
        s = rest;
    }
    s
}

/// `@media` を照らす文書の大きさ：外側の `svg` の `width`・`height` 属性（絶対の長さか単位のない数）と `viewBox` から
/// （ADR-66 (2)。CSS の width・height は見ない）。中身で決まるとき・相対の長さのときは None。
fn attribute_size(doc: &XmlDoc, input: &[u8], budget: &Budget<'_>) -> Result<Option<(f64, f64)>, Error> {
    let budget = budget_ref(budget);
    let Some(root) = doc.nodes.as_slice().iter().find(|n| n.kind == KIND_ELEMENT && n.parent == xml::NONE) else {
        return Ok(None);
    };
    if root.ns != NS_SVG || root.local.of(input) != b"svg" {
        return Ok(None);
    }
    let len = |name: &[u8]| -> Result<Option<Option<f64>>, Error> {
        // 外の None は属性がない、内の None は解けない長さ
        let Some(a) = doc.attrs_of(root).iter().find(|a| a.ns == NS_NONE && a.local.of(input) == name) else {
            return Ok(None);
        };
        let (buf, span) = attr_buf(doc, input, a);
        let mut v = Values::new(buf, span, budget);
        let l: Option<Length> = v.length(true)?;
        let ok = v.at_end()?;
        Ok(Some(l.filter(|_| ok).filter(|l| l.unit != Unit::Percent).and_then(|l| l.absolute_px())))
    };
    let w = len(b"width")?;
    let h = len(b"height")?;
    let vb = match doc.attrs_of(root).iter().find(|a| a.ns == NS_NONE && a.local.of(input) == b"viewBox") {
        Some(a) => {
            let (buf, span) = attr_buf(doc, input, a);
            view_box(buf, span, budget)?.filter(|v| v.w > 0.0 && v.h > 0.0)
        }
        None => None,
    };
    Ok(match (w, h, vb) {
        (Some(Some(w)), Some(Some(h)), _) => Some((w, h)),
        (Some(Some(w)), None, Some(v)) => Some((w, w * v.h / v.w)),
        (None, Some(Some(h)), Some(v)) => Some((h * v.w / v.h, h)),
        (None, None, Some(v)) => Some((v.w, v.h)),
        _ => None,
    })
}

/// 属性の値の置き場と範囲。
fn attr_buf<'a>(doc: &'a XmlDoc, input: &'a [u8], a: &xml::Attr) -> (&'a [u8], Span) {
    if a.flags & xml::FLAG_IN_STORE != 0 { (doc.store.as_slice(), a.value) } else { (input, a.value) }
}

/// 規則の並びの覚えとカスケードの警告を報告にする（数えた容量で）。
fn report(sheet: &Sheet, styles: &Styles, budget: &Budget<'_>) -> Result<Report, Error> {
    let budget = budget_ref(budget);
    let notes = sheet.notes.as_slice();
    budget.spend_large(cost::scan(notes.len() as u64))?;
    let unread = notes.iter().filter(|n| n.unread != 0).count();
    let href_bytes = notes.iter().filter(|n| n.unread != 0).fold(0usize, |a, n| a.saturating_add(n.href.len()));
    let warns = notes.len().saturating_sub(unread).saturating_add(styles.warnings.len());
    let mut r = ReportBuilder::with_capacity(budget, warns, unread, href_bytes)?;
    let text = sheet.text.as_slice();
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
