//! 構造から組み立てた SVG を、読む段・木を作る段・描く段まで通す（sumie::fuzzing の document_parse・build_ops・render_ops
//! と sumie_testkit::svg_gen。ADR-100 (3) (b)）。頭の 4 バイトは倍率と範囲、残りを生成器の選択の並びにして、必ず整った
//! SVG にする。見積もりが通ったのに断ったり、範囲の描画が全体と違ったり、段が見積もりを超えたりしたら落とす。
#![no_main]

use libfuzzer_sys::fuzz_target;
use sumie::fuzzing::{Check, build_ops, document_parse, render_ops};

fuzz_target!(|data: &[u8]| {
    let (head, rest) = data.split_at(data.len().min(4));
    let svg = sumie_testkit::svg_gen::svg(rest);
    let mut input = head.to_vec();
    input.resize(4, 0);
    input.extend_from_slice(&svg);
    // 読む段の全体（`document_parse`。重い）は頭の 2 バイト目で 8 件に 1 回（読む段は `document` の対象が受け持つ。ファズの
    // 作り直しの 2 回目のレビュー）
    let doc = if input[1] & 7 == 0 { document_parse(&svg).check } else { Check::Ok };
    for (stage, check) in [("render", render_ops(&input).check), ("tree", build_ops(&svg).check), ("document", doc)] {
        if let Check::Violation(what) = check {
            panic!("render_gen（{stage}）：{what}");
        }
    }
});
