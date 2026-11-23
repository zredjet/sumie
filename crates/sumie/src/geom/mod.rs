//! 層 1：幾何の型（architecture.md §6.1・ADR-30・ADR-38）。変形・点・矩形・パス（命令と座標の置き場）・弧・図形のパス・
//! 厳密な外接矩形。字句を知らない（パスのデータの読みは `values`）。
//!
//! - パスはユーザー単位の `f64` で持つ。命令は `MOVE`・`LINE`・`QUAD`・`CUBIC`・`ARC`・`CLOSE` で、命令ごとに決まった数の
//!   座標を持つ（[`coords_of`]）。命令と座標は別の並びに詰める（木の幾何の置き場をそのまま使う。use で展開した節は同じ
//!   範囲を指す）
//! - 弧は 3 次曲線に分けず、中心と 2 × 2 の行列（x 軸の回転と半径を掛けたもの）・始めの角・回る角・終わりの点で持つ
//!   （SVG 1.1 付録 F.6.5 の式で中心の形に直す。半径が足りなければ F.6.6 のとおり広げる）。平坦化は角を均等に分けて点を
//!   求める（変形の下でもそのまま楕円の弧なので、どの倍率でも 3 次曲線の近似の誤差がない）。終わりの点は書いたとおりの点
//! - 図形は SVG 2 の同じ形のパス（rect は右上へ向かう角から時計回り、円と楕円は `(cx + rx, cy)` から時計回り）。角の弧は
//!   中心の形で直に作る（端点の形からの変換の丸めがない）
//! - 計算で有限でない値が出たら、その弧は線にする（数の大きさの上限を持たない。ADR-32）

#![allow(dead_code, reason = "試験と M3 の外接矩形（objectBoundingBox）から使う口がある")]

pub(crate) mod flatten;
pub(crate) mod stroke;

use crate::error::Error;
use crate::math;
use crate::mem::{MVec, Plain};

/// 2 × 3 の変形の行列（`[a c e; b d f]`）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Transform {
    pub(crate) a: f64,
    pub(crate) b: f64,
    pub(crate) c: f64,
    pub(crate) d: f64,
    pub(crate) e: f64,
    pub(crate) f: f64,
}

impl Plain for Transform {}

impl Transform {
    pub(crate) const IDENTITY: Transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// `self × o`（`o` を先に当てる）。
    pub(crate) fn then(&self, o: &Transform) -> Transform {
        Transform {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }

    pub(crate) fn finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f].iter().all(|v| v.is_finite())
    }

    /// 点を写す。
    pub(crate) fn apply(&self, p: Point) -> Point {
        Point { x: self.a * p.x + self.c * p.y + self.e, y: self.b * p.x + self.d * p.y + self.f }
    }

    /// 向き（平行移動なし）を写す。
    pub(crate) fn apply_vec(&self, p: Point) -> Point {
        Point { x: self.a * p.x + self.c * p.y, y: self.b * p.x + self.d * p.y }
    }

    /// 行列式。
    pub(crate) fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// 一様な倍率の `s` 倍（描く座標系 = ユーザー単位 × 倍率。FR-OUT-03）。
    pub(crate) fn scaled(&self, s: f64) -> Transform {
        Transform { a: self.a * s, b: self.b * s, c: self.c * s, d: self.d * s, e: self.e * s, f: self.f * s }
    }

    /// 線形の部分の最大の特異値（単位の向きを最も伸ばす倍率）。有限でなければ ∞。
    pub(crate) fn max_scale(&self) -> f64 {
        max_singular(self.a, self.c, self.b, self.d)
    }
}

/// 2 × 2 の行列 `[a b; c d]` の最大の特異値（`√((q + √(q² − 4Δ²)) / 2)`。q は成分の 2 乗の和、Δ は行列式）。有限で
/// なければ ∞。
pub(crate) fn max_singular(a: f64, b: f64, c: f64, d: f64) -> f64 {
    // 最大の成分の大きさで割ってから求める（2 乗があふれない。単位の 1 回目のレビュー）
    let m = math::max(math::max(a.abs(), b.abs()), math::max(c.abs(), d.abs()));
    if !m.is_finite() {
        return f64::INFINITY;
    }
    if m == 0.0 {
        return 0.0;
    }
    let (a, b, c, d) = (a / m, b / m, c / m, d / m);
    let q = a * a + b * b + c * c + d * d;
    let det = a * d - b * c;
    let disc = q * q - 4.0 * det * det;
    let v = (q + math::max(disc, 0.0).sqrt()) / 2.0;
    let s = math::max(v, 0.0).sqrt() * m;
    if s.is_finite() { s } else { f64::INFINITY }
}

