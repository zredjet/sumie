# 照合の集まり

見た目を resvg・Chrome と比べる試験の SVG（ADR-21）。出どころ・版・ライセンスを確かめ、写しを許すものだけをここに置く。
取り込む前に一覧をユーザーに見せて承認を得る（2026-11-22 に下の表の扱いで承認。実例の SVG（`real/`）は同じ日に別に承認。ADR-59）。

## 出どころの一覧

| 出どころ | 置き場 | 版 | 件数 | ライセンス | 扱い |
|---|---|---|---|---|---|
| resvg の試験 | `resvg/`（コミットする） | タグ v0.48.1（コミット `68b14c4c3bcc`。Kura の resvg と同じ版） | 1,739 | MIT OR Apache-2.0（`resvg/LICENSE-*`・`resvg/SOURCE.md`） | SVG だけを写す。resvg の描いた PNG・画像・フォントは写さない |
| W3C SVG 1.1 第 2 版の試験 | `local/w3c-svg11-20110816/`（git の外。`.gitignore`） | 2011-08-16 版のアーカイブ（SHA-256 `b5f46cca…1030`） | 525（範囲の内 416） | 食い違う（総覧のページは W3C Document License と Member agreement、方針のページは W3C test suite license と 3-clause BSD。各ファイルの頭は W3C の著作権の表示） | リポジトリに入れない。`sumie-tools corpus fetch-w3c` が取り、SHA-256 を照らして `svg/*.svg` だけを置く。照合の結果の一覧だけを残す |
| Kura のファズのコーパス | Kura の `fuzz/corpus/decode_svg/`（読むだけ。写さない） | — | 約 8,000 | ユーザーのもの | 見た目の照合に使わず、ファズの種と「断るか描けるか」の照合に使う（M1 から） |
| Kura の手元の SVG | — | — | 1,439 | ライセンスでコミットできない | 消えていて使えない（ADR-58 (3)）。代わりに下の実例の SVG を使う（ADR-59） |
| 実例の SVG | `real/<名前>/`（コミットする） | 集まりごとに固定した rev（`real/<名前>/SOURCE.md`） | 1,241 | 集まりごと（下の表） | Kura の手元の SVG の代わり。`sumie-tools corpus fetch-real` が取り、git の blob の SHA-1 を照らして写す |

**実例の SVG**（`real/`。ADR-59。件数の多い集まりは道の順から ChaCha8 の種つきで抜き取る）：

| 名前 | 出どころ（rev） | ライセンス | 件数 | 性質 |
|---|---|---|---|---|
| `fluent-emoji-color` | github.com/microsoft/fluentui-emoji（`1ffb34c752ec`）の `Color` | MIT（Copyright (c) Microsoft Corporation） | 120（3,145 件から） | 放射・線形のグラデーションの多い絵文字（`filter` を使うものもある） |
| `fluent-emoji-flat` | 同じ（`Flat`） | MIT | 150（3,145 件から） | 平坦な塗りの絵文字 |
| `twemoji` | github.com/jdecked/twemoji（`b6b55fef1e86`）の `assets/svg` | 絵は CC BY 4.0（`LICENSE-GRAPHICS`）・コードは MIT | 300（4,009 件から） | 詰めた平坦な path の絵文字 |
| `flag-icons` | github.com/lipis/flag-icons（`ac7e0eb3259e`）の `flags/4x3` | MIT（Copyright (c) 2013 Panayiotis Lipiridis） | 271（全部） | 国旗（紋章の細かい path・グラデーション） |
| `bootstrap-icons` | github.com/twbs/icons（`829d2bc2b875`）の `icons` | MIT（Copyright (c) 2019-2024 The Bootstrap Authors） | 200（2,078 件から） | 塗りのアイコン |
| `tabler-icons` | github.com/tabler/tabler-icons（`85e870dbe868`）の `icons/outline` | MIT（Copyright (c) 2020-2026 Paweł Kuna） | 200（5,491 件から） | 線のアイコン（丸い端と継ぎ目） |

帰属：`real/twemoji/` の絵は Twemoji（2014〜2021 年は Twitter, Inc、2022 年からは Jason Sofonia と Justine De Caires
（jdecked/twemoji）と、ほかの貢献者による）のもので、
CC BY 4.0（https://creativecommons.org/licenses/by/4.0/）で使う。変えていない（取ったバイト列のまま。`MANIFEST.jsonl` の
BLAKE3 と git の blob）。

Wikimedia Commons の秀逸なベクター画像（パブリックドメイン・CC0・CC BY だけ。git の外の予定）は、取りに行ったところ
Wikimedia が自動の取り出しを 429（取りすぎ）で断ったので、M0 では取っていない（`corpus fetch-real --only
commons-featured-vector` は 1 分に 20 件まで・やり直しつきで取る。ADR-59 (3)）。

自作の試験の SVG は `tests/svg/`（コミットする）。

## 一覧（index.jsonl）

`sumie-tools corpus index [--root <根>]` が根の下の SVG を並べる（1 件 1 行）：

| 鍵 | 中身 |
|---|---|
| `path` | 根からの相対の道 |
| `source` | 出どころ（`resvg`・`real`（実例の SVG）・`w3c-svg11`・`gen-heavy` など） |
| `category` | 分類（resvg は `tests/` の下のフォルダ、実例は集まりの名前、W3C は名前の頭の章） |
| `scope` | `in` か `out:<理由>`（アニメーション・操作・スクリプトは v1 の外。requirements.md §4） |
| `bytes`・`blake3` | 大きさとダイジェスト |
| `features` | 使う機能の印（`text`・`filter`・`gradient`・`pattern`・`mask`・`clip`・`marker`・`image`・`use`・`switch`・`style`・`symbol`・`foreignObject`・`script`・`animation`・`blend`・`ub`）。マイルストーンごとの範囲と ADR-15 の群を選ぶのに使う。`filter` は CSS のフィルタ関数の `filter` の属性も、`image` は外の画像を指す `feImage` も、`ub` は題に「(UB)」とある resvg の試験の未定義の振る舞いの例（ADR-56 (1)） |

## 作り直し

```bash
# resvg の試験（rev 68b14c4c3bcc と SHA-256 は道具の定数。取ったアーカイブを照らしてから写す。--archive <取ったもの> も使える）
cargo run -p sumie-tools --release -- corpus import-resvg
cargo run -p sumie-tools --release -- corpus index
# 実例の SVG（rev は道具の定数。git の blob の SHA-1 を照らしてから写す。集まりの名前を指して 1 つずつ。
# Commons（commons-featured-vector）は名前で指したときだけ取る。M0 では取っていない。ADR-59 (3)）
cargo run -p sumie-tools --release -- corpus fetch-real --only fluent-emoji-color   # fluent-emoji-flat・twemoji・flag-icons・bootstrap-icons・tabler-icons も同じ
# W3C の試験（git の外）
cargo run -p sumie-tools --release -- corpus fetch-w3c
cargo run -p sumie-tools --release -- corpus index --root local/w3c-svg11-20110816
```
