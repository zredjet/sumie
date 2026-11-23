//! 段 4 の口：木を作る（architecture.md §4.4・`impl Document` の `estimate_build`・`build`）。
//!
//! 文書を木を作るたどり方（`doc::walk` の `build`：use を use の文脈で展開し、描く木に入らない部分木に降りない）で行きがけの
//! 順にたどり、描く図形ごとにパスの節を 1 つ作る。
//!
//! - パスのデータ（`d` の属性と CSS の `path()`）・`points`・`stroke-dasharray` は、宣言ごとに 1 度だけ読んで置き場に書き、
//!   use で展開した節も同じ範囲を指す（写さない）。宣言の鍵は先に数えて並べておく（値の置き場と範囲。同じ規則の宣言を
//!   当てた要素は同じ鍵）
//! - 図形（rect・circle・ellipse・line）は、節ごとに長さを解いて書く（% や em は要素の文脈に依る）
//! - M2 でまだ描かない機能は `Unsupported`（`Feature::NotYet`。ADR-89 (1)）：グラデーション・pattern の塗り・`opacity` < 1・
//!   clip・mask・filter・blend・isolation・marker・切り取りの要る入れ子の svg と symbol
//! - 文書の大きさが中身で決まるときは、描く図形の幾何の外接矩形（線の太さを含まない。`visibility: hidden` は含める）の
//!   右下（FR-OUT-01）

use crate::budget::{Budget, Estimate};
use crate::cancel::Cancel;
use crate::cost;
use crate::css::lex::{Kind as Tk, decode_string, ident_is};
use crate::doc::refs::{EDGE_CLIP, EDGE_FILTER, EDGE_MARKER_END, EDGE_MARKER_MID, EDGE_MARKER_START, EDGE_MASK};
use crate::doc::walk::{Used, WalkItem, WalkOptions, used_paint, walk_with};
use crate::doc::{Document, Size};
use crate::error::{Error, Feature, InvalidReason, Later, Stage, Unsupported};
use crate::geom::{self, PathRef, PathWriter, Point, Rect, Transform};
use crate::mem::{MVec, Plain, lower_bound_try, stable_sort};
use crate::style::cascade::{ORIGIN_ATTR, SRC_INPUT, SRC_STORE, WIDE_NONE};
use crate::style::computed::{
    Axis, Base, Bufs, CPaint, Computed, DASH_NONE, FX_BLEND, FX_CLIP, FX_FILTER, FX_ISOLATE, FX_MASK, G_CX, G_CY,
    G_HEIGHT, G_R, G_RX, G_RY, G_WIDTH, G_X, G_Y, PAINT_CONTEXT_FILL, PAINT_CONTEXT_STROKE, VISIBLE, resolve,
};
use crate::style::props::lookup;
use crate::tree::{
    CAP_BUTT, CAP_ROUND, CAP_SQUARE, CRISP, EVEN_ODD, HAS_FILL, HAS_STROKE, HAS_VP, JOIN_BEVEL, JOIN_MITER, JOIN_ROUND,
    NON_SCALING, PATH_LENGTH_ZERO, PathNode, STROKE_FIRST, Tree,
};
use crate::values::{Color, Length, Values, path};
use crate::xml::lex::Span;
use crate::xml::{FLAG_IN_STORE, KIND_ELEMENT, NS_NONE, NS_SVG};

/// 宣言ごとに 1 度だけ読むものの種類。
const KEY_D_ATTR: u8 = 0;
const KEY_D_CSS: u8 = 1;
const KEY_POLYLINE: u8 = 2;
const KEY_POLYGON: u8 = 3;
const KEY_DASH: u8 = 4;

/// 宣言の鍵（値の置き場・範囲・種類）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DeclKey {
    start: u32,
    end: u32,
    src: u8,
    kind: u8,
    pad: [u8; 2],
}

impl Plain for DeclKey {}

impl DeclKey {
    fn new(src: u8, span: Span, kind: u8) -> DeclKey {
        DeclKey { start: span.start, end: span.end, src, kind, pad: [0; 2] }
    }

    fn less(&self, o: &DeclKey) -> bool {
        (self.src, self.start, self.end, self.kind) < (o.src, o.start, o.end, o.kind)
    }
}

/// 鍵ごとの読んだ結果（置き場の範囲：幾何は命令と座標、破線は破線の置き場。`done` が 0 ならまだ読んでいない）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DeclSlot {
    a: [u32; 2],
    b: [u32; 2],
    done: u8,
    pad: [u8; 3],
}

