//! 手間の式（architecture.md §5.1・§5.2）。見積もりと実際の勘定の両方がここの関数を呼ぶ（式が食い違わない）。
//! 見積もりは「これから回る数の上限」、勘定は「実際に回った数」を同じ式に入れる。
//!
//! 係数は M0 の仮の校正（M4 Max・release。`mem::tests::calibrate`。decisions.md ADR-47）。1 単位 ≈ 1 ns を目安にし、
//! M7 で全体を校正し直す。係数を変えても値は変わらない（D2。determinism.md §1.1）。

/// 0 埋めの 1 単位あたりのバイト数（新しく取った領域に初めて触れる分を含む）。
pub(crate) const FILL_BYTES_PER_UNIT: u64 = 16;
/// 写し（入れ物を伸ばすときの写し・`extend_from_slice`）の 1 単位あたりのバイト数。
pub(crate) const COPY_BYTES_PER_UNIT: u64 = 16;
/// 安定な併合の並べ替えの、1 つの要素を 1 段で比べて写す手間。
pub(crate) const MERGE_STEP: u64 = 4;
/// 2 分の探索の 1 歩の手間。
pub(crate) const SEARCH_STEP: u64 = 2;

/// 切り上げの割り算（`b` が 0 なら `a`）。
const fn ceil_div(a: u64, b: u64) -> u64 {
    match (a.checked_div(b), a.checked_rem(b)) {
        (Some(q), Some(r)) => q.saturating_add(if r == 0 { 0 } else { 1 }),
        _ => a,
    }
}

/// `bytes` バイトを 0（か同じ値）で埋める手間。
pub(crate) const fn fill(bytes: u64) -> u64 {
    ceil_div(bytes, FILL_BYTES_PER_UNIT)
}

/// `bytes` バイトを写す手間。
pub(crate) const fn copy(bytes: u64) -> u64 {
    ceil_div(bytes, COPY_BYTES_PER_UNIT)
}

/// ⌈log₂ n⌉（n ≤ 1 なら 0）。併合の段の数と 2 分の探索の歩数に使う。
pub(crate) const fn ceil_log2(n: u64) -> u64 {
    if n <= 1 { 0 } else { u64::BITS.saturating_sub(n.saturating_sub(1).leading_zeros()) as u64 }
}

/// `n` 個（1 つ `size` バイト）の安定な併合の並べ替えの手間の上限（作業の領域への写し・段ごとの併合・戻しの写し）。
pub(crate) const fn stable_sort(n: u64, size: u64) -> u64 {
    let bytes = n.saturating_mul(size);
    let merge = n.saturating_mul(ceil_log2(n)).saturating_mul(MERGE_STEP);
    merge.saturating_add(copy(bytes)).saturating_add(copy(bytes))
}

/// `n` 個の並びの 2 分の探索の手間（歩数 ⌈log₂ (n + 1)⌉）。
pub(crate) const fn search(n: u64) -> u64 {
    ceil_log2(n.saturating_add(1)).saturating_mul(SEARCH_STEP).saturating_add(1)
}

/// 差し込むときに `moved` 個（1 つ `size` バイト）を後ろへずらす手間（写しと同じ式に、差し込みの 1 を足す）。
pub(crate) const fn shift(moved: u64, size: u64) -> u64 {
    copy(moved.saturating_mul(size)).saturating_add(1)
}

#[cfg(test)]
mod tests;
