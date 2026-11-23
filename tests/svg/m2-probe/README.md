# M2 の照らし（試しの SVG）

`cargo run -p sumie-tools --release -- probe --set m2` が作って描いた結果（roadmap の依頼文。範囲はこの結果を見て
決める）。「効く」は決めた点の色が効いたときの色に近いこと。resvg は Kura の設定の 0.48.1、Chrome はヘッドレス
（`--disable-gpu`）。試しの SVG はこのリポジトリのために書いたもの（MIT OR Apache-2.0）。

## パスの文法

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `d-basic` | 基本の四角（確かめ） | 効く | 効く |
| `d-implicit-line` | M のあとの座標の組は暗黙の L | 効く | 効く |
| `d-plus-sign` | 数の頭の `+` | 効く | 効く |
| `d-trailing-dot` | 末尾の `.` の数（`10.`。SVG 1.1 の文法は受ける） | 効く | 効かない |
| `d-exp` | 指数（`9e1`） | 効く | 効く |
| `d-exp-no-digits` | 数字のない指数（`90e`）を数 90 として続けるか（続けなければ三角） | 効かない | 効かない |
| `d-compact-flags` | 弧の旗の詰め書き（`1190` は旗 1・1 と 90） | 効く | 効く |
| `d-flags` | 弧の旗を分けて書く（確かめ） | 効く | 効く |
| `d-arc-scale` | 足りない半径を広げる（半径 1 でも半円） | 効く | 効く |
| `d-arc-zero` | 半径 0 の弧は線 | 効く | 効く |
| `d-error-partial` | 誤り（引数の足りない Q）の前までを描く | 効く | 効く |
| `d-error-keeps-before` | 誤りの前の区間は描く（三角の右上） | 効く | 効く |
| `d-error-drops-broken` | 引数の足りない区間は描かない（左下は描かない） | 効かない | 効かない |
| `d-numbers-after-z` | Z のあとの数（暗黙の L として続けるか） | 効かない | 効かない |
| `d-first-not-m` | M で始まらない（何も描かない） | 効かない | 効かない |
| `d-double-comma` | 続く `,,`（誤り） | 効かない | 効かない |
| `d-leading-comma` | 命令のすぐあとの `,`（誤り） | 効かない | 効かない |
| `d-css-path` | CSS の `d: path()` | 効かない | 効く |
| `d-css-over-attr` | CSS の `d` が属性に勝つ | 効かない | 効く |

## 図形

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `rect-rx-auto` | rect の `rx` の auto は `ry`（角が丸い） | 効く | 効く |
| `rect-rx-clamp` | rect の `rx` は幅の半分に切り詰める | 効く | 効く |
| `rect-rx-negative` | 負の `rx` は壊れた値（角は直角） | 効く | 効く |
| `rect-css-size` | CSS の `width`・`height` | 効かない | 効く |
| `rect-percent` | % の幅（表示の領域の幅に対して） | 効く | 効く |
| `circle-css-r` | CSS の `r` | 効かない | 効く |
| `ellipse-rx-auto` | ellipse の `rx` の auto は `ry`（円） | 効く | 効く |
| `polygon-odd` | 数が奇数の points は最後を捨てる（三角。左下は描かない） | 効かない | 効かない |
| `polygon-odd-keeps` | 数が奇数の points でも残りの点は描く（三角の右上） | 効く | 効く |

## パスの文法

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `d-comma-before-command` | 最後の引数のあとの `,` を受けて次の命令へ続けるか | 効く | 効く |
| `d-trailing-dot-mid` | 途中の末尾の `.`（前の区間は描くか） | 効く | 効く |

## 図形

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `polygon-error` | 壊れた points は誤りの前まで（三角。左下は描かない） | 効かない | 効かない |
| `polygon-error-keeps` | 壊れた points の前の点は描く（三角の右上） | 効く | 効かない |
| `polyline-fill` | polyline も塗る（閉じない形を閉じて塗る） | 効く | 効く |
| `line-no-fill` | line は塗らない（線だけ） | 効かない | 効かない |
