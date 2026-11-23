//! 手間と確保の勘定（architecture.md §5.1・§5.3。ADR-31・ADR-39）。
//!
//! 1 つの段の中で 1 つの `Budget` を使う。上限は見積もりの値（「見積もり ≤ 上限」を確かめてから段に入る）で、勘定が
//! それを越えるのは見積もりの不具合（本番は `Limit`、試験は落とす）。中は `Cell`（ライブラリはスレッドを作らない）。

use std::cell::Cell;

use crate::cancel::Cancel;
use crate::error::{Error, Resource, Stage};

/// 中止の印を読む間隔（手間の単位。ADR-25 (3)）。長く回るループは、これ未満の手間の塊に分けて、塊ごとに前払いする。
pub(crate) const CANCEL_INTERVAL: u64 = 1 << 16;

/// 段に入る前に返す見積もり（architecture.md §3）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Estimate {
    /// 手間（1 単位 ≈ 1 ns。FR-LIM-04）
    pub work: u64,
    /// その段で同時に生きる確保の最大（出力のバッファを除く）
    pub peak_bytes: u64,
    /// その段の結果が持ち続ける量（`Document`・`Tree`・`Plan`。描く段は 0）
    pub result_bytes: u64,
}

/// 1 つの段の勘定。
pub(crate) struct Budget<'c> {
    stage: Stage,
    work: Cell<u64>,
    work_limit: u64,
    bytes: Cell<u64>,
    peak: Cell<u64>,
    bytes_limit: u64,
    /// 次に中止の印を読む手間（CANCEL_INTERVAL の倍数）
    next_check: Cell<u64>,
    cancel: &'c dyn Cancel,
    // 試験とファズで照らす見張り（値と勘定には効かない）
    /// 数えた量より多く返した（二重に返した）か
    over_released: Cell<bool>,
    /// 数えた確保の合計と回数（ヒープの合計・回数と照らす）
    charged_total: Cell<u64>,
    charges: Cell<u64>,
    /// 1 回の spend の最大（CANCEL_INTERVAL 未満の約束を照らす）
    max_spend: Cell<u64>,
}

