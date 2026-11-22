# M1 の照らし（試しの SVG）

`cargo run -p sumie-tools --release -- probe` が作って描いた結果（roadmap M1 の依頼文。範囲はこの結果を見て
決める）。「効く」は決めた点の色が効いたときの色に近いこと。resvg は Kura の設定の 0.48.1、Chrome はヘッドレス
（`--disable-gpu`）。試しの SVG はこのリポジトリのために書いたもの（MIT OR Apache-2.0）。

## セレクタ

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `type` | 型 | 効く | 効く |
| `type-upper` | 型の大文字（XML では区別する。当たらないのが正しい） | 効かない | 効かない |
| `universal` | `*` | 効く | 効く |
| `id` | `#id` | 効く | 効く |
| `class` | `.class` | 効く | 効く |
| `class-two` | `.a.b` | 効く | 効く |
| `attr-exists` | `[a]` | 効く | 効く |
| `attr-eq` | `[a=v]` | 効く | 効く |
| `attr-word` | `[a~=v]` | 効く | 効く |
| `attr-dash` | `[a\|=v]` | 効く | 効く |
| `attr-prefix` | `[a^=v]` | 効かない | 効く |
| `attr-suffix` | `[a$=v]` | 効かない | 効く |
| `attr-substr` | `[a*=v]` | 効かない | 効く |
| `attr-case-i` | `[a=v i]`（大文字小文字を無視する印） | 効かない | 効く |
| `first-child` | `:first-child` | 効く | 効く |
| `last-child` | `:last-child` | 効かない | 効く |
| `nth-child` | `:nth-child(2)` | 効かない | 効く |
| `nth-child-an-b` | `:nth-child(2n+1)` | 効かない | 効く |
| `nth-child-odd` | `:nth-child(odd)` | 効かない | 効く |
| `nth-last-child` | `:nth-last-child(1)` | 効かない | 効く |
| `only-child` | `:only-child` | 効かない | 効く |
| `first-of-type` | `:first-of-type` | 効かない | 効く |
| `last-of-type` | `:last-of-type` | 効かない | 効く |
| `nth-of-type` | `:nth-of-type(1)` | 効かない | 効く |
| `only-of-type` | `:only-of-type` | 効かない | 効く |
| `not` | `:not(.c)` | 効かない | 効く |
| `is` | `:is(.a, .z)` | 効かない | 効く |
| `where` | `:where(.a)` | 効かない | 効く |
| `has` | `:has(> rect)` | 効かない | 効く |
| `root` | `:root` | 効かない | 効く |
| `empty` | `:empty` | 効かない | 効く |
| `lang` | `:lang(en)` | 効かない | 効く |
| `hover` | `:hover`（動く擬似クラス。当たらないのが正しい） | 効かない | 効かない |
| `pseudo-element` | `::before`（擬似要素。当たらないのが正しい） | 効かない | 効かない |
| `descendant` | 子孫 `g rect` | 効く | 効く |
| `child` | 子 `g > rect` | 効く | 効く |
| `adjacent` | 直後の兄弟 `desc + rect` | 効く | 効く |
| `sibling` | 後の兄弟 `desc ~ rect` | 効かない | 効く |
| `list` | 並び `circle, rect` | 効く | 効く |
| `list-invalid` | 並びの 1 つが壊れている（規則ごと捨てるのが正しい） | 効く | 効かない |
| `namespace` | 名前空間 `svg\|rect`（`@namespace`） | 効かない | 効く |
| `namespace-any` | `*\|rect` | 効かない | 効く |
| `specificity` | 詳細度（`#r` が後の `.a` に勝つ） | 効く | 効く |
| `order` | 同じ詳細度は後が勝つ | 効く | 効く |
| `deep-descendant` | 長い子孫の鎖 | 効く | 効く |

