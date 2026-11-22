//! 入力を CSS として決まった文書の `<style>` に入れ、読む段の全体に通す（sumie::fuzzing::css_parse）。確かめることは
//! document と同じ（2 つの誤りが同じ・見積もりが実際と同じ・勘定・上限のきわ）。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, css_parse};

fuzz_target!(|data: &[u8]| {
    if let Check::Violation(what) = css_parse(data).check {
        panic!("css：{what}");
    }
});
