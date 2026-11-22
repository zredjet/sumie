//! 種つきの変異の試験（CLAUDE.md・architecture.md §9）。種と回数を固定し、`sumie-testkit` の ChaCha8 で作った入力を
//! ファズの入り口（`sumie::fuzzing`）に通して、不変の条件・panic しないこと・ヒープの山 ≤ 数えた山を確かめる。
//! 段ごとのパーサーができたら、ここに対象を足す（M1 から）。

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects
)]

use std::sync::atomic::{AtomicU32, Ordering};

use sumie::Cancel;
use sumie::fuzzing::{Check, math_ops, mem_ops, mem_ops_with};
use sumie_testkit::alloc::{assert_heap_within, measure};
use sumie_testkit::mutate::cases;

/// mem_ops の種：上限の 2 バイトと、(操作, 引数) の並び。
/// fuzz/seeds/mem_ops と同じもの（fuzz/README.md）。
const MEM_SEEDS: &[&[u8]] = &[
    &[20, 19, 0, 1, 0, 2, 0, 3, 1, 9, 7, 0],
    &[23, 19, 2, 200, 7, 3, 11, 0, 11, 5, 9, 3],
    &[4, 6, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1],
    &[23, 19, 5, 1, 5, 1, 5, 2, 6, 3, 6, 3, 10, 3, 9, 1, 8, 0, 4, 2, 3, 0],
    // 大きく伸ばしてから並べ替え・差し込み（数えた山が数十 KB になる）
    &[23, 19, 1, 16, 1, 16, 1, 16, 1, 16, 2, 255, 1, 16, 2, 250, 7, 2, 11, 200, 11, 3, 7, 5, 2, 10, 7, 0],
];
const MEM_DICT: &[&[u8]] = &[&[0, 255], &[1, 16], &[2, 255], &[7, 1], &[11, 0], &[5, 7], &[6, 31], &[4, 3]];

#[test]
fn mem_ops_survive_seeded_mutations() {
    let mut limited = 0usize;
    for (n, case) in cases(0x5eed_0001, 4000, MEM_SEEDS, MEM_DICT, 1024).iter().enumerate() {
        let (out, heap) = measure(|| mem_ops(case));
        assert_eq!(out.check, Check::Ok, "{n} 件目 {case:?}");
        // mem の入れ物は頼んだ容量 × size_of だけを確保する（数えない確保がない）
        assert_heap_within(&heap, out.counted_peak, 0, &format!("{n} 件目"));
        assert_eq!(heap.current, 0, "{n} 件目：返していない確保");
        // 山でない時点の数えない確保も見つける：ヒープの確保の合計・回数 ≤ 数えた合計・回数
        assert!(heap.total <= out.charged_total, "{n} 件目：確保の合計 {} > 数えた {}", heap.total, out.charged_total);
        assert!(heap.count <= out.charges, "{n} 件目：確保の回数 {} > 数えた {}", heap.count, out.charges);
        limited += usize::from(out.limited);
    }
    // 上限で止まる件も、止まらない件もある（試験が両方の道を通る）
    assert!(limited > 100 && limited < 3900, "上限で止まった件 {limited}");
}

/// fuzz/seeds/mem_ops の種が MEM_SEEDS と同じ（ずれない）。
#[test]
fn fuzz_seeds_match_mem_seeds() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/mem_ops");
    let mut files: Vec<Vec<u8>> =
        std::fs::read_dir(dir).unwrap().flatten().map(|e| std::fs::read(e.path()).unwrap()).collect();
    files.sort();
    let mut seeds: Vec<Vec<u8>> = MEM_SEEDS.iter().map(|s| s.to_vec()).collect();
    seeds.sort();
    assert_eq!(files, seeds, "fuzz/seeds/mem_ops と tests/mutate.rs の MEM_SEEDS を合わせる");
}

/// 決まった回数だけ読んだら立つ中止の印。
struct AfterCalls {
    left: AtomicU32,
}

impl Cancel for AfterCalls {
    fn cancelled(&self) -> bool {
        self.left.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_err()
    }
}

/// 途中で中止しても、不変の条件と「数えた量を返す」が崩れない。中止は印が立った最初の確かめで返る。
#[test]
fn mem_ops_survive_cancellation() {
    let big = MEM_SEEDS[4];
    let mut cancelled = 0usize;
    for (n, case) in cases(0x5eed_0003, 600, &[big], MEM_DICT, 2048).iter().enumerate() {
        for calls in [0u32, 1, 3] {
            let c = AfterCalls { left: AtomicU32::new(calls) };
            let (out, heap) = measure(|| mem_ops_with(case, &c));
            assert_eq!(out.check, Check::Ok, "{n} 件目（{calls} 回で中止）");
            assert_eq!(heap.current, 0);
            if out.cancelled {
                cancelled += 1;
                // 印は CANCEL_INTERVAL（2^16）ごとに読むので、calls + 1 回目の確かめまでに止まる
                assert!(out.work < (u64::from(calls) + 2) << 16, "{n} 件目：{} 単位まで回った", out.work);
            }
        }
    }
    assert!(cancelled > 0, "中止の道を通らなかった");
}

#[test]
fn math_ops_survive_seeded_mutations() {
    let seeds: &[&[u8]] = &[
        &[0u8; 48],
        &[0xff; 48],
        &[0, 0, 0, 0, 0, 0, 0xf0, 0x7f, 0, 0, 0, 0, 0, 0, 0xf8, 0x7f, 1, 0, 0, 0, 0, 0, 0, 0x80],
        &[0x18, 0x2d, 0x44, 0x54, 0xfb, 0x21, 0x09, 0x40, 0, 0, 0, 0, 0, 0, 0xe0, 0x3f, 0, 0, 0, 0, 0, 0, 0, 0],
    ];
    let dict: &[&[u8]] = &[&[0, 0, 0, 0, 0, 0, 0xf0, 0x7f], &[0, 0, 0, 0, 0, 0, 0, 0x80], &[1, 0, 0, 0, 0, 0, 0, 0]];
    for (n, case) in cases(0x5eed_0002, 4000, seeds, dict, 240).iter().enumerate() {
        assert_eq!(math_ops(case), Check::Ok, "{n} 件目 {case:?}");
    }
}
