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
    /// 確保（数えた量が上限を超える）
    Memory,
    /// 確保器が断った（上限の内でも環境で失敗しうる。D1・D2 の約束の外。`needed` は頼んだバイト数、`allowed` は 0。
    /// determinism.md §1.2）
    Allocator,
    /// 入力のバイト数（`Limits::input_bytes`）
    Input,
    /// 実体を展開したあとのバイト数（`Limits::expanded_bytes`）
    Expanded,
    /// XML の節の数（`Limits::nodes`）
    Nodes,
    /// 入れ子の深さ（`Limits::depth`）
    Depth,
    /// 展開したあとの要素の数（`Limits::instances`。use・marker・pattern などを参照ごとに数える）
    Instances,
    /// キャンバスの 1 辺（`RenderLimits::canvas_side`）
    CanvasSide,
    /// キャンバスの画素の数（`RenderLimits::canvas_pixels`）
    CanvasPixels,
    /// 描く節の数（`RenderLimits::render_nodes`）
    RenderNodes,
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
    /// まだ描かない機能（M2〜M6 の一時の印。M7 までになくす。ADR-89 (1)）
    NotYet(Later),
}

/// [`Feature::NotYet`] の機能（描くマイルストーン）。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Later {
    /// グラデーションの塗り（M3）
    Gradient,
    /// pattern の塗り（M3）
    Pattern,
    /// `opacity` < 1（層。M3）
    Opacity,
    /// `clip-path`（M3）
    Clip,
    /// `mask`（M3）
    Mask,
    /// `mix-blend-mode`・`isolation`（M3）
    Blend,
    /// marker（M3）
    Marker,
    /// 切り取りの要る入れ子の svg・symbol（M3）
    Viewport,
    /// filter（M4）
    Filter,
    /// 外接矩形の要る `transform-box`（fill-box など）の変形と、表示の領域の決まらない % の位置（M3）
    TransformBox,
    /// CSS の個別の変形（`translate`・`rotate`・`scale`）と `offset-path`（M3）
    CssTransform,
    /// `display: contents`（M3）
    DisplayContents,
    /// 根の背景の画像の層（`data:` の画像。デコーダを渡す口と同じ M3）
    BackgroundImage,
    /// CSS の箱のプロパティ（根の `border`・`padding`・`margin`・`box-shadow`・`position` など、要素の `outline`、rect の
    /// 論理の大きさ `inline-size`・`block-size`）が初期の値と同じ絵にならない（Chrome は描く。M3 で決める。ADR-103）
    CssBox,
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
    /// 外側の svg の幅か高さが 0 か負（ADR-64 (8)）
    Size,
}

impl Error {
    /// 上限の誤り。
    pub(crate) const fn limit(resource: Resource, needed: u64, allowed: u64, stage: Stage) -> Error {
        Error::Limit(LimitError { resource, needed, allowed, stage })
    }
}
