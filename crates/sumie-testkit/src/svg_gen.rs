//! 構造から SVG を組み立てる生成器（ファズの `render_gen` と、種を固定した試験。ADR-100 (3)）。
//!
//! バイト列を選択の並びに読み替え、必ず整った SVG を書き出す（XML として壊れない。値の字句は決まった断片の組み合わせ）。
//! バイト列が尽きたら 0 を読んだものとする（どんな長さでも文書になる）。同じバイト列からは同じ文書。
//!
//! 出すもの：根の svg（幅・高さ・viewBox・preserveAspectRatio・background-color）・`<style>`・rect・circle・ellipse・line・
//! polyline・polygon・path（全命令・弧・極端な数）・g・defs・use・symbol・入れ子の svg・switch。値は transform（極端な倍率・
//! 回転・skew）・塗り・線・不透明度・線の太さ・端・継ぎ目・miterlimit・破線・位相・pathLength・vector-effect・
//! shape-rendering・fill-rule・paint-order・style 属性・CSS の transform・transform-origin・transform-box・% の長さ・
//! 根の transform・長いパス。まだ描かない機能（グラデーション・opacity < 1 など）はまれにしか出さない（描く段まで届く割合を
//! 保つ）。`chance` は上のバイトで真になるので、0 のバイト（libFuzzer がよく入れる）は普通の枝を選ぶ（ファズの作り直しの
//! 2 回目のレビュー）。

use std::fmt::Write as _;

/// 選択の源（バイト列。尽きたら 0）。
struct Src<'a> {
    d: &'a [u8],
    i: usize,
    /// キャンバスの大きさ（座標と大きさをこれに合わせる。描く段まで届き、画素を描く文書の割合を保つ）
    span: f64,
}

impl Src<'_> {
    fn byte(&mut self) -> u8 {
        let b = self.d.get(self.i).copied().unwrap_or(0);
        self.i += 1;
        b
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { usize::from(self.byte()) % n }
    }

    /// 確率およそ k/256 で真（上の k 個のバイトで真。0 のバイトは偽）。
    fn chance(&mut self, k: u8) -> bool {
        u16::from(self.byte()) + u16::from(k) > 255
    }

    fn pick<'b>(&mut self, xs: &'b [&'b str]) -> &'b str {
        xs.get(self.below(xs.len())).copied().unwrap_or("")
    }

    fn done(&self) -> bool {
        self.i >= self.d.len()
    }
}

/// 極端な数の断片。
const EXTREME: &[&str] = &[
    "0",
    "-0",
    "1e-7",
    "-1e-7",
    "1e7",
    "-1e7",
    "1e15",
    "-1e15",
    "3.4e38",
    "1e300",
    "-1e300",
    "0.0001",
    "99999.5",
    "4503599627370496",
    ".5",
    "-.5",
    "1e-300",
    "2147483647",
    // 決まった箱（±2²²）の近く
    "4194304",
    "-4194303.75",
    "4194305",
];

/// キャンバスの中の座標（キャンバスの −5%〜105%。たいていは 4 分の 1 の格子、ときどき細かい端数）か、まれに極端な数。
fn coord(s: &mut Src<'_>) -> String {
    if s.chance(4) {
        return s.pick(EXTREME).to_string();
    }
    // 0 のバイトがキャンバスの中に来るようにずらす
    let v = (f64::from(s.byte().wrapping_add(40)) / 255.0 * 1.1 - 0.05) * s.span;
    let v = (v * 4.0).round() / 4.0;
    if s.chance(48) { format!("{}", v + f64::from(s.byte()) / 1024.0) } else { format!("{v}") }
}

/// 曲線の制御点（極端な数をさらにまれに。キャンバスにまたがる巨大な曲線は見積もりを上限まで回して遅い）。
fn control(s: &mut Src<'_>) -> String {
    if s.chance(2) {
        return s.pick(EXTREME).to_string();
    }
    let v = (f64::from(s.byte()) / 255.0 * 1.4 - 0.2) * s.span;
    format!("{}", (v * 4.0).round() / 4.0)
}

