//! cost の試験。
#![allow(clippy::disallowed_macros, clippy::arithmetic_side_effects)]

use super::*;

#[test]
fn ceil_log2_and_div() {
    assert_eq!(ceil_log2(0), 0);
    assert_eq!(ceil_log2(1), 0);
    assert_eq!(ceil_log2(2), 1);
    assert_eq!(ceil_log2(3), 2);
    assert_eq!(ceil_log2(4), 2);
    assert_eq!(ceil_log2(5), 3);
    assert_eq!(ceil_log2(u64::MAX), 64);
    assert_eq!(fill(0), 0);
    assert_eq!(fill(1), 1);
    assert_eq!(fill(16), 1);
    assert_eq!(fill(17), 2);
    assert_eq!(shift(0, 4), 1);
    assert_eq!(shift(4, 4), 2);
    assert_eq!(copy(u64::MAX), u64::MAX / 16 + 1);
    assert_eq!(stable_sort(u64::MAX, 8), u64::MAX);
}
