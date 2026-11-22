//! 値の読みの口（style の値の確かめ・values の読み・`@media`・`@supports`）を通す（sumie::fuzzing::values_parse）。勘定が
//! 崩れたり、同じ入力で結果が違ったり、手間が長さに比例しなかったりしたら落とす。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, values_parse};

fuzz_target!(|data: &[u8]| {
    if let Check::Violation(what) = values_parse(data) {
        panic!("values：{what}");
    }
});
