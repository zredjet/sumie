//! 確保の口（ADR-31・ADR-39。architecture.md §5.1）。
//!
//! ライブラリの確保は、ここの入れ物（`MVec`・`MBytes`・`SortedVec`・`SortedMap`・`Frozen`）と `stable_sort` だけで行う。
//!
//! - 伸ばすときに `Budget` に数え（確保の前に）、捨てるとき（`Drop`）に返す。山は数えた量の最大
//! - 数える量は頼んだ容量 × `size_of`（`capacity()` は std の実装に依るので使わない）
//! - 見積もりで数が決まるものは `with_capacity` でまとめて取り、伸ばさない。決まらないものは倍々に伸ばし、新しい領域を
//!   先に数えてから古い領域を返す（取り直しの間の両方が山に入る）
//! - 写し・埋めの手間は `cost` の式で、`CANCEL_INTERVAL` 未満の塊ごとに前払いする
//! - 入れる型は `Plain`（`#[repr(C)]` か基本の型で `Copy`。`tests/rules.rs` が見張る）
//! - 段の結果に残すものは `freeze` で `Frozen` にする（`Budget` から切り離す。取り直さない）
//! - 手間は操作ごとに `cost` の式で払う。`MVec` の `push`・`append_slice`・`resize_to`・`insert` は `cost::append`・`resize`・
//!   `insert` とぴったり同じ（試験で照らす）。`SortedVec`・`SortedMap` は `sorted_insert`・`map_insert` 以下（最悪の位置の式）
//! - 入れる型の大きさは `MAX_PLAIN_SIZE` まで（組むときに止める。1 回の spend を `CANCEL_INTERVAL` 未満に保つ）
//! - 誤り（上限・中止）を返したあとの中身は決めない（段は誤りで止まり、結果を作らない。FR-LIM-06）。数えた量は Drop で
//!   正しく返す
//!
//! このモジュールの中だけは、std の確保の口と、範囲を確かめて使う口を、理由を書いて許す（`#[expect(…, reason)]`）。

mod sort;
mod sorted;

use std::sync::Arc;

use crate::budget::{Budget, chunk_len};
use crate::cost;
use crate::error::Error;

pub(crate) use sort::{stable_sort, stable_sort_try};
pub(crate) use sorted::{SortedMap, SortedVec, lower_bound, lower_bound_try};

/// `MVec`・`Frozen` に入れてよい型：`#[repr(C)]` か基本の型で、写して捨てても何も起きないもの（ADR-39 (3)）。
/// 大きさが Rust の版で変わらないので、数える量（D2）がツールチェーンに依らない。
pub(crate) trait Plain: Copy + 'static {}

/// 基本の型に `Plain` を付ける（`usize`・`isize` は大きさがターゲットで変わるので入れない）。
macro_rules! plain {
    ($($t:ty),* $(,)?) => { $(impl Plain for $t {})* };
}
plain!(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64, bool, char);

/// 入れ物に入れる型の大きさの上限（バイト）。1 要素の写しの手間（`cost::copy`）を `CANCEL_INTERVAL` よりずっと小さく保つ。
pub(crate) const MAX_PLAIN_SIZE: usize = 4096;

/// 数を数える入れ物（`Vec` の包み）。
pub(crate) struct MVec<'b, T: Plain> {
    v: Vec<T>,
    /// 頼んだ容量（数えた量はこれ × size_of）
    cap: usize,
    budget: &'b Budget<'b>,
}

/// バイトの入れ物。
#[expect(dead_code, reason = "M1 の xml の文字の置き場から使う")]
pub(crate) type MBytes<'b> = MVec<'b, u8>;

/// バイトの数に比べる手間の式 `f`（`cost::copy`・`cost::fill`）で、塊ごとに累計の差を払う。`done` はこれまでの要素の数で、
/// `n` 個（1 つ `size` バイト）を足した累計を返す。差を払うので、払う手間の合計は `f(全体のバイト数)` とぴったり同じ
/// （塊ごとに切り上げると、合計が見積もりの式を超えうる）。
pub(super) fn spend_cumulative(
    budget: &Budget<'_>,
    f: fn(u64) -> u64,
    done: u64,
    n: u64,
    size: u64,
) -> Result<u64, Error> {
    let after = done.saturating_add(n);
    let units = f(after.saturating_mul(size)).saturating_sub(f(done.saturating_mul(size)));
    budget.spend(units)?;
    Ok(after)
}

