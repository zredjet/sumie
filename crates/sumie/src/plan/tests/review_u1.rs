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
//! 単位 1 の 3 回目のレビューの再現（見積もりが通れば計画と描く段が断らない・破線の落とし方・pathLength・3 次曲線の外接矩形）。

use super::*;
use crate::tree::{CAP_ROUND, JOIN_BEVEL};

/// 見積もりの計画の分（断れば None）と計画。見積もりが断れば、計画も同じ誤りで断る（architecture.md §3.1）。
fn try_plan(t: &Tree, scale: f64) -> (Option<Estimate>, Result<Plan, Error>) {
    let opts = RenderOptions { scale, ..RenderOptions::DEFAULT };
    let est = t.estimate(&opts, &Never);
    let plan = t.plan(&opts, &Never);
    if let Err(e) = &est {
        assert_eq!(plan.as_ref().err(), Some(e));
    }
    (est.ok().map(|e| e.plan), plan)
}

/// (A) 見る箱の角の外を斜めに通る線分：見積もりは Liang–Barsky で「箱にかからない」とし、計画の Cull は弦の外接矩形で
/// 「かかる」として破線の部分を作る。
#[test]
fn corner_chords_dash_pieces() {
    for n_sub in [10usize, 100, 1000] {
        let mut b = Builder::new();
        let d = b.dashes(&[2000.0, 1.0]);
        b.add(
            |w| {
                for _ in 0..n_sub {
                    w.move_to(Point::new(-1000.0, 0.0)).unwrap();
                    w.line_to(Point::new(0.0, -1000.0)).unwrap();
                }
            },
            |n| {
                n.flags = HAS_STROKE;
                n.stroke = [0.0, 0.0, 0.0, 1.0];
                n.stroke_width = 20.0;
                n.cap = CAP_ROUND;
                n.join = JOIN_BEVEL;
                n.dashes = d;
            },
        );
        let t = b.tree(100.0, 100.0);
        let (est, r) = try_plan(&t, 1.0);
        match r {
            Ok(p) => eprintln!("A n={n_sub}: OK edges={} est={:?}", p.edges.len(), est),
            Err(e) => eprintln!("A n={n_sub}: ERR {e:?} est={:?}", est),
        }
    }
}

/// (B) non-scaling-stroke と pathLength と破線と、一様でない変形の下の曲線：見積もりの倍（制御多角形の長さ）と計画の倍
/// （平坦化した長さ）の比が、描く座標系と要素の座標系で違う。
#[test]
fn non_scaling_path_length_nonuniform() {
    for pl in [2.0e6, 2.0e7] {
        let mut b = Builder::new();
        let d = b.dashes(&[1.0, 1.0]);
        b.add(
            |w| {
                w.move_to(Point::new(0.0, 50.0e6)).unwrap();
                w.line_to(Point::new(1000.0, 50.0e6)).unwrap();
                w.move_to(Point::new(500.0, 50.0e6)).unwrap();
                w.cubic_to(Point::new(500.0, 51.0e6), Point::new(500.0, 49.0e6), Point::new(500.0, 50.0e6)).unwrap();
            },
            |n| {
                n.flags = HAS_STROKE | NON_SCALING;
                n.stroke = [0.0, 0.0, 0.0, 1.0];
                n.stroke_width = 20.0;
                n.cap = CAP_ROUND;
                n.join = JOIN_BEVEL;
                n.dashes = d;
                n.path_length = pl;
                n.transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1e-6, e: 0.0, f: 0.0 };
            },
        );
        let t = b.tree(1000.0, 100.0);
        let (est, r) = try_plan(&t, 1.0);
        match r {
            Ok(p) => eprintln!("B pl={pl}: OK edges={} est={:?}", p.edges.len(), est),
            Err(e) => eprintln!("B pl={pl}: ERR {e:?} est={:?}", est),
        }
    }
}

