//! ファズの対象と変異の試験の入り口（`#[doc(hidden)]`。公開の API ではなく、Kura は使わない）。
//!
//! 入力のバイト列で決めた操作で中の部品を動かし、不変の条件を確かめる。条件が崩れたら [`Check::Violation`] を返す
//! （ライブラリは panic しないので、ファズの対象と試験の側が落とす）。上限・中止で止まるのは正しい振る舞い。
//!
//! 操作ごとに、払った手間と確保の山を `cost` の式（見積もり）と照らす（見積もり ≥ 実際）。

use crate::budget::{Budget, CANCEL_INTERVAL};
use crate::cancel::{Cancel, Never};
use crate::cost;
use crate::error::{Error, Resource, Stage};
use crate::math;
use crate::mem::{Frozen, MVec, Plain, SortedMap, SortedVec, lower_bound, stable_sort};

/// 確かめた結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// 条件を満たした
    Ok,
    /// 条件が崩れた（何が）
    Violation(&'static str),
}

/// [`mem_ops`] の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemOutcome {
    pub check: Check,
    /// 数えた確保の山（試験で、ヒープの実際の山と比べる）
    pub counted_peak: u64,
    /// 数えた確保の合計と回数（試験で、ヒープの確保の合計・回数と比べる）
    pub charged_total: u64,
    pub charges: u64,
    /// 払った手間
    pub work: u64,
    /// 上限で止まったか
    pub limited: bool,
    /// 中止で止まったか
    pub cancelled: bool,
}

/// 並べ替えの安定さを確かめる組（鍵と元の位置）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Keyed {
    key: u32,
    idx: u32,
}

impl Plain for Keyed {}

/// バイト列を (操作, 引数) の組として読む。
struct Ops<'a> {
    data: &'a [u8],
    at: usize,
}

impl Ops<'_> {
    fn next(&mut self) -> Option<(u8, u8)> {
        let op = *self.data.get(self.at)?;
        let arg = self.data.get(self.at.saturating_add(1)).copied().unwrap_or(0);
        self.at = self.at.saturating_add(2);
        Some((op, arg))
    }
}

/// 1 つの操作の前の勘定（払った手間・数えた量・山）。操作のあとに見積もり（手間・確保の増え）と照らす。
struct Before {
    work: u64,
    bytes: u64,
    peak: u64,
}

impl Before {
    fn of(b: &Budget<'_>) -> Before {
        Before { work: b.work(), bytes: b.bytes(), peak: b.peak() }
    }

    /// 払った手間 ≤ 見積もりの手間、山 ≤ max(前の山, 前の量 + 見積もりの増え)。
    fn within(&self, b: &Budget<'_>, est: (u64, u64)) -> bool {
        let spent = b.work().saturating_sub(self.work);
        let peak_bound = self.peak.max(self.bytes.saturating_add(est.1));
        spent <= est.0 && b.peak() <= peak_bound
    }
}

/// mem の入れ物（`MVec`・`SortedVec`・`SortedMap`・`stable_sort`）を動かす。最初の 2 バイトで手間と確保の上限を決める。
pub fn mem_ops(data: &[u8]) -> MemOutcome {
    mem_ops_with(data, &Never)
}

/// [`mem_ops`] を、渡した中止の印で回す。
pub fn mem_ops_with(data: &[u8], cancel: &dyn Cancel) -> MemOutcome {
    let b0 = data.first().copied().unwrap_or(0);
    let b1 = data.get(1).copied().unwrap_or(0);
    let work_limit = 1u64.checked_shl(u32::from(b0 % 24).saturating_add(4)).unwrap_or(u64::MAX);
    let bytes_limit = 1u64.checked_shl(u32::from(b1 % 20).saturating_add(6)).unwrap_or(u64::MAX);
    let budget = Budget::new(Stage::Build, work_limit, bytes_limit, cancel);
    let r = run_mem_ops(&budget, data.get(2..).unwrap_or(&[]), bytes_limit);
    let (check, limited, cancelled) = match r {
        Ok(c) => (c, false, false),
        Err(Error::Limit(l)) => {
            // 断るのは上限を超えるときだけ
            let ok = l.needed > l.allowed && matches!(l.resource, Resource::Work | Resource::Memory);
            (if ok { Check::Ok } else { Check::Violation("Limit の needed が allowed を超えていない") }, true, false)
        }
        Err(Error::Cancelled) => (Check::Ok, false, true),
        Err(_) => (Check::Violation("Limit・Cancelled でない誤り"), true, false),
    };
    // すべて捨てたら、数えた量は 0 に戻る（Drop で返し忘れがない・二重に返さない）
    let check = if check != Check::Ok {
        check
    } else if budget.over_released() {
        Check::Violation("数えた量より多く返した（二重に返した）")
    } else if budget.bytes() != 0 {
        Check::Violation("捨てたあとも数えた量が残る")
    } else if budget.max_spend() >= CANCEL_INTERVAL {
        Check::Violation("1 回の spend が CANCEL_INTERVAL 以上（中止の速さの約束が崩れる）")
    } else {
        check
    };
    let (charged_total, charges) = budget.charged();
    MemOutcome { check, counted_peak: budget.peak(), charged_total, charges, work: budget.work(), limited, cancelled }
}

