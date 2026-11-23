//! `probe --set m2`：M2 のパスの文法と図形の細部を resvg・Chrome と照らす試し（roadmap M2。ADR-89）。赤い全面の上に
//! 緑の図形を描き、決めた点の色で「効く」（緑か、角を丸めるなど背景が見える方）を見る。試しの SVG はこのリポジトリの
//! ために書いたもの。

use crate::probe::{GREEN, Probe, RED, svg};

/// 赤い全面の上に `shape` を描いた試し。点が緑なら効く。
fn on_red(group: &'static str, name: &'static str, what: &'static str, shape: &str, point: (u32, u32)) -> Probe {
    Probe {
        group,
        name,
        what,
        body: svg(&format!("<rect width=\"100\" height=\"100\" fill=\"red\"/>\n{shape}")),
        yes: GREEN,
        no: RED,
        point,
    }
}

/// 赤い全面の上の緑の path（d を書いたもの）。
fn d(name: &'static str, what: &'static str, data: &str, point: (u32, u32)) -> Probe {
    on_red("パスの文法", name, what, &format!("<path fill=\"green\" d=\"{data}\"/>"), point)
}

/// 効けば背景（赤）が見える試し（角の丸めなど）。
fn shows_red(name: &'static str, what: &'static str, shape: &str, point: (u32, u32)) -> Probe {
    let mut p = on_red("図形", name, what, shape, point);
    p.yes = RED;
    p.no = GREEN;
    p
}

/// 線の試し（赤い全面の上の緑の線）。`yes_red` なら効いたとき背景が見える。
fn stroke(name: &'static str, what: &'static str, shape: &str, point: (u32, u32), yes_red: bool) -> Probe {
    let mut p = on_red("線", name, what, shape, point);
    if yes_red {
        p.yes = RED;
        p.no = GREEN;
    }
    p
}

