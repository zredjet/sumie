#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::unwrap_used,
    clippy::panic
)]
//! 敵対的な計画と描画の試験（単位 1 の 2 回目・単位 2 の 1 回目のレビューの再現を取り込んだもの）。

use super::*;
use crate::tree::CAP_SQUARE;

fn render_full(t: &Tree, scale: f64) -> Result<(u32, u32, Vec<u8>), Error> {
    let opts = RenderOptions { scale, ..RenderOptions::DEFAULT };
    let plan = t.plan(&opts, &Never)?;
    let (w, h) = (plan.width, plan.height);
    let mut px = vec![0u8; (w * h * 4) as usize];
    let mut out = PixmapMut::new(&mut px, w, h, (w * 4) as usize).unwrap();
    plan.render(&mut out, &Never)?;
    Ok((w, h, px))
}

/// 扁平な楕円の弧（長軸の端の近くの細い弧）の折り返しを並べた線。弧の曲がる角を |Δθ| で数えると、平坦化した折れ線の
/// 曲がる角（ほぼ π）を大きく下回る。
#[test]
fn eccentric_arc_turning_is_underestimated() {
    let mut b = Builder::new();
    let delta: f64 = 0.05;
    let (rx, ry) = (1000.0, 1.0);
    let x = rx * delta.cos();
    let dy = ry * delta.sin();
    b.add(
        |w| {
            let mut y = 0.0;
            w.move_to(Point::new(x, y - dy)).unwrap();
            for i in 0..40 {
                let side = if i % 2 == 0 { 1.0 } else { -1.0 };
                // 長軸の端（x = ±rx）を回る細い弧
                w.arc_to(rx, ry, 0.0, false, side > 0.0, Point::new(side * x, y + dy)).unwrap();
                y += 2.0 * dy;
                w.line_to(Point::new(-side * x, y - dy)).unwrap();
            }
        },
        |n| {
            n.flags = HAS_STROKE;
            n.stroke = [0.0, 0.0, 0.0, 1.0];
            n.stroke_width = 2000.0;
            n.join = JOIN_ROUND;
            n.transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 1100.0, f: 1100.0 };
        },
    );
    let t = b.tree(2200.0, 2200.0);
    let opts = RenderOptions::DEFAULT;
    let est = t.estimate(&opts, &Never).unwrap();
    let r = t.plan(&opts, &Never);
    match r {
        Ok(_) => {}
        Err(e) => panic!("見積もり {:?} の内で計画が作れない：{e:?}", est.plan),
    }
}

/// pathLength と non-scaling-stroke の破線は、倍率で破線の割合が変わる（倍率を 2 度掛ける）。
#[test]
fn non_scaling_path_length_dash_scales_twice() {
    let mk = |non_scaling: bool| {
        let mut b = Builder::new();
        let d = b.dashes(&[5.0, 5.0]);
        b.add(
            |w| {
                w.move_to(Point::new(0.0, 10.0)).unwrap();
                w.line_to(Point::new(100.0, 10.0)).unwrap();
            },
            |n| {
                n.flags = HAS_STROKE | if non_scaling { NON_SCALING } else { 0 };
                n.stroke = [0.0, 0.0, 0.0, 1.0];
                n.stroke_width = 2.0;
                n.dashes = d;
                n.path_length = 10.0;
            },
        );
        b.tree(100.0, 20.0)
    };
    for ns in [false, true] {
        let mut seen = Vec::new();
        for scale in [1.0, 2.0, 3.0] {
            let (w, _, px) = render_full(&mk(ns), scale).unwrap();
            // 道の 3/4・1/4 の所（割れ目と線の中）
            let at = |f: f64| {
                let (x, y) = ((100.0 * f * scale) as u32, (10.0 * scale) as u32);
                px[((y * w + x) * 4 + 3) as usize]
            };
            seen.push((at(0.75), at(0.25)));
        }
        // 倍率で破線の割合が変わらない（FR-OUT-03）
        assert!(seen.iter().all(|&(gap, on)| gap == 0 && on > 0), "non-scaling {ns}: {seen:?}");
    }
}

/// 破線の並びが合計 0（[0, 0]）で pathLength があると、計画は pathLength の分の平坦化をもう 1 度するが、見積もりは数えない。
#[test]
fn zero_sum_dash_with_path_length() {
    let mut b = Builder::new();
    let d = b.dashes(&[0.0, 0.0]);
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 0.0)).unwrap();
            for i in 0..200 {
                let x = f64::from(i) * 5.0;
                w.cubic_to(Point::new(x + 1.0, 400.0), Point::new(x + 4.0, -400.0), Point::new(x + 5.0, 0.0)).unwrap();
            }
        },
        |n| {
            n.flags = HAS_STROKE;
            n.stroke = [0.0, 0.0, 0.0, 1.0];
            n.stroke_width = 1.0;
            n.dashes = d;
            n.path_length = 10.0;
            n.transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 500.0 };
        },
    );
    let t = b.tree(1000.0, 1000.0);
    let est = t.estimate(&RenderOptions::DEFAULT, &Never).unwrap();
    let r = t.plan(&RenderOptions::DEFAULT, &Never);
    assert!(r.is_ok(), "見積もり {:?} の内で計画が作れない：{:?}", est.plan, r.err());
}

