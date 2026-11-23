# 設計

段の分け方・型・公開の口。要件は `requirements.md`、値の規則は `determinism.md`。
ここに書いた方式は承認済み（ADR-26 ほか）。AAT は ADR-29・ADR-36・ADR-42、レビューで直した所は ADR-30〜44、M0 の詰めは ADR-45〜。変えるときは
`decisions.md` に追記してから変える。

この文書の言葉：**節**は XML と木の節（node）。文書の区切りは「§」で書く。**文字**は符号位置、**字形**はフォントの
グリフ。**パス**は path の幾何（塗りの部分パス＝subpath）。**組版の方式**は OpenType の方式と AAT の方式。

---

## 1. リポジトリの形

```text
Cargo.toml                    # workspace（resolver 3）。[workspace.lints]（`lints.workspace = true` で受け継ぐのは crates/sumie だけ）、profile
rust-toolchain.toml           # 1.99.0 に固定（Kura と同じ）
crates/sumie/                 # ライブラリ。本番の依存は libm だけ。publish = false
  clippy.toml                 # 厳しい規則（disallowed-methods・disallowed-types・disallowed-macros）。ライブラリだけにかかる
  src/
  tests/                      # 結合テスト（rules.rs・golden.rs・region.rs・stack.rs・limits.rs・alloc.rs …）
  goldens/                    # render-v<N>.jsonl（凍結した版は追記だけ）・estimate.jsonl（D2）
crates/sumie-testkit/         # 試験の共通の道具（自前の ChaCha8・種つきの変異・照合の集まりの読み込み・確保を数える確保器の包み）。dev だけ
crates/sumie-tools/           # 照合・計測・生成の道具（本番に入らない。resvg =0.48.1 などに依存してよい）。clippy.toml は別のゆるいもの
fuzz/                         # cargo-fuzz（nightly。独立の workspace。段ができるまではライブラリの doc(hidden) の fuzzing の入り口を使う。ADR-50）
tests/svg/                    # 自作の試験の SVG（コミットする）
tests/fonts/                  # 試験用のフォント（写しを許すライセンスのもの・それに表を足して作ったもの。出どころと作り方の一覧つき）
corpus/                       # 照合の集まり（取り込んだ試験の SVG と実例の SVG（real/）。ADR-21・ADR-59）
scripts/                      # check-goldens-append-only.sh・verify.sh など
docs/
```

- ライブラリは 1 つのクレートにまとめ、中をモジュールで段に分ける（§2）。Kura が取り込む単位を 1 つにし、段の向きは
  `tests/rules.rs` で見張る
- 厳しい `clippy.toml` は `crates/sumie/` に置く（clippy はクレートの場所から上へ設定を探すので、根に置くと道具にもかかる）。
  ライブラリの結合テストのファイルは、頭で試験に要る口（`clippy::disallowed_methods`・`disallowed_macros`・
  `arithmetic_side_effects` など）を `allow` する（`allow-*-in-tests` の設定は unwrap・expect・panic・添字にしかない）
- 照合の道具と試験の共通の道具を別のクレートにするのは、resvg などの dev の依存がライブラリの `cargo test` に入らない
  ようにするため。ライブラリの dev 依存は、BLAKE3、`sumie-testkit`、確保を数える確保器のクレート（§5.1。allocation-counter。ADR-46）
  くらいにとどめる。ライブラリにも試験にも `unsafe` を書かない

---

## 2. 段の分け方

```text
SVG のバイト列 ─────────── estimate_parse（読む段と同じ道を同じ上限で走らせて、手間・確保の山・持ち続ける量を測る。ADR-70）
  │
  ▼ 段 1  XML（xml）
  │   バイト列の字句 → 節の配列。DTD の内部の実体を上限つきで展開。名前空間を解く
  ▼ 段 2  スタイル（css・style・values）
  │   style 要素と属性から規則を集め、要素ごとの指定の値を決める。値の字句（長さ・色・変形・パス…）を型にする
  ▼ 段 3  文書（doc）                    → Document（size・needs・report。estimate_build・build は tree に置く）
  │   ID の表・参照の解決・輪の検出・展開の数の勘定・switch・大きさ
  ▼ 段 4  木（tree・text・font・geom の一部）  → Tree（size・report。estimate・plan は plan に置く）
  │   use・marker の展開、計算した値（継承）、塗り・clip・mask・filter の解決、
  │   文字の組版と字形の輪郭（文字は木の中でパスになる。描く段はフォントを知らない）
  ▼ 段 5  計画（plan）                   → Plan（estimate_region・render・render_region）
  │   倍率から、平坦化した点・線の外形・破線・節ごとの画素の範囲・層の大きさ・一時の画像の積み上げの最大を決める
  ▼ 段 6  描画（raster・paint・filter・compose・render）
  │   セルの被覆 → 塗り → フィルタ・層の合成（範囲ごと）
  ▼
RGBA8（乗算済み）
```

モジュールの層（番号の大きい層は小さい層を使ってよい。小さい層は大きい層を知らない。`tests/rules.rs` が `use crate::`
の向きを見る）：

| 層 | モジュール |
|---|---|
| 0 基礎 | `math`・`fixed`・`mem`・`budget`・`cost`・`error`・`limits`（上限）・`cancel`（中止の印）・`report`・`unicode`（生成した表） |
| 1 幾何の型 | `geom`（変形・パス・平坦化・線の外形・破線。字句を知らない） |
| 2 読み取り | `xml` → `css` → `values`（`geom` の型を作る）→ `style` → `doc` |
| 3 フォントと文字 | `font` → `text` |
| 4 木 | `tree`（`impl Document` の `estimate_build`・`build` もここ） |
| 5 描画 | `raster` → `paint`（塗り・blend の式）→ `filter`（feBlend は `paint` の式を使う）→ `compose`（層・clip・mask・フィルタを当てる）→ `plan`（`impl Tree` の `estimate`・`plan` もここ）→ `render` |
| 6 公開の口 | `lib.rs`・`fuzzing`（ファズの入り口。`#[doc(hidden)]`。ADR-50） |

層 1〜5 の中は矢印の向きにだけ使う。層 0 の中は互いに使ってよい（順を決めない。`mem` が `budget`・`cost`・`error` を
使い、`budget` が `cancel` を使う）。`tests/rules.rs` の `LAYERS` が同じ表を持つ。

---

## 3. 公開の口

