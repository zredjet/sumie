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
mod css;
mod doc;
mod error;
mod geom;
mod limits;
mod math;
mod mem;
mod report;
mod style;
mod values;
mod xml;

#[doc(hidden)]
pub mod fuzzing;

pub use crate::budget::Estimate;
pub use crate::cancel::{Cancel, Never};
pub use crate::doc::{Document, Needs, PixelSize, Size};
pub use crate::error::{Error, Feature, Invalid, InvalidReason, LimitError, Resource, Stage, Unsupported};
pub use crate::limits::Limits;
pub use crate::report::{Report, UnreadRef, Warning, WarningKind};
pub use crate::style::{ColorScheme, GenericFamilies, ParseOptions, StyleDefaults};

/// 描画器の識別子。値を変える変更で数を上げる（determinism.md §8）。開発中は 0、M7 で 1 に凍結する。
pub const RENDERER: &str = "sumie-render/0";

/// 読む段の見積もり（architecture.md §3・ADR-70）。読む段の道（XML を数えて節を作り、`<style>` を読んでカスケードし、
/// 報告にする）を `parse` と同じ上限（`Limits::parse_work`・`parse_bytes`）で走らせ、払った手間・確保の山・持ち続ける量を
/// 返す（見積もりは `parse` の実際とぴったり同じ）。誤り（整形式・符号化・上限）は `parse` と同じ（D2）。これが通れば、
/// 同じ入力・同じ設定の `parse` は `Limit` にならない（確保器の失敗と中止を除く）。`Limits::estimate_work`・
/// `estimate_bytes` は読む段には効かない。
pub fn estimate_parse(svg: &[u8], opts: &ParseOptions<'_>, cancel: &dyn Cancel) -> Result<Estimate, Error> {
    doc::estimate_parse(svg, opts, cancel)
}

/// 読む（architecture.md §3）。`estimate_parse` と同じ道を 1 度だけ走らせ、結果を持つ（ADR-70）。`Document` は入力を
/// 借りる。
pub fn parse<'s>(svg: &'s [u8], opts: &ParseOptions<'_>, cancel: &dyn Cancel) -> Result<Document<'s>, Error> {
    doc::parse(svg, opts, cancel)
}
