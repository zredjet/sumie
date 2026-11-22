//! XML の段（数える段と作る段）を入力のバイト列で通す（sumie::fuzzing::xml_parse）。作る段が数える段の見積もりを
//! 超えたり、節の鎖が崩れたりしたら落とす。誤り（Invalid・Unsupported・Limit）で止まるのは正しい振る舞い。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, xml_parse};

fuzz_target!(|data: &[u8]| {
    if let Check::Violation(what) = xml_parse(data).check {
        panic!("xml：{what}");
    }
});