## カスケード

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `style-attr-over-rule` | `style` 属性が規則に勝つ | 効く | 効く |
| `important-over-style-attr` | 規則の `!important` が `style` 属性に勝つ | 効く | 効く |
| `style-attr-important` | `style` 属性の `!important` が規則の `!important` に勝つ | 効かない | 効く |
| `style-type-other` | `<style type="text/foo">` は読まない（当たらないのが正しい） | 効かない | 効かない |
| `style-cdata` | `<style>` の CDATA | 効く | 効く |
| `style-after-element` | 要素のあとに書いた `<style>` も当たる | 効く | 効く |

## アットルール

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `media-screen` | `@media screen` | 効かない | 効く |
| `media-all` | `@media all` | 効かない | 効く |
| `media-print` | `@media print`（当たらないのが正しい） | 効かない | 効かない |
| `media-min-width` | `@media (min-width: 1px)` | 効かない | 効く |
| `supports` | `@supports (fill: green)` | 効かない | 効く |
| `import-then-rule` | `@import` のあとの規則 | 効く | 効く |
| `font-face-then-rule` | `@font-face` のあとの規則 | 効く | 効く |
| `charset-then-rule` | `@charset` のあとの規則 | 効く | 効く |
| `layer` | `@layer` | 効かない | 効く |
| `unknown-at-then-rule` | 知らないアットルールのあとの規則 | 効く | 効く |

## SVG 2 の属性・プロパティ

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `transform-attr` | `transform` 属性（SVG 1.1。比べの基準） | 効く | 効く |
| `transform-css-rule` | CSS の `transform` プロパティ（`<style>`） | 効かない | 効く |
| `transform-css-style-attr` | CSS の `transform` プロパティ（`style` 属性） | 効かない | 効く |
| `transform-origin` | `transform-origin` 属性 | 効く | 効く |
| `paint-order` | `paint-order` 属性 | 効く | 効く |
| `mix-blend-mode` | `mix-blend-mode` 属性（multiply で白を重ねると下の緑のまま） | 効かない | 効かない |
| `mix-blend-mode-css` | CSS の `mix-blend-mode`（`style` 属性） | 効く | 効く |
| `isolation` | `isolation` 属性（切り離すと multiply の下は透明で、白のまま） | 効く | 効く（見た目。ただし getComputedStyle では属性を読んでいない。M3 で照らし直す） |
| `mask-type` | `mask-type="alpha"` 属性（黒い不透明の中身で、α なら見える） | 効く | 効く |
| `vector-effect` | `vector-effect="non-scaling-stroke"` 属性 | どちらでもない（127,64,0） | 効く |
| `geometry-css-width` | CSS の `width`・`height`（rect の幾何のプロパティ） | 効かない | 効く |
| `geometry-css-r` | CSS の `r`（circle） | 効かない | 効く |
| `geometry-css-d` | CSS の `d`（path） | 効かない | 効く |
| `marker-shorthand` | `marker` のまとめのプロパティ（`style` 属性） | 効く | 効く |
| `marker-attr` | `marker-start` 属性（SVG 1.1。比べの基準） | 効く | 効く |

## 値の書き方

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `color-hex3` | `#rgb` | 効く | 効く |
| `color-hex6` | `#rrggbb` | 効く | 効く |
| `color-hex4` | `#rgba` | 効く | 効く |
| `color-hex8` | `#rrggbbaa` | 効く | 効く |
| `color-hex-upper` | 大文字の `#0A0` | 効く | 効く |
| `color-rgb` | `rgb(0, 128, 0)` | 効く | 効く |
| `color-rgb-percent` | `rgb(0%, 50%, 0%)` | 効く | 効く |
| `color-rgb-space` | `rgb(0 128 0)`（CSS Color 4） | 効く | 効く |
| `color-rgb-slash` | `rgb(0 128 0 / 1)` | 効かない | 効く |
| `color-rgba` | `rgba(0, 128, 0, 1)` | 効く | 効く |
| `color-hsl` | `hsl(120, 100%, 25%)` | 効く | 効く |
| `color-hsla` | `hsla(120, 100%, 25%, 1)` | 効く | 効く |
| `color-hwb` | `hwb(120 0% 50%)` | 効かない | 効く |
| `color-named` | 名前 `green` | 効く | 効く |
| `color-named-upper` | 大文字の名前 `GREEN` | 効く | 効く |
| `color-system` | システムの色 `ButtonText` | 効かない | 効かない |
| `unit-mm` | 長さの `mm` | 効く | 効く |
| `unit-in` | 長さの `in` | 効く | 効く |
| `unit-pt` | 長さの `pt` | 効く | 効く |
| `unit-pc` | 長さの `pc` | 効く | 効く |
| `unit-em` | 長さの `em`（font-size 50） | 効く | 効く |
| `unit-ex` | 長さの `ex`（font-size 50。usvg は em の半分） | 効く | 効く |
| `unit-percent` | 長さの `%`（表示の領域の幅） | 効く | 効く |
| `unit-vw` | 長さの `vw` | 効かない | 効く |
| `unit-px-upper` | 大文字の単位 `100PX` | 効かない | 効く |