/// 正の大きさ（線の太さ・半径など。キャンバスの 1/2 まで）。
fn size(s: &mut Src<'_>) -> String {
    match s.below(10) {
        9 => s.pick(&["0", "0.3", "0.5", "0.8", "1"]).to_string(),
        8 if s.chance(64) => s.pick(EXTREME).to_string(),
        _ => format!("{}", ((f64::from(s.byte().wrapping_add(40)) / 255.0 * 0.5 * s.span) * 4.0).round() / 4.0 + 0.25),
    }
}

/// 図形の大きさ（幅・高さ・半径）。まれに % を付ける。
fn size_len(s: &mut Src<'_>) -> String {
    let v = size(s);
    if s.chance(30) { format!("{}%", s.byte() % 120) } else { v }
}

/// 線の太さ（ごく細い線から、キャンバスの 1/5 まで。まれに %）。
fn stroke_width(s: &mut Src<'_>) -> String {
    match s.below(8) {
        7 => s.pick(&["0.3", "0.5", "0.8", "1"]).to_string(),
        6 => s.pick(&["0.05", "0.1", "0.2", "0.25"]).to_string(),
        5 if s.chance(80) => format!("{}%", 1 + s.byte() % 10),
        _ => format!("{}", ((f64::from(s.byte()) / 255.0 * 0.2 * s.span) * 4.0).round() / 4.0 + 0.5),
    }
}

/// 長さ（単位か % をまれに付ける）。
fn length(s: &mut Src<'_>) -> String {
    let n = coord(s);
    let unit = if s.chance(40) { s.pick(&["%", "px", "em", "mm", "pt", "in"]) } else { "" };
    format!("{n}{unit}")
}

fn color(s: &mut Src<'_>) -> String {
    match s.below(10) {
        0 => "none".to_string(),
        1 => "currentColor".to_string(),
        2 => format!("#{:02x}{:02x}{:02x}{:02x}", s.byte(), s.byte(), s.byte(), s.byte()),
        3 => format!("rgb({},{},{})", s.byte(), s.byte(), s.byte()),
        4 => "transparent".to_string(),
        // まだ描かない（グラデーションの参照）か、壊れた参照の代わりの色
        5 if s.chance(24) => s.pick(&["url(#grad)", "url(#i0) red", "url(#none) blue"]).to_string(),
        _ => s.pick(&["black", "red", "green", "blue", "lime", "navy", "purple", "orange", "#808080"]).to_string(),
    }
}

fn transform(s: &mut Src<'_>) -> String {
    let mut t = String::new();
    for _ in 0..1 + s.below(3) {
        let part = match s.below(8) {
            0 => format!("translate({} {})", coord(s), coord(s)),
            1 => format!("scale({})", s.pick(&["2", "0.5", "-1", "1e-6", "1e6", "3", "0.25", "1 -1"])),
            2 => format!("scale({} {})", s.pick(&["2", "0.01", "100", "-1"]), s.pick(&["1", "1e-6", "4", "-2"])),
            3 => format!("rotate({})", s.pick(&["30", "45", "90", "-12.5", "180", "1e-9", "359.99"])),
            4 => format!("rotate({} {} {})", s.pick(&["30", "90", "-45"]), coord(s), coord(s)),
            5 => format!("skewX({})", s.pick(&["30", "-45", "89.9", "10"])),
            6 => format!("skewY({})", s.pick(&["30", "-60", "1"])),
            _ => {
                let scale = ["1", "0.5", "2", "-1", "0.25", "4", "1e-6", "-0.5"];
                let shear = ["0", "0", "0.2", "-0.3", "1", "-1"];
                format!(
                    "matrix({} {} {} {} {} {})",
                    s.pick(&scale),
                    s.pick(&shear),
                    s.pick(&shear),
                    s.pick(&scale),
                    coord(s),
                    coord(s)
                )
            }
        };
        t.push_str(&part);
        t.push(' ');
    }
    t.trim_end().to_string()
}

fn dasharray(s: &mut Src<'_>) -> String {
    match s.below(6) {
        0 => "none".to_string(),
        1 => s.pick(&["0.0005 0.000501", "1e-6", "0 1", "1 0", "-1 2"]).to_string(),
        2 => format!("{}%", s.byte() % 50),
        _ => {
            let n = 1 + s.below(5);
            (0..n).map(|_| format!("{}", f64::from(s.byte() % 20) * 0.5)).collect::<Vec<_>>().join(" ")
        }
    }
}