/// (A) と (B) の対照。
#[test]
fn controls() {
    // A の対照：外接矩形も箱にかからない線分
    let mut b = Builder::new();
    let d = b.dashes(&[2000.0, 1.0]);
    b.add(
        |w| {
            for _ in 0..1000 {
                w.move_to(Point::new(-1000.0, -100.0)).unwrap();
                w.line_to(Point::new(-100.0, -1000.0)).unwrap();
            }
        },
        |n| {
            n.flags = HAS_STROKE;
            n.stroke = [0.0, 0.0, 0.0, 1.0];
            n.stroke_width = 20.0;
            n.cap = CAP_ROUND;
            n.join = JOIN_BEVEL;
            n.dashes = d;
        },
    );
    let t = b.tree(100.0, 100.0);
    let (_, r) = try_plan(&t, 1.0);
    eprintln!("A-control: {:?}", r.map(|p| p.edges.len()));
    // B の対照：non-scaling なし・一様な変形
    for (ns, tr) in [
        (false, Transform { a: 1.0, b: 0.0, c: 0.0, d: 1e-6, e: 0.0, f: 0.0 }),
        (true, Transform { a: 1e-3, b: 0.0, c: 0.0, d: 1e-3, e: 0.0, f: 0.0 }),
    ] {
        let mut b = Builder::new();
        let d = b.dashes(&[1.0, 1.0]);
        b.add(
            |w| {
                w.move_to(Point::new(0.0, 50.0e6)).unwrap();
                w.line_to(Point::new(1000.0, 50.0e6)).unwrap();
                w.move_to(Point::new(500.0, 50.0e6)).unwrap();
                w.cubic_to(Point::new(500.0, 51.0e6), Point::new(500.0, 49.0e6), Point::new(500.0, 50.0e6)).unwrap();
            },
            |n| {
                n.flags = HAS_STROKE | if ns { NON_SCALING } else { 0 };
                n.stroke = [0.0, 0.0, 0.0, 1.0];
                n.stroke_width = 20.0;
                n.cap = CAP_ROUND;
                n.join = JOIN_BEVEL;
                n.dashes = d;
                n.path_length = 2.0e6;
                n.transform = tr;
            },
        );
        let t = b.tree(1000.0, 100.0);
        let (_, r) = try_plan(&t, 1.0);
        eprintln!("B-control ns={ns}: {:?}", r.map(|p| p.edges.len()));
    }
}

#[test]
fn control_b_uniform() {
    let mut b = Builder::new();
    let d = b.dashes(&[1.0, 1.0]);
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 50.0e3)).unwrap();
            w.line_to(Point::new(1.0e6, 50.0e3)).unwrap();
            w.move_to(Point::new(5.0e5, 50.0e3)).unwrap();
            w.cubic_to(Point::new(5.0e5, 51.0e3), Point::new(5.0e5, 49.0e3), Point::new(5.0e5, 50.0e3)).unwrap();
        },
        |n| {
            n.flags = HAS_STROKE | NON_SCALING;
            n.stroke = [0.0, 0.0, 0.0, 1.0];
            n.stroke_width = 20.0;
            n.cap = CAP_ROUND;
            n.join = JOIN_BEVEL;
            n.dashes = d;
            n.path_length = 2.0e4;
            n.transform = Transform { a: 1e-3, b: 0.0, c: 0.0, d: 1e-3, e: 0.0, f: 0.0 };
        },
    );
    let t = b.tree(1000.0, 100.0);
    let (est, r) = try_plan(&t, 1.0);
    eprintln!("B-uniform: {:?} est={:?}", r.map(|p| p.edges.len()), est);
}

fn svg_plan(src: &str) -> String {
    use crate::style::ParseOptions;
    let d = crate::doc::parse(src.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    let t = d.build(&Never).unwrap();
    let opts = RenderOptions::DEFAULT;
    let est = match t.estimate(&opts, &Never) {
        Ok(e) => e,
        Err(e) => return format!("estimate ERR {e:?}"),
    };
    match t.plan(&opts, &Never) {
        Ok(p) => format!("OK edges={} est={:?}", p.edges.len(), est),
        Err(e) => format!("plan ERR {e:?} est={:?}", est),
    }
}

/// (C) 破線の合計があふれる（見積もりは破線として数え、計画は実線）。(D) pathLength で平坦化した長さが 0（計画は実線）。
#[test]
fn dash_mode_mismatch() {
    let mut sub = String::new();
    for _ in 0..1000 {
        sub.push_str("M -500 -500 L -400 -500 ");
    }
    for da in ["1e308", "1e308 1e308", "5 5"] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="{sub}" fill="none" stroke="black" stroke-width="40" stroke-linecap="round" stroke-dasharray="{da}"/></svg>"#
        );
        eprintln!("C da={da}: {}", svg_plan(&src));
    }
    let mut sub = String::new();
    for _ in 0..1000 {
        sub.push_str("M -100 -100 q 0.01 0 0 0 ");
    }
    let src = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="{sub}" pathLength="1" fill="none" stroke="black" stroke-width="40" stroke-linecap="round" stroke-dasharray="1 1"/></svg>"#
    );
    eprintln!("D: {}", svg_plan(&src));
}

