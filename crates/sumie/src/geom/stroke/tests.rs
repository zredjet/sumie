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

/// 輪を集める。
#[derive(Default)]
struct Loops(Vec<Vec<Point>>);

impl LoopSink for Loops {
    fn begin(&mut self) -> Result<(), Error> {
        self.0.push(Vec::new());
        Ok(())
    }

    fn point(&mut self, p: Point) -> Result<(), Error> {
        self.0.last_mut().unwrap().push(p);
        Ok(())
    }
}

impl Loops {
    /// 点の巻き数（輪の和）。
    fn winding(&self, q: Point) -> i32 {
        let mut w = 0;
        for lp in &self.0 {
            let n = lp.len();
            for i in 0..n {
                let (a, b) = (lp[i], lp[(i + 1) % n]);
                if a.y <= q.y {
                    if b.y > q.y && cross(a, b, q) > 0.0 {
                        w += 1;
                    }
                } else if b.y <= q.y && cross(a, b, q) < 0.0 {
                    w -= 1;
                }
            }
        }
        w
    }

    fn inside(&self, x: f64, y: f64) -> bool {
        self.winding(Point::new(x, y)) != 0
    }
}

fn cross(a: Point, b: Point, q: Point) -> f64 {
    (b.x - a.x) * (q.y - a.y) - (q.x - a.x) * (b.y - a.y)
}

fn pen(cap: Cap, join: Join) -> Pen {
    Pen { half: 1.0, cap, join, miter_limit: 4.0, scale: 50.0, tol: 0.1 }
}

fn stroke(pts: &[(f64, f64)], closed: bool, pen: &Pen) -> Loops {
    let budget = Budget::new(Stage::Plan, u64::MAX, u64::MAX, &Never);
    let mut scratch = PolyBuf::new(&budget);
    let mut out = Loops::default();
    let v: Vec<Point> = pts.iter().map(|&(x, y)| Point::new(x, y)).collect();
    stroke_polyline(Poly::corners(&v), closed, pen, &mut scratch, &budget, &mut { u64::MAX }, &mut out).unwrap();
    out
}

#[test]
fn caps() {
    let line = [(0.0, 0.0), (10.0, 0.0)];
    let b = stroke(&line, false, &pen(Cap::Butt, Join::Miter));
    assert!(b.inside(5.0, 0.9) && !b.inside(5.0, 1.1) && !b.inside(-0.1, 0.0) && !b.inside(10.1, 0.0));
    let s = stroke(&line, false, &pen(Cap::Square, Join::Miter));
    assert!(s.inside(10.9, 0.9) && !s.inside(11.1, 0.0) && s.inside(-0.9, -0.9));
    let r = stroke(&line, false, &pen(Cap::Round, Join::Miter));
    assert!(r.inside(10.9, 0.0) && !r.inside(10.8, 0.8) && r.inside(-0.6, 0.6) && !r.inside(-0.8, -0.8));
}

#[test]
fn joins() {
    // 右へ進んでから下へ（画面で右回り）：外の角は右上
    let corner = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
    let m = stroke(&corner, false, &pen(Cap::Butt, Join::Miter));
    assert!(m.inside(10.9, -0.9));
    let b = stroke(&corner, false, &pen(Cap::Butt, Join::Bevel));
    assert!(!b.inside(10.9, -0.9) && b.inside(10.2, -0.2));
    let r = stroke(&corner, false, &pen(Cap::Butt, Join::Round));
    assert!(r.inside(10.6, -0.6) && !r.inside(10.8, -0.8));
    // 内の角は塗る（隙間がない）
    for x in [&m, &b, &r] {
        assert!(x.inside(9.5, 0.5));
    }
    // 鋭い角は miterlimit を超えると bevel（比 1/sin(θ/2)。θ ≈ 11.4° で比 ≈ 10）
    let sharp = [(0.0, 0.0), (10.0, 1.0), (0.0, 2.0)];
    let m4 = stroke(&sharp, false, &pen(Cap::Butt, Join::Miter));
    assert!(!m4.inside(12.0, 1.0));
    let mut p20 = pen(Cap::Butt, Join::Miter);
    p20.miter_limit = 20.0;
    let m20 = stroke(&sharp, false, &p20);
    assert!(m20.inside(12.0, 1.0));
    // miterlimit が 1 未満ならいつも bevel
    let mut p05 = pen(Cap::Butt, Join::Miter);
    p05.miter_limit = 0.5;
    assert!(!stroke(&corner, false, &p05).inside(10.9, -0.9));
}