/// 線と塗りなどの属性（プレゼンテーション属性か style 属性の宣言として）。
fn paint_props(s: &mut Src<'_>) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let pool: &[&str] = &[
        "fill",
        "stroke",
        "stroke-width",
        "stroke-linecap",
        "stroke-linejoin",
        "stroke-miterlimit",
        "stroke-dasharray",
        "stroke-dashoffset",
        "vector-effect",
        "shape-rendering",
        "fill-rule",
        "fill-opacity",
        "stroke-opacity",
        "opacity",
        "color",
        "visibility",
        "display",
        "paint-order",
        "transform-origin",
    ];
    for _ in 0..s.below(6) {
        let name = s.pick(pool);
        let value = match name {
            "fill" | "stroke" => color(s),
            "color" => s.pick(&["red", "blue", "#0f0"]).to_string(),
            "stroke-width" => size(s),
            "stroke-linecap" => s.pick(&["butt", "round", "square"]).to_string(),
            "stroke-linejoin" => s.pick(&["miter", "round", "bevel", "miter-clip", "arcs"]).to_string(),
            "stroke-miterlimit" => s.pick(&["1", "4", "0.5", "10", "1e6"]).to_string(),
            "stroke-dasharray" => dasharray(s),
            "stroke-dashoffset" => coord(s),
            "vector-effect" => s.pick(&["non-scaling-stroke", "none"]).to_string(),
            "shape-rendering" => s.pick(&["crispEdges", "optimizeSpeed", "geometricPrecision", "auto"]).to_string(),
            "fill-rule" => s.pick(&["evenodd", "nonzero"]).to_string(),
            "fill-opacity" | "stroke-opacity" => s.pick(&["0.5", "1", "0", "0.25"]).to_string(),
            // 1 未満はまだ描かない（NotYet）ので、まれに
            "opacity" => {
                if s.chance(4) {
                    "0.5".to_string()
                } else {
                    "1".to_string()
                }
            }
            "visibility" => s.pick(&["visible", "hidden", "visible"]).to_string(),
            "paint-order" => s.pick(&["stroke", "normal", "markers stroke", "fill stroke", "stroke fill"]).to_string(),
            "transform-origin" => s.pick(&["center", "50% 50%", "10px 20px", "left top", "0 0"]).to_string(),
            _ => s.pick(&["inline", "inline", "inline", "none"]).to_string(),
        };
        // 同じ属性を 2 度書くと XML として壊れるので、名前の重なりは捨てる
        if !v.iter().any(|(n, _)| n == name) {
            v.push((name.to_string(), value));
        }
    }
    v
}

fn path_data(s: &mut Src<'_>) -> String {
    let mut d = format!("M{} {}", coord(s), coord(s));
    // たいていは 12 区間まで、ときどき長いパス
    let n = if s.chance(20) { 40 + s.below(60) } else { s.below(12) };
    for _ in 0..n {
        let cmd = s.pick(&[
            "L", "l", "H", "h", "V", "v", "C", "c", "S", "s", "Q", "q", "T", "t", "A", "a", "Z", "z", "M", "m",
        ]);
        let args = match cmd {
            "H" | "h" | "V" | "v" => coord(s),
            "L" | "l" | "T" | "t" | "M" | "m" => format!("{} {}", coord(s), coord(s)),
            "C" | "c" => (0..6).map(|k| if k < 4 { control(s) } else { coord(s) }).collect::<Vec<_>>().join(" "),
            "S" | "s" | "Q" | "q" => {
                (0..4).map(|k| if k < 2 { control(s) } else { coord(s) }).collect::<Vec<_>>().join(" ")
            }
            "A" | "a" => format!(
                "{} {} {} {} {} {} {}",
                size(s),
                size(s),
                s.pick(&["0", "30", "45", "90", "-20"]),
                s.below(2),
                s.below(2),
                coord(s),
                coord(s)
            ),
            _ => String::new(),
        };
        d.push(' ');
        d.push_str(cmd);
        d.push_str(&args);
    }
    d
}

fn points(s: &mut Src<'_>) -> String {
    let n = if s.chance(20) { 20 + s.below(60) } else { 1 + s.below(8) };
    (0..n).map(|_| format!("{},{}", coord(s), coord(s))).collect::<Vec<_>>().join(" ")
}