```rust
/// 描画器の識別子。値を変える変更で数を上げる（determinism.md §8）。開発中は 0、M7 で 1 に凍結する
pub const RENDERER: &str = "sumie-render/0";

// ---- 上限（requirements.md §2.10 の表と 1 対 1）----
pub struct Limits {                    // 読む・木を作る段。Document が写して持ち、build でも使う
    pub input_bytes: u64, pub expanded_bytes: u64, pub nodes: u64, pub depth: u32,
    pub instances: u64, pub estimate_work: u64, pub estimate_bytes: u64,
    pub parse_work: u64, pub parse_bytes: u64,      // 読む段の手間と確保のピーク
    pub build_work: u64, pub build_bytes: u64,      // 木を作る段の手間と確保のピーク
}
pub struct RenderLimits {              // 計画と描く段。Plan が写して持ち、render・render_region でも使う
    pub canvas_side: u32, pub canvas_pixels: u64, pub render_nodes: u64,
    pub estimate_work: u64, pub estimate_bytes: u64,
    pub plan_work: u64, pub plan_bytes: u64,        // 計画の手間と、Plan が持ち続ける量
    pub render_work: u64, pub temp_bytes: u64,      // 描く手間と、同時に生きる一時の画像
}
pub struct FontLimits { pub faces: u32, pub work: u64, pub index_bytes: u64 }

// ---- 読む ----
pub fn estimate_parse(svg: &[u8], opts: &ParseOptions<'_>, cancel: &dyn Cancel) -> Result<Estimate, Error>;
pub fn parse<'s>(svg: &'s [u8], opts: &ParseOptions<'_>, cancel: &dyn Cancel) -> Result<Document<'s>, Error>;

pub struct ParseOptions<'a> {          // 借りて受ける。Document は要る分を mem で写して数える
    pub limits: Limits,
    pub style: StyleDefaults<'a>,      // 値に効く（D1 の「読みの設定」）
}
pub struct StyleDefaults<'a> {         // 既定は const の StyleDefaults::USVG（std の確保の口を使わない）
    pub font_family: &'a str,          // 既定 "Times New Roman"（usvg と同じ）
    pub font_size: f64,                // 既定 12
    pub languages: &'a [&'a str],      // switch の systemLanguage。既定 ["en"]（usvg と同じ）。ロケールは読まない
    pub generic: GenericFamilies<'a>,  // serif・sans-serif・cursive・fantasy・monospace（FR-FO-04）
    pub fallback: &'a [&'a str],       // 代わりの書体の並び（FR-FO-05）
    pub color_scheme: ColorScheme,     // @media の prefers-color-scheme（既定 Light。ADR-66 (2)）
}
pub struct GenericFamilies<'a> { pub serif: &'a str, pub sans_serif: &'a str, pub cursive: &'a str,
                                 pub fantasy: &'a str, pub monospace: &'a str }   // 既定 GenericFamilies::USVG（FR-FO-04）
pub enum ColorScheme { Light, Dark }

// ---- 報告（FR-REP-03・04）----
impl Report {
    pub fn warnings(&self) -> impl ExactSizeIterator<Item = Warning> + '_;      // 位置の順（同じ位置は足した順、位置なしは最後。D2）
    pub fn unread_refs(&self) -> impl ExactSizeIterator<Item = UnreadRef<'_>> + '_;   // 位置の順
}
pub struct Warning { pub kind: WarningKind, pub position: Option<u64> }        // #[non_exhaustive]
pub struct UnreadRef<'a> { pub href: &'a [u8], pub position: u64 }            // #[non_exhaustive]
pub enum WarningKind { UnknownElement, InvalidValue, AtRule, ReferenceCycle, InvalidSelector, … }

impl<'s> Document<'s> {
    pub fn size(&self) -> Option<Size>;                 // ユーザー単位の幅・高さ（None は中身で決まる＝木が要る。ADR-63 (3)）
    pub fn pixel_size(&self, scale: f64) -> Result<Option<PixelSize>, Error>;
    pub fn needs(&self) -> Needs;                       // 文字があるか・描く木に入りうる <image> の種類（§3.1）
    pub fn report(&self) -> &Report;                    // この段までの警告・読まなかった参照
    pub fn estimate_build(&self, res: &Resources<'_>, cancel: &dyn Cancel) -> Result<Estimate, Error>;   // M1・M2 は res なし（画像・文字は Unsupported。M3・M5 で足す）
    pub fn build(&self, res: &Resources<'_>, cancel: &dyn Cancel) -> Result<Tree, Error>;
}
pub struct Resources<'a> {
    pub fonts: &'a FontSet,
    pub images: Option<&'a dyn ImageDecoder>,          // なければ <image> は Unsupported（ADR-16・FR-IMG-01）
}

// ---- フォント（ADR-23・ADR-34・ADR-40）----
pub type FontBytes = Arc<dyn AsRef<[u8]> + Send + Sync>;           // 中身は共有で受け、写さない（Kura の mmap をそのまま）
pub struct FontSource { pub key: FontKey, pub data: FontBytes }     // TTC は中の書体をすべて登録する
pub struct FontKey(pub [u8; 32]);                                    // 呼び手の鍵（Kura はファイルの BLAKE3）
impl FontSet {                                                       // 'static。Send + Sync。作ったあと変わらない
    pub fn new(sources: &[FontSource], limits: &FontLimits, cancel: &dyn Cancel) -> Result<FontSet, Error>;
    pub fn faces(&self) -> &[FaceId];                  // 登録した書体の並び（鍵と書体の番号）。呼び手は集まり全体のダイジェストを作れる
    pub fn report(&self) -> &FontSetReport;            // 飛ばした書体（壊れた・空の入れ物）
}

// ---- 画像（ADR-16。M3 で作る）----
pub trait ImageDecoder: Sync {
    fn id(&self) -> &str;                               // 版に入る報告の識別子（例：Kura の "png-rs-dec/1"）
    fn size(&self, mime: &str, data: &[u8]) -> Result<PixelSize, ImageError>;   // estimate_build が使う
    fn decode(&self, mime: &str, data: &[u8], out: &mut PixmapMut<'_>) -> Result<(), ImageError>;  // RGBA8 乗算済み
}
pub enum ImageError { Unsupported, Invalid, Limit }    // Unsupported・Invalid・Limit にそのまま移す
// デコーダの中の手間・確保・中止は呼び手の責め（勘定の外。FR-LIM-05）。出力のバッファはこちらで確保して数える
// data: の URL の base64 は自前で解く。拡大縮小は自前で決定的に（preserveAspectRatio・image-rendering）

// ---- 描く（ADR-38）----
pub struct RenderOptions { pub scale: f64, pub limits: RenderLimits }
impl Tree {                                             // 'static。Send + Sync。作ったあと変わらない
    pub fn size(&self) -> Size;
    pub fn pixel_size(&self, scale: f64) -> Result<PixelSize, Error>;
    pub fn report(&self) -> &Report;
    pub fn held_bytes(&self) -> u64;                    // 木が持ち続ける量（Kura の held_bytes に当たる）
    pub fn estimate(&self, opts: &RenderOptions, cancel: &dyn Cancel) -> Result<RenderEstimate, Error>;
    pub fn plan(&self, opts: &RenderOptions, cancel: &dyn Cancel) -> Result<Plan, Error>;
    pub fn render(&self, opts: &RenderOptions, out: &mut PixmapMut<'_>, cancel: &dyn Cancel) -> Result<(), Error>;  // plan して全体を描く
}
pub struct RenderEstimate { pub plan: Estimate, pub render: Estimate }   // 計画（Plan が持ち続ける分）と、全体を 1 度描く分
impl Plan {                                             // 'static。Send + Sync。作ったあと変わらない。作ったときの RenderLimits を持つ
    pub fn held_bytes(&self) -> u64;
    pub fn estimate_region(&self, rect: PixelRect, cancel: &dyn Cancel) -> Result<Estimate, Error>;
    pub fn render(&self, out: &mut PixmapMut<'_>, cancel: &dyn Cancel) -> Result<(), Error>;
    pub fn render_region(&self, rect: PixelRect, out: &mut PixmapMut<'_>, cancel: &dyn Cancel) -> Result<(), Error>;  // ADR-13
}
pub struct PixmapMut<'a> { /* data: &'a mut [u8], width, height, stride（行のバイト数）。RGBA8 乗算済み */ }

// ---- 見積もり・中止・誤り ----
pub struct Estimate {
    pub work: u64,          // 手間（1 単位 ≈ 1 ns。FR-LIM-04）
    pub peak_bytes: u64,    // その段で同時に生きる確保の最大（出力のバッファを除く）
    pub result_bytes: u64,  // その段の結果が持ち続ける量（Document・Tree・Plan。描く段は 0）
}
pub trait Cancel: Sync { fn cancelled(&self) -> bool; }
impl Cancel for core::sync::atomic::AtomicBool { /* Relaxed で読む */ }
pub struct Never;  impl Cancel for Never { /* いつも偽 */ }

#[non_exhaustive]
pub enum Error {
    Limit(LimitError),           // { resource: Resource, needed: u64, allowed: u64, stage: Stage }
    Cancelled,
    Unsupported(Unsupported),    // { feature: Feature, position: Option<u64> }
    Invalid(Invalid),            // { reason: InvalidReason, position: Option<u64> }。呼び手の誤り（倍率・バッファ・範囲）は
                                 //   InvalidReason::Argument（位置なし）
}
```

- Kura の今の見積もりの口（`Plan::Tree`・`Plan::Render`）に当たるのは `estimate_build` と `Tree::estimate`。Kura はフックを
  渡す代わりに、段ごとに見積もりを読んで予算を確保してから次の段を呼ぶ（待つのは呼び手の側。ライブラリは待たない）
- 見積もりの関数は、値を変えない決定的な関数。中で要る小さな確保と手間は、上限の「見積もりの手間」の中で数え、中止の印も見る
- 読む段は 1 本の道（XML を数える → XML の節を作る → `<style>` を規則の並びに読む → 条件を照らしてカスケードする → 報告）で、
  道の全体を `parse_work`・`parse_bytes` を上限にした 1 つの `Budget` で払う。`estimate_parse` は同じ道を同じ上限で走らせ、
  払った手間・確保の山・持ち続ける量を返す（見積もりは実際とぴったり同じ）。`parse` は同じ道を 1 度走らせて結果を持つ。
  同じ道なので 2 つは同じ誤りを返し（D2）、`estimate_parse` が通れば `parse` も通る（確保器の失敗と中止を除く。ADR-70）。
  ほかの見積もりの口（`estimate_build`・`Tree::estimate`）は「その口に当たる段を呼んだときの全体」を返す
- 段に入るときは「見積もり ≤ 上限」を欄ごとに確かめ、`Budget` の上限には見積もりの値を入れる（§5.1。ADR-39）。Kura は
  `RenderEstimate.plan` を共有の分（`shared`）、`Plan::estimate_region` を範囲ごとの作業領域（`per_task`）として予算にできる
- `Document<'s>` は入力を借りる（名前・値を写さない）。`Tree`・`Plan`・`FontSet` は何も借りない（字形の輪郭は木に写す。
  フォントのバイト列は `Arc` で共有する）ので、Kura は木と計画とフォントの集まりを持っておいて、タイルを何度でも、
  複数のスレッドで同時に描ける
- 出力は乗算済み。Kura はストレートに戻す式を自分の側で凍結する（determinism.md §1.4）
- 列挙の型（`Resource`・`Feature`・`InvalidReason`・`Stage`・`WarningKind`・`ColorScheme`・`ImageError`）と、返すだけの
  構造体（`Warning`・`UnreadRef`）は `#[non_exhaustive]` にする。呼び手が作る設定の構造体（`ParseOptions`・`StyleDefaults`・
  `Limits`）は `..DEFAULT` で書けるように付けない

### 3.1 形と、誤りを決める段（ADR-37）

Kura の「形」（値を描かずに大きさなどを読む。今の Kura は usvg で木を作り、描く前の資源の見積もり `check_render` まで通す）
に当たるのは、`parse` → `estimate_build` → `build` → `Tree::estimate`。形を通ったものは、同じ上限・同じ資源・同じ倍率なら、
`plan`・`render` で `Limit`・`Unsupported` にならない（確保の失敗と中止を除く）。範囲が全体に含まれるなら、
`Plan::estimate_region` は `RenderEstimate.render` を超えない（Kura がタイルで描いても形の約束が保てる。試験で確かめる）。
この約束は普通の入力（照合の集まり・実例・Kura の遅い入力）で守る。上限を狙って作った入力（極端な倍率・極小の破線・巨大な
半径など）では、形を通っても `plan`・`render` が `Limit` で安全に断りうる（その段の上限は見積もりの値なので、確保も手間も
上限を超えない。決定的なので D2 は保つ。ADR-100 (5)）。
大きさだけなら `parse` → `pixel_size` で足りる。ただし viewBox も幅・高さもない（か %）文書は、usvg と同じく中身の外接矩形の
右下を大きさにする（FR-OUT-01・ADR-58 (1)）ので、木を作るまで大きさが決まらない。`Document::size`・`pixel_size` はそれを
「中身で決まる（木が要る）」として `None` を返し（ADR-63 (3)）、大きさは `Tree::size`・`pixel_size` から読む。Kura の形は
もともと木まで作るので、Kura の側の手間は変わらない。

`Document::needs` の画像は「描く木に入りうる `<image>`」：参照の辺（§4.3）でたどれるものを保守的に数え、参照されない `defs`・
`display: none` の部分木・通らない `switch` の枝の中のものは除く（今の Kura は usvg が実際に解いた `<image>` だけで断るので、
これらは描ける。M1 で usvg が実際に解いた `<image>` と照らす。ADR-63 (1)）。文字は要素の有無で決める（Kura と同じ）。

| 誤り | 決まる段 |
|---|---|
| 入力のバイト数・符号化・svgz・DTD のパラメタ実体と外部の実体・XML の整形式 | `estimate_parse`・`parse` |
| 実体の展開・節の数・深さ・読む手間 | `estimate_parse`（読む段の道を走らせて測る。ADR-70）・`parse` |
| `<image>` があるのにデコーダがない・`data:image/svg+xml` | `parse` で `Document::needs` に載り、`estimate_build` が `Unsupported` を返す |
| 文字があるのにフォントの集まりが空 | 同上 |
| 展開の数・木を作る手間（組版を含む）・木を作る確保 | `estimate_build` |
| キャンバスの大きさ・描く節の数・一時の画像・計画と描く手間 | `Tree::estimate` |

