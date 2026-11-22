# Kura の側への依頼：照合の集まりを今の `SvgCodec` に通した「描く／断る」の一覧（NFR-10・roadmap M0）

sumie のリポジトリからは Kura で cargo を走らせないので、Kura の側で作ってもらう一覧です。sumie の M1 以降で、「Kura が今
描けるものを sumie が断らない」（NFR-10）を照らすのに使います。

## 入力

- sumie のリポジトリの `corpus/index.jsonl` の `path`（`corpus/` からの相対の道。今は resvg の試験の 1,739 件）
- 手元の W3C の試験（`local/w3c-svg11-20110816/index.jsonl`。git の外。`sumie-tools corpus fetch-w3c` で作れる）も通して
  もらえると助かる（任意）

## 通し方

- 働き手（`svg_worker`）を通さず、**プロセスの中の `SvgCodec`**（アプリと同じ固定の段のデコーダ。フォントは
  `kura_mac::pinned_options()` のシステムのフォント）で、形（`shape`）と値（`decode`）の両方を読む
- 時間で断られるもの（働き手の 2 秒・10 秒）は「描く」に入れる（プロセスの中では時間で止めないので、そのまま描ける）
- 値の画素は要らない（大きさと、描けたか・断ったかだけ）

## 出してほしい一覧

`corpus/kura-svgcodec-<Kura の rev の先頭 12 桁>.jsonl`（1 件 1 行。sumie のリポジトリの `corpus/` の横に置く）：

| 鍵 | 中身 |
|---|---|
| `path` | 入力の `path`（そのまま） |
| `blake3` | 入力のバイト列の BLAKE3（`index.jsonl` と照らす） |
| `kura_rev` | Kura のコミット |
| `shape` | `ok` か誤りの種類（`Unsupported`・`Limit`・`Decode` など。Kura の `EngineError` の名前） |
| `decode` | 同じ |
| `width`・`height` | 形で読んだ大きさ（読めたときだけ） |
| `message` | 誤りの文（読めなかったときだけ） |

## Kura の手元の SVG（1,439 件）

ライセンスでコミットできないので、置き場所を教えてください。sumie の M7 で、git の外で Kura の一覧と sumie の結果を
照らし、結果の一覧（道のダイジェスト・描く／断る・大きさの一致）だけを残します。