/// 生成の状態（付けた id の数・深さ・要素の数）。
struct Gen<'a, 'b> {
    s: &'b mut Src<'a>,
    ids: usize,
    count: usize,
    out: String,
}

impl Gen<'_, '_> {
    fn attr(&mut self, name: &str, value: &str) {
        let _ = write!(self.out, " {name}=\"{value}\"");
    }

    fn common(&mut self, shape: bool) {
        if self.s.chance(80) {
            let id = self.ids;
            self.ids += 1;
            self.attr("id", &format!("i{id}"));
        }
        if self.s.chance(40) {
            let c = self.s.pick(&["a", "b"]).to_string();
            self.attr("class", &c);
        }
        if self.s.chance(60) {
            let t = transform(self.s);
            self.attr("transform", &t);
        }
        let mut props = paint_props(self.s);
        // 図形のおよそ半分に線（色と太さ）を付け、線があれば破線を高い割合で（描く段の線と破線まで届かせる）
        if shape && self.s.chance(128) {
            let mut put = |n: &str, v: String| {
                props.retain(|(m, _)| m != n);
                props.push((n.to_string(), v));
            };
            let c = self.s.pick(&["black", "red", "navy", "#0a0", "rgb(200,100,0)"]).to_string();
            put("stroke", c);
            put("stroke-width", stroke_width(self.s));
            if self.s.chance(140) {
                let d = (0..1 + self.s.below(4))
                    .map(|_| format!("{}", f64::from(1 + self.s.byte() % 24) * 0.5))
                    .collect::<Vec<_>>()
                    .join(" ");
                put("stroke-dasharray", d);
                if self.s.chance(100) {
                    put("stroke-dashoffset", coord(self.s));
                }
            }
            if self.s.chance(100) {
                let j = self.s.pick(&["round", "bevel", "miter", "miter-clip"]).to_string();
                put("stroke-linejoin", j);
                let c = self.s.pick(&["round", "square", "butt"]).to_string();
                put("stroke-linecap", c);
                if self.s.chance(80) {
                    let m = self.s.pick(&["1", "1.5", "4", "10", "0.5"]).to_string();
                    put("stroke-miterlimit", m);
                }
            }
        }
        // CSS のプロパティとしてだけ書く宣言（変形と、その原点と箱）
        let mut css: Vec<(String, String)> = Vec::new();
        if self.s.chance(24) {
            let t = self.s.pick(&["rotate(30deg)", "translate(5px, 3px)", "scale(2)", "skewX(10deg)", "none"]);
            css.push(("transform".to_string(), t.to_string()));
        }
        // 外接矩形の要る箱はまだ描かない（NotYet）ので、まれに
        if self.s.chance(6) {
            let b = self.s.pick(&["view-box", "view-box", "fill-box", "stroke-box"]);
            css.push(("transform-box".to_string(), b.to_string()));
        }
        if self.s.chance(64) {
            let decls: Vec<String> = props.iter().chain(&css).map(|(n, v)| format!("{n}: {v}")).collect();
            self.attr("style", &decls.join("; "));
        } else {
            for (n, v) in &props {
                self.attr(n, v);
            }
            if !css.is_empty() {
                let decls: Vec<String> = css.iter().map(|(n, v)| format!("{n}: {v}")).collect();
                self.attr("style", &decls.join("; "));
            }
        }
        if shape && self.s.chance(24) {
            let p = self.s.pick(&["10", "0", "100", "1e-9", "-5"]).to_string();
            self.attr("pathLength", &p);
        }
    }

