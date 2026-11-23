//! 段 6：描く（architecture.md §3・§4.5・§8・ADR-13・ADR-38）。範囲と交わる項目だけを選び、行ごとに被覆を求めて塗る。
//! 範囲の画素は、全体を描いて同じ矩形を切り出したものとビット単位で同じ（点と辺は範囲に依らずに計画で決め、セルは辺の
//! 元の両端から閉じた式で求める）。

use crate::budget::{Budget, Estimate};
use crate::cancel::Cancel;
use crate::cost;
use crate::error::{Error, Invalid, InvalidReason, Stage};
use crate::plan::{Plan, RenderOptions};
use crate::raster::{PixelBox, Scan, coverage_alpha, fill_region};
use crate::tree::Tree;

/// 出力の画素（RGBA8 の乗算済み。行のバイト数 `stride` ≥ 幅 × 4）。描く前に全体を透明にする（FR-OUT-04）。
#[derive(Debug)]
pub struct PixmapMut<'a> {
    data: &'a mut [u8],
    width: u32,
    height: u32,
    stride: usize,
}

impl<'a> PixmapMut<'a> {
    /// 画素の並びを包む（大きさと行のバイト数が合わなければ `Invalid`（呼び手の誤り））。
    pub fn new(data: &'a mut [u8], width: u32, height: u32, stride: usize) -> Result<PixmapMut<'a>, Error> {
        let row = (width as usize).checked_mul(4).ok_or_else(argument)?;
        let need = stride.checked_mul(height as usize).ok_or_else(argument)?;
        if width == 0 || height == 0 || stride < row || data.len() < need {
            return Err(argument());
        }
        Ok(PixmapMut { data, width, height, stride })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

/// 画素の範囲（`x`・`y` から幅 `width`・高さ `height`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

fn argument() -> Error {
    Error::Invalid(Invalid { reason: InvalidReason::Argument, position: None })
}

impl Plan {
    /// 範囲を描く見積もり（範囲が全体に含まれるなら `RenderEstimate.render` を超えない。FR-OUT-06）。
    pub fn estimate_region(&self, rect: PixelRect, cancel: &dyn Cancel) -> Result<Estimate, Error> {
        let r = self.region(rect)?;
        let budget = Budget::new(Stage::Estimate, self.limits.estimate_work, self.limits.estimate_bytes, cancel);
        let mut work = 0u64;
        for it in self.items.as_slice() {
            budget.spend(cost::OP_STEP)?;
            let b = it.pixel_box().intersect(&r);
            if b.is_empty() {
                continue;
            }
            // 範囲の行にかかりうる辺だけ（2 分の探索で選ぶ）の、範囲の行の中の行とセルの上限（§6.1）。全体の範囲なら計画が
            // 項目ごとに数えた上限と同じ
            let edges = if b == it.pixel_box() { self.edges_of(it) } else { self.region_edges(it, &b, &budget)? };
            let (cells, rows, started, skipped) = if b == it.pixel_box() {
                (it.cells, it.edge_rows, edges.len() as u64, 0u64)
            } else {
                // 範囲の右の外の辺は足さない（描く段と同じ判定。`raster::skips_right`）
                let right = b.x1.saturating_mul(crate::raster::ONE);
                let (mut cells, mut rows, mut started, mut skipped) = (0u64, 0u64, 0u64, 0u64);
                for e in edges {
                    budget.spend(cost::OP_STEP)?;
                    if crate::raster::skips_right(e, right) {
                        skipped = skipped.saturating_add(1);
                        continue;
                    }
                    let (er, ec) = crate::plan::item_bounds(e, &b);
                    cells = cells.saturating_add(ec);
                    rows = rows.saturating_add(er);
                    started = started.saturating_add(1);
                }
                (cells, rows, started, skipped)
            };
            work = work
                .saturating_add(b.height().saturating_mul(crate::plan::row_work(b.width())))
                .saturating_add(started.saturating_mul(crate::plan::EDGE_START_WORK))
                .saturating_add(skipped.saturating_mul(cost::OP_STEP))
                .saturating_add(rows.saturating_mul(crate::plan::EDGE_ROW_WORK))
                .saturating_add(cells.saturating_mul(cost::RASTER_CELL))
                .saturating_add(crate::plan::region_search(u64::from(it.edges[1])));
        }
        let (frame, peak_bytes) =
            crate::plan::render_frame(r.width(), r.height(), self.max_item_edges, self.items.len() as u64);
        let bg = crate::plan::background_work(self.background, r.width().saturating_mul(r.height()));
        Ok(Estimate { work: work.saturating_add(frame).saturating_add(bg), peak_bytes, result_bytes: 0 })
    }

    /// 全体を描く（出力は計画の大きさ）。
    pub fn render(&self, out: &mut PixmapMut<'_>, cancel: &dyn Cancel) -> Result<(), Error> {
        let rect = PixelRect { x: 0, y: 0, width: self.width, height: self.height };
        self.render_region(rect, out, cancel)
    }

    /// 範囲を描く（出力は範囲の大きさ）。全体の描画の同じ矩形とビット単位で同じ（ADR-13）。
    pub fn render_region(&self, rect: PixelRect, out: &mut PixmapMut<'_>, cancel: &dyn Cancel) -> Result<(), Error> {
        self.render_region_watch(rect, out, cancel, &mut crate::doc::Watch::default())
    }

    /// [`Plan::render_region`] と同じに描き、描く段の勘定の見張りの値を `watch` に書く（試験とファズが実際の山で照らす）。
    pub(crate) fn render_region_watch(
        &self,
        rect: PixelRect,
        out: &mut PixmapMut<'_>,
        cancel: &dyn Cancel,
        watch: &mut crate::doc::Watch,
    ) -> Result<(), Error> {
        let r = self.region(rect)?;
        if out.width != rect.width || out.height != rect.height {
            return Err(argument());
        }
        let est = self.estimate_region(rect, cancel)?;
        if est.work > self.limits.render_work {
            return Err(Error::limit(crate::error::Resource::Work, est.work, self.limits.render_work, Stage::Render));
        }
        if est.peak_bytes > self.limits.temp_bytes {
            return Err(Error::limit(
                crate::error::Resource::Memory,
                est.peak_bytes,
                self.limits.temp_bytes,
                Stage::Render,
            ));
        }
        let budget = Budget::new(Stage::Render, est.work, est.peak_bytes, cancel);
        let res = self.render_in(&budget, r, out);
        *watch = crate::doc::Watch::of(&budget);
        res
    }

    /// 描く段の本体（`budget` は範囲の見積もりを上限にしたもの）。
    fn render_in(&self, budget: &Budget<'_>, r: PixelBox, out: &mut PixmapMut<'_>) -> Result<(), Error> {
        let budget = budget_of(budget);
        // 出力を透明にする（行ごとに、塊に分けて払う）
        let row_len = (out.width as usize).saturating_mul(4);
        let step = crate::budget::chunk_len(1).saturating_mul(cost::FILL_BYTES_PER_UNIT as usize);
        for y in 0..out.height as usize {
            budget.spend(cost::OP_STEP)?;
            let start = y.saturating_mul(out.stride);
            let mut i = 0usize;
            while i < row_len {
                let end = i.saturating_add(step).min(row_len);
                budget.spend(cost::fill((end.saturating_sub(i)) as u64))?;
                if let Some(part) = out.data.get_mut(start.saturating_add(i)..start.saturating_add(end)) {
                    for b in part.iter_mut() {
                        *b = 0;
                    }
                }
                i = end;
            }
        }
        // 背景（根の svg の background-color。透明な出力の上なので置くだけ。塊ごとに払う。ADR-97 (4)）
        if self.background[3] != 0 {
            let px_step = crate::budget::chunk_len(cost::PAINT_PIXEL);
            for y in 0..out.height as usize {
                let start = y.saturating_mul(out.stride);
                let mut x = 0usize;
                while x < out.width as usize {
                    let end = x.saturating_add(px_step).min(out.width as usize);
                    budget.spend((end.saturating_sub(x) as u64).saturating_mul(cost::PAINT_PIXEL))?;
                    let a = start.saturating_add(x.saturating_mul(4));
                    let b = start.saturating_add(end.saturating_mul(4));
                    for (k, v) in out.data.get_mut(a..b).into_iter().flatten().enumerate() {
                        *v = self.background.get(k & 3).copied().unwrap_or(0);
                    }
                    x = end;
                }
            }
        }
        let width = usize::try_from(r.width()).map_err(|_| argument())?;
        let max_edges = usize::try_from(self.max_item_edges).map_err(|_| budget.too_large())?;
        let mut scan = Scan::new(r.x0, width, max_edges, budget)?;
        for it in self.items.as_slice() {
            budget.spend(cost::OP_STEP)?;
            let b = it.pixel_box().intersect(&r);
            if b.is_empty() {
                continue;
            }
            let color = it.color;
            let stride = out.stride;
            let data = &mut *out.data;

            fill_region(
                self.region_edges(it, &b, budget)?,
                it.fill_rule(),
                it.aa != 0,
                b,
                &mut scan,
                budget,
                &mut |x, y, len, cov| {
                    let a = coverage_alpha(cov);
                    if a == 0 {
                        return Ok(());
                    }
                    let row = usize::try_from(y.saturating_sub(r.y0)).unwrap_or(0).saturating_mul(stride);
                    let col = usize::try_from(x.saturating_sub(r.x0)).unwrap_or(0).saturating_mul(4);
                    let mut start = row.saturating_add(col);
                    let mut left = usize::try_from(len).unwrap_or(0);
                    // 塊ごとに払ってから合成する（中止を見ない間を間隔の内に）
                    let step = crate::budget::chunk_len(cost::PAINT_PIXEL);
                    while left > 0 {
                        let k = left.min(step);
                        budget.spend((k as u64).saturating_mul(cost::PAINT_PIXEL))?;
                        let n = k.saturating_mul(4);
                        if let Some(px) = data.get_mut(start..start.saturating_add(n)) {
                            crate::paint::blend_span(px, color, a);
                        }
                        start = start.saturating_add(n);
                        left = left.saturating_sub(k);
                    }
                    Ok(())
                },
            )?;
        }
        Ok(())
    }

    /// 範囲を確かめて画素の矩形にする（空・キャンバスの外は呼び手の誤り）。
    fn region(&self, rect: PixelRect) -> Result<PixelBox, Error> {
        let x1 = u64::from(rect.x).saturating_add(u64::from(rect.width));
        let y1 = u64::from(rect.y).saturating_add(u64::from(rect.height));
        if rect.width == 0 || rect.height == 0 || x1 > u64::from(self.width) || y1 > u64::from(self.height) {
            return Err(argument());
        }
        Ok(PixelBox { x0: i64::from(rect.x), y0: i64::from(rect.y), x1: x1 as i64, y1: y1 as i64 })
    }
}

impl Tree {
    /// 計画して全体を描く（出力は `pixel_size(scale)` の大きさ）。
    pub fn render(&self, opts: &RenderOptions, out: &mut PixmapMut<'_>, cancel: &dyn Cancel) -> Result<(), Error> {
        let plan = self.plan(opts, cancel)?;
        let rect = PixelRect { x: 0, y: 0, width: plan.width, height: plan.height };
        plan.render_region(rect, out, cancel)
    }
}

fn budget_of<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}