/// 箱の境の近くの点：根のユーザー単位の外接矩形 × 倍率と、倍率を掛けた変形で写した点が丸めで食い違い、箱で分かれる辺を
/// 見積もりが数えない。
#[test]
fn box_split_uncounted_by_rounding() {
    // 倍率 s・平行移動 e で、(x + e)·s ≥ −BOX なのに (s·x) + (e·s) < −BOX になる x を探す
    let s = 0.1;
    let e = 0.1;
    let mut found = None;
    let mut x = -BOX / s - e + 1e-5;
    for _ in 0..200_000 {
        let user = 1.0 * x + e; // n.transform.apply（a = 1）
        let dev = (1.0 * s) * x + e * s; // t.apply（t = n.transform.scaled(s)）
        if user * s >= -BOX && dev < -BOX {
            found = Some(x);
            break;
        }
        x = f64::from_bits(x.to_bits() + 1);
        let _ = x;
    }
    let Some(x0) = found else { return };
    // ほぼ縦のジグザグ：x が x0 と x0 + 10 を行き来し、y が ±BOX を行き来する（どの辺も箱の境をまたぐ）
    let mut b = Builder::new();
    b.add(
        |w| {
            let top = -500.0 / s;
            let bot = 500.0 / s;
            w.move_to(Point::new(x0, top)).unwrap();
            for i in 0..50 {
                let (xx, yy) = if i % 2 == 0 { (x0 + 1e-5, bot) } else { (x0, top) };
                w.line_to(Point::new(xx, yy)).unwrap();
            }
        },
        |n| {
            n.flags = HAS_FILL;
            n.fill = [0.0, 0.0, 0.0, 1.0];
            n.transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e, f: 0.0 };
        },
    );
    let t = b.tree(100.0, 100.0);
    let opts = RenderOptions { scale: s, ..RenderOptions::DEFAULT };
    let est = t.estimate(&opts, &Never).unwrap();
    let r = t.plan(&opts, &Never);
    match r {
        Ok(_) => {}
        Err(err) => panic!("見積もり {:?} の内で計画が作れない：{err:?}", est.plan),
    }
}

/// 太い線の範囲の描画（幅 1 の範囲・キャンバスの端）が全体と同じ。
#[test]
fn thick_lines_regions() {
    let mut b = Builder::new();
    let d = b.dashes(&[3.0, 2.0]);
    b.add(
        |w| {
            w.move_to(Point::new(-30.0, 5.0)).unwrap();
            w.cubic_to(Point::new(10.0, 80.0), Point::new(40.0, -40.0), Point::new(90.0, 50.0)).unwrap();
            w.line_to(Point::new(20.0, 60.0)).unwrap();
            w.close().unwrap();
        },
        |n| {
            n.flags = HAS_STROKE | HAS_FILL | EVEN_ODD;
            n.fill = [10.0, 200.0, 30.0, 0.7];
            n.stroke = [200.0, 0.0, 30.0, 0.6];
            n.stroke_width = 23.0;
            n.dashes = d;
            n.join = JOIN_ROUND;
            n.cap = CAP_ROUND;
        },
    );
    let t = b.tree(64.0, 48.0);
    let (w, h, full) = render_full(&t, 1.0).unwrap();
    let plan = t.plan(&RenderOptions::DEFAULT, &Never).unwrap();
    for x in 0..w {
        for (y, rh) in [(0u32, h), (h - 1, 1)] {
            let rect = PixelRect { x, y, width: 1, height: rh };
            let mut part = vec![9u8; (rh * 4) as usize];
            plan.render_region(rect, &mut PixmapMut::new(&mut part, 1, rh, 4).unwrap(), &Never).unwrap();
            for yy in 0..rh {
                let a = (((y + yy) * w + x) * 4) as usize;
                assert_eq!(&full[a..a + 4], &part[(yy * 4) as usize..(yy * 4 + 4) as usize], "({x}, {})", y + yy);
            }
        }
    }
}