    fn shape(&mut self) {
        let kind = self.s.pick(&["rect", "circle", "ellipse", "line", "polyline", "polygon", "path", "path"]);
        let _ = write!(self.out, "<{kind}");
        match kind {
            "rect" => {
                for a in ["x", "y"] {
                    let v = length(self.s);
                    self.attr(a, &v);
                }
                for a in ["width", "height"] {
                    let v = size_len(self.s);
                    self.attr(a, &v);
                }
                if self.s.chance(80) {
                    let v = size_len(self.s);
                    self.attr("rx", &v);
                }
                if self.s.chance(60) {
                    let v = self.s.pick(&["auto", "5", "-1", "1e9"]).to_string();
                    self.attr("ry", &v);
                }
            }
            "circle" => {
                for a in ["cx", "cy"] {
                    let v = length(self.s);
                    self.attr(a, &v);
                }
                let v = size_len(self.s);
                self.attr("r", &v);
            }
            "ellipse" => {
                for a in ["cx", "cy"] {
                    let v = length(self.s);
                    self.attr(a, &v);
                }
                for a in ["rx", "ry"] {
                    let v = if self.s.chance(30) { "auto".to_string() } else { size_len(self.s) };
                    self.attr(a, &v);
                }
            }
            "line" => {
                for a in ["x1", "y1", "x2", "y2"] {
                    let v = length(self.s);
                    self.attr(a, &v);
                }
            }
            "polyline" | "polygon" => {
                let v = points(self.s);
                self.attr("points", &v);
            }
            _ => {
                let v = path_data(self.s);
                self.attr("d", &v);
            }
        }
        self.common(true);
        self.out.push_str("/>");
    }

    /// 要素を 1 つ（子を持つものは深さ `depth` まで）。
    fn element(&mut self, depth: usize) {
        self.count += 1;
        let kind = if depth >= 4 || self.count > 24 { 0 } else { self.s.below(12) };
        match kind {
            0..=5 => self.shape(),
            6 if self.ids > 0 => {
                let target = self.s.below(self.ids + 1);
                let href = if self.s.chance(64) { "xlink:href" } else { "href" };
                let _ = write!(self.out, "<use {href}=\"#i{target}\"");
                for a in ["x", "y"] {
                    let v = length(self.s);
                    self.attr(a, &v);
                }
                if self.s.chance(60) {
                    for a in ["width", "height"] {
                        let v = size(self.s);
                        self.attr(a, &v);
                    }
                }
                self.common(false);
                self.out.push_str("/>");
            }
            7 => self.container("defs", depth),
            8 => self.container("symbol", depth),
            9 => self.container("svg", depth),
            10 => self.container("switch", depth),
            _ => self.container("g", depth),
        }
    }

    fn container(&mut self, kind: &str, depth: usize) {
        let _ = write!(self.out, "<{kind}");
        match kind {
            "symbol" | "svg" => {
                if self.s.chance(160) {
                    let vb = format!("{} {} {} {}", coord(self.s), coord(self.s), size(self.s), size(self.s));
                    self.attr("viewBox", &vb);
                    if self.s.chance(100) {
                        let par =
                            self.s.pick(&["none", "xMidYMid meet", "xMinYMax slice", "xMaxYMin meet"]).to_string();
                        self.attr("preserveAspectRatio", &par);
                    }
                }
                // 見えない overflow の入れ子の svg・symbol はまだ描かない（NotYet）ので、たいていは visible
                let o = if self.s.chance(10) {
                    "hidden"
                } else if self.s.chance(64) {
                    "auto"
                } else {
                    "visible"
                }
                .to_string();
                self.attr("overflow", &o);
                if kind == "svg" {
                    for a in ["x", "y", "width", "height"] {
                        let v = length(self.s);
                        self.attr(a, &v);
                    }
                }
            }
            "switch" => {}
            _ => {}
        }
        self.common(false);
        self.out.push('>');
        for _ in 0..1 + self.s.below(4) {
            if kind == "switch" && self.s.chance(80) {
                // 条件つきの子
                let _ = write!(self.out, "<g systemLanguage=\"{}\">", self.s.pick(&["en", "ja", "x-none"]));
                self.shape();
                self.out.push_str("</g>");
            } else {
                self.element(depth + 1);
            }
        }
        let _ = write!(self.out, "</{kind}>");
    }
}

