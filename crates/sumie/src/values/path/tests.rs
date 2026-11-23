#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::arithmetic_side_effects
)]

use super::*;
use crate::cancel::Never;
use crate::error::Stage;
use crate::geom::{ArcKind, PathRef, PathSeg, endpoint_to_center};

/// 読んだ区間の並び・結果・置き場に書いたバイト数。
fn read(d: &str) -> (Vec<PathSeg>, Parsed, u64) {
    let budget = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
    let mut verbs = MVec::new(&budget);
    let mut coords = MVec::new(&budget);
    let r = {
        let mut w = PathWriter::new(&mut verbs, &mut coords);
        path_data(d.as_bytes(), &mut w, &budget).unwrap()
    };
    let bytes = verbs.len() as u64 + 8 * coords.len() as u64;
    let segs = PathRef { verbs: verbs.as_slice(), coords: coords.as_slice() }.segments().collect();
    (segs, r, bytes)
}

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

#[test]
fn commands_and_relative_coordinates() {
    let (s, r, _) = read("M10 10 H90 V90 H10 Z");
    assert_eq!(r, Parsed::Complete);
    assert_eq!(
        s,
        [
            PathSeg::Move(p(10.0, 10.0)),
            PathSeg::Line(p(10.0, 10.0), p(90.0, 10.0)),
            PathSeg::Line(p(90.0, 10.0), p(90.0, 90.0)),
            PathSeg::Line(p(90.0, 90.0), p(10.0, 90.0)),
            PathSeg::Close(p(10.0, 90.0), p(10.0, 10.0)),
        ]
    );
    // 相対・暗黙の繰り返し（m のあとは l）
    let (s, _, _) = read("m10 10 20 0 0 20h-20z");
    assert_eq!(s[1], PathSeg::Line(p(10.0, 10.0), p(30.0, 10.0)));
    assert_eq!(s[2], PathSeg::Line(p(30.0, 10.0), p(30.0, 30.0)));
    assert_eq!(s[3], PathSeg::Line(p(30.0, 30.0), p(10.0, 30.0)));
    // 区切りのない数：符号と `.` で切れる
    let (s, _, _) = read("M-1-2L.5.5");
    assert_eq!(s, [PathSeg::Move(p(-1.0, -2.0)), PathSeg::Line(p(-1.0, -2.0), p(0.5, 0.5))]);
    // 指数
    let (s, _, _) = read("M1e1,2E-1");
    assert_eq!(s, [PathSeg::Move(p(10.0, 0.2))]);
    // 3 次・2 次と映し
    let (s, _, _) = read("M0 0 C 0 10 10 10 10 0 S 20 -10 20 0 Q 25 5 30 0 T 40 0");
    assert_eq!(s[1], PathSeg::Cubic(p(0.0, 0.0), p(0.0, 10.0), p(10.0, 10.0), p(10.0, 0.0)));
    assert_eq!(s[2], PathSeg::Cubic(p(10.0, 0.0), p(10.0, -10.0), p(20.0, -10.0), p(20.0, 0.0)));
    assert_eq!(s[3], PathSeg::Quad(p(20.0, 0.0), p(25.0, 5.0), p(30.0, 0.0)));
    assert_eq!(s[4], PathSeg::Quad(p(30.0, 0.0), p(35.0, -5.0), p(40.0, 0.0)));
    // 映す前の命令が違えば今の点
    let (s, _, _) = read("M0 0 L5 5 S 10 10 10 0 T 20 0");
    assert_eq!(s[2], PathSeg::Cubic(p(5.0, 5.0), p(5.0, 5.0), p(10.0, 10.0), p(10.0, 0.0)));
    assert_eq!(s[3], PathSeg::Quad(p(10.0, 0.0), p(10.0, 0.0), p(20.0, 0.0)));
    // 最後の引数のあとの `,` は受ける
    let (s, r, _) = read("M10 10 H90 V90, H10 Z");
    assert_eq!((s.len(), r), (5, Parsed::Complete));
}

#[test]
fn arcs_with_compact_flags() {
    let (s, r, _) = read("M10 50 A40 40 0 1190 50 Z");
    assert_eq!(r, Parsed::Complete);
    let ArcKind::Curve(want) = endpoint_to_center(p(10.0, 50.0), 40.0, 40.0, 0.0, true, true, p(90.0, 50.0)) else {
        panic!()
    };
    assert_eq!(s[1], PathSeg::ArcTo(p(10.0, 50.0), want));
    // 半径 0 は線、始めと終わりが同じ弧は長さ 0 の線（resvg・Chrome と同じ。ADR-92 (2)）
    let (s, _, _) = read("M0 0 A0 5 0 0 1 10 0 A5 5 0 0 1 10 0");
    assert_eq!(
        s,
        [
            PathSeg::Move(p(0.0, 0.0)),
            PathSeg::Line(p(0.0, 0.0), p(10.0, 0.0)),
            PathSeg::Line(p(10.0, 0.0), p(10.0, 0.0))
        ]
    );
    // 旗が 0・1 でなければ誤り（そこまで）
    let (s, r, _) = read("M0 0 L5 0 A5 5 0 2 1 10 0");
    assert_eq!(s.len(), 2);
    assert!(matches!(r, Parsed::Error(_)));
}

