//! 誤りの型（requirements.md FR-REP-01・architecture.md §3）。
//!
//! 4 種：`Limit`（上限）・`Cancelled`（中止）・`Unsupported`（この描画器では描かない。Kura は次の段へ回す）・
//! `Invalid`（壊れた入力。位置つき。呼び手の誤りは `InvalidReason::Argument`）。列挙はすべて `#[non_exhaustive]`。
//! 文言は持たない（呼び手が訳す。FR-REP-04）。

/// 誤り。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// 上限を超える（何が・いくつ要り・上限はいくつか・どの段で）
    Limit(LimitError),
    /// 中止の印が立った
    Cancelled,
    /// この描画器では描かない
    Unsupported(Unsupported),
    /// 壊れた入力・呼び手の誤り
    Invalid(Invalid),
}

/// 上限を超えた誤りの中身。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitError {
    pub resource: Resource,
    /// 要る量（手間なら手間の単位、確保ならバイト）
    pub needed: u64,
    /// 上限
    pub allowed: u64,
    pub stage: Stage,
}

/// 上限の種類（requirements.md §2.10 の表の欄。段の手間と確保は `Work`・`Memory` と段で分ける）。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    /// 手間
    Work,
    /// 確保（数えた量。`try_reserve_exact` の失敗も含む）
    Memory,
}

/// 段（どこで断ったか）。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    EstimateParse,
    Parse,
    EstimateBuild,
    Build,
    Estimate,
    Plan,
    Render,
    FontSet,
}

/// この描画器では描かないもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unsupported {
    pub feature: Feature,
    /// 入力のバイトの位置（わかるとき）
    pub position: Option<u64>,
}

/// 描かない機能（requirements.md FR-REP-02）。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// svgz（gzip）
    Svgz,
    /// UTF-8 でない符号化（UTF-16 の BOM・UTF-8 以外を名乗る宣言）
    Encoding,
    /// DTD のパラメタ実体
    ParameterEntity,
    /// DTD の外部の実体
    ExternalEntity,
    /// マークアップか参照を含む実体
    MarkupEntity,
    /// `<image>`（デコーダが渡されていない）
    Image,
    /// `data:image/svg+xml` の入れ子の SVG
    NestedSvg,
    /// 文字があるのにフォントの集まりが空
    TextWithoutFonts,
}

/// 壊れた入力・呼び手の誤り。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid {
    pub reason: InvalidReason,
    /// 入力のバイトの位置（呼び手の誤りは None）
    pub position: Option<u64>,
}

/// 壊れている理由。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidReason {
    /// 呼び手の誤り（倍率・バッファ・範囲。位置なし。ADR-41 (4)）
    Argument,
    /// UTF-8 として読めない
    Utf8,
    /// XML の整形式でない
    Xml,
}

impl Error {
    /// 上限の誤り。
    pub(crate) const fn limit(resource: Resource, needed: u64, allowed: u64, stage: Stage) -> Error {
        Error::Limit(LimitError { resource, needed, allowed, stage })
    }
}