---

## 4. 段ごとの表し方

### 4.1 XML（段 1）

- 節は 1 本の `Frozen<Node>`（`mem` の入れ物。§5.1）に行きがけの順で並べ、番号（u32）で指す。`Node { kind, ns, flags,
  parent, first_child, next_sibling, local: 範囲, attrs: 範囲, text: 範囲, pos }`
- 文字の節と属性の値は、実体を展開して空白を決めたあとの形で 1 本のバイト列（文字の置き場）に詰め、範囲で指す。
  展開しない値は入力を借りる
- 名前は名前空間の種類（u8：なし・SVG・XLink・XML・そのほか）と局所名（入力の範囲）。「そのほか」は URI を持たない。
  既知の名前を整数にするのは style の段で行う
- ATTLIST の既定の属性値は、名前空間を解く前に書いていない属性として足す（ADR-67 (7)・ADR-69 (2)(3)）
- 隣り合う文字と CDATA は別の節のまま持つ（roxmltree はまとめる。文字の段は続く文字の節を続けて読む。ADR-69 (7)）
- 木をたどるのは `first_child`・`next_sibling`・`parent` の鎖で行う（行きがけの順のたどりは積み上げすら要らない）
- 走査はバイト列の上の位置で進め、どの枝も必ず 1 バイト以上進む（`tests/rules.rs` では見られないので、ファズと
  「すべての 3 バイトの並び」のテストで確かめる）
- 空白は XML の空白（#x20・#x9・#xD・#xA）だけ。std の `trim`・`is_whitespace` は使わない（determinism.md §4.3）
- 2 つの段で読む：数える段が整形式・名前空間・属性の重なり・実体を確かめて、節・属性・
  文字の置き場の数と作る段の手間を数え、作る段が数えた容量を最初に取って同じ歩きで節を作る（歩きの手間と確保は 2 つの段で
  同じ）。読むバイトは 1 バイト 1 単位で、読む前に塊ずつ払う（`ScanMeter`）
- 整形式の厳しさは roxmltree 0.21.1（resvg の中）に合わせる（ADR-65）。照らしは `sumie-tools wellformed`（§10）

### 4.2 スタイル（段 2）

- CSS は CSS Syntax Level 3 の字句の部分集合を自作する。規則は `(セレクタ, 宣言の範囲, 詳細度, 順)` の並び
- セレクタの範囲は Selectors 3 の全部と、Selectors 4 の `:is()`・`:where()`・`:not()`・`:lang()`・属性の印（FR-ST-07・
  ADR-66 (1)）。後ろの兄弟を見る構造の擬似クラス（`:last-child`・`:nth-last-child()`・`:only-child`・`*-of-type` の後ろの側）は、
  作り終えた節の配列から兄弟の数（要素の兄弟の数と、同じ型の兄弟の数）を照合の前に数えておくので、照合は行きがけに 1 度流す
  だけで決まる。`:is()` などの中の複合のセレクタは、同じ要素の旗として外の部分より先に決める。`:has()` は当てない（警告）
- セレクタの照合は、文書を行きがけの順に 1 度流しながら、セレクタの部分（複合のセレクタ）ごとの旗で行う（ADR-41）。
  要素 e で「部分 1〜k が当たり、部分 k が e に当たる」を `M[e][k]` とし、結合子ごとに次の旗から決める：
  - 子孫：祖先のどれかで `M[·][k-1]` が立っているか（親から受け継ぐ旗。深さごとの積み上げで持つ）
  - 子：親の `M[親][k-1]`
  - 直後の兄弟：直前の兄弟の `M[兄][k-1]`
  - 後の兄弟：前の兄弟のどれかで `M[·][k-1]` が立っているか（兄弟の並びで受け継ぐ旗）
  - 要素ごとの仕事は「セレクタの部分の数の合計」で、合計は「部分の数の合計 × 要素の数」。後戻りがなく、どう結合子を
    混ぜても爆発しない。複合ごと・単純な選択子ごとに払い、属性を探す走査は属性ごとに払う。要素の id・class・言語・
    `:link`・`:empty` は使うときだけ要素ごとに 1 度だけ求める。複合ごとに、当たる要素が必ず持つ鍵（id・class・型）を決めて
    索引にし、要素ごとに「鍵のない複合」と「要素の id・class・局所名を鍵にする複合」だけを番号の順に照らす。規則は、当たった
    右端の複合から引く（すべての規則を回らない。ADR-75 (1)）
- 見積もりは読む段の道をそのまま走らせて測る（ADR-70）。実体から来る CSS（`<style>&css;</style>`・`style="&s;"`）も、
  展開した `<style>` の中身と属性の値を読む
- 宣言を要素に当てる手間は「当たった規則の宣言の数の和」と、プロパティの欄を要素ごとに書き出す手間で、§5.2 の表に入れる
- `@media`・`@supports`・`<style media>` の条件は css が読むだけで、style の段が照らす（設定 `ParseOptions` とプロパティの表が
  そこにある。ADR-68 (4)）
- 要素ごとの指定の値は `(PropertyId, 値)` の並べた `MVec`。計算した値は全要素には持たず、木を作る段で行きがけの順に
  たどりながら「深さごとの計算した値」の積み上げで求める。1 つの要素の計算した値は `style::computed::compute`（親の計算
  した値と指定の値から。`currentColor` は計算した値でも `currentColor` のまま受け継ぎ、使うときに要素の `color` で解く。
  長さは要素の文字の大きさ・最も近い表示の領域・文書の大きさで解く）、文書の木のたどり方（根からの変形・表示の領域・
  描く木に入るか）は `doc::walk`（M1 は照合の道具と試験から使い、M2 で use の展開を足して木を作る段が使う）
- 継承の文脈：use の中身だけは use の文脈で継承する。pattern・marker・clipPath・mask・symbol（use から参照されたときを除く）・
  filter の中身は、参照した要素でなく、自分の祖先から継承する（SVG の規則）

### 4.3 文書（段 3）

- ID の表は `mem::SortedMap<名前, NodeId>`（並べた `MVec` と自前の `lower_bound`。同じ id は文書の順で最初の要素。FR-ST-13・
  ADR-66 (5)）
- 参照の辺（保守的に作る。輪を見落とさない側に倒す。辺の数は要素の数に比例する）：
  - 要素 → 子（参照先の中身を描くと子を描くので、pattern → 中身の rect のような辺もこれで張られる）
  - use・feImage の href、href の鎖（gradient・pattern・filter・textPath）
  - 受け継がない `url(#)`（clip-path・mask・filter）：その要素から参照先へ
  - 受け継ぐ `url(#)`（fill・stroke・marker-start・marker-mid・marker-end など。属性・style 属性・当たる CSS の規則から拾う）：
    行きがけの順に、受け継ぐプロパティごとの「最も近い祖先の指定」を深さごとの積み上げで持ち、塗る要素（図形・文字）と
    use・feImage（中身が use の文脈で継承しうるので、塗る要素とみなす）から、その計算した参照へ辺を張る。要素ごとの辺は
    受け継ぐプロパティの数（定数）以下
  - 例：`<g fill="url(#p)"><pattern id="p"><rect/></pattern></g>` は、rect（中身は自分の祖先から継承）→ p → rect の輪になる
  - 輪は強連結成分（Tarjan。明示の積み上げ）で見つけ、輪のある成分の中の、文書の順に後ろ向きの参照の辺（子の辺はいつも
    前向き）と、受け継がない参照の並びの節に入る辺を取り除く（1 度の歩き。取り除く辺はどれももとの図の輪の上にある。
    ADR-82 (2)）。参照先の種類の合わない辺は張らない（ADR-80 (4)）。取り除いた辺は use の文脈では輪でないことがあり、M2 の
    木を作る段は文脈で確かめて、取り除いた参照の先を 1 段だけ描き直す（その中でまた取り除いた参照に当たったら描かない）。
    展開の数は 2 度数え（1 度目は取り除いた辺を飛ばし、2 度目は取り除いた辺の先を 1 度目の数で足す）、`needs` は取り除いた
    辺もたどる（ADR-86 (1)）。行き先が自分か祖先で、その間に参照の辺で入る節がない本当の輪は、M2 も描かないので足さない
    （ADR-87 (1)）
- 展開の数：輪になる参照を取り除いたあと（FR-CO-05）の辺には輪がないので、帰りがけの順に「展開したあとの要素の数」を
  飽和の足し算で求め、木を作る前に上限と比べる（展開してから数えない）。objectBoundingBox の pattern・clip・mask の
  ように参照ごとに中身を作り直すものも、この数に入れる。子を描かない入れ物（`defs`・`symbol`・`pattern`・`marker`・
  `clipPath`・`mask`・グラデーション・`filter` など）の中身は、参照の辺で入ったときだけ数える（木を作る段も参照された
  ものだけを展開する。`defs` の中の use の鎖を n² に数えない）。use・feImage の中身は use の文脈で受け継ぐので、節ごとに
  「その節の塗りを使う要素の数・marker を付ける要素の数・受け継がない参照を `inherit` の鎖で受け取る要素の数」も求め、
  use から出る塗りの辺・marker の辺・並びの辺はその数の倍で数える。marker の中の `context-fill` は marker を付けた要素の
  塗りを使うので、塗りの参照先はその数（marker の中の塗る要素は fill と stroke の両方で使えるので 2 倍）の倍で数える
  （marker の中から使われる pattern・mask・clipPath・filter の中身には文脈の要素がない。Chrome と同じ。ADR-87 (4)）。
  塗りの倍率に子は入れない（子は自分の辺で数える）。use から付く marker の中の `context-fill` は中身の要素が自分で持つ
  塗りも使うので、marker を付ける要素の自分の塗りの参照先の数の和も数える（ADR-81 (1)・ADR-82 (1)・ADR-84 (1)・
  ADR-86 (2)(3)。素朴に展開した数と乱数の文書で照らす試験がある）
- ここまでで `estimate_build` が返せる：展開の数・文字の数（とフォントの索引から決まる組版の手間の上限。§7.1）・パスの量

### 4.4 木（段 4）

- 描く木の節は 1 本の `Frozen` の配列（§5.1）。`Group { transform, opacity, blend, isolate, clip, mask, filters,
  children: 範囲 }`・`Path { geometry: PathId, fill, stroke, paint_order, anti_alias, bbox }`・`Image`（呼び手のデコーダで
  読んだもの。ADR-16）