/// (E) non-scaling でなくても：pathLength のとき、見積もりの倍は制御多角形の長さ（大きい）で、計画の倍は平坦化した長さ。
/// 見る箱にかかる長さを見積もりの倍の周期で割るので、箱の外の曲線が長さを水増しすると下回る。
#[test]
fn path_length_visible_dashes() {
    for (name, extra) in
        [("far-quad-outside-box", "M 5e6 50 Q 6e6 50 5e6 50"), ("far-cubic-inside-box", "M 5000 50 c 0 1e6 0 -1e6 0 0")]
    {
        for pl in ["2000", "20000"] {
            let src = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="100"><path d="M 0 50 L 1000 50 {extra}" pathLength="{pl}" fill="none" stroke="black" stroke-width="20" stroke-linecap="round" stroke-dasharray="1 1"/></svg>"#
            );
            eprintln!("E {name} pl={pl}: {}", svg_plan(&src));
        }
    }
}

/// (G) round の継ぎ目の点の数：乱れたパス（曲線・弧・一様でない変形・non-scaling）で計画が見積もりの内か。
#[test]
fn random_round_joins_fit() {
    use crate::tree::JOIN_ROUND;
    let mut r = Rng(0xfeed_beef_1234);
    let mut fails = 0;
    for case in 0..3000 {
        let mut b = Builder::new();
        let ns = r.i(2) == 0;
        let closed = r.i(2) == 0;
        let segs = 3 + r.i(10);
        let mut kinds = Vec::new();
        let mut pts = Vec::new();
        for _ in 0..segs {
            kinds.push(r.i(4));
            let p: Vec<(f64, f64)> = (0..3).map(|_| (r.f(0.0, 100.0), r.f(0.0, 100.0))).collect();
            pts.push(p);
        }
        let arcp = (r.f(1.0, 80.0), r.f(0.5, 80.0), r.f(0.0, 360.0));
        let t = Transform {
            a: r.f(0.05, 3.0),
            b: r.f(-2.0, 2.0),
            c: r.f(-2.0, 2.0),
            d: r.f(0.05, 3.0),
            e: r.f(-50.0, 50.0),
            f: r.f(-50.0, 50.0),
        };
        let width = r.f(20.0, 400.0);
        let flags = [r.i(2), r.i(2)];
        b.add(
            |w| {
                w.move_to(Point::new(50.0, 50.0)).unwrap();
                for (k, p) in kinds.iter().zip(pts.iter()) {
                    let q = |i: usize| Point::new(p[i].0, p[i].1);
                    match k {
                        0 => w.line_to(q(0)).unwrap(),
                        1 => w.quad_to(q(0), q(1)).unwrap(),
                        2 => w.cubic_to(q(0), q(1), q(2)).unwrap(),
                        _ => w.arc_to(arcp.0, arcp.1, arcp.2, flags[0] == 1, flags[1] == 1, q(0)).unwrap(),
                    }
                }
                if closed {
                    w.close().unwrap();
                }
            },
            |n| {
                n.flags = HAS_STROKE | if ns { NON_SCALING } else { 0 };
                n.stroke = [0.0, 0.0, 0.0, 1.0];
                n.stroke_width = width;
                n.join = JOIN_ROUND;
                n.transform = t;
            },
        );
        let tr = b.tree(200.0, 200.0);
        let opts = RenderOptions::DEFAULT;
        let Ok(_) = tr.estimate(&opts, &Never) else { continue };
        if let Err(e) = tr.plan(&opts, &Never)
            && matches!(e, Error::Limit(ref l) if l.stage == Stage::Plan)
        {
            fails += 1;
            if fails < 5 {
                eprintln!("G case {case}: {e:?} ns={ns} closed={closed} t={t:?} width={width}");
            }
        }
    }
    assert_eq!(fails, 0);
}

