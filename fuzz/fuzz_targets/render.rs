//! 描く段までを通す（sumie::fuzzing::render_ops）。見積もりが通ったのに断ったり、範囲の描画が全体と違ったり、同じ入力で
//! 画素が違ったりしたら落とす。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, render_ops};

fuzz_target!(|data: &[u8]| {
    if let Check::Violation(what) = render_ops(data).check {
        panic!("render：{what}");
    }
});