- 幾何（パスのデータ）は別の置き場に詰め、use で展開した節は同じ `PathId` を指す（写さない）
- clipPath・mask・pattern・marker・filter の中身は、それぞれ木の中の部分の範囲として持ち、参照は番号で指す
- 文字は字形の輪郭（書体と字形の番号ごとに 1 度だけ作り、置き場で共有）と、字形ごとの変形を持つ `Group` になる
- 報告（使ったフォント・見つからなかった文字・使った画像のデコーダ・警告）は木と一緒に持つ
- 入れ子の `Box` を持つ型を作らない（捨てるとき・`derive` の `Debug`・`Clone`・`PartialEq` で再帰する）。平らな配列と番号だけ

### 4.5 計画と描画（段 5・6。ADR-38）

- 計画（`Tree::plan`）：倍率から、パスごとの平坦化した点・線の外形・破線（§6.1）、節ごとの画素の外接矩形、層の要る節
  （opacity・blend・isolation・clip・mask・filter）の層の矩形と、層の積み上げの深さごとの最大のバイト数を決めて、`Plan` に
  持つ。`Plan` は変わらないので、タイルごとに作り直さない（平坦化はタイルの数だけくり返さない）
- 描画（`Plan::render_region`）：範囲と交わる節だけを選び（§8）、一時の画像の最大の分の領域を最初に 1 度だけ確保して、
  層はその中に積む（描く途中で確保しない）
- 木のたどりは明示の積み上げ。clip・mask・pattern・feImage の中身を描くのも同じ積み上げに積む（再帰しない）

---

## 5. 手間・確保・中止

### 5.1 勘定の型と確保の口（ADR-31・ADR-39）

```rust
struct Budget<'c> {                       // 1 つの段の中で 1 つ。中は Cell（ライブラリはスレッドを作らないので Sync は要らない）
    work: Cell<u64>, work_limit: u64,     // 上限は見積もりの値（見積もり ≤ Limits を確かめてから入る）
    bytes: Cell<u64>, peak: Cell<u64>, bytes_limit: u64,
    next_check: Cell<u64>, cancel: &'c dyn Cancel,
}
impl Budget<'_> {
    fn spend(&self, units: u64) -> Result<(), Error>;   // 先に払う。上限を超えたら Limit、間隔ごとに中止を見る
}
struct MVec<'b, T> { /* Vec<T> と &'b Budget。伸ばすときに数え、捨てるときに返す */ }
struct Frozen<T> { /* 数え終えた Vec<T>（取り直さない）と、数えた量。段の結果（Document・Tree・Plan・FontSet）が持つ */ }
```

- 確保は `mem` モジュールの入れ物（`MVec`・`MBytes`・`SortedVec`・`SortedMap`・`Frozen`）と、`mem::stable_sort`（作業の
  領域を `mem` から取る自前の安定な併合の並べ替え）だけで行う
- 伸ばし方：見積もりで数が決まるものは、最初に要る量を `try_reserve_exact` でまとめて取り、伸ばさない。決まらないものは
  倍々に伸ばし、新しい領域の大きさを先に `bytes` に足してから古い領域を返す（取り直しの間の両方が `peak` に入る）
- 数える量は、頼んだ容量 × `size_of::<T>()`（`capacity()` は std の実装に依るので使わない）。`MVec`・`Frozen` に入れる型は
  すべて `#[repr(C)]` か基本の型にし、大きさを `cost` の定数と試験で照らす（D2 がツールチェーンで変わらないように）
- 捨てるとき（`Drop`）に `bytes` から引く。`peak` は `bytes` の最大。数えた量が上限を超えたら `Limit`（resource は `Memory`）、
  確保器が断ったら `Limit`（resource は `Allocator`。環境に依るので約束の外。ADR-55）
- 段を終えるとき、結果に残す `MVec` は `Frozen` にして `Budget` から切り離し、数えた量を `result_bytes`・`held_bytes` に入れる
  （容量と長さが違っても取り直さない）
- `mem` の外では、std の中で数えずに確保する口を使わない（一覧は CLAUDE.md。`disallowed-methods`・`disallowed-types`・
  `disallowed-macros` と `tests/rules.rs`。`Clone::clone` はすべて禁じ、`Arc` の共有は `mem::share` の包みで行う）
- 禁じきれない口の漏れは、試験で見つける：試験のバイナリでは確保を数える確保器（`allocation-counter =0.8.1`。ADR-46）を
  `#[global_allocator]` にし、各段の「ヒープの実際の増え ≤ `Budget` の数えた量 ＋ 小さな定数」を照合の集まりで確かめる。
  libtest のスレッドと出力の捕まえの確保が雑音にならないよう、スレッドごとに数える（ADR-46）
- 長く回るループは `CANCEL_INTERVAL` 未満の塊に分け、塊ごとに回る前に `spend` する（後払いにしない。ループ全体を一度に前払い
  しない。中止の速さ。NFR-06）。確保したあとの 0 埋め（一時の画像を透明にするなど）も同じ塊で数える
- 手間の式は `cost` モジュールに 1 か所だけ書き、見積もりと実際の勘定の両方がその関数を呼ぶ（式が食い違わない）。
  見積もりは「これから回る数の上限」、勘定は「実際に回った数」を同じ式に入れる
- `spend` が見積もり（`work_limit`）を越えるのは見積もりの不具合。本番では `Limit` を返し、試験では落とす（見積もり ≥ 実際）。
  本番のコードに `debug_assert!` を書かない

### 5.2 手間の単位の目安

| 段 | 1 単位の目安 |
|---|---|
| XML | 入力の 1 バイト・実体の展開の 1 バイト |
| CSS | 宣言の 1 字・セレクタの照合の 1 つの旗（部分 × 要素）・宣言を要素に当てる 1 回と並べ替えの 1 歩 |
| 参照 | 辺 1 本・展開した要素 1 つ |
| フォントの集まり | `FontSet::new` で読む表の 1 バイト・索引の 1 項目 |
| 組版 | 文字ごと・GSUB・GPOS の lookup の 1 回の当てはめ・文脈の 1 字形の照合・`morx`・`kerx` の状態機械の 1 歩・代わりのフォントの索引の 1 回の引き |
| 計画 | 平坦化で作る点 1 つ・線の外形の点 1 つ・破線 1 本・節 1 つ |
| ラスタライズ | セル 1 つ・走査線の 1 画素 |
| 塗り・合成 | 1 画素 × 層 |
| フィルタ | 1 画素 × プリミティブ × 核の大きさ |

係数は M7 で、1 単位 ≈ 1 ns（M4 Max・release）に合うよう校正する。係数を変えても値は変わらない（版の外。determinism.md
§1.1 の D2）。

### 5.3 中止

- `CANCEL_INTERVAL` は定数（2¹⁶）。`spend` の中で、数えた手間がその倍数を越えるたびに `cancel.cancelled()` を読む
- 印が立ってから返すまでの仕事は、前払いの 1 回の分を合わせて 2 × `CANCEL_INTERVAL` 単位を超えない（FR-LIM-05）
- 時刻を読まない。中止の印は呼び手が立てる（Kura の `Monitor::canceled()` を包む）

---

## 6. 描画の方式

### 6.1 幾何（ADR-30・ADR-38）

範囲の描画が全体の切り出しとビット単位で同じになる（ADR-13）ように、**点と線分は、切る矩形（キャンバス・範囲）に依らずに
決め、セルの値は線分の元の両端だけから決める**。

- 座標と変形は `f64`。変形は 2 × 3 の行列。変形のあとの点が有限でない節は描かない（警告しない。ADR-97 (7)・ADR-99 (3)）
- 曲線の平坦化（計画の段で 1 度だけ）：曲線の制御点・描く座標系への変形・許容の誤差（0.1 画素の予定）だけから、**分ける数を
  先に式で決めて**、t を均等に分けた位置で点を求める（適応的な再帰の分割をしない。切る矩形を見ない）。点の数は曲線の大きさの
  およそ平方根で、見積もりと一致する（大きすぎれば `Limit`）。太い線で外形の外側の誤差が大きくなる分（およそ (r + w/2)/r 倍）を、
  分ける数に線の太さとして入れるかを M2 で決める
- 制御点がすべて決まった箱（±2²² 画素。キャンバスにも範囲にも依らない）の外の同じ側にある曲線は、平坦化しない（下の規則で
  縦の線にするか落とす）。巨大な図形でも点の数が増えない
- 固定小数（1/256 画素）に直す前に、決まった箱で線分を切る（箱は全体で 1 つなので、範囲の描画でも同じ点になる）。切る点は
  境に近い方の端から内挿して求める（遠い端からだと t が 1 の近くで丸まり、巨大な線分の箱の中の部分を失う）。見積もりが見る箱と
  行の帯にかかる長さを数えるときも同じ切り方を使う。項目の画素の矩形は辺の固定小数の端から求める（ADR-98）
- 線の外形は、平坦化した折れ線に継ぎ目・端を付けて作る。曲線の内側の点と接線の続く曲線のつなぎ目（滑らかな点）は、内側の
  縁の交わる点が区間に収まらなければ頂点を通さずに縁の点どうしをつなぐ（曲線の縁を曲線のまま写すのと同じ。曲がりの半径より
  太い線で中に穴が残る。ADR-99 (2)）。多角形の頂点は頂点を通す。破線は部分パスごとに `stroke-dashoffset` の位相から始め直し、
  部分パスの平坦化した線分の長さで区切る（閉じた部分パスで頭と終わりがどちらも線の中なら、最初の破線を最後の破線の後ろに
  つないで始めの角を継ぎ目にする。ADR-91 (4)・ADR-92 (1)）。破線の数は算術で数える
- 範囲での描画（`Plan::render_region`）：
  - 描く項目（節の塗りか線）ごとに、計画が作った描く座標系の点の外接矩形（1 画素の余白。線の外形の点を含むので太さ・miter・
    端の分も入る）が範囲と交わらなければ、丸ごと落とす。破線の部分は、計画の段でキャンバスを線の届く幅と 2 画素だけ広げた
    箱にかかるものだけを作る（範囲に依らない。ADR-30 (4)・ADR-94 (7)）。
    閉じた形（塗りの部分パス・線の外形の 1 周・破線の 1 つ）も同じに落とす（範囲の左にある閉じた形でも、各行の cover の
    和は整数で 0 になる）
  - 線分は切らない。各セルの `cover`・`area` は、その線分の元の両端（固定小数）だけから、整数の閉じた式で求める（どの行・
    列から始めても同じ値）。範囲の行ごとに、線分がその行にかかる x の範囲を閉じた式で求めて、範囲の中のセルだけを作る
  - 全部が範囲の左にある線分は、同じ両端の y を持つ縦の線（範囲の 1 列左）に置き換える。左の縁をまたぐ線分は、範囲の左の
    外のセルの分を cover として足す（どちらも各行の cover の和が整数で同じ）。右・上・下にあるものは落とす
