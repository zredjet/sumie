//! mem の試験：入れ物の振る舞い・数えた量とヒープの実際の山（確保を数える確保器。ADR-31 (4)）・見積もり ≥ 実際・
//! 小さいスタック・中止。`calibrate` は手間の係数の仮の校正（`cargo test -p sumie --release -- --ignored calibrate
//! --nocapture`）。
#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::sync::atomic::{AtomicBool, Ordering};

use sumie_testkit::alloc::{assert_heap_within, measure};
use sumie_testkit::chacha::ChaCha;
use sumie_testkit::stack::on_small_stack;

use super::*;
use crate::budget::CANCEL_INTERVAL;
use crate::cancel::Never;
use crate::error::{LimitError, Resource, Stage};

fn budget(limit: u64) -> Budget<'static> {
    Budget::new(Stage::Build, u64::MAX, limit, &Never)
}

#[test]
fn mvec_counts_and_returns() {
    let b = budget(1 << 20);
    {
        let mut v: MVec<'_, u64> = MVec::new(&b);
        assert_eq!(b.bytes(), 0);
        for i in 0..5 {
            v.push(i).unwrap();
        }
        // 4 → 8 に伸ばした。山は新旧の両方（32 + 64）
        assert_eq!(v.capacity(), 8);
        assert_eq!(b.bytes(), 64);
        assert_eq!(b.peak(), 96);
        assert_eq!(v.as_slice(), &[0, 1, 2, 3, 4]);
        v.append_slice(&[5, 6, 7, 8]).unwrap();
        assert_eq!(v.capacity(), 16);
        v.resize_to(3, 0).unwrap();
        assert_eq!(v.as_slice(), &[0, 1, 2]);
        v.resize_to(6, 9).unwrap();
        assert_eq!(v.as_slice(), &[0, 1, 2, 9, 9, 9]);
        v.insert(1, 100).unwrap();
        v.insert(0, 200).unwrap();
        v.insert(99, 300).unwrap();
        assert_eq!(v.as_slice(), &[200, 0, 100, 1, 2, 9, 9, 9, 300]);
        assert_eq!(v.pop(), Some(300));
        v.truncate(2);
        assert_eq!(v.as_slice(), &[200, 0]);
    }
    assert_eq!(b.bytes(), 0, "捨てたら数えた量を返す");
}

#[test]
fn with_capacity_respects_the_limit() {
    let b = budget(100);
    assert!(MVec::<u64>::with_capacity(&b, 12).is_ok());
    assert_eq!(b.bytes(), 0);
    assert_eq!(
        MVec::<u64>::with_capacity(&b, 13).err(),
        Some(Error::Limit(LimitError { resource: Resource::Memory, needed: 104, allowed: 100, stage: Stage::Build }))
    );
    // バイト数があふれる
    assert!(matches!(MVec::<u64>::with_capacity(&b, usize::MAX), Err(Error::Limit(_))));
    // 伸ばすときも上限を見る
    let mut v: MVec<'_, u8> = MVec::new(&b);
    let r = (0..200).try_for_each(|i| v.push(i as u8));
    assert!(matches!(r, Err(Error::Limit(LimitError { resource: Resource::Memory, .. }))));
    assert!(b.peak() <= 100);
}

#[test]
fn freeze_detaches_from_the_budget() {
    let b = budget(1 << 20);
    let mut v: MVec<'_, u32> = MVec::with_capacity(&b, 10).unwrap();
    v.append_slice(&[1, 2, 3]).unwrap();
    let f = v.freeze();
    assert_eq!(f.as_slice(), &[1, 2, 3]);
    assert_eq!(f.bytes(), 40);
    // 段の勘定には残る（段が終わるまで生きている）
    assert_eq!(b.bytes(), 40);
    drop(f);
    assert_eq!(b.bytes(), 40, "Frozen は Budget を知らない");
    assert!(Frozen::<u8>::empty().is_empty());
}

