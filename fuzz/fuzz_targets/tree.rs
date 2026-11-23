//! 木を作る段を通す（sumie::fuzzing::build_ops）。見積もりが実際を下回ったり、払いの勘定が崩れたり、同じ入力で結果が
//! 違ったりしたら落とす。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, build_ops};

fuzz_target!(|data: &[u8]| {
    if let Check::Violation(what) = build_ops(data) {
        panic!("tree：{what}");
    }
});
