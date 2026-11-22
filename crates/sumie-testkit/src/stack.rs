//! 小さいスタックのスレッドで試験を回す道具（NFR-03：debug と release の両方の 128 KiB のスタック）。

/// NFR-03 のスタックの大きさ。
pub const SMALL_STACK: usize = 128 * 1024;

/// `bytes` のスタックのスレッドで `f` を回して、結果を返す。`f` が panic したら、そのまま呼び手に伝える。
/// スタックがあふれたらプロセスごと落ちる（受け止められない）ので、試験はそれで失敗と分かる。
pub fn on_stack<R: Send + 'static>(bytes: usize, f: impl FnOnce() -> R + Send + 'static) -> R {
    let h = std::thread::Builder::new()
        .name(format!("stack-{bytes}"))
        .stack_size(bytes)
        .spawn(f)
        .unwrap_or_else(|e| panic!("小さいスタックのスレッドを作れない：{e}"));
    match h.join() {
        Ok(r) => r,
        Err(e) => std::panic::resume_unwind(e),
    }
}

/// [`SMALL_STACK`] で回す。
pub fn on_small_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    on_stack(SMALL_STACK, f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_and_returns() {
        assert_eq!(on_small_stack(|| 41 + 1), 42);
    }

    #[test]
    #[should_panic(expected = "中で")]
    fn propagates_panics() {
        on_small_stack(|| panic!("中で"));
    }
}