/// `n` 個の `T` のバイト数。あふれたら `Limit`（上限を超える）。
fn bytes_for<T>(budget: &Budget<'_>, n: usize) -> Result<u64, Error> {
    let size = size_of::<T>() as u64;
    (n as u64).checked_mul(size).ok_or_else(|| budget.too_large())
}

impl<'b, T: Plain> MVec<'b, T> {
    /// 空の入れ物（確保しない）。
    #[expect(clippy::disallowed_methods, reason = "Vec::new は確保しない")]
    pub(crate) fn new(budget: &'b Budget<'b>) -> MVec<'b, T> {
        MVec { v: Vec::new(), cap: 0, budget }
    }

    /// `n` 個の容量をまとめて取る（数えてから確保する）。
    #[expect(clippy::disallowed_methods, reason = "mem の中の確保の口（数えてから try_reserve_exact で取る）")]
    pub(crate) fn with_capacity(budget: &'b Budget<'b>, n: usize) -> Result<MVec<'b, T>, Error> {
        const {
            if size_of::<T>() > MAX_PLAIN_SIZE {
                panic!("MVec に入れる型は MAX_PLAIN_SIZE バイトまで（組むときに止める）");
            }
        }
        let bytes = bytes_for::<T>(budget, n)?;
        if bytes > 0 {
            budget.spend(cost::ALLOC_STEP)?;
        }
        budget.charge(bytes)?;
        let mut v = Vec::new();
        if v.try_reserve_exact(n).is_err() {
            budget.release(bytes);
            return Err(budget.out_of_memory(bytes));
        }
        Ok(MVec { v, cap: n, budget })
    }

    /// 入れ物の勘定。
    pub(crate) fn budget(&self) -> &'b Budget<'b> {
        self.budget
    }

    pub(crate) fn len(&self) -> usize {
        self.v.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.v.is_empty()
    }

    /// 頼んだ容量。
    pub(crate) fn capacity(&self) -> usize {
        self.cap
    }

    /// 数えている量（頼んだ容量 × size_of）。
    pub(crate) fn counted_bytes(&self) -> u64 {
        bytes_for::<T>(self.budget, self.cap).unwrap_or(u64::MAX)
    }

    pub(crate) fn as_slice(&self) -> &[T] {
        &self.v
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.v
    }

    pub(crate) fn get(&self, i: usize) -> Option<&T> {
        self.v.get(i)
    }

    pub(crate) fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        self.v.get_mut(i)
    }

    pub(crate) fn last(&self) -> Option<&T> {
        self.v.last()
    }

    pub(crate) fn last_mut(&mut self) -> Option<&mut T> {
        self.v.last_mut()
    }

    pub(crate) fn pop(&mut self) -> Option<T> {
        self.v.pop()
    }

    /// 長さを `n` 以下にする（容量は変えない）。
    pub(crate) fn truncate(&mut self, n: usize) {
        self.v.truncate(n);
    }

    pub(crate) fn clear(&mut self) {
        self.v.clear();
    }

    /// 少なくとも `need` 個を入れられるようにする（足りなければ倍々に伸ばす。新しい領域を先に数える）。
    fn ensure(&mut self, need: usize) -> Result<(), Error> {
        if need <= self.cap {
            return Ok(());
        }
        let new_cap = cost::grow_cap(self.cap as u64, need as u64);
        let mut next = MVec::with_capacity(self.budget, usize::try_from(new_cap).unwrap_or(usize::MAX))?;
        next.copy_within_cap(&self.v)?;
        // 古い領域はここで捨てられ、Drop で数えた量を返す
        *self = next;
        Ok(())
    }

    /// 容量の内で `s` を後ろに写す（写しの手間を塊ごとに前払いする）。容量は呼び手が ensure で取っておく。足りなければ
    /// 写さずに誤りを返す（起きない。std の Vec が数えずに取り直さないための歯止め。ensure は呼ばない＝互いを呼ばない）。
    #[expect(clippy::disallowed_methods, reason = "mem の中の写し（容量は確かめた）")]
    fn copy_within_cap(&mut self, s: &[T]) -> Result<(), Error> {
        if self.v.len().saturating_add(s.len()) > self.cap {
            return Err(self.budget.out_of_memory(0));
        }
        let size = size_of::<T>() as u64;
        let per = chunk_len(cost::copy(size).max(1));
        let mut rest = s;
        let mut done = 0u64;
        while !rest.is_empty() {
            let n = per.min(rest.len());
            let (head, tail) = rest.split_at_checked(n).unwrap_or((rest, &[]));
            done = spend_cumulative(self.budget, cost::copy, done, n as u64, size)?;
            self.v.extend_from_slice(head);
            rest = tail;
        }
        Ok(())
    }

    /// 1 つ足す（容量が足りなければ倍々に伸ばす）。
    #[expect(clippy::disallowed_methods, reason = "mem の中の push（容量は ensure で確かめたので取り直さない）")]
    pub(crate) fn push(&mut self, x: T) -> Result<(), Error> {
        // 1 回の決まった手間と 1 つの写し（cost::append の n = 1）
        self.budget.spend(cost::OP_STEP.saturating_add(cost::copy(size_of::<T>() as u64)))?;
        self.ensure(self.v.len().saturating_add(1))?;
        if self.v.len() >= self.cap {
            return Err(self.budget.out_of_memory(0));
        }
        self.v.push(x);
        Ok(())
    }

    /// 並びを後ろに写す（std の `extend_from_slice` と名前を分ける：`tests/rules.rs` が mem の外の std の口を名前で拾う）。
    pub(crate) fn append_slice(&mut self, s: &[T]) -> Result<(), Error> {
        self.budget.spend(cost::OP_STEP)?;
        self.ensure(self.v.len().saturating_add(s.len()))?;
        self.copy_within_cap(s)
    }

    /// 長さを `n` にする（伸ばす分は `value` で埋める。埋める手間を塊ごとに前払いする。std の `resize` と名前を分ける）。
    #[expect(clippy::disallowed_methods, reason = "mem の中の resize（容量は ensure で確かめた）")]
    pub(crate) fn resize_to(&mut self, n: usize, value: T) -> Result<(), Error> {
        if n <= self.v.len() {
            self.v.truncate(n);
            return Ok(());
        }
        self.budget.spend(cost::OP_STEP)?;
        self.ensure(n)?;
        let size = size_of::<T>() as u64;
        let per = chunk_len(cost::fill(size).max(1));
        let mut done = 0u64;
        while self.v.len() < n {
            let k = per.min(n.saturating_sub(self.v.len()));
            done = spend_cumulative(self.budget, cost::fill, done, k as u64, size)?;
            let to = self.v.len().saturating_add(k);
            self.v.resize(to, value);
        }
        Ok(())
    }

    /// `i` の位置に差し込む（後ろをずらす手間を前払いする）。`i > len` なら後ろに足す。
    #[expect(clippy::disallowed_methods, reason = "mem の中の差し込み（範囲を確かめてから rotate_right する）")]
    pub(crate) fn insert(&mut self, i: usize, x: T) -> Result<(), Error> {
        let i = i.min(self.v.len());
        self.push(x)?;
        // 後ろに足した x を、後ろから塊ごとに前へ回す（塊 [lo, hi] を右へ 1 つ回すと、hi の x が lo に来る。塊は 1 つ
        // 重ねる）。差し込みの 1 と、塊ごとにずらす数の写しの手間を前払いする（合計は cost::shift とぴったり同じ）
        self.budget.spend(1)?;
        let size = size_of::<T>() as u64;
        let per = chunk_len(cost::copy(size).max(1));
        let mut hi = self.v.len().saturating_sub(1);
        let mut done = 0u64;
        while hi > i {
            let lo = hi.saturating_sub(per).max(i);
            done = spend_cumulative(self.budget, cost::copy, done, hi.saturating_sub(lo) as u64, size)?;
            if let Some(w) = self.v.get_mut(lo..=hi) {
                w.rotate_right(1);
            }
            hi = lo;
        }
        Ok(())
    }

    /// 段の結果に残す（`Budget` から切り離す。取り直さない。数えた量は `Frozen::bytes`）。
    pub(crate) fn freeze(mut self) -> Frozen<T> {
        let bytes = self.counted_bytes();
        let v = std::mem::take(&mut self.v);
        // 切り離した分は Drop で返さない（段の勘定には最後まで残る。段の結果の量は Frozen が持つ）
        self.cap = 0;
        Frozen { v, bytes }
    }
}

