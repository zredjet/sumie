# 要件

優先度：**M**＝v1 に必須／**S**＝v1 に入れたい（条件つき。各行に書く）／**C**＝v1 の外（余裕があれば）

決まったことの記録は `docs/decisions.md`（ADR-xx）。Kura の ADR は「Kura の ADR-xx」と書く。

---

## 1. 目的と前提

### 1.1 目的

静的な SVG を読んで RGBA8（乗算済み）の画素に描く Rust のライブラリ。Kura の編集のエンジン（`kura-engine`）が、
今の resvg 0.48.1 と、それを囲う次のものをやめられるようにする。

| Kura が今していること | この描画器で代わるもの |
|---|---|
| resvg（usvg・tiny-skia・harfrust・fontdb・roxmltree など 61 クレート）で読む | 依存が `libm` だけの自作の描画器 |
| OS の libm を使うので `reproducible()` を偽にし、読むたびにアンカーを残す | 決定性（D1。determinism.md）。文字のない SVG はアンカーが要らなくなる（Kura の `reproducible()` はデコーダごとの値なので、Kura の口の変更が要る。A-09）。文字のある SVG は、フォントの集まり全体（`FontSet::faces` の鍵の並び）と、報告の使ったフォントで約束する（代わりのフォントは集まりの順に探すので、使っていない書体の増減でも値が変わりうる） |
| 止められないので、別のプロセスで読み、形（大きさを読む）2 秒・値 10 秒で殺す（`svg_worker`。Kura の ADR-164） | 中止の印（決まった手間ごとに見る）と、手間の単位の予算 |
| 再帰であふれるので、生のバイト列で深さを数え、64 段で断る | 再帰しない実装（明示の積み上げ） |
| 約 3,000 行の見積もりを外から書く（DTD・use・marker の複製・CSS・文字の 2 乗・textPath の精度・破線・pattern） | 自分の実装に合わせた見積もりを、読む前・木を作る前・描く前に返す口 |

### 1.2 前提

- 利用者は Kura（Mac 用、Tauri 2 ＋ Rust）。ただしライブラリは Kura を知らない（Kura の型に依存しない）
- Rust の edition 2024・stable。OS に依存しない（Linux でも `cargo test` が通る）
- 本番の依存は `libm = "=0.2.16"`（`default-features = false`）だけ。ほかを足すときは理由を書いて承認を得る
- resvg・usvg・tiny-skia などのコードを写さない。W3C の SVG 1.1・SVG 2・CSS・OpenType・Apple の TrueType の仕様
  （AAT）・Unicode の仕様から書く。resvg は dev 依存の照合の相手として使う（`=0.48.1`。Kura と同じ）
- crates.io に公開しない（`publish = false`。ADR-11）。ライセンスは MIT OR Apache-2.0（ADR-12）
- 「Kura が今 resvg で読めているもの」を v1 で落とさない（§3 NFR-10）

### 1.3 用語

| 用語 | 意味 |
|---|---|
| 読む（parse） | バイト列から文書（`Document`）を作る段。XML・CSS・参照の解決・大きさまで |
| 木を作る（build） | 文書から描く木（`Tree`）を作る段。use・marker の展開・塗りの解決・文字の組版と輪郭まで |
| 描く（render） | 木から画素を作る段 |
| 形 | Kura の言葉で、値を描かずに大きさと受け付けるかを読むこと（今の Kura は木を作り、描く前の資源の見積もりまで通す）。この描画器では `parse` → `estimate_build` → `build` → `Tree::estimate`（architecture.md §3.1）。大きさだけなら `parse` → `Document::pixel_size`（大きさを中身で決める文書は木が要る。ADR-58 (1)） |
| 文字・字形 | 文字は符号位置（Unicode の code point）、字形はフォントのグリフ |
| 手間 | 決めた数え方の仕事の量（§2.10）。時刻でなく手間で断る・中止を見る |
| 見積もり | 段に入る前に返す、その段の手間と確保の上限 |
| 上限（`Limits`） | 呼び手が渡す、断る基準（値は変えない） |
| 報告（`Report`） | 使ったフォント・見つからなかった文字・使った画像のデコーダ・読まなかった参照・警告 |
| フォントの集まり | 呼び手が渡すフォント（`FontSet`） |
| 照合の集まり | 見た目を resvg・Chrome と比べる試験の SVG（`tests/svg/`・`corpus/`） |
| ユーザー単位 | SVG の座標の単位。96 dpi で 1 ユーザー単位＝1 画素 |

---

## 2. 機能要件

### 2.1 入力（FR-IN）

| ID | 優先 | 要件 |
|---|---|---|
| FR-IN-01 | M | 入力は SVG のバイト列。UTF-8 だけを読む（先頭の BOM は飛ばす）。読めるかは XML の宣言の `encoding` に依らず、UTF-8 として正しいかで決める（ASCII だけの ISO-8859-1 の宣言のものなどは読む） |
| FR-IN-02 | M | UTF-8 として読めないときだけ、BOM と宣言を見て誤りの種類を分ける：UTF-16 の BOM があるもの・宣言が UTF-8 以外を名乗るものは `Unsupported`（Kura が次の段へ回せる。今の Kura は読み取りの誤り）、それ以外は `Invalid` |
| FR-IN-03 | M | svgz（gzip。先頭が `1f 8b`）は `Unsupported`（今の Kura と同じ。展開を自作するかは v1 の外） |
| FR-IN-04 | M | XML 1.0 の整形式でなければ `Invalid`（入力の位置つき）。閉じていない要素・属性の重複・不正な名前・不正な文字の参照など。roxmltree（resvg の中）が受けて描いているものを `Invalid` にしないかを、M1 で照合の集まりと Kura のファズのコーパスで照らし、受けるものは受ける（NFR-10）。細かい扱いは roxmltree 0.21.1 に合わせる（ADR-65） |
| FR-IN-05 | M | 名前空間：接頭辞は宣言で URI に解いて見る（接頭辞の文字では見ない）。描くのは SVG の名前空間の要素だけ。`href` と `xlink:href` の両方を読み、両方あれば `href` を使う |
| FR-IN-06 | M | 文字の参照（`&#…;`・`&#x…;`）と定義済みの 5 つの実体。値が u32 に入らないもの・文書の文字でないもの（`&#0;`・`&#xFFFE;` など）は `Invalid`、サロゲートと U+10FFFF を超える値は U+FFFD（roxmltree と同じ。ADR-65） |
| FR-IN-07 | M | DTD：内部のサブセットの一般の実体だけを、展開した量に上限を付けて扱う。パラメタ実体・外部の実体・マークアップか参照を含む実体は `Unsupported`（今の Kura と同じ）。外部の DTD の識別子（`SYSTEM`・`PUBLIC`）は読まずに無視する（Illustrator の DOCTYPE を読める）。ELEMENT・NOTATION の宣言は読み飛ばす。宣言の外の文字と知らない宣言は `Invalid`（ADR-65）。ATTLIST の既定の属性値は、要素が書いていない属性に当て、CDATA でない型の値は空白を揃える（XML 1.0 §3.3・§5.1。読めない ATTLIST の宣言は当てずに受ける。ADR-66 (7)・ADR-67 (7)） |
| FR-IN-08 | M | 外部の資源（ファイル・http・相対のパス・`@import`・`@font-face` の `src`）を読まない。外を指す参照（use・塗り・filter など）は描かずに、報告の「読まなかった参照」に載せる。`<image>` は FR-IMG |
| FR-IN-09 | M | 外から来たバイト列を `&str` の位置で切らない。名前・値の走査はバイト列で行い、文字の節・属性の値は UTF-8 として確かめてから使う |

