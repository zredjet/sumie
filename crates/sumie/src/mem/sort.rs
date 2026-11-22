//! 安定な併合の並べ替え（determinism.md §4.1・ADR-31 (2)）。std の `sort*` は std の中で数えずに確保するので使わない。
//!
//! 下から上への併合（幅 1・2・4 …）で、再帰しない。作業の領域（`n` 個）は `mem` から取って数える。手間は
//! `cost::stable_sort` の式（段ごとに `n × MERGE_STEP` を、`CANCEL_INTERVAL` 未満の塊ごとに前払いする）。比べ方は
//! 全順序にする（浮動小数は `total_cmp` か整数の鍵）。

use super::{MVec, Plain};
use crate::budget::{Budget, chunk_len};
use crate::cost;
use crate::error::Error;

/// `s` を `is_less` の順に安定に並べる（等しいものは元の順）。
pub(crate) fn stable_sort<T: Plain>(
    budget: &Budget<'_>,
    s: &mut [T],
    mut is_less: impl FnMut(&T, &T) -> bool,
) -> Result<(), Error> {
    let n = s.len();
    if n < 2 {
        return Ok(());
    }
    // 作業の領域（写しの手間は MVec が払う）
    let mut work: MVec<'_, T> = MVec::with_capacity(budget_ref(budget), n)?;
    work.append_slice(s)?;
    let step = cost::merge_step(size_of::<T>() as u64);
    let chunk = chunk_len(step);
    // 今の並びが s にあるか（偽なら work にある）
    let mut in_s = true;
    let mut width = 1usize;
    while width < n {
        if in_s {
            merge_pass(budget, s, work.as_mut_slice(), width, chunk, step, &mut is_less)?;
        } else {
            merge_pass(budget, work.as_slice(), s, width, chunk, step, &mut is_less)?;
        }
        in_s = !in_s;
        width = width.saturating_mul(2);
    }
    if !in_s {
        // 戻しの写し
        let size = size_of::<T>() as u64;
        let per = chunk_len(cost::copy(size).max(1));
        let mut start = 0usize;
        let mut done = 0u64;
        while start < n {
            let end = start.saturating_add(per).min(n);
            done = super::spend_cumulative(budget, cost::copy, done, end.saturating_sub(start) as u64, size)?;
            if let (Some(dst), Some(src)) = (s.get_mut(start..end), work.as_slice().get(start..end)) {
                for (d, x) in dst.iter_mut().zip(src) {
                    *d = *x;
                }
            }
            start = end;
        }
    }
    Ok(())
}

/// `&Budget<'c>` を MVec の寿命に合わせる（`Budget` は寿命について共変）。
fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 幅 `width` の隣り合う 2 つの並びを、`src` から `dst` へ併合する（1 段）。
#[expect(clippy::arithmetic_side_effects, reason = "添字はすべて n 以下で、n は確保した長さ（usize を超えない）")]
fn merge_pass<T: Plain>(
    budget: &Budget<'_>,
    src: &[T],
    dst: &mut [T],
    width: usize,
    chunk: usize,
    step: u64,
    is_less: &mut impl FnMut(&T, &T) -> bool,
) -> Result<(), Error> {
    let n = src.len().min(dst.len());
    let mut lo = 0usize;
    // 出した数（塊ごとに前払いする）
    let mut paid_until = 0usize;
    while lo < n {
        let mid = lo.saturating_add(width).min(n);
        let hi = mid.saturating_add(width).min(n);
        let (mut i, mut j) = (lo, mid);
        for o in lo..hi {
            if o >= paid_until {
                let k = chunk.min(n - o);
                budget.spend((k as u64).saturating_mul(step))?;
                paid_until = o + k;
            }
            // 右が左より小さいときだけ右を取る（安定）
            let take_right = match (src.get(i).filter(|_| i < mid), src.get(j).filter(|_| j < hi)) {
                (Some(a), Some(b)) => is_less(b, a),
                (None, Some(_)) => true,
                _ => false,
            };
            let x = if take_right {
                j += 1;
                src.get(j - 1)
            } else {
                i += 1;
                src.get(i - 1)
            };
            if let (Some(x), Some(d)) = (x, dst.get_mut(o)) {
                *d = *x;
            }
        }
        lo = hi;
    }
    Ok(())
}
