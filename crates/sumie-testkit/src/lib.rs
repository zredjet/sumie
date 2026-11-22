//! sumie の試験の共通の道具（architecture.md §1。ADR-39 (5)）。ライブラリの試験・道具・ファズから使う。
//!
//! 本番に入らないので、ライブラリの厳しい規則（clippy.toml・`tests/rules.rs`）はかからない。ただし `unsafe` は書かない。
//!
//! - [`chacha`]：自前の ChaCha（種つきの擬似乱数）
//! - [`mutate`]：種つきの変異（壊れた入力の試験）
//! - [`alloc`]：確保を数える確保器の包み（feature `count-alloc`）
//! - [`stack`]：小さいスタック（128 KiB）のスレッドで回す道具
//! - [`jsonl`]：平らな JSON の行（ゴールデン・一覧）
//! - [`rustscan`]：Rust のソースの字句（`tests/rules.rs`・`gen-disallowed`）

#[cfg(feature = "count-alloc")]
pub mod alloc;
pub mod chacha;
pub mod jsonl;
pub mod mutate;
pub mod rustscan;
pub mod stack;