/// 点（ユーザー単位か描く座標系）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Point {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

impl Plain for Point {}

impl Point {
    pub(crate) const fn new(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    pub(crate) fn finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// 軸に沿った矩形（`x0 ≤ x1`・`y0 ≤ y1`。空の矩形は [`Rect::EMPTY`]）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x0: f64,
    pub(crate) y0: f64,
    pub(crate) x1: f64,
    pub(crate) y1: f64,
}

impl Plain for Rect {}

impl Rect {
    /// 何も含まない矩形（点を足すと、その点だけの矩形になる）。
    pub(crate) const EMPTY: Rect =
        Rect { x0: f64::INFINITY, y0: f64::INFINITY, x1: f64::NEG_INFINITY, y1: f64::NEG_INFINITY };

    pub(crate) fn is_empty(&self) -> bool {
        !(self.x0 <= self.x1 && self.y0 <= self.y1)
    }

    /// 点を含むように広げる。
    pub(crate) fn add(&mut self, p: Point) {
        self.x0 = math::min(self.x0, p.x);
        self.y0 = math::min(self.y0, p.y);
        self.x1 = math::max(self.x1, p.x);
        self.y1 = math::max(self.y1, p.y);
    }

    /// 2 つを含む矩形。
    pub(crate) fn union(&self, o: &Rect) -> Rect {
        if o.is_empty() {
            return *self;
        }
        if self.is_empty() {
            return *o;
        }
        Rect {
            x0: math::min(self.x0, o.x0),
            y0: math::min(self.y0, o.y0),
            x1: math::max(self.x1, o.x1),
            y1: math::max(self.y1, o.y1),
        }
    }

    /// 四隅を写した矩形を含む矩形。
    pub(crate) fn transformed(&self, t: &Transform) -> Rect {
        if self.is_empty() {
            return Rect::EMPTY;
        }
        let mut r = Rect::EMPTY;
        for p in [
            Point::new(self.x0, self.y0),
            Point::new(self.x1, self.y0),
            Point::new(self.x1, self.y1),
            Point::new(self.x0, self.y1),
        ] {
            r.add(t.apply(p));
        }
        r
    }
}

/// パスの命令。
pub(crate) const MOVE: u8 = 0;
pub(crate) const LINE: u8 = 1;
pub(crate) const QUAD: u8 = 2;
pub(crate) const CUBIC: u8 = 3;
pub(crate) const ARC: u8 = 4;
pub(crate) const CLOSE: u8 = 5;

/// 命令が持つ座標（`f64`）の数。知らない命令は 0。
pub(crate) const fn coords_of(verb: u8) -> usize {
    match verb {
        MOVE | LINE => 2,
        QUAD => 4,
        CUBIC => 6,
        ARC => 10,
        _ => 0,
    }
}

/// 楕円の弧（中心の形）。点は `c + M · (cos θ, sin θ)`（`M = [m0 m1; m2 m3]`）、θ は `theta1` から `theta1 + dtheta` まで。
/// 終わりの点は `end`（書いたとおりの点。計算した点でない）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ArcSeg {
    pub(crate) c: Point,
    pub(crate) m: [f64; 4],
    pub(crate) theta1: f64,
    pub(crate) dtheta: f64,
    pub(crate) end: Point,
}

impl ArcSeg {
    /// 角 θ の点（中心からじかに求める。中心が遠いと桁が落ちるので、弧の上の点は [`ArcSeg::at_from`] で求める）。
    pub(crate) fn at(&self, theta: f64) -> Point {
        let (s, co) = (math::sin(theta), math::cos(theta));
        let [m0, m1, m2, m3] = self.m;
        Point::new(self.c.x + m0 * co + m1 * s, self.c.y + m2 * co + m3 * s)
    }

