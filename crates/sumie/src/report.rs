//! 報告（requirements.md FR-REP-03・04）：警告（種類と位置）と、読まなかった参照（href と要素の位置）。使ったフォント・
//! 見つからなかった文字・使った画像のデコーダは、それを作る段（M3・M5）で足す。
//!
//! - 並びは入力の位置の順（同じ位置は足した順、位置のない警告は最後。D2。determinism.md §1.1）。段は足す順を気にしなくて
//!   よい（終わりに並べる）
//! - 文言は持たない（種類の列挙を返し、呼び手が訳す。FR-REP-04）
//! - 報告は何も借りない（`Tree` が持ち続ける。href は写して持つ）

use crate::budget::Budget;
use crate::cost;
use crate::error::{Error, Resource};
use crate::mem::{Frozen, MVec, Plain, stable_sort};

/// 警告の種類（版に入らない。足してよい。determinism.md §8）。
#[non_exhaustive]
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WarningKind {
    /// 知らない要素（子ごと描かない。FR-ST-02）
    UnknownElement,
    /// 壊れた値（その属性・宣言を無視した。FR-ST-15）
    InvalidValue,
    /// 飛ばしたアットルール（FR-ST-08）
    AtRule,
    /// 参照の輪（輪の参照を取り除いた。FR-CO-05）
    ReferenceCycle,
    /// 壊れたセレクタ・閉じない規則（規則ごと捨てた。FR-ST-07）
    InvalidSelector,
    /// 読むが当てないセレクタ（`:has()`・`:nth-child(… of S)`。それを含む選択子を当てない。FR-ST-07）
    UnsupportedSelector,
    /// 規則の中の入れ子の規則・アットルール（CSS Nesting。読まずに捨てた。ADR-66 (4)）
    NestedRule,
}

impl Plain for WarningKind {}

/// 警告。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warning {
    pub kind: WarningKind,
    /// 入力のバイトの位置（わかるとき）
    pub position: Option<u64>,
}

/// 読まなかった参照（外を指す `href`・`url()` など。FR-IN-08）。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnreadRef<'a> {
    /// 参照の中身（展開したあとのバイト列）
    pub href: &'a [u8],
    /// 参照を書いた要素の入力のバイトの位置
    pub position: u64,
}

/// 位置がないことの印。
const NO_POSITION: u64 = u64::MAX;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct WarningEntry {
    position: u64,
    kind: WarningKind,
    pad: [u8; 6],
}

impl Plain for WarningEntry {}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct UnreadEntry {
    position: u64,
    /// `store` の中の範囲
    start: u32,
    end: u32,
}

impl Plain for UnreadEntry {}

/// 段までの報告。
#[derive(Debug)]
pub struct Report {
    warnings: Frozen<WarningEntry>,
    unread: Frozen<UnreadEntry>,
    store: Frozen<u8>,
}