- 手間：平坦化と線の外形は計画で 1 度。範囲の描画の手間は、範囲と交わる項目の数と、その項目の辺のうち範囲の行にかかりうる
  もの（y0 の順に並べた辺を 2 分の探索で選ぶ：範囲の上 − 辺の高さの最大 から範囲の下まで。範囲の右の外の辺は足さない）と、
  範囲の中のセルの数で決まる。範囲の左の外の辺は cover を足すので範囲の行ごとに回る（1 つの項目がキャンバスの全体に広がる
  と、横に並んだタイルの合計はタイルの列の数に比例する。行ごとの cover を前もって持つかは M7 で決める。ADR-98）

### 6.2 ラスタライズ（ADR-15）

- 面積の被覆の方式：辺がかかるセル（画素）ごとに、整数の `cover`（縦の通過）と `area`（面積の重み）を積み、
  走査線を左から掃いて被覆を決める。座標は 1/256 画素の固定小数
- 塗りの規則（非ゼロ・偶奇）は、積んだ巻き数の値に当てる。積む幅はあふれない整数にする（多く重なる辺でも）
- 手間はセルの数と行の幅で決まり、辺どうしの交わりの数に依らない（Kura が見た「長い辺が多く交わる形で見積もりの 5 倍」が
  起きない）
- `shape-rendering: crispEdges` は画素の中心で内外を決める（反エイリアスなし）。`optimizeSpeed` と文字の `text-rendering`
  は resvg と同じ扱いにする（M2・M5 で照らす）

### 6.3 塗りと合成

- 画素は RGBA8 の乗算済み。合成（source-over・blend・opacity・mask）は整数で、255 で割る丸めを 1 つの関数に決める
- グラデーションは位置から `t` を `f64` で求め、色に直して決めた丸めで 8 ビットにする。補間を乗算済みでするかは
  M3 で resvg・Chrome と照らして決める。stop の数・同じ offset の stop は、並べた表を 1 度作ってから引く（Kura の S2・S3 の
  ように stop の数に比例する仕事を画素ごとにしない）
- pattern はタイルを層に描き、変形して写す。タイルは範囲に依らない格子と大きさで作る（計画で 1 度作って `Plan` に持つか、範囲
  ごとに作るかは M3 で決める）。写すときの標本の取り方は M3 で決める
- 非分離の blend（hue・saturation・color・luminosity）は仕様の式を `f64` で計算する

### 6.4 フィルタ

- 一時の画像は RGBA8 の乗算済み（1 画素 4 バイト。確保の見積もりが単純）。プリミティブの中の計算は `f64` か整数
- linearRGB との変換は 8 ビットの表（道具で作って `math` の関数と照らした表をコミットする）
- feGaussianBlur は σ に応じて仕様の 3 回の箱のぼかし（整数の走る和）か、核の重みを整数にした畳み込み。どちらも範囲に
  依らない。箱の走る和は半径に依らない手間で求める
- フィルタの計算面は、範囲に依らない格子と大きさで決める。回転・斜めの変形の下では、キャンバスの格子で計算するか、局所の
  座標系で計算して写すかを M4 で決める（reach の求め方が変わる）
- 近くの画素に依るもの（ぼかし・morphology・convolve・displacement・照明・offset）は、範囲を描くときは、中で計算する範囲を
  reach の分だけ広げる。
  連鎖では reach を足し合わせる。広げる量が決まらないもの（feTile・feImage の一部・`edgeMode` が `wrap`・`duplicate` の
  feConvolveMatrix）は filter の領域全体を求める

---

## 7. 文字とフォント

### 7.1 フォントの読み取り（font。ADR-29・ADR-36）

- sfnt の表を共有したバイト列の上で読む。読む口は範囲を確かめる関数だけ（`get(位置, 長さ) -> Option<&[u8]>`）
- 表：
  - 共通：head・hhea・hmtx・maxp・cmap（形式 0・4・6・12・13・14）・name・OS/2・post・glyf・loca・CFF・STAT（`trak` を当てるか
    の判定）
  - OpenType の組版：GDEF・GSUB・GPOS・`kern`（OpenType の形。版 0）
  - AAT の組版（macOS のシステムのフォントの約 2 割。787 書体のうち 165 が `morx` を持ち GSUB を持たない。Times・Helvetica・
    Courier・Apple Chancery・GeezaPro など）：`morx`（副表の形 0 並べ替え・1 文脈・2 合字・4 非文脈・5 挿入）・`kerx`
    （と `ankr`）・`trak`・`kern`（Apple の形。版 0x00010000、副表の形 0〜3）
  - M6 で縦書き（vhea・vmtx・VORG）。可変フォント（fvar・gvar・avar・HVAR）は M5 の前に決める（ADR-14）
- 組版の方式の選び方（HarfBuzz・harfrust と同じ。M5 で resvg の描いた結果と照らして確定する。ADR-36）：
  - 置き換え：横書きは `morx` があれば AAT（GSUB があっても）、なければ GSUB。縦書きは GSUB があれば GSUB
  - 位置（2 段。ADR-42）：(1) `kerx` があれば `kerx`（GSUB と GPOS の両方を当てるときを除く）、なければ GPOS (2) (1) で `kerx` を
    当てず、かつ GPOS を当てないか GPOS に `kern` の feature がないなら、`kerx` があれば `kerx`、なければ `kern` 表（OpenType の
    形か Apple の形）。ただし (2) は、文字の種類の組版の規則が後ろ盾の位置合わせを許すときだけ（アラビア文字などでは当てない）
  - `font-kerning: none` は `kern` の feature・`kerx`・`kern` 表をどれも当てない
  - `trak`：方式と別に、`trak` と `STAT` の両方を持つ書体だけに当てる
- 字形ごとの上限（版に入る定数。`Limits` に入れない）：CFF の字形の命令は仕様の上限（積み上げ 48・副手続きの呼び出し 10 段）と
  字形ごとの手間の上限、複合の glyf は入れ子と部品の数の上限、字形ごとの点の数の上限。超えた字形・壊れた字形は輪郭を空に
  して警告する（文書は断らない。ADR-34）
- 組版の上限（版に入る定数）：区切りの中の文字の数に比例する、GSUB・GPOS の lookup の当てはめの回数・文脈の照合の歩数・
  `morx`・`kerx` の状態機械の歩数（「進めない」旗で同じ字形に留まり続けうる）の上限と、入れ子の lookup の深さの上限（GSUB の
  型 5・6、GPOS の型 7・8 はほかの lookup を呼ぶ。拡張の型 7・9 を通しても同じ。`morx` の文脈の副表が呼ぶ置き換えも同じに
  扱う）。入れ子は明示の積み上げで書く。超えたら、それ以上当てはめずに続ける（決定的。輪郭は空にしない）
- 表が壊れていれば、その表がないものとして扱って警告する（GSUB が壊れていれば置き換えなし、など）
- `FontSet::new` で書体ごとの索引を作る：書体名・太さ・幅・傾き・輪郭の表の有無・組版の係数（GSUB・GPOS の lookup の数・
  文脈の最長・`morx`・`kerx` の副表と状態の数など。読む手間に上限のある数）・cmap の範囲の並び（文字ごとに「その文字を持つ
  書体」を引け、代わりのフォントの見積もりを狭められる。持ち方は M5 で決める）。書体名は name 表の Windows の名前（UTF-16BE）と
  Mac の名前（Mac Roman）を読み、ASCII の大文字小文字だけを無視して比べる（std の `to_lowercase` を使わない。fontdb の照らし方と
  M5 で照らす）。手間と確保を `FontLimits` で数え、中止の印を
  見る。字の範囲（cmap）は文字を引くときに読む
- 組版の手間の見積もり：1 文字の組版の手間の上限は「版に入る定数 × その書体の係数」。代わりのフォントは、cmap の範囲の
  並びで引いた「その文字を持つ書体」だけを数えて見積もる。照合の集まりで「手間の見積もり ≤ k × 実際」を目標にする（k は
  M5・M7 で決める。NFR-05）
- 絵文字の書体（Apple Color Emoji は sbix と glyf の両方を持ち、glyf の字形はほぼすべて仮の輪郭）：その文字の sbix の絵が
  ある書体は、その文字を持たないと数える（字ごとに数える。ADR-40・ADR-56 (2)）。M0 で、raster-images なしの resvg は
  Apple Color Emoji を選んで何も描かない（送りの幅だけ空く）と確かめた。glyf も CFF も持たない書体（sbix だけ・ビットマップ
  だけ）は、これまでどおり字を持つと数えない（ADR-43）

### 7.2 組版（text）

1. 文字の塊（chunk）と文字ごとの位置の並び（x・y・dx・dy・rotate）を、SVG 2 の addressable character（符号位置ごと。畳んだ
   空白は数えない）に割り当てる（resvg の数え方と M5 で照らす）
2. 双方向（UAX #9。`direction`・`unicode-bidi` を含む。M6。ADR-14）で向きの区切りを決める
3. 文字の種類（Unicode の Script・Script_Extensions）で区切り、文字ごとに書体を選ぶ（FR-FO-03〜05）
4. 区切りごとに、組版の方式（§7.1）で字形を置き換え、位置を決める。OpenType の方式は cmap → GSUB → GPOS（HarfBuzz の
   既定に当たる feature の組を、書体の文字の種類と言語で選ぶ）、AAT の方式は cmap → `morx`（既定の旗）→ `kerx`・`kern`。
   `trak` は §7.1 の判定で当てる
5. 位置を決め、text-anchor・textLength・letter-spacing・word-spacing・textPath・装飾を当てる。textPath の弧長の精度は、倍率に
   依らない量で決める（根のユーザー単位に、参照先の path 自身と祖先の変形の拡大を掛けた座標系での誤差）。木を作る段は倍率を
   知らず、`Tree` は倍率をまたいで使うため（ADR-43）
6. 字形の輪郭を木のパスにする

Unicode の表は、`sumie-tools` の `gen-unicode` が Unicode の文字のデータベース（UCD。版と SHA-256 を固定）から生成して
`src/unicode/` にコミットする。手で書き換えない。生成する属性：

