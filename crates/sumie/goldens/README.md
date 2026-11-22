# ゴールデン

determinism.md §10。入力と画素のダイジェストを版つきで凍結した行。全ターゲットで同じファイルを照らす
（`cargo test -p sumie --test golden`）。

| ファイル | 中身 | 書き換え |
|---|---|---|
| `render-v<N>.jsonl` | 版 `sumie-render/<N>` の描いた結果（D1） | 凍結した版は**追記だけ**（`scripts/check-goldens-append-only.sh`）。版 0 は M7 まで作り直してよい（ADR-20。作り直すときはコミットを分け、差の一覧を付ける） |
| `estimate.jsonl` | 見積もりと警告の並び（D2。ADR-33） | rev ごとに作り直してよい |

`render-v<N>.jsonl` の 1 行（鍵はこの順。値は文字列か整数）：

| 鍵 | 中身 |
|---|---|
| `name` | 例の名前（ファイルの中で重ならない。中身を変えたいときは新しい名前の例を足す） |
| `input` | 入力の SVG のバイト列の BLAKE3（`tests/svg/` か `corpus/` のファイル。`path` で指す） |
| `path` | 入力の場所（workspace の根から） |
| `fonts` | フォントの鍵（`tests/fonts/` のファイルの BLAKE3）の並びを `,` でつないだもの（なければ空） |
| `decoder` | 画像のデコーダの識別子（なければ空） |
| `scale` | 倍率（10 進の文字列。浮動小数の字句の揺れを避ける） |
| `region` | 範囲（`x,y,w,h`。全体なら空） |
| `width`・`height` | 画素の大きさ |
| `pixels` | 画素（幅・高さ・行を詰めた RGBA8 乗算済み）の BLAKE3 |
| `report` | 版に入る報告（使ったフォントの鍵と書体の番号・見つからなかった文字・デコーダの識別子）の BLAKE3 |

`estimate.jsonl` の形は、見積もりの口ができる M1 で決めてここに書く。