impl<'c> Budget<'c> {
    /// 上限（見積もりの値）と中止の印で勘定を始める。
    pub(crate) fn new(stage: Stage, work_limit: u64, bytes_limit: u64, cancel: &'c dyn Cancel) -> Budget<'c> {
        Budget {
            stage,
            work: Cell::new(0),
            work_limit,
            bytes: Cell::new(0),
            peak: Cell::new(0),
            bytes_limit,
            next_check: Cell::new(CANCEL_INTERVAL),
            cancel,
            over_released: Cell::new(false),
            charged_total: Cell::new(0),
            charges: Cell::new(0),
            max_spend: Cell::new(0),
        }
    }

    /// 手間を先に払う。上限を超えたら `Limit`、`CANCEL_INTERVAL` の倍数を越えるたびに中止の印を読む。
    ///
    /// `units` は `CANCEL_INTERVAL` 未満にする（長いループは [`chunk_len`] で塊に分ける）。そうすれば、印が立ってから
    /// 返すまでの手間は 2 × `CANCEL_INTERVAL` を超えない（FR-LIM-05）。
    pub(crate) fn spend(&self, units: u64) -> Result<(), Error> {
        let Some(w) = self.work.get().checked_add(units) else {
            return Err(Error::limit(Resource::Work, u64::MAX, self.work_limit, self.stage));
        };
        if w > self.work_limit {
            return Err(Error::limit(Resource::Work, w, self.work_limit, self.stage));
        }
        self.work.set(w);
        if units > self.max_spend.get() {
            self.max_spend.set(units);
        }
        if w >= self.next_check.get() {
            // w を越える最初の CANCEL_INTERVAL の倍数
            let next = (w >> CANCEL_SHIFT).saturating_add(1).saturating_mul(CANCEL_INTERVAL);
            self.next_check.set(next);
            if self.cancel.cancelled() {
                return Err(Error::Cancelled);
            }
        }
        Ok(())
    }

    /// 確保の量を数える（確保の前に呼ぶ）。上限を超えたら `Limit`。
    /// `n` 個（1 つ `per` 単位）の仕事を、1 回の払いが `CANCEL_INTERVAL` 未満になる塊に分けて前払いする。
    pub(crate) fn spend_units(&self, n: u64, per: u64) -> Result<(), Error> {
        let step = chunk_len(per) as u64;
        let mut left = n;
        while left > 0 {
            let k = if left < step { left } else { step };
            self.spend(k.saturating_mul(per))?;
            left = left.saturating_sub(k);
        }
        Ok(())
    }

    pub(crate) fn charge(&self, bytes: u64) -> Result<(), Error> {
        let Some(b) = self.bytes.get().checked_add(bytes) else {
            return Err(Error::limit(Resource::Memory, u64::MAX, self.bytes_limit, self.stage));
        };
        if b > self.bytes_limit {
            return Err(Error::limit(Resource::Memory, b, self.bytes_limit, self.stage));
        }
        self.bytes.set(b);
        if b > self.peak.get() {
            self.peak.set(b);
        }
        self.charged_total.set(self.charged_total.get().saturating_add(bytes));
        self.charges.set(self.charges.get().saturating_add(1));
        Ok(())
    }

    /// 捨てた量を返す。数えた量より多く返したら（二重に返したら）印を立てる（試験とファズが照らす）。
    pub(crate) fn release(&self, bytes: u64) {
        let now = self.bytes.get();
        if bytes > now {
            self.over_released.set(true);
        }
        self.bytes.set(now.saturating_sub(bytes));
    }

    /// 確保器が断ったときの誤り（`try_reserve_exact` の失敗。環境に依るので D1・D2 の約束の外。determinism.md §1.2）。
    /// `needed` は頼んだバイト数、`allowed` は 0。
    pub(crate) fn out_of_memory(&self, bytes: u64) -> Error {
        Error::limit(Resource::Allocator, bytes, 0, self.stage)
    }

    /// 頼んだ量がバイト数で表せない（上限を超える）ときの誤り。
    pub(crate) fn too_large(&self) -> Error {
        Error::limit(Resource::Memory, u64::MAX, self.bytes_limit, self.stage)
    }

    /// 払った手間。
    pub(crate) fn work(&self) -> u64 {
        self.work.get()
    }

    /// 今数えている確保の量。
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.get()
    }

    /// 数えた確保の最大。
    pub(crate) fn peak(&self) -> u64 {
        self.peak.get()
    }

    /// 数えた量より多く返したことがあるか（不具合の印）。
    pub(crate) fn over_released(&self) -> bool {
        self.over_released.get()
    }

    /// 数えた確保の合計（バイト）と回数。
    pub(crate) fn charged(&self) -> (u64, u64) {
        (self.charged_total.get(), self.charges.get())
    }

    /// 1 回の spend の最大（`CANCEL_INTERVAL` 未満のはず）。
    pub(crate) fn max_spend(&self) -> u64 {
        self.max_spend.get()
    }

    /// 段。
    pub(crate) fn stage(&self) -> Stage {
        self.stage
    }
}

/// `CANCEL_INTERVAL` の 2 の冪の指数。
const CANCEL_SHIFT: u32 = CANCEL_INTERVAL.trailing_zeros();

/// 1 つあたり `per` 単位の仕事を、`CANCEL_INTERVAL` 未満の手間の塊に分けたときの、1 つの塊に入れる数（1 以上）。
/// `per` が `CANCEL_INTERVAL` 以上なら 1（1 つずつ払う。1 回の spend が CANCEL_INTERVAL 以上になるので、そうなる型は
/// 入れ物に入れない。試験は `max_spend` で照らす）。値は `CANCEL_INTERVAL - 1` 以下なので `usize` に収まる。
pub(crate) const fn chunk_len(per: u64) -> usize {
    let most = CANCEL_INTERVAL.saturating_sub(1);
    let n = match most.checked_div(per) {
        Some(n) => n,
        None => most,
    };
    if n == 0 { 1 } else { n as usize }
}

#[cfg(test)]
mod tests;
