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

use sumie::fuzzing::{Check, math_ops, mem_ops};
use sumie_testkit::alloc::{assert_heap_within, measure};
use sumie_testkit::mutate::cases;

/// mem_ops の種：上限の 2 バイトと、(操作, 引数) の並び。
const MEM_SEEDS: &[&[u8]] = &[
    &[20, 19, 0, 1, 0, 2, 0, 3, 1, 9, 7, 0],
    &[23, 19, 2, 200, 7, 3, 11, 0, 11, 5, 9, 3],
    &[4, 6, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1],
    &[23, 19, 5, 1, 5, 1, 5, 2, 6, 3, 6, 3, 10, 3, 9, 1, 8, 0, 4, 2, 3, 0],
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
        limited += usize::from(out.limited);
    }
    // 上限で止まる件も、止まらない件もある（試験が両方の道を通る）
    assert!(limited > 100 && limited < 3900, "上限で止まった件 {limited}");
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