impl<T: Plain> Drop for MVec<'_, T> {
    fn drop(&mut self) {
        self.budget.release(self.counted_bytes());
    }
}

/// 段の結果に残す入れ物（数え終えて `Budget` から切り離したもの。ADR-39 (1)）。
#[derive(Debug)]
pub(crate) struct Frozen<T: Plain> {
    v: Vec<T>,
    /// 数えた量（頼んだ容量 × size_of）
    bytes: u64,
}

impl<T: Plain> Frozen<T> {
    /// 空（確保しない）。
    #[expect(clippy::disallowed_methods, reason = "Vec::new は確保しない")]
    pub(crate) const fn empty() -> Frozen<T> {
        Frozen { v: Vec::new(), bytes: 0 }
    }

    pub(crate) fn as_slice(&self) -> &[T] {
        &self.v
    }

    pub(crate) fn len(&self) -> usize {
        self.v.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.v.is_empty()
    }

    /// 持ち続ける量（`held_bytes`・`result_bytes` に入れる）。
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// 一度に比べるバイトの数（1 回の払いの `cost::copy` が `CANCEL_INTERVAL` よりずっと小さい）。
const CMP_CHUNK: usize = 1 << 18;

/// 2 つのバイト列を辞書の順に比べる。比べるバイトの手間は塊ずつ、比べる前に払う（長い名前の比べで中止を見る間が延びない）。
pub(crate) fn bytes_cmp(budget: &Budget<'_>, a: &[u8], b: &[u8]) -> Result<core::cmp::Ordering, Error> {
    let n = a.len().min(b.len());
    let mut k = 0usize;
    loop {
        let e = k.saturating_add(CMP_CHUNK).min(n);
        budget.spend(cost::copy(e.saturating_sub(k) as u64).saturating_add(1))?;
        let (x, y) = (a.get(k..e).unwrap_or(&[]), b.get(k..e).unwrap_or(&[]));
        let o = x.cmp(y);
        if o != core::cmp::Ordering::Equal {
            return Ok(o);
        }
        if e >= n {
            return Ok(a.len().cmp(&b.len()));
        }
        k = e;
    }
}

/// 2 つのバイト列が ASCII の大文字小文字を問わずに同じか（塊ずつ払う）。
pub(crate) fn bytes_eq_ignore_case(budget: &Budget<'_>, a: &[u8], b: &[u8]) -> Result<bool, Error> {
    if a.len() != b.len() {
        budget.spend(1)?;
        return Ok(false);
    }
    let mut k = 0usize;
    loop {
        let e = k.saturating_add(CMP_CHUNK).min(a.len());
        budget.spend(cost::copy(e.saturating_sub(k) as u64).saturating_add(1))?;
        if !a.get(k..e).unwrap_or(&[]).eq_ignore_ascii_case(b.get(k..e).unwrap_or(&[])) {
            return Ok(false);
        }
        if e >= a.len() {
            return Ok(true);
        }
        k = e;
    }
}

/// 2 つのバイト列が同じか（長さが違えば払わずに偽。同じ長さなら [`bytes_cmp`] と同じに払う）。
pub(crate) fn bytes_eq(budget: &Budget<'_>, a: &[u8], b: &[u8]) -> Result<bool, Error> {
    if a.len() != b.len() {
        budget.spend(1)?;
        return Ok(false);
    }
    Ok(bytes_cmp(budget, a, b)? == core::cmp::Ordering::Equal)
}

/// `Arc` を共有する（`Clone::clone` を mem の外で使わないための包み。ADR-41 (6)）。
#[expect(clippy::disallowed_methods, reason = "Arc の共有は数を増やすだけで確保しない")]
#[expect(dead_code, reason = "M5 の FontSet（フォントのバイト列の共有）から使う")]
pub(crate) fn share<T: ?Sized>(a: &Arc<T>) -> Arc<T> {
    Arc::clone(a)
}

#[cfg(test)]
mod tests;