fn svg_pixels(src: &str) -> (u32, Vec<u8>) {
    use crate::style::ParseOptions;
    let d = crate::doc::parse(src.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    let t = d.build(&Never).unwrap();
    let opts = RenderOptions::DEFAULT;
    let plan = t.plan(&opts, &Never).unwrap();
    let (w, h) = (plan.width, plan.height);
    let mut px = vec![0u8; (w * h * 4) as usize];
    let mut out = PixmapMut::new(&mut px, w, h, (w * 4) as usize).unwrap();
    plan.render(&mut out, &Never).unwrap();
    (w, px)
}

/// (H) 境の足し算が進まないと、部分パスの残りを描かない。
#[test]
fn stuck_dash_drops_rest() {
    for da in ["99999 1e-12", "99999 1e-6", "99999 1"] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="M -99950 50 L 100050 50" fill="none" stroke="black" stroke-width="10" stroke-dasharray="{da}"/></svg>"#
        );
        let (w, px) = svg_pixels(&src);
        let a = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
        eprintln!("H da={da}: alpha x=25 {} x=75 {}", a(25, 50), a(75, 50));
    }
}

/// (J) ほぼ 2 次の 3 次曲線で、厳密な外接矩形が極を落とす（2 次の根の公式の桁落ち）。
#[test]
fn cubic_bounds_cancellation() {
    use crate::geom::{PathSeg, add_bounds, cubic_at};
    let mut worst = (0.0f64, 0.0f64, 0.0f64);
    for scale in [1.0f64, 100.0, 1e4, 1e6] {
        let base = -3.0 * scale;
        let mut d = base;
        for _ in 0..8 {
            d = f64::from_bits(d.to_bits() - 1); // -3s に向けて 1 ulp ずつ（0 に近づく）
            let (p0, p1, p2, p3) =
                (Point::new(0.0, 0.0), Point::new(scale, 0.0), Point::new(0.0, 0.0), Point::new(d, 0.0));
            let mut r = crate::geom::Rect::EMPTY;
            add_bounds(&mut r, &PathSeg::Cubic(p0, p1, p2, p3));
            let mut mx = f64::NEG_INFINITY;
            for i in 0..=10000 {
                let q = cubic_at(p0, p1, p2, p3, i as f64 / 10000.0);
                if q.x > mx {
                    mx = q.x;
                }
            }
            let miss = mx - r.x1;
            if miss > worst.0 {
                worst = (miss, scale, d);
            }
        }
    }
    eprintln!("J worst miss={} scale={} d={:e}", worst.0, worst.1, worst.2);
}

/// (J2) 外接矩形が極を落とした節の、描く段の見積もり。
#[test]
fn cubic_bounds_render_estimate() {
    use crate::geom::{PathSeg, add_bounds};
    let s = 2000.0f64;
    let mut d = -3.0 * s;
    let mut found = None;
    for _ in 0..64 {
        d = f64::from_bits(d.to_bits() - 1);
        let mut r = crate::geom::Rect::EMPTY;
        add_bounds(
            &mut r,
            &PathSeg::Cubic(Point::new(0.0, 0.0), Point::new(0.0, s), Point::new(1000.0, 0.0), Point::new(1000.0, d)),
        );
        if r.y1 < 1.0 {
            found = Some(d);
            break;
        }
    }
    let Some(d) = found else {
        eprintln!("J2 not found");
        return;
    };
    let mut b = Builder::new();
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 0.0)).unwrap();
            w.cubic_to(Point::new(0.0, s), Point::new(1000.0, 0.0), Point::new(1000.0, d)).unwrap();
            w.close().unwrap();
        },
        |n| {
            n.flags = HAS_FILL;
            n.fill = [0.0, 0.0, 0.0, 1.0];
        },
    );
    eprintln!("J2 d={d:e} bounds={:?}", b.nodes[0].bounds);
    let t = b.tree(1000.0, 1000.0);
    let opts = RenderOptions::DEFAULT;
    let est = t.estimate(&opts, &Never).unwrap();
    let plan = t.plan(&opts, &Never).unwrap();
    let mut px = vec![0u8; 1000 * 1000 * 4];
    let mut out = PixmapMut::new(&mut px, 1000, 1000, 4000).unwrap();
    let r = plan.render(&mut out, &Never);
    let full = plan.estimate_region(PixelRect { x: 0, y: 0, width: 1000, height: 1000 }, &Never);
    eprintln!("J2 render={r:?} est.render={:?} region_est={full:?}", est.render);
}

