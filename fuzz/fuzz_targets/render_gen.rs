//! 構造から組み立てた SVG を描く段まで通す（sumie::fuzzing::render_ops と sumie_testkit::svg_gen。ADR-100 (3) (b)）。頭の
//! 4 バイトは倍率と範囲、残りを生成器の選択の並びにして、必ず整った SVG にする。見積もりが通ったのに断ったり、範囲の描画が
//! 全体と違ったり、描く段が範囲の見積もりを超えたりしたら落とす。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, render_ops};

fuzz_target!(|data: &[u8]| {
    let (head, rest) = data.split_at(data.len().min(4));
    let mut input = head.to_vec();
    input.resize(4, 0);
    input.extend_from_slice(&sumie_testkit::svg_gen::svg(rest));
    if let Check::Violation(what) = render_ops(&input).check {
        panic!("render_gen：{what}");
    }
});
