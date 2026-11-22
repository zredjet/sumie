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
#[derive(Debug)]
pub struct Document<'s> {
    input: &'s [u8],
    xml: XmlDoc,
    limits: Limits,
    report: Report,
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
}

/// 数える段（`estimate_parse` と `parse` の前半）。見積もりの上限で払い、本体の見積もりが上限の内かを確かめる。
fn count(svg: &[u8], opts: &ParseOptions<'_>, cancel: &dyn Cancel) -> Result<Counted, Error> {
    opts.style.check()?;
    let limits = &opts.limits;
    let budget = Budget::new(Stage::EstimateParse, limits.estimate_work, limits.estimate_bytes, cancel);
    let xml = xml::count(svg, limits, &budget, Stage::EstimateParse)?;
    // 報告は今は空（容量 0 の入れ物は確保も手間もない）
    let estimate = Estimate { work: xml.build_work(), peak_bytes: xml.build_peak(), result_bytes: xml.result_bytes() };
    if estimate.work > limits.parse_work {
        return Err(Error::limit(Resource::Work, estimate.work, limits.parse_work, Stage::Parse));
    }
    if estimate.peak_bytes > limits.parse_bytes {
        return Err(Error::limit(Resource::Memory, estimate.peak_bytes, limits.parse_bytes, Stage::Parse));
    }
    Ok(Counted { xml, estimate })
}

/// 読む段の見積もり（architecture.md §3）。本文を 1 度だけ字句に分けて数える。見積もりが上限を超えれば `Limit`。
pub(crate) fn estimate_parse(svg: &[u8], opts: &ParseOptions<'_>, cancel: &dyn Cancel) -> Result<Estimate, Error> {
    count(svg, opts, cancel).map(|c| c.estimate)
}

/// 読む（architecture.md §3）。
pub(crate) fn parse<'s>(svg: &'s [u8], opts: &ParseOptions<'_>, cancel: &dyn Cancel) -> Result<Document<'s>, Error> {
    let c = count(svg, opts, cancel)?;
    let budget = Budget::new(Stage::Parse, c.estimate.work, c.estimate.peak_bytes, cancel);
    let xml = xml::build(svg, &c.xml, &opts.limits, &budget, Stage::Parse)?;
    let report = ReportBuilder::with_capacity(&budget, 0, 0, 0)?.finish()?;
    Ok(Document { input: svg, xml, limits: opts.limits, report })
}

#[cfg(test)]
mod tests;