impl Plain for DeclSlot {}

const SLOT_EMPTY: DeclSlot = DeclSlot { a: [0, 0], b: [0, 0], done: 0, pad: [0; 3] };

impl<'s> Document<'s> {
    /// 木を作る段の見積もり（architecture.md §3.1）。描く木に入りうる画像と文字の要るものを確かめ（今は画像のデコーダと
    /// フォントの集まりを受けないので、ラスターの画像は `Unsupported`（`Image`）、SVG の画像は `Unsupported`（`NestedSvg`）、
    /// 文字は `Unsupported`（`TextWithoutFonts`））、展開の数を `Limits::instances` と比べる（超えれば `Limit`（`Instances`））。
    /// 手間と確保は、展開した要素ごとの重みの和（`doc::refs` が展開の数と同じ数え方で足したもの）と、宣言ごとに 1 度だけ読む
    /// 分の和。
    pub fn estimate_build(&self, cancel: &dyn Cancel) -> Result<Estimate, Error> {
        let budget = Budget::new(Stage::EstimateBuild, self.limits.estimate_work, self.limits.estimate_bytes, cancel);
        budget.spend(cost::OP_STEP)?;
        self.check_build()?;
        let w = &self.refs.weights;
        let r = self.report.bytes();
        let work = w.work.saturating_add(self.report.copy_work()).saturating_add(BUILD_FIXED);
        Ok(Estimate {
            work,
            peak_bytes: w.peak_bytes.saturating_add(r).saturating_add(BUILD_FIXED_BYTES),
            result_bytes: w.result_bytes.saturating_add(r),
        })
    }

    /// 木を作る（architecture.md §3）。見積もりを上限と比べ（超えれば `Limit`）、見積もりの値を上限にして作る。
    pub fn build(&self, cancel: &dyn Cancel) -> Result<Tree, Error> {
        let est = self.estimate_build(cancel)?;
        let limits = &self.limits;
        if est.work > limits.build_work {
            return Err(Error::limit(crate::error::Resource::Work, est.work, limits.build_work, Stage::Build));
        }
        if est.peak_bytes > limits.build_bytes {
            return Err(Error::limit(crate::error::Resource::Memory, est.peak_bytes, limits.build_bytes, Stage::Build));
        }
        let budget = Budget::new(Stage::Build, est.work, est.peak_bytes, cancel);
        build_tree(self, &budget)
    }

    /// [`Document::build`] と同じに、見積もりの上限を外して作り、勘定の見張りの値を `watch` に書く（試験とファズが
    /// 見積もり ≥ 実際を確かめる）。
    pub(crate) fn build_watch(&self, cancel: &dyn Cancel, watch: &mut crate::doc::Watch) -> Result<Tree, Error> {
        self.check_build()?;
        let budget = Budget::new(Stage::Build, u64::MAX, u64::MAX, cancel);
        let r = build_tree(self, &budget);
        watch.work = budget.work();
        watch.peak = budget.peak();
        watch.max_spend = budget.max_spend();
        watch.over_released = budget.over_released();
        r
    }

    /// 木を作る前に決まる断り（展開の数・画像・文字）。
    fn check_build(&self) -> Result<(), Error> {
        let n = self.refs.instances;
        if n > self.limits.instances {
            return Err(Error::limit(
                crate::error::Resource::Instances,
                n,
                self.limits.instances,
                Stage::EstimateBuild,
            ));
        }
        let unsupported = |feature| Err(Error::Unsupported(Unsupported { feature, position: None }));
        if self.needs.svg_images {
            return unsupported(Feature::NestedSvg);
        }
        if self.needs.raster_images {
            return unsupported(Feature::Image);
        }
        if self.needs.text {
            return unsupported(Feature::TextWithoutFonts);
        }
        Ok(())
    }
}

/// 木を作る段の決まった手間と確保（根の計算・入れ物の確保の始まりなど）。
const BUILD_FIXED: u64 = 64 * cost::ALLOC_STEP;
const BUILD_FIXED_BYTES: u64 = 4096;

