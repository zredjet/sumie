//! 線の外形と破線を通す（sumie::fuzzing::stroke_ops）。払いの勘定が崩れたり、点ごとに払わなかったり、有限な入力の外形に
//! 有限でない点が出たり、同じ入力で結果が違ったりしたら落とす。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, stroke_ops};

fuzz_target!(|data: &[u8]| {
    if let Check::Violation(what) = stroke_ops(data) {
        panic!("stroke：{what}");
    }
});