| UCD のファイル | 属性 | 使い道 |
|---|---|---|
| UnicodeData.txt | General_Category・Canonical_Combining_Class・分解 | 結合の記号・mark の並べ・書体にない合成済みの文字を分解して探す |
| CompositionExclusions.txt・DerivedNormalizationProps.txt | 合成の除外 | 分解した文字の合成 |
| BidiBrackets.txt | Bidi_Paired_Bracket・Bidi_Paired_Bracket_Type | UAX #9 の N0 |
| BidiMirroring.txt | Bidi_Mirroring_Glyph | 右から左の鏡の文字 |
| extracted/DerivedBidiClass.txt | Bidi_Class | UAX #9 |
| extracted/DerivedJoiningType.txt・ArabicShaping.txt | Joining_Type（載らない Mn・Me・Cf を T とする規則を含む）・Joining_Group | アラビア文字の接続 |
| Scripts.txt・ScriptExtensions.txt | Script・Script_Extensions | 文字の種類の区切り |
| DerivedCoreProperties.txt | Default_Ignorable_Code_Point | ZWJ・異体字セレクタなどを描かない |
| VerticalOrientation.txt | Vertical_Orientation | 縦書き（UAX #50） |

---

## 8. 範囲の描画（ADR-13・ADR-38）

- 約束：`Plan::render_region(rect)` の画素は、`Plan::render` の結果の同じ矩形とビット単位で同じ
- 方式：
  - 点・線分・線の外形・破線は計画の段で 1 度だけ、範囲に依らずに決める（§6.1）
  - 範囲と交わる節だけを選ぶ（画素の外接矩形が範囲と交わる節。フィルタは reach で広げた範囲）
  - 層の原点はキャンバスの画素の格子に揃え、層の矩形は「全体の描画での層の矩形 ∩ 広げた範囲」にする
  - pattern のタイル・フィルタの計算面・mask の輝度の層・feImage の格子は、範囲に依らない格子と大きさで作る（範囲で切るのは
    最後の写しだけ）
  - セルは線分の元の両端から閉じた式で求め、範囲の左の外の分は cover だけを足す（整数なので同じ値。§6.1）
  - フィルタは reach で広げた矩形で計算して範囲を切り出す。広げられないものは filter の領域全体を求める
- `estimate_region` は範囲ごとの手間と確保を返す。Kura はタイル 1 枚あたりの作業領域（`per_task`）として使い、`Plan` の
  `held_bytes` を共有の分（`shared`）にする
- 確かめ方：種つきのランダムな矩形（1 画素・端・空・全体・256 に揃えたタイル）で全体の切り出しと比べる性質のテスト。
  範囲の境目をまたぐ曲線・破線・太い線・巨大な図形の例を必ず入れる

---

## 9. 安全の仕組み

| 危険 | 仕組み |
|---|---|
| スタックのあふれ | 再帰しない。木のたどり・セレクタの照合・参照の解決・展開・フィルタの連鎖・CFF の副手続き・複合の glyf・入れ子の lookup は明示の積み上げ。入れ子の `Box` を持つ型を作らない（捨てるときと `derive` で再帰する）。大きな配列をスタックに置かない（`clippy::large_stack_arrays`・`large_stack_frames`）。NFR-03 の小さいスタックのテスト |
| 終わらないループ | 走査はどの枝も 1 バイト以上進む。状態機械（`morx`・`kerx`）は歩数の上限。ほかのループは回る数を先に `spend` する |
| 上限を超える確保 | `mem` の口だけで確保し、先に見積もって断る。漏れは数える確保器の試験で見つける（§5.1） |
| panic | CLAUDE.md の一覧（clippy の lint・`disallowed-*`・`tests/rules.rs`）。一覧は M0 で、std の文書の Panics の節と確保する口から機械的に作る |
| ビルドで分かれる振る舞い | `cfg(debug_assertions)`・`cfg!(debug_assertions)`・ライブラリの `[features]` を持たない（`tests/rules.rs`） |
| 壊れたフォント | §7.1。表の範囲の外を読まない。壊れた書体は `FontSet` が飛ばし、壊れた表は無いもの、壊れた字形は空として警告 |
| 巨大な数・座標 | 数は有限の `f64` だけ（FR-ST-15）。変形のあとの点が有限でない節は描かない。決まった箱の外の片側にある曲線は平坦化しない。決まった箱で切ってから固定小数にする。平坦化の点の数は見積もりで抑える（§6.1。ADR-32） |

パーサーごとに cargo-fuzz の対象と、種つきの変異のテスト（`tests/mutate.rs`。種と回数を固定。`sumie-testkit` の ChaCha8）
を持つ。

---

## 10. 照合と試験の道具（sumie-tools）

M0 にあるのは `oracle`・`compare`・`bench`（と子の `bench-one`）・`corpus`（`import-resvg`・`resvg-source`・`fetch-w3c`・
`fetch-real`・`index`）・`gen-heavy`・`kura-compat`・`oracle-lock`・`gen-disallowed`・`check-lint-probe`。M1 で `probe`・`wellformed`・`latency`・`sizes`・`values`
を、M2 で `compare-sumie`・`bench-sumie` を足した。「（M5 から）」などの印のものはまだない。描いた画素を PNG にする口と
ゴールデンの行は、道具でなく `compare-sumie` の出力（`img/<id>-s.png`）と試験（`cargo test -p sumie --test golden -- --ignored
regenerate_renders`）が受け持つ。

| 道具 | すること |
|---|---|
| `oracle resvg` | resvg 0.48.1（Kura と同じ feature）を、Kura と同じ usvg の設定（総称の書体 Times・Helvetica・Apple Chancery・Papyrus・Courier、フォントを名前の順に登録、languages、`default_size`）で描く（出力は乗算済み）。resvg の下の依存（svgtypes・roxmltree・unicode-script など）の版も Kura の Cargo.lock に揃え、`crates/sumie-tools/oracle-lock.txt`（`oracle-lock` が Kura の lock から書く。Kura の rev つき）と試験で照らす（ADR-57 (1)） |
| `oracle chrome` | `/Applications/Google Chrome.app` のヘッドレスで描く（決めた描き方は下の「Chrome の描き方」。Chrome の版を記録）。PNG はストレートなので、決めた式で乗算済みに直す |
| `compare` | 2 つの描画の差を乗算済みの RGBA8 で測る（ADR-15 の基準）・差の画像・一覧の HTML。行に描いた入力の BLAKE3（`input`）を書く |
| `compare-sumie` | sumie を resvg・Chrome と比べる（M2 から。ADR-89 (2)）：後のマイルストーンの機能の印を外し、`NotYet` は理由ごとに数えて外す（ほかの断りは「描けない」）。合否は sumie と resvg に ADR-15 の基準、resvg と Chrome が基準の内で sumie が両方から外れるものを不具合の候補にする。`--cache` の `compare` の出力の絵を、道・入力のダイジェスト・倍率が同じときだけ使う |
| `bench-sumie` | sumie の段ごとの時間（読む・木を作る・計画・描く・タイルの合計。`--repeat` 回の最小）と見積もりの手間、時間 ÷ 手間（ns／単位）を並べる（M2 から。NFR-05・NFR-07）。見積もりが断れば、断った段と要る量を書く |
| `corpus` | 照合の集まりの取り込み（出どころ・版・ライセンス・ダイジェストの一覧。resvg の試験・W3C・実例の SVG。ADR-21・ADR-59）と、使っている機能での分類（マイルストーンごとの範囲を選ぶ） |
| `probe` | M1 の範囲を決める照らし：CSS のセレクタ・アットルール・カスケード・SVG 2 の属性・値の書き方・大きさの端の小さな SVG を `tests/svg/m1-probe/` に書き、resvg と Chrome で描いて、決めた点の色で「効く・効かない」を表にする（`README.md`。ADR-64） |
| `wellformed` | XML の整形式の照らし：照合の集まり・Kura のファズのコーパス（`--kura-fuzz <dir>`。読むだけで、名前は BLAKE3 にする）・変異（`--mutate N`）・端の例を、roxmltree（usvg が外に出しているもの）と sumie の XML の段（`sumie::fuzzing::xml_parse`）の両方で読み、受ける・断るの食い違いと、どちらも読む文書の木（名前空間・名前・属性・文字。`sumie::fuzzing::xml_tree`。ATTLIST のある文書は比べない）の違いを `local/wellformed/`（`summary.md`・`result.jsonl`・食い違った入力の `diff/`）に書く（FR-IN-04・ADR-65） |
| `values` | 計算した値の照らし（ADR-63 (5)）：照合の集まりを Kura の設定の usvg の木と sumie の `fuzzing::computed_values` で読み、id のある要素（id がどちらにも 1 つだけのもの）ごとに、描く木に入るか・根からの変形・塗り・線（塗り・不透明度・太さ）・不透明度・表示を比べる。図形は usvg の path と、群は usvg の群と比べ、図形の不透明度は最も近い id のある祖先までの積で比べる（usvg は図形の上に名前のない群を作る）。`local/values/`（`summary.md`・`result.jsonl`）に書く。`--dump <in.svg>` で 1 つの文書の両方を並べる |
| `sizes` | 受ける・断ると大きさの照らし：照合の集まりを Kura の設定の resvg と sumie の `parse` で読み、`Document::size`・`pixel_size(1)` を resvg の木の大きさと比べる（同じ・中身で決まる（M2 から比べる）・違う・sumie が断る・resvg が断る）。`local/sizes/`（`summary.md`・`result.jsonl`）に書く（FR-OUT-02・NFR-10）。あわせて、usvg が描く木のために解こうとした画像の種類（画像の参照を解く口で記録し、読まずに捨てる）と `Document::needs` を比べる（ADR-63 (1)） |
| `latency` | 敵対的な入力（レビューで見つかった、払わない長いループや 2 乗の形を集めたもの。`--scale N` で大きくする）を `sumie::parse` に通し、手間・実時間・1 単位あたりの実時間・中止の印を読む間の実時間の最大を表にする（FR-LIM-04・FR-LIM-05・NFR-05・NFR-06。M7 の校正の資料にもする） |
| `kura-compat` | Kura の側の「描く／断る」と大きさの一覧（`corpus/kura-request.md`）と照らす（ADR-62 (4)。依頼は取り下げたので、一覧が届いたときに使う。ADR-63 (1)） |
| `oracle-lock` | 照合の相手の依存の閉包の版と checksum を Kura の Cargo.lock と照らして記録する（ADR-57 (1)・ADR-60 (5)） |
| `gen-disallowed`・`check-lint-probe` | ライブラリの禁じる口の一覧を std の文書から作る・それが本当に拾われるかを試しのクレートで確かめる（ADR-49） |
| `gen-heavy` | 重い入力の生成（下の表）。種と引数を固定し、作った入力のダイジェストを記録する |
| `gen-fonts` | （M5 から）試験用の AAT のフォントの生成（写しを許すフォントに fontTools で `morx`・`kerx`・Apple の `kern`・`trak` を足す。作り方と出どころを `tests/fonts/` の一覧に書く） |
| `bench` | resvg 単体と同じ入力で時間・確保のピークを比べる（1 件ずつ子のプロセス `bench-one`）。タイルで描いたときの合計の時間も測る（M2 から） |
| `gen-unicode` | （M5 から）Unicode の表の生成（§7.2） |