    /// 角 θ の点を、始めの点 `from`（角 `theta1` の点）からの差で求める（和積の形：`cos θ − cos θ1 = −2 sin((θ+θ1)/2)
    /// sin((θ−θ1)/2)`・`sin θ − sin θ1 = 2 cos((θ+θ1)/2) sin((θ−θ1)/2)`。半径が座標より桁違いに大きくても桁が落ちない。
    /// 単位 1 の 2 回目のレビュー）。
    pub(crate) fn at_from(&self, from: Point, theta: f64) -> Point {
        let h = (theta - self.theta1) / 2.0;
        let mid = (theta + self.theta1) / 2.0;
        let sh = math::sin(h);
        let (dc, ds) = (-2.0 * math::sin(mid) * sh, 2.0 * math::cos(mid) * sh);
        let [m0, m1, m2, m3] = self.m;
        Point::new(from.x + (m0 * dc + m1 * ds), from.y + (m2 * dc + m3 * ds))
    }

    /// 変形した弧（楕円の弧は変形の下でも楕円の弧）。
    pub(crate) fn transformed(&self, t: &Transform) -> ArcSeg {
        let [m0, m1, m2, m3] = self.m;
        ArcSeg {
            c: t.apply(self.c),
            m: [t.a * m0 + t.c * m2, t.a * m1 + t.c * m3, t.b * m0 + t.d * m2, t.b * m1 + t.d * m3],
            theta1: self.theta1,
            dtheta: self.dtheta,
            end: t.apply(self.end),
        }
    }

    /// 楕円の最大の半径（行列の最大の特異値）。
    pub(crate) fn max_radius(&self) -> f64 {
        let [m0, m1, m2, m3] = self.m;
        max_singular(m0, m1, m2, m3)
    }

    fn to_coords(self) -> [f64; 10] {
        let [m0, m1, m2, m3] = self.m;
        [self.c.x, self.c.y, m0, m1, m2, m3, self.theta1, self.dtheta, self.end.x, self.end.y]
    }

    fn from_coords(c: &[f64]) -> ArcSeg {
        let g = |i: usize| c.get(i).copied().unwrap_or(0.0);
        ArcSeg {
            c: Point::new(g(0), g(1)),
            m: [g(2), g(3), g(4), g(5)],
            theta1: g(6),
            dtheta: g(7),
            end: Point::new(g(8), g(9)),
        }
    }
}

/// パスの区間（始めの点つき）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PathSeg {
    /// 部分パスの始まり
    Move(Point),
    Line(Point, Point),
    Quad(Point, Point, Point),
    Cubic(Point, Point, Point, Point),
    /// 始めの点と弧
    ArcTo(Point, ArcSeg),
    /// 今の点から部分パスの始めの点へ閉じる
    Close(Point, Point),
}

/// 置き場の中のパスを読む口（命令と座標の並び）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PathRef<'a> {
    pub(crate) verbs: &'a [u8],
    pub(crate) coords: &'a [f64],
}

impl<'a> PathRef<'a> {
    pub(crate) const EMPTY: PathRef<'static> = PathRef { verbs: &[], coords: &[] };

    pub(crate) fn segments(&self) -> Segments<'a> {
        Segments { path: *self, verb: 0, coord: 0, cur: Point::new(0.0, 0.0), start: Point::new(0.0, 0.0) }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }
}

/// [`PathRef::segments`] の歩き。
pub(crate) struct Segments<'a> {
    path: PathRef<'a>,
    verb: usize,
    coord: usize,
    cur: Point,
    start: Point,
}

impl Iterator for Segments<'_> {
    type Item = PathSeg;

    fn next(&mut self) -> Option<PathSeg> {
        let &v = self.path.verbs.get(self.verb)?;
        self.verb = self.verb.saturating_add(1);
        let n = coords_of(v);
        let at = self.coord;
        self.coord = self.coord.saturating_add(n);
        let c = self.path.coords.get(at..self.coord).unwrap_or(&[]);
        let p =
            |i: usize| Point::new(c.get(i).copied().unwrap_or(0.0), c.get(i.saturating_add(1)).copied().unwrap_or(0.0));
        let from = self.cur;
        let seg = match v {
            MOVE => {
                self.start = p(0);
                PathSeg::Move(p(0))
            }
            LINE => PathSeg::Line(from, p(0)),
            QUAD => PathSeg::Quad(from, p(0), p(2)),
            CUBIC => PathSeg::Cubic(from, p(0), p(2), p(4)),
            ARC => PathSeg::ArcTo(from, ArcSeg::from_coords(c)),
            _ => PathSeg::Close(from, self.start),
        };
        self.cur = match seg {
            PathSeg::Move(q) | PathSeg::Line(_, q) | PathSeg::Quad(_, _, q) | PathSeg::Cubic(_, _, _, q) => q,
            PathSeg::ArcTo(_, a) => a.end,
            PathSeg::Close(_, s) => s,
        };
        Some(seg)
    }
}