## 文書

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `duplicate-id` | 同じ id の 2 つ（緑＝最初のもの・赤＝後のもの） | 効かない | 効く |
| `required-features-bogus` | `requiredFeatures` の知らない機能（赤＝照らして外す・緑＝無視する） | 効く | 効かない |
| `required-features-known` | `requiredFeatures` の SVG 1.1 の機能（`#Shape`。緑＝通る） | 効く | 効く |
| `required-features-empty` | 空の `requiredFeatures`（赤＝偽とみなす・緑＝無視する） | 効く | 効かない |
| `required-extensions` | `requiredExtensions` の知らない拡張（赤＝外す） | 効く | 効く |
| `system-language` | `systemLanguage="ja"`（言語の並びは en。緑＝照らして外す） | 効く | 効かない |
| `system-language-region` | `systemLanguage="en-US"`（言語の並びは en。緑＝通る） | 効く | 効く |
| `use-self-cycle` | 自分を指す use（緑＝ほかは描く） | 効く | 効く |
| `pattern-cycle` | 中身が自分を塗る pattern（緑＝輪の外は描く） | 効く | 効く |
| `attlist-default` | ATTLIST の既定の属性値（緑＝当てる・黒＝当てない） | 効かない | 効く |
| `entity` | 内部の一般の実体（比べの基準） | 効く | 効く |
| `xml-space-preserve-attr` | 属性の値の改行（属性の値の正規化。比べの基準） | 効く | 効く |

