//! パスのデータ・`points` の読みと平坦化を通す（sumie::fuzzing::path_ops）。置き場に書く量が上限を超えたり、平坦化の点の
//! 数が分ける数の和と違ったり、点が外接矩形の外に出たり、同じ入力で結果が違ったりしたら落とす。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, path_ops};

fuzz_target!(|data: &[u8]| {
    if let Check::Violation(what) = path_ops(data) {
        panic!("path：{what}");
    }
});
