# 出どころ：resvg の試験

| 項目 | 値 |
|---|---|
| リポジトリ | https://github.com/linebender/resvg |
| 版 | タグ `v0.48.1`（コミット `68b14c4c3bccdb60344c777406486b54c36ec1a4`。Kura が使う resvg と同じ版） |
| 取ったもの | `https://codeload.github.com/linebender/resvg/tar.gz/68b14c4c3bccdb60344c777406486b54c36ec1a4`（SHA-256 `41bf46fdb8abc6fc49ec6f83facff0cdab2759a7212734c92f3f6aec12c51820`） |
| 写したもの | `crates/resvg/tests/` の `tests/`・`extra/`・`resources/` の `.svg`（1739 件。道は `crates/resvg/tests/` からの相対のまま） |
| 写さなかったもの | resvg の描いた PNG（照合の相手の絵はゴールデンにしない）・画像・フォント・試験のコード |
| ライセンス | MIT OR Apache-2.0（リポジトリの README の License の節。`LICENSE-MIT`・`LICENSE-APACHE` はリポジトリの根の写し。試験の README にほかの定めはない） |
| 一覧 | `MANIFEST.jsonl`（道・バイト数・BLAKE3） |

作り直し：`cargo run -p sumie-tools -- corpus import-resvg [--archive <取ったアーカイブ>]`（rev と SHA-256 は道具の定数。アーカイブの SHA-256 が違うときは、この MANIFEST.jsonl の BLAKE3 で 1 件ずつ照らす）