### 2.2 構造とスタイル（FR-ST）

| ID | 優先 | 要件 |
|---|---|---|
| FR-ST-01 | M | `svg`：外側の `width`・`height`・`viewBox`・`preserveAspectRatio`。入れ子の `svg` は `x`・`y`・`width`・`height`・`viewBox`・`preserveAspectRatio` と表示の領域での切り取り（`overflow`） |
| FR-ST-02 | M | `g`・`defs`・`a`（`g` と同じに描く）・`title`・`desc`・`metadata`（描かない）。知らない要素は子ごと描かない（警告） |
| FR-ST-03 | M | `use`：要素・`symbol`・`svg` への参照、`x`・`y`（平行移動）、`symbol`・`svg` への `width`・`height`。参照先の中身は use の文脈で継承する。輪は FR-CO-05。外の文書への参照は FR-IN-08 |
| FR-ST-04 | M | `symbol`：`viewBox`・`preserveAspectRatio`・`refX`・`refY`、SVG 2 の `x`・`y`・`width`・`height` |
| FR-ST-05 | M | `switch`：`systemLanguage`（呼び手が渡す言語の並びで照らす。言語の頭で照らし、`en-US` は `en` で通る。空は偽）・`requiredExtensions`（属性がなければ真、空と知らない拡張は偽）で、通る最初の子だけを描く。`requiredFeatures` は無視する（SVG 2 で廃止。Chrome と同じ。ADR-66 (6)） |
| FR-ST-06 | M | `style` 要素（SVG の名前空間のもので、`type` がないか空か `text/css`。`media` 属性は `@media` と同じに照らす）と `style` 属性。CDATA の中も読む（ADR-66 (3)） |
| FR-ST-07 | M | CSS のセレクタ（ADR-66 (1)）：Selectors 3 の全部（型（大文字小文字を区別する）・`*`・`#id`・`.class`・属性（`[a]`・`=`・`~=`・`\|=`・`^=`・`$=`・`*=`。名前空間のない属性だけに当たる）・構造の擬似クラス（`:first-child`・`:last-child`・`:only-child`・`:nth-child()`・`:nth-last-child()`・`*-of-type`・`:root`・`:empty`）・`:not()`、結合子の子孫・`>`・`+`・`~`）と、Selectors 4 の `:is()`・`:where()`・`:not()`（複雑なセレクタの並び。ADR-73 (7)）・`:lang()`・属性の印 `i`・`s`、名前空間のセレクタ（`@namespace`）。`:link`・`:any-link` は href のある `a`。動く擬似クラスと擬似要素は当てない。`:has()`・`:nth-child(… of S)` は確かめて読むが当てず、それを含む選択子を捨てる（警告。ADR-73 (4)）。詳細度と出てきた順で決める。並びの中の壊れたセレクタは規則ごと捨てる（CSS の仕様のとおり）。CSS の escape を解く |
| FR-ST-08 | M | アットルール（ADR-66 (2)）：`@media` は照らす（`screen`・`all` は真、`print` などは偽。`prefers-color-scheme` は呼び手の設定で既定は light。大きさの条件は文書の大きさ（FR-OUT-01 で width・height・viewBox から決まるもの）で照らし、中身で大きさが決まる文書では偽）。`@supports` は読めるプロパティと値か・`selector()` で照らす。`@layer` は層の順でカスケードする。`@namespace` を読む。`@import`（外を読まない）・`@font-face`・`@charset`・`@keyframes`・`@page`・知らないものは飛ばし（警告）、そのあとの規則は当てる。`<!--`・`-->` は飛ばす |
| FR-ST-09 | M | カスケードの順（弱い順。CSS Cascade のとおり。ADR-66 (3)）：プレゼンテーション属性 ＜ `style` 要素の規則（層・詳細度・順） ＜ `style` 属性 ＜ 規則の `!important`（層は逆の順） ＜ `style` 属性の `!important`。プロパティの名前と `!important` は大文字小文字を区別しない。`<style>` は文書のどこにあってもすべての要素に当てる。プレゼンテーション属性の一覧は SVG 2 のもの（SVG 1.1 のものと、`transform-origin`・`paint-order`・`isolation`・`mask-type`・`vector-effect`、幾何の `x`・`y`・`width`・`height`・`r`・`rx`・`ry`・`cx`・`cy`・`d`）。幾何のプロパティは CSS でも読む（効くのは M2 から）。`mix-blend-mode` は CSS のプロパティとしてだけ読み、属性としては読まない（Chrome と同じ）。`marker` のまとめのプロパティを読む（ADR-66 (4)） |
| FR-ST-10 | M | 継承・`inherit`・`currentColor`（`color` の継承）。em・ex は計算した `font-size` から |
| FR-ST-11 | M | `transform` 属性（SVG 1.1 の書き方だけ：`matrix`・`translate`・`scale`・`rotate`・`skewX`・`skewY`、名前は大文字小文字を区別、単位のない数、区切りのない数 `10-5`・`.5.5` を正しく区切る、名前と `(` の間の空白を受ける。CSS Transforms 1 §7.2・Chrome と同じ。ADR-74）と `transform-origin` 属性。CSS の `transform` プロパティ（CSS Transforms の単位つきの 2 次元の関数。長さはどの単位も受ける。3 次元の関数は読まない）と `transform-box` も読む（ADR-66 (4)）。`transform-origin` の既定は UA の規則の `0 0` で、根の svg だけは表示の領域の 50% 50%（参照の箱は表示の領域の箱。Chrome と同じ。ADR-85 (1)） |
| FR-ST-12 | M | `display: none`・`visibility`（`hidden`・`collapse`。子の `visible` は描く）・`opacity` |
| FR-ST-13 | M | 同じ `id` が複数あれば、文書の順で最初の要素を指す（DOM・Chrome と同じ。ADR-66 (5)） |
| FR-ST-14 | M | `tref`（SVG 1.1。usvg が解くので受ける） |
| FR-ST-15 | M | 数：字句を自前で切り出し、`f64` に読む。有限でない数（`1e999` など）は壊れた値として、その属性・宣言を無視して警告する（CSS・SVG の誤りの扱い）。数の大きさの上限は持たない（ADR-32。安全は幾何の側で守る。architecture.md §6.1） |

