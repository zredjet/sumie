//! ファズの対象と変異の試験の入り口（`#[doc(hidden)]`。公開の API ではなく、Kura は使わない）。
//!
//! 入力のバイト列で決めた操作で中の部品を動かし、不変の条件を確かめる。条件が崩れたら [`Check::Violation`] を返す
//! （ライブラリは panic しないので、ファズの対象と試験の側が落とす）。上限・中止で止まるのは正しい振る舞い。
//!
//! 操作ごとに、払った手間と確保の山を `cost` の式（見積もり）と照らす（見積もり ≥ 実際）。

use crate::budget::{Budget, CANCEL_INTERVAL};
use crate::cancel::{Always, Cancel, Never};
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

/// mem の入れ物（`MVec`・`SortedVec`・`SortedMap`・`stable_sort`）を動かす。最初の 2 バイトで手間と確保の上限を決める
/// （手間の上限のバイトの上の位が 1 なら、中止の印を立てたまま回して、中止の道を通す）。
pub fn mem_ops(data: &[u8]) -> MemOutcome {
    if data.first().is_some_and(|b| b & 0x80 != 0) { mem_ops_with(data, &Always) } else { mem_ops_with(data, &Never) }
}

/// [`mem_ops`] を、渡した中止の印で回す。
pub fn mem_ops_with(data: &[u8], cancel: &dyn Cancel) -> MemOutcome {
    let b0 = data.first().copied().unwrap_or(0) & 0x7f;
    let b1 = data.get(1).copied().unwrap_or(0);
    let work_limit = 1u64.checked_shl(u32::from(b0 % 24).saturating_add(4)).unwrap_or(u64::MAX);
    let bytes_limit = 1u64.checked_shl(u32::from(b1 % 20).saturating_add(6)).unwrap_or(u64::MAX);
    let budget = Budget::new(Stage::Build, work_limit, bytes_limit, cancel);
    let r = run_mem_ops(&budget, data.get(2..).unwrap_or(&[]), bytes_limit);
    let (check, limited, cancelled) = match r {
        Ok(c) => (c, false, false),
        Err(Error::Limit(l)) => {
            // 断るのは上限を超えるとき（Work・Memory）か、確保器が断ったとき（Allocator。環境に依る）だけ
            let ok = (l.needed > l.allowed && matches!(l.resource, Resource::Work | Resource::Memory))
                || l.resource == Resource::Allocator;
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
    let s4 = size_of::<u32>() as u64;
    let s2 = size_of::<u16>() as u64;
    while let Some((op, arg)) = ops.next() {
        serial = serial.wrapping_add(1);
        let before = Before::of(budget);
        let (len, cap) = (v.len() as u64, v.capacity() as u64);
        let est = match op % 12 {
            0 => {
                v.push(u32::from(arg))?;
                cost::append(len, cap, 1, s4)
            }
            1 => {
                let src = [u32::from(arg); 16];
                let k = usize::from(arg % 17);
                v.append_slice(src.get(..k).unwrap_or(&[]))?;
                cost::append(len, cap, k as u64, s4)
            }
            2 => {
                // 上の位が立っていれば 2 の冪（塊の境目を越える大きさまで。上限は bytes_limit が抑える）
                let n = if arg & 0x80 != 0 {
                    1usize.checked_shl(u32::from(arg % 22)).unwrap_or(0)
                } else {
                    usize::from(arg).saturating_mul(4)
                };
                v.resize_to(n, 7)?;
                cost::resize(len, cap, n as u64, s4)
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
                let est = cost::sorted_insert(sv.len() as u64, sv.capacity() as u64, s2);
                let at = sv.insert(x)?;
                if sv.as_slice().get(at) != Some(&x) {
                    return Ok(Check::Violation("SortedVec::insert の位置が違う"));
                }
                est
            }
            6 => {
                let k = u16::from(arg % 32);
                let est = cost::map_insert(m.len() as u64, m.capacity() as u64, s2, s4);
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
                // 線形の走査（最初の等しい位置）と、lower_bound（最初の x 以上の位置）と照らす
                let want = sv.as_slice().iter().position(|y| *y == x);
                let lb = lower_bound(sv.as_slice(), |y| *y < x);
                let first_ge = sv.as_slice().iter().position(|y| *y >= x).unwrap_or(sv.len());
                if found != want || lb != first_ge {
                    return Ok(Check::Violation("SortedVec::find・lower_bound が最初の位置でない"));
                }
                let est = cost::sorted_insert(sv.len() as u64, sv.capacity() as u64, s2);
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
                let est = cost::map_insert(m.len() as u64, m.capacity() as u64, s2, s4);
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
                cost::insert(len, cap, at as u64, s4)
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
    let n = v.len() as u64;
    let size = size_of::<Keyed>() as u64;
    // 組を作る：まとめて取る 1 回の確保と、n 回の push（伸ばさない）
    let build = Before::of(budget);
    let mut keyed: MVec<'_, Keyed> = MVec::with_capacity(v.budget(), v.len())?;
    for (i, x) in v.as_slice().iter().enumerate() {
        keyed.push(Keyed {
            key: x.checked_shr(u32::from(shift % 8)).unwrap_or(0),
            idx: u32::try_from(i).unwrap_or(u32::MAX),
        })?;
    }
    let alloc = if n > 0 { cost::ALLOC_STEP } else { 0 };
    let build_est = (alloc.saturating_add(n.saturating_mul(cost::append(0, 1, 1, size).0)), n.saturating_mul(size));
    if !build.within(budget, build_est) {
        return Ok(Some(Check::Violation("組を作る手間か確保が見積もりを超えた")));
    }
    let before = Before::of(budget);
    stable_sort(budget, keyed.as_mut_slice(), |a, b| a.key < b.key)?;
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
        // 小さな冪は掛ける順の決まった式と同じビット（x^2 = x·x、x^3 = x·(x·x)、負は逆数）
        let small = match n {
            1 => Some(a),
            2 => Some(a * a),
            3 => Some(a * (a * a)),
            -1 => Some(1.0 / a),
            -2 => Some(1.0 / (a * a)),
            _ => None,
        };
        if let Some(want) = small
            && !(want.is_nan() && p.is_nan())
            && want.to_bits() != p.to_bits()
        {
            return Check::Violation("powi の小さな冪が決めた掛ける順と違う");
        }
        // 超越関数は panic しないこと（値はダイジェストで照らす）。呼び出しを消されないように black_box で包む
        std::hint::black_box((math::sin(a), math::atan2(a, b), math::pow(a, b), math::hypot(a, b), math::ln(a)));
        std::hint::black_box((math::cbrt(a), math::cos(b), math::tan(b), math::asin(b), math::acos(b)));
        std::hint::black_box((math::atan(b), math::exp(b), math::exp2(b), math::log2(c), math::log10(c)));
        std::hint::black_box((math::sinh(c), math::cosh(c), math::tanh(c), math::DEG_TO_RAD, math::RAD_TO_DEG));
    }
    Check::Ok
}

/// [`xml_parse`] の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlOutcome {
    pub check: Check,
    /// 読めたか（誤りなら None）
    pub nodes: Option<u64>,
    /// 誤り（読めなかったとき）
    pub error: Option<Error>,
    /// 数える段と作る段の手間・確保の山（作る段は数える段の見積もり以下のはず）
    pub count_work: u64,
    pub count_peak: u64,
    pub build_work: u64,
    pub build_estimate: u64,
    pub build_peak: u64,
    pub build_peak_estimate: u64,
}

/// XML の段（数える段と作る段）を通し、作る段が見積もり（数える段の結果）を超えないこと・節の鎖が正しいことを確かめる。
pub fn xml_parse(data: &[u8]) -> XmlOutcome {
    xml_parse_with(data, &crate::limits::Limits::DEFAULT)
}

/// [`xml_parse`] の上限を変えたもの（試験で深さ・節の数の上限を広げる）。
pub fn xml_parse_with(data: &[u8], limits: &crate::limits::Limits) -> XmlOutcome {
    xml_parse_cancel(data, limits, &Never)
}

/// [`xml_parse_with`] に中止の印を渡すもの（中止の試験。両方の段で同じ印を見る）。
pub fn xml_parse_cancel(data: &[u8], limits: &crate::limits::Limits, cancel: &dyn Cancel) -> XmlOutcome {
    let mut out = XmlOutcome {
        check: Check::Ok,
        nodes: None,
        error: None,
        count_work: 0,
        count_peak: 0,
        build_work: 0,
        build_estimate: 0,
        build_peak: 0,
        build_peak_estimate: 0,
    };
    // 数える段は読む段の上限で払う（読む段の道の頭。ADR-70）
    let est_budget = Budget::new(Stage::Parse, limits.parse_work, limits.parse_bytes, cancel);
    let counts = match crate::xml::count(data, limits, &est_budget, Stage::Parse) {
        Ok(c) => c,
        Err(e) => {
            out.error = Some(e);
            out.count_work = est_budget.work();
            out.count_peak = est_budget.peak();
            // 誤りで止まった道でも、1 回の払いが中止を見る間隔を超えない
            if est_budget.over_released() || est_budget.max_spend() >= CANCEL_INTERVAL {
                out.check = Check::Violation("数える段の勘定が崩れた（誤りの道）");
            }
            return out;
        }
    };
    out.count_work = est_budget.work();
    out.count_peak = est_budget.peak();
    if est_budget.over_released() || est_budget.max_spend() >= CANCEL_INTERVAL {
        out.check = Check::Violation("数える段の勘定が崩れた");
        return out;
    }
    let (work, peak) = (counts.build_work(), counts.build_peak());
    out.build_estimate = work;
    out.build_peak_estimate = peak;
    let budget = Budget::new(Stage::Parse, work, peak, cancel);
    let doc = match crate::xml::build(data, &counts, limits, &budget, Stage::Parse) {
        Ok(d) => d,
        Err(Error::Cancelled) => {
            out.error = Some(Error::Cancelled);
            out.build_work = budget.work();
            if budget.over_released() || budget.max_spend() >= CANCEL_INTERVAL {
                out.check = Check::Violation("作る段の勘定が崩れた（中止の道）");
            }
            return out;
        }
        Err(e) => {
            out.error = Some(e);
            // 数える段が通ったのに作る段が上限で止まるのは見積もりの不具合
            if matches!(e, Error::Limit(l) if l.resource == Resource::Work || l.resource == Resource::Memory) {
                out.check = Check::Violation("作る段が数える段の見積もりを超えた");
            } else {
                out.check = Check::Violation("数える段が通ったのに作る段が誤りになった");
            }
            return out;
        }
    };
    out.build_work = budget.work();
    out.build_peak = budget.peak();
    out.nodes = Some(doc.nodes.len() as u64);
    if doc.nodes.len() as u64 != counts.nodes
        || doc.attrs.len() as u64 != counts.attrs
        || doc.store.len() as u64 != counts.store
    {
        out.check = Check::Violation("作った量が数えた量と違う");
        return out;
    }
    if budget.over_released() || budget.max_spend() >= CANCEL_INTERVAL {
        out.check = Check::Violation("作る段の勘定が崩れた");
        return out;
    }
    // 節の鎖：子の親は自分、兄弟の親は同じ、番号は行きがけの順（子と兄弟は自分より後ろ）
    let nodes = doc.nodes.as_slice();
    for (i, n) in nodes.iter().enumerate() {
        let i = i as u32;
        for (next, want_parent) in [(n.first_child, i), (n.next_sibling, n.parent)] {
            if next == crate::xml::NONE {
                continue;
            }
            match nodes.get(next as usize) {
                Some(c) if next > i && c.parent == want_parent => {}
                _ => {
                    out.check = Check::Violation("節の鎖が崩れた");
                    return out;
                }
            }
        }
        if n.kind == crate::xml::KIND_ELEMENT && doc.attrs_of(n).len() != n.attrs.len() {
            out.check = Check::Violation("属性の範囲が外れた");
            return out;
        }
    }
    out
}

/// [`xml_tree`] が知らせる節。名前空間は種類の名前（`svg`・`xlink`・`xml`・空・`other`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeItem<'a> {
    /// 要素（深さは根の要素が 0）
    Element { depth: u32, ns: &'static str, local: &'a [u8] },
    /// 直前の要素の属性（名前空間の宣言は知らせない）
    Attr { ns: &'static str, local: &'a [u8], value: &'a [u8] },
    /// 文字（親の深さ）
    Text { depth: u32, text: &'a [u8] },
}

fn ns_name(ns: u8) -> &'static str {
    match ns {
        crate::xml::NS_NONE => "",
        crate::xml::NS_SVG => "svg",
        crate::xml::NS_XLINK => "xlink",
        crate::xml::NS_XML => "xml",
        crate::xml::NS_XMLNS => "xmlns",
        _ => "other",
    }
}

/// 入力を読み（[`xml_parse`] と同じ上限）、木を行きがけの順に `f` に知らせる（照らしの道具が roxmltree の木と比べる）。
/// 読めなければ偽。
pub fn xml_tree(data: &[u8], f: &mut dyn FnMut(TreeItem<'_>)) -> bool {
    let limits = crate::limits::Limits::DEFAULT;
    let budget = Budget::new(Stage::Parse, limits.parse_work, limits.parse_bytes, &Never);
    let Ok(counts) = crate::xml::count(data, &limits, &budget, Stage::Parse) else { return false };
    let Ok(doc) = crate::xml::build(data, &counts, &limits, &budget, Stage::Parse) else { return false };
    let nodes = doc.nodes.as_slice();
    // 深さ（親は子より前）
    let budget2 = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut depth: MVec<'_, u32> = MVec::new(&budget2);
    for n in nodes {
        let d = if n.parent == crate::xml::NONE {
            0
        } else {
            depth.get(n.parent as usize).copied().unwrap_or(0).saturating_add(1)
        };
        if depth.push(d).is_err() {
            return false;
        }
    }
    for (i, n) in nodes.iter().enumerate() {
        let d = depth.get(i).copied().unwrap_or(0);
        if n.kind == crate::xml::KIND_ELEMENT {
            // 名前空間のない根の svg の文書で SVG にした要素は、XML の名前空間としてはなし（ADR-78 (2)）
            let ns = if n.flags & crate::xml::FLAG_IMPLIED_SVG != 0 { crate::xml::NS_NONE } else { n.ns };
            f(TreeItem::Element { depth: d, ns: ns_name(ns), local: n.local.of(data) });
            for a in doc.attrs_of(n) {
                if a.ns == crate::xml::NS_XMLNS {
                    continue;
                }
                f(TreeItem::Attr { ns: ns_name(a.ns), local: a.local.of(data), value: doc.value(data, a) });
            }
        } else {
            f(TreeItem::Text { depth: d.saturating_sub(1), text: doc.text(data, n) });
        }
    }
    true
}

/// DTD の内部のサブセットを `data` にして、実体を本文と属性から参照する文書を [`xml_parse`] に通す（ファズの `dtd` の対象）。
pub fn dtd_parse(data: &[u8]) -> XmlOutcome {
    // 先頭の 1 バイトで、参照する実体の名前を選ぶ（英小文字ならそのまま、ほかは a〜p に写す）
    let first = data.first().copied().unwrap_or(b'a');
    let name = if first.is_ascii_lowercase() {
        first
    } else {
        b"abcdefghijklmnop".get(usize::from(first % 16)).copied().unwrap_or(b'a')
    };
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut doc: MVec<'_, u8> = MVec::new(&budget);
    let parts: [&[u8]; 3] =
        [b"<!DOCTYPE svg [", data.get(1..).unwrap_or(&[]), b"]><svg xmlns=\"http://www.w3.org/2000/svg\" a=\"&"];
    for p in parts {
        if doc.append_slice(p).is_err() {
            return xml_parse(&[]);
        }
    }
    let tail = [name, b';', b'"', b'>', b'&', name, b';', b'<', b'/', b's', b'v', b'g', b'>'];
    if doc.append_slice(&tail).is_err() {
        return xml_parse(&[]);
    }
    xml_parse(doc.as_slice())
}

/// 入力を CSS として、決まった文書の `<style>` に入れて読む段の全体に通す（ファズの `css` の対象。確かめることは
/// [`document_parse`] と同じ）。文書には、型・id・class・言語・名前空間・リンク・空の要素・深い入れ子を置く。
/// あわせて、入力を規則の並びに読んで文書に照らし、鍵の索引を使う照合と使わない照合が同じ（要素, 規則, 詳細度）の並びを
/// 返すことを確かめる（ADR-75 (1)。CSS の 3 回目のレビュー）。
pub fn css_parse(data: &[u8]) -> DocOutcome {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut doc: MVec<'_, u8> = MVec::new(&budget);
    for p in [CSS_HEAD, data, CSS_TAIL] {
        if doc.append_slice(p).is_err() {
            return document_parse(&[]);
        }
    }
    let mut out = document_parse(doc.as_slice());
    if out.check == Check::Ok && !index_matches_unindexed(doc.as_slice(), data) {
        out.check = Check::Violation("鍵の索引の照合が、索引を使わない照合と違う");
    }
    out
}

/// 照合の知らせの並びの要約（数と、順に混ぜた 64 ビット）。
struct MatchDigest {
    n: u64,
    h: u64,
}

impl MatchDigest {
    fn mix(&mut self, v: u64) {
        self.n = self.n.wrapping_add(1);
        self.h = (self.h ^ v).wrapping_mul(0x0000_0100_0000_01B3).rotate_left(17);
    }
}

impl crate::css::matcher::MatchSink for MatchDigest {
    fn element(&mut self, node: u32) -> Result<(), Error> {
        self.mix(u64::from(node) | (1 << 40));
        Ok(())
    }

    fn rule(&mut self, rule: u32, spec: u32) -> Result<(), Error> {
        self.mix(u64::from(rule) | (u64::from(spec) << 32));
        Ok(())
    }
}

/// 文書 `svg` に CSS `css` を、索引を使う照合と使わない照合で当て、同じ並びか（読めない・誤りなら真）。
fn index_matches_unindexed(svg: &[u8], css: &[u8]) -> bool {
    use crate::css::matcher::{match_rules, match_rules_unindexed};
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let limits = crate::limits::Limits::DEFAULT;
    let Ok(counts) = crate::xml::count(svg, &limits, &budget, Stage::Parse) else { return true };
    let Ok(doc) = crate::xml::build(svg, &counts, &limits, &budget, Stage::Parse) else { return true };
    let mut sb = crate::css::sheet::SheetBuilder::new(&budget);
    if sb.add(css, None, 0).is_err() {
        return true;
    }
    let Ok(sheet) = sb.finish() else { return true };
    let mut active: MVec<'_, u8> = MVec::new(&budget);
    if active.resize_to(sheet.rules.len(), 1).is_err() {
        return true;
    }
    let mut a = MatchDigest { n: 0, h: 0 };
    let mut b = MatchDigest { n: 0, h: 0 };
    let ra = match_rules(&doc, svg, &sheet, active.as_slice(), &budget, &mut a);
    let rb = match_rules_unindexed(&doc, svg, &sheet, active.as_slice(), &budget, &mut b);
    match (ra, rb) {
        (Ok(()), Ok(())) => a.n == b.n && a.h == b.h,
        _ => true,
    }
}

/// [`css_parse`] の文書の頭（CSS の前）。
pub const CSS_HEAD: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:x=\"urn:x\" xmlns:xl=\"http://www.w3.org/1999/xlink\" width=\"120\" height=\"80\" xml:lang=\"en-US\"><style><![CDATA[";
/// [`css_parse`] の文書の終わり（CSS のあと）。
pub const CSS_TAIL: &[u8] = b"]]></style><g id=\"g\" class=\"a b\" fill=\"red\"><rect id=\"r\" class=\"c\" width=\"5\" height=\"5\" style=\"stroke: blue\"/><x:q id=\"q\"/><circle r=\"1\" lang=\"fr\"/><g><g><path d=\"M0 0\"/></g></g><a xl:href=\"#r\"><text>t</text></a><g/></g></svg>";

/// 値の読みの口（style の値の確かめ・values の読み・`@media`・`@supports`）を通す（ファズの `values` の対象）。頭の 1 バイトで
/// プロパティ（下の 7 ビット）と、属性か（上の 1 ビット）を選び、残りを値の字句にする（正しい UTF-8 でなければ何もしない）。
/// 1 回の払いが中止の間隔を超えないこと・同じ入力で同じ結果になること・手間が字句の長さにおおよそ比例することを確かめる。
pub fn values_parse(data: &[u8]) -> Check {
    use crate::style::cond::{Env, media, supports};
    use crate::style::props::PROPS;
    use crate::style::valid::{Ctx, font_parts, valid, wide};
    let Some((&head, text)) = data.split_first() else { return Check::Ok };
    if core::str::from_utf8(text).is_err() {
        return Check::Ok;
    }
    let range = crate::xml::lex::Span::new(0, text.len());
    let Some(p) = PROPS.get(usize::from(head & 0x7F).checked_rem(PROPS.len()).unwrap_or(0)) else { return Check::Ok };
    let attr = head & 0x80 != 0;
    let run = |viewport: bool| -> Result<[bool; 12], Error> {
        let b = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
        let cx = Ctx { attr, unitless: attr || p.unitless, viewport };
        let env = Env {
            color_scheme: crate::style::ColorScheme::Light,
            size: viewport.then_some((100.0, 50.0)),
            font_size: 12.0,
        };
        let out = [
            wide(text, range, &b)?.is_some(),
            valid(p.kind, text, range, cx, &b)?,
            values_at(text, range, &b).paint()?.is_some(),
            values_at(text, range, &b).length(attr)?.is_some(),
            crate::values::transform_attr(text, range, &b)?.is_some(),
            crate::values::transform_css_valid(text, range, viewport, &b)?,
            crate::values::view_box(text, range, &b)?.is_some(),
            crate::values::aspect(text, range, &b)?.is_some(),
            font_parts(text, range, cx, &b)?.is_some(),
            media(text, range, &env, &b)?,
            supports(text, range, crate::css::select::NsTable::EMPTY, &b)?,
            values_at(text, range, &b).color()?.is_some(),
        ];
        if b.max_spend() >= CANCEL_INTERVAL || b.over_released() {
            return Err(Error::Cancelled);
        }
        // 手間は字句の長さにおおよそ比例する（2 乗に伸びない）
        let bound = (text.len() as u64).saturating_add(16).saturating_mul(4096);
        if b.work() > bound {
            return Err(Error::Cancelled);
        }
        Ok(out)
    };
    for viewport in [false, true] {
        match (run(viewport), run(viewport)) {
            (Ok(a), Ok(b)) if a == b => {}
            (Err(Error::Cancelled), _) | (_, Err(Error::Cancelled)) => {
                return Check::Violation("値の読みの勘定が崩れたか、手間が長さに比例しない");
            }
            _ => return Check::Violation("同じ入力で値の読みの結果が違う"),
        }
    }
    Check::Ok
}

/// 値の字句を読む口（[`values_parse`] が使う）。
fn values_at<'a>(text: &'a [u8], range: crate::xml::lex::Span, b: &'a Budget<'_>) -> crate::values::Values<'a, 'a> {
    let b: &'a Budget<'a> = b;
    crate::values::Values::new(text, range, b)
}

/// [`document_parse`] の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocOutcome {
    pub check: Check,
    /// 見積もり（誤りなら None）
    pub estimate: Option<crate::budget::Estimate>,
    /// 誤り（読めなかったとき）
    pub error: Option<Error>,
    /// 読む段の確保の山（ヒープの山と照らす）
    pub peak: u64,
}

/// 読む段の全体（公開の `estimate_parse` と `parse`）を通し、次を確かめる（ファズの `document` の対象）：
///
/// - 2 つの誤りが同じ（D2）・見積もりが通れば読める・手間・山・持ち続ける量が見積もりと同じ（ADR-70）
/// - 1 回の払いが中止の間隔を超えず、返しすぎない
/// - 上限を見積もりのちょうどにすれば読め、1 少なくすれば `estimate_parse` と `parse` が同じ `Limit` を返す
/// - 読めた文書の計算した値のたどり（`doc::walk`）が誤りを返さず、払いが崩れない（確保は `peak` に入れる）
pub fn document_parse(data: &[u8]) -> DocOutcome {
    document_parse_cancel(data, &Never)
}

/// [`document_parse`] に中止の印を渡すもの（中止の試験）。上限のきわの確かめは中止の印なしで行う。
pub fn document_parse_cancel(data: &[u8], cancel: &dyn Cancel) -> DocOutcome {
    use crate::doc::{Watch, estimate_parse, parse, parse_watch};
    let opts = crate::style::ParseOptions::DEFAULT;
    let est = estimate_parse(data, &opts, cancel);
    let mut watch = Watch::default();
    let doc = parse_watch(data, &opts, cancel, &mut watch);
    let mut out =
        DocOutcome { check: Check::Ok, estimate: est.ok(), error: doc.as_ref().err().copied(), peak: watch.peak };
    if watch.over_released || watch.max_spend >= CANCEL_INTERVAL {
        out.check = Check::Violation("読む段の勘定が崩れた");
        return out;
    }
    let e = match (est, doc) {
        (Ok(e), Ok(d)) => {
            if d.held_bytes() != e.result_bytes {
                out.check = Check::Violation("持ち続ける量が見積もりと違う");
                return out;
            }
            // 見積もりは parse と同じ道の実際（ADR-70）
            if watch.work != e.work || watch.peak != e.peak_bytes {
                out.check = Check::Violation("parse の手間か山が見積もりと違う");
                return out;
            }
            // 計算した値のたどり（木を作る段の前の部分。払いを確かめ、確保をヒープの山と照らす山に入れる）
            let wb = Budget::new(Stage::Build, u64::MAX, u64::MAX, cancel);
            // 計算した値は有限（NaN・±∞ を値に入れない。文書の 4 回目のレビュー）
            let finite = core::cell::Cell::new(true);
            let paint = |v: &crate::doc::walk::WalkItem| -> Result<(), Error> {
                use crate::doc::walk::used_paint;
                used_paint(&d, v.node, &v.computed.fill, true, v.computed.color, &wb)?;
                used_paint(&d, v.node, &v.computed.stroke, false, v.computed.color, &wb)?;
                let c = &v.computed;
                let t = [c.transform, c.transform_raw, v.ctm];
                let ok = [c.font_size, c.line_height, c.stroke_width, c.opacity, c.origin_x, c.origin_y]
                    .iter()
                    .chain(c.raw_coef.iter())
                    .chain(c.origin_spec.iter())
                    .all(|x| x.is_finite())
                    && t.iter().all(|m| [m.a, m.b, m.c, m.d, m.e, m.f].iter().all(|x| x.is_finite()));
                if !ok {
                    finite.set(false);
                }
                Ok(())
            };
            match crate::doc::walk::walk(&d, &wb, &mut |v| paint(v)) {
                Ok(()) | Err(Error::Cancelled) => {}
                Err(_) => {
                    out.check = Check::Violation("計算した値のたどりが誤りを返した");
                    return out;
                }
            }
            if wb.over_released() || wb.max_spend() >= CANCEL_INTERVAL {
                out.check = Check::Violation("計算した値のたどりの勘定が崩れた");
                return out;
            }
            if !finite.get() {
                out.check = Check::Violation("計算した値に NaN か ±∞ が入った");
                return out;
            }
            out.peak = out.peak.max(d.held_bytes().saturating_add(wb.peak()));
            // 木の見積もり（展開の数・要るもの）は、同じ文書なら同じで、断るなら上限か未対応（文書の 5 回目のレビュー）
            let b1 = d.estimate_build(cancel);
            let needs = d.needs();
            match &b1 {
                Ok(_) | Err(Error::Cancelled | Error::Unsupported(_)) => {}
                Err(Error::Limit(l)) if l.resource == Resource::Instances => {}
                Err(_) => {
                    out.check = Check::Violation("木の見積もりが思わぬ誤りを返した");
                    return out;
                }
            }
            if !matches!(b1, Err(Error::Cancelled)) && (d.estimate_build(&Never) != b1 || d.needs() != needs) {
                out.check = Check::Violation("木の見積もりか要るものが呼ぶたびに違う");
                return out;
            }
            e
        }
        (_, Err(Error::Cancelled)) | (Err(Error::Cancelled), _) => return out,
        (Ok(_), Err(Error::Limit(l))) if l.resource == Resource::Work || l.resource == Resource::Memory => {
            out.check = Check::Violation("読む段の本体が見積もりを超えた");
            return out;
        }
        (Ok(_), Err(_)) => {
            out.check = Check::Violation("見積もりが通ったのに読めない");
            return out;
        }
        (Err(a), Err(b)) => {
            if a != b {
                out.check = Check::Violation("estimate_parse と parse の誤りが違う");
            }
            // 上限より小さな量の Limit（Memory）は、数えた容量の数え違い（文書の 3 回目のレビュー）
            if let Error::Limit(l) = a
                && l.resource == Resource::Memory
                && l.allowed < opts.limits.parse_bytes
            {
                out.check = Check::Violation("数えた容量が足りず、上限より小さな量で断った");
            }
            return out;
        }
        (Err(_), Ok(_)) => {
            out.check = Check::Violation("見積もりが断ったのに読めた");
            return out;
        }
    };
    // 上限のきわ：ちょうどなら読め、1 少なければ同じ Limit
    let mut exact = opts;
    exact.limits.parse_work = e.work;
    exact.limits.parse_bytes = e.peak_bytes;
    if parse(data, &exact, &Never).is_err() {
        out.check = Check::Violation("上限を見積もりのちょうどにすると読めない");
        return out;
    }
    for less in [0u8, 1] {
        let mut o = exact;
        if less == 0 {
            o.limits.parse_work = e.work.saturating_sub(1);
        } else {
            o.limits.parse_bytes = e.peak_bytes.saturating_sub(1);
        }
        let a = estimate_parse(data, &o, &Never);
        let b = parse(data, &o, &Never).map(|_| ());
        let same = matches!((&a, &b), (Err(x), Err(y)) if x == y && matches!(x, Error::Limit(_)));
        // 見積もりが 0 なら 1 少なくできない
        let zero = if less == 0 { e.work == 0 } else { e.peak_bytes == 0 };
        if !same && !zero {
            out.check = Check::Violation("上限を見積もりより 1 少なくしても、同じ Limit にならない");
            return out;
        }
    }
    out
}

/// 計算した値の照合（`sumie-tools values`。ADR-63 (5)）に渡す 1 つの要素。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComputedItem<'a> {
    /// 節の番号と、親の要素の節の番号
    pub node: u32,
    pub parent: Option<u32>,
    /// 局所名と、SVG の名前空間か
    pub local: &'a [u8],
    pub svg: bool,
    /// `id` 属性の値
    pub id: Option<&'a [u8]>,
    /// 描く木に入る
    pub rendered: bool,
    /// 根からの変形 `[a, b, c, d, e, f]`（解けなければ `ctm_known` が偽）
    pub ctm: [f64; 6],
    pub ctm_known: bool,
    pub opacity: f64,
    pub fill: UsedPaint<'a>,
    pub fill_opacity: f64,
    pub stroke: UsedPaint<'a>,
    pub stroke_opacity: f64,
    pub stroke_width: f64,
    /// `visibility` が `visible`
    pub visible: bool,
}

/// 使う塗り（[`ComputedItem`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UsedPaint<'a> {
    None,
    /// sRGB の r・g・b（0〜255）と α（0〜1）
    Color([f32; 4]),
    /// 塗りの参照先の局所名と id
    Server {
        local: &'a [u8],
        id: &'a [u8],
    },
    /// 参照の輪で取り除いた
    Cut,
    ContextFill,
    ContextStroke,
}