struct Rng2(u64);
impl Rng2 {
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

/// 極端な線（太い・小さな破線・多い破線・pathLength・non-scaling・歪んだ変形・閉じた部分パス・長さ 0 の部分パス）で、
/// 見積もりの内で計画と描画ができるか。弧は円（rx = ry）だけ（扁平な弧は別の試しで落ちる）。
#[test]
fn extreme_strokes_fit_estimates() {
    let mut r = Rng2(0xabad_1dea_0007);
    let mut fails = Vec::new();
    for case in 0..400 {
        let mut b = Builder::new();
        let nodes = 1 + r.i(3);
        for _ in 0..nodes {
            let ndash = r.i(6);
            let dash_range = if ndash > 0 {
                let mut d = Vec::new();
                for _ in 0..(ndash * 2) {
                    d.push(match r.i(4) {
                        0 => 0.0,
                        1 => r.f(0.0, 0.05),
                        _ => r.f(0.0, 10.0),
                    });
                }
                Some(b.dashes(&d))
            } else {
                None
            };
            let width = match r.i(3) {
                0 => r.f(0.01, 1.0),
                1 => r.f(1.0, 20.0),
                _ => r.f(20.0, 300.0),
            };
            let skew = r.i(2) == 0;
            let t = if skew {
                Transform {
                    a: r.f(0.05, 3.0),
                    b: r.f(-2.0, 2.0),
                    c: r.f(-2.0, 2.0),
                    d: r.f(0.05, 3.0),
                    e: r.f(0.0, 50.0),
                    f: r.f(0.0, 50.0),
                }
            } else {
                Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: r.f(0.0, 50.0), f: r.f(0.0, 50.0) }
            };
            let segs = 1 + r.i(30);
            let mut pts: Vec<(u64, [f64; 6])> = Vec::new();
            for _ in 0..segs {
                let k = r.i(7);
                let mut a = [0.0; 6];
                for v in a.iter_mut() {
                    *v = r.f(-40.0, 80.0);
                }
                pts.push((k, a));
            }
            let flags = [HAS_STROKE, HAS_STROKE | HAS_FILL][r.i(2) as usize]
                | if r.i(3) == 0 { NON_SCALING } else { 0 }
                | if r.i(4) == 0 { CRISP } else { 0 };
            let cap = r.i(3) as u8;
            let join = r.i(3) as u8;
            let limit = [1.0, 4.0, 50.0, 0.5][r.i(4) as usize];
            let path_length = if r.i(3) == 0 { r.f(0.1, 500.0) } else { 0.0 };
            let off = r.f(-20.0, 20.0);
            b.add(
                |w| {
                    w.move_to(Point::new(pts[0].1[0], pts[0].1[1])).unwrap();
                    for (k, a) in &pts {
                        match k {
                            0 => w.line_to(Point::new(a[0], a[1])).unwrap(),
                            1 => w.quad_to(Point::new(a[0], a[1]), Point::new(a[2], a[3])).unwrap(),
                            2 => w
                                .cubic_to(Point::new(a[0], a[1]), Point::new(a[2], a[3]), Point::new(a[4], a[5]))
                                .unwrap(),
                            3 => {
                                let rr = a[2].abs() + 1.0;
                                // 扁平な楕円も（弧の曲がる角。単位 1・2 のレビュー）
                                let ry = if a[5] > 20.0 { rr / 50.0 } else { rr };
                                w.arc_to(rr, ry, a[5], a[3] > 0.0, a[4] > 0.0, Point::new(a[0], a[1])).unwrap()
                            }
                            4 => w.close().unwrap(),
                            5 => {
                                // 長さ 0 の部分パス
                                w.move_to(Point::new(a[0], a[1])).unwrap();
                                w.line_to(Point::new(a[0], a[1])).unwrap();
                            }
                            _ => {
                                // 戻る線（折り返し）
                                let p = w.current();
                                w.line_to(Point::new(a[0], a[1])).unwrap();
                                w.line_to(p).unwrap();
                            }
                        }
                    }
                },
                |n| {
                    n.transform = t;
                    n.flags = flags;
                    n.fill = [10.0, 20.0, 30.0, 0.8];
                    n.stroke = [200.0, 20.0, 30.0, 0.9];
                    n.stroke_width = width;
                    n.cap = cap;
                    n.join = join;
                    n.miter_limit = limit;
                    n.path_length = path_length;
                    n.dash_offset = off;
                    if let Some(d) = dash_range {
                        n.dashes = d;
                    }
                },
            );
        }
        let t = b.tree(64.0, 48.0);
        let scale = [1.0, 0.3, 2.5][r.i(3) as usize];
        let opts = RenderOptions {
            scale,
            limits: RenderLimits {
                plan_work: u64::MAX,
                plan_bytes: u64::MAX,
                render_work: u64::MAX,
                temp_bytes: u64::MAX,
                estimate_work: u64::MAX,
                estimate_bytes: u64::MAX,
                ..RenderLimits::DEFAULT
            },
        };
        let est = match t.estimate(&opts, &Never) {
            Ok(e) => e,
            Err(e) => {
                fails.push(format!("{case} 見積もり {e:?}"));
                continue;
            }
        };
        match t.plan(&opts, &Never) {
            Ok(plan) => {
                let (w, h) = (plan.width, plan.height);
                let mut px = vec![0u8; (w * h * 4) as usize];
                if let Err(e) = plan.render(&mut PixmapMut::new(&mut px, w, h, (w * 4) as usize).unwrap(), &Never) {
                    fails.push(format!("{case} 描く {e:?}"));
                }
            }
            Err(e) => fails.push(format!("{case} 計画 {e:?} 見積もり {:?}", est.plan)),
        }
    }
    assert!(fails.is_empty(), "{} 件：{:?}", fails.len(), &fails[..fails.len().min(5)]);
}

/// 巨大な図形（箱の外へ出る円・線）と 256 に揃えたタイル・1 画素の範囲で、全体と同じ。
#[test]
fn huge_shapes_tiles() {
    let mut b = Builder::new();
    b.add(
        |w| crate::geom::ellipse(w, 1.0e7 + 300.0, 300.0, 1.0e7, 1.0e7).unwrap(),
        |n| {
            n.flags = HAS_FILL | HAS_STROKE;
            n.fill = [0.0, 100.0, 200.0, 0.6];
            n.stroke = [200.0, 0.0, 0.0, 0.8];
            n.stroke_width = 37.0;
        },
    );
    let d = b.dashes(&[13.0, 7.0]);
    b.add(
        |w| {
            w.move_to(Point::new(-1.0e8, -3.0e7)).unwrap();
            w.line_to(Point::new(1.0e8, 3.0e7 + 600.0)).unwrap();
            w.line_to(Point::new(-5.0e6, 590.0)).unwrap();
        },
        |n| {
            n.flags = HAS_STROKE | HAS_FILL | EVEN_ODD;
            n.fill = [0.0, 200.0, 0.0, 0.5];
            n.stroke = [0.0, 0.0, 0.0, 0.7];
            n.stroke_width = 61.0;
            n.join = JOIN_ROUND;
            n.cap = CAP_ROUND;
        },
    );
    b.add(
        |w| {
            w.move_to(Point::new(-50.0, 30.0)).unwrap();
            w.cubic_to(Point::new(200.0, 900.0), Point::new(400.0, -300.0), Point::new(650.0, 500.0)).unwrap();
        },
        |n| {
            n.flags = HAS_STROKE;
            n.stroke = [100.0, 0.0, 100.0, 0.7];
            n.stroke_width = 45.0;
            n.dashes = d;
            n.dash_offset = 5.0;
            n.join = JOIN_ROUND;
            n.cap = CAP_SQUARE;
        },
    );
    b.add(
        |w| crate::geom::ellipse(w, 260.0, 250.0, 200.0, 120.0).unwrap(),
        |n| {
            n.flags = HAS_STROKE | CRISP;
            n.stroke = [0.0, 0.0, 200.0, 1.0];
            n.stroke_width = 9.0;
            n.transform = Transform { a: 1.0, b: 0.3, c: -0.4, d: 1.0, e: 40.0, f: 20.0 };
        },
    );
    let t = b.tree(600.0, 600.0);
    // 既定の上限で断らない（キャンバスの外の長さに比例させない・箱にかからない破線は作らない。ADR-30 (4)）
    let opts = RenderOptions::DEFAULT;
    let est = t.estimate(&opts, &Never).unwrap();
    assert!(est.render.work < 1 << 30, "{est:?}");
    let plan = t.plan(&opts, &Never).unwrap();
    let (w, h) = (plan.width, plan.height);
    let mut full = vec![0u8; (w * h * 4) as usize];
    plan.render(&mut PixmapMut::new(&mut full, w, h, (w * 4) as usize).unwrap(), &Never).unwrap();
    let mut rects = Vec::new();
    for ty in (0..h).step_by(256) {
        for tx in (0..w).step_by(256) {
            rects.push(PixelRect { x: tx, y: ty, width: 256.min(w - tx), height: 256.min(h - ty) });
        }
    }
    for k in 0..40 {
        rects.push(PixelRect { x: (k * 37) % w, y: (k * 53) % h, width: 1, height: 1 });
        rects.push(PixelRect { x: (k * 31) % w, y: 0, width: 1, height: h });
    }
    for rect in rects {
        let mut part = vec![3u8; (rect.width * rect.height * 4) as usize];
        plan.render_region(
            rect,
            &mut PixmapMut::new(&mut part, rect.width, rect.height, (rect.width * 4) as usize).unwrap(),
            &Never,
        )
        .unwrap();
        for yy in 0..rect.height {
            for xx in 0..rect.width {
                let a = (((rect.y + yy) * w + rect.x + xx) * 4) as usize;
                let bb = ((yy * rect.width + xx) * 4) as usize;
                assert_eq!(&full[a..a + 4], &part[bb..bb + 4], "{rect:?} ({xx},{yy})");
            }
        }
    }
    let painted = full.chunks(4).filter(|p| p[3] != 0).count();
    assert!(painted > 1000);
}