**Chrome の描き方**（M0 で決めた。ADR-52・ADR-57 (2)・ADR-60）：SVG を HTML の `<img>`（CSS の画素で resvg の小数のままの大きさ、切り出しは resvg の整数の大きさ W・H）で
左上に置き、`--headless --disable-gpu --hide-scrollbars --default-background-color=00000000 --force-device-scale-factor=1
--force-color-profile=srgb --window-size=max(W+1,800),max(H+1,600) --screenshot` で撮って、左上の W × H を切り出す。ヘッドレスの
窓の幅は 500 より小さくできず、SVG の文書を直に開くと viewBox だけの SVG が広い表示の領域の真ん中に描かれるため。
`--disable-gpu` は GPU を使わないソフトウェアのラスタ化に固定する（既定の起動では 64 × 64 の円の線で縁の α が 126 → 116
などと変わり、GPU と駆動の層に依るので、機械が変わると分布が再現しない）。Chrome が SVG を読めないときは `<img>` の
`onerror` で窓の背景を印の色（rgb(255,0,255)）にし、窓の右下の画素（切り出す領域の外）でそれを見て「Chrome が描けない」に
する（壊れた画像の印を差として数えない）。撮った絵が窓より小さいときも誤りにする。1 回の撮影の時間の上限は 120 秒で、撮れない・
時間切れは 1 度だけやり直す（並べて回すと、まれに Chrome が絵を書かずに終わるか止まる。やり直した回数は照合の結果に書く）。
大きさは CSS の画素で小数のまま渡すが、Chrome は `<img>` を画素の格子に合わせるので、大きさが整数でない文書（`10.5`・`10mm`
など）は引き伸ばして描き、縁の値が resvg と食い違う（照合の結果に `fractional_size` の印を付ける）。Chrome の CPU のラスタは
命令の種類で縁の値が変わりうるので、照合と計測の記録に CPU の名前を書く。`prefers-color-scheme` を使う SVG は OS の見た目の
設定で変わる（今の集まりにはない）。照合の働き手は 256 MiB のスタックで回し（深い入れ子で全体が落ちないように）、前の回の
出力を消してから書く。
画像としての SVG は外の資源を読まない（resvg の `resources_dir: None` と揃う）。乗算済みに直す式は `(c × a + 127) / 255`。
M0 の照合は Chrome 157.0.8103.79。

`gen-heavy` の入力（M0 で引数を決めた。`sumie-tools gen-heavy` が `local/heavy/` に作り、`index.jsonl` に引数と BLAKE3 を書く。
超越関数を使わず（角は定数の表）、どの OS でも同じバイト列で、道具の試験が BLAKE3 を照らす。S1・S4 の 1 本・pattern の入れ子は
Kura の試験の入力（`SLOW_SHAPE`・`SLOW_DECODE`・`nested_pattern_clones_are_refused_before_usvg`）と同じ形。ほかは Kura の説明から
作った同じ性質の入力で、Kura の元の入力の写しではない。地図風の線と S5 は Kura の元の引数が分からない）。「Kura」の列は Kura の
記録の時間（S1〜S7 は Kura の `docs/editing.md` §9.5 と `svg_worker` の注釈。地図風の線は働き手を通した値の時間）。resvg 単体の
時間と確保の山は M0 の計測（下の「resvg 単体の計測」）：

| 名前 | 引数 | Kura |
|---|---|---|
| `map-lines` | 4096 × 4096・折れ線 1,000 本 × 300 区間（30 万区間）・ランダムウォーク（8 方向・1 歩 2〜12 画素）・線の幅 1.5・継ぎ目と端 round・ChaCha8 の種 1 | 4.5 秒（働き手を通した値。元の引数は分からない） |
| `map-shapes` | 4096 × 4096・閉じた path 45,000 本（1 本 20 頂点・半径 4〜20 画素を頂点ごとに 0.6〜1.0 倍に揺らす（2.4〜20 画素）・8 色の塗り）・種 2（約 14.5 MiB） | 形 0.24 秒 |
| `s1-rotated-thick-stroke` | 100 × 100・`translate(1e9)` と `rotate(-12.5)` の下の、太さ 1e7 の線の 3 次曲線 2 区間（`SLOW_SHAPE`） | 2 区間で 2.6 秒（40 区間・1.4 KB で 7 分） |
| `s2-many-stops` | 1000 × 1000・stop 10 万のグラデーションで塗った全面の rect 10 枚 | 32 秒 |
| `s3-same-offset-stops` | 100 × 100・offset がすべて 0.5 の stop 40 万（短く書いて入力の上限 16 MiB に収める） | 形 14 秒 |
| `s4-fine-dashes-one` | 1000 × 1000・3 次曲線 1 本・太さ 3・破線 `0.0005 0.000501`・丸い端（`SLOW_DECODE`） | 8.9 秒 |
| `s4-fine-dashes-37` | 同じ曲線を縦に 26 ずつずらした 37 本 | 310 秒 |
| `s5-rotated-pattern` | 4000 × 4000・`patternTransform="rotate(30)"` の 4 × 4 のタイルで塗った全面 | 102 秒（元の引数は分からない） |
| `s6-textpath-unseparated-transform` | 200 × 100・`transform="translate(10-5)"` の path へのふつうの textPath（かならず描く互換の例） | — |
| `s7-turbulence-octaves` | 1000 × 1000・`numOctaves="1000"` の feTurbulence | 18 秒（大きさは分からない） |
| `bomb-use-doubling-16` | use の倍々 16 段（2¹⁶ 個。usvg 0.48.1 の節の上限の内で受ける最大） | — |
| `bomb-use-doubling-17` | use の倍々 17 段（2¹⁷ 個。usvg が「nodes limit reached」で断る） | — |
| `bomb-pattern-nesting` | 8 × 8・objectBoundingBox の pattern の入れ子 16 段・1 段に参照 2 つ（2¹⁶ の複製。Kura の試験は 23 段で、Kura が木を作る前に断る） | — |
| `bomb-marker-dup` | 頂点 10 万の path の `marker-mid`（種 3） | — |

**resvg 単体の計測**（M0。`sumie-tools bench`。Apple M4 Max・道具の release・数える確保器のもと・1 件ずつ子のプロセスで、
組み立てなどを止めた機械で測った。重い入力は 1 回、集まりは 3 回の最小。時間は木を作る分 ＋ 描く分、確保の山は 1 回目の
山。山は出力の画像（幅 × 高さ × 4 バイト。`s5` の 64 MB の大半・`map-lines` の 72 MB のうち 67 MB）を含む（sumie の
`peak_bytes` は出力のバッファを除くので、比べるときは引く）。フォントの集まりを作る分は数えない。ADR-57 (4)。生の結果は
`local/bench/`（`heavy-resvg.jsonl`：`bench --root local/heavy --repeat 1 --timeout 900`、`corpus-resvg.jsonl`：`bench --root
corpus --repeat 3 --timeout 30`。どちらも道具は 3c6233e。`real-resvg.jsonl`：`bench --root corpus --filter real/ --repeat 3
--timeout 30`。道具の bench は 3c6233e と同じ）。確保の山は allocation-counter が realloc を
「新しく確保 → 写す → 古いものを放す」として数えるので、伸ばす瞬間の新旧の和を数える。Vec を伸ばしながら使う resvg は、
`try_reserve_exact` で決めた量を確保する sumie より大きめに出る（比べるときは参考にする。ADR-60 (7)））：

| 入力 | 大きさ | 木を作る | 描く | 確保の山 | Kura の記録 |
|---|---|---|---|---|---|
| `map-lines` | 4096² | 62 ms | 445 ms | 72 MB | 4.5 秒（働き手を通した値。入力が違う） |
| `map-shapes` | 4096² | 77 ms | 364 ms | 98 MB | 形 0.24 秒 |
| `s1-rotated-thick-stroke` | 100² | 2.77 秒 | 20 ms | 1.40 GB | 2.6 秒 |
| `s2-many-stops` | 1000² | 24 ms | 33.5 秒 | 48 MB | 32 秒 |
| `s3-same-offset-stops` | 100² | 15.1 秒 | 0.1 ms | 158 MB | 形 14 秒 |
| `s4-fine-dashes-one` | 1000² | 0.1 ms | 9.4 秒 | 1.27 GB | 8.9 秒 |
| `s4-fine-dashes-37` | 1000² | 0.2 ms | 387 秒 | 2.27 GB | 310 秒 |
| `s5-rotated-pattern` | 4000² | 0.1 ms | 600 ms | 64 MB | 102 秒（入力が違う） |
| `s6-textpath-unseparated-transform` | 200 × 100 | 0.2 ms | 0.1 ms | 0.15 MB | — |
| `s7-turbulence-octaves` | 1000² | 0.1 ms | 17.1 秒 | 12 MB | 18 秒 |
| `bomb-use-doubling-16` | 200² | 286 ms | 32 ms | 196 MB | — |
| `bomb-use-doubling-17` | — | 断る（nodes limit reached） | — | — | — |
| `bomb-pattern-nesting` | 8² | 1.07 秒 | 0.02 ms | 111 MB | — |
| `bomb-marker-dup` | 1000² | 118 ms | 2.82 秒 | 97 MB | — |

照合の集まり（resvg の試験 1,739 件。描けた件のうち文字の印のある 422 件はシステムのフォント。分布の表の「文字」の群は
題に (UB) とある例を先に分けるので 408 件（描けない 1）で、数え方が違う）：描けたのは 1,736 件（読めない 3 件：大きさが
不正 2・UTF-8 でない 1）。1 件の時間は中央 0.13 ms・90% 0.77 ms・99% 3.2 ms・最大 850 ms（`filters/feMorphology/huge-radius`）・
合計 1.51 秒。確保の山は中央 184 KB・90% 605 KB・99% 1.08 MB・最大 12.2 MB（`filters/filter/huge-region`）。