/// 文書の要素を行きがけの順にたどり、計算した値を `f` に渡す（`doc::walk`）。
pub fn computed_values(d: &crate::Document<'_>, f: &mut dyn FnMut(&ComputedItem<'_>)) -> Result<(), Error> {
    use crate::doc::walk::{Used, used_paint, walk};
    let budget = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
    let doc = d.xml();
    let input = d.input();
    let attr_id = |n: u32| -> Result<Option<&[u8]>, Error> {
        match doc.node(n) {
            Some(x) => doc.attr(input, x, crate::xml::NS_NONE, b"id", &budget),
            None => Ok(None),
        }
    };
    walk(d, &budget, &mut |v| {
        let Some(n) = doc.node(v.node) else { return Ok(()) };
        let c = &v.computed;
        let conv = |u: Used| -> Result<UsedPaint<'_>, Error> {
            Ok(match u {
                Used::None => UsedPaint::None,
                Used::Color(x) => UsedPaint::Color([x.r, x.g, x.b, x.a]),
                Used::Server(t) => UsedPaint::Server {
                    local: doc.node(t).map_or(&[][..], |x| x.local.of(input)),
                    id: attr_id(t)?.unwrap_or(&[]),
                },
                Used::Cut => UsedPaint::Cut,
                // 描く木の要素（walk は use・marker の中身を展開しない）には文脈の要素がないので none（FR-PT-01。文書の
                // 6 回目のレビュー）。描かない要素（marker の中など）は文脈の印のまま
                Used::ContextFill | Used::ContextStroke if v.rendered => UsedPaint::None,
                Used::ContextFill => UsedPaint::ContextFill,
                Used::ContextStroke => UsedPaint::ContextStroke,
            })
        };
        let fill = conv(used_paint(d, v.node, &c.fill, true, c.color, &budget)?)?;
        let stroke = conv(used_paint(d, v.node, &c.stroke, false, c.color, &budget)?)?;
        let t = v.ctm;
        f(&ComputedItem {
            node: v.node,
            parent: (v.parent != crate::xml::NONE).then_some(v.parent),
            local: n.local.of(input),
            svg: n.ns == crate::xml::NS_SVG,
            id: attr_id(v.node)?,
            rendered: v.rendered,
            ctm: [t.a, t.b, t.c, t.d, t.e, t.f],
            ctm_known: v.ctm_known,
            opacity: c.opacity,
            fill,
            fill_opacity: c.fill_opacity,
            stroke,
            stroke_opacity: c.stroke_opacity,
            stroke_width: c.stroke_width,
            visible: c.visibility == crate::style::computed::VISIBLE,
        });
        Ok(())
    })
}

