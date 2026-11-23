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
//! 単位 2 の 2 回目のレビューの再現（見積もりが通れば計画が断らない・キャンバスの大きさ・有限でない節）。

use super::*;

fn big() -> RenderLimits {
    RenderLimits {
        plan_work: u64::MAX,
        plan_bytes: u64::MAX,
        render_work: u64::MAX,
        temp_bytes: u64::MAX,
        estimate_work: u64::MAX,
        estimate_bytes: u64::MAX,
        ..RenderLimits::DEFAULT
    }
}

fn plan_result(t: &Tree, scale: f64) -> (RenderEstimate, Result<Plan, Error>) {
    let opts = RenderOptions { scale, limits: big() };
    let est = t.estimate(&opts, &Never).unwrap();
    (est, t.plan(&opts, &Never))
}

/// 破線の部分を落とす所（Cull::visible）は区間の外接矩形で見るが、見積もりは区間そのもの（Liang–Barsky）で数える。
/// 区間は見る箱にかからないのに外接矩形はかかる、を並べると、作る部分の数が見積もりを超える。
#[test]
fn r1_cull_bbox_vs_segment() {
    let mut b = Builder::new();
    let d = b.dashes(&[150.0, 0.5]);
    b.add(
        |w| {
            w.move_to(Point::new(-45.0, 5.0)).unwrap();
            for i in 0..4000 {
                let p = if i % 2 == 0 { Point::new(5.0, -45.0) } else { Point::new(-45.0, 5.0) };
                w.line_to(p).unwrap();
            }
        },
        |n| {
            n.flags = HAS_STROKE;
            n.stroke = [0.0, 0.0, 0.0, 1.0];
            n.stroke_width = 0.2;
            n.dashes = d;
        },
    );
    let t = b.tree(100.0, 100.0);
    let (est, r) = plan_result(&t, 1.0);
    match r {
        Ok(p) => println!("r1: ok edges {} est {:?}", p.edges.len(), est.plan),
        Err(e) => panic!("r1: 見積もり {:?} の内で計画が作れない：{e:?}", est.plan),
    }
}

/// pathLength の倍：見積もりは制御多角形の長さ（len_user）で割るが、計画は平坦化した長さで割る。見る箱の中の破線の部分の
/// 数（vis）は、箱の外の長い曲線（制御多角形が曲線の 3.4 倍）があると見積もりが実際の 1/3.4 になる。round の太い端では
/// 部分ごとの足し分でも補えない。
#[test]
fn r2_path_length_factor_vis() {
    let mut b = Builder::new();
    let d = b.dashes(&[1.0, 1.0]);
    let l = 1.0e5;
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 500.0)).unwrap();
            w.line_to(Point::new(1000.0, 500.0)).unwrap();
            let o = Point::new(-1.0e6, -1.0e6);
            w.move_to(o).unwrap();
            w.cubic_to(Point::new(o.x + l, o.y), Point::new(o.x - l, o.y), o).unwrap();
        },
        |n| {
            n.flags = HAS_STROKE;
            n.stroke = [0.0, 0.0, 0.0, 1.0];
            n.stroke_width = 400.0;
            n.cap = crate::tree::CAP_ROUND;
            n.join = JOIN_ROUND;
            n.dashes = d;
            n.path_length = 116_470.0;
        },
    );
    let t = b.tree(1000.0, 1000.0);
    let (est, r) = plan_result(&t, 1.0);
    match r {
        Ok(p) => println!("r2: ok edges {} est {:?}", p.edges.len(), est.plan),
        Err(e) => panic!("r2: 見積もり {:?} の内で計画が作れない：{e:?}", est.plan),
    }
}

