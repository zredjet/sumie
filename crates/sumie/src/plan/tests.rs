#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use super::*;
use crate::cancel::Never;
use crate::doc::Size;
use crate::geom::{PathWriter, Rect, bounds, ellipse, rect};
use crate::render::{PixelRect, PixmapMut};
use crate::report::ReportBuilder;
use crate::tree::{CAP_BUTT, JOIN_MITER, PathNode};

/// 試しの木を組む口。
struct Builder {
    nodes: Vec<PathNode>,
    verbs: Vec<u8>,
    coords: Vec<f64>,
    dashes: Vec<f64>,
}

fn node() -> PathNode {
    PathNode {
        transform: Transform::IDENTITY,
        bounds: Rect::EMPTY,
        fill: [0.0, 0.0, 0.0, 0.0],
        stroke: [0.0, 0.0, 0.0, 0.0],
        stroke_width: 1.0,
        miter_limit: 4.0,
        dash_offset: 0.0,
        path_length: 0.0,
        verbs: [0, 0],
        coords: [0, 0],
        dashes: [0, 0],
        flags: 0,
        cap: CAP_BUTT,
        join: JOIN_MITER,
        pad: [0; 5],
    }
}

impl Builder {
    fn new() -> Builder {
        Builder { nodes: Vec::new(), verbs: Vec::new(), coords: Vec::new(), dashes: Vec::new() }
    }

    /// パスを書いて節を足す（`f` が節を整える）。
    fn add(&mut self, w: impl FnOnce(&mut PathWriter<'_, '_>), f: impl FnOnce(&mut PathNode)) {
        let b = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
        let mut verbs = MVec::new(&b);
        let mut coords = MVec::new(&b);
        {
            let mut pw = PathWriter::new(&mut verbs, &mut coords);
            w(&mut pw);
        }
        let mut n = node();
        n.verbs = [self.verbs.len() as u32, verbs.len() as u32];
        n.coords = [self.coords.len() as u32, coords.len() as u32];
        self.verbs.extend_from_slice(verbs.as_slice());
        self.coords.extend_from_slice(coords.as_slice());
        f(&mut n);
        let p = PathRef { verbs: verbs.as_slice(), coords: coords.as_slice() };
        n.bounds = bounds(p, &n.transform);
        self.nodes.push(n);
    }

    fn dashes(&mut self, d: &[f64]) -> [u32; 2] {
        let start = self.dashes.len() as u32;
        self.dashes.extend_from_slice(d);
        [start, d.len() as u32]
    }

    fn tree(&self, w: f64, h: f64) -> Tree {
        // 置き場は Budget の外に作る（木は何も借りない）
        let b = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
        let mut nodes = MVec::new(&b);
        nodes.append_slice(&self.nodes).unwrap();
        let mut verbs = MVec::new(&b);
        verbs.append_slice(&self.verbs).unwrap();
        let mut coords = MVec::new(&b);
        coords.append_slice(&self.coords).unwrap();
        let mut dashes = MVec::new(&b);
        dashes.append_slice(&self.dashes).unwrap();
        let report = ReportBuilder::with_capacity(&b, 0, 0, 0).unwrap().finish().unwrap();
        Tree {
            size: Size { width: w, height: h },
            nodes: nodes.freeze(),
            verbs: verbs.freeze(),
            coords: coords.freeze(),
            dashes: dashes.freeze(),
            report,
        }
    }
}

const GREEN: [f64; 4] = [0.0, 128.0, 0.0, 1.0];
const RED: [f64; 4] = [255.0, 0.0, 0.0, 1.0];

/// 全体を描いた画素。
fn draw(t: &Tree, scale: f64) -> (u32, u32, Vec<u8>) {
    let opts = RenderOptions { scale, ..RenderOptions::DEFAULT };
    let plan = t.plan(&opts, &Never).unwrap();
    let (w, h) = (plan.width, plan.height);
    let mut px = vec![0u8; (w * h * 4) as usize];
    let mut out = PixmapMut::new(&mut px, w, h, (w * 4) as usize).unwrap();
    plan.render(&mut out, &Never).unwrap();
    (w, h, px)
}

