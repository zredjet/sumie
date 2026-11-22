//! DTD の内部のサブセットを入力のバイト列にして、実体を参照する文書を XML の段に通す（sumie::fuzzing::dtd_parse）。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, dtd_parse};

fuzz_target!(|data: &[u8]| {
    if let Check::Violation(what) = dtd_parse(data).check {
        panic!("dtd：{what}");
    }
});
