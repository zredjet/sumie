//! 段 2：CSS（architecture.md §4.2・requirements.md FR-ST-06〜09）。字句・規則・セレクタ・照合。

#![allow(dead_code, reason = "M1 の css の単位で、規則とセレクタの読みと照合をこの上に足す")]

pub(crate) mod decl;
pub(crate) mod lex;
pub(crate) mod matcher;
pub(crate) mod select;
pub(crate) mod sheet;
