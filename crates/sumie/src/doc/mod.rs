//! 段 3：文書（architecture.md §4.3）。読む段の全体（xml → css → style → doc）をまとめ、[`Document`] を作る。
//!
//! 読む段は「見積もり（数える段）＋ 本体（作る段）」：`parse` は中で `estimate_parse` と同じ数える段を走らせ（見積もりの
//! 手間と確保の上限で払う）、見積もりが上限の内なら、見積もりの値を上限にした `Budget` で本体を走らせる（ADR-37・ADR-39）。
//! `estimate_parse` が通れば、同じ入力・同じ設定の `parse` は `Limit` にならない（確保器の失敗と中止を除く。D2）。
//!
//! 今は XML の段だけ。css・style・doc の段は M1 の残りの単位で足す。

use crate::budget::{Budget, Estimate};
use crate::cancel::Cancel;
use crate::error::{Error, Resource, Stage};
use crate::limits::Limits;
use crate::report::{Report, ReportBuilder};
use crate::style::ParseOptions;
use crate::xml::{self, XmlCounts, XmlDoc};

/// 読んだ文書（入力を借りる。名前と値は写さない）。
pub struct Document<'s> {
    input: &'s [u8],
    xml: XmlDoc,
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
        self.xml.bytes().saturating_add(self.report.bytes())
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

    pub(crate) fn limits(&self) -> &Limits {
        &self.limits
    }
}

/// 数える段の結果（本体の容量と見積もり）。
struct Counted {
    xml: XmlCounts,
    estimate: Estimate,
    /// 本体の手間と確保の山（本体の `Budget` の上限）
    body_work: u64,
    body_peak: u64,
}

/// 勘定の見張りの値（ファズと試験が見る。`parse_watch`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Watch {
    pub(crate) count_work: u64,
    pub(crate) count_peak: u64,
    pub(crate) body_work: u64,
    pub(crate) body_peak: u64,
    /// 1 回の払いの最大と、返しすぎ（数える段と本体の両方）
    pub(crate) max_spend: u64,
    pub(crate) over_released: bool,
}

impl Watch {
    fn see(&mut self, b: &Budget<'_>) {
        self.max_spend = self.max_spend.max(b.max_spend());
        self.over_released |= b.over_released();
    }
}

/// 数える段（`estimate_parse` と `parse` の前半）。見積もりの上限で払い、見積もり（数える段 ＋ 本体）が上限の内かを
/// 確かめる。
fn count(svg: &[u8], opts: &ParseOptions<'_>, cancel: &dyn Cancel, watch: &mut Watch) -> Result<Counted, Error> {
    opts.style.check()?;
    let limits = &opts.limits;
    let budget = Budget::new(Stage::EstimateParse, limits.estimate_work, limits.estimate_bytes, cancel);
    let r = xml::count(svg, limits, &budget, Stage::EstimateParse);
    watch.count_work = budget.work();
    watch.count_peak = budget.peak();
    watch.see(&budget);
    let xml = r?;
    // 報告は今は空（容量 0 の入れ物は確保も手間もない）
    let body_work = xml.build_work();
    let body_peak = xml.build_peak();
    // `parse` は数える段を走らせてから本体を走らせる。手間はその和、確保の山は大きい方（数える段の確保は本体の前に返す）
    let estimate = Estimate {
        work: budget.work().saturating_add(body_work),
        peak_bytes: budget.peak().max(body_peak),
        result_bytes: xml.result_bytes(),
    };
    if estimate.work > limits.parse_work {
        return Err(Error::limit(Resource::Work, estimate.work, limits.parse_work, Stage::Parse));
    }
    if estimate.peak_bytes > limits.parse_bytes {
        return Err(Error::limit(Resource::Memory, estimate.peak_bytes, limits.parse_bytes, Stage::Parse));
    }
    Ok(Counted { xml, estimate, body_work, body_peak })
}

/// 読む段の見積もり（architecture.md §3）。本文を 1 度だけ字句に分けて数える。`work` は `parse` の全体（中で走らせる
/// 数える段 ＋ 本体）の手間。見積もりが上限を超えれば `Limit`。
pub(crate) fn estimate_parse(svg: &[u8], opts: &ParseOptions<'_>, cancel: &dyn Cancel) -> Result<Estimate, Error> {
    count(svg, opts, cancel, &mut Watch::default()).map(|c| c.estimate)
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
    let c = count(svg, opts, cancel, watch)?;
    // 本体の Budget は払った手間が CANCEL_INTERVAL に届くまで中止を見ないので、段の間で 1 度見る（印が立ってからの手間を
    // 2 × CANCEL_INTERVAL の内に）
    if cancel.cancelled() {
        return Err(Error::Cancelled);
    }
    let budget = Budget::new(Stage::Parse, c.body_work, c.body_peak, cancel);
    let r = body(svg, opts, &c, &budget);
    watch.body_work = budget.work();
    watch.body_peak = budget.peak();
    watch.see(&budget);
    r
}

fn body<'s>(svg: &'s [u8], opts: &ParseOptions<'_>, c: &Counted, budget: &Budget<'_>) -> Result<Document<'s>, Error> {
    let budget = budget_ref(budget);
    let xml = xml::build(svg, &c.xml, &opts.limits, budget, Stage::Parse)?;
    let report = ReportBuilder::with_capacity(budget, 0, 0, 0)?.finish()?;
    Ok(Document { input: svg, xml, limits: opts.limits, report })
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
