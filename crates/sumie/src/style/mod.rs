//! 段 2：スタイル（architecture.md §4.2）。プロパティの表・カスケード・継承は M1 の style の単位で足す。ここには読む段の
//! 設定（[`ParseOptions`]・[`StyleDefaults`]）を置く（値に効く「読みの設定」。D1）。`@media`・`@supports` の条件も、
//! この設定とプロパティの表を使うので、この段で照らす（css は条件を読むだけ）。

#![allow(dead_code, reason = "M1 の style の単位で、カスケードからプロパティの表と値の確かめを使う")]

pub(crate) mod cascade;
pub(crate) mod cond;
pub(crate) mod props;
pub(crate) mod valid;

use crate::error::{Error, Invalid, InvalidReason};
use crate::limits::Limits;

/// 読む段の設定（借りて受ける。`Document` は要る分を写して数える）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParseOptions<'a> {
    pub limits: Limits,
    /// 値に効く（D1 の「読みの設定」）
    pub style: StyleDefaults<'a>,
}

impl ParseOptions<'static> {
    /// 既定（上限は `Limits::DEFAULT`、スタイルは usvg と同じ）。
    pub const DEFAULT: ParseOptions<'static> = ParseOptions { limits: Limits::DEFAULT, style: StyleDefaults::USVG };
}

impl Default for ParseOptions<'static> {
    fn default() -> ParseOptions<'static> {
        ParseOptions::DEFAULT
    }
}

/// スタイルの既定（書体・大きさ・言語は usvg の `Options` の同じ名前のものに当たる）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleDefaults<'a> {
    /// 書体の既定
    pub font_family: &'a str,
    /// 文字の大きさの既定（有限で正。em・ex の基準にもなる）
    pub font_size: f64,
    /// `switch` の `systemLanguage` と照らす言語（ロケールは読まない）
    pub languages: &'a [&'a str],
    /// 総称の書体の割り当て（FR-FO-04）
    pub generic: GenericFamilies<'a>,
    /// 代わりの書体の並び（FR-FO-05）
    pub fallback: &'a [&'a str],
    /// `@media (prefers-color-scheme)` で照らす明暗（ADR-66 (2)）
    pub color_scheme: ColorScheme,
}

/// 明暗の設定（`prefers-color-scheme`）。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

impl StyleDefaults<'static> {
    /// usvg 0.48 の既定と同じ（書体 Times New Roman・大きさ 12・言語 en・総称の書体は fontdb の既定）。明暗は light。
    pub const USVG: StyleDefaults<'static> = StyleDefaults {
        font_family: "Times New Roman",
        font_size: 12.0,
        languages: &["en"],
        generic: GenericFamilies::USVG,
        fallback: &[],
        color_scheme: ColorScheme::Light,
    };
}

impl StyleDefaults<'_> {
    /// 呼び手の誤りを確かめる（文字の大きさが有限で正でなければ `Invalid`（`Argument`））。
    pub(crate) fn check(&self) -> Result<(), Error> {
        if !(self.font_size.is_finite() && self.font_size > 0.0) {
            return Err(Error::Invalid(Invalid { reason: InvalidReason::Argument, position: None }));
        }
        Ok(())
    }
}

/// 総称の書体（`serif`・`sans-serif`・`cursive`・`fantasy`・`monospace`）の割り当て。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericFamilies<'a> {
    pub serif: &'a str,
    pub sans_serif: &'a str,
    pub cursive: &'a str,
    pub fantasy: &'a str,
    pub monospace: &'a str,
}

impl GenericFamilies<'static> {
    /// FR-FO-04 の既定（macOS のほかの fontdb の既定と同じ。fontdb は macOS では fantasy を Papyrus にする）。
    pub const USVG: GenericFamilies<'static> = GenericFamilies {
        serif: "Times New Roman",
        sans_serif: "Arial",
        cursive: "Comic Sans MS",
        fantasy: "Impact",
        monospace: "Courier New",
    };
}
