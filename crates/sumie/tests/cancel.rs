//! 中止の試験（FR-LIM-05）：木を作る段・計画・全体の描画・範囲の描画を、n 回目に読んだときから立つ中止の印で回す。
//! 確かめること：印が立つ前に終われば、中止しないときと同じ結果（同じ画素）。印が立てば `Cancelled` を返し、印が立って
//! からは読み直さない（印は払った手間が `CANCEL_INTERVAL` の倍数を越えるたびに読むので、立ってから返すまでの手間は払いの
//! 塊 1 つの内）。どちらでも確保はすべて返す（ヒープの残りが 0）。読む段の中止は `tests/mutate.rs` が確かめる。単位 4 の
//! レビュー。

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};

use sumie::{Cancel, Error, Never, ParseOptions, PixelRect, PixmapMut, Plan, RenderOptions, Tree};
use sumie_testkit::alloc::measure;

/// `left` 回読まれたあとから立つ中止の印。立ってから読まれた回数も数える。
struct From {
    left: AtomicU32,
    after: AtomicU32,
}

impl From {
    fn new(left: u32) -> From {
        From { left: AtomicU32::new(left), after: AtomicU32::new(0) }
    }

    /// 立たなかった読みの回数（`new(u32::MAX)` で回したとき、印を読んだ回数）。
    fn polls(&self) -> u32 {
        u32::MAX - self.left.load(Ordering::Relaxed)
    }
}

impl Cancel for From {
    fn cancelled(&self) -> bool {
        if self.left.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_ok() {
            false
        } else {
            self.after.fetch_add(1, Ordering::Relaxed);
            true
        }
    }
}

/// 太い破線・round の継ぎ目と端・曲線と弧のパスを並べ、use で写した文書（どの段も印を何度も読む重さ）。
fn heavy_svg() -> String {
    let mut s = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"240\" height=\"180\">\
         <defs><g id=\"cell\" stroke=\"navy\" stroke-width=\"3\" stroke-dasharray=\"8 5\" stroke-linejoin=\"round\" \
         stroke-linecap=\"round\" fill=\"green\" fill-opacity=\"0.5\">",
    );
    for i in 0..60 {
        let (x, y) = ((i % 10) * 4, (i / 10) * 5);
        let _ = write!(s, "<path d=\"M{x} {y} q 3 -4 6 0 t 6 0 a 3 2 30 1 1 -5 4 c 1 2 -3 4 -6 1 z\"/>");
    }
    s.push_str("</g></defs>");
    for i in 0..16 {
        let (x, y) = ((i % 4) * 60, (i / 4) * 45);
        let _ = write!(s, "<use xlink:href=\"#cell\" x=\"{x}\" y=\"{y}\" transform=\"rotate({} 120 90)\"/>", i * 7);
    }
    s.push_str("</svg>");
    s
}

/// 計画を全体（`rect` が None）か範囲で描いた画素。
fn draw(plan: &Plan, rect: Option<PixelRect>, cancel: &dyn Cancel) -> Result<Vec<u8>, Error> {
    let size = plan.pixel_size();
    let r = rect.unwrap_or(PixelRect { x: 0, y: 0, width: size.width, height: size.height });
    let mut px = vec![0u8; r.width as usize * r.height as usize * 4];
    let mut out = PixmapMut::new(&mut px, r.width, r.height, r.width as usize * 4)?;
    match rect {
        None => plan.render(&mut out, cancel)?,
        Some(r) => plan.render_region(r, &mut out, cancel)?,
    }
    Ok(px)
}

/// 画素の要約（FNV-1a。測る間に画素の置き場を返すため）。
fn digest(px: &[u8]) -> u64 {
    px.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// 段 `run` を、印を読む回数のどこで立てても確かめる。`run` は段を回し、続きの段まで描いた画素を返す（段が断れば誤り）。
/// 段を回して、続きの段まで描いた画素を返す口。
type Stage<'a> = &'a dyn Fn(&dyn Cancel) -> Result<Vec<u8>, Error>;

fn check_stage(name: &str, run: Stage<'_>) {
    let base = From::new(u32::MAX);
    let (want, heap) = measure(|| run(&base).map(|px| digest(&px)));
    let want = want.unwrap_or_else(|e| panic!("{name}：中止しないで断った：{e:?}"));
    assert_eq!(heap.current, 0, "{name}：返していない確保");
    let polls = base.polls();
    assert!(polls >= 3, "{name}：印を読んだのが {polls} 回（重さが足りない）");
    // 立てる所：はじめの数回・終わりの数回と、その間を 16 か所ほど
    let step = (polls / 16).max(1);
    let mut at: Vec<u32> = (0..polls).step_by(step as usize).collect();
    at.extend(polls.saturating_sub(3)..=polls + 1);
    at.sort();
    at.dedup();
    for n in at {
        let c = From::new(n);
        let (r, heap) = measure(|| run(&c).map(|px| digest(&px)));
        assert_eq!(heap.current, 0, "{name}：{n} 回目で立てて返していない確保");
        if n < polls {
            assert_eq!(r.err(), Some(Error::Cancelled), "{name}：{n} 回目で立てたのに中止しない");
            assert_eq!(c.after.load(Ordering::Relaxed), 1, "{name}：{n} 回目で立ててから印を読み直した");
        } else {
            assert_eq!(r.as_ref().ok(), Some(&want), "{name}：{n} 回目（読みの回数 {polls} 以上）で違う結果");
            assert_eq!(c.after.load(Ordering::Relaxed), 0);
        }
    }
}

#[test]
fn stages_stop_when_cancelled() {
    let svg = heavy_svg();
    let doc = sumie::parse(svg.as_bytes(), &ParseOptions::DEFAULT, &Never).unwrap();
    let opts = RenderOptions::DEFAULT;
    let tree: Tree = doc.build(&Never).unwrap();
    let plan = tree.plan(&opts, &Never).unwrap();
    let region = PixelRect { x: 50, y: 30, width: 120, height: 90 };
    check_stage("木を作る段", &|c| {
        let t = doc.build(c)?;
        draw(&t.plan(&opts, &Never)?, None, &Never)
    });
    check_stage("見積もりと計画", &|c| draw(&tree.plan(&opts, c)?, None, &Never));
    check_stage("全体の描画", &|c| draw(&plan, None, c));
    check_stage("範囲の描画", &|c| draw(&plan, Some(region), c));
}

/// 中止の印が初めから立っていても、印を読む前に終わる小さな段は結果を返し、読む段は `Cancelled` を返す（どちらでも
/// 確保を返す）。
#[test]
fn raised_flag_on_small_documents() {
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"><rect width=\"4\" height=\"4\" fill=\"red\"/></svg>";
    let flag = std::sync::atomic::AtomicBool::new(true);
    let doc = sumie::parse(svg, &ParseOptions::DEFAULT, &flag).unwrap();
    let tree = doc.build(&flag).unwrap();
    let plan = tree.plan(&RenderOptions::DEFAULT, &flag).unwrap();
    let (px, heap) = measure(|| draw(&plan, None, &flag).map(|px| digest(&px)));
    assert_eq!(heap.current, 0);
    assert_eq!(px.unwrap(), digest(&draw(&plan, None, &Never).unwrap()));
}