#[test]
fn fills_and_strokes() {
    let mut b = Builder::new();
    b.add(
        |w| rect(w, 10.0, 10.0, 20.0, 20.0, 0.0, 0.0).unwrap(),
        |n| {
            n.flags = HAS_FILL;
            n.fill = GREEN;
        },
    );
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 35.0)).unwrap();
            w.line_to(Point::new(40.0, 35.0)).unwrap();
        },
        |n| {
            n.flags = HAS_STROKE;
            n.stroke = RED;
            n.stroke_width = 4.0;
        },
    );
    let t = b.tree(40.0, 40.0);
    let (w, _, px) = draw(&t, 1.0);
    let at = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    assert_eq!(at(20, 20), [0, 128, 0, 255]);
    assert_eq!(at(5, 5), [0, 0, 0, 0]);
    // 太さ 4 の線は y 33〜37：行 33〜36
    assert_eq!(at(20, 33), [255, 0, 0, 255]);
    assert_eq!(at(20, 36), [255, 0, 0, 255]);
    assert_eq!(at(20, 37), [0, 0, 0, 0]);
    assert_eq!(at(20, 32), [0, 0, 0, 0]);
    // 倍率 2 で大きさも倍
    let (w2, h2, _) = draw(&t, 2.0);
    assert_eq!((w2, h2), (80, 80));
}