/// non-scaling と pathLength：倍はユーザー単位の長さで、破線の数の見積もり（all）は描く座標系の制御多角形の長さを
/// ユーザー単位の制御多角形の長さの倍で割る。歪んだ変形で 2 つの比が食い違うと、all も実際を下回る。
#[test]
fn r2b_non_scaling_path_length_skew() {
    let mut b = Builder::new();
    let d = b.dashes(&[1.0, 1.0]);
    let l = 1.0e6;
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 0.0)).unwrap();
            w.line_to(Point::new(1000.0, 0.0)).unwrap();
            let o = Point::new(500.0, 0.0);
            w.move_to(o).unwrap();
            w.cubic_to(Point::new(o.x, o.y + l), Point::new(o.x, o.y - l), o).unwrap();
        },
        |n| {
            n.flags = HAS_STROKE | NON_SCALING;
            n.stroke = [0.0, 0.0, 0.0, 1.0];
            n.stroke_width = 400.0;
            n.cap = crate::tree::CAP_ROUND;
            n.join = JOIN_ROUND;
            n.dashes = d;
            n.path_length = 1_155_700.0;
            n.transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0e-6, e: 0.0, f: 500.0 };
        },
    );
    let t = b.tree(1000.0, 1000.0);
    let (est, r) = plan_result(&t, 1.0);
    match r {
        Ok(p) => println!("r2b: ok edges {} est {:?}", p.edges.len(), est.plan),
        Err(e) => panic!("r2b: 見積もり {:?} の内で計画が作れない：{e:?}", est.plan),
    }
}

/// キャンバスが決まった箱（2²² 画素）より大きいと、箱の外のキャンバスの中の図形が落ちる（誤った絵）。
#[test]
fn r3_canvas_beyond_box() {
    let mut b = Builder::new();
    b.add(
        |w| rect(w, 4.5e6, 0.0, 1.0e5, 4.0, 0.0, 0.0).unwrap(),
        |n| {
            n.flags = HAS_FILL;
            n.fill = [0.0, 0.0, 0.0, 1.0];
        },
    );
    let t = b.tree(5.0e6, 4.0);
    let limits = RenderLimits { canvas_side: 6_000_000, canvas_pixels: 1 << 30, ..big() };
    let opts = RenderOptions { scale: 1.0, limits };
    // 1 辺が決まった箱（2²²）を超えるキャンバスは、上限を上げても見積もりで断る（黙って箱の外を落とさない）
    let r = t.estimate(&opts, &Never);
    assert!(matches!(r, Err(Error::Limit(l)) if l.resource == Resource::CanvasSide && l.allowed == 1 << 22), "{r:?}");
}

/// 1 行で幅いっぱいを回る辺でも、セルは塊ごとに払ってから回る（1 回の払いは中止の間隔の内で、払った手間は上限の数と
/// 同じ。単位 2 の 2 回目のレビュー）。
#[test]
fn r4_wide_row_pays_per_chunk() {
    let budget = Budget::new(Stage::Render, u64::MAX, u64::MAX, &Never);
    let mut out = MVec::new(&budget);
    // ほぼ水平の 1 本（1 行の中で 0 → 32768 画素）
    crate::raster::push_segment(Point::new(0.0, 0.1), Point::new(32768.0, 0.9), &mut out).unwrap();
    let e = out.as_slice()[0];
    let mut scan = crate::raster::Scan::new(0, 32768, 1, &budget).unwrap();
    let cells = scan.cells.cells_bound(&e, 0);
    assert!(cells * cost::RASTER_CELL > 2 * crate::budget::CANCEL_INTERVAL);
    let b2 = Budget::new(Stage::Render, u64::MAX, u64::MAX, &Never);
    let mut pay = crate::raster::Payer::new(&b2, cells, cost::RASTER_CELL);
    let visited = scan.cells.add_edge(&e, 0, &mut pay).unwrap();
    assert!(visited <= cells);
    assert!(b2.max_spend() < crate::budget::CANCEL_INTERVAL);
    assert!(b2.work() <= cells * cost::RASTER_CELL);
}