#[test]
fn lower_bound_finds_the_first_equal() {
    let s = [1, 2, 2, 2, 5, 7];
    assert_eq!(lower_bound(&s, |x| *x < 0), 0);
    assert_eq!(lower_bound(&s, |x| *x < 2), 1);
    assert_eq!(lower_bound(&s, |x| *x < 3), 4);
    assert_eq!(lower_bound(&s, |x| *x < 8), 6);
    assert_eq!(lower_bound::<i32>(&[], |_| true), 0);
}

#[test]
fn sorted_vec_and_map() {
    let b = budget(1 << 20);
    let mut sv: SortedVec<'_, u16> = SortedVec::new(&b);
    for x in [5, 1, 3, 3, 9, 1] {
        sv.insert(x).unwrap();
    }
    assert_eq!(sv.as_slice(), &[1, 1, 3, 3, 5, 9]);
    assert_eq!(sv.find(&3).unwrap(), Some(2));
    assert_eq!(sv.find(&4).unwrap(), None);
    assert!(!sv.insert_unique(5).unwrap());
    assert!(sv.insert_unique(4).unwrap());
    assert_eq!(sv.len(), 7);

    let mut m: SortedMap<'_, u32, u32> = SortedMap::new(&b);
    assert!(m.insert_first(10, 1).unwrap());
    assert!(m.insert_first(5, 2).unwrap());
    assert!(!m.insert_first(10, 3).unwrap(), "同じ鍵は先のものを残す");
    assert_eq!(m.get(&10).unwrap(), Some(&1));
    assert!(m.insert_or_replace(10, 4).unwrap());
    assert_eq!(m.get(&10).unwrap(), Some(&4));
    assert_eq!(m.get(&7).unwrap(), None);
    assert_eq!(m.keys(), &[5, 10]);
    assert_eq!(m.values(), &[2, 4]);
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pair {
    key: u32,
    idx: u32,
}
impl Plain for Pair {}

fn random_pairs(seed: u64, n: usize, keys: u64) -> Vec<Pair> {
    let mut r = ChaCha::from_u64(seed);
    (0..n).map(|i| Pair { key: r.below(keys) as u32, idx: i as u32 }).collect()
}

#[test]
fn stable_sort_matches_std_stable_sort() {
    for (seed, n, keys) in
        [(1, 0, 1), (2, 1, 1), (3, 2, 2), (4, 17, 3), (5, 1000, 10), (6, 4097, 1 << 30), (7, 65_537, 50)]
    {
        let mut ours = random_pairs(seed, n, keys);
        let mut want = ours.clone();
        want.sort_by_key(|p| p.key);
        let b = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
        stable_sort(&b, &mut ours, |a, c| a.key < c.key).unwrap();
        assert_eq!(ours, want, "n {n}");
        // 見積もり ≥ 実際（併合の分は見積もりとぴったり同じ）
        let est = cost::stable_sort(n as u64, size_of::<Pair>() as u64);
        assert!(b.work() <= est, "n {n}：実際 {} > 見積もり {est}", b.work());
        let merge = n as u64 * cost::ceil_log2(n as u64) * cost::MERGE_STEP;
        assert!(b.work() >= merge);
        assert_eq!(b.bytes(), 0);
        assert!(b.peak() <= (n * size_of::<Pair>()) as u64);
    }
}

/// ヒープの実際の山が数えた山を超えない（測る中で数えない確保をしない形）。
#[test]
fn heap_peak_is_within_the_counted_peak() {
    let input = random_pairs(3, 30_000, 100);
    let ((counted, n), heap) = measure(|| {
        let b = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
        let mut v: MVec<'_, u64> = MVec::new(&b);
        for i in 0..10_000u64 {
            v.push(i).unwrap();
        }
        let mut m: SortedMap<'_, u32, u64> = SortedMap::new(&b);
        let mut r = ChaCha::from_u64(9);
        for _ in 0..2000 {
            m.insert_first(r.below(5000) as u32, r.next_u64()).unwrap();
        }
        let mut pairs: MVec<'_, Pair> = MVec::with_capacity(&b, input.len()).unwrap();
        pairs.append_slice(&input).unwrap();
        stable_sort(&b, pairs.as_mut_slice(), |a, c| a.key < c.key).unwrap();
        let f = v.freeze();
        let n = f.len();
        drop(f);
        (b.peak(), n)
    });
    assert_eq!(n, 10_000);
    assert_heap_within(&heap, counted, 0, "mem の入れ物");
    assert_eq!(heap.peak, counted, "数える量は頼んだ容量 × size_of で、std の確保と同じ");
    assert_eq!(heap.current, 0);
}

#[test]
fn works_on_a_small_stack() {
    let ok = on_small_stack(|| {
        let b = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
        let mut v: MVec<'_, u32> = MVec::new(&b);
        let mut r = ChaCha::from_u64(4);
        for _ in 0..200_000 {
            v.push(r.next_u32()).unwrap();
        }
        stable_sort(&b, v.as_mut_slice(), |a, c| a < c).unwrap();
        v.as_slice().windows(2).all(|w| w[0] <= w[1])
    });
    assert!(ok);
}

/// 並べ替えの途中で印が立ったら、2 × CANCEL_INTERVAL のうちに Cancelled を返す。
#[test]
fn stable_sort_can_be_cancelled() {
    let flag = AtomicBool::new(false);
    let b = Budget::new(Stage::Build, u64::MAX, u64::MAX, &flag);
    let mut v: Vec<u32> = (0..1_000_000u32).rev().collect();
    // 先に印を立てる（最初の塊の前払いのあとに見つかる）
    flag.store(true, Ordering::Relaxed);
    let r = stable_sort(&b, &mut v, |a, c| a < c);
    assert_eq!(r, Err(Error::Cancelled));
    assert!(b.work() <= 2 * CANCEL_INTERVAL, "印から {} 単位", b.work());
    assert_eq!(b.bytes(), 0, "途中で止めても数えた量を返す");
}

/// 手間の係数の仮の校正（M0。1 単位 ≈ 1 ns を目安にする）。
#[test]
#[ignore = "手で回す：cargo test -p sumie --release -- --ignored calibrate --nocapture"]
fn calibrate() {
    use std::time::Instant;
    let b = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
    let n = 4_000_000usize;
    // 写し
    let src: Vec<u64> = (0..n as u64).collect();
    let t = Instant::now();
    let mut v: MVec<'_, u64> = MVec::with_capacity(&b, n).unwrap();
    let w0 = b.work();
    v.append_slice(&src).unwrap();
    let ns = t.elapsed().as_nanos() as f64;
    println!("copy：{:.3} ns/単位（{} 単位）", ns / (b.work() - w0) as f64, b.work() - w0);
    // 埋め
    let mut z: MVec<'_, u8> = MVec::with_capacity(&b, n * 8).unwrap();
    let w0 = b.work();
    let t = Instant::now();
    z.resize_to(n * 8, 0).unwrap();
    let ns = t.elapsed().as_nanos() as f64;
    println!("fill：{:.3} ns/単位（{} 単位）", ns / (b.work() - w0) as f64, b.work() - w0);
    // 並べ替え
    let mut pairs = random_pairs(11, n, 1 << 20);
    let w0 = b.work();
    let t = Instant::now();
    stable_sort(&b, &mut pairs, |a, c| a.key < c.key).unwrap();
    let ns = t.elapsed().as_nanos() as f64;
    println!("stable_sort：{:.3} ns/単位（{} 単位）", ns / (b.work() - w0) as f64, b.work() - w0);
    // 差し込み（ずらし）
    let mut s: MVec<'_, u32> = MVec::with_capacity(&b, 20_000).unwrap();
    let w0 = b.work();
    let t = Instant::now();
    for i in 0..20_000u32 {
        s.insert(0, i).unwrap();
    }
    let ns = t.elapsed().as_nanos() as f64;
    println!("shift：{:.3} ns/単位（{} 単位）", ns / (b.work() - w0) as f64, b.work() - w0);
    std::hint::black_box((v.len(), z.len(), s.len()));
}
