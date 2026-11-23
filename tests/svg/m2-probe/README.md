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
| `d-comma-before-command` | 最後の引数のあとの `,` を受けて次の命令へ続けるか | 効く | 効く |
| `d-trailing-dot-mid` | 途中の末尾の `.`（前の区間は描くか） | 効く | 効く |
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
| `polygon-error` | 壊れた points は誤りの前まで（三角。左下は描かない） | 効かない | 効かない |
| `polygon-error-keeps` | 壊れた points の前の点は描く（三角の右上） | 効く | 効かない |
| `points-trailing-comma` | points の終わりの `,` を受ける（三角を描く。ADR-92 (3)） | 効く | 効く |
| `polyline-fill` | polyline も塗る（閉じない形を閉じて塗る） | 効く | 効く |
| `line-no-fill` | line は塗らない（線だけ） | 効かない | 効かない |

## 線

| 試し | 何を見るか | resvg | Chrome |
|---|---|---|---|
| `join-miter` | miter（比 2.24 ≤ 4）は先まで描く | 効く | 効く |
| `join-miter-limit` | miterlimit を超える miter は bevel（先を描かない） | 効く | 効く |
| `join-miter-clip` | miter-clip は miterlimit の所で切った先を描く | 効く | 効かない |
| `join-arcs` | arcs（直線どうしは miter-clip と同じ形） | 効かない | 効かない |
| `join-miter-clip-inherit` | 親が round のとき子の miter-clip は壊れた値として親の round を受け継ぐか（受け継げば頂きの 9 上も描く） | 効く | 効く |
| `join-arcs-inherit` | 親が round のとき子の arcs は壊れた値として親の round を受け継ぐか | 効かない | 効く |
| `join-round` | round は円の形（bevel より先、miter の先より手前） | 効く | 効く |
| `join-bevel` | bevel | 効く | 効く |
| `miterlimit-below-1` | miterlimit が 1 未満（壊れた値として既定の 4 にするか） | 効かない | 効かない |
| `cap-zero-round` | 長さ 0 の部分パス（M L 同じ点）の丸い端は点を描く | 効く | 効く |
| `cap-zero-close` | 長さ 0 の閉じた部分パス（M z）の丸い端 | 効く | 効く |
| `cap-zero-square` | 長さ 0 の四角い端は軸に沿う正方形（円の外の角も描く） | 効く | 効く |
| `cap-zero-butt` | 長さ 0 の平らな端は描かない | 効く | 効く |
| `cap-moveto-only` | M だけの部分パスは描かない（丸い端でも） | 効く | 効く |
| `dash-negative` | 負の値のある stroke-dasharray は実線 | 効く | 効く |
| `dash-zero-sum` | 合計 0 の stroke-dasharray は実線 | 効く | 効く |
| `dash-odd` | 奇数個の stroke-dasharray は 2 度並べる（`10` は 10 10。20〜30 は隙間） | 効く | 効く |
| `dash-percent` | % の stroke-dasharray（対角の √2 で割ったものに対して。10% は 10） | 効く | 効く |
| `dash-subpath-reset` | 破線は部分パスごとに始め直す（2 本目の頭も線） | 効く | 効く |
| `dash-offset` | stroke-dashoffset（5 ずらすと 15〜20 は隙間：効けば 17 は背景） | 効かない | 効かない |
| `dash-path-length` | pathLength が破線の長さに効く（長さ 80 を 4 とみなすと 1 1 は 20 20。30〜50 は隙間） | 効かない | 効く |
| `non-scaling-stroke` | non-scaling-stroke は画面の太さ（縦 4 倍でも太さ 2） | 効かない | 効く |
| `crisp-edges` | crispEdges は反エイリアスなし（縁 10.3 の画素 10 は全部塗る） | 効く | 効く |
| `optimize-speed` | optimizeSpeed（crispEdges と同じに扱うか） | 効く | 効く |
| `geometric-precision` | geometricPrecision は反エイリアス（縁の画素は混ざる。どちらでもないが正しい） | 効く | 効く |
| `closed-join` | 閉じたパスの始めの点も継ぎ目（miter の角を描く） | 効く | 効く |
| `paint-order-stroke` | paint-order: stroke は塗りを線の上に描く（線の内側の半分は緑の塗り） | 効く | 効く |
| `stroke-width-zero` | 太さ 0 の線は描かない | 効く | 効く |
| `dash-closed-join` | 閉じた形の最初と最後の破線をつなぐ（始めの角を miter の継ぎ目で描く。ADR-92 (1)） | 効く | 効く |
| `arc-zero-length` | 始めと終わりが同じ弧は長さ 0 の線（丸い端の点を描く。ADR-92 (2)） | 効く | 効く |
| `dash-zero-closed` | 閉じた長さ 0 の部分パス（M z）に破線を付けると描かない（Chrome と同じ） | 効く | 効く |