#[test]
fn reversal_with_round_join() {
    let back = [(0.0, 0.0), (10.0, 0.0), (0.0, 0.0)];
    assert!(stroke(&back, false, &pen(Cap::Butt, Join::Round)).inside(10.9, 0.0));
    assert!(!stroke(&back, false, &pen(Cap::Butt, Join::Miter)).inside(10.5, 0.0));
}

#[test]
fn closed_paths_leave_the_inside() {
    let sq = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let c = stroke(&sq, true, &pen(Cap::Butt, Join::Miter));
    assert!(!c.inside(5.0, 5.0));
    assert!(c.inside(5.0, 0.5) && c.inside(0.5, 5.0) && c.inside(5.0, 9.5));
    // 始めの点も継ぎ目（miter の角）
    assert!(c.inside(-0.9, -0.9));
    // 終わりが始めと同じ点でも同じ
    let mut closed_again = sq.to_vec();
    closed_again.push((0.0, 0.0));
    let c2 = stroke(&closed_again, true, &pen(Cap::Butt, Join::Miter));
    assert!(c2.inside(-0.9, -0.9) && !c2.inside(5.0, 5.0));
}

#[test]
fn zero_length_subpaths() {
    let dot = [(5.0, 5.0), (5.0, 5.0)];
    let r = stroke(&dot, false, &pen(Cap::Round, Join::Miter));
    assert!(r.inside(5.0, 5.9) && !r.inside(5.8, 5.8));
    let s = stroke(&dot, false, &pen(Cap::Square, Join::Miter));
    assert!(s.inside(5.9, 5.9) && !s.inside(6.1, 5.0));
    assert!(stroke(&dot, false, &pen(Cap::Butt, Join::Miter)).0.is_empty());
    // 閉じた 1 点（M z）も長さ 0
    assert!(stroke(&[(5.0, 5.0)], true, &pen(Cap::Round, Join::Miter)).inside(5.0, 5.0));
    // M だけは描かない
    assert!(stroke(&[(5.0, 5.0)], false, &pen(Cap::Round, Join::Miter)).0.is_empty());
}

#[test]
fn round_pieces_follow_the_arc_formula() {
    let p = pen(Cap::Round, Join::Round);
    // r = 1 × 50 = 50：π √(50 / 0.8) = 24.8 → 25
    assert_eq!(p.round_pieces(core::f64::consts::PI), 25);
    let tiny = Pen { scale: 1e-6, ..p };
    assert_eq!(tiny.round_pieces(core::f64::consts::PI), 1);
    let huge = Pen { half: 1e300, scale: 1e300, ..p };
    assert_eq!(huge.round_pieces(1.0), u64::MAX);
}

fn nb() -> Budget<'static> {
    Budget::new(Stage::Plan, u64::MAX, u64::MAX, &Never)
}

fn pieces(pts: &[(f64, f64)], closed: bool, dashes: &[f64], offset: f64) -> Vec<Vec<Point>> {
    pieces_closed(pts, closed, dashes, offset).into_iter().map(|(p, _)| p).collect()
}