/// 木を作る（`budget` は見積もりの値を上限にしたもの）。
pub(crate) fn build_tree(d: &Document<'_>, budget: &Budget<'_>) -> Result<Tree, Error> {
    let budget = budget_ref(budget);
    let keys = collect_keys(d, budget)?;
    let mut slots: MVec<'_, DeclSlot> = MVec::with_capacity(budget, keys.len())?;
    slots.resize_to(keys.len(), SLOT_EMPTY)?;
    let w = &d.refs.weights;
    let mut out = Out {
        nodes: MVec::with_capacity(budget, cap(w.shapes)?)?,
        verbs: MVec::with_capacity(budget, cap(w.verbs)?)?,
        coords: MVec::with_capacity(budget, cap(w.coords)?)?,
        dashes: MVec::with_capacity(budget, cap(w.dashes)?)?,
        scratch: MVec::new(budget),
        points: MVec::new(budget),
        bbox: Rect::EMPTY,
        root: None,
    };
    let mut root_drawn = false;
    walk_with(d, WalkOptions { build: true }, budget, &mut |item| {
        if item.node == 0 {
            out.root = Some(item.computed);
            root_drawn = item.rendered;
        }
        if !item.rendered {
            return Ok(());
        }
        visit(d, item, keys.as_slice(), &mut slots, &mut out, budget)
    })?;
    let Out { nodes, verbs, coords, dashes, scratch, points, bbox, root } = out;
    drop((scratch, points, slots, keys));
    let size = match d.size.known() {
        Some(s) => s,
        None => content_size(&bbox)?,
    };
    let root = root.unwrap_or(Computed::initial(d.font_size));
    let background = if root_drawn { background_of(d, &root, budget)? } else { [0.0; 4] };
    let report = d.report.copy_in(budget)?;
    Ok(Tree {
        size,
        nodes: exact(nodes)?,
        verbs: exact(verbs)?,
        coords: exact(coords)?,
        dashes: exact(dashes)?,
        root_font: root.font_size,
        root_line: root.line_height,
        doc_size: d.media_size,
        background,
        report,
    })
}

/// 根の要素の `background-color`（`currentColor` は根の `color`。書いていない・読めなければ透明。resvg・Chrome と同じく描く。
/// ADR-97 (4)）。
fn background_of(d: &Document<'_>, root: &Computed, budget: &Budget<'_>) -> Result<[f64; 4], Error> {
    let budget = budget_ref(budget);
    let Some(id) = lookup(b"background-color", false) else { return Ok([0.0; 4]) };
    let bufs = Bufs { input: d.input, store: d.xml.store.as_slice(), sheet: d.sheet.text.as_slice() };
    for s in d.styles.of(0) {
        budget.spend(cost::OP_STEP)?;
        if s.prop != id || s.wide != WIDE_NONE {
            continue;
        }
        let c = match Values::new(bufs.of(s.src), s.value, budget).color()? {
            Some(crate::values::ColorValue::Color(c)) => c,
            Some(crate::values::ColorValue::CurrentColor) => root.color,
            None => return Ok([0.0; 4]),
        };
        return Ok(with_opacity(c, 1.0));
    }
    Ok([0.0; 4])
}

/// 中身で決まる文書の大きさ（外接矩形の右下。中身がなければ 100 × 100。幅か高さが 0 以下なら `Invalid`。FR-OUT-01）。
fn content_size(bbox: &Rect) -> Result<Size, Error> {
    if bbox.is_empty() {
        return Ok(Size { width: 100.0, height: 100.0 });
    }
    let (w, h) = (bbox.x1, bbox.y1);
    if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
        return Err(Error::Invalid(crate::error::Invalid { reason: InvalidReason::Size, position: None }));
    }
    Ok(Size { width: w, height: h })
}

/// 上限の容量で取った置き場を、使った長さに詰めて写す（木が持ち続ける量を実際の量にする）。
fn exact<T: Plain>(v: MVec<'_, T>) -> Result<crate::mem::Frozen<T>, Error> {
    let mut e: MVec<'_, T> = MVec::with_capacity(v.budget(), v.len())?;
    e.append_slice(v.as_slice())?;
    drop(v);
    Ok(e.freeze())
}

fn cap(n: u64) -> Result<usize, Error> {
    usize::try_from(n).map_err(|_| Error::limit(crate::error::Resource::Memory, n, u64::MAX, Stage::Build))
}

/// 作る途中の置き場。
struct Out<'b> {
    nodes: MVec<'b, PathNode>,
    verbs: MVec<'b, u8>,
    coords: MVec<'b, f64>,
    dashes: MVec<'b, Length>,
    /// CSS の `path("…")` の escape を解いた中身
    scratch: MVec<'b, u8>,
    /// `points` の点
    points: MVec<'b, Point>,
    /// 描く図形の外接矩形（根のユーザー単位）
    bbox: Rect,
    /// 根の要素の計算した値（破線の rem・rlh）
    root: Option<Computed>,
}

