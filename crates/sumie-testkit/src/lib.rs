//! sumie の試験の共通の道具（architecture.md §1。ADR-39 (5)）。ライブラリの試験・道具・ファズから使う。
//!
//! 本番に入らないので、ライブラリの厳しい規則（clippy.toml・`tests/rules.rs`）はかからない。ただし `unsafe` は書かない。

pub mod rustscan;