/// 破線の部分と、閉じた線として描くか。
fn pieces_closed(pts: &[(f64, f64)], closed: bool, dashes: &[f64], offset: f64) -> Vec<(Vec<Point>, bool)> {
    let budget = Budget::new(Stage::Plan, u64::MAX, u64::MAX, &Never);
    let mut buf = PolyBuf::new(&budget);
    let mut head = PolyBuf::new(&budget);
    let v: Vec<Point> = pts.iter().map(|&(x, y)| Point::new(x, y)).collect();
    let d = Dashes::new(dashes, offset, &budget).unwrap().unwrap();
    let mut out = Vec::new();
    dash_polyline(Poly::corners(&v), closed, &d, &mut buf, &mut head, &budget, None, &mut |s, c| {
        out.push((s.pts.to_vec(), c));
        Ok(())
    })
    .unwrap();
    out
}

#[test]
fn dashes_split_by_length() {
    let line = [(0.0, 0.0), (100.0, 0.0)];
    let p = pieces(&line, false, &[10.0, 5.0], 0.0);
    let spans: Vec<(f64, f64)> = p.iter().map(|s| (s[0].x, s.last().unwrap().x)).collect();
    let want = [(0.0, 10.0), (15.0, 25.0), (30.0, 40.0), (45.0, 55.0), (60.0, 70.0), (75.0, 85.0), (90.0, 100.0)];
    assert_eq!(spans.len(), want.len());
    for (g, w) in spans.iter().zip(want) {
        assert!((g.0 - w.0).abs() < 1e-9 && (g.1 - w.1).abs() < 1e-9, "{spans:?}");
    }
    // 位相 5：初めの線は 5 だけ（15 で割った余り）
    let p = pieces(&line, false, &[10.0, 5.0], 5.0);
    assert_eq!((p[0][0].x, p[0].last().unwrap().x), (0.0, 5.0));
    // 負の位相は合計で割った余り（−5 は 10）
    let p = pieces(&line, false, &[10.0, 5.0], -5.0);
    assert_eq!((p[0][0].x, p[0].last().unwrap().x), (5.0, 15.0));
    // 曲がる折れ線では頂点も入る
    let bent = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
    let p = pieces(&bent, false, &[15.0, 100.0], 0.0);
    assert_eq!(p[0], [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 5.0)]);
    // 長さ 0 の線は同じ 2 点
    let p = pieces(&line, false, &[0.0, 50.0], 0.0);
    assert_eq!(p.len(), 2);
    assert_eq!(p[1], [Point::new(50.0, 0.0), Point::new(50.0, 0.0)]);
    // 閉じた折れ線は閉じる区間も分け、頭が線の中なら最初の部分を最後の部分につなぐ（resvg・Chrome と同じ。ADR-92 (1)）
    let sq = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let p = pieces(&sq, true, &[35.0, 1.0], 0.0);
    // 線 0〜35・隙間 35〜36・線 36〜40（始めの点で終わり、最初の部分 0〜35 につながる）
    assert_eq!(p.len(), 1);
    assert_eq!(p[0][0], Point::new(0.0, 4.0));
    assert_eq!(p[0][1], Point::new(0.0, 0.0));
    assert_eq!(*p[0].last().unwrap(), Point::new(0.0, 5.0));
    // 最後の部分がちょうど始めの点で終われば（隙間 0 の破線）つながない：始めの角もほかの角と同じく継ぎ目を作らない（Chrome
    // と同じ。resvg はつなぐ。tests/painting/stroke-dasharray/n-0）
    // （境が角ごとに 2 つずつ進まずに続いても、残りを実線にしない）
    let p = pieces(&sq, true, &[10.0, 0.0], 0.0);
    assert_eq!(p.len(), 4, "{p:?}");
    assert!(p.iter().all(|s| s.windows(2).all(|w| w[0].x == w[1].x || w[0].y == w[1].y)), "{p:?}");
    assert_eq!(p.iter().filter(|s| s.len() > 1 && s[0] == Point::new(0.0, 0.0)).count(), 1);
    // 最後の部分が始めの点でちょうど終わっても、続く隙間が正なら最初の部分につなぐ（Chrome と同じ。単位 1 の 8 回目の
    // レビュー）：周 40 に [4, 2] は 36〜40 の部分が 0〜4 の部分につながる
    let p = pieces(&sq, true, &[4.0, 2.0], 0.0);
    assert_eq!(p.len(), 6, "{p:?}");
    assert!(p.iter().any(|s| s[0] == Point::new(0.0, 4.0) && *s.last().unwrap() == Point::new(4.0, 0.0)), "{p:?}");
    // 全体が 1 本の部分で始めの点でちょうど終わるとき、続く隙間が 0 なら開いた線、正なら閉じた線
    let p = pieces_closed(&sq, true, &[40.0, 0.0], 0.0);
    assert_eq!(p.len(), 1);
    assert!(!p[0].1, "{p:?}");
    let p = pieces_closed(&sq, true, &[40.0, 5.0], 0.0);
    assert!(p[0].1, "{p:?}");
    // 頭が隙間ならつながない
    let p = pieces(&sq, true, &[5.0, 5.0], 5.0);
    assert!(p.iter().all(|s| s[0] != Point::new(0.0, 0.0) || s.len() > 1));
    // 全体が線（最初の部分が終わらない）は 1 つの部分
    let p = pieces(&sq, true, &[100.0, 1.0], 0.0);
    assert_eq!(p.len(), 1);
    // 閉じた長さ 0 の部分パスは描かない（Chrome と同じ）
    assert!(pieces(&[(5.0, 5.0)], true, &[5.0, 5.0], 0.0).is_empty());
    // 開いた長さ 0 は端を描く（同じ 2 点）
    assert_eq!(pieces(&[(5.0, 5.0), (5.0, 5.0)], false, &[5.0, 5.0], 0.0).len(), 1);
}

