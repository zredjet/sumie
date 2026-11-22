//! sumie — 決定的で安全な SVG の描画器。
//!
//! 静的な SVG を読み、RGBA8（乗算済み）の画素に描く。段の分け方・型・公開の口は `docs/architecture.md`、値の規則は
//! `docs/determinism.md`。
//!
//! M0 で基礎のモジュール（`math`・`budget`・`cost`・`mem`・中止・誤り）を、M1 で読む段（`xml`・`css`・`values`・`style`・`doc`）を作る。
//!
//! std のうち使ってよいモジュールは `tests/rules.rs` の表で決める（時刻・環境変数・スレッド・ファイル・入出力などを
//! 使わない。determinism.md §5・§6）。禁じる口は `clippy.toml`（`sumie-tools gen-disallowed`）。

#![forbid(unsafe_code)]

mod budget;
mod cancel;
mod cost;
mod error;
mod limits;
mod math;
mod mem;
mod xml;

#[doc(hidden)]
pub mod fuzzing;

pub use crate::budget::Estimate;
pub use crate::cancel::{Cancel, Never};
pub use crate::error::{Error, Feature, Invalid, InvalidReason, LimitError, Resource, Stage, Unsupported};
pub use crate::limits::Limits;

/// 描画器の識別子。値を変える変更で数を上げる（determinism.md §8）。開発中は 0、M7 で 1 に凍結する。
pub const RENDERER: &str = "sumie-render/0";
