//! 確保を数える確保器の包み（ADR-31 (4)・architecture.md §5.1）。feature `count-alloc` のときだけ。
//!
//! 確保器は allocation-counter（クレートの中で `#[global_allocator]` を置く）。数えるのは今のスレッドの確保だけなので、
//! libtest のほかのスレッド（ほかの試験・出力の捕まえ）の確保は入らない。ライブラリはスレッドを作らないので、段の
//! 確保はすべて今のスレッドに入る。`realloc` は確保器の既定（新しく取って写して古いのを返す）なので、伸ばす間は新旧の
//! 両方が山に入る（`mem` の数え方と同じ向き）。
//!
//! 落とし穴（allocation-counter 0.8.1）：
//! - 測る前から持っていた確保を測る中で捨てると、`current` が負になり、そのぶん山を小さく出す（山は今の量が正のとき
//!   だけ更新する）。測る中では、前から持っていた入れ物を捨てない
//! - 2⁶³ に近い大きさを頼むと、確保器の中の i64 があふれて（debug では）panic し、プロセスが止まる。そのため、
//!   `count-alloc` の入った試験のバイナリでは「確保器が失敗を返す」道を試せない（試すなら count-alloc なしのバイナリで）

/// 測ったヒープ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Heap {
    /// 測り始めてからの、同時に生きた確保の最大（バイト）
    pub peak: u64,
    /// 終わったときに残った確保（バイト。返していなければ正）
    pub current: i64,
    /// 確保の合計（バイト）
    pub total: u64,
    /// 確保の回数
    pub count: u64,
}

/// `f` を回して、その間の今のスレッドの確保を測る。
pub fn measure<R>(f: impl FnOnce() -> R) -> (R, Heap) {
    let mut out = None;
    let info = allocation_counter::measure(|| {
        out = Some(f());
    });
    let heap =
        Heap { peak: info.bytes_max, current: info.bytes_current, total: info.bytes_total, count: info.count_total };
    match out {
        Some(r) => (r, heap),
        None => unreachable!("measure の中で f が呼ばれなかった"),
    }
}

/// 測ったヒープの山が、数えた量（`Budget` の山）＋ `slack` 以下かを確かめる（NFR-04）。
#[track_caller]
pub fn assert_heap_within(heap: &Heap, counted_peak: u64, slack: u64, what: &str) {
    assert!(
        heap.peak <= counted_peak.saturating_add(slack),
        "{what}：ヒープの山 {} バイトが、数えた量 {counted_peak} ＋ {slack} を超える（数え漏れ）",
        heap.peak
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_this_thread() {
        let (v, h) = measure(|| {
            let v: Vec<u64> = Vec::with_capacity(1000);
            v.capacity()
        });
        assert!(v >= 1000);
        assert_eq!(h.peak, 8000);
        assert_eq!(h.current, 0);
        assert_eq!(h.count, 1);
        // ほかのスレッドの確保は入らない
        let (_, h) = measure(|| {
            std::thread::scope(|s| {
                s.spawn(|| {
                    let _ = std::hint::black_box(vec![0u8; 1 << 20]);
                });
            });
        });
        assert!(h.peak < 1 << 20, "ほかのスレッドの確保が入った：{h:?}");
    }
}