/// 宣言ごとに 1 度だけ読むものの鍵を数えて並べる（重なりを除く）。
fn collect_keys<'b>(d: &Document<'_>, budget: &'b Budget<'b>) -> Result<MVec<'b, DeclKey>, Error> {
    let doc = &d.xml;
    let input = d.input;
    let d_id = lookup(b"d", false);
    let dash_id = lookup(b"stroke-dasharray", false);
    let mut keys: MVec<'_, DeclKey> = MVec::with_capacity(budget, cap(d.refs.weights.keys)?)?;
    for (i, n) in doc.nodes.as_slice().iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        if n.kind != KIND_ELEMENT {
            continue;
        }
        for s in d.styles.of(i as u32) {
            budget.spend(cost::OP_STEP)?;
            if s.wide != WIDE_NONE {
                continue;
            }
            let p = Some(s.prop);
            if p == d_id {
                let kind = if s.origin == ORIGIN_ATTR { KEY_D_ATTR } else { KEY_D_CSS };
                keys.push(DeclKey::new(s.src, s.value, kind))?;
            } else if p == dash_id {
                keys.push(DeclKey::new(s.src, s.value, KEY_DASH))?;
            }
        }
        if n.ns == NS_SVG {
            let local = n.local.of(input);
            let kind = match local {
                b"polyline" => KEY_POLYLINE,
                b"polygon" => KEY_POLYGON,
                _ => continue,
            };
            if let Some(a) = doc.find_attr(input, n, NS_NONE, b"points", budget)? {
                let src = if a.flags & FLAG_IN_STORE != 0 { SRC_STORE } else { SRC_INPUT };
                keys.push(DeclKey::new(src, a.value, kind))?;
            }
        }
    }
    stable_sort(budget, keys.as_mut_slice(), |a, b| a.less(b))?;
    let mut keep = 0usize;
    for k in 0..keys.len() {
        budget.spend(cost::OP_STEP)?;
        let Some(x) = keys.get(k).copied() else { break };
        if keep > 0 && keys.get(keep.saturating_sub(1)).copied() == Some(x) {
            continue;
        }
        if let Some(y) = keys.get_mut(keep) {
            *y = x;
        }
        keep = keep.saturating_add(1);
    }
    keys.truncate(keep);
    Ok(keys)
}

/// 鍵の番号（ない鍵は None：数えた鍵の外の宣言。起こらない）。
fn find_key(keys: &[DeclKey], k: &DeclKey, budget: &Budget<'_>) -> Result<Option<usize>, Error> {
    let i = lower_bound_try(budget, keys, |x| Ok(x.less(k)))?;
    Ok(keys.get(i).filter(|x| *x == k).map(|_| i))
}

fn range_of(start: usize, end: usize) -> Result<[u32; 2], Error> {
    let s = u32::try_from(start).map_err(|_| too_big())?;
    let n = u32::try_from(end.saturating_sub(start)).map_err(|_| too_big())?;
    Ok([s, n])
}

fn too_big() -> Error {
    Error::limit(crate::error::Resource::Memory, u64::MAX, u64::from(u32::MAX), Stage::Build)
}

