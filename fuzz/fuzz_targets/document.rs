//! 読む段の全体（公開の estimate_parse と parse）を入力のバイト列で通す（sumie::fuzzing::document_parse）。2 つの誤りが
//! 違ったり、本体が見積もりを超えたりしたら落とす。誤り（Invalid・Unsupported・Limit）で止まるのは正しい振る舞い。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, document_parse};

fuzz_target!(|data: &[u8]| {
    if let Check::Violation(what) = document_parse(data).check {
        panic!("document：{what}");
    }
});