fn run_mem_ops(budget: &Budget<'_>, data: &[u8], bytes_limit: u64) -> Result<Check, Error> {
    // 先頭のバイトで、まとめて取るか空から伸ばすかを選ぶ
    let pre = usize::from(data.first().copied().unwrap_or(0) % 8);
    let mut v: MVec<'_, u32> = MVec::new(budget);
    let mut sv: SortedVec<'_, u16> =
        if pre % 2 == 0 { SortedVec::new(budget) } else { SortedVec::with_capacity(budget, pre)? };
    let mut m: SortedMap<'_, u16, u32> =
        if pre < 4 { SortedMap::new(budget) } else { SortedMap::with_capacity(budget, pre)? };
    let mut ops = Ops { data, at: 0 };
    let mut serial = 0u32;
    const S4: u64 = 4;
    const S2: u64 = 2;
    while let Some((op, arg)) = ops.next() {
        serial = serial.wrapping_add(1);
        let before = Before::of(budget);
        let (len, cap) = (v.len() as u64, v.capacity() as u64);
        let est = match op % 12 {
            0 => {
                v.push(u32::from(arg))?;
                cost::append(len, cap, 1, S4)
            }
            1 => {
                let src = [u32::from(arg); 16];
                let k = usize::from(arg % 17);
                v.append_slice(src.get(..k).unwrap_or(&[]))?;
                cost::append(len, cap, k as u64, S4)
            }
            2 => {
                let n = usize::from(arg).saturating_mul(4);
                v.resize_to(n, 7)?;
                cost::resize(len, cap, n as u64, S4)
            }
            3 => {
                let last = v.last().copied();
                if v.pop() != last {
                    return Ok(Check::Violation("MVec::pop が last と違う"));
                }
                (0, 0)
            }
            4 => {
                if arg < 8 {
                    v.clear();
                } else {
                    v.truncate(usize::from(arg));
                }
                (0, 0)
            }
            5 => {
                let x = u16::from(arg).saturating_mul(3) % 50;
                let est = cost::sorted_insert(sv.len() as u64, sv.capacity() as u64, S2);
                let at = sv.insert(x)?;
                if sv.as_slice().get(at) != Some(&x) {
                    return Ok(Check::Violation("SortedVec::insert の位置が違う"));
                }
                est
            }
            6 => {
                let k = u16::from(arg % 32);
                let est = cost::map_insert(m.len() as u64, m.capacity() as u64, S2, S4);
                let first = m.get(&k)?.copied();
                let before = Before::of(budget);
                let inserted = m.insert_first(k, serial)?;
                if !before.within(budget, est) {
                    return Ok(Check::Violation("SortedMap::insert_first が見積もりを超えた"));
                }
                if inserted == first.is_some() {
                    return Ok(Check::Violation("insert_first の真偽が違う"));
                }
                if m.get(&k)?.copied() != Some(first.unwrap_or(serial)) {
                    return Ok(Check::Violation("insert_first が先の値を残さない"));
                }
                (u64::MAX, u64::MAX)
            }
            7 => {
                if let Some(c) = sort_check(budget, &v, arg)? {
                    return Ok(c);
                }
                (u64::MAX, u64::MAX)
            }
            8 => {
                // 書き換え
                let i = usize::from(arg).checked_rem(v.len()).unwrap_or(0);
                if let Some(x) = v.get_mut(i) {
                    *x = x.wrapping_add(1);
                }
                (0, 0)
            }
            9 => {
                // 探す（lower_bound は最初の等しい位置）
                let x = u16::from(arg).saturating_mul(3) % 50;
                let found = sv.find(&x)?;
                let lb = lower_bound(sv.as_slice(), |y| *y < x);
                let want = if sv.as_slice().get(lb) == Some(&x) { Some(lb) } else { None };
                if found != want {
                    return Ok(Check::Violation("SortedVec::find が最初の等しい位置でない"));
                }
                let est = cost::sorted_insert(sv.len() as u64, sv.capacity() as u64, S2);
                let before = Before::of(budget);
                if sv.insert_unique(x)? == found.is_some() {
                    return Ok(Check::Violation("insert_unique の真偽が違う"));
                }
                if !before.within(budget, est) {
                    return Ok(Check::Violation("insert_unique が見積もりを超えた"));
                }
                (u64::MAX, u64::MAX)
            }
            10 => {
                let k = u16::from(arg % 32);
                let had = m.get(&k)?.is_some();
                let est = cost::map_insert(m.len() as u64, m.capacity() as u64, S2, S4);
                let before = Before::of(budget);
                if m.insert_or_replace(k, serial)? != had || !before.within(budget, est) {
                    return Ok(Check::Violation("insert_or_replace が置き換えないか、見積もりを超えた"));
                }
                if m.get(&k)?.copied() != Some(serial) {
                    return Ok(Check::Violation("insert_or_replace のあとの値が違う"));
                }
                (u64::MAX, u64::MAX)
            }
            _ => {
                let at = usize::from(arg).checked_rem(v.len().saturating_add(1)).unwrap_or(0);
                v.insert(at, u32::from(arg))?;
                if v.get(at) != Some(&u32::from(arg)) {
                    return Ok(Check::Violation("MVec::insert の位置が違う"));
                }
                cost::insert(len, cap, at as u64, S4)
            }
        };
        if !before.within(budget, est) {
            return Ok(Check::Violation("操作の手間か確保の山が cost の式（見積もり）を超えた"));
        }
        // 不変の条件
        if budget.bytes() > bytes_limit {
            return Ok(Check::Violation("数えた量が上限を超えた"));
        }
        if v.len() > v.capacity() || v.counted_bytes() > budget.bytes() || v.is_empty() != v.last().is_none() {
            return Ok(Check::Violation("MVec の容量・数えた量が合わない"));
        }
        if !sorted_by(sv.as_slice(), |a, b| a <= b) {
            return Ok(Check::Violation("SortedVec が並んでいない"));
        }
        if sv.is_empty() != sv.as_slice().is_empty()
            || sv.len() != sv.as_slice().len()
            || m.is_empty() != m.keys().is_empty()
            || m.len() != m.keys().len()
        {
            return Ok(Check::Violation("is_empty と len が合わない"));
        }
        if !sorted_by(m.keys(), |a, b| a < b) || m.keys().len() != m.values().len() {
            return Ok(Check::Violation("SortedMap の鍵が狭義に並んでいない"));
        }
    }
    // 凍らせたものは Budget から切り離される（数えた量は持ち続ける量として Frozen に移る）
    let held = v.counted_bytes();
    let len = v.len();
    let frozen = v.freeze();
    if frozen.bytes() != held
        || frozen.len() != len
        || frozen.is_empty() != (len == 0)
        || frozen.as_slice().len() != len
    {
        return Ok(Check::Violation("Frozen の量が数えた量と違う"));
    }
    let empty: Frozen<u8> = Frozen::empty();
    if !empty.is_empty() || empty.bytes() != 0 {
        return Ok(Check::Violation("Frozen::empty が空でない"));
    }
    // 凍らせた分は段の勘定に残るので、ここで返して「すべて捨てたら 0」を確かめる
    budget.release(held);
    drop(frozen);
    Ok(Check::Ok)
}

