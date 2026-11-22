//! Budget の試験。
#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::arithmetic_side_effects)]

use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::cancel::Never;
use crate::error::LimitError;

#[test]
fn spend_counts_and_limits() {
    let b = Budget::new(Stage::Parse, 100, 0, &Never);
    assert_eq!(b.spend(60), Ok(()));
    assert_eq!(b.spend(40), Ok(()));
    assert_eq!(b.work(), 100);
    assert_eq!(
        b.spend(1),
        Err(Error::Limit(LimitError { resource: Resource::Work, needed: 101, allowed: 100, stage: Stage::Parse }))
    );
    // 断ったら払わない
    assert_eq!(b.work(), 100);
    // あふれ
    let b = Budget::new(Stage::Render, u64::MAX, 0, &Never);
    assert_eq!(b.spend(u64::MAX), Ok(()));
    assert!(matches!(b.spend(1), Err(Error::Limit(LimitError { resource: Resource::Work, .. }))));
}

#[test]
fn charge_release_and_peak() {
    let b = Budget::new(Stage::Build, 0, 1000, &Never);
    assert_eq!(b.charge(600), Ok(()));
    assert_eq!(b.charge(300), Ok(()));
    b.release(600);
    assert_eq!(b.charge(400), Ok(()));
    assert_eq!(b.bytes(), 700);
    assert_eq!(b.peak(), 900);
    assert_eq!(
        b.charge(301),
        Err(Error::Limit(LimitError { resource: Resource::Memory, needed: 1001, allowed: 1000, stage: Stage::Build }))
    );
    b.release(u64::MAX);
    assert_eq!(b.bytes(), 0);
}

/// 印が立ってから返すまでの手間は 2 × CANCEL_INTERVAL を超えない（FR-LIM-05）。塊は CANCEL_INTERVAL 未満。
#[test]
fn cancel_is_seen_within_two_intervals() {
    for step in [1u64, 7, 1000, CANCEL_INTERVAL - 1] {
        for start in [0u64, 1, CANCEL_INTERVAL / 2, CANCEL_INTERVAL - 1, CANCEL_INTERVAL * 3 + 5] {
            let flag = AtomicBool::new(false);
            let b = Budget::new(Stage::Render, u64::MAX, 0, &flag);
            let mut done = 0u64;
            while done < start {
                let k = step.min(start - done);
                b.spend(k).unwrap();
                done += k;
            }
            flag.store(true, Ordering::Relaxed);
            let set_at = b.work();
            let mut result = Ok(());
            while result.is_ok() {
                result = b.spend(step);
            }
            assert_eq!(result, Err(Error::Cancelled));
            let after = b.work() - set_at;
            assert!(after <= 2 * CANCEL_INTERVAL, "step {step} start {start}：印から {after} 単位");
        }
    }
}

#[test]
fn never_cancels() {
    let b = Budget::new(Stage::Render, u64::MAX, 0, &Never);
    for _ in 0..10 {
        assert_eq!(b.spend(CANCEL_INTERVAL - 1), Ok(()));
    }
}

#[test]
fn chunk_len_keeps_chunks_below_the_interval() {
    for per in [0u64, 1, 2, 3, 16, 1000, CANCEL_INTERVAL - 1, CANCEL_INTERVAL, u64::MAX] {
        let n = chunk_len(per) as u64;
        assert!(n >= 1);
        if per < CANCEL_INTERVAL {
            assert!(n.saturating_mul(per) < CANCEL_INTERVAL, "per {per}：塊 {n}");
        } else {
            assert_eq!(n, 1);
        }
    }
    assert_eq!(chunk_len(1) as u64, CANCEL_INTERVAL - 1);
}