### 2.3 図形（FR-SH）

| ID | 優先 | 要件 |
|---|---|---|
| FR-SH-01 | M | `path`：パスのデータの文法すべて（暗黙の命令の繰り返し・区切りのない数・弧の旗の詰め書き）。途中で壊れていれば、そこまでを描く（SVG の誤りの扱い）。弧は SVG 1.1 の付録 F の式で中心の形に直し、半径が足りなければ広げる |
| FR-SH-02 | M | `rect`（`rx`・`ry` の `auto` と半分への切り詰め）・`circle`・`ellipse`（SVG 2 の `auto`）・`line`・`polyline`・`polygon`（数が奇数なら最後を捨てる）。大きさが 0 か負なら描かない |
| FR-SH-03 | M | `pathLength`（破線と textPath の `startOffset` の割合に効く。Chrome と同じ。resvg は読まない。ADR-91 (2)。non-scaling-stroke との組み合わせと `pathLength="0"`（実線）は ADR-95） |
| FR-SH-04 | M | 長さの単位：数・px・in・cm・mm・Q・pt・pc・em・ex（em の半分）・ch（em の半分）・ic（1em）・cap（0.7em）・lh（`line-height` の計算した値）・rlh（根の要素の `line-height`）・rem・%（向きに応じた表示の領域の幅・高さ・対角の √2 で割ったもの）・vw・vh・vmin・vmax・vi・vb（横書きなので vi は vw、vb は vh。`@media` と同じ文書の大きさから。中身で大きさが決まる文書では壊れた値）。ch・ic・cap は読む段にフォントがないので CSS Values 4 の代わりの値（ADR-74）。単位の大文字小文字は区別しない（ADR-66 (4)）。CSS の宣言では、単位のない長さは SVG のプロパティ（x・y・r・rx・ry・cx・cy・stroke-width・stroke-dashoffset・baseline-shift・stroke-dasharray）だけが受ける（属性はどれも受ける。Chrome と同じ。ADR-74）。`calc()`・`var()` は M1 では読まずに壊れた値として警告する |

### 2.4 塗りと線（FR-PT）

| ID | 優先 | 要件 |
|---|---|---|
| FR-PT-01 | M | 塗り：`none`・色・`url(#id)`（あとに代わりの色を付けられる）・`currentColor`・`context-fill`・`context-stroke`（marker の中と use の中身。SVG 2。use の中身は最も近い use の塗りと線。marker の中から使われる pattern・mask・clipPath・filter の中身と、marker と use の外には文脈の要素がなく、塗らない。Chrome と同じ。ADR-87 (4)・ADR-93 (2)） |
| FR-PT-02 | M | 色：SVG の 147 の名前・`#rgb`・`#rgba`・`#rrggbb`・`#rrggbbaa`・`rgb()`・`rgba()`・`hsl()`・`hsla()`・`transparent`。CSS Color 4 の `/` の α と `hwb()` も読む。システムの色は読まない。`lab()`・`oklch()` などは M1 では読まずに警告し、照らしてから決める（ADR-66 (4)） |
| FR-PT-03 | M | `fill-rule`・`fill-opacity`・`stroke-opacity`・`stroke-width`・`stroke-linecap`・`stroke-linejoin`（`miter`・`round`・`bevel`。SVG 2 の `miter-clip`・`arcs` は Chrome と同じく壊れた値。ADR-91 (1)）・`stroke-miterlimit`（負だけが壊れた値。1 未満は継ぎ目がいつも bevel。ADR-76）・`stroke-dasharray`（奇数個は 2 度並べる・% ・負や合計 0 は実線）・`stroke-dashoffset` |
| FR-PT-04 | M | `paint-order`・`vector-effect: non-scaling-stroke`・`shape-rendering`（`crispEdges` は反エイリアスなし。`optimizeSpeed` は resvg と同じ扱い。M2 で照らす） |
| FR-PT-04b | M | 描く座標系の太さが 1 画素以下の反エイリアスの線は、描く座標系の 1 画素の線を太さ（2 つの軸の太さの平均）の α で描く（Chrome・resvg の細い線と同じ。ADR-97 (2)） |
| FR-PT-04c | M | 曲がりの半径より太い線は、曲線の縁を曲線のまま写したものとして描く（円の中に半径「太さの半分 − 半径」の穴が残る。多角形の頂点は頂点を通す。resvg・Chrome と同じ。ADR-99 (2)） |
| FR-PT-05 | M | `linearGradient`・`radialGradient`：`gradientUnits`・`gradientTransform`・`spreadMethod`（pad・reflect・repeat）・`href` の鎖での属性と stop の受け継ぎ（輪は FR-CO-05）・焦点（`fx`・`fy`・SVG 2 の `fr`）・stop の offset の切り詰めと単調化・`stop-color`・`stop-opacity`。stop が 1 つなら単色、0 なら塗らない。stop が多くても・同じ offset が多くても、画素ごとの仕事は stop の数に比例しない（Kura の S2・S3） |
| FR-PT-06 | M | `pattern`：`patternUnits`・`patternContentUnits`・`patternTransform`・`viewBox`・`preserveAspectRatio`・`x`・`y`・`width`・`height`・`href` の鎖での受け継ぎ・入れ子の pattern |
| FR-PT-07 | M | `marker`：`marker-start`・`marker-mid`・`marker-end` と CSS の `marker`、`orient`（角度・`auto`・`auto-start-reverse`）・`markerUnits`・`refX`・`refY`・`viewBox`・`preserveAspectRatio`・`markerWidth`・`markerHeight`・切り取り。付ける図形は SVG 2・resvg と同じ図形のすべて（`path`・`line`・`polyline`・`polygon`・`rect`・`circle`・`ellipse`。Chrome は前の 4 つだけ。ADR-87 (3)）。marker の中の marker は描かない |