/// 描く木に入る要素を 1 つ見る（図形ならパスの節を作る）。
fn visit(
    d: &Document<'_>,
    item: &WalkItem,
    keys: &[DeclKey],
    slots: &mut MVec<'_, DeclSlot>,
    out: &mut Out<'_>,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    let doc = &d.xml;
    let input = d.input;
    let Some(n) = doc.node(item.node) else { return Ok(()) };
    let c = &item.computed;
    let not_yet = |l: Later| {
        Err(Error::Unsupported(Unsupported { feature: Feature::NotYet(l), position: Some(u64::from(n.pos)) }))
    };
    // M2 でまだ描かない効果（ADR-89 (1)）
    if c.opacity < 1.0 {
        return not_yet(Later::Opacity);
    }
    if c.fx & FX_CLIP != 0 {
        return not_yet(Later::Clip);
    }
    if c.fx & FX_MASK != 0 {
        return not_yet(Later::Mask);
    }
    if c.fx & FX_FILTER != 0 {
        return not_yet(Later::Filter);
    }
    if c.fx & (FX_BLEND | FX_ISOLATE) != 0 {
        return not_yet(Later::Blend);
    }
    // 受け継がない参照（clip-path・mask・filter）の辺は値の並びの節へ張る：並びの節に参照先への辺があれば効く
    let xml_nodes = doc.nodes.len();
    let effective = |t: u32| (t as usize) < xml_nodes || !d.refs.of(t).is_empty();
    for e in d.refs.of(item.node) {
        budget.spend(cost::OP_STEP)?;
        match e.kind {
            EDGE_CLIP if effective(e.target) => return not_yet(Later::Clip),
            EDGE_MASK if effective(e.target) => return not_yet(Later::Mask),
            EDGE_FILTER if effective(e.target) => return not_yet(Later::Filter),
            EDGE_MARKER_START | EDGE_MARKER_MID | EDGE_MARKER_END => return not_yet(Later::Marker),
            _ => {}
        }
    }
    if item.clips {
        return not_yet(Later::Viewport);
    }
    // 変形が解けない（外接矩形の要る transform-box・表示の領域の決まらない %）：黙って違う所に描かない（単位 3 のレビュー）
    if !item.ctm_known {
        return not_yet(Later::TransformBox);
    }
    if n.ns != NS_SVG {
        return Ok(());
    }
    let local = n.local.of(input);
    let base = Base {
        default_font: d.font_size,
        root_font: out.root.map_or(d.font_size, |r| r.font_size),
        root_line: out.root.map_or(c.line_height, |r| r.line_height),
        viewport: item.viewport,
        doc: d.media_size,
        outermost: false,
    };
    let vp = item.viewport;
    let basis = |axis: Axis| -> Option<f64> {
        let (w, h) = vp?;
        Some(match axis {
            Axis::X => w,
            Axis::Y => h,
            Axis::Diag => crate::math::hypot(w, h) / core::f64::consts::SQRT_2,
        })
    };
    let g = &c.geo;
    let v0 = |k: usize, axis: Axis| g.used(k, basis(axis)).unwrap_or(0.0);
    let start_v = out.verbs.len();
    let start_c = out.coords.len();
    // 幾何：宣言ごとに読むもの（パスのデータと points）は置き場の範囲、図形は節ごとに書く
    let mut shared: Option<Ranges> = None;
    match local {
        b"path" => {
            if g.d_src == DASH_NONE {
                return Ok(());
            }
            let kind = if g.d_attr != 0 { KEY_D_ATTR } else { KEY_D_CSS };
            let key = DeclKey::new(g.d_src, g.d, kind);
            shared = geometry_slot(d, keys, slots, out, &key, budget)?;
        }
        b"polyline" | b"polygon" => {
            let Some(a) = doc.find_attr(input, n, NS_NONE, b"points", budget)? else { return Ok(()) };
            let src = if a.flags & FLAG_IN_STORE != 0 { SRC_STORE } else { SRC_INPUT };
            let kind = if local == b"polygon" { KEY_POLYGON } else { KEY_POLYLINE };
            shared = geometry_slot(d, keys, slots, out, &DeclKey::new(src, a.value, kind), budget)?;
        }
        b"rect" => {
            let (x, y) = (v0(G_X, Axis::X), v0(G_Y, Axis::Y));
            let (w, h) = (v0(G_WIDTH, Axis::X), v0(G_HEIGHT, Axis::Y));
            let rx = g.used(G_RX, basis(Axis::X));
            let ry = g.used(G_RY, basis(Axis::Y));
            // auto は他方の値、両方 auto なら 0。幅・高さの半分に切り詰める（SVG 2 §10.2）
            let (rx, ry) = match (rx, ry) {
                (None, None) => (0.0, 0.0),
                (Some(a), None) => (a, a),
                (None, Some(b)) => (b, b),
                (Some(a), Some(b)) => (a, b),
            };
            let rx = crate::math::min(rx, w / 2.0);
            let ry = crate::math::min(ry, h / 2.0);
            budget.spend(cost::OP_STEP)?;
            let mut pw = PathWriter::new(&mut out.verbs, &mut out.coords);
            geom::rect(&mut pw, x, y, w, h, rx, ry)?;
        }
        b"circle" => {
            let (cx, cy) = (v0(G_CX, Axis::X), v0(G_CY, Axis::Y));
            let r = v0(G_R, Axis::Diag);
            budget.spend(cost::OP_STEP)?;
            let mut pw = PathWriter::new(&mut out.verbs, &mut out.coords);
            geom::ellipse(&mut pw, cx, cy, r, r)?;
        }
        b"ellipse" => {
            let (cx, cy) = (v0(G_CX, Axis::X), v0(G_CY, Axis::Y));
            let rx = g.used(G_RX, basis(Axis::X));
            let ry = g.used(G_RY, basis(Axis::Y));
            let (rx, ry) = match (rx, ry) {
                (None, None) => (0.0, 0.0),
                (Some(a), None) => (a, a),
                (None, Some(b)) => (b, b),
                (Some(a), Some(b)) => (a, b),
            };
            budget.spend(cost::OP_STEP)?;
            let mut pw = PathWriter::new(&mut out.verbs, &mut out.coords);
            geom::ellipse(&mut pw, cx, cy, rx, ry)?;
        }
        b"line" => {
            let get = |name: &[u8], axis: Axis| -> Result<f64, Error> {
                Ok(attr_length(d, n, name, c, &base, axis, budget)?.unwrap_or(0.0))
            };
            let p1 = Point::new(get(b"x1", Axis::X)?, get(b"y1", Axis::Y)?);
            let p2 = Point::new(get(b"x2", Axis::X)?, get(b"y2", Axis::Y)?);
            budget.spend(cost::OP_STEP)?;
            let mut pw = PathWriter::new(&mut out.verbs, &mut out.coords);
            geom::line(&mut pw, p1, p2)?;
        }
        _ => return Ok(()),
    }
    let (verbs, coords) = match shared {
        Some(r) => r,
        None => (range_of(start_v, out.verbs.len())?, range_of(start_c, out.coords.len())?),
    };
    if verbs[1] == 0 {
        return Ok(());
    }
    let path = PathRef {
        verbs: out
            .verbs
            .as_slice()
            .get(verbs[0] as usize..(verbs[0] as usize).saturating_add(verbs[1] as usize))
            .unwrap_or(&[]),
        coords: out
            .coords
            .as_slice()
            .get(coords[0] as usize..(coords[0] as usize).saturating_add(coords[1] as usize))
            .unwrap_or(&[]),
    };
    let bounds = paid_bounds(path, &item.ctm, budget)?;
    if bounds.is_empty() || ![bounds.x0, bounds.y0, bounds.x1, bounds.y1].iter().all(|v| v.is_finite()) {
        return Ok(());
    }
    out.bbox = out.bbox.union(&bounds);
    if c.visibility != VISIBLE {
        return Ok(());
    }
    // 塗りと線（use の中身の context-fill・context-stroke は use の塗り）
    let fill = paint(d, item, c.fill, true, budget)?;
    let stroke = paint(d, item, c.stroke, false, budget)?;
    let fill = match fill {
        // line は塗らない（面がない。resvg・Chrome と同じ。ADR-90 (3)）
        _ if local == b"line" => None,
        Painted::NotYet(l) => return not_yet(l),
        Painted::Color(col) => Some(with_opacity(col, c.fill_opacity)),
        Painted::None => None,
    };
    let stroke = match stroke {
        Painted::NotYet(l) => return not_yet(l),
        Painted::Color(col) if c.stroke_width > 0.0 => Some(with_opacity(col, c.stroke_opacity)),
        _ => None,
    };
    if fill.is_none() && stroke.is_none() {
        return Ok(());
    }
    let l = &c.line;
    let mut flags = 0u8;
    if fill.is_some() {
        flags |= HAS_FILL;
    }
    if stroke.is_some() {
        flags |= HAS_STROKE;
    }
    if l.fill_rule == 1 {
        flags |= EVEN_ODD;
    }
    if l.stroke_first != 0 {
        flags |= STROKE_FIRST;
    }
    if l.crisp != 0 {
        flags |= CRISP;
    }
    if l.non_scaling != 0 {
        flags |= NON_SCALING;
    }
    if vp.is_some() {
        flags |= HAS_VP;
    }
    // 破線（線があるときだけ）。位相は要素の文脈で解く
    let mut dashes = [0u32, 0];
    let mut dash_offset = 0.0;
    if stroke.is_some() && l.dash_src != DASH_NONE {
        dashes = dash_slot(d, keys, slots, out, &DeclKey::new(l.dash_src, l.dash, KEY_DASH), budget)?;
        dash_offset = resolve(l.dash_offset, c.font_size, c.line_height, &base, Axis::Diag).unwrap_or(0.0);
    }
    // pathLength（負か読めなければ無いもの。0 は倍が無限で破線が実線になる：SVG 2・Chrome。ADR-95 (2)）
    let path_length = match doc.attr(input, n, NS_NONE, b"pathLength", budget)? {
        Some(v) => {
            let mut vals = Values::new(v, Span::new(0, v.len()), budget);
            match vals.number()? {
                Some(x) if x >= 0.0 && x.is_finite() && vals.at_end()? => x,
                _ => -1.0,
            }
        }
        None => -1.0,
    };
    if path_length == 0.0 {
        flags |= PATH_LENGTH_ZERO;
    }
    let path_length = if path_length > 0.0 { path_length } else { 0.0 };
    let (w, h) = vp.unwrap_or((0.0, 0.0));
    out.nodes.push(PathNode {
        transform: item.ctm,
        bounds,
        fill: fill.unwrap_or([0.0; 4]),
        stroke: stroke.unwrap_or([0.0; 4]),
        stroke_width: c.stroke_width,
        miter_limit: l.miter_limit,
        dash_offset,
        path_length,
        font_size: c.font_size,
        line_height: c.line_height,
        viewport: [w, h],
        verbs,
        coords,
        dashes,
        flags,
        cap: match l.cap {
            1 => CAP_ROUND,
            2 => CAP_SQUARE,
            _ => CAP_BUTT,
        },
        join: match l.join {
            1 => JOIN_ROUND,
            2 => JOIN_BEVEL,
            _ => JOIN_MITER,
        },
        pad: [0; 5],
    })
}