#[test]
fn errors_draw_up_to_the_error() {
    // 初めが M でない・続く `,,`・命令のすぐあとの `,`
    for d in ["L10 10 H90", "M10,,10 H90", "M,10 10 H90", "10 10"] {
        let (s, r, _) = read(d);
        assert!(s.is_empty(), "{d}");
        assert!(matches!(r, Parsed::Error(_)), "{d}");
    }
    // 末尾の `.`（SVG 2 の文法は数字が要る。Chrome と同じ。ADR-90）
    let (s, r, _) = read("M10. 10 H90");
    assert!(s.is_empty() && matches!(r, Parsed::Error(3)), "{r:?}");
    let (s, _, _) = read("M10 10 H90 V90 H10. Z");
    assert_eq!(s.len(), 4);
    // 引数の足りない命令は最後の正しい区間まで
    let (s, r, _) = read("M10 10 H90 V90 L10");
    assert_eq!(s.len(), 3);
    assert!(matches!(r, Parsed::Error(_)));
    let (s, _, _) = read("M0 0 L1 1 2 2 3");
    assert_eq!(s.len(), 3);
    // 数字のない指数は、数がその前で終わり、e で誤り
    let (s, r, _) = read("M10 10 H90 V90e H10 Z");
    assert_eq!(s.len(), 3);
    assert!(matches!(r, Parsed::Error(_)));
    // Z のあとの数は誤り
    let (s, r, _) = read("M10 10 H90 V90 Z 10 90 90 90");
    assert_eq!(s.len(), 4);
    assert!(matches!(r, Parsed::Error(_)));
    // 有限でない数
    let (s, r, _) = read("M10 10 L1e400 0");
    assert_eq!(s.len(), 1);
    assert!(matches!(r, Parsed::Error(_)));
    // 空・空白だけ
    assert_eq!(read("").1, Parsed::Complete);
    assert_eq!(read(" \t\n").0.len(), 0);
}

#[test]
fn drawing_after_close_starts_a_new_subpath() {
    let (s, _, _) = read("M10 10 L20 10 Z L30 30");
    assert_eq!(s[3], PathSeg::Move(p(10.0, 10.0)));
    assert_eq!(s[4], PathSeg::Line(p(10.0, 10.0), p(30.0, 30.0)));
    // m のあとは始めの点からの相対
    let (s, _, _) = read("M10 10 L20 10 z m5 5 h1");
    assert_eq!(s[3], PathSeg::Move(p(15.0, 15.0)));
}

#[test]
fn long_numbers_round_like_css() {
    let long = format!("0.{}1", "0".repeat(200));
    let (s, _, _) = read(&format!("M{long} 0.1"));
    assert_eq!(s[0], PathSeg::Move(p(1e-201, 0.1)));
    let (s, _, _) = read(&format!("M1{} 0", "0".repeat(100)));
    assert_eq!(s[0], PathSeg::Move(p(1e100, 0.0)));
}

/// 置き場に書く量は入力の 1 バイトあたり `cost::PATH_GEOMETRY_BYTES` 以下（`estimate_build` の上限）。
#[test]
fn geometry_bytes_stay_within_the_bound() {
    for d in [
        format!("M0 0{}", "zh1".repeat(1000)),
        format!("M0 0h{}", "-1".repeat(1000)),
        format!("M0 0a{}", ".1.1.1 11.1.1".repeat(500)),
        format!("M0 0L{}", ".1.1".repeat(1000)),
        format!("M{}", "1 ".repeat(1000)),
        format!("M0 0{}", "z".repeat(1000)),
    ] {
        let (_, _, bytes) = read(&d);
        assert!(
            bytes <= crate::cost::PATH_GEOMETRY_BYTES * d.len() as u64,
            "{bytes} > 12 × {}: {}",
            d.len(),
            d.chars().take(20).collect::<String>()
        );
    }
}

#[test]
fn reading_pays_per_byte() {
    let budget = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
    let mut verbs = MVec::new(&budget);
    let mut coords = MVec::new(&budget);
    let d = format!("M0 0 L{}", " ".repeat(1_000_000));
    let mut w = PathWriter::new(&mut verbs, &mut coords);
    path_data(d.as_bytes(), &mut w, &budget).unwrap();
    assert!(budget.work() >= 1_000_000);
    assert!(budget.max_spend() < crate::budget::CANCEL_INTERVAL);
}

