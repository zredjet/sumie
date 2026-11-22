//! 上限（requirements.md §2.10・architecture.md §3）。呼び手が渡し、段に入る前に「見積もり ≤ 上限」を確かめる。
//! 既定の値は requirements.md §2.10 の表（M7 で校正する。Kura は自分の値を渡す）。

/// 読む・木を作る段の上限。`Document` が写して持ち、`build` でも使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// 入力のバイト数
    pub input_bytes: u64,
    /// 実体を展開したあとのバイト数（入力のバイト数に、展開で増える分を足したもの）
    pub expanded_bytes: u64,
    /// XML の節（要素と文字）の数
    pub nodes: u64,
    /// 入れ子の深さ
    pub depth: u32,
    /// 展開したあとの要素の数（use・marker・複製。木を作る前に見る）
    pub instances: u64,
    /// 見積もり自身の手間と確保
    pub estimate_work: u64,
    pub estimate_bytes: u64,
    /// 読む段の手間と確保の山
    pub parse_work: u64,
    pub parse_bytes: u64,
    /// 木を作る段の手間と確保の山
    pub build_work: u64,
    pub build_bytes: u64,
}

impl Limits {
    /// 既定の上限（requirements.md §2.10）。
    pub const DEFAULT: Limits = Limits {
        input_bytes: 16 << 20,
        expanded_bytes: 64 << 20,
        nodes: 1 << 21,
        depth: 1024,
        instances: 1 << 21,
        estimate_work: 1 << 28,
        estimate_bytes: 256 << 20,
        parse_work: 1 << 31,
        parse_bytes: 1 << 30,
        build_work: 1 << 31,
        build_bytes: 4 << 30,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