/// 外接矩形（区間ごとに払う）。
fn paid_bounds(path: PathRef<'_>, t: &Transform, budget: &Budget<'_>) -> Result<Rect, Error> {
    let mut r = Rect::EMPTY;
    for seg in path.segments() {
        budget.spend(cost::BOUNDS_SEGMENT)?;
        geom::add_bounds(&mut r, &geom::transform_seg(&seg, t));
    }
    Ok(r)
}

/// 使う塗り。
enum Painted {
    None,
    Color(Color),
    NotYet(Later),
}

/// 要素の塗り（use の中身の context-fill・context-stroke は use の塗りで。ほかの文脈の塗りは none）。
fn paint(d: &Document<'_>, item: &WalkItem, p: CPaint, is_fill: bool, budget: &Budget<'_>) -> Result<Painted, Error> {
    let c = &item.computed;
    let (p, color, node) = match (p.kind, item.context) {
        (PAINT_CONTEXT_FILL, Some(x)) => (x.fill, x.fill_color, x.fill_node),
        (PAINT_CONTEXT_STROKE, Some(x)) => (x.stroke, x.stroke_color, x.stroke_node),
        (PAINT_CONTEXT_FILL | PAINT_CONTEXT_STROKE, None) => return Ok(Painted::None),
        _ => (p, c.color, item.node),
    };
    Ok(match used_paint(d, node, &p, is_fill, color, budget)? {
        Used::None | Used::ContextFill | Used::ContextStroke => Painted::None,
        Used::Color(col) => Painted::Color(col),
        Used::Server(t) => {
            let grad = d.xml.node(t).is_some_and(|x| x.local.of(d.input) != b"pattern");
            Painted::NotYet(if grad { Later::Gradient } else { Later::Pattern })
        }
        Used::Cut => Painted::NotYet(Later::Pattern),
    })
}