### 2.5 合成（FR-CO）

| ID | 優先 | 要件 |
|---|---|---|
| FR-CO-01 | M | グループの `opacity`（層に描いてから掛ける） |
| FR-CO-02 | M | `clipPath`：`clipPathUnits`・`clip-rule`・clipPath と子の `transform`・clipPath への `clip-path`・中の `use`・`text`。中の塗り・線・不透明度は見ない |
| FR-CO-03 | M | `mask`：`maskUnits`・`maskContentUnits`・`x`・`y`・`width`・`height`・`mask-type`（`luminance`・`alpha`。SVG 2）・mask への `mask` |
| FR-CO-04 | M | `mix-blend-mode`（16 種）・`isolation` |
| FR-CO-05 | M | 参照の輪（clip・mask・pattern・filter・marker・use・gradient の href・祖先から受け継いだ塗りで自分を描き直す pattern）は、保守的に作った参照の辺（architecture.md §4.3）の上で明示の積み上げで見つけ、resvg と同じ扱いにする（usvg は輪になる参照を取り除いて残りを描く。M1 で照らして決め、警告を出す）。保守的な辺で取り除いた参照のうち use の文脈では輪でないものは、M2 が先を 1 段だけ描き直す（ADR-78 (8)・ADR-86 (1)） |
| FR-CO-06 | M | 今の Kura は輪を前段で `Unsupported` にしているが、この描画器は受ける側に倒す（ADR-24） |
| FR-CO-07 | M | 参照先がない・種類が違う参照は、仕様の誤りの扱い（塗りなら代わりの色、clip・mask・filter なら resvg と同じ扱い。M3 で照らす） |

### 2.6 フィルタ（FR-FI）

| ID | 優先 | 要件 |
|---|---|---|
| FR-FI-01 | M | プリミティブ：feGaussianBlur・feOffset・feFlood・feBlend・feComposite・feMerge・feColorMatrix・feComponentTransfer・feMorphology・feConvolveMatrix・feTurbulence・feDisplacementMap・feTile・feImage（要素への参照）・feDropShadow・feDiffuseLighting・feSpecularLighting（feDistantLight・fePointLight・feSpotLight、`lighting-color`） |
| FR-FI-02 | M | filter の領域（`filterUnits`・既定の −10%・120%）・`primitiveUnits`・プリミティブの部分領域・`in`・`in2`・`result`（SourceGraphic・SourceAlpha。BackgroundImage・FillPaint・StrokePaint は resvg と同じ扱い） |
| FR-FI-03 | M | `color-interpolation-filters`（既定 linearRGB・sRGB・auto） |
| FR-FI-04 | M | `filter` の並び（複数の filter）と CSS の filter 関数（`blur()`・`drop-shadow()`・`grayscale()`・`sepia()`・`saturate()`・`hue-rotate()`・`invert()`・`opacity()`・`brightness()`・`contrast()`。resvg が受ける範囲） |
| FR-FI-05 | M | 壊れた filter（参照先がない・不正なプリミティブ）の扱いは resvg と同じにする（M4 で照らす） |
| FR-FI-06 | M | feTurbulence は仕様の参照の実装の擬似乱数と計算の順をそのまま使う（写すのは仕様の文書の式で、resvg のコードではない）。オクターブが多くても、手間は見積もりで抑える（Kura の S7） |

### 2.7 文字（FR-TX）