/// 範囲の見積もりと全体の見積もり・計画の外接矩形と節の箱の丸めの差：ユーザー単位の外接矩形 × 倍率と、倍率を掛けた変形
/// で写した点の差で、項目の矩形が節の箱より 1 画素広くなるか。
#[test]
fn r5_item_box_vs_node_box() {
    // (x + e)·s が整数 k 以上なのに x·s + e·s < k となる x を探す
    let s: f64 = 3.0;
    let e: f64 = 0.1;
    let mut found = None;
    let mut x: f64 = 10.0 / 3.0 - e - 1e-9;
    for _ in 0..2_000_000 {
        let user = x + e;
        let dev = s * x + e * s;
        if (user * s).floor() > dev.floor() {
            found = Some(x);
            break;
        }
        x = f64::from_bits(x.to_bits() + 1);
    }
    let Some(x0) = found else {
        println!("r5: 見つからない");
        return;
    };
    let mut b = Builder::new();
    b.add(
        |w| rect(w, x0, x0, 0.5, 0.5, 0.0, 0.0).unwrap(),
        |n| {
            n.flags = HAS_FILL;
            n.fill = [0.0, 0.0, 0.0, 1.0];
            n.transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e, f: e };
        },
    );
    let t = b.tree(20.0, 20.0);
    let opts = RenderOptions { scale: s, ..RenderOptions::DEFAULT };
    let est = t.estimate(&opts, &Never).unwrap();
    let plan = t.plan(&opts, &Never).unwrap();
    let it = plan.items.as_slice()[0];
    let full = plan.estimate_region(PixelRect { x: 0, y: 0, width: plan.width, height: plan.height }, &Never).unwrap();
    let nb = node_box(&t.nodes.as_slice()[0], s, 0.0, PixelBox { x0: 0, y0: 0, x1: 60, y1: 60 });
    println!("r5: x0 {x0} item {:?} node {:?} region {} full {}", it.bbox, nb, full.work, est.render.work);
    assert!(full.work <= est.render.work);
}

/// r2 の対照：pathLength なしで実際の周期を同じ（2）にすると通る（原因は倍の求め方）。
#[test]
fn r2_control_without_path_length() {
    let mut b = Builder::new();
    let d = b.dashes(&[1.0, 1.0]);
    let l = 1.0e5;
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 500.0)).unwrap();
            w.line_to(Point::new(1000.0, 500.0)).unwrap();
            let o = Point::new(-1.0e6, -1.0e6);
            w.move_to(o).unwrap();
            w.cubic_to(Point::new(o.x + l, o.y), Point::new(o.x - l, o.y), o).unwrap();
        },
        |n| {
            n.flags = HAS_STROKE;
            n.stroke = [0.0, 0.0, 0.0, 1.0];
            n.stroke_width = 400.0;
            n.cap = crate::tree::CAP_ROUND;
            n.join = JOIN_ROUND;
            n.dashes = d;
        },
    );
    let t = b.tree(1000.0, 1000.0);
    let (est, r) = plan_result(&t, 1.0);
    match r {
        Ok(p) => println!("r2c: ok edges {} est {:?}", p.edges.len(), est.plan),
        Err(e) => panic!("r2c: 見積もり {:?} の内で計画が作れない：{e:?}", est.plan),
    }
}

/// r1 の対照：同じ形で区間が見る箱を通る（x + y = 20）と通る。
#[test]
fn r1_control_through_view() {
    let mut b = Builder::new();
    let d = b.dashes(&[150.0, 0.5]);
    b.add(
        |w| {
            w.move_to(Point::new(-25.0, 45.0)).unwrap();
            for i in 0..4000 {
                let p = if i % 2 == 0 { Point::new(45.0, -25.0) } else { Point::new(-25.0, 45.0) };
                w.line_to(p).unwrap();
            }
        },
        |n| {
            n.flags = HAS_STROKE;
            n.stroke = [0.0, 0.0, 0.0, 1.0];
            n.stroke_width = 0.2;
            n.dashes = d;
        },
    );
    let t = b.tree(100.0, 100.0);
    let (est, r) = plan_result(&t, 1.0);
    match r {
        Ok(p) => println!("r1c: ok edges {} est {:?}", p.edges.len(), est.plan),
        Err(e) => panic!("r1c: 見積もり {:?} の内で計画が作れない：{e:?}", est.plan),
    }
}

