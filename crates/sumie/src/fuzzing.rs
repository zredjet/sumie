//! ファズの対象と変異の試験の入り口（`#[doc(hidden)]`。公開の API ではなく、Kura は使わない）。
//!
//! 入力のバイト列で決めた操作で中の部品を動かし、不変の条件を確かめる。条件が崩れたら [`Check::Violation`] を返す
//! （ライブラリは panic しないので、ファズの対象と試験の側が落とす）。上限で止まるのは正しい振る舞い。

use crate::budget::Budget;
use crate::cancel::Never;
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
    /// 払った手間
    pub work: u64,
    /// 上限で止まったか
    pub limited: bool,
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

/// mem の入れ物（`MVec`・`SortedVec`・`SortedMap`・`stable_sort`）を動かす。最初の 2 バイトで手間と確保の上限を決める。
pub fn mem_ops(data: &[u8]) -> MemOutcome {
    let b0 = data.first().copied().unwrap_or(0);
    let b1 = data.get(1).copied().unwrap_or(0);
    let work_limit = 1u64.checked_shl(u32::from(b0 % 24).saturating_add(4)).unwrap_or(u64::MAX);
    let bytes_limit = 1u64.checked_shl(u32::from(b1 % 20).saturating_add(6)).unwrap_or(u64::MAX);
    let never = Never;
    let budget = Budget::new(Stage::Build, work_limit, bytes_limit, &never);
    let r = run_mem_ops(&budget, data.get(2..).unwrap_or(&[]), bytes_limit);
    let (check, limited) = match r {
        Ok(c) => (c, false),
        Err(Error::Limit(l)) => {
            // 断るのは上限を超えるときだけ
            let ok = l.needed > l.allowed && matches!(l.resource, Resource::Work | Resource::Memory);
            (if ok { Check::Ok } else { Check::Violation("Limit の needed が allowed を超えていない") }, true)
        }
        Err(_) => (Check::Violation("Limit でない誤り"), true),
    };
    // すべて捨てたら、数えた量は 0 に戻る（Drop で返し忘れがない）
    let check = if check == Check::Ok && budget.bytes() != 0 {
        Check::Violation("捨てたあとも数えた量が残る")
    } else {
        check
    };
    MemOutcome { check, counted_peak: budget.peak(), work: budget.work(), limited }
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
    while let Some((op, arg)) = ops.next() {
        serial = serial.wrapping_add(1);
        match op % 12 {
            0 => v.push(u32::from(arg))?,
            1 => {
                let src = [u32::from(arg); 16];
                v.append_slice(src.get(..usize::from(arg % 17)).unwrap_or(&[]))?;
            }
            2 => v.resize_to(usize::from(arg).saturating_mul(4), 7)?,
            3 => {
                let last = v.last().copied();
                if v.pop() != last {
                    return Ok(Check::Violation("MVec::pop が last と違う"));
                }
            }
            4 if arg < 8 => v.clear(),
            4 => v.truncate(usize::from(arg)),
            5 => {
                let x = u16::from(arg).saturating_mul(3) % 50;
                let at = sv.insert(x)?;
                if sv.as_slice().get(at) != Some(&x) {
                    return Ok(Check::Violation("SortedVec::insert の位置が違う"));
                }
            }
            6 => {
                let k = u16::from(arg % 32);
                let first = m.get(&k)?.copied();
                let inserted = m.insert_first(k, serial)?;
                if inserted == first.is_some() {
                    return Ok(Check::Violation("insert_first の真偽が違う"));
                }
                let now = m.get(&k)?.copied();
                if now != Some(first.unwrap_or(serial)) {
                    return Ok(Check::Violation("insert_first が先の値を残さない"));
                }
            }
            7 => {
                if let Some(c) = sort_check(budget, &v, arg)? {
                    return Ok(c);
                }
            }
            8 => {
                // 書き換え
                let i = usize::from(arg).checked_rem(v.len()).unwrap_or(0);
                if let Some(x) = v.get_mut(i) {
                    *x = x.wrapping_add(1);
                }
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
                let had = found.is_some();
                if sv.insert_unique(x)? == had {
                    return Ok(Check::Violation("insert_unique の真偽が違う"));
                }
            }
            10 => {
                let k = u16::from(arg % 32);
                let had = m.get(&k)?.is_some();
                if m.insert_or_replace(k, serial)? != had || m.get(&k)?.copied() != Some(serial) {
                    return Ok(Check::Violation("insert_or_replace が置き換えない"));
                }
            }
            _ => {
                let at = usize::from(arg).checked_rem(v.len().saturating_add(1)).unwrap_or(0);
                // 見積もり：ずらす分と、伸ばすなら今の全体の写し
                let size = size_of::<u32>() as u64;
                let moved = v.len().saturating_sub(at) as u64;
                let grow = if v.len() == v.capacity() { cost::copy((v.len() as u64).saturating_mul(size)) } else { 0 };
                let estimate = cost::shift(moved, size).saturating_add(grow);
                let before = budget.work();
                v.insert(at, u32::from(arg))?;
                if budget.work().saturating_sub(before) > estimate {
                    return Ok(Check::Violation("MVec::insert の手間が見積もりを超えた"));
                }
                if v.get(at) != Some(&u32::from(arg)) {
                    return Ok(Check::Violation("MVec::insert の位置が違う"));
                }
            }
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

/// `v` の値を鍵（下の 4 ビットを落とす）で安定に並べ、並びと安定さを確かめる。
fn sort_check(budget: &Budget<'_>, v: &MVec<'_, u32>, shift: u8) -> Result<Option<Check>, Error> {
    let mut keyed: MVec<'_, Keyed> = MVec::with_capacity(v.budget(), v.len())?;
    for (i, x) in v.as_slice().iter().enumerate() {
        keyed.push(Keyed {
            key: x.checked_shr(u32::from(shift % 8)).unwrap_or(0),
            idx: u32::try_from(i).unwrap_or(u32::MAX),
        })?;
    }
    let before = budget.work();
    stable_sort(budget, keyed.as_mut_slice(), |a, b| a.key < b.key)?;
    let spent = budget.work().saturating_sub(before);
    let estimate = cost::stable_sort(keyed.len() as u64, size_of::<Keyed>() as u64);
    if spent > estimate {
        return Ok(Some(Check::Violation("stable_sort の手間が見積もりを超えた")));
    }
    let ok = sorted_by(keyed.as_slice(), |a, b| a.key < b.key || (a.key == b.key && a.idx < b.idx));
    Ok(if ok { None } else { Some(Check::Violation("stable_sort が並ばないか、安定でない")) })
}

/// math の max・min・clamp・powi の性質を、入力のバイト列から作った `f64` で確かめる（超越関数は panic しないことだけ）。
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
            if mx.to_bits() != a.to_bits() && mx.to_bits() != b.to_bits() {
                return Check::Violation("max がどちらでもない");
            }
        } else if (a.is_nan() && mx.to_bits() != b.to_bits())
            || (!a.is_nan() && b.is_nan() && mx.to_bits() != a.to_bits())
        {
            return Check::Violation("max が NaN を捨てない");
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
        // 超越関数は panic しないこと（値は golden で照らす）
        let _ = (math::sin(a), math::atan2(a, b), math::pow(a, b), math::hypot(a, b), math::ln(a), math::cbrt(a));
        let _ = (math::cos(b), math::tan(b), math::asin(b), math::acos(b), math::atan(b), math::exp(b), math::exp2(b));
        let _ = (math::log2(c), math::log10(c), math::sinh(c), math::cosh(c), math::tanh(c));
        let _ = (math::DEG_TO_RAD, math::RAD_TO_DEG);
    }
    Check::Ok
}