| ID | 優先 | 要件 |
|---|---|---|
| FR-TX-01 | M | `text`・`tspan`・`textPath`（`href`・`startOffset`・SVG 2 の `path` は resvg が受けるなら）・`tref`。textPath の弧長の精度は倍率に依らない量で決める（木を作る段は倍率を知らない。ADR-43） |
| FR-TX-02 | M | `x`・`y`・`dx`・`dy`・`rotate` の並び（SVG 2 の addressable character ごと。符号位置ごとで、畳んだ空白は数えない。resvg の数え方と M5 で照らす）・text の塊（chunk）ごとの `text-anchor` |
| FR-TX-03 | M | `font-family`・`font-size`・`font-weight`・`font-style`・`font-stretch`・`font` の一括の書き方・`font-variant`（`small-caps` は resvg と同じ扱い）・`font-kerning` |
| FR-TX-04 | M | `letter-spacing`・`word-spacing`・`textLength`・`lengthAdjust`（`spacing`・`spacingAndGlyphs`） |
| FR-TX-05 | M | `text-decoration`（下線・上線・取り消し線。線の位置と太さはフォントの post・OS/2 から。塗りは装飾を付けた要素のもの） |
| FR-TX-06 | M | `dominant-baseline`・`alignment-baseline`・`baseline-shift`（resvg が受ける範囲） |
| FR-TX-07 | M | 空白の扱い：`xml:space`・SVG 2 の `white-space`（resvg が受ける範囲） |
| FR-TX-08 | M | 文字の塗りと線は他の図形と同じ（グラデーションの外接矩形は text 全体）。`paint-order` も効く |
| FR-TX-09 | M | 和文の横書き（かな・漢字・全角の約物。異体字セレクタ（cmap の形式 14）を含む）（M5） |
| FR-TX-10 | M | ラテン文字・ギリシャ文字・キリル文字（M5）：OpenType の書体ではカーニング（GPOS のペアと OpenType の `kern`）・合字（GSUB の `liga`・`clig`）・文脈の置き換え（`calt`・`ccmp`・`locl`）・結合の記号の位置（GPOS の mark・mkmk）。AAT だけの書体（Times・Helvetica・Courier・Apple Chancery など。macOS のシステムのフォント 787 書体のうち 165）では `morx` の非文脈・合字・文脈の副表・`kerx`（`ankr`）・Apple の形の `kern`・`trak`（ADR-29）。組版の方式の選び方と `trak` の当て方は HarfBuzz・harfrust と同じにする（architecture.md §7.1。ADR-36） |
| FR-TX-11 | M | 双方向（UAX #9）・ヘブライ文字・アラビア文字（接続の形 `init`・`medi`・`fina`・`isol`・`rlig`、AAT の書体（GeezaPro・Raanana など）では `morx` の並べ替え・挿入を含むすべての副表）。Kura は今 resvg（harfrust）でこれらを描けている（M6。ADR-14・ADR-29） |
| FR-TX-12 | M | 縦書き（`writing-mode` の `tb`・`tb-rl`・`vertical-rl`・`vertical-lr`。`vert`・`vrt2`・vmtx・UAX #50 の向き。resvg が受ける値に合わせる）（M6。重ければ v1.x へ回すことを相談する。ADR-14） |
| FR-TX-13 | C | インド系の文字の並べ替え・カラーの絵文字（sbix・CBDT・COLR）（ADR-14。sbix は画像のデコードが要るので、あとで ADR-16 の口を使える） |
| FR-TX-14 | S | 可変フォント（fvar・gvar・avar。`font-weight`・`font-stretch` を `wght`・`wdth` に当てる）。macOS の SF の書体が可変。M5 の前に resvg 0.48.1 の扱いを確かめ、resvg が軸を動かしているなら M6 で M にする。既定の実体だけなら v1 の外（ADR-14） |
| FR-TX-15 | M | フォントの形式：TrueType（glyf）・OpenType CFF・TTC。cmap は形式 0・4・6・12・13・14。WOFF・WOFF2・CFF2・ビットマップだけのフォントは v1 の外 |
| FR-TX-16 | M | `text-rendering`（`optimizeSpeed` で字形の反エイリアスを切るかなど）・`direction`・`unicode-bidi` は resvg と同じ扱いにする（M5・M6 で照らす） |

### 2.8 フォント（FR-FO）

| ID | 優先 | 要件 |
|---|---|---|
| FR-FO-01 | M | ライブラリはファイルを開かず、フォントを探さない。呼び手がフォントのバイト列の集まり（`FontSet`）を渡す。バイト列は `Arc` で共有して受け、写さない（Kura は mmap したものを渡す） |
| FR-FO-02 | M | フォントは信用しない入力として読む（範囲の外を読まない・表の大きさで確保しない）。字形ごとの上限（CFF の字形の命令は仕様の上限（積み上げ 48・副手続きの入れ子 10）と手間の上限・複合の glyf の入れ子と部品の数・字形ごとの点の数）と、組版の上限（区切りの中の文字の数に比例する、GSUB・GPOS の当てはめの回数・文脈の照合の歩数・`morx`・`kerx` の状態機械の歩数と、入れ子の lookup の深さ）は、版に入る定数にする（`Limits` に入れない）。字形ごとの上限を超えた字形・壊れた字形は輪郭を空にして警告する。組版の上限を超えたら、それ以上当てはめずに続ける（輪郭は空にしない）。どちらも文書は断らない（ADR-34）。壊れた表は無いものとして警告。`FontSet::new` で壊れた書体は飛ばして報告に数える |
| FR-FO-03 | M | 選び方：`font-family` の並びを前から、CSS Fonts 3 の照合（幅 → 傾き → 太さ）で選ぶ。書体名は name 表の ID 1・16 のすべての言語（「ヒラギノ角ゴシック」でも当たる）。書体名は name 表の Windows の名前（UTF-16BE）と Mac の名前（Mac Roman）を読み、ASCII の大文字小文字だけを無視して比べる（fontdb の照らし方と M5 で照らす） |
| FR-FO-04 | M | 総称の書体（`serif`・`sans-serif`・`cursive`・`fantasy`・`monospace`）の割り当ては呼び手が渡す（Kura は Times・Helvetica・Apple Chancery・Papyrus・Courier）。既定は Times New Roman・Arial・Comic Sans MS・Impact・Courier New |
| FR-FO-05 | M | 代わりのフォント：選んだ書体にない文字は、呼び手が渡す代わりの書体の並び（例：Hiragino Sans）→ `FontSet` の順、でその文字を持つ最初の書体を使う。絵文字の書体（Apple Color Emoji は sbix と、ほぼ仮の輪郭だけの glyf の両方を持つ）は、その文字の sbix の絵があれば、その文字を持たないと数える（字ごとに数える。ADR-40・ADR-56 (2)）。glyf も CFF も持たない書体（sbix だけ・ビットマップだけ）は字を持つと数えない（ADR-43）。見つからなければ報告の「見つからなかった文字」に載せる（描き方は M5 で resvg・Chrome と照らして決める） |
| FR-FO-06 | M | 報告に、使ったフォント（呼び手の鍵と書体の番号）を並べる。鍵は呼び手が `FontSource` に 32 バイトで付ける（Kura はファイルの BLAKE3）。ライブラリはハッシュを持たない（ADR-23） |
| FR-FO-07 | M | `FontSet` の索引（書体名・太さ・幅・傾き・輪郭の表の有無）は `FontSet::new` で作る（手間と確保を `FontLimits` で数え、中止の印を見る）。文書ごとの手間は索引の大きさから決まる式で数え、内部の覚え（キャッシュ）の有無で見積もり・断るかどうかが変わらない（D2） |

### 2.9 画像（FR-IMG）