fn sorted_by<T>(s: &[T], ok: impl Fn(&T, &T) -> bool) -> bool {
    s.iter().zip(s.iter().skip(1)).all(|(a, b)| ok(a, b))
}

/// `v` の値を鍵（下の数ビットを落とす）で安定に並べ、並びと安定さと、手間 ≤ 見積もりを確かめる。
fn sort_check(budget: &Budget<'_>, v: &MVec<'_, u32>, shift: u8) -> Result<Option<Check>, Error> {
    let mut keyed: MVec<'_, Keyed> = MVec::with_capacity(v.budget(), v.len())?;
    for (i, x) in v.as_slice().iter().enumerate() {
        keyed.push(Keyed {
            key: x.checked_shr(u32::from(shift % 8)).unwrap_or(0),
            idx: u32::try_from(i).unwrap_or(u32::MAX),
        })?;
    }
    let before = Before::of(budget);
    stable_sort(budget, keyed.as_mut_slice(), |a, b| a.key < b.key)?;
    let n = keyed.len() as u64;
    let size = size_of::<Keyed>() as u64;
    // 作業の領域の確保（1 回）と、写し・併合の手間
    let est = (cost::stable_sort(n, size), cost::stable_sort_bytes(n, size));
    if !before.within(budget, est) {
        return Ok(Some(Check::Violation("stable_sort の手間か確保が見積もりを超えた")));
    }
    let ok = sorted_by(keyed.as_slice(), |a, b| a.key < b.key || (a.key == b.key && a.idx < b.idx));
    Ok(if ok { None } else { Some(Check::Violation("stable_sort が並ばないか、安定でない")) })
}