/// パスのデータと `points` の読みと平坦化を通す（ファズの `path` の対象）。頭の 1 バイトで、読む口（下の 1 ビット：0 は
/// パスのデータ、1 は `points`）と変形（次の 2 ビット）を選び、残りを値にする。確かめること：
/// - 置き場に書く量は入力の 1 バイトあたり `cost::PATH_GEOMETRY_BYTES` 以下（`estimate_build` の上限）
/// - 1 回の払いが中止の間隔を超えない・同じ入力で同じ結果
/// - 平坦化で渡した点の数は区間ごとの分ける数の和（見積もりと同じ式）で、点は区間の厳密な外接矩形の中
pub fn path_ops(data: &[u8]) -> Check {
    use crate::geom::flatten::{flatten_seg, pieces};
    use crate::geom::{PathRef, PathSeg, PathWriter, Point, Rect, Transform, add_bounds};
    let Some((&head, text)) = data.split_first() else { return Check::Ok };
    let t = match (head >> 1) & 3 {
        0 => Transform::IDENTITY,
        1 => Transform::IDENTITY.scaled(7.5),
        2 => Transform { a: 0.6, b: 0.8, c: -0.8, d: 0.6, e: 3.0, f: -2.0 },
        _ => Transform { a: 1e-3, b: 0.0, c: 2.0, d: 1e3, e: 0.0, f: 0.0 },
    };
    let run = || -> Result<(u64, u64, u64, bool, bool), Error> {
        let b = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
        let mut verbs: MVec<'_, u8> = MVec::new(&b);
        let mut coords: MVec<'_, f64> = MVec::new(&b);
        let ok = if head & 1 == 0 {
            let mut w = PathWriter::new(&mut verbs, &mut coords);
            matches!(crate::values::path::path_data(text, &mut w, &b)?, crate::values::path::Parsed::Complete)
        } else {
            let mut pts: MVec<'_, Point> = MVec::new(&b);
            let ok = crate::values::path::points(text, &mut pts, &b)?;
            let mut w = PathWriter::new(&mut verbs, &mut coords);
            for (i, &p) in pts.as_slice().iter().enumerate() {
                if i == 0 { w.move_to(p)? } else { w.line_to(p)? }
            }
            ok
        };
        let bytes = (verbs.len() as u64).saturating_add((coords.len() as u64).saturating_mul(8));
        let limit = cost::PATH_GEOMETRY_BYTES.saturating_mul(text.len() as u64).saturating_add(64);
        if head & 1 == 0 && bytes > limit {
            return Err(Error::Limit(crate::error::LimitError {
                resource: Resource::Memory,
                needed: bytes,
                allowed: limit,
                stage: Stage::Build,
            }));
        }
        // 平坦化（点の数が大きすぎるものは数えるだけ）
        let path = PathRef { verbs: verbs.as_slice(), coords: coords.as_slice() };
        let mut total = 0u64;
        for seg in path.segments() {
            if matches!(seg, PathSeg::Move(_) | PathSeg::Close(..)) {
                continue;
            }
            total = total.saturating_add(pieces(&seg, &t, 0.1, 0.0));
        }
        let mut emitted = 0u64;
        let mut inside = true;
        // 弧の始めの点と終わりの点は、書いた点に半径に対して近い（端点の形から中心の形への変換。単位の 1 回目のレビュー）
        for seg in path.segments() {
            if let PathSeg::ArcTo(from, a) = seg {
                let r = a.max_radius();
                let (p0, p1) = (a.at(a.theta1), a.at(a.theta1 + a.dtheta));
                let near = |p: Point, q: Point| {
                    let d = math::hypot(p.x - q.x, p.y - q.y);
                    !(d.is_finite() && r.is_finite()) || d <= 1e-6 * (r + from.x.abs() + from.y.abs() + 1.0)
                };
                inside &= near(p0, from) && near(p1, a.end);
            }
        }
        if total <= 200_000 {
            for seg in path.segments() {
                let mut r = Rect::EMPTY;
                add_bounds(&mut r, &seg);
                let pad = |v: f64| 1e-9 * (1.0 + v.abs());
                flatten_seg(&seg, &t, 0.1, 0.0, &b, &mut |p| {
                    emitted = emitted.saturating_add(1);
                    if p.finite() && !r.is_empty() {
                        let ok = p.x >= r.x0 - pad(r.x0)
                            && p.x <= r.x1 + pad(r.x1)
                            && p.y >= r.y0 - pad(r.y0)
                            && p.y <= r.y1 + pad(r.y1);
                        inside &= ok;
                    }
                    Ok(())
                })?;
            }
            if emitted != total {
                return Err(Error::Cancelled);
            }
        }
        if b.max_spend() >= CANCEL_INTERVAL || b.over_released() {
            return Err(Error::Cancelled);
        }
        Ok((bytes, total, emitted, ok, inside))
    };
    match (run(), run()) {
        (Ok(a), Ok(b)) if a == b => {
            if a.4 {
                Check::Ok
            } else {
                Check::Violation("平坦化した点が区間の外接矩形の外")
            }
        }
        (Err(Error::Limit(_)), _) => Check::Violation("置き場に書く量が入力の 12 倍を超えた"),
        (Err(Error::Cancelled), _) => Check::Violation("平坦化の点の数が分ける数の和と違うか、勘定が崩れた"),
        _ => Check::Violation("同じ入力で結果が違う"),
    }
}