impl Report {
    /// 警告（位置の順）。
    pub fn warnings(&self) -> impl ExactSizeIterator<Item = Warning> + '_ {
        self.warnings.as_slice().iter().map(|w| Warning {
            kind: w.kind,
            position: if w.position == NO_POSITION { None } else { Some(w.position) },
        })
    }

    /// 読まなかった参照（位置の順）。
    pub fn unread_refs(&self) -> impl ExactSizeIterator<Item = UnreadRef<'_>> + '_ {
        let store = self.store.as_slice();
        self.unread.as_slice().iter().map(move |u| UnreadRef {
            href: store.get(u.start as usize..u.end as usize).unwrap_or(&[]),
            position: u.position,
        })
    }

    /// 持ち続ける量。
    pub(crate) fn bytes(&self) -> u64 {
        self.warnings.bytes().saturating_add(self.unread.bytes()).saturating_add(self.store.bytes())
    }

    /// 写し（次の段の報告の始まり。`budget` で取る。手間は [`Report::copy_work`]）。
    pub(crate) fn copy_in(&self, budget: &Budget<'_>) -> Result<Report, Error> {
        let budget = budget_ref(budget);
        let mut r = ReportBuilder::with_capacity(budget, self.warnings.len(), self.unread.len(), self.store.len())?;
        r.warnings.append_slice(self.warnings.as_slice())?;
        r.unread.append_slice(self.unread.as_slice())?;
        r.store.append_slice(self.store.as_slice())?;
        Ok(Report { warnings: r.warnings.freeze(), unread: r.unread.freeze(), store: r.store.freeze() })
    }

    /// [`Report::copy_in`] の手間の上限（確保 3 回と写し）。
    pub(crate) fn copy_work(&self) -> u64 {
        let w =
            cost::append(0, self.warnings.len() as u64, self.warnings.len() as u64, size_of::<WarningEntry>() as u64).0;
        let u = cost::append(0, self.unread.len() as u64, self.unread.len() as u64, size_of::<UnreadEntry>() as u64).0;
        let s = cost::append(0, self.store.len() as u64, self.store.len() as u64, 1).0;
        cost::ALLOC_STEP.saturating_mul(3).saturating_add(w).saturating_add(u).saturating_add(s)
    }
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 報告を作る口。段の中で足し、終わりに [`ReportBuilder::finish`] で位置の順に並べて `Report` にする。容量は段が数えた
/// 量で最初に取る（伸ばさない）。
pub(crate) struct ReportBuilder<'b> {
    budget: &'b Budget<'b>,
    warnings: MVec<'b, WarningEntry>,
    unread: MVec<'b, UnreadEntry>,
    store: MVec<'b, u8>,
}

#[allow(dead_code, reason = "M1 の css・style・doc から警告と読まなかった参照を足す")]
impl<'b> ReportBuilder<'b> {
    /// 警告 `warnings` 個・読まなかった参照 `unread` 個・その href のバイト数 `store` の容量で作る。
    pub(crate) fn with_capacity(
        budget: &'b Budget<'b>,
        warnings: usize,
        unread: usize,
        store: usize,
    ) -> Result<Self, Error> {
        Ok(ReportBuilder {
            budget,
            warnings: MVec::with_capacity(budget, warnings)?,
            unread: MVec::with_capacity(budget, unread)?,
            store: MVec::with_capacity(budget, store)?,
        })
    }

    pub(crate) fn warn(&mut self, kind: WarningKind, position: Option<u64>) -> Result<(), Error> {
        full(self.budget, self.warnings.len(), self.warnings.capacity(), 1, size_of::<WarningEntry>())?;
        self.warnings.push(WarningEntry { position: position.unwrap_or(NO_POSITION), kind, pad: [0; 6] })
    }

    pub(crate) fn unread(&mut self, href: &[u8], position: u64) -> Result<(), Error> {
        full(self.budget, self.unread.len(), self.unread.capacity(), 1, size_of::<UnreadEntry>())?;
        full(self.budget, self.store.len(), self.store.capacity(), href.len(), 1)?;
        let start = self.store.len();
        self.store.append_slice(href)?;
        let end = self.store.len();
        let to_u32 = |x: usize| u32::try_from(x).map_err(|_| self.budget.too_large());
        self.unread.push(UnreadEntry { position, start: to_u32(start)?, end: to_u32(end)? })
    }

    /// 位置の順に並べて（同じ位置は足した順）報告にする。
    pub(crate) fn finish(mut self) -> Result<Report, Error> {
        stable_sort(self.budget, self.warnings.as_mut_slice(), |x, y| x.position < y.position)?;
        stable_sort(self.budget, self.unread.as_mut_slice(), |x, y| x.position < y.position)?;
        Ok(Report { warnings: self.warnings.freeze(), unread: self.unread.freeze(), store: self.store.freeze() })
    }
}

/// 数えた容量を超えて足そうとしたら誤り（見積もりの不具合。伸ばさない）。
fn full(budget: &Budget<'_>, len: usize, cap: usize, n: usize, size: usize) -> Result<(), Error> {
    if len.saturating_add(n) > cap {
        let need = (len.saturating_add(n) as u64).saturating_mul(size as u64);
        return Err(Error::limit(Resource::Memory, need, (cap as u64).saturating_mul(size as u64), budget.stage()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