#[test]
fn bad_dash_lists_are_solid() {
    assert!(Dashes::new(&[], 0.0, &nb()).unwrap().is_none());
    assert!(Dashes::new(&[1.0, 2.0, 3.0], 0.0, &nb()).unwrap().is_none());
    assert!(Dashes::new(&[1.0, -2.0], 0.0, &nb()).unwrap().is_none());
    assert!(Dashes::new(&[0.0, 0.0], 0.0, &nb()).unwrap().is_none());
    assert!(Dashes::new(&[f64::NAN, 1.0], 0.0, &nb()).unwrap().is_none());
    let d = Dashes::new(&[2.0, 3.0], 12.0, &nb()).unwrap().unwrap();
    assert_eq!(d.phase, 2.0);
}

/// 長い隙間の下の折れ線・同じ点の続き・有限でない向きの頂点も、点ごとに払う（単位の 1 回目のレビュー）。
#[test]
fn every_point_is_paid() {
    let n = 50_000usize;
    let v: Vec<Point> = (0..n).map(|i| Point::new(i as f64, (i % 2) as f64)).collect();
    let budget = Budget::new(Stage::Plan, u64::MAX, u64::MAX, &Never);
    let mut buf = PolyBuf::new(&budget);
    let mut head = PolyBuf::new(&budget);
    let d = Dashes::new(&[1.0, 1e30], 0.0, &budget).unwrap().unwrap();
    dash_polyline(Poly::corners(&v), false, &d, &mut buf, &mut head, &budget, None, &mut |_, _| Ok(())).unwrap();
    assert!(budget.work() >= n as u64 * crate::cost::OP_STEP, "{}", budget.work());
    assert!(budget.max_spend() < crate::budget::CANCEL_INTERVAL);
    let same: Vec<Point> = vec![Point::new(1.0, 1.0); n];
    let budget = Budget::new(Stage::Plan, u64::MAX, u64::MAX, &Never);
    let mut scratch = PolyBuf::new(&budget);
    let mut out = Loops::default();
    stroke_polyline(
        Poly::corners(&same),
        false,
        &pen(Cap::Round, Join::Round),
        &mut scratch,
        &budget,
        &mut { u64::MAX },
        &mut out,
    )
    .unwrap();
    assert!(budget.work() >= n as u64 * crate::cost::OP_STEP, "{}", budget.work());
}

