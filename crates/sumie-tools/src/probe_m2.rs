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
}