/// (J3) 落とした極が箱の左の境（−2²²）をまたぐ：分ける辺を数えない。
#[test]
fn cubic_bounds_box_split() {
    use crate::geom::{PathSeg, add_bounds};
    let s = 2.0e7f64;
    let mut d = 3.0 * s;
    let mut found = None;
    for _ in 0..64 {
        d = f64::from_bits(d.to_bits() - 1);
        let mut r = crate::geom::Rect::EMPTY;
        add_bounds(
            &mut r,
            &PathSeg::Cubic(Point::new(0.0, 0.0), Point::new(-s, 30.0), Point::new(0.0, 60.0), Point::new(d, 90.0)),
        );
        if r.x0 > -1.0 {
            found = Some(d);
            break;
        }
    }
    let Some(d) = found else {
        eprintln!("J3 not found");
        return;
    };
    let mut b = Builder::new();
    for _ in 0..1 {
        b.add(
            |w| {
                w.move_to(Point::new(0.0, 0.0)).unwrap();
                w.cubic_to(Point::new(-s, 30.0), Point::new(0.0, 60.0), Point::new(d, 90.0)).unwrap();
                w.close().unwrap();
            },
            |n| {
                n.flags = HAS_FILL;
                n.fill = [0.0, 0.0, 0.0, 1.0];
            },
        );
    }
    eprintln!("J3 d={d:e} bounds={:?}", b.nodes[0].bounds);
    let t = b.tree(100.0, 100.0);
    let opts = RenderOptions::DEFAULT;
    let est = t.estimate(&opts, &Never).unwrap();
    let plan = t.plan(&opts, &Never);
    eprintln!("J3 plan={:?} est={:?}", plan.map(|p| p.edges.len()), est.plan);
}

/// (J4) ほぼ 2 次の 3 次曲線（両端 0・頂 −M）。落とした極が箱の左の境をまたぐ。
#[test]
fn cubic_bounds_box_split2() {
    use crate::geom::{PathSeg, add_bounds};
    let m = 6.0e6f64;
    let c = -4.0 * m / 3.0;
    let mut found = None;
    for k in 1..2000 {
        let d = k as f64 * 1e-10;
        let mut r = crate::geom::Rect::EMPTY;
        add_bounds(
            &mut r,
            &PathSeg::Cubic(Point::new(0.0, 10.0), Point::new(c, 40.0), Point::new(c, 60.0), Point::new(d, 90.0)),
        );
        if r.x0 > -1.0 {
            found = Some(d);
            break;
        }
    }
    let Some(d) = found else {
        eprintln!("J4 not found");
        return;
    };
    for nsub in [1usize, 50] {
        let mut b = Builder::new();
        b.add(
            |w| {
                for _ in 0..nsub {
                    w.move_to(Point::new(0.0, 10.0)).unwrap();
                    w.cubic_to(Point::new(c, 40.0), Point::new(c, 60.0), Point::new(d, 90.0)).unwrap();
                    w.close().unwrap();
                }
            },
            |n| {
                n.flags = HAS_FILL;
                n.fill = [0.0, 0.0, 0.0, 1.0];
            },
        );
        eprintln!("J4 d={d:e} bounds={:?}", b.nodes[0].bounds);
        let t = b.tree(100.0, 100.0);
        let opts = RenderOptions::DEFAULT;
        let est = t.estimate(&opts, &Never).unwrap();
        let plan = t.plan(&opts, &Never);
        eprintln!("J4 nsub={nsub} plan={:?} est={:?}", plan.map(|p| p.edges.len()), est.plan);
    }
}