/// 節が同じ破線の並びを指すとき、見積もりは節ごとに並びを払わずに足す。
#[test]
fn shared_dash_list_estimate_unpaid() {
    let mut b = Builder::new();
    let d: Vec<f64> = (0..200_000).map(|i| 1.0 + f64::from(i % 3)).collect();
    let range = b.dashes(&d);
    for _ in 0..2000 {
        b.add(
            |w| {
                w.move_to(Point::new(1.0, 1.0)).unwrap();
                w.line_to(Point::new(5.0, 5.0)).unwrap();
            },
            |n| {
                n.flags = HAS_STROKE;
                n.stroke = [0.0, 0.0, 0.0, 1.0];
                n.dashes = range;
            },
        );
    }
    let t = b.tree(10.0, 10.0);
    let limits = RenderLimits { estimate_work: 100_000, ..RenderLimits::DEFAULT };
    let opts = RenderOptions { scale: 1.0, limits };
    // 破線の並びをなめる分を払うので、見積もりの手間の上限で止まる
    let est = t.estimate(&opts, &Never);
    assert!(matches!(est, Err(Error::Limit(l)) if l.resource == Resource::Work), "{est:?}");
}

fn hairpins(join: u8, ry: f64) -> Tree {
    let mut b = Builder::new();
    let delta: f64 = 0.05;
    let rx = 1000.0;
    let x = rx * delta.cos();
    let dy = ry * delta.sin();
    b.add(
        |w| {
            let mut y = 0.0;
            w.move_to(Point::new(x, y - dy)).unwrap();
            for i in 0..40 {
                let side = if i % 2 == 0 { 1.0 } else { -1.0 };
                w.arc_to(rx, ry, 0.0, false, side > 0.0, Point::new(side * x, y + dy)).unwrap();
                y += 2.0 * dy;
                w.line_to(Point::new(-side * x, y - dy)).unwrap();
            }
        },
        |n| {
            n.flags = HAS_STROKE;
            n.stroke = [0.0, 0.0, 0.0, 1.0];
            n.stroke_width = 2000.0;
            n.join = join;
            n.transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 1100.0, f: 1100.0 };
        },
    );
    b.tree(2200.0, 2200.0)
}

#[test]
fn hairpin_variants() {
    let big = RenderLimits {
        plan_work: u64::MAX,
        plan_bytes: u64::MAX,
        render_work: u64::MAX,
        temp_bytes: u64::MAX,
        ..RenderLimits::DEFAULT
    };
    for (name, join, ry) in
        [("round・扁平", JOIN_ROUND, 1.0), ("miter・扁平", JOIN_MITER, 1.0), ("round・円に近い", JOIN_ROUND, 1000.0)]
    {
        let t = hairpins(join, ry);
        let opts = RenderOptions { scale: 1.0, limits: big };
        t.estimate(&opts, &Never).unwrap();
        let r = t.plan(&opts, &Never);
        assert!(r.is_ok(), "{name}: {:?}", r.err());
    }
}

/// ちょうどの折り返しの round の継ぎ目を並べても、見積もりの内で計画が作れる（単位 1 の 2 回目のレビュー）。
#[test]
fn exact_reversals_fit_estimates() {
    for width in [40.0, 100.0] {
        let mut b = Builder::new();
        b.add(
            |w| {
                w.move_to(Point::new(20.0, 100.0)).unwrap();
                for i in 0..200 {
                    w.line_to(Point::new(if i % 2 == 0 { 180.0 } else { 20.0 }, 100.0)).unwrap();
                }
            },
            |n| {
                n.flags = HAS_STROKE;
                n.stroke = [0.0, 0.0, 0.0, 1.0];
                n.stroke_width = width;
                n.join = JOIN_ROUND;
            },
        );
        let t = b.tree(200.0, 200.0);
        t.estimate(&RenderOptions::DEFAULT, &Never).unwrap();
        let r = t.plan(&RenderOptions::DEFAULT, &Never);
        assert!(r.is_ok(), "{width}: {:?}", r.err());
    }
}