/// math の max・min・clamp・powi の性質を、入力のバイト列から作った `f64` で確かめる（超越関数は panic しないことだけ。
/// 値はダイジェストで照らす）。
pub fn math_ops(data: &[u8]) -> Check {
    let mut at = 0usize;
    let mut next = || -> Option<f64> {
        let b = data.get(at..at.saturating_add(8))?;
        at = at.saturating_add(8);
        let mut a = [0u8; 8];
        for (d, s) in a.iter_mut().zip(b) {
            *d = *s;
        }
        Some(f64::from_bits(u64::from_le_bytes(a)))
    };
    while let (Some(a), Some(b), Some(c)) = (next(), next(), next()) {
        let mx = math::max(a, b);
        let mn = math::min(a, b);
        if !a.is_nan() && !b.is_nan() {
            if mx < a || mx < b || mn > a || mn > b {
                return Check::Violation("max・min が順を守らない");
            }
            if (mx.to_bits() != a.to_bits() && mx.to_bits() != b.to_bits())
                || (mn.to_bits() != a.to_bits() && mn.to_bits() != b.to_bits())
            {
                return Check::Violation("max・min がどちらでもない");
            }
            // ±0：max は +0 を、min は -0 を選ぶ
            if a == 0.0 && b == 0.0 && (a.is_sign_positive() || b.is_sign_positive()) && mx.is_sign_negative() {
                return Check::Violation("max(±0) が +0 でない");
            }
            if a == 0.0 && b == 0.0 && (a.is_sign_negative() || b.is_sign_negative()) && mn.is_sign_positive() {
                return Check::Violation("min(±0) が -0 でない");
            }
        } else {
            // NaN は捨てる（片方が NaN ならもう片方）
            let want = if a.is_nan() { b } else { a };
            if (!want.is_nan() && (mx.to_bits() != want.to_bits() || mn.to_bits() != want.to_bits()))
                || (want.is_nan() && (!mx.is_nan() || !mn.is_nan()))
            {
                return Check::Violation("max・min が NaN を捨てない");
            }
        }
        let (lo, hi) = (math::min(b, c), math::max(b, c));
        let cl = math::clamp(a, lo, hi);
        if !a.is_nan() && !lo.is_nan() && !hi.is_nan() && (cl < lo || cl > hi) {
            return Check::Violation("clamp が範囲の外");
        }
        let n = i32::from(c.to_bits() as i8);
        let p = math::powi(a, n);
        if n == 0 && p != 1.0 {
            return Check::Violation("powi(x, 0) が 1 でない");
        }
        // 超越関数は panic しないこと（値はダイジェストで照らす）。呼び出しを消されないように black_box で包む
        std::hint::black_box((math::sin(a), math::atan2(a, b), math::pow(a, b), math::hypot(a, b), math::ln(a)));
        std::hint::black_box((math::cbrt(a), math::cos(b), math::tan(b), math::asin(b), math::acos(b)));
        std::hint::black_box((math::atan(b), math::exp(b), math::exp2(b), math::log2(c), math::log10(c)));
        std::hint::black_box((math::sinh(c), math::cosh(c), math::tanh(c), math::DEG_TO_RAD, math::RAD_TO_DEG));
    }
    Check::Ok
}
