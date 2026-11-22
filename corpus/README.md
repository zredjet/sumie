# 照合の集まり

見た目を resvg・Chrome と比べる試験の SVG（ADR-21）。出どころ・版・ライセンスを確かめ、写しを許すものだけをここに置く。
取り込む前に一覧をユーザーに見せて承認を得る（2026-11-22 に下の表の扱いで承認）。

## 出どころの一覧

| 出どころ | 置き場 | 版 | 件数 | ライセンス | 扱い |
|---|---|---|---|---|---|
| resvg の試験 | `resvg/`（コミットする） | タグ v0.48.1（コミット `68b14c4c3bcc`。Kura の resvg と同じ版） | 1,739 | MIT OR Apache-2.0（`resvg/LICENSE-*`・`resvg/SOURCE.md`） | SVG だけを写す。resvg の描いた PNG・画像・フォントは写さない |
| W3C SVG 1.1 第 2 版の試験 | `local/w3c-svg11-20110816/`（git の外。`.gitignore`） | 2011-08-16 版のアーカイブ（SHA-256 `b5f46cca…1030`） | 525（範囲の内 416） | 食い違う（総覧のページは W3C Document License と Member agreement、方針のページは W3C test suite license と 3-clause BSD。各ファイルの頭は W3C の著作権の表示） | リポジトリに入れない。`sumie-tools corpus fetch-w3c` が取り、SHA-256 を照らして `svg/*.svg` だけを置く。照合の結果の一覧だけを残す |
| Kura のファズのコーパス | Kura の `fuzz/corpus/decode_svg/`（読むだけ。写さない） | — | 約 8,000 | ユーザーのもの | 見た目の照合に使わず、ファズの種と「断るか描けるか」の照合に使う（M1 から） |
| Kura の手元の SVG | 置き場はユーザーに聞く（git の外） | — | 1,439 | ライセンスでコミットできない | Kura の `SvgCodec` と sumie の両方に通し、「描く／断る」と大きさの一致の結果の一覧だけを残す（M0 で仕組み、M7 で照らす。NFR-10・ADR-44 (4)） |

自作の試験の SVG は `tests/svg/`（コミットする）。

## 一覧（index.jsonl）

`sumie-tools corpus index [--root <根>]` が根の下の SVG を並べる（1 件 1 行）：

| 鍵 | 中身 |
|---|---|
| `path` | 根からの相対の道 |
| `source` | 出どころ（`resvg`・`w3c-svg11`・`gen-heavy` など） |
| `category` | 分類（resvg は `tests/` の下のフォルダ、W3C は名前の頭の章） |
| `scope` | `in` か `out:<理由>`（アニメーション・操作・スクリプトは v1 の外。requirements.md §4） |
| `bytes`・`blake3` | 大きさとダイジェスト |
| `features` | 使う機能の印（`text`・`filter`・`gradient`・`pattern`・`mask`・`clip`・`marker`・`image`・`use`・`switch`・`style`・`symbol`・`foreignObject`・`script`・`animation`・`blend`）。マイルストーンごとの範囲を選ぶのに使う |

## 作り直し

```bash
# resvg の試験（取ったソースの根を渡す）
cargo run -p sumie-tools --release -- corpus import-resvg --src <linebender-resvg-68b14c4> --rev 68b14c4c3bccdb60344c777406486b54c36ec1a4 --sha256 41bf46fdb8abc6fc49ec6f83facff0cdab2759a7212734c92f3f6aec12c51820
cargo run -p sumie-tools --release -- corpus index
# W3C の試験（git の外）
cargo run -p sumie-tools --release -- corpus fetch-w3c
cargo run -p sumie-tools --release -- corpus index --root local/w3c-svg11-20110816
```