/// パスを置き場に書く口（命令と座標の並びの後ろに足す）。今の点と部分パスの始めの点を持つ。
pub(crate) struct PathWriter<'v, 'b> {
    pub(crate) verbs: &'v mut MVec<'b, u8>,
    pub(crate) coords: &'v mut MVec<'b, f64>,
    cur: Point,
    start: Point,
    /// 部分パスが始まっているか（MOVE を書いたか）
    open: bool,
}

impl<'v, 'b> PathWriter<'v, 'b> {
    pub(crate) fn new(verbs: &'v mut MVec<'b, u8>, coords: &'v mut MVec<'b, f64>) -> PathWriter<'v, 'b> {
        PathWriter { verbs, coords, cur: Point::new(0.0, 0.0), start: Point::new(0.0, 0.0), open: false }
    }

    pub(crate) fn current(&self) -> Point {
        self.cur
    }

    pub(crate) fn subpath_start(&self) -> Point {
        self.start
    }

    fn put(&mut self, verb: u8, c: &[f64]) -> Result<(), Error> {
        self.verbs.push(verb)?;
        self.coords.append_slice(c)
    }

    pub(crate) fn move_to(&mut self, p: Point) -> Result<(), Error> {
        self.put(MOVE, &[p.x, p.y])?;
        self.cur = p;
        self.start = p;
        self.open = true;
        Ok(())
    }

    pub(crate) fn line_to(&mut self, p: Point) -> Result<(), Error> {
        self.put(LINE, &[p.x, p.y])?;
        self.cur = p;
        Ok(())
    }

    pub(crate) fn quad_to(&mut self, c: Point, p: Point) -> Result<(), Error> {
        self.put(QUAD, &[c.x, c.y, p.x, p.y])?;
        self.cur = p;
        Ok(())
    }

    pub(crate) fn cubic_to(&mut self, c1: Point, c2: Point, p: Point) -> Result<(), Error> {
        self.put(CUBIC, &[c1.x, c1.y, c2.x, c2.y, p.x, p.y])?;
        self.cur = p;
        Ok(())
    }

    /// 中心の形の弧を足す。
    pub(crate) fn arc(&mut self, a: ArcSeg) -> Result<(), Error> {
        self.put(ARC, &a.to_coords())?;
        self.cur = a.end;
        Ok(())
    }

    /// 端点の形の弧（SVG のパスの `A`。`phi` は x 軸の回転の度）を足す。始めと終わりが同じなら長さ 0 の線、半径の片方が
    /// 0 か、計算が有限でなければ線にする（SVG 1.1 付録 F.6.2・F.6.5・F.6.6）。
    pub(crate) fn arc_to(
        &mut self,
        rx: f64,
        ry: f64,
        phi: f64,
        large: bool,
        sweep: bool,
        p: Point,
    ) -> Result<(), Error> {
        match endpoint_to_center(self.cur, rx, ry, phi, large, sweep, p) {
            ArcKind::Line => self.line_to(p),
            ArcKind::Curve(a) => self.arc(a),
        }
    }

    pub(crate) fn close(&mut self) -> Result<(), Error> {
        self.put(CLOSE, &[])?;
        self.cur = self.start;
        Ok(())
    }

    /// 部分パスが始まっているか。
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }
}

/// 端点の形の弧を直した結果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ArcKind {
    /// 線にする
    Line,
    Curve(ArcSeg),
}