/// バイト列から、整った SVG のバイト列を作る（キャンバスは 128 × 128 以内）。
pub fn svg(data: &[u8]) -> Vec<u8> {
    let mut src = Src { d: data, i: 0, span: 100.0 };
    let mut g = Gen { s: &mut src, ids: 0, count: 0, out: String::new() };
    let wn = *[16u32, 32, 64, 100, 128, 50, 20].get(g.s.below(7)).unwrap_or(&64);
    let hn = *[16u32, 32, 64, 100, 128, 40, 25].get(g.s.below(7)).unwrap_or(&64);
    // 座標と大きさは短い辺に合わせる（長い辺に合わせると、細長いキャンバスで図形が短い辺の外に出る）
    g.s.span = f64::from(wn.min(hn));
    // 根の大きさ：たいていは px、まれに % か書かない（viewBox か中身で決まる大きさ。ADR-58）
    let size_attrs = match g.s.below(10) {
        0 => String::new(),
        1 => " width=\"100%\" height=\"100%\"".to_string(),
        _ => format!(" width=\"{wn}\" height=\"{hn}\""),
    };
    let _ = write!(
        g.out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"{size_attrs}"
    );
    if g.s.chance(100) {
        // 原点は 0 の近く、大きさはキャンバスの 0.5〜2 倍（中身がキャンバスに入るように）
        let k = *[0.5f64, 1.0, 1.0, 2.0].get(g.s.below(4)).unwrap_or(&1.0);
        let (ox, oy) = (f64::from(g.s.byte() % 9) - 4.0, f64::from(g.s.byte() % 9) - 4.0);
        let vb = format!("{ox} {oy} {} {}", f64::from(wn) * k, f64::from(hn) * k);
        g.attr("viewBox", &vb);
        let par = g.s.pick(&["none", "xMidYMid meet", "xMinYMax slice", "xMaxYMin meet"]).to_string();
        g.attr("preserveAspectRatio", &par);
    }
    if g.s.chance(30) {
        // 根の変形（SVG 2）
        let t = transform(g.s);
        g.attr("transform", &t);
    }
    if g.s.chance(40) {
        let c = color(g.s);
        let style = if g.s.chance(128) { format!("background-color: {c}") } else { format!("background: {c}") };
        g.attr("style", &style);
    }
    g.out.push('>');
    if g.s.chance(24) {
        // まだ描かない塗り（グラデーション）の参照先
        g.out.push_str(
            "<defs><linearGradient id=\"grad\"><stop offset=\"0\" stop-color=\"red\"/></linearGradient></defs>",
        );
    }
    if g.s.chance(64) {
        g.out.push_str("<style>");
        for _ in 0..1 + g.s.below(3) {
            let sel =
                g.s.pick(&["rect", "circle", "path", ".a", ".b", "#i1", "g > *", ":first-child", "*"]).to_string();
            let props = paint_props(g.s);
            let decls: Vec<String> = props.iter().map(|(n, v)| format!("{n}: {v}")).collect();
            let _ = write!(g.out, "{sel} {{ {} }}", decls.join("; "));
        }
        g.out.push_str("</style>");
    }
    // 根の直下に図形を 1 つは置く（defs・symbol の中だけの、何も描かない文書ばかりにしない）
    g.count += 1;
    g.shape();
    while g.count < 24 {
        g.element(0);
        if g.s.done() || g.s.chance(40) {
            break;
        }
    }
    g.out.push_str("</svg>");
    g.out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_well_formed() {
        let mut rng = crate::chacha::ChaCha::from_u64(7);
        for _ in 0..200 {
            let mut data = vec![0u8; 400];
            rng.fill(&mut data);
            let a = svg(&data);
            assert_eq!(a, svg(&data));
            let text = String::from_utf8(a).unwrap();
            assert!(text.starts_with("<svg ") && text.ends_with("</svg>"), "{text}");
            // 1 つの要素の属性の名前が重ならない（重なると XML として壊れる）
            for tag in text.split('<').filter(|t| !t.starts_with('/') && !t.is_empty()) {
                let head = tag.split('>').next().unwrap_or("");
                let mut names: Vec<&str> = head
                    .split('"')
                    .step_by(2)
                    .filter_map(|p| p.trim().rsplit(' ').next())
                    .filter(|n| n.ends_with('='))
                    .collect();
                let before = names.len();
                names.sort_unstable();
                names.dedup();
                assert_eq!(names.len(), before, "属性が重なる：{head}");
            }
            // 開くタグと閉じるタグの数が合う（子を持つ要素）
            for tag in ["g", "defs", "symbol", "switch", "style"] {
                assert_eq!(
                    text.matches(&format!("<{tag}")).count(),
                    text.matches(&format!("</{tag}>")).count(),
                    "{tag}：{text}"
                );
            }
        }
        // 空でも文書
        assert!(svg(&[]).starts_with(b"<svg "));
    }
}