| ID | 優先 | 要件 |
|---|---|---|
| FR-IMG-01 | M | 呼び手がデコーダを渡さなければ、描く木に入りうる `<image>`（外部・`data:` のラスター・入れ子の SVG。pattern・mask・feImage の中も）は `Unsupported`（今の Kura と同じ。Kura は次の段のデコーダへ回す）。参照されない `defs`・`display: none` の部分木・通らない `switch` の枝の中の `<image>` は数えない（今の Kura は usvg が実際に解いた `<image>` だけで断る。architecture.md §3.1。M1 で Kura の一覧と照らす） |
| FR-IMG-02 | M | 呼び手がデコーダを渡したとき：`data:` のラスターの画像は、そのデコーダで読み、`preserveAspectRatio`・`image-rendering` に従って決定的に拡大・縮小して描く。報告に呼び手のデコーダの識別子を入れる。外を指す `<image>` は描かずに「読まなかった参照」に載せる。`data:image/svg+xml` は `Unsupported`（FR-IMG-03 まで）（M3。ADR-16） |
| FR-IMG-03 | C | `data:image/svg+xml` の入れ子の SVG を、上限と手間を親と分け合って自前で描く |

### 2.10 上限・見積もり・中止（FR-LIM）

| ID | 優先 | 要件 |
|---|---|---|
| FR-LIM-01 | M | 上限（`Limits`・`RenderLimits`・`FontLimits`。architecture.md §3）を呼び手が渡す。段に入る前に「見積もり ≤ 上限」を確かめ、超えれば確保・仕事の前に `Limit`（何が・いくつ要り・上限はいくつか）を返す。段の中の勘定の上限は見積もりの値で、それを越えるのは見積もりの不具合（本番では `Limit`、試験では落とす）。木を作る段の上限は、`parse` に渡した `Limits` を `Document` が持つ |
| FR-LIM-02 | M | 見積もりを返す口が 3 つある：読む前（`estimate_parse`。読む段と同じ道（XML の節を作り、規則を読み、カスケードする）を `parse` と同じ上限で走らせて量を測る。見積もりは実際とぴったり同じ。ADR-70）・木を作る前（`Document::estimate_build`）・計画と描く前（`Tree::estimate` は計画の分と全体を描く分を分けて返す・`Plan::estimate_region`）。どれも値を変えない決定的な関数で、手間・確保のピーク・持ち続ける量を返す。見積もり自身の手間と確保も上限で数え、中止の印を見る |
| FR-LIM-03 | M | 見積もりは、自分の実装が実際に確保する大きさ・回る数の上限にする（キャンバスに比べた相対でなく絶対の量）。数えた手間・確保の量が見積もりを超えないことを、照合の集まりのすべてで確かめる |
| FR-LIM-04 | M | 手間の単位は、どの段でも M4 Max の release で 1 単位 ≈ 1 ns になるように係数を決める（Kura の見積もりと同じ目安）。M7 で校正して記録する |
| FR-LIM-05 | M | 中止：呼び手が渡す中止の印（`Cancel`）を、決まった手間（`CANCEL_INTERVAL`、2¹⁶ 単位）ごとに確かめ、立っていれば `Cancelled` を返す。読む前の見積もり・読む・木を作る・計画・描く・`FontSet::new` のどの段のどのループも、印が立ってから 2 × `CANCEL_INTERVAL` 単位を超えて回らない。呼び手が注入した画像のデコーダの中は勘定の外（呼び手の責め） |
| FR-LIM-06 | M | `Limit`・`Cancelled` のとき、出力の画素の中身は決めない（呼び手は使わない）。`Tree`・`Document` は作られない |

**上限の既定値**（M7 で校正する。Kura は自分の値を渡す）

| 項目 | 既定 | 見る段 | 由来・注 |
|---|---|---|---|
| 入力のバイト数 | 16 MiB | 読む前 | Kura の `MAX_SOURCE` |
| 実体を展開したあとのバイト数 | 64 MiB | 読む前（読む段の道を走らせて測る。ADR-70）・読む | `MAX_EXPANDED` |
| XML の節の数 | 2²¹ | 読む前・読む | `MAX_ELEMENTS`（roxmltree の `nodes_limit`） |
| 入れ子の深さ | 1,024 | 読む前・読む | Kura は 64（usvg の再帰のため）。再帰しないので広げる。深さに比べた手間は数える |
| 展開したあとの要素の数（use・marker・複製） | 2²¹ | 木を作る前（`estimate_build`。`Resource::Instances`） | `MAX_ELEMENTS` |
| 描く節の数（pattern などの描き直しを含む） | 2²² | 描く前 | `MAX_RENDER_NODES` |
| キャンバスの 1 辺 | 32,768 画素 | 描く前 | Kura の `max_dim` |
| キャンバスの画素の数 | 2²⁸ | 描く前 | Kura の `max_pixels` |
| 同時に生きる一時の画像のバイト数 | 1 GiB | 描く前 | Kura は `max_value_bytes`（4 GiB）を渡す |
| 見積もりの手間・確保 | 2²⁸・256 MiB | 各見積もり | 見積もり自身の仕事（`Limits` と `RenderLimits` の両方にある）。`estimate_parse` は読む段と同じ道を走らせるので、読む段の上限（`parse_work`・`parse_bytes`）で払い、これは使わない（ADR-70） |
| 読む段の確保のピーク | 1 GiB | 読む前・読む | |
| 読む手間 | 2³¹ | 読む前・読む | |
| 木を作る手間（文字の組版を含む） | 2³¹ | 木を作る前 | Kura の文字は 1 件 10⁸。自作の組版は字の数の 2 乗の手間を持たない |
| 木を作る確保のピーク | 4 GiB | 木を作る前 | |
| 計画の手間・`Plan` が持ち続ける量 | 2³²・1 GiB | 描く前 | 平坦化した点・線の外形・層の計画 |
| 描く手間 | 2³³ | 描く前 | Kura は max(2³², 64 × 画素) |
| フォントの集まりの書体の数・手間・索引のバイト数 | 4,096・2³⁰・64 MiB | `FontSet::new` | Kura のシステムのフォントは 370 ファイル・787 書体 |

数の絶対値の上限（Kura の `MAX_NUMBER` 10⁹）は持たない（FR-ST-15・ADR-32）。

### 2.11 出力（FR-OUT）