#[test]
fn stuck_dash_small() {
    for da in ["100 1e-14", "100 1e-13", "30 1e-15 30 1e-15"] {
        let src = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><path d="M 0 50 L 200 50" fill="none" stroke="black" stroke-width="10" stroke-dasharray="{da}"/></svg>"#
        );
        let (w, px) = svg_pixels(&src);
        let a = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
        eprintln!("H2 da={da}: alpha x=50 {} x=150 {}", a(50, 50), a(150, 50));
    }
}

#[test]
fn corner_chords_svg() {
    let mut sub = String::new();
    for _ in 0..1000 {
        sub.push_str("M -1000 0 L 0 -1000 ");
    }
    let src = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="{sub}" fill="none" stroke="black" stroke-width="20" stroke-linecap="round" stroke-linejoin="bevel" stroke-dasharray="2000 1"/></svg>"#
    );
    eprintln!("A-svg: {}", svg_plan(&src));
    let src2 = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"100\"><path d=\"M 0 1e-310 A 1e-310 1e-310 0 0 1 100 1e-310\" fill=\"none\" stroke=\"black\" stroke-width=\"4\"/></svg>";
    let (w, px) = svg_pixels(src2);
    let a = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
    eprintln!("arc-denormal: alpha(50,50)={} alpha(50,0)={}", a(50, 50), a(50, 0));
}

/// (K) 箱の外の曲線を弦にする近道が、戻ってきた線の破線の位相を変える（倍率で絵が変わる）。
#[test]
fn box_shortcut_dash_phase() {
    use crate::style::ParseOptions;
    let src = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><path d="M 0 40 L 5e6 40 Q 6000005 40 5e6 60 L 0 60" fill="none" stroke="black" stroke-width="4" stroke-dasharray="10 10"/></svg>"#;
    let d = crate::doc::parse(src.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    let t = d.build(&Never).unwrap();
    for scale in [1.0f64, 0.5] {
        let opts = RenderOptions { scale, ..RenderOptions::DEFAULT };
        let plan = t.plan(&opts, &Never).unwrap();
        let (w, h) = (plan.width, plan.height);
        let mut px = vec![0u8; (w * h * 4) as usize];
        let mut out = PixmapMut::new(&mut px, w, h, (w * 4) as usize).unwrap();
        plan.render(&mut out, &Never).unwrap();
        let y = (60.0 * scale) as u32;
        let row: String = (0..(40.0 * scale) as u32)
            .map(|x| if px[((y * w + x) * 4 + 3) as usize] > 128 { '#' } else { '.' })
            .collect();
        let y0 = (40.0 * scale) as u32;
        let row0: String = (0..(40.0 * scale) as u32)
            .map(|x| if px[((y0 * w + x) * 4 + 3) as usize] > 128 { '#' } else { '.' })
            .collect();
        eprintln!("K scale={scale}: y=40 {row0} | y=60 {row}");
    }
}

#[test]
fn cubic_bounds_render_limit() {
    let s = 2000.0f64;
    let d = -5.999999999999999e3;
    let mut b = Builder::new();
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 0.0)).unwrap();
            w.cubic_to(Point::new(0.0, s), Point::new(1000.0, 0.0), Point::new(1000.0, d)).unwrap();
            w.close().unwrap();
        },
        |n| {
            n.flags = HAS_FILL;
            n.fill = [0.0, 0.0, 0.0, 1.0];
        },
    );
    let t = b.tree(1000.0, 1000.0);
    let mut opts = RenderOptions::DEFAULT;
    opts.limits.render_work = 1_000_000;
    // 外接矩形が極を落とさなければ、描く手間の見積もりが上限を超えて、計画の前に断る（描く段で断らない）
    match t.plan(&opts, &Never) {
        Err(Error::Limit(l)) if l.stage == Stage::Render => {}
        Ok(plan) => {
            let mut px = vec![0u8; 1000 * 1000 * 4];
            let mut out = PixmapMut::new(&mut px, 1000, 1000, 4000).unwrap();
            plan.render(&mut out, &Never).unwrap();
        }
        Err(e) => panic!("{e:?}"),
    }
}
