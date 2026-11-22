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
- 上限は Kura のアプリと同じ既定の `Limits`（`Limits::default()` のまま。変えたときは一覧の頭か `limits` の鍵に書く）
- 時間で断られるもの（働き手の 2 秒・10 秒）は「描く」に入れる（プロセスの中では時間で止めないので、そのまま描ける）
- 値の画素は要らない（大きさと、描けたか・断ったかだけ）

## 出してほしい一覧

`corpus/kura-svgcodec-<Kura の rev の先頭 12 桁>.jsonl`（1 件 1 行。sumie のリポジトリの `corpus/` の横に置く）：

| 鍵 | 中身 |
|---|---|
| `path` | 入力の `path`（そのまま） |
| `blake3` | 入力のバイト列の BLAKE3（`index.jsonl` と照らす） |
| `kura_rev` | Kura のコミット |
| `limits` | 既定の `Limits` なら `default`（変えたときはその値） |
| `shape` | `ok` か誤りの種類（`Unsupported`・`Limit`・`Decode` など。Kura の `EngineError` の名前） |
| `decode` | 同じ |
| `width`・`height` | 形で読んだ大きさ（読めたときだけ） |
| `message` | 誤りの文（読めなかったときだけ） |

## Kura の手元の SVG（1,439 件）

ライセンスでコミットできないので、置き場所を教えてください。照らす仕組みは M0 で作った（`sumie-tools kura-compat
--kura <Kura の一覧> --dir <SVG の根>`。git の外に結果の一覧だけを書き、道は書かずに入力の BLAKE3 で指す）。M0 は resvg
（Kura の設定）の大きさを Kura の一覧と照らし、M1 から sumie の読む口の結果を並べ、M7 で全体を照らす。