/// SVG 1.1 付録 F.6.5（端点の形から中心の形へ）と F.6.6（半径を広げる）。始めと終わりが同じなら長さ 0 の線（resvg・Chrome と
/// 同じ。ADR-92 (2)）。半径で割った形（`xn = x1'/rx`・`yn = y1'/ry`・`s = |(xn, yn)|`）で求めるので、半径の 2 乗を作らない：
/// `s > 1` なら半径を `s` 倍に広げて中心は真ん中、そうでなければ `coef = ±√(1/s² − 1)`。有限でなければ線にする。
pub(crate) fn endpoint_to_center(
    p1: Point,
    rx: f64,
    ry: f64,
    phi_deg: f64,
    large: bool,
    sweep: bool,
    p2: Point,
) -> ArcKind {
    if p1 == p2 {
        return ArcKind::Line;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 {
        return ArcKind::Line;
    }
    let phi = math::rem_euclid(phi_deg, 360.0) * math::DEG_TO_RAD;
    let (sp, cp) = (math::sin(phi), math::cos(phi));
    // 中点の差を回した座標（F.6.5.1。差は半分にしてから）
    let dx = p1.x / 2.0 - p2.x / 2.0;
    let dy = p1.y / 2.0 - p2.y / 2.0;
    let x1 = cp * dx + sp * dy;
    let y1 = -sp * dx + cp * dy;
    let (mut xn, mut yn) = (x1 / rx, y1 / ry);
    let s = math::hypot(xn, yn);
    let mut coef = 0.0;
    if s > 1.0 {
        // 半径が足りない：広げて中心は真ん中（F.6.6.2）
        rx *= s;
        ry *= s;
        xn /= s;
        yn /= s;
    } else if s > 0.0 {
        coef = math::max(1.0 / (s * s) - 1.0, 0.0).sqrt();
        if large == sweep {
            coef = -coef;
        }
    }
    // 中心（F.6.5.2・F.6.5.3）
    let cx1 = coef * rx * yn;
    let cy1 = -coef * ry * xn;
    let cx = cp * cx1 - sp * cy1 + (p1.x / 2.0 + p2.x / 2.0);
    let cy = sp * cx1 + cp * cy1 + (p1.y / 2.0 + p2.y / 2.0);
    // 角（F.6.5.5・F.6.5.6。半径で割った形）
    let ux = xn - coef * yn;
    let uy = yn + coef * xn;
    let vx = -xn - coef * yn;
    let vy = -yn + coef * xn;
    let theta1 = math::atan2(uy, ux);
    let mut dtheta = math::atan2(ux * vy - uy * vx, ux * vx + uy * vy);
    let tau = 2.0 * core::f64::consts::PI;
    if !sweep && dtheta > 0.0 {
        dtheta -= tau;
    } else if sweep && dtheta < 0.0 {
        dtheta += tau;
    }
    let a = ArcSeg { c: Point::new(cx, cy), m: [cp * rx, -sp * ry, sp * rx, cp * ry], theta1, dtheta, end: p2 };
    let ok =
        a.c.finite() && a.m.iter().all(|v| v.is_finite()) && theta1.is_finite() && dtheta.is_finite() && dtheta != 0.0;
    if ok { ArcKind::Curve(a) } else { ArcKind::Line }
}

/// 中心 `c`・半径 `rx`・`ry`（軸に沿う）の、角 `from` から `to` まで（ラジアン。`to > from` で時計回り＝y が下向きの座標で
/// 正の向き）の弧。終わりの点は `end`。
fn axis_arc(c: Point, rx: f64, ry: f64, from: f64, to: f64, end: Point) -> ArcSeg {
    ArcSeg { c, m: [rx, 0.0, 0.0, ry], theta1: from, dtheta: to - from, end }
}

/// 4 分の 1 の角（ラジアン）。
const QUARTER: f64 = core::f64::consts::FRAC_PI_2;

/// `rect` の同じ形のパス（SVG 2 §10.2）。`rx`・`ry` は解いて半分に切り詰めたもの（0 なら角は直角）。大きさが 0 以下なら
/// 何も書かない（呼び手が描かないと決める）。
pub(crate) fn rect(
    w: &mut PathWriter<'_, '_>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    rx: f64,
    ry: f64,
) -> Result<(), Error> {
    let (x1, y1) = (x + width, y + height);
    // 大きさが 0 以下か、足すとあふれるなら書かない（NaN・±∞ を値に入れない）
    if !(width > 0.0 && height > 0.0 && x1.is_finite() && y1.is_finite() && x.is_finite() && y.is_finite()) {
        return Ok(());
    }
    if rx <= 0.0 || ry <= 0.0 {
        w.move_to(Point::new(x, y))?;
        w.line_to(Point::new(x1, y))?;
        w.line_to(Point::new(x1, y1))?;
        w.line_to(Point::new(x, y1))?;
        return w.close();
    }
    w.move_to(Point::new(x + rx, y))?;
    w.line_to(Point::new(x1 - rx, y))?;
    w.arc(axis_arc(Point::new(x1 - rx, y + ry), rx, ry, -QUARTER, 0.0, Point::new(x1, y + ry)))?;
    w.line_to(Point::new(x1, y1 - ry))?;
    w.arc(axis_arc(Point::new(x1 - rx, y1 - ry), rx, ry, 0.0, QUARTER, Point::new(x1 - rx, y1)))?;
    w.line_to(Point::new(x + rx, y1))?;
    w.arc(axis_arc(Point::new(x + rx, y1 - ry), rx, ry, QUARTER, 2.0 * QUARTER, Point::new(x, y1 - ry)))?;
    w.line_to(Point::new(x, y + ry))?;
    w.arc(axis_arc(Point::new(x + rx, y + ry), rx, ry, 2.0 * QUARTER, 3.0 * QUARTER, Point::new(x + rx, y)))?;
    w.close()
}

/// `circle`・`ellipse` の同じ形のパス（SVG 2 §10.4・§10.5：`(cx + rx, cy)` から時計回りに 4 つの弧）。半径が 0 以下なら
/// 何も書かない。
pub(crate) fn ellipse(w: &mut PathWriter<'_, '_>, cx: f64, cy: f64, rx: f64, ry: f64) -> Result<(), Error> {
    let ext = [cx + rx, cx - rx, cy + ry, cy - ry];
    if !(rx > 0.0 && ry > 0.0 && ext.iter().all(|v| v.is_finite())) {
        return Ok(());
    }
    let c = Point::new(cx, cy);
    w.move_to(Point::new(cx + rx, cy))?;
    w.arc(axis_arc(c, rx, ry, 0.0, QUARTER, Point::new(cx, cy + ry)))?;
    w.arc(axis_arc(c, rx, ry, QUARTER, 2.0 * QUARTER, Point::new(cx - rx, cy)))?;
    w.arc(axis_arc(c, rx, ry, 2.0 * QUARTER, 3.0 * QUARTER, Point::new(cx, cy - ry)))?;
    w.arc(axis_arc(c, rx, ry, 3.0 * QUARTER, 4.0 * QUARTER, Point::new(cx + rx, cy)))?;
    w.close()
}

/// 区間の始めと終わりの接線の向き（長さ 0 の向きは使わない：3 次曲線は次の制御点へ。なければ None）。
pub(crate) fn seg_tangents(seg: &PathSeg) -> Option<(Point, Point)> {
    let d = |a: Point, b: Point| Point::new(b.x - a.x, b.y - a.y);
    let first_nonzero = |v: &[Point]| v.iter().copied().find(|p| p.x != 0.0 || p.y != 0.0);
    match *seg {
        PathSeg::Move(_) => None,
        PathSeg::Line(a, b) | PathSeg::Close(a, b) => {
            let v = d(a, b);
            (v.x != 0.0 || v.y != 0.0).then_some((v, v))
        }
        PathSeg::Quad(a, b, e) => Some((first_nonzero(&[d(a, b), d(a, e)])?, first_nonzero(&[d(b, e), d(a, e)])?)),
        PathSeg::Cubic(a, b, c, e) => {
            Some((first_nonzero(&[d(a, b), d(a, c), d(a, e)])?, first_nonzero(&[d(c, e), d(b, e), d(a, e)])?))
        }
        PathSeg::ArcTo(_, arc) => {
            let [m0, m1, m2, m3] = arc.m;
            let sign = if arc.dtheta < 0.0 { -1.0 } else { 1.0 };
            let tan = |th: f64| {
                let (sn, cs) = (math::sin(th), math::cos(th));
                Point::new(sign * (-m0 * sn + m1 * cs), sign * (-m2 * sn + m3 * cs))
            };
            let (t0, t1) = (tan(arc.theta1), tan(arc.theta1 + arc.dtheta));
            let ok = |v: Point| v.x.is_finite() && v.y.is_finite() && (v.x != 0.0 || v.y != 0.0);
            (ok(t0) && ok(t1)).then_some((t0, t1))
        }
    }
}

/// 2 つの向きが同じか（なす角がおよそ 0.06° 未満。曲線のつなぎ目の滑らかさ）。
pub(crate) fn same_direction(u: Point, v: Point) -> bool {
    let cross = u.x * v.y - u.y * v.x;
    let dot = u.x * v.x + u.y * v.y;
    let lens = math::hypot(u.x, u.y) * math::hypot(v.x, v.y);
    dot > 0.0 && cross.abs() <= 1e-3 * lens && lens.is_finite()
}

/// `line` の同じ形のパス。
pub(crate) fn line(w: &mut PathWriter<'_, '_>, p1: Point, p2: Point) -> Result<(), Error> {
    w.move_to(p1)?;
    w.line_to(p2)
}

/// 弧の外接矩形の丸めの余白の係数（2⁻⁴⁴）。
const ARC_PAD: f64 = 1.0 / 17_592_186_044_416.0;

/// 区間の厳密な外接矩形を `r` に足す（始めの点を含む。曲線は導関数の 0 の所、弧は軸に沿う向きの角）。
pub(crate) fn add_bounds(r: &mut Rect, seg: &PathSeg) {
    match *seg {
        PathSeg::Move(p) => r.add(p),
        PathSeg::Line(a, b) | PathSeg::Close(a, b) => {
            r.add(a);
            r.add(b);
        }
        PathSeg::Quad(p0, p1, p2) => {
            r.add(p0);
            r.add(p2);
            for t in [quad_extremum(p0.x, p1.x, p2.x), quad_extremum(p0.y, p1.y, p2.y)].into_iter().flatten() {
                r.add(quad_at(p0, p1, p2, t));
            }
        }
        PathSeg::Cubic(p0, p1, p2, p3) => {
            r.add(p0);
            r.add(p3);
            for axis in [(p0.x, p1.x, p2.x, p3.x), (p0.y, p1.y, p2.y, p3.y)] {
                for t in cubic_extrema(axis.0, axis.1, axis.2, axis.3).into_iter().flatten() {
                    r.add(cubic_at(p0, p1, p2, p3, t));
                }
            }
        }
        PathSeg::ArcTo(p, a) => {
            let mut ar = Rect::EMPTY;
            ar.add(p);
            ar.add(a.end);
            let [m0, m1, m2, m3] = a.m;
            // x = cx + m0 cos θ + m1 sin θ の極は θ = atan2(m1, m0)（と + π）。y も同じ
            for base in [math::atan2(m1, m0), math::atan2(m3, m2)] {
                for k in [0.0, core::f64::consts::PI] {
                    if let Some(th) = angle_in_sweep(base + k, a.theta1, a.dtheta) {
                        let q = a.at_from(p, th);
                        if q.finite() {
                            ar.add(q);
                        }
                    }
                }
            }
            // 角の丸め（±π/2 の近くで 10⁻¹⁶ ほど）は、掃く角がそれに近いほど細い弧で、平坦化の点の位置を動かす（細長い
            // 楕円の小さな弧で、点の動く量の 0.1% ほど）。その上限 Σ|m| × |Δθ| × 2⁻⁴⁴ が座標に比べて意味のある大きさの
            // ときだけ広げる（ふつうの弧の外接矩形と、中身で決まる大きさは変えない。ファズの path が見つけた）
            let pad = (m0.abs() + m1.abs() + m2.abs() + m3.abs()) * a.dtheta.abs() * ARC_PAD;
            let scale = 1.0 + math::max(math::max(ar.x0.abs(), ar.x1.abs()), math::max(ar.y0.abs(), ar.y1.abs()));
            if pad.is_finite() && pad > 1e-9 * scale && !ar.is_empty() {
                ar = Rect { x0: ar.x0 - pad, y0: ar.y0 - pad, x1: ar.x1 + pad, y1: ar.y1 + pad };
            }
            *r = r.union(&ar);
        }
    }
}

/// 角 `th` を、`from` から `from + sweep` の間に入る同じ向きの角に直す（入らなければ None。端は含めない）。
fn angle_in_sweep(th: f64, from: f64, sweep: f64) -> Option<f64> {
    let tau = 2.0 * core::f64::consts::PI;
    let (lo, hi) = if sweep >= 0.0 { (from, from + sweep) } else { (from + sweep, from) };
    // lo 以上の最小の同じ向きの角
    let k = ((lo - th) / tau).ceil();
    let cand = th + k * tau;
    (cand > lo && cand < hi).then_some(cand)
}

/// 2 次曲線の 1 つの軸の極の t（0 と 1 の間にあれば）。
fn quad_extremum(a: f64, b: f64, c: f64) -> Option<f64> {
    let den = a - 2.0 * b + c;
    if den == 0.0 {
        return None;
    }
    let t = (a - b) / den;
    (t > 0.0 && t < 1.0).then_some(t)
}

/// 3 次曲線の 1 つの軸の極の t（0 と 1 の間のもの、多くて 2 つ）。導関数 3[(b−a)(1−t)² + 2(c−b)(1−t)t + (d−c)t²] の根。
fn cubic_extrema(a: f64, b: f64, c: f64, d: f64) -> [Option<f64>; 2] {
    // 差の大きさの最大で割ってから解く（2 乗があふれない）
    let (p, q, r) = (b / 2.0 - a / 2.0, c / 2.0 - b / 2.0, d / 2.0 - c / 2.0);
    let m = math::max(math::max(p.abs(), q.abs()), r.abs());
    if !(m > 0.0 && m.is_finite()) {
        return [None, None];
    }
    let (p, q, r) = (p / m, q / m, r / m);
    // p − 2qt + ... を t の 2 次式に：(p − 2q + r) t² + 2(q − p) t + p
    let qa = p - 2.0 * q + r;
    let qb = 2.0 * (q - p);
    let qc = p;
    let inside = |t: f64| (t.is_finite() && t > 0.0 && t < 1.0).then_some(t);
    let disc = qb * qb - 4.0 * qa * qc;
    if disc < 0.0 {
        return [None, None];
    }
    // 桁の落ちない根の公式（`k = −(b + sign(b)√D) / 2`、根は `k / a` と `c / k`。ほぼ 2 次の 3 次曲線で a が小さくても
    // 極を落とさない。単位 1 の 3 回目のレビュー）
    let k = -(qb + if qb < 0.0 { -disc.sqrt() } else { disc.sqrt() }) / 2.0;
    if k == 0.0 {
        // b = 0 かつ D = 0（a c = 0）：根は 0（区間の外）
        return [None, None];
    }
    let r1 = if qa == 0.0 { None } else { inside(k / qa) };
    [r1, inside(qc / k)]
}

/// 2 次曲線の t の点。
pub(crate) fn quad_at(p0: Point, p1: Point, p2: Point, t: f64) -> Point {
    let u = 1.0 - t;
    let (a, b, c) = (u * u, 2.0 * u * t, t * t);
    Point::new(a * p0.x + b * p1.x + c * p2.x, a * p0.y + b * p1.y + c * p2.y)
}

/// 3 次曲線の t の点。
pub(crate) fn cubic_at(p0: Point, p1: Point, p2: Point, p3: Point, t: f64) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(a * p0.x + b * p1.x + c * p2.x + d * p3.x, a * p0.y + b * p1.y + c * p2.y + d * p3.y)
}

/// パス全体の厳密な外接矩形（区間を変形 `t` で写してから求める）。
pub(crate) fn bounds(path: PathRef<'_>, t: &Transform) -> Rect {
    let mut r = Rect::EMPTY;
    for seg in path.segments() {
        add_bounds(&mut r, &transform_seg(&seg, t));
    }
    r
}

/// 区間を写す。
pub(crate) fn transform_seg(seg: &PathSeg, t: &Transform) -> PathSeg {
    match *seg {
        PathSeg::Move(p) => PathSeg::Move(t.apply(p)),
        PathSeg::Line(a, b) => PathSeg::Line(t.apply(a), t.apply(b)),
        PathSeg::Quad(a, b, c) => PathSeg::Quad(t.apply(a), t.apply(b), t.apply(c)),
        PathSeg::Cubic(a, b, c, d) => PathSeg::Cubic(t.apply(a), t.apply(b), t.apply(c), t.apply(d)),
        PathSeg::ArcTo(p, a) => PathSeg::ArcTo(t.apply(p), a.transformed(t)),
        PathSeg::Close(a, b) => PathSeg::Close(t.apply(a), t.apply(b)),
    }
}

#[cfg(test)]
mod tests;