/// 線の外形と破線を通す（ファズの `stroke` の対象）。頭の 4 バイトで端・継ぎ目・太さ・miterlimit・破線を選び、残りを
/// パスのデータとして読む。確かめること：1 回の払いが中止の間隔を超えない・同じ入力で同じ結果・点ごとに払う（手間 ≥ 点の数）・
/// 外形の点が有限（入力が有限なら）。手間の上限は 2²⁴（速く回すため。超えれば `Limit` で、両方の回で同じなら良い）。
pub fn stroke_ops(data: &[u8]) -> Check {
    use crate::geom::flatten::flatten_seg;
    use crate::geom::stroke::{Cap, Dashes, Join, LoopSink, Pen, dash_polyline, stroke_polyline};
    use crate::geom::{PathRef, PathSeg, PathWriter, Point, Transform};
    let Some((head, text)) = data.split_first_chunk::<4>() else { return Check::Ok };
    let cap = match head[0] % 3 {
        0 => Cap::Butt,
        1 => Cap::Round,
        _ => Cap::Square,
    };
    let join = match (head[0] / 3) % 3 {
        0 => Join::Miter,
        1 => Join::Round,
        _ => Join::Bevel,
    };
    let half = [0.5, 3.0, 40.0, 1e6, 1e-6, 1e300].get(usize::from(head[1] % 6)).copied().unwrap_or(1.0);
    let limit = [4.0, 0.5, 1.0, 50.0].get(usize::from(head[1] / 6 % 4)).copied().unwrap_or(4.0);
    let dash_sets: [&[f64]; 5] = [&[], &[5.0, 3.0], &[0.0, 7.0], &[1.0, 1e30], &[0.01, 0.02, 3.0, 0.5]];
    let dash = dash_sets.get(usize::from(head[2] % 5)).copied().unwrap_or(&[]);
    let offset = f64::from(head[3]) - 100.0;
    struct Count {
        loops: u64,
        points: u64,
        finite: bool,
    }
    impl LoopSink for Count {
        fn begin(&mut self) -> Result<(), Error> {
            self.loops = self.loops.saturating_add(1);
            Ok(())
        }
        fn point(&mut self, p: Point) -> Result<(), Error> {
            self.points = self.points.saturating_add(1);
            self.finite &= p.finite();
            Ok(())
        }
    }
    let run = || -> Result<(u64, u64, bool, u64), Error> {
        let b = Budget::new(Stage::Plan, 1 << 24, 1 << 28, &Never);
        let mut verbs: MVec<'_, u8> = MVec::new(&b);
        let mut coords: MVec<'_, f64> = MVec::new(&b);
        {
            let mut w = PathWriter::new(&mut verbs, &mut coords);
            crate::values::path::path_data(text, &mut w, &b)?;
        }
        let pen = Pen { half, cap, join, miter_limit: limit, scale: 1.0, tol: 0.25 };
        let dashes = Dashes::new(dash, offset);
        let mut poly: MVec<'_, Point> = MVec::new(&b);
        let mut scratch: MVec<'_, Point> = MVec::new(&b);
        let mut buf: MVec<'_, Point> = MVec::new(&b);
        let mut hd: MVec<'_, Point> = MVec::new(&b);
        let mut sink = Count { loops: 0, points: 0, finite: true };
        let mut input_points = 0u64;
        let mut all_finite = true;
        let path = PathRef { verbs: verbs.as_slice(), coords: coords.as_slice() };
        let mut flush = |poly: &mut MVec<'_, Point>, closed: bool, sink: &mut Count| -> Result<(), Error> {
            match &dashes {
                None => stroke_polyline(poly.as_slice(), closed, &pen, &mut scratch, &b, sink)?,
                Some(d) => dash_polyline(poly.as_slice(), closed, d, &mut buf, &mut hd, &b, &mut |piece| {
                    stroke_polyline(piece, false, &pen, &mut scratch, &b, sink)
                })?,
            }
            poly.clear();
            Ok(())
        };
        for seg in path.segments() {
            match seg {
                PathSeg::Move(p) => {
                    if !poly.is_empty() {
                        flush(&mut poly, false, &mut sink)?;
                    }
                    poly.push(p)?;
                }
                PathSeg::Close(..) => flush(&mut poly, true, &mut sink)?,
                _ => flatten_seg(&seg, &Transform::IDENTITY, 0.25, half, &b, &mut |p| {
                    input_points = input_points.saturating_add(1);
                    all_finite &= p.finite();
                    poly.push(p)
                })?,
            }
        }
        if !poly.is_empty() {
            flush(&mut poly, false, &mut sink)?;
        }
        if b.max_spend() >= CANCEL_INTERVAL || b.over_released() {
            return Err(Error::Cancelled);
        }
        if b.work() < input_points.saturating_mul(cost::OP_STEP) {
            return Err(Error::Cancelled);
        }
        let finite_ok = !all_finite || half >= 1e300 || sink.finite;
        Ok((sink.loops, sink.points, finite_ok, b.work()))
    };
    match (run(), run()) {
        (Ok(a), Ok(b)) if a == b => {
            if a.2 {
                Check::Ok
            } else {
                Check::Violation("有限な入力の外形に有限でない点がある")
            }
        }
        (Err(Error::Limit(_)), Err(Error::Limit(_))) => Check::Ok,
        (Err(Error::Cancelled), _) | (_, Err(Error::Cancelled)) => {
            Check::Violation("線の勘定が崩れたか、点ごとに払っていない")
        }
        _ => Check::Violation("同じ入力で線の結果が違う"),
    }
}
