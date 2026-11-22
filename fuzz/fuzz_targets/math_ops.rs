//! math の max・min・clamp・powi の性質と、超越関数が panic しないこと（sumie::fuzzing::math_ops）。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, math_ops};

fuzz_target!(|data: &[u8]| {
    if let Check::Violation(what) = math_ops(data) {
        panic!("math_ops：{what}");
    }
});