/// 色に不透明度を掛けた節の色（r・g・b は 0〜255、α は 0〜1）。
fn with_opacity(c: Color, opacity: f64) -> [f64; 4] {
    [f64::from(c.r), f64::from(c.g), f64::from(c.b), f64::from(c.a) * opacity]
}

/// 属性の長さ（単位のない数も受ける。書いていない・壊れた値・解けない長さは None）。
fn attr_length(
    d: &Document<'_>,
    n: &crate::xml::Node,
    name: &[u8],
    c: &Computed,
    base: &Base,
    axis: Axis,
    budget: &Budget<'_>,
) -> Result<Option<f64>, Error> {
    let Some(v) = d.xml.attr(d.input, n, NS_NONE, name, budget)? else { return Ok(None) };
    let mut vals = Values::new(v, Span::new(0, v.len()), budget_ref(budget));
    let Some(l) = vals.length(true)? else { return Ok(None) };
    if !vals.at_end()? {
        return Ok(None);
    }
    Ok(resolve(l, c.font_size, c.line_height, base, axis))
}

/// 置き場の範囲（命令と座標）。
type Ranges = ([u32; 2], [u32; 2]);

/// 宣言ごとの幾何（パスのデータ・points）を読む（2 度目からは置き場の範囲を返す）。空なら None でなく長さ 0 の範囲。
fn geometry_slot(
    d: &Document<'_>,
    keys: &[DeclKey],
    slots: &mut MVec<'_, DeclSlot>,
    out: &mut Out<'_>,
    key: &DeclKey,
    budget: &Budget<'_>,
) -> Result<Option<Ranges>, Error> {
    let Some(i) = find_key(keys, key, budget)? else { return Ok(Some(([0, 0], [0, 0]))) };
    if let Some(s) = slots.get(i)
        && s.done != 0
    {
        return Ok(Some((s.a, s.b)));
    }
    let bufs = Bufs { input: d.input, store: d.xml.store.as_slice(), sheet: d.sheet.text.as_slice() };
    let buf = bufs.of(key.src);
    let span = Span::new(key.start as usize, key.end as usize);
    let (v0, c0) = (out.verbs.len(), out.coords.len());
    match key.kind {
        KEY_D_ATTR => {
            let mut pw = PathWriter::new(&mut out.verbs, &mut out.coords);
            path::path_data(span.of(buf), &mut pw, budget)?;
        }
        KEY_D_CSS => {
            // path("…") の文字列の escape を解いてから読む
            let mut v = Values::new(buf, span, budget_ref(budget));
            let f = v.next()?;
            let s = v.next()?;
            if f.kind == Tk::Function && ident_is(buf, f.inner, f.escaped, b"path") && s.kind == Tk::Str {
                out.scratch.clear();
                if s.escaped {
                    for ch in decode_string(buf, s.inner, budget) {
                        let (b, k) = crate::css::select::utf8_pub(ch?);
                        out.scratch.append_slice(b.get(..k).unwrap_or(&[]))?;
                    }
                } else {
                    out.scratch.append_slice(s.inner.of(buf))?;
                }
                let text = out.scratch.as_slice();
                let mut pw = PathWriter::new(&mut out.verbs, &mut out.coords);
                path::path_data(text, &mut pw, budget)?;
            }
        }
        _ => {
            out.points.clear();
            if path::points(span.of(buf), &mut out.points, budget)? {
                let mut pw = PathWriter::new(&mut out.verbs, &mut out.coords);
                let pts = out.points.as_slice();
                // 点が 1 つなら描かない（線分が要る。SVG 2 §10.6）
                if pts.len() >= 2 {
                    for (k, &p) in pts.iter().enumerate() {
                        budget.spend(cost::OP_STEP)?;
                        if k == 0 {
                            pw.move_to(p)?;
                        } else {
                            pw.line_to(p)?;
                        }
                    }
                    if key.kind == KEY_POLYGON {
                        pw.close()?;
                    }
                }
            }
        }
    }
    let r = (range_of(v0, out.verbs.len())?, range_of(c0, out.coords.len())?);
    if let Some(s) = slots.get_mut(i) {
        *s = DeclSlot { a: r.0, b: r.1, done: 1, pad: [0; 3] };
    }
    Ok(Some(r))
}