/// 乱れた木を、丸めの出やすい倍率で：範囲の見積もり ≤ 全体の見積もり・項目の矩形 ⊂ 節の箱・描ける・範囲が全体と同じ。
#[test]
fn r6_random_nasty_scales() {
    let mut r = Rng(0x007e_575c_a1e0);
    let mut bad = Vec::new();
    for case in 0..1500 {
        let t = random_tree(&mut r);
        let scale = [0.1, 1.0 / 3.0, 7.3, 0.7, 2.9, 0.3][(case % 6) as usize];
        let opts = RenderOptions { scale, ..RenderOptions::DEFAULT };
        let est = t.estimate(&opts, &Never).unwrap();
        let plan = match t.plan(&opts, &Never) {
            Ok(p) => p,
            Err(e) => {
                bad.push(format!("{case} plan {e:?}"));
                continue;
            }
        };
        let (w, h) = (plan.width, plan.height);
        let full_rect = PixelRect { x: 0, y: 0, width: w, height: h };
        let fe = plan.estimate_region(full_rect, &Never).unwrap();
        if fe.work > est.render.work || fe.peak_bytes > est.render.peak_bytes {
            bad.push(format!("{case} region {} > {}", fe.work, est.render.work));
        }
        let mut full = vec![0u8; (w * h * 4) as usize];
        if let Err(e) = plan.render(&mut PixmapMut::new(&mut full, w, h, (w * 4) as usize).unwrap(), &Never) {
            bad.push(format!("{case} render {e:?}"));
            continue;
        }
        for _ in 0..3 {
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
                    if full[a..a + 4] != part[b..b + 4] {
                        bad.push(format!("{case} mismatch"));
                    }
                }
            }
        }
    }
    assert!(bad.is_empty(), "{} {:?}", bad.len(), &bad[..bad.len().min(5)]);
}

/// キャンバスの外の長い破線：見る箱にかかる部分は少ないのに、計画は道の全体の破線の境を 1 つずつ歩き、見積もりもその分を
/// 数えて断る（ADR-30 (4) の「破線の数は算術で数え」と違う）。
#[test]
fn r7_offcanvas_dashes_refused() {
    for dash in [0.05, 0.01] {
        let mut b = Builder::new();
        let d = b.dashes(&[dash, dash]);
        b.add(
            |w| crate::geom::ellipse(w, 50.0, 1.0e5 + 50.0, 1.0e5, 1.0e5).unwrap(),
            |n| {
                n.flags = HAS_STROKE;
                n.stroke = [0.0, 0.0, 0.0, 1.0];
                n.stroke_width = 1.0;
                n.dashes = d;
            },
        );
        let t = b.tree(100.0, 100.0);
        let est = t.estimate(&RenderOptions::DEFAULT, &Never).unwrap();
        let r = t.plan(&RenderOptions::DEFAULT, &Never);
        println!(
            "r7: dash {dash} est plan work {} render {} → {:?}",
            est.plan.work,
            est.render.work,
            r.as_ref().map(|p| p.edges.len()).map_err(|e| *e)
        );
    }
}

/// 頂点の多い太い round の線：描く手間の見積もり（外形の点ごとに 2r × max(miterlimit, 2) の長さ）と、計画の項目ごとの
/// 上限（範囲の見積もり）の比。
#[test]
fn r8_render_estimate_ratio() {
    let mut r = Rng(0x88);
    for (n, width) in [(20_000u32, 8.0), (20_000, 60.0), (200_000, 60.0)] {
        let mut b = Builder::new();
        b.add(
            |w| {
                w.move_to(Point::new(100.0, 100.0)).unwrap();
                for _ in 0..n {
                    w.line_to(Point::new(r.f(100.0, 1948.0), r.f(100.0, 1948.0))).unwrap();
                }
            },
            |nn| {
                nn.flags = HAS_STROKE;
                nn.stroke = [0.0, 0.0, 0.0, 1.0];
                nn.stroke_width = width;
                nn.join = JOIN_ROUND;
            },
        );
        let t = b.tree(2048.0, 2048.0);
        let opts = RenderOptions { scale: 1.0, limits: big() };
        let est = t.estimate(&opts, &Never).unwrap();
        let plan = t.plan(&opts, &Never).unwrap();
        let fe = plan.estimate_region(PixelRect { x: 0, y: 0, width: 2048, height: 2048 }, &Never).unwrap();
        println!(
            "r8: n {n} w {width}: est.render {} region(full) {} ratio {:.1} (既定の上限 {})",
            est.render.work,
            fe.work,
            est.render.work as f64 / fe.work as f64,
            RenderLimits::DEFAULT.render_work
        );
    }
}