/// 差があふれる 2 点でも向きが出る。破線の区間の長さが有限でなければ、その先を分けない（終わる）。
#[test]
fn huge_differences() {
    let far = [(-1.5e308, 0.0), (1.5e308, 1.5e308)];
    let s = stroke(&far, false, &pen(Cap::Butt, Join::Miter));
    assert!(!s.0.is_empty());
    let budget = Budget::new(Stage::Plan, 1 << 24, u64::MAX, &Never);
    let mut buf = PolyBuf::new(&budget);
    let mut head = PolyBuf::new(&budget);
    let v: Vec<Point> = far.iter().map(|&(x, y)| Point::new(x, y)).collect();
    let d = Dashes::new(&[1.0, 1.0], 0.0, &budget).unwrap().unwrap();
    dash_polyline(Poly::corners(&v), false, &d, &mut buf, &mut head, &budget, None, &mut |_, _| Ok(())).unwrap();
    // 区間に比べて細かすぎる破線は境が多すぎるので、手間の上限で止まる（見積もりが先に断る）
    let v = [Point::new(1e17, 0.0), Point::new(2e17, 0.0)];
    let d = Dashes::new(&[1.0, 1.0], 0.0, &budget).unwrap().unwrap();
    let budget = Budget::new(Stage::Plan, 1 << 24, u64::MAX, &Never);
    let r = dash_polyline(Poly::corners(&v), false, &d, &mut buf, &mut head, &budget, None, &mut |_, _| Ok(()));
    assert!(matches!(r, Err(Error::Limit(_))));
}

/// 閉じた部分パスの全体が最初の破線に入るなら、閉じた線として描く（始めの角も継ぎ目。単位 1 の 2 回目のレビュー）。
#[test]
fn whole_closed_dash_is_closed() {
    let sq = [(25.0, 25.0), (75.0, 25.0), (75.0, 75.0), (25.0, 75.0)];
    let p = pieces_closed(&sq, true, &[1000.0, 10.0], 0.0);
    assert_eq!(p.len(), 1);
    assert!(p[0].1);
    // 角の手前に隙間があれば開いた部分（Chrome と同じ）
    let p = pieces_closed(&sq, true, &[190.0, 10.0], 0.0);
    assert!(p.iter().all(|(_, c)| !c));
}

/// ちょうどの折り返し（外積 0）の round の継ぎ目は、半円を 1 度だけ出す（単位 1 の 2 回目のレビュー）。
#[test]
fn exact_reversal_makes_one_round_join() {
    let pen = Pen { half: 20.0, cap: Cap::Butt, join: Join::Round, miter_limit: 4.0, scale: 1.0, tol: 0.25 };
    let count = |pts: &[(f64, f64)]| {
        let budget = nb();
        let mut scratch = PolyBuf::new(&budget);
        let v: Vec<Point> = pts.iter().map(|&(x, y)| Point::new(x, y)).collect();
        let mut sink = Loops::default();
        stroke_polyline(Poly::corners(&v), false, &pen, &mut scratch, &budget, &mut { u64::MAX }, &mut sink).unwrap();
        sink.0.iter().map(Vec::len).sum::<usize>()
    };
    let half_circle = pen.round_pieces(core::f64::consts::PI) as usize;
    let straight = count(&[(20.0, 100.0), (180.0, 100.0)]);
    let folded = count(&[(20.0, 100.0), (180.0, 100.0), (20.0, 100.0)]);
    // 折り返しても塗る所は線分と同じ（先の半円を含む）
    let budget = nb();
    let mut scratch = PolyBuf::new(&budget);
    let v = [Point::new(20.0, 100.0), Point::new(180.0, 100.0), Point::new(20.0, 100.0)];
    let mut l = Loops::default();
    stroke_polyline(Poly::corners(&v), false, &pen, &mut scratch, &budget, &mut { u64::MAX }, &mut l).unwrap();
    assert!(l.inside(195.0, 100.0) && l.inside(100.0, 115.0) && l.inside(100.0, 85.0) && !l.inside(10.0, 100.0));
    // 折り返しの頂点：前へ回る縁は円弧、後ろへ回る縁は頂点を通す 3 点
    assert!(folded <= straight + half_circle + 8, "{folded} {straight} {half_circle}");
}