/// 大きな miterlimit でも、ふつうの線を断らない（描く手間は節の画素の箱で抑える。単位 1 の 2 回目のレビュー）。
#[test]
fn huge_miter_limit_is_not_refused() {
    for limit in [1e6, 1e300] {
        let mut b = Builder::new();
        b.add(
            |w| {
                w.move_to(Point::new(0.0, 50.0)).unwrap();
                for i in 1..100 {
                    w.line_to(Point::new(f64::from(i), if i % 2 == 0 { 50.0 } else { 52.0 })).unwrap();
                }
            },
            |n| {
                n.flags = HAS_STROKE;
                n.stroke = [0.0, 0.0, 0.0, 1.0];
                n.stroke_width = 10.0;
                n.miter_limit = limit;
            },
        );
        let t = b.tree(100.0, 100.0);
        let est = t.estimate(&RenderOptions::DEFAULT, &Never).unwrap();
        // 副走査線の重なりの確かめ（ADR-103）で辺の行の手間が増えた分を見込んで 2²⁷（既定の上限よりはるかに小さい）
        assert!(est.render.work < 1 << 27, "{limit}: {est:?}");
        render_full(&t, 1.0).unwrap();
    }
}

/// キャンバスの横の外へ長く延びる線と塗りの描く見積もりは、横の外の長さで膨らまない（描く段は範囲の左の外を 1 列に
/// まとめ、右の外を落とす。ファズの作り直しの種 `dash-huge-line` で見つけたゆるさ）。延びた線は、キャンバスの少し外まで
/// の線と見積もりがほぼ同じで、見積もりの内で描ける。
#[test]
fn off_canvas_length_is_not_counted_for_raster() {
    let mk = |reach: f64, dashed: bool, fill: bool| {
        let mut b = Builder::new();
        let d = b.dashes(&[3.0, 2.0]);
        b.add(
            |w| {
                w.move_to(Point::new(-reach, 150.0)).unwrap();
                w.line_to(Point::new(200.0 + reach, 160.0)).unwrap();
                if fill {
                    w.line_to(Point::new(200.0 + reach, 170.0)).unwrap();
                    w.line_to(Point::new(-reach, 165.0)).unwrap();
                    w.close().unwrap();
                }
            },
            |n| {
                if fill {
                    n.flags = HAS_FILL;
                    n.fill = GREEN;
                } else {
                    n.flags = HAS_STROKE;
                    n.stroke = RED;
                    n.stroke_width = 3.0;
                    if dashed {
                        n.dashes = d;
                    }
                }
            },
        );
        // 曲線もキャンバスの横の外に置く（制御多角形の y の差で抑える）
        b.add(
            |w| {
                w.move_to(Point::new(-reach, 20.0)).unwrap();
                w.cubic_to(Point::new(-reach - 50.0, 40.0), Point::new(-reach + 50.0, 60.0), Point::new(-reach, 80.0))
                    .unwrap();
            },
            |n| {
                n.flags = HAS_STROKE;
                n.stroke = RED;
                n.stroke_width = 2.0;
            },
        );
        b.tree(200.0, 200.0)
    };
    for (dashed, fill) in [(true, false), (false, false), (false, true)] {
        let near = mk(10.0, dashed, fill).estimate(&RenderOptions::DEFAULT, &Never).unwrap();
        let far_tree = mk(1e8, dashed, fill);
        let far = far_tree.estimate(&RenderOptions::DEFAULT, &Never).unwrap();
        assert!(
            far.render.work <= near.render.work.saturating_mul(2),
            "破線 {dashed}・塗り {fill}：延びた線の描く見積もり {} が近い線 {} の 2 倍を超える",
            far.render.work,
            near.render.work
        );
        render_full(&far_tree, 1.0).unwrap();
    }
}

/// 曲線のあとに接線の続く直線が来る線の端（照合の control-points-clamping-1）：端の区間が直線なら、端は直線の向きで作る
/// （曲線の弦からの外挿は、端の 2 本の弦が同じ曲線のときだけ。つなぎ目の点を曲線の内側の点と分ける）。始めの側も同じ。
#[test]
fn line_after_curve_keeps_its_cap_direction() {
    for reversed in [false, true] {
        let mut b = Builder::new();
        b.add(
            |w| {
                if reversed {
                    // 直線で始まり、曲線へ接線が続く
                    w.move_to(Point::new(15.0, 175.0)).unwrap();
                    w.line_to(Point::new(175.0, 175.0)).unwrap();
                    w.cubic_to(Point::new(175.1, 175.0), Point::new(175.0, 30.0), Point::new(175.0, 15.0)).unwrap();
                } else {
                    w.move_to(Point::new(175.0, 15.0)).unwrap();
                    w.cubic_to(Point::new(175.0, 30.0), Point::new(175.1, 175.0), Point::new(175.0, 175.0)).unwrap();
                    w.line_to(Point::new(15.0, 175.0)).unwrap();
                }
            },
            |n| {
                n.flags = HAS_STROKE;
                n.stroke = GREEN;
                n.stroke_width = 20.0;
            },
        );
        let (w, _, px) = draw(&b.tree(200.0, 200.0), 1.0);
        let alpha = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
        // butt の端は x = 15 の縦の縁：線の太さの中のどの行も、x = 14 は透明、x = 15 は不透明
        for y in 166..184 {
            assert_eq!((alpha(14, y), alpha(15, y)), (0, 255), "逆向き {reversed}・y = {y}：端が斜め");
        }
    }
}

/// 試しの木：パスを 1 本書いて線を引く（`dashes` が空なら実線）。
fn stroked(build: impl FnOnce(&mut PathWriter<'_, '_>), width: f64, join: u8, cap: u8, dashes: &[f64]) -> Tree {
    let mut b = Builder::new();
    let d = b.dashes(dashes);
    b.add(build, |n| {
        n.flags = HAS_STROKE;
        n.stroke = GREEN;
        n.stroke_width = width;
        n.join = join;
        n.cap = cap;
        if !dashes.is_empty() {
            n.dashes = d;
        }
    });
    b.tree(200.0, 200.0)
}

/// 開いた線の最初の破線が最初の弦の中で終わっても、始めの端は曲線の向き（弦の向きでない）で作る（単位 1 の 6 回目の
/// レビュー。円弧の始めの接線は真上なので、butt の端は y = 120 の横の縁）。
#[test]
fn first_dash_on_a_curve_starts_along_the_curve() {
    let t = stroked(
        |w| {
            w.move_to(Point::new(40.0, 120.0)).unwrap();
            w.arc_to(30.0, 30.0, 0.0, false, true, Point::new(100.0, 120.0)).unwrap();
        },
        40.0,
        JOIN_MITER,
        CAP_BUTT,
        &[2.0, 1000.0],
    );
    let (w, _, px) = draw(&t, 1.0);
    for x in 15..66u32 {
        assert_eq!(px[((121 * w + x) * 4 + 3) as usize], 0, "x = {x}：始めの端が傾く");
    }
}