/// 宣言ごとの破線の並び（解かない長さ。負の長さを含むか読めなければ長さ 0 の範囲：実線）。
fn dash_slot(
    d: &Document<'_>,
    keys: &[DeclKey],
    slots: &mut MVec<'_, DeclSlot>,
    out: &mut Out<'_>,
    key: &DeclKey,
    budget: &Budget<'_>,
) -> Result<[u32; 2], Error> {
    let Some(i) = find_key(keys, key, budget)? else { return Ok([0, 0]) };
    if let Some(s) = slots.get(i)
        && s.done != 0
    {
        return Ok(s.a);
    }
    let bufs = Bufs { input: d.input, store: d.xml.store.as_slice(), sheet: d.sheet.text.as_slice() };
    let buf = bufs.of(key.src);
    let start = out.dashes.len();
    let mut v = Values::new(buf, Span::new(key.start as usize, key.end as usize), budget_ref(budget));
    let mut ok = !v.eat_ident(b"none")?;
    while ok && !v.at_end()? {
        budget.spend(cost::OP_STEP)?;
        match v.length(true)? {
            Some(l) if l.value >= 0.0 && l.value.is_finite() => out.dashes.push(l)?,
            _ => ok = false,
        }
        if v.peek()?.kind == Tk::Comma {
            v.next()?;
        }
    }
    if !ok {
        out.dashes.truncate(start);
    }
    let r = range_of(start, out.dashes.len())?;
    if let Some(s) = slots.get_mut(i) {
        *s = DeclSlot { a: r, b: [0, 0], done: 1, pad: [0; 3] };
    }
    Ok(r)
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