/// 線の試しの並び（ADR-89 の照らして決める項目）。
fn stroke_probes() -> Vec<Probe> {
    let g = "fill=\"none\" stroke=\"green\"";
    // 頂きの角は約 53°。太さ 20 の miter の先は頂きの 22.4 上、bevel の縁は 4.5 上、miterlimit 1.5 の切り詰めは 15 上
    let apex = |join: &str, limit: &str| {
        format!(
            "<polyline points=\"20,80 50,20 80,80\" {g} stroke-width=\"20\" stroke-linejoin=\"{join}\" stroke-miterlimit=\"{limit}\"/>"
        )
    };
    let hline =
        |extra: &str| format!("<line x1=\"10\" y1=\"50\" x2=\"90\" y2=\"50\" {g} stroke-width=\"10\" {extra}/>");
    let crisp = |mode: &str| {
        format!("<rect x=\"10.3\" y=\"10\" width=\"80\" height=\"80\" fill=\"green\" shape-rendering=\"{mode}\"/>")
    };
    vec![
        stroke("join-miter", "miter（比 2.24 ≤ 4）は先まで描く", &apex("miter", "4"), (50, 5), false),
        stroke(
            "join-miter-limit",
            "miterlimit を超える miter は bevel（先を描かない）",
            &apex("miter", "1.5"),
            (50, 10),
            true,
        ),
        stroke(
            "join-miter-clip",
            "miter-clip は miterlimit の所で切った先を描く",
            &apex("miter-clip", "1.5"),
            (50, 10),
            false,
        ),
        stroke("join-arcs", "arcs（直線どうしは miter-clip と同じ形）", &apex("arcs", "1.5"), (50, 10), false),
        stroke(
            "join-miter-clip-inherit",
            "親が round のとき子の miter-clip は壊れた値として親の round を受け継ぐか（受け継げば頂きの 9 上も描く）",
            &format!("<g stroke-linejoin=\"round\">{}</g>", apex("miter-clip", "1.5")),
            (50, 11),
            false,
        ),
        stroke(
            "join-arcs-inherit",
            "親が round のとき子の arcs は壊れた値として親の round を受け継ぐか",
            &format!("<g stroke-linejoin=\"round\">{}</g>", apex("arcs", "1.5")),
            (50, 11),
            false,
        ),
        stroke(
            "join-round",
            "round は円の形（bevel より先、miter の先より手前）",
            &apex("round", "4"),
            (50, 11),
            false,
        ),
        stroke("join-bevel", "bevel", &apex("bevel", "4"), (50, 10), true),
        stroke(
            "miterlimit-below-1",
            "miterlimit が 1 未満（壊れた値として既定の 4 にするか）",
            &apex("miter", "0.5"),
            (50, 5),
            false,
        ),
        stroke(
            "cap-zero-round",
            "長さ 0 の部分パス（M L 同じ点）の丸い端は点を描く",
            &format!("<path d=\"M50 50 L50 50\" {g} stroke-width=\"20\" stroke-linecap=\"round\"/>"),
            (50, 50),
            false,
        ),
        stroke(
            "cap-zero-close",
            "長さ 0 の閉じた部分パス（M z）の丸い端",
            &format!("<path d=\"M50 50 z\" {g} stroke-width=\"20\" stroke-linecap=\"round\"/>"),
            (50, 50),
            false,
        ),
        stroke(
            "cap-zero-square",
            "長さ 0 の四角い端は軸に沿う正方形（円の外の角も描く）",
            &format!("<path d=\"M50 50 L50 50\" {g} stroke-width=\"20\" stroke-linecap=\"square\"/>"),
            (58, 58),
            false,
        ),
        stroke(
            "cap-zero-butt",
            "長さ 0 の平らな端は描かない",
            &format!("<path d=\"M50 50 L50 50\" {g} stroke-width=\"20\"/>"),
            (50, 50),
            true,
        ),
        stroke(
            "cap-moveto-only",
            "M だけの部分パスは描かない（丸い端でも）",
            &format!("<path d=\"M50 50\" {g} stroke-width=\"20\" stroke-linecap=\"round\"/>"),
            (50, 50),
            true,
        ),
        stroke(
            "dash-negative",
            "負の値のある stroke-dasharray は実線",
            &hline("stroke-dasharray=\"10 -5\""),
            (22, 50),
            false,
        ),
        stroke(
            "dash-zero-sum",
            "合計 0 の stroke-dasharray は実線",
            &hline("stroke-dasharray=\"0 0\""),
            (50, 50),
            false,
        ),
        stroke(
            "dash-odd",
            "奇数個の stroke-dasharray は 2 度並べる（`10` は 10 10。20〜30 は隙間）",
            &hline("stroke-dasharray=\"10\""),
            (25, 50),
            true,
        ),
        stroke(
            "dash-percent",
            "% の stroke-dasharray（対角の √2 で割ったものに対して。10% は 10）",
            &hline("stroke-dasharray=\"10%\""),
            (25, 50),
            true,
        ),
        stroke(
            "dash-subpath-reset",
            "破線は部分パスごとに始め直す（2 本目の頭も線）",
            &format!("<path d=\"M10 30 H60 M10 70 H60\" {g} stroke-width=\"10\" stroke-dasharray=\"30 30\"/>"),
            (15, 70),
            false,
        ),
        stroke(
            "dash-offset",
            "stroke-dashoffset（5 ずらすと 15〜20 は隙間：効けば 17 は背景）",
            &hline("stroke-dasharray=\"10 5\" stroke-dashoffset=\"5\""),
            (17, 50),
            false,
        ),
        stroke(
            "dash-path-length",
            "pathLength が破線の長さに効く（長さ 80 を 4 とみなすと 1 1 は 20 20。30〜50 は隙間）",
            &format!("<path d=\"M10 50 H90\" pathLength=\"4\" {g} stroke-width=\"10\" stroke-dasharray=\"1 1\"/>"),
            (40, 50),
            true,
        ),
        stroke(
            "non-scaling-stroke",
            "non-scaling-stroke は画面の太さ（縦 4 倍でも太さ 2）",
            "<g transform=\"scale(1 4)\"><line x1=\"10\" y1=\"12.5\" x2=\"90\" y2=\"12.5\" fill=\"none\" stroke=\"green\" stroke-width=\"2\" vector-effect=\"non-scaling-stroke\"/></g>",
            (50, 53),
            true,
        ),
        stroke(
            "crisp-edges",
            "crispEdges は反エイリアスなし（縁 10.3 の画素 10 は全部塗る）",
            &crisp("crispEdges"),
            (10, 50),
            false,
        ),
        stroke(
            "optimize-speed",
            "optimizeSpeed（crispEdges と同じに扱うか）",
            &crisp("optimizeSpeed"),
            (10, 50),
            false,
        ),
        stroke(
            "geometric-precision",
            "geometricPrecision は反エイリアス（縁の画素は混ざる。どちらでもないが正しい）",
            &crisp("geometricPrecision"),
            (10, 50),
            false,
        ),
        stroke(
            "closed-join",
            "閉じたパスの始めの点も継ぎ目（miter の角を描く）",
            &format!("<path d=\"M30 30 H70 V70 H30 Z\" {g} stroke-width=\"20\"/>"),
            (22, 22),
            false,
        ),
        stroke(
            "paint-order-stroke",
            "paint-order: stroke は塗りを線の上に描く（線の内側の半分は緑の塗り）",
            "<rect x=\"20\" y=\"20\" width=\"60\" height=\"60\" fill=\"green\" stroke=\"red\" stroke-width=\"20\" paint-order=\"stroke\"/>",
            (25, 50),
            false,
        ),
        stroke(
            "stroke-width-zero",
            "太さ 0 の線は描かない",
            &format!("<line x1=\"10\" y1=\"50\" x2=\"90\" y2=\"50\" {g} stroke-width=\"0\"/>"),
            (50, 50),
            true,
        ),
        stroke(
            "dash-closed-join",
            "閉じた形の最初と最後の破線をつなぐ（始めの角を miter の継ぎ目で描く。ADR-92 (1)）",
            "<rect x=\"20\" y=\"20\" width=\"60\" height=\"60\" fill=\"none\" stroke=\"green\" stroke-width=\"20\" stroke-dasharray=\"50 10\" stroke-dashoffset=\"20\"/>",
            (12, 12),
            false,
        ),
        stroke(
            "arc-zero-length",
            "始めと終わりが同じ弧は長さ 0 の線（丸い端の点を描く。ADR-92 (2)）",
            &format!("<path d=\"M50 50 A10 10 0 0 1 50 50\" {g} stroke-width=\"20\" stroke-linecap=\"round\"/>"),
            (50, 50),
            false,
        ),
        stroke(
            "dash-zero-closed",
            "閉じた長さ 0 の部分パス（M z）に破線を付けると描かない（Chrome と同じ）",
            &format!(
                "<path d=\"M50 50 z\" {g} stroke-width=\"20\" stroke-linecap=\"round\" stroke-dasharray=\"5 5\"/>"
            ),
            (50, 50),
            true,
        ),
    ]
}