/// 巨大な座標の向きもあふれずに求める（単位 1 の 2 回目のレビュー）。
#[test]
fn direction_of_huge_coordinates() {
    let d = direction(Point::new(-1.5e308, -1.5e308), Point::new(1.5e308, 1.5e308)).unwrap();
    assert!((d.x - core::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12 && (d.y - d.x).abs() < 1e-12);
    assert!(seg_len(Point::new(-1.5e308, 0.0), Point::new(1.5e308, 0.0)).is_infinite());
    assert_eq!(seg_len(Point::new(1.0, 1.0), Point::new(4.0, 5.0)), 5.0);
}

/// 境の足し算が進まない小さな隙間は長さ 0 の部分と同じに扱い、残りを捨てない（単位 1 の 3 回目のレビュー）。
#[test]
fn tiny_gaps_do_not_drop_the_rest() {
    let p = pieces(&[(0.0, 50.0), (200.0, 50.0)], false, &[30.0, 1e-15, 30.0, 1e-15], 0.0);
    let last = p.last().unwrap();
    assert_eq!(last.last().unwrap().x, 200.0);
    // 遠い所で隙間が小さすぎて進まない：隙間は長さ 0 と同じで、残りを描く
    let p = pieces(&[(-99950.0, 50.0), (100050.0, 50.0)], false, &[99999.0, 1e-12], 0.0);
    assert_eq!(p.last().unwrap().last().unwrap().x, 100050.0);
    // 線の部分も隙間も小さすぎて 1 周しても進まない：残りは線（区間の中の位置が大きくなってから）
    let p = pieces(&[(0.0, 0.0), (1e6, 0.0)], false, &[999_999.0, 1e-12, 1e-12, 1e-12], 0.0);
    assert_eq!(p.last().unwrap().last().unwrap().x, 1e6);
}

/// 破線の境が f32 で区間の端の点と同じになるときは、角の境にする（ごく短い部分や、隣の区間の向きの端を作らない。Chrome は
/// 座標を f32 で持つ。ユーザーの判断。単位 1 の 8 回目・単位 2 の 6 回目のレビュー。ADR-104）。
#[test]
fn dash_boundaries_snap_to_corners_in_f32() {
    // 縦の辺の長さは 30.000000000000004 なので、30 の境は角の 4e-15 手前に来る
    let bent = [(50.2, 10.3), (50.2, 40.3), (10.2, 40.3)];
    let p = pieces(&bent, false, &[30.0, 10.0], 0.0);
    // 角ちょうどの境と同じ形（角の点が 2 度入り、線を作るときにまとめる）
    let mut first = p[0].clone();
    first.dedup();
    assert_eq!(first, [Point::new(50.2, 10.3), Point::new(50.2, 40.3)], "{p:?}");
    // 角の 1e-4 手前の境は寄せない（f32 で別の点）
    let p = pieces(&bent, false, &[29.9999, 10.0], 0.0);
    assert_ne!(*p[0].last().unwrap(), Point::new(50.2, 40.3));
    // 閉じた形で、境が角のわずかに先に来るときも角から始まる
    let rect = [(10.2, 10.7), (50.2, 10.7), (50.2, 40.7), (10.2, 40.7)];
    let p = pieces(&rect, true, &[7.0, 3.0], 0.0);
    assert!(p.iter().all(|s| s.windows(2).all(|w| w[0] == w[1] || w[0].x == w[1].x || w[0].y == w[1].y)), "{p:?}");
    assert!(p.iter().any(|s| s[0] == Point::new(50.2, 40.7) || *s.last().unwrap() == Point::new(50.2, 40.7)), "{p:?}");
}
