//! 並べた入れ物（determinism.md §4.1）。探すのは自前の `lower_bound`（等しいものが並ぶときは最初の位置）。
//! `HashMap`・`BTreeMap` の代わり。

use std::cmp::Ordering;

use super::{MVec, Plain};
use crate::budget::Budget;
use crate::cost;
use crate::error::Error;

/// 並べた `s` の中で、`is_less(x)` が偽になる最初の位置（`is_less` は「x がさがす値より小さい」。等しいものが並ぶときは
/// 最初の位置。`slice::binary_search*` は等しいものの位置が決まらないので使わない）。歩数は ⌈log₂ (n + 1)⌉ 以下。
#[expect(clippy::arithmetic_side_effects, reason = "lo < hi ≤ len なので hi - lo はあふれず、mid + 1 ≤ hi")]
pub(crate) fn lower_bound<T>(s: &[T], mut is_less: impl FnMut(&T) -> bool) -> usize {
    let (mut lo, mut hi) = (0usize, s.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match s.get(mid) {
            Some(x) if is_less(x) => lo = mid + 1,
            _ => hi = mid,
        }
    }
    lo
}

/// [`lower_bound`] の、比べる関数が誤りを返せるもの。1 歩ごとに `cost::SEARCH_STEP`（大きな表で読む所がキャッシュから
/// 外れる分を含む）を払い、名前の長さに比例する手間は比べる関数の中で払う。
#[expect(clippy::arithmetic_side_effects, reason = "lo ≤ mid < hi ≤ len なので、あふれない")]
pub(crate) fn lower_bound_try<T>(
    budget: &crate::budget::Budget<'_>,
    s: &[T],
    mut is_less: impl FnMut(&T) -> Result<bool, Error>,
) -> Result<usize, Error> {
    let (mut lo, mut hi) = (0usize, s.len());
    while lo < hi {
        budget.spend(crate::cost::SEARCH_STEP)?;
        let mid = lo + (hi - lo) / 2;
        match s.get(mid) {
            Some(x) if is_less(x)? => lo = mid + 1,
            _ => hi = mid,
        }
    }
    Ok(lo)
}

/// 並べた並び（同じ値は入れた順）。
pub(crate) struct SortedVec<'b, T: Plain + Ord> {
    v: MVec<'b, T>,
}

impl<'b, T: Plain + Ord> SortedVec<'b, T> {
    pub(crate) fn new(budget: &'b Budget<'b>) -> SortedVec<'b, T> {
        SortedVec { v: MVec::new(budget) }
    }

    pub(crate) fn with_capacity(budget: &'b Budget<'b>, n: usize) -> Result<SortedVec<'b, T>, Error> {
        Ok(SortedVec { v: MVec::with_capacity(budget, n)? })
    }

    pub(crate) fn len(&self) -> usize {
        self.v.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.v.is_empty()
    }

    pub(crate) fn as_slice(&self) -> &[T] {
        self.v.as_slice()
    }

    /// 頼んだ容量（見積もりの式に渡す）。
    pub(crate) fn capacity(&self) -> usize {
        self.v.capacity()
    }

    /// `x` と等しい最初の位置（なければ None）。
    pub(crate) fn find(&self, x: &T) -> Result<Option<usize>, Error> {
        self.v.budget().spend(cost::search(self.v.len() as u64))?;
        let i = lower_bound(self.v.as_slice(), |y| y < x);
        Ok(self.v.get(i).filter(|y| *y == x).map(|_| i))
    }

    /// 入れる（同じ値があれば、その後ろ。入れた位置を返す）。
    pub(crate) fn insert(&mut self, x: T) -> Result<usize, Error> {
        self.v.budget().spend(cost::search(self.v.len() as u64))?;
        let i = lower_bound(self.v.as_slice(), |y| *y <= x);
        self.v.insert(i, x)?;
        Ok(i)
    }

    /// なければ入れる（入れたら真）。探すのは 1 度（cost::sorted_insert の式）。
    pub(crate) fn insert_unique(&mut self, x: T) -> Result<bool, Error> {
        self.v.budget().spend(cost::search(self.v.len() as u64))?;
        let i = lower_bound(self.v.as_slice(), |y| *y < x);
        if self.v.get(i) == Some(&x) {
            return Ok(false);
        }
        self.v.insert(i, x)?;
        Ok(true)
    }
}

/// 鍵で並べた表（同じ鍵は 1 つ。`insert_first` は先に入れたものを残す。FR-ST-13 の「同じ id は最初の要素」）。
pub(crate) struct SortedMap<'b, K: Plain + Ord, V: Plain> {
    keys: MVec<'b, K>,
    vals: MVec<'b, V>,
}

impl<'b, K: Plain + Ord, V: Plain> SortedMap<'b, K, V> {
    pub(crate) fn new(budget: &'b Budget<'b>) -> SortedMap<'b, K, V> {
        SortedMap { keys: MVec::new(budget), vals: MVec::new(budget) }
    }

    pub(crate) fn with_capacity(budget: &'b Budget<'b>, n: usize) -> Result<SortedMap<'b, K, V>, Error> {
        Ok(SortedMap { keys: MVec::with_capacity(budget, n)?, vals: MVec::with_capacity(budget, n)? })
    }

    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// 頼んだ容量（鍵と値で同じ。見積もりの式に渡す）。
    pub(crate) fn capacity(&self) -> usize {
        self.keys.capacity().min(self.vals.capacity())
    }

    pub(crate) fn keys(&self) -> &[K] {
        self.keys.as_slice()
    }

    pub(crate) fn values(&self) -> &[V] {
        self.vals.as_slice()
    }

    fn position(&self, k: &K) -> Result<(usize, bool), Error> {
        self.keys.budget().spend(cost::search(self.keys.len() as u64))?;
        let i = lower_bound(self.keys.as_slice(), |y| y.cmp(k) == Ordering::Less);
        let found = self.keys.get(i).is_some_and(|y| y == k);
        Ok((i, found))
    }

    /// 鍵の値（なければ None）。
    pub(crate) fn get(&self, k: &K) -> Result<Option<&V>, Error> {
        let (i, found) = self.position(k)?;
        Ok(if found { self.vals.get(i) } else { None })
    }

    /// 鍵がなければ入れる（入れたら真。あれば先のものを残して偽）。
    pub(crate) fn insert_first(&mut self, k: K, v: V) -> Result<bool, Error> {
        let (i, found) = self.position(&k)?;
        if found {
            return Ok(false);
        }
        self.reserve_one()?;
        self.keys.insert(i, k)?;
        self.vals.insert(i, v)?;
        Ok(true)
    }

    /// 鍵の値を入れる（あれば置き換える。置き換えたら真）。
    pub(crate) fn insert_or_replace(&mut self, k: K, v: V) -> Result<bool, Error> {
        let (i, found) = self.position(&k)?;
        if found {
            if let Some(slot) = self.vals.get_mut(i) {
                *slot = v;
            }
            return Ok(true);
        }
        self.reserve_one()?;
        self.keys.insert(i, k)?;
        self.vals.insert(i, v)?;
        Ok(false)
    }

    /// 鍵と値の両方に 1 つ分の容量を先に取る（確保の失敗で鍵だけが入ることのないように）。
    fn reserve_one(&mut self) -> Result<(), Error> {
        let need = self.keys.len().saturating_add(1);
        self.keys.ensure(need)?;
        self.vals.ensure(need)
    }
}
