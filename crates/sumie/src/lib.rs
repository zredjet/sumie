//! sumie — 決定的で安全な SVG の描画器。
//!
//! 静的な SVG を読み、RGBA8（乗算済み）の画素に描く。段の分け方・型・公開の口は `docs/architecture.md`、値の規則は
//! `docs/determinism.md`。
//!
//! M0 は基礎のモジュール（`math`・`budget`・`cost`・`mem`・中止・誤り）だけを持つ。
//!
//! std のうち使ってよいモジュールは `tests/rules.rs` の表で決める（時刻・環境変数・スレッド・ファイル・入出力などを
//! 使わない。determinism.md §5・§6）。禁じる口は `clippy.toml`（`sumie-tools gen-disallowed`）。

#![forbid(unsafe_code)]

/// 描画器の識別子。値を変える変更で数を上げる（determinism.md §8）。開発中は 0、M7 で 1 に凍結する。
pub const RENDERER: &str = "sumie-render/0";