/// 倍率を掛けて点が有限でなくなる節（ユーザー単位では有限）：§6.1 は「描かずに警告」。見積もりと計画はどうするか。
#[test]
fn r9_overflow_after_scale() {
    let mut b = Builder::new();
    b.add(
        |w| rect(w, 10.0, 10.0, 20.0, 20.0, 0.0, 0.0).unwrap(),
        |n| {
            n.flags = HAS_FILL;
            n.fill = [0.0, 0.0, 0.0, 1.0];
        },
    );
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 0.0)).unwrap();
            w.line_to(Point::new(30.0, 0.0)).unwrap();
            w.line_to(Point::new(1.0e308, 30.0)).unwrap();
            w.line_to(Point::new(0.0, 30.0)).unwrap();
            w.close().unwrap();
        },
        |n| {
            n.flags = HAS_FILL | HAS_STROKE;
            n.fill = [255.0, 0.0, 0.0, 1.0];
            n.stroke = [0.0, 0.0, 255.0, 1.0];
        },
    );
    let t = b.tree(40.0, 40.0);
    let opts = RenderOptions { scale: 2.0, ..RenderOptions::DEFAULT };
    let est = t.estimate(&opts, &Never);
    println!("r9: est {est:?}");
    let plan = t.plan(&opts, &Never);
    match plan {
        Ok(p) => println!("r9: plan items {} edges {}", p.items.len(), p.edges.len()),
        Err(e) => println!("r9: plan err {e:?}"),
    }
}

#[test]
fn r9b_overflow_picture() {
    let mut b = Builder::new();
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 0.0)).unwrap();
            w.line_to(Point::new(30.0, 0.0)).unwrap();
            w.line_to(Point::new(1.0e308, 30.0)).unwrap();
            w.line_to(Point::new(0.0, 30.0)).unwrap();
            w.close().unwrap();
        },
        |n| {
            n.flags = HAS_FILL;
            n.fill = [255.0, 0.0, 0.0, 1.0];
        },
    );
    let t = b.tree(40.0, 40.0);
    let (w, _, px) = draw(&t, 2.0);
    let at = |x: u32, y: u32| px[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize].to_vec();
    println!(
        "r9b: (10,10) {:?} (55,50) {:?} (70,50) {:?} (10,70) {:?}",
        at(10, 10),
        at(55, 50),
        at(70, 50),
        at(10, 70)
    );
}

/// 倍率を掛けた変形が有限でない節：見積もりは 0 を足すが、計画は節ごとに OP_STEP を払う。
#[test]
fn r10_nonfinite_scaled_transform_nodes() {
    let mut b = Builder::new();
    for _ in 0..300 {
        b.add(
            |w| rect(w, 0.0, 0.0, 1e-300, 1e-300, 0.0, 0.0).unwrap(),
            |n| {
                n.flags = HAS_FILL;
                n.fill = [0.0, 0.0, 0.0, 1.0];
                n.transform = Transform { a: 1.0e10, b: 0.0, c: 0.0, d: 1.0e10, e: 0.0, f: 0.0 };
            },
        );
    }
    let t = b.tree(1.0e-299, 1.0e-299);
    let opts = RenderOptions { scale: 1.0e300, ..RenderOptions::DEFAULT };
    let est = t.estimate(&opts, &Never).unwrap();
    let r = t.plan(&opts, &Never);
    println!("r10: est {:?} plan {:?}", est.plan, r.as_ref().map(|p| p.items.len()).map_err(|e| *e));
    assert!(r.is_ok());
}

#[test]
fn r2b_butt() {
    let mut b = Builder::new();
    let d = b.dashes(&[1.0, 1.0]);
    let l = 1.0e6;
    b.add(
        |w| {
            w.move_to(Point::new(0.0, 0.0)).unwrap();
            w.line_to(Point::new(1000.0, 0.0)).unwrap();
            let o = Point::new(500.0, 0.0);
            w.move_to(o).unwrap();
            w.cubic_to(Point::new(o.x, o.y + l), Point::new(o.x, o.y - l), o).unwrap();
        },
        |n| {
            n.flags = HAS_STROKE | NON_SCALING;
            n.stroke = [0.0, 0.0, 0.0, 1.0];
            n.stroke_width = 2.0;
            n.dashes = d;
            n.path_length = 1_155_700.0;
            n.transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0e-6, e: 0.0, f: 500.0 };
        },
    );
    let t = b.tree(1000.0, 1000.0);
    let (est, r) = plan_result(&t, 1.0);
    match r {
        Ok(p) => println!("r2b_butt: ok edges {} est {:?}", p.edges.len(), est.plan),
        Err(e) => panic!("r2b_butt: 見積もり {:?} の内で計画が作れない：{e:?}", est.plan),
    }
}