fn pts(s: &str) -> (Vec<Point>, bool) {
    let budget = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
    let mut v = MVec::new(&budget);
    let ok = points(s.as_bytes(), &mut v, &budget).unwrap();
    (v.as_slice().to_vec(), ok)
}

#[test]
fn points_lists() {
    assert_eq!(pts("10,10 90,10 90,90"), (vec![p(10.0, 10.0), p(90.0, 10.0), p(90.0, 90.0)], true));
    // 数が奇数なら最後を捨てる
    assert_eq!(pts("10,10 90,10 90").0.len(), 2);
    assert!(pts("10,10 90,10 90").1);
    // 区切りのない数
    assert_eq!(pts("-1-2 .5.5").0, [p(-1.0, -2.0), p(0.5, 0.5)]);
    assert_eq!(pts("-1-2.5.5").0, [p(-1.0, -2.5)]);
    // 途中の誤り・続く `,,` は誤り（呼び手は何も描かない）
    assert!(!pts("10,10 90,10 x 10,90").1);
    assert!(!pts("10,,10").1);
    // 終わりの `,` は受ける（resvg・Chrome と同じ）。頭の `,` は誤り
    assert_eq!(pts("10,10 90,10 90,90,"), (vec![p(10.0, 10.0), p(90.0, 10.0), p(90.0, 90.0)], true));
    assert!(pts("10,10 90,10 90,90 ,").1);
    assert!(!pts(",10,10").1);
    assert_eq!(pts(""), (vec![], true));
    assert_eq!(pts("  "), (vec![], true));
}

/// 読む手間は 1 バイトあたり `cost::DECL_BYTE` 以下で、書く命令は 1 バイトあたり 1 つ以下（＋ 1。木を作る段の見積もり）。
#[test]
fn work_and_verbs_per_byte() {
    let mut worst_work = 0.0f64;
    let mut worst_verbs = 0.0f64;
    let unit = [
        "z",
        "zh1",
        "l1 1",
        "l.1.1",
        "h1",
        "M0 0",
        "c1 1 1 1 1 1",
        "a1 1 0001 1",
        "q1-1-1-1",
        "t1 1",
        "s1 1 1 1",
        "L1e5-1e5",
        "h.5",
        "v-.5",
        "Z",
        "a1,1,0,1,1,1,1",
        "m1-1",
        "h1111111111111111111111",
        " ",
        ",",
    ];
    for u in unit {
        let s = format!("M0 0{}", u.repeat(2000));
        let b = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
        let mut verbs = MVec::new(&b);
        let mut coords = MVec::new(&b);
        {
            let mut w = PathWriter::new(&mut verbs, &mut coords);
            path_data(s.as_bytes(), &mut w, &b).unwrap();
        }
        let n = s.len() as f64;
        worst_work = worst_work.max(b.work() as f64 / n);
        worst_verbs = worst_verbs.max(verbs.len() as f64 / (n + 1.0));
        assert!(verbs.len() as u64 <= s.len() as u64 + 1, "{u}");
        assert!(b.work() <= crate::cost::PATH_BYTE * s.len() as u64 + 64, "{u}: {}", b.work() as f64 / n);
        let (sv, sc) = crate::doc::weights::path_scan(s.as_bytes(), &b).unwrap();
        assert!(
            verbs.len() as u64 <= sv && coords.len() as u64 <= sc,
            "{u}: {} {} / {sv} {sc}",
            verbs.len(),
            coords.len()
        );
    }
    eprintln!("work/byte {worst_work:.2} verbs/byte {worst_verbs:.3}");
}

/// 相対の足し算や映しがあふれる点は書かない（その命令の位置で誤り。単位 1 の 2 回目のレビュー）。
#[test]
fn overflowing_points_stop() {
    for d in [
        "m1e308 0 1e308 0",
        "M1e308 0 C 0 0 0 0 -1e308 0 S 1e308 0 1 1",
        "M0 0 h1e308 h1e308",
        "M1e308 1 a1 1 0 0 1 1e308 0",
    ] {
        let b = Budget::new(Stage::Build, u64::MAX, u64::MAX, &Never);
        let mut verbs = MVec::new(&b);
        let mut coords = MVec::new(&b);
        let r = {
            let mut w = PathWriter::new(&mut verbs, &mut coords);
            path_data(d.as_bytes(), &mut w, &b).unwrap()
        };
        assert!(matches!(r, Parsed::Error(_)), "{d}");
        assert!(coords.as_slice().iter().all(|v| v.is_finite()), "{d}");
    }
}