重い入力の S1〜S4・S7 は Kura の記録と同じ程度の時間になり、Kura の性質を持つ入力だと確かめられた。地図風の線と S5 は Kura の
元の入力と違う（ADR-58 (2)。NFR-07 の約束は同じ入力の resvg 単体との比で見る）。

実例の SVG（1,241 件。ADR-59。倍率 1）：すべて描けた。1 件の時間は中央 0.05 ms・90% 1.2 ms・99% 3.8 ms・最大 11.3 ms・
合計 0.50 秒、確保の山は中央 18 KB・90% 1.3 MB・99% 5.4 MB・最大 6.0 MB。

**sumie の計測（M2。`sumie-tools bench-sumie <file> --repeat 3 --tile 256`。Apple M4 Max・道具の release・3 回の最小・0eb1503
の木（係数は ADR-98 の仮の校正）。時間はミリ秒、ns／単位は時間 ÷ 見積もりの手間）**：

| 入力 | 読む | 木を作る | 計画 | 描く（全体） | 256 のタイルの合計 | 見積もりとの比 |
|---|---|---|---|---|---|---|
| `map-lines`（30 万区間） | 30.7 | 20.9 | 120.2 | 184.1 | 209.6 | 読む 1.17・木 0.37・計画 0.32・描く 0.47 ns／単位 |
| `map-shapes` | 209.3 | 104.6 | 106.5 | 275.2 | 259.1 | 読む 1.08・木 0.33・計画 1.13・描く 1.10 ns／単位 |
| `s1-rotated-thick-stroke` | 0.014 | 0.005 | 0.002 | 0.001 | 0.002 | 見積もりの内（計画 409 万・描く 7,762 万単位の見積もりに対して短い） |
| `s4-fine-dashes-one` | 0.015 | 0.005 | — | — | — | `Tree::estimate` が計画の見積もり 84 億単位で `plan_work`（2³²）を超えて断る（描く前） |
| `s4-fine-dashes-37` | 0.16 | 0.040 | — | — | — | 計画の見積もり 3,119 億単位で、`Tree::estimate` が断る |
| `bomb-use-doubling-16` | 0.059 | 72.7 | 19.5 | 4.4 | 4.4 | 木 0.51・計画 0.56・描く 0.15 ns／単位 |
| `bomb-use-doubling-17` | 0.059 | 142.6 | 38.4 | 8.7 | 8.7 | 木 0.50・計画 0.55・描く 0.14 ns／単位（usvg は断る） |

どれも時間 ≤ 見積もりの手間 × 2 ns（roadmap M2 の完了条件）。地図風の線の全体は 356 ms（resvg 単体 507 ms）。M2 の集まり
（1,445 件）の分布は ADR-98 (1)。

**M0 の照合の分布**（resvg 0.48.1（Kura の設定）と Chrome 157.0.8103.79（`--disable-gpu`）、Apple M4 Max。`sumie-tools compare`。
割合の分母は全画素、形の内側は平らな所、長い辺が 200 画素に満たない絵は整数倍で 200 画素以上にして比べる。ADR-56 (1)・
ADR-61）：

| 集まり・群 | 測れた件数（描けない） | 基準で通る | 割合の分布（中央・90%・95%・99%） | ほか |
|---|---|---|---|---|
| resvg の試験・図形と単色（8/255・2%・平らな所 2/255） | 640（resvg 2） | 595 | 0.03%・1.0%・3.3%・64% | 目標 0.5% で 478。もとの内側（α だけ）なら 575 |
| resvg の試験・フィルタ・グラデーション（16/255・1%） | 561（0） | 501 | 0.01%・1.6%・18.6%・64% | — |
| resvg の試験・文字（分布の外） | 407（resvg 1） | — | 4.6%・8.5%・10.2%・16.0% | 書体が両者で違う |
| resvg の試験・画像（分布の外） | 72（0） | — | 19.6%・64%・64%・64% | resvg は画像を読まない |
| resvg の試験・未定義の振る舞い（題に (UB)。分布の外） | 56（0） | — | 0.49%・31.6%・64%・64% | — |
| 実例・図形と単色 | 1,114（0） | 1,007 | 0.98%・2.0%・2.5%・3.4% | 目標 0.5% で 253。もとの内側なら 429。倍率 1 なら 283（もとの内側なら 60） |
| 実例・フィルタ・グラデーション | 127（0） | 108 | 0.63%・1.2%・1.5%・1.7% | 倍率 1 なら 9 |

実例の集まりごとに通る数：Bootstrap Icons 168/200・Tabler Icons 192/200・flag-icons 263/271・Fluent Emoji の Color 101/120・
Flat 146/150・Twemoji 245/300。resvg の試験は 1,691 件が倍率 1 のまま（拡大したのは 200 画素より小さい 45 件）。Chrome を
やり直して撮れたものは resvg の試験で 3 件・実例で 2 件、大きさが整数でないものは 0 件。回の結果は `local/compare/` に残す
（`resvg-tests`・`real` がこの表。倍率 1 は `resvg-tests-scale1`・`real-scale1`、ADR-61 を決めたときの 256 画素の回は
`real-256`・`resvg-tests-256`、GPU のラスタ化は `resvg-tests.gpu`。git の外）。

Chrome の描き方の違いの記録（倍率 1・もとの内側（α だけ）で、`resvg-tests-scale1`（ソフトウェアのラスタ化）と
`resvg-tests.gpu`（GPU のラスタ化。その回の report.jsonl を今の群分けで数え直した）を同じ 640・561 件で比べた）：図形と単色の
2%・内側 2/255 でソフトウェア 576 件・GPU 562 件、0.5% で 463 件・369 件、フィルタ・グラデーションの 1% で 500 件・502 件
（1,739 件のうち 1,706 件で 8/255 を超える画素の数が変わった）。ソフトウェアのラスタ化は、図形と単色では resvg に近く、
フィルタ・グラデーションではほぼ同じ。ADR-56 の理由の数（639 件で 575・462 など）は、Chrome のやり直しを入れる前の回
（035792c のときの §10。出力は残っていない）のもの。外れるものは、
resvg と Chrome の対応の違い（全面が違う 64% の山）と、縁の細かい違いが多い。Chrome は合否にしない（差の一覧に並べる）。

もとの表（下敷き）：

| 名前 | 中身 | 下敷き |
|---|---|---|
| 地図風の線 | 折れ線を合わせて 30 万区間（本数・1 本の区間の数・線の幅・継ぎ目・端・キャンバスの大きさを引数にする） | Kura の `svg_worker/mod.rs` の実測（働き手を通した値で 4.5 秒） |
| 地図風の形 | 短い path 4.5 万本・16 MiB | 同上（形 0.24 秒） |
| 遅い入力 S1〜S5・S7 | 回転の下の太い線・stop の多いグラデーション・同じ offset の stop・細かい破線の丸い端・回転した pattern・オクターブの多い feTurbulence | Kura の `docs/editing.md` §9.5 |
| 互換の例 S6 | 区切りのない数の transform（svgo の `translate(10-5)` など）を持つ path への、ふつうの textPath。かならず描く | Kura の `docs/editing.md` §9.5（Kura が断りすぎていた不具合） |
| 展開の爆弾 | use の倍々・objectBoundingBox の pattern の入れ子・marker の複製 | Kura の `svg.rs` の試験 |

---

## 11. Kura との境目

| Kura の今 | この描画器のあと（M7 で Kura に渡す案） |
|---|---|
| `codec/svg.rs` の前段の見積もり（DTD・深さ・CSS・参照・marker・数） | `estimate_parse`・`parse` の中の上限と `estimate_build` |
| `codec/svg_text.rs` の文字の手間の見積もり | 木を作る段の手間（組版の中で数える） |
| 形（`SvgCodec::shape`：usvg の木を作り `check_render` まで通す） | `parse` → `estimate_build` → `build` → `Tree::estimate`（§3.1） |
| `check_render`・`Plan::Render` | `Tree::estimate`・`Plan::estimate_region` |
| `SvgFonts`（fontdb の Database。名前の順に並べて登録） | `FontSet`（同じバイト列を `Arc` で共有。`FontKey` は Kura が作る。渡す順は D1 に入るので、Kura は今と同じく名前の順で渡す） |
| `svg_worker`（別のプロセス・2 秒・10 秒・AllocMeter） | 要らない（中止の印と手間の予算） |
| `reproducible() = false`・アンカー | Kura の `Decoder::reproducible(&self)` はデコーダごとの値で文書を受けないので、文書ごとに変えるには Kura の trait の変更（文書ごとの再現の印を返す）か、文字のある SVG を別の識別子に分けることが要る（M7 の `kura-integration.md` で案を出す）。そのうえで、文字のない SVG は真にできる。文字のある SVG は、フォントの集まり全体（`FontSet::faces` の鍵の並び）と、報告の使ったフォントで約束する（代わりのフォントは集まりの順に探すので、使っていない書体の増減でも値が変わりうる。Kura の側の判断） |
| 画素を描いてから `demultiply` | 同じ（Kura の側で凍結した式） |
| デコーダに範囲・倍率の口がない | `Plan::render_region`・倍率（Kura の側で口を足すかは Kura の判断。タイルで描くか全体で描くかは M7 で決める） |

**誤りの種類が変わる所**（Kura の `Unsupported` は次の段へ回る。ADR-24）：

| 入力 | Kura の今 | この描画器 |
|---|---|---|
| 描く前の資源の見積もりを超えるもの | `Unsupported` | `Limit` |
| 深さ 64 を超える入れ子・結合子が 32 を超えるセレクタ | `Unsupported` | 上限の内なら描く。超えれば `Limit` |
| 参照の輪 | `Unsupported` | 輪の参照を取り除いて描く（警告） |
| 絶対値が 10⁹ を超える数 | `Unsupported` | 描く（平坦化の点の数が上限を超えれば `Limit`。ADR-32） |
| UTF-8 でない符号化 | 読み取りの誤り | `Unsupported` |

SVG の次の段のデコーダ（macOS の ImageIO と見られる）が SVG を描けるかは確かめていない（要確認。M7 で Kura の側の段の並びを
確かめる）。描けないなら、`Unsupported` と `Limit` の違いの実害は小さい。