| ID | 優先 | 要件 |
|---|---|---|
| FR-OUT-01 | M | 大きさ：外側の svg の `width`・`height`（単位つき、96 dpi）。ない・% なら `viewBox` から。どちらもなければ、usvg と同じく中身の外接矩形の右下を大きさにする（usvg 0.48.1 の振る舞い。100 × 100 ではない。ADR-58 (1)）。その文書の大きさは木を作るまで決まらない（`Document::size` は `None` を返し、大きさは `Tree` から読む。ADR-63 (3)）。resvg と照らして決めた規則（ADR-64 (8)）：viewBox がなく、幅か高さが欠ける・%・壊れた値なら、幅と高さの両方を中身の外接矩形の右下にする（片方だけある値も使わない）。外接矩形は線の太さを含まず、`visibility: hidden` は含め、`display: none` は含めない。移動の点は、続く移動に置き換わるものと描く区間のないパスのものを含めない（末尾の移動は含める。ADR-98 (8)）。中身がなければ 100 × 100。この文書の % の長さは、書いた幅・高さ（絶対の値。% は 100 に対する割合）、書いていないか壊れた側は 100 の表示の領域で解く（usvg・Chrome と同じ。ADR-102 (2) が ADR-99 (1) の 100 × 100 を置き換える）。viewBox があれば、% は viewBox の大きさに対する割合、片方だけなら viewBox の縦横の比で補う。幅か高さが 0 か負なら `Invalid`。壊れた viewBox（幅か高さが 0 以下）は大きさを決めるときは無いものとする（描く段では、幅か高さが 0 の viewBox の要素を描かない。SVG 2 §8.2。ADR-78 (7)）。画素は倍率を掛けたものを f32 にしてから四捨五入する（0.5 は遠い側。最小 1。resvg の `to_int_size` と同じ。10.5 は 11・0.3 は 1・233.33 は 233。ADR-78 (5)） |
| FR-OUT-02 | M | 大きさは、照合の集まりのすべての入力で resvg 0.48.1 と同じ画素になる（Kura の編集の文書は画素の座標を持つので、大きさが変わると壊れる）。resvg の 32 ビットの丸めに合わせる必要があれば、大きさの計算だけ合わせる（M1 で確かめる） |
| FR-OUT-03 | M | 倍率（正の有限の数）。描く座標系は「ユーザー単位 × 倍率」。倍率・出力のバッファの大きさと行のバイト数・範囲が正しくなければ `Invalid`（理由は呼び手の誤り。位置なし） |
| FR-OUT-04 | M | 出力は呼び手が渡す RGBA8 の乗算済みのバッファ（幅・高さ・行のバイト数）。描く前に全体を透明にする。色は sRGB のまま（色の管理をしない。ADR-17） |
| FR-OUT-04b | M | 根の svg の CSS の `background-color`（属性でなくプロパティ。`currentColor` は根の `color`）で、描く前にキャンバスの全体を塗る（resvg・Chrome と同じ。ADR-97 (4)）。`background` のまとめは最後の層の色を `background-color` として読む（ADR-99 (4)）。画像の層は、外の url の画像は描かず（Chrome も img では読まない）、グラデーション・`data:` の画像・`image()` の層は `NotYet`（ADR-102 (6)・ADR-103）。根の `display: none` と偽の条件でも塗る（ADR-102 (1)） |
| FR-OUT-05 | M | 範囲の描画（`render_region`・`estimate_region`）：全体を描いて同じ範囲を切り出したものとビット単位で同じ（ADR-13） |
| FR-OUT-06 | M | 形（§1.3）を通ったものは、同じ上限・同じ資源・同じ倍率なら、計画と描く段で `Limit`・`Unsupported` にならない（確保の失敗と中止を除く）。どの誤りをどの段で決めるかは architecture.md §3.1（ADR-37）。範囲が全体に含まれるなら、範囲の見積もりは全体の描画の見積もりを超えない |

### 2.12 報告と誤り（FR-REP）

| ID | 優先 | 要件 |
|---|---|---|
| FR-REP-01 | M | 誤りは 4 種：`Limit`（上限）・`Cancelled`（中止）・`Unsupported`（この描画器では描かない。Kura は次の段へ回す）・`Invalid`（壊れた入力。位置つき） |
| FR-REP-02 | M | `Unsupported` にするもの：FR-IN-02・03・07、FR-IMG-01・02 の `data:image/svg+xml`、文字があるのにフォントの集まりが空のもの。そのほかの対応しない要素・属性・値は、描かずに警告にする（今の Kura で resvg が黙って飛ばしているものを、断るようにしない）（ADR-24）。Kura の今と誤りの種類が変わる所は architecture.md §11 |
| FR-REP-03 | M | 報告：使ったフォント（鍵・書体の番号）・見つからなかった文字（文字と数）・使った画像のデコーダの識別子・読まなかった参照（href と要素の位置）・警告（種類と位置）。並びは決まった順（D1・D2） |
| FR-REP-04 | M | 警告の種類は列挙の型で返す。文言は持たない（呼び手が訳す） |

---

## 3. 非機能要件

