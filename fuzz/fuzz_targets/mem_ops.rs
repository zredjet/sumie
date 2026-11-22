//! mem の入れ物（MVec・SortedVec・SortedMap・stable_sort）を入力のバイト列で動かす（sumie::fuzzing::mem_ops）。
//! 不変の条件が崩れたら落とす。上限で止まるのは正しい振る舞い。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, mem_ops};

fuzz_target!(|data: &[u8]| {
    let out = mem_ops(data);
    if let Check::Violation(what) = out.check {
        panic!("mem_ops：{what}");
    }
});
