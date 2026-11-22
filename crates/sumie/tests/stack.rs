//! 小さいスタック（128 KiB）の試験（NFR-03）。debug と release の両方で回す（CI は `cargo test --release` も回す）。
//! スタックがあふれたらプロセスごと落ちるので、落ちずに終われば通る。段ができたら、10 万段の入れ子・参照の鎖・
//! セレクタの結合子を、読む・木を作る・描くの最後まで通す例を足す（M1 から）。

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects
)]

use sumie::Limits;
use sumie::fuzzing::{Check, mem_ops, xml_parse_with};
use sumie_testkit::stack::on_small_stack;

#[test]
fn mem_ops_on_a_small_stack() {
    // 大きな上限で、伸ばす・並べ替える（8.5 万個。並べ替えは再帰しない）
    let mut ops = vec![23u8, 19];
    for i in 0..5000u32 {
        ops.extend_from_slice(&[1, 16, 0, (i % 251) as u8]);
    }
    ops.extend_from_slice(&[7, 0, 7, 3, 11, 0, 11, 7, 7, 1]);
    let out = on_small_stack(move || mem_ops(&ops));
    assert_eq!(out.check, Check::Ok);
    assert!(!out.limited, "上限で止まった：{out:?}");
    assert!(out.work > 100_000);
}

/// 10 万段の入れ子の XML を、上限を広げて 128 KiB のスタックで読む（歩きは再帰しない）。
#[test]
fn xml_deep_nesting_on_a_small_stack() {
    let n = 100_000;
    let mut src = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\">");
    for _ in 0..n {
        src.push_str("<g a=\"1\">");
    }
    for _ in 0..n {
        src.push_str("</g>");
    }
    src.push_str("</svg>");
    let limits = Limits { depth: 200_000, nodes: 400_000, ..Limits::DEFAULT };
    let out = on_small_stack(move || xml_parse_with(src.as_bytes(), &limits));
    assert_eq!(out.check, Check::Ok, "{out:?}");
    assert_eq!(out.nodes, Some(n + 1), "{out:?}");
}