pub(crate) fn probes() -> Vec<Probe> {
    let lower_left = (30, 70);
    let upper_right = (70, 30);
    let upper_mid = (50, 30);
    vec![
        d("d-basic", "基本の四角（確かめ）", "M10 10 H90 V90 H10 Z", lower_left),
        d("d-implicit-line", "M のあとの座標の組は暗黙の L", "M10 10 90 10 90 90 10 90 Z", lower_left),
        d("d-plus-sign", "数の頭の `+`", "M+10 +10 H+90 V+90 H+10 Z", lower_left),
        d("d-trailing-dot", "末尾の `.` の数（`10.`。SVG 1.1 の文法は受ける）", "M10. 10 H90 V90 H10 Z", lower_left),
        d("d-exp", "指数（`9e1`）", "M10 10 H9e1 V90 H1e1 Z", lower_left),
        d(
            "d-exp-no-digits",
            "数字のない指数（`90e`）を数 90 として続けるか（続けなければ三角）",
            "M10 10 H90 V90e H10 Z",
            lower_left,
        ),
        d("d-compact-flags", "弧の旗の詰め書き（`1190` は旗 1・1 と 90）", "M10 50 A40 40 0 1190 50 Z", upper_mid),
        d("d-flags", "弧の旗を分けて書く（確かめ）", "M10 50 A40 40 0 1 1 90 50 Z", upper_mid),
        d("d-arc-scale", "足りない半径を広げる（半径 1 でも半円）", "M10 50 A1 1 0 0 1 90 50 Z", upper_mid),
        d("d-arc-zero", "半径 0 の弧は線", "M10 10 H90 V90 A0 10 0 0 1 10 90 Z", lower_left),
        d("d-error-partial", "誤り（引数の足りない Q）の前までを描く", "M10 10 H90 V90 H10 Q", lower_left),
        d("d-error-keeps-before", "誤りの前の区間は描く（三角の右上）", "M10 10 H90 V90 L10", upper_right),
        d("d-error-drops-broken", "引数の足りない区間は描かない（左下は描かない）", "M10 10 H90 V90 L10", lower_left),
        d("d-numbers-after-z", "Z のあとの数（暗黙の L として続けるか）", "M10 10 H90 V90 Z 10 90 90 90", lower_left),
        d("d-first-not-m", "M で始まらない（何も描かない）", "L10 10 H90 V90 H10 Z", lower_left),
        d("d-double-comma", "続く `,,`（誤り）", "M10,,10 H90 V90 H10 Z", lower_left),
        d("d-leading-comma", "命令のすぐあとの `,`（誤り）", "M,10 10 H90 V90 H10 Z", lower_left),
        on_red(
            "パスの文法",
            "d-comma-before-command",
            "最後の引数のあとの `,` を受けて次の命令へ続けるか",
            "<path fill=\"green\" d=\"M10 10 H90 V90, H10 Z\"/>",
            lower_left,
        ),
        on_red(
            "パスの文法",
            "d-trailing-dot-mid",
            "途中の末尾の `.`（前の区間は描くか）",
            "<path fill=\"green\" d=\"M10 10 H90 V90 H10. Z\"/>",
            upper_right,
        ),
        on_red(
            "パスの文法",
            "d-css-path",
            "CSS の `d: path()`",
            "<path fill=\"green\" style=\"d: path('M10 10 H90 V90 H10 Z')\"/>",
            lower_left,
        ),
        on_red(
            "パスの文法",
            "d-css-over-attr",
            "CSS の `d` が属性に勝つ",
            "<path fill=\"green\" d=\"M0 0\" style=\"d: path('M10 10 H90 V90 H10 Z')\"/>",
            lower_left,
        ),
        shows_red(
            "rect-rx-auto",
            "rect の `rx` の auto は `ry`（角が丸い）",
            "<rect x=\"10\" y=\"10\" width=\"80\" height=\"80\" ry=\"40\" fill=\"green\"/>",
            (13, 13),
        ),
        shows_red(
            "rect-rx-clamp",
            "rect の `rx` は幅の半分に切り詰める",
            "<rect x=\"10\" y=\"10\" width=\"80\" height=\"80\" rx=\"100\" fill=\"green\"/>",
            (13, 13),
        ),
        on_red(
            "図形",
            "rect-rx-negative",
            "負の `rx` は壊れた値（角は直角）",
            "<rect x=\"10\" y=\"10\" width=\"80\" height=\"80\" rx=\"-20\" fill=\"green\"/>",
            (11, 11),
        ),
        on_red(
            "図形",
            "rect-css-size",
            "CSS の `width`・`height`",
            "<rect x=\"10\" y=\"10\" style=\"width: 80px; height: 80px\" fill=\"green\"/>",
            (50, 50),
        ),
        on_red(
            "図形",
            "rect-percent",
            "% の幅（表示の領域の幅に対して）",
            "<rect x=\"10\" y=\"10\" width=\"80%\" height=\"80%\" fill=\"green\"/>",
            (80, 80),
        ),
        on_red(
            "図形",
            "circle-css-r",
            "CSS の `r`",
            "<circle cx=\"50\" cy=\"50\" style=\"r: 40px\" fill=\"green\"/>",
            (20, 50),
        ),
        on_red(
            "図形",
            "ellipse-rx-auto",
            "ellipse の `rx` の auto は `ry`（円）",
            "<ellipse cx=\"50\" cy=\"50\" ry=\"40\" fill=\"green\"/>",
            (20, 50),
        ),
        on_red(
            "図形",
            "polygon-odd",
            "数が奇数の points は最後を捨てる（三角。左下は描かない）",
            "<polygon points=\"10,10 90,10 90,90 10\" fill=\"green\"/>",
            lower_left,
        ),
        on_red(
            "図形",
            "polygon-odd-keeps",
            "数が奇数の points でも残りの点は描く（三角の右上）",
            "<polygon points=\"10,10 90,10 90,90 10\" fill=\"green\"/>",
            upper_right,
        ),
        on_red(
            "図形",
            "polygon-error",
            "壊れた points は誤りの前まで（三角。左下は描かない）",
            "<polygon points=\"10,10 90,10 90,90 x 10,90\" fill=\"green\"/>",
            lower_left,
        ),
        on_red(
            "図形",
            "polygon-error-keeps",
            "壊れた points の前の点は描く（三角の右上）",
            "<polygon points=\"10,10 90,10 90,90 x 10,90\" fill=\"green\"/>",
            upper_right,
        ),
        on_red(
            "図形",
            "points-trailing-comma",
            "points の終わりの `,` を受ける（三角を描く。ADR-92 (3)）",
            "<polygon points=\"10,10 90,10 90,90,\" fill=\"green\"/>",
            (70, 30),
        ),
        on_red(
            "図形",
            "polyline-fill",
            "polyline も塗る（閉じない形を閉じて塗る）",
            "<polyline points=\"10,10 90,10 90,90 10,90\" fill=\"green\"/>",
            lower_left,
        ),
        on_red(
            "図形",
            "line-no-fill",
            "line は塗らない（線だけ）",
            "<line x1=\"10\" y1=\"10\" x2=\"90\" y2=\"90\" fill=\"green\" stroke=\"none\"/>",
            (50, 50),
        ),
    ]
    .into_iter()
    .chain(stroke_probes())
    .collect()
}