/// 種つきの xorshift。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn f(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() % 1_000_000) as f64 / 1_000_000.0)
    }

    fn i(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// 乱れた木（四角・楕円・曲線と弧のパス。塗り・線・破線・継ぎ目と端・変形・crispEdges）。
fn random_tree(r: &mut Rng) -> Tree {
    let mut b = Builder::new();
    for _ in 0..(1 + r.i(6)) {
        let kind = r.i(3);
        let (x, y, w, h) = (r.f(-20.0, 50.0), r.f(-20.0, 50.0), r.f(1.0, 40.0), r.f(1.0, 40.0));
        let pts: Vec<(f64, f64)> = (0..8).map(|_| (r.f(-10.0, 70.0), r.f(-10.0, 70.0))).collect();
        let arc = (r.f(1.0, 30.0), r.f(1.0, 30.0), r.f(0.0, 90.0), r.i(2) == 1, r.i(2) == 1);
        let closed = r.i(2) == 1;
        let flags = [HAS_FILL, HAS_STROKE, HAS_FILL | HAS_STROKE, HAS_FILL | HAS_STROKE | STROKE_FIRST]
            [r.i(4) as usize]
            | if r.i(4) == 0 { EVEN_ODD } else { 0 }
            | if r.i(5) == 0 { CRISP } else { 0 };
        let width = r.f(0.5, 12.0);
        let cap = r.i(3) as u8;
        let join = r.i(3) as u8;
        let limit = r.f(0.5, 6.0);
        let dash = if r.i(3) == 0 { Some(vec![r.f(0.0, 8.0), r.f(0.5, 8.0)]) } else { None };
        let dash_range = dash.as_ref().map(|d| b.dashes(d));
        let t = Transform {
            a: r.f(0.5, 1.5),
            b: r.f(-0.5, 0.5),
            c: r.f(-0.5, 0.5),
            d: r.f(0.5, 1.5),
            e: r.f(-5.0, 5.0),
            f: r.f(-5.0, 5.0),
        };
        let alpha = r.f(0.2, 1.0);
        let col = [r.f(0.0, 255.0), r.f(0.0, 255.0), r.f(0.0, 255.0), alpha];
        b.add(
            |pw| match kind {
                0 => rect(pw, x, y, w, h, r.f(0.0, 5.0), r.f(0.0, 5.0)).unwrap(),
                1 => ellipse(pw, x, y, w, h).unwrap(),
                _ => {
                    pw.move_to(Point::new(pts[0].0, pts[0].1)).unwrap();
                    pw.cubic_to(
                        Point::new(pts[1].0, pts[1].1),
                        Point::new(pts[2].0, pts[2].1),
                        Point::new(pts[3].0, pts[3].1),
                    )
                    .unwrap();
                    pw.line_to(Point::new(pts[4].0, pts[4].1)).unwrap();
                    pw.arc_to(arc.0, arc.1, arc.2, arc.3, arc.4, Point::new(pts[5].0, pts[5].1)).unwrap();
                    pw.quad_to(Point::new(pts[6].0, pts[6].1), Point::new(pts[7].0, pts[7].1)).unwrap();
                    if closed {
                        pw.close().unwrap();
                    }
                }
            },
            |n| {
                n.transform = t;
                n.flags = flags;
                n.fill = col;
                n.stroke = [col[2], col[1], col[0], alpha];
                n.stroke_width = width;
                n.cap = cap;
                n.join = join;
                n.miter_limit = limit;
                if let Some(d) = dash_range {
                    n.dashes = d;
                    n.dash_offset = 3.0;
                }
            },
        );
    }
    b.tree(48.0, 40.0)
}

/// 範囲の描画は全体の描画の同じ矩形とビット単位で同じ。見積もりの内で描け、範囲の見積もりは全体の見積もりを超えない。
#[test]
fn regions_match_and_estimates_hold() {
    let mut r = Rng(0x5eed_9a1e_0002);
    for case in 0..150 {
        let t = random_tree(&mut r);
        let scale = [1.0, 1.5, 0.75][(case % 3) as usize];
        let opts = RenderOptions { scale, ..RenderOptions::DEFAULT };
        let est = t.estimate(&opts, &Never).unwrap();
        let plan = t.plan(&opts, &Never).unwrap_or_else(|e| panic!("{case} 件目：計画が見積もりの内で作れない {e:?}"));
        assert!(plan.held_bytes() <= est.plan.result_bytes, "{case}");
        let (w, h) = (plan.width, plan.height);
        let full_rect = PixelRect { x: 0, y: 0, width: w, height: h };
        let full_est = plan.estimate_region(full_rect, &Never).unwrap();
        assert!(
            full_est.work <= est.render.work,
            "{case} 件目：範囲の見積もり {} > 全体 {}",
            full_est.work,
            est.render.work
        );
        assert!(full_est.peak_bytes <= est.render.peak_bytes, "{case}");
        let mut full = vec![0u8; (w * h * 4) as usize];
        plan.render(&mut PixmapMut::new(&mut full, w, h, (w * 4) as usize).unwrap(), &Never)
            .unwrap_or_else(|e| panic!("{case} 件目：描く段が見積もりを超えた {e:?}"));
        for _ in 0..6 {
            let x = r.i(u64::from(w)) as u32;
            let y = r.i(u64::from(h)) as u32;
            let rw = 1 + r.i(u64::from(w - x)) as u32;
            let rh = 1 + r.i(u64::from(h - y)) as u32;
            let rect = PixelRect { x, y, width: rw, height: rh };
            let mut part = vec![7u8; (rw * rh * 4) as usize];
            plan.render_region(rect, &mut PixmapMut::new(&mut part, rw, rh, (rw * 4) as usize).unwrap(), &Never)
                .unwrap();
            for yy in 0..rh {
                for xx in 0..rw {
                    let a = ((((y + yy) * w) + (x + xx)) * 4) as usize;
                    let b = ((yy * rw + xx) * 4) as usize;
                    assert_eq!(&full[a..a + 4], &part[b..b + 4], "{case} 件目 ({}, {}) 範囲 {rect:?}", x + xx, y + yy);
                }
            }
        }
    }
}

#[test]
fn bad_arguments() {
    let mut b = Builder::new();
    b.add(
        |w| rect(w, 0.0, 0.0, 5.0, 5.0, 0.0, 0.0).unwrap(),
        |n| {
            n.flags = HAS_FILL;
            n.fill = GREEN;
        },
    );
    let t = b.tree(10.0, 10.0);
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let opts = RenderOptions { scale, ..RenderOptions::DEFAULT };
        assert!(matches!(t.estimate(&opts, &Never), Err(Error::Invalid(_))), "{scale}");
    }
    let plan = t.plan(&RenderOptions::DEFAULT, &Never).unwrap();
    let mut px = vec![0u8; 400];
    // 大きさの違う出力・キャンバスの外の範囲・空の範囲
    assert!(plan.render(&mut PixmapMut::new(&mut px, 5, 5, 20).unwrap(), &Never).is_err());
    let bad = PixelRect { x: 8, y: 0, width: 5, height: 5 };
    assert!(plan.render_region(bad, &mut PixmapMut::new(&mut px, 5, 5, 20).unwrap(), &Never).is_err());
    assert!(PixmapMut::new(&mut px, 10, 10, 39).is_err());
    // キャンバスの上限
    let small = RenderOptions { scale: 1.0, limits: RenderLimits { canvas_side: 5, ..RenderLimits::DEFAULT } };
    assert!(matches!(t.estimate(&small, &Never), Err(Error::Limit(_))));
}