/// 曲線に接する角の外側の継ぎ目は、曲線の側を接線の向きで作る（単位 1 の 6 回目のレビュー）。接線のなす角が miterlimit 4
/// の境より鋭い角は bevel（弦の向きでは miter の尖りが出ていた）、境より鈍い角は miter（弦の向きでは bevel だった）。
#[test]
fn joins_beside_curves_use_the_curve_direction() {
    let sharp = stroked(
        |w| {
            w.move_to(Point::new(20.0, 170.0)).unwrap();
            w.quad_to(Point::new(80.65, 107.6), Point::new(100.0, 30.0)).unwrap();
            w.quad_to(Point::new(119.35, 107.6), Point::new(180.0, 170.0)).unwrap();
        },
        20.0,
        JOIN_MITER,
        CAP_BUTT,
        &[],
    );
    let (w, _, px) = draw(&sharp, 1.0);
    assert_eq!(px[((18 * w + 100) * 4 + 3) as usize], 0, "鋭い角に miter の尖りが出る");
    let blunt = stroked(
        |w| {
            w.move_to(Point::new(60.0, 190.0)).unwrap();
            w.quad_to(Point::new(120.0, 120.0), Point::new(100.0, 50.0)).unwrap();
            w.quad_to(Point::new(80.0, 120.0), Point::new(140.0, 190.0)).unwrap();
        },
        12.0,
        JOIN_MITER,
        CAP_BUTT,
        &[],
    );
    let (w, _, px) = draw(&blunt, 1.0);
    assert!(px[((35 * w + 100) * 4 + 3) as usize] > 200, "鈍い角に miter の先がない");
}

/// 破線の部分の端が、直線と曲線のつなぎ目のすぐ後ろの曲線の上にあるとき、端は曲線の向き（つなぎ目の弦の二等分でない）で
/// 作る（単位 1 の 6 回目のレビュー）。円弧は真横の向きで始まり、1 進んだ所の端は 1.4° だけ傾いた縁（上の端で x ≈ 61.5、
/// 下の端で x ≈ 60.5）。弦の二等分の向きでは傾きが 2 倍ほどになり、上の端が列 61 をほぼ埋め、下の端が列 60 に届かなかった。
#[test]
fn dash_ends_after_a_joint_follow_the_curve() {
    let t = stroked(
        |w| {
            w.move_to(Point::new(20.0, 60.0)).unwrap();
            w.line_to(Point::new(60.0, 60.0)).unwrap();
            w.arc_to(40.0, 40.0, 0.0, false, true, Point::new(100.0, 100.0)).unwrap();
        },
        40.0,
        JOIN_MITER,
        CAP_BUTT,
        &[41.0, 1000.0],
    );
    let (w, _, px) = draw(&t, 1.0);
    let alpha = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
    assert!(alpha(61, 42) < 180, "上の端が右へ傾きすぎる：{}", alpha(61, 42));
    assert!(alpha(60, 78) > 64, "下の端が左へ傾きすぎる：{}", alpha(60, 78));
    assert_eq!(alpha(62, 60), 0);
}

/// 破線の部分が、曲線に接する角の手前の最後の弦の中で始まって角の先の最初の弦の中で終わるとき、角の継ぎ目は実線と同じく
/// 曲線の側の外挿の向きで作る（単位 1 の 7 回目のレビュー：部分の中の弦だけでは外挿できず、弦の向きの継ぎ目で miter の
/// 尖りが出た）。最初の曲線の長さは約 163.17 なので、部分は角の約 3 手前から 3 先まで。角より上の継ぎ目の画素は実線と同じ。
#[test]
fn dash_pieces_keep_curve_sides_at_corners() {
    let path = |w: &mut PathWriter<'_, '_>| {
        w.move_to(Point::new(20.0, 170.0)).unwrap();
        w.quad_to(Point::new(80.65, 107.6), Point::new(100.0, 30.0)).unwrap();
        w.quad_to(Point::new(119.35, 107.6), Point::new(180.0, 170.0)).unwrap();
    };
    let (w, _, solid) = draw(&stroked(path, 20.0, JOIN_MITER, CAP_BUTT, &[]), 1.0);
    let (_, _, dashed) = draw(&stroked(path, 20.0, JOIN_MITER, CAP_BUTT, &[0.0, 160.0, 6.0, 1000.0]), 1.0);
    let alpha = |px: &[u8], x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
    assert!(alpha(&dashed, 100, 31) > 200, "部分が角を覆わない");
    for y in 0..=26 {
        for x in 70..130 {
            assert_eq!(alpha(&dashed, x, y), alpha(&solid, x, y), "({x}, {y}) の継ぎ目が実線と違う");
        }
    }
}

/// 太さ 1 画素以下の細い線は継ぎ目を作らない（Skia の細い線は区間ごとに描く。Chrome・resvg と同じ。
/// tests/painting/context/in-use の星の鋭い角で、miter の先が頂点の 1 画素上まで出て α 153 だった。ADR-103）。太い線は
/// 書いたとおりの miter。
#[test]
fn thin_strokes_have_no_miter_tips() {
    let star = |w: &mut PathWriter<'_, '_>| {
        w.move_to(Point::new(100.0, 15.0)).unwrap();
        for (x, y) in [(150.0, 175.0), (20.0, 75.0), (180.0, 75.0), (50.0, 175.0)] {
            w.line_to(Point::new(x, y)).unwrap();
        }
        w.close().unwrap();
    };
    let (w, _, px) = draw(&stroked(star, 1.0, JOIN_MITER, CAP_BUTT, &[]), 1.0);
    let alpha = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
    assert!(alpha(99, 14) < 32 && alpha(100, 14) < 32, "{} {}", alpha(99, 14), alpha(100, 14));
    assert!(alpha(99, 16) > 128 || alpha(100, 16) > 128);
    let (w, _, px) = draw(&stroked(star, 2.0, JOIN_MITER, CAP_BUTT, &[]), 1.0);
    assert!(px[((13 * w + 100) * 4 + 3) as usize] > 128, "太い線の miter の先");
}