## CSS の細部

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `prop-upper` | プロパティの名前の大文字 `FILL`（CSS は区別しない） | 効かない | 効く |
| `prop-upper-attr` | style 属性のプロパティの名前の大文字 | 効かない | 効く |
| `value-upper` | 色の名前の大文字 `GREEN` | 効く | 効く |
| `escape-class` | クラスの名前の escape `.\61` | 効かない | 効く |
| `escape-type` | 型の名前の escape `r\65 ct` | 効かない | 効く |
| `escape-hex-space` | escape のあとの空白 `#\72 ` | 効かない | 効く |
| `id-upper` | `#R`（id は区別する。当たらないのが正しい） | 効かない | 効かない |
| `class-upper` | `.A`（class は区別する。当たらないのが正しい） | 効かない | 効かない |
| `attr-name-upper` | `[DATA-X]`（XML の属性の名前は区別する。当たらないのが正しい） | 効かない | 効かない |
| `attr-unquoted` | `[data-x=foo]`（引用符なしの値） | 効く | 効く |
| `attr-xlink-href` | `[href]` が `xlink:href` に当たるか | 効く | 効かない |
| `first-child-foreign` | `:first-child` の前に別の名前空間の要素 | 効かない | 効かない |
| `adjacent-foreign` | `desc + rect` の間に別の名前空間の要素 | 効かない | 効かない |
| `child-foreign` | `g > rect` の間に別の名前空間の要素（子でないので当たらないのが CSS） | 透明 | 透明 |
| `first-child-text` | `:first-child` の前に文字（文字は数えない） | 効く | 効く |
| `first-child-root` | 根の要素の `:first-child` | 効く | 効く |
| `type-foreign` | 別の名前空間の要素の型 `x rect`（`foo:x` に当たるか） | 透明 | 透明 |
| `important-space` | `! important`（間に空白） | 効く | 効く |
| `important-upper` | `!IMPORTANT` | 効かない | 効く |
| `decl-later-wins` | 同じ規則の後の宣言が勝つ | 効く | 効く |
| `decl-invalid-later` | 後の宣言が壊れた値（前の宣言が残るのが CSS） | どちらでもない（0,0,0） | 効く |
| `attr-invalid-value` | プレゼンテーション属性の壊れた値（受け継いだ値が残るか） | どちらでもない（0,0,0） | 効く |
| `cdo-cdc` | `<!--`・`-->` を CSS の中に書く | 効かない | 効く |
| `comment-in-selector` | セレクタの中の注釈 `rect/**/.a` | 効く | 効く |
| `combinator-tight` | 空白のない結合子 `g>rect` | 効く | 効く |
| `unclosed-block` | 閉じていない規則（文書の終わりで閉じる） | 効く | 効く |
| `string-in-value` | 宣言の値の中の `;` を含む文字列 | 効く | 効く |
| `bad-selector-block` | 読めないセレクタの規則のあとの規則 | 効く | 効かない |
| `style-two` | 2 つの `<style>`（後が勝つ） | 効く | 効く |
| `style-media-attr` | `<style media="print">`（media 属性を見るか。見なければ当たる） | 効く | 効かない |
| `style-cdata-split` | 文字と CDATA に分かれた `<style>` | 効く | 効く |
| `style-in-foreign` | 別の名前空間の要素の中の `<style>` | 効く | 効く |
| `style-foreign-ns` | 別の名前空間の `style` 要素（当たらないのが正しい） | 効く | 効かない |

## 大きさ

| 試し | 何を見るか | resvg の大きさ |
|---|---|---|
| `size/w-h` | 幅と高さ（比べの基準） | 100 × 50（100 × 50） |
| `size/viewbox-only` | viewBox だけ | 120 × 80（120 × 80） |
| `size/none` | どちらもない（中身の外接矩形） | 60 × 50（60 × 50） |
| `size/width-only` | 幅だけ | 60 × 50（60 × 50） |
| `size/height-only` | 高さだけ | 60 × 50（60 × 50） |
| `size/width-only-viewbox` | 幅だけと viewBox | 240 × 160（240 × 160） |
| `size/percent` | 幅・高さが %（viewBox なし） | 60 × 50（60 × 50） |
| `size/percent-viewbox` | 幅・高さが % と viewBox | 60 × 80（60 × 80） |
| `size/zero` | 幅が 0 | 誤り（usvg：SVG has an invalid size） |
| `size/negative` | 幅が負 | 誤り（usvg：SVG has an invalid size） |
| `size/fraction` | 幅が 1 に満たない | 1 × 50（0.3 × 50） |
| `size/fraction-up` | 幅が 10.5（切り上げ） | 11 × 50（10.5 × 50） |
| `size/mm` | 幅が 10mm | 38 × 50（37.795277 × 50） |
| `size/em` | 幅が 10em | 120 × 50（120 × 50） |
| `size/auto` | 幅が auto | 60 × 50（60 × 50） |
| `size/bogus` | 幅が壊れた値 | 60 × 50（60 × 50） |
| `size/viewbox-zero` | viewBox の幅が 0 | 60 × 50（60 × 50） |
| `size/viewbox-negative` | viewBox の幅が負 | 60 × 50（60 × 50） |
| `size/none-negative-content` | どちらもない・中身が負の座標 | 20 × 20（20 × 20） |
| `size/none-empty` | どちらもない・中身もない | 100 × 100（100 × 100） |
| `size/none-transform` | どちらもない・変形した中身 | 110 × 60（110 × 60） |
| `size/none-hidden` | どちらもない・見えない中身（visibility・display） | 200 × 200（200 × 200） |