| ID | 要件（M4 Max、release、1 スレッド） |
|---|---|
| NFR-01 | 決定性：determinism.md の D1（値）と D2（勘定）。対象のターゲットで、D1 は版ごと、D2 は rev ごとにビット単位で同じ |
| NFR-02 | 安全：どんな入力（SVG・フォントの両方）でも panic・スタックのあふれ・終わらないループ・上限を超える確保をしない。`unsafe` は `forbid` |
| NFR-03 | スタック：使う量が入力に依らず一定。上限を広げた設定で、debug と release の両方の 128 KiB のスタックのスレッドで、10 万段の入れ子・10 万段の参照の鎖・10 万個のセレクタの結合子を、読む・木を作る・描くの最後まで通して落ちない |
| NFR-04 | 確保：数えた確保のピーク ≤ 見積もり（必ず）。確保を数える確保器で測ったヒープの増え ≤ 数えた量 ＋ 小さな定数（必ず）。照合の集まりで 見積もり ≤ 2 × 実際 ＋ 1 MiB（目標）。（Kura の予算の判定は 実測 ≤ 予算 で、見積もり × 1.25 ＋ 4 MiB は Kura の `mem_smoke` の上限） |
| NFR-05 | 手間：数えた手間 ≤ 見積もり（必ず）。敵対的な入力の集まり（Kura の遅い入力 S1〜S5・S7・Kura の試験の攻撃の例・ファズで見つけた遅い入力）で、時間 ≤ 手間 × 2 ns（目標）。Kura の S6（区切りのない数の transform を持つ path へのふつうの textPath）は、かならず描く互換の例。照合の集まりで 手間の見積もり ≤ k × 実際（目標。k は M5・M7 で決める。見積もりが大きすぎて、ふつうの文書を断らないため） |
| NFR-06 | 中止の速さ：印が立ってから、2 × `CANCEL_INTERVAL` 単位の手間のうちに `Cancelled` を返す（目安 0.15 ms 以下） |
| NFR-07 | 性能：resvg 単体と同じ入力で、時間の比（この描画器 ÷ resvg）の幾何平均 ≤ 1.5、どの 1 件も 3 倍以内か resvg ＋ 10 ms 以内。Kura の重い例「地図風の線 30 万区間」は、`gen-heavy` の `map-lines`（architecture.md §10）で、同じ入力の resvg 単体の時間（M0 で 0.51 秒）以下。Kura の元の入力の引数は分からないので、Kura の記録の 4.5 秒（働き手を通した値の時間。入力が違う）は記録として並べるだけにする（ADR-58 (2)。もとの「4.5 秒以内・目標 2 秒」は、入力が分からないので約束にしない）。確保のピークは参考として記録する（ADR-19・ADR-35） |
| NFR-08 | 移植性：OS に依存しない。Linux（x86_64・aarch64）でも `cargo test` が通る。`cfg(target_*)` で処理を分けない |
| NFR-09 | 依存：本番は `libm` だけ（`cargo tree -e normal,build -p sumie` で確かめる）。libm の `arch` の feature が入っていないことを `cargo tree -e features -i libm` で確かめる |
| NFR-10 | 互換：照合の集まり（実例の SVG を含む。ADR-59）のうち、Kura の設定の resvg 0.48.1（`sumie-tools oracle resvg`）が描くものは、既定の上限で描ける（断らない）。Kura の今の `SvgCodec` は「Kura の前段の見積もり ＋ resvg 0.48.1」なので、Kura が描くものはこれに含まれ、Kura の側の一覧より厳しい側から確かめる（ADR-63 (1)。Kura の側の一覧の依頼は取り下げた）。外れるものは一覧にして承認を得る。Kura の手元の SVG（1,439 件）は消えていて使えない（ADR-58 (3)） |
| NFR-11 | 一致：resvg との差は ADR-15 の基準の内。数は M0 の分布で確定した（ADR-56 (1)・ADR-61）：割合の分母はキャンバスの全画素。図形と単色は「差が 8/255 を超える画素が 2% 以下」かつ「形の内側（両方の画像で 3 × 3 の近傍の画素がすべて同じ値の平らな所）の差が 2/255 以下」、フィルタ・グラデーションを含むものは「16/255 を超える画素が 1% 以下」。長い辺が 200 画素に満たない絵は、整数の倍率で 200 画素以上にして両方を描いて比べる。0.5% は目標として差の一覧に並べる。文字を含むもの（M5）・画像を含むもの（M3）・resvg の試験の未定義の振る舞いの例（題に (UB)）をどの基準で見るかは、そのマイルストーンで決める（ADR-56 (1) で外したのは M0 の resvg と Chrome の分布だけ）。Chrome は合否にせず、差の一覧に並べる |
| NFR-12 | ファズ：パーサーごと（XML・DTD・CSS・値・パス・フォントの各表・CFF・`morx`・`kerx`・組版・描画の全体）に cargo-fuzz の対象と種つきの変異のテストがある。v1 の前に、全部の対象の時間の合計で 24 時間以上（文書の全体を描く対象は、そのうち 8 時間以上）回し、見つかったものを直して試験に足す |

**NFR-10 の判定の範囲**：Kura の今の前段は usvg の都合で断りすぎている所がある（深さ 64・CSS の結合子 32・marker の中の
`style` の `url(#)`・`<style>` に「transform」の文字があれば textPath を断る、など）。新しい描画器はこれらを受ける。
Kura が断っていて新しい描画器が描くものは、差として記録するだけでよい。

---

## 4. 対象外（v1）

- アニメーション（SMIL）・スクリプト・操作への反応（`:hover` など）
- 外部の資源の読み込み（FR-IN-08）
- SVG の書き出し・編集の API
- `<image>` のラスター画像を自前で展開すること（呼び手のデコーダを注入する口は作る。ADR-16）・入れ子の SVG の `<image>`
- svgz・UTF-8 でない符号化
- 色の管理（ICC・`color-profile`）・CSS Color 4 の広い色域
- SVG フォント（`<font>`）・`@font-face`・WOFF・WOFF2・CFF2
- `foreignObject`（描かない。警告）
- 文字の折り返し（SVG 2 の `inline-size`・`shape-inside`）
- インド系の文字の並べ替え・カラーの絵文字・（M5 の前の確かめしだいで）可変フォント（ADR-14）

---

## 5. 受け入れの条件（v1 の全体）

| ID | 条件 |
|---|---|
| A-01 | 対象のすべてのターゲットで、ゴールデン（`goldens/render-v1.jsonl`）と D2 の照らし（`goldens/estimate.jsonl`）がすべて一致する |
| A-02 | NFR-10 の互換を、Kura の側で作った一覧で満たす（外れの一覧は承認済み） |
| A-03 | 大きさが照合の集まりのすべてで resvg と同じ（FR-OUT-02） |
| A-04 | resvg との差が ADR-15 の基準の内。外れは一つずつ理由を `docs/differences.md`（M7 で作る）に書く |
| A-05 | 見積もり ≥ 実際（手間・確保）と、ヒープの増え ≤ 数えた量が、照合の集まりと敵対的な入力の集まりのすべてで成り立つ |
| A-06 | 範囲の描画の、全体の切り出しとの一致の性質のテストが通る（ADR-13） |
| A-07 | NFR-03 のスタックのテスト、NFR-12 のファズの時間 |
| A-08 | 性能の目標（NFR-07・ADR-19） |
| A-09 | `docs/kura-integration.md`（M7 で作る）に、Kura の今の SVG の呼び出し（`codec/svg.rs`・`svg_text.rs`・`fonts.rs`・`svg_worker`・`Plan` のフック）の一覧があり、そのすべてに置き換え先の口が書かれている。Kura の `Decoder::reproducible` を文書ごとの印に変える案（または文字のある SVG を別の識別子に分ける案）を含む |