/// non-scaling の線の滑らかなつなぎ目は描く座標系の接線で決める（単位 2 の 5 回目のレビュー）：縦に 60 倍する変形の下で、
/// まっすぐな 3 次曲線のあとの線分のつなぎ目は、ユーザー単位では 1.15° でも描く座標系では 50° の角なので、指定の miter で
/// 作る（同じ形を線分で書いたものと同じ画素。Chrome・resvg と同じ）。
#[test]
fn non_scaling_joints_are_judged_in_device_space() {
    let mk = |curve: bool| {
        let mut b = Builder::new();
        b.add(
            |w| {
                w.move_to(Point::new(0.0, 0.0)).unwrap();
                if curve {
                    w.cubic_to(Point::new(30.0, 0.0), Point::new(60.0, 0.0), Point::new(100.0, 0.0)).unwrap();
                } else {
                    w.line_to(Point::new(100.0, 0.0)).unwrap();
                }
                w.line_to(Point::new(200.0, 2.0)).unwrap();
            },
            |n| {
                n.flags = HAS_STROKE | NON_SCALING;
                n.stroke = GREEN;
                n.stroke_width = 20.0;
                n.join = JOIN_MITER;
                n.transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 60.0, e: 20.0, f: 20.0 };
            },
        );
        b.tree(300.0, 200.0)
    };
    let (_, _, a) = draw(&mk(true), 1.0);
    let (_, _, b) = draw(&mk(false), 1.0);
    let differ = a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0.iter()).filter(|(p, q)| p != q).count();
    assert_eq!(differ, 0, "曲線と線分で {differ} 画素違う");
}

/// 同じ向きの縁が画素に重なっても、副走査線ごとに規則を当てる（Chrome・resvg と同じ。ADR-103。単位 2 の 5 回目の
/// レビュー）：2 度書いた部分パスの縁は 1 度のときと同じ濃さ（非ゼロ）で、偶奇なら何も描かない。向きの逆な 2 つの形が画素の
/// 中で上下に接しても透けない。
#[test]
fn overlapping_edges_follow_the_rule_per_subrow() {
    let mk = |twice: bool, even_odd: bool| {
        let mut b = Builder::new();
        b.add(
            |w| {
                for _ in 0..if twice { 2 } else { 1 } {
                    w.move_to(Point::new(0.5, 0.0)).unwrap();
                    w.line_to(Point::new(4.5, 0.0)).unwrap();
                    w.line_to(Point::new(4.5, 2.0)).unwrap();
                    w.line_to(Point::new(0.5, 2.0)).unwrap();
                    w.close().unwrap();
                }
            },
            |n| {
                n.flags = HAS_FILL | if even_odd { EVEN_ODD } else { 0 };
                n.fill = GREEN;
            },
        );
        b.tree(6.0, 2.0)
    };
    let row = |t: &Tree| -> Vec<u8> {
        let (w, _, px) = draw(t, 1.0);
        (0..6).map(|x| px[((w + x) * 4 + 3) as usize]).collect()
    };
    let once = row(&mk(false, false));
    assert_eq!(once, vec![128, 255, 255, 255, 128, 0]);
    assert_eq!(row(&mk(true, false)), once, "2 度書いた部分パス（非ゼロ）");
    assert_eq!(row(&mk(true, true)), vec![0; 6], "2 度書いた部分パス（偶奇）");
    // 向きの逆な 2 つの矩形が画素の行 5 の中（y = 5.5）で接する：行 5 は全部覆う
    let mut b = Builder::new();
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 0.0)).unwrap();
            w.line_to(Point::new(10.0, 0.0)).unwrap();
            w.line_to(Point::new(10.0, 5.5)).unwrap();
            w.line_to(Point::new(0.0, 5.5)).unwrap();
            w.close().unwrap();
            w.move_to(Point::new(0.0, 5.5)).unwrap();
            w.line_to(Point::new(0.0, 10.0)).unwrap();
            w.line_to(Point::new(10.0, 10.0)).unwrap();
            w.line_to(Point::new(10.0, 5.5)).unwrap();
            w.close().unwrap();
        },
        |n| {
            n.flags = HAS_FILL;
            n.fill = GREEN;
        },
    );
    let (w, _, px) = draw(&b.tree(10.0, 10.0), 1.0);
    for x in 0..10u32 {
        assert_eq!(px[((5 * w + x) * 4 + 3) as usize], 255, "x = {x}：向きの逆な形の継ぎ目が透ける");
    }
    // なぞり返した線の縁は 1 本の線と同じ濃さ
    let line = |retrace: bool| {
        let mut b = Builder::new();
        b.add(
            |w| {
                w.move_to(Point::new(10.0, 2.0)).unwrap();
                w.line_to(Point::new(10.0, 18.0)).unwrap();
                if retrace {
                    w.line_to(Point::new(10.0, 6.0)).unwrap();
                }
            },
            |n| {
                n.flags = HAS_STROKE;
                n.stroke = GREEN;
                n.stroke_width = 1.5;
                n.cap = crate::tree::CAP_ROUND;
            },
        );
        let (w, _, px) = draw(&b.tree(20.0, 20.0), 1.0);
        px[((10 * w + 9) * 4 + 3) as usize]
    };
    assert_eq!(line(true), line(false), "なぞり返した線の縁");
}

