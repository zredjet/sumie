//! 中止の印（architecture.md §3・§5.3。FR-LIM-05）。
//!
//! 呼び手が立てる印を、`Budget::spend` が決まった手間（`CANCEL_INTERVAL`）ごとに読む。時刻は読まない。

use std::sync::atomic::{AtomicBool, Ordering};

/// 中止の印。`cancelled` が真を返したら、段は `Error::Cancelled` を返す。
pub trait Cancel: Sync {
    /// 中止の印が立っているか。
    fn cancelled(&self) -> bool;
}

impl Cancel for AtomicBool {
    fn cancelled(&self) -> bool {
        // 印を読むだけで、ほかの値の受け渡しに使わないので Relaxed でよい
        self.load(Ordering::Relaxed)
    }
}

/// 立たない印。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Never;

impl Cancel for Never {
    fn cancelled(&self) -> bool {
        false
    }
}

/// いつも立っている印（ファズの入り口が、中止の道を通すのに使う。公開の API ではない）。
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Always;

impl Cancel for Always {
    fn cancelled(&self) -> bool {
        true
    }
}