/// 範囲 `r`（右半分・下半分・真ん中）を、範囲の見積もりを上限にして描ける（断らない）。
fn draws_regions(t: &Tree, scale: f64) {
    let plan = t.plan(&RenderOptions { scale, ..RenderOptions::DEFAULT }, &Never).unwrap();
    let (w, h) = (plan.width, plan.height);
    for r in [
        PixelRect { x: w / 2, y: 0, width: w - w / 2, height: h },
        PixelRect { x: 0, y: h / 2, width: w, height: h - h / 2 },
        PixelRect { x: w / 4, y: h / 4, width: (w / 2).max(1), height: (h / 2).max(1) },
    ] {
        let mut part = vec![0u8; (r.width * r.height * 4) as usize];
        let mut out = PixmapMut::new(&mut part, r.width, r.height, (r.width * 4) as usize).unwrap();
        plan.render_region(r, &mut out, &Never).unwrap();
    }
}

/// 単位 2 の 7 回目のレビュー：同じ矩形を何度も書いたパスと、細い縦線の密な格子を、見積もりを上限にして全体と範囲で描ける
/// （欄に入らない副走査線の交わりの push を 2 度払い、5 度書いた矩形で断っていた）。2〜8 度書いた矩形の縦の縁は 1 度の
/// ときと同じ濃さ（欄と並びを使う厳密な被覆）。9 度以上は面積の被覆に戻る（断らない）。
#[test]
fn many_crossings_stay_within_the_estimate() {
    let copies = |n: usize| {
        let mut b = Builder::new();
        b.add(
            |w| {
                for _ in 0..n {
                    rect(w, 10.3, 10.2, 20.0, 80.0, 0.0, 0.0).unwrap();
                }
            },
            |node| {
                node.flags = HAS_FILL;
                node.fill = GREEN;
            },
        );
        b.tree(50.0, 100.0)
    };
    let edges = |t: &Tree| {
        let (w, _, px) = draw(t, 1.0);
        (px[((50 * w + 10) * 4 + 3) as usize], px[((50 * w + 30) * 4 + 3) as usize])
    };
    let one = edges(&copies(1));
    for n in [2, 4, 5, 6, 8] {
        assert_eq!(edges(&copies(n)), one, "{n} 度書いた矩形");
    }
    for n in [5, 6, 9, 12] {
        let t = copies(n);
        let _ = edges(&t);
        for scale in [0.5, 1.0, 3.0] {
            draws_regions(&t, scale);
        }
    }
    // 0.25 画素おきの幅 0.1 の縦線 200 本（1 画素に 4 本：列の欄に 8 つの交わり）
    let mut b = Builder::new();
    b.add(
        |w| {
            for i in 0..200 {
                rect(w, 10.0 + i as f64 * 0.25, 10.0, 0.1, 80.0, 0.0, 0.0).unwrap();
            }
        },
        |node| {
            node.flags = HAS_FILL;
            node.fill = GREEN;
        },
    );
    let hatch = b.tree(80.0, 100.0);
    let _ = draw(&hatch, 1.0);
    for scale in [0.5, 1.0, 2.0] {
        draws_regions(&hatch, scale);
    }
}

/// 重ならない細い楔は面積の被覆のまま（左と右の縁が同じ画素を横切っても、巻き数の幅は規則の線形な区間の内。単位 2 の
/// 6 回目のレビュー：Chrome は 13・7）。
#[test]
fn thin_wedges_keep_the_area_coverage() {
    let mut b = Builder::new();
    b.add(
        |w| {
            w.move_to(Point::new(50.0, 50.0)).unwrap();
            w.line_to(Point::new(190.0, 40.3)).unwrap();
            w.line_to(Point::new(190.0, 41.0)).unwrap();
            w.close().unwrap();
        },
        |node| {
            node.flags = HAS_FILL;
            node.fill = GREEN;
        },
    );
    let (w, _, px) = draw(&b.tree(200.0, 100.0), 1.0);
    let alpha = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
    assert!((10..=15).contains(&alpha(59, 49)), "{}", alpha(59, 49));
    assert!((5..=9).contains(&alpha(55, 49)), "{}", alpha(55, 49));
}

/// Chrome が枠で描く細い rect は本当の太さで描く（太さ 0.5 の縁 x = 10.3 は列 10 に半分、列 9 に入らない。1 画素の線 × α
/// なら列 9 に 26・列 10 に 102。ADR-104 (4)(c)）。
#[test]
fn thin_rect_frames_use_the_true_width() {
    let mut b = Builder::new();
    b.add(
        |w| rect(w, 10.3, 10.3, 30.0, 30.0, 0.0, 0.0).unwrap(),
        |node| {
            node.flags = HAS_STROKE;
            node.stroke = GREEN;
            node.stroke_width = 0.5;
            node.shape = crate::tree::SHAPE_RECT;
        },
    );
    let (w, _, px) = draw(&b.tree(50.0, 50.0), 1.0);
    let alpha = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
    assert_eq!((alpha(9, 25), alpha(10, 25)), (0, 128));
}

/// 決まった箱（±2²² 画素）の右の外へ延びる図形も、キャンバスの中の部分を描く（右の外の部分は箱の右の縁の縦の辺にする。
/// 単位 1 の 9 回目のレビュー：項目の矩形が残った左の辺で止まり、何も描かなかった）。
#[test]
fn shapes_beyond_the_right_of_the_box_are_drawn() {
    for (x, wd) in [(-100.0, 5_000_000.0), (-10_000_000.0, 20_000_000.0)] {
        let mut b = Builder::new();
        b.add(
            |w| rect(w, x, 20.0, wd, 20.0, 0.0, 0.0).unwrap(),
            |node| {
                node.flags = HAS_FILL;
                node.fill = GREEN;
            },
        );
        let t = b.tree(60.0, 60.0);
        let (w, _, px) = draw(&t, 1.0);
        assert_eq!(px[((30 * w + 30) * 4 + 3) as usize], 255, "{x} {wd}");
        assert_eq!(px[((30 * w + 59) * 4 + 3) as usize], 255, "{x} {wd}");
        draws_regions(&t, 1.0);
    }
}
