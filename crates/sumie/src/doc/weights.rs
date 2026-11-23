//! 木を作る段の見積もりの重み（architecture.md §4.3 の「ここまでで `estimate_build` が返せる」。ADR-93）。
//!
//! 木を作る段の手間は、展開した要素ごとの手間の和と、宣言ごとに 1 度だけ読む分（パスのデータ・`points`・破線）の和。
//! 要素ごとの手間は要素の形（子・属性・指定の値・参照の辺の数と、値のバイト数）で決まる重みで、展開の数と同じ数え方
//! （`refs::count_instances`。輪として取り除いた辺の先も同じ）で足す。文脈に依るもの（受け継いだ url の塗り・受け継いだ
//! `d`）は文書の中の最大で数える。確保は、宣言の鍵・たどる段の積み上げ・作る節と置き場（上限の容量で取り、最後に詰めて
//! 写す）の和。

use crate::budget::Budget;
use crate::cost;
use crate::error::Error;
use crate::mem::{MVec, Plain, lower_bound_try, stable_sort};
use crate::style::cascade::{ORIGIN_ATTR, SRC_SHEET, Styles, WIDE_NONE};
use crate::style::props::lookup;
use crate::xml::lex::Span;
use crate::xml::{KIND_ELEMENT, NONE, NS_NONE, NS_SVG, XmlDoc};

/// 木のパスの節（`tree::PathNode`）と、木を作る段の宣言の鍵と読んだ結果の大きさ（doc は層の下なので木の型を見ない。木の
/// 試験が同じことを確かめる）。
pub(crate) const PATH_NODE_BYTES: u64 = 240;
pub(crate) const KEY_BYTES: u64 = 12;
pub(crate) const SLOT_BYTES: u64 = 20;

/// 木を作る段の見積もり（`Document::estimate_build` が返す元）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BuildWeights {
    pub(crate) work: u64,
    pub(crate) peak_bytes: u64,
    pub(crate) result_bytes: u64,
    /// 作るパスの節・命令・座標・破線の数の上限と、宣言の鍵の数の上限（木を作る段が容量に使う）
    pub(crate) shapes: u64,
    pub(crate) verbs: u64,
    pub(crate) coords: u64,
    pub(crate) dashes: u64,
    pub(crate) keys: u64,
}

/// 文書の全体の量（展開に依らない分）。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Globals {
    nodes: u64,
    specs: u64,
    keys: u64,
    poly_attrs: u64,
    /// 宣言ごとに 1 度だけ読む手間と、置き場に書く命令・座標・破線の上限
    decl_work: u64,
    decl_verbs: u64,
    decl_coords: u64,
    dashes: u64,
    /// CSS の `d` の値・`points` の値の最大のバイト数（escape を解いた写しと点の一時の置き場）
    max_css_d: u64,
    max_points: u64,
    /// url の塗りの値の最大のバイト数（受け継いだ url の中身の上限）と、escape があるか
    max_paint: u64,
    paint_escaped: bool,
    max_depth: u64,
    uses: u64,
}

/// 図形の要素（宣言でなく節ごとに書くもの）。
fn basic_shape(local: &[u8]) -> bool {
    matches!(local, b"rect" | b"circle" | b"ellipse" | b"line")
}

fn shape(local: &[u8]) -> bool {
    basic_shape(local) || matches!(local, b"path" | b"polyline" | b"polygon")
}

/// パスのデータ `bytes` バイトが書く命令と座標の上限（命令は 1 バイトあたり 1 つ ＋ 1 以下：`values::path` の試験
/// `work_and_verbs_per_byte`。置き場の量は 1 バイトあたり `PATH_GEOMETRY_BYTES` 以下なので、座標はその 8 分の 1。CSS の
/// `path("…")` は escape を解くと 1.5 倍まで長くなるので、呼び手が 2 倍にして渡す）。
const fn path_verbs(bytes: u64) -> u64 {
    bytes.saturating_add(1)
}

const fn path_coords(bytes: u64) -> u64 {
    cost::PATH_GEOMETRY_BYTES.saturating_mul(bytes).saturating_add(8) / 8
}

/// 属性のパスのデータが書く命令と座標の上限を、字句をなめて数える（命令の文字ごとに、続く数の個数を命令の引数の数で
/// 割ったもの ＋ 1（`Z` のあとの暗黙の移動）。数は、数字か `.` で、前が数字でないものの数で数える（`1.5` を 2 つと数え、
/// 多めになる）。弧は旗が次の数とつながりうるので 5 つで 1 つ。読むバイトを塊ずつ払う）。
pub(crate) fn path_scan(v: &[u8], budget: &Budget<'_>) -> Result<(u64, u64), Error> {
    // (引数の数, 命令 1 つの座標の数)
    let arity = |c: u8| -> (u64, u64) {
        match c.to_ascii_lowercase_checked() {
            b'm' | b'l' => (2, 2),
            b'h' | b'v' => (1, 2),
            b't' => (2, 4),
            b'q' => (4, 4),
            b's' => (4, 6),
            b'c' => (6, 6),
            b'a' => (5, 10),
            _ => (0, 0),
        }
    };
    let mut meter = crate::xml::lex::ScanMeter::new(0, v.len());
    let (mut verbs, mut coords) = (1u64, 2u64);
    let mut cur: Option<(u64, u64)> = None;
    let mut nums = 0u64;
    let mut prev_digit = false;
    let flush = |cur: Option<(u64, u64)>, nums: u64, verbs: &mut u64, coords: &mut u64| {
        let Some((a, c)) = cur else { return };
        let k = if a == 0 { 1 } else { nums.div_ceil_checked(a).max(1) };
        // 暗黙の移動（2 つの座標）を含めて
        *verbs = verbs.saturating_add(k).saturating_add(1);
        *coords = coords.saturating_add(k.saturating_mul(c)).saturating_add(2);
    };
    for (k, &b) in v.iter().enumerate() {
        meter.pay(budget, k)?;
        let digit = b.is_ascii_digit();
        if b.is_ascii_alphabetic() && b != b'e' && b != b'E' {
            flush(cur, nums, &mut verbs, &mut coords);
            cur = Some(arity(b));
            nums = 0;
        } else if (digit || b == b'.') && !prev_digit {
            nums = nums.saturating_add(1);
        }
        prev_digit = digit;
    }
    flush(cur, nums, &mut verbs, &mut coords);
    Ok((verbs, coords))
}

/// `d` の宣言が書く命令と座標の上限（属性と、escape のない CSS の `path("…")` は字句をなめて数え（`path` の字も命令として
/// 数えるので多めになる）、escape のある CSS は解くと 1.5 倍まで長くなるのでバイト数の 2 倍から）。
fn d_bound(buf: &[u8], value: Span, css: bool, budget: &Budget<'_>) -> Result<(u64, u64), Error> {
    let v = value.of(buf);
    if !css || !contains(v, b'\\', budget)? {
        return path_scan(v, budget);
    }
    let eff = (value.len() as u64).saturating_mul(2);
    Ok((path_verbs(eff), path_coords(eff)))
}

/// 値にバイト `c` があるか（読むバイトを塊ずつ払う）。
fn contains(v: &[u8], c: u8, budget: &Budget<'_>) -> Result<bool, Error> {
    let mut meter = crate::xml::lex::ScanMeter::new(0, v.len());
    for (k, &b) in v.iter().enumerate() {
        meter.pay(budget, k)?;
        if b == c {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 宣言の種類（塗り・`d`・`stroke-dasharray`）。
const DECL_PAINT: u8 = 0;
const DECL_D: u8 = 1;
const DECL_DASH: u8 = 2;

/// 規則の並びの宣言（重なりを除いて 1 度だけ数える）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct SheetDecl {
    start: u32,
    end: u32,
    kind: u8,
    pad: [u8; 7],
    /// `d` の命令の数の上限
    verbs: u64,
}

impl Plain for SheetDecl {}

/// 宣言 1 つを全体の量に足す（`d` なら命令の数の上限を返す）。
fn decl(
    g: &mut Globals,
    max_d: &mut u64,
    buf: &[u8],
    value: Span,
    css: bool,
    kind: u8,
    budget: &Budget<'_>,
) -> Result<u64, Error> {
    let bytes = value.len() as u64;
    match kind {
        DECL_PAINT => {
            // url の塗り（受け継いだ url の中身を引く手間の上限。色の塗りは引かない。escape で書いた url の関数もあるので、
            // `\` があれば url とみなし、解く手間で数える。単位 3 のレビュー）
            let v = value.of(buf);
            let escaped = contains(v, b'\\', budget)?;
            if escaped || has_url(buf, value, budget)? {
                g.max_paint = g.max_paint.max(bytes);
                g.paint_escaped |= escaped;
            }
            Ok(0)
        }
        DECL_D => {
            let eff = if css { bytes.saturating_mul(2) } else { bytes };
            let (verbs, coords) = d_bound(buf, value, css, budget)?;
            *max_d = (*max_d).max(verbs);
            if css {
                g.max_css_d = g.max_css_d.max(eff);
            }
            g.keys = g.keys.saturating_add(1);
            g.decl_work =
                g.decl_work.saturating_add(eff.saturating_mul(cost::PATH_BYTE)).saturating_add(cost::BUILD_SPEC);
            g.decl_verbs = g.decl_verbs.saturating_add(verbs);
            g.decl_coords = g.decl_coords.saturating_add(coords);
            Ok(verbs)
        }
        _ => {
            g.keys = g.keys.saturating_add(1);
            g.decl_work =
                g.decl_work.saturating_add(bytes.saturating_mul(cost::DECL_BYTE)).saturating_add(cost::BUILD_SPEC);
            g.dashes = g.dashes.saturating_add(bytes);
            Ok(0)
        }
    }
}

/// 小文字にする（ASCII の文字だけ。std の Unicode の表に依らない）。
trait Lower {
    fn to_ascii_lowercase_checked(self) -> u8;
}

impl Lower for u8 {
    fn to_ascii_lowercase_checked(self) -> u8 {
        if self.is_ascii_uppercase() { self | 0x20 } else { self }
    }
}

/// 切り上げの割り算（0 で割るなら 0）。
trait DivCeil {
    fn div_ceil_checked(self, d: u64) -> u64;
}

impl DivCeil for u64 {
    fn div_ceil_checked(self, d: u64) -> u64 {
        match (self.checked_div(d), self.checked_rem(d)) {
            (Some(q), Some(r)) => q.saturating_add(u64::from(r != 0)),
            _ => 0,
        }
    }
}

/// `points` の値 `bytes` バイトが書く命令と座標の上限（数は 2 つめから区切りか符号か点が要るので (bytes + 1) / 2 個まで。
/// 点はその半分。移動と線と閉じ）。
const fn points_verbs(bytes: u64) -> u64 {
    (bytes >> 1).saturating_add(3)
}

const fn points_coords(bytes: u64) -> u64 {
    bytes.saturating_add(4)
}

/// 要素ごとに名前で引く属性（たどる段：use の x・y と、use の中の symbol が引く use の width・height、入れ子の svg と
/// symbol の x・y・width・height・viewBox・preserveAspectRatio。木を作る段：line の x1・y1・x2・y2、図形の pathLength、
/// polyline・polygon の points（値は宣言ごとに 1 度だけ読むので、引く回数だけ））。
fn looked_up(local: &[u8]) -> &'static [&'static [u8]] {
    match local {
        b"use" => &[b"x", b"y", b"width", b"height"],
        b"svg" | b"symbol" => &[b"x", b"y", b"width", b"height", b"viewBox", b"preserveAspectRatio"],
        b"line" => &[b"x1", b"y1", b"x2", b"y2", b"pathLength"],
        b"polyline" | b"polygon" => &[b"pathLength"],
        b"rect" | b"circle" | b"ellipse" | b"path" => &[b"pathLength"],
        _ => &[],
    }
}

/// 値に `url` があるか（大文字小文字を問わない。読むバイトを塊ずつ払う）。
fn has_url(buf: &[u8], span: crate::xml::lex::Span, budget: &Budget<'_>) -> Result<bool, Error> {
    let v = span.of(buf);
    let mut meter = crate::xml::lex::ScanMeter::new(0, v.len());
    for k in 0..v.len() {
        meter.pay(budget, k)?;
        if v.get(k..k.saturating_add(3)).is_some_and(|w| w.eq_ignore_ascii_case(b"url")) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 要素ごとの重み（展開した要素 1 つの手間）と、文書の全体の量を求める。`start` は要素ごとの辺の始まり（節の数 ＋ 1）。
pub(crate) fn node_weights<'b>(
    doc: &XmlDoc,
    input: &[u8],
    sheet: &[u8],
    styles: &Styles,
    start: &[u32],
    ids: u64,
    budget: &'b Budget<'b>,
) -> Result<(MVec<'b, u64>, Globals), Error> {
    let bufs = crate::style::computed::Bufs { input, store: doc.store.as_slice(), sheet };
    let nodes = doc.nodes.as_slice();
    let n = nodes.len();
    let d_id = lookup(b"d", false);
    let dash_id = lookup(b"stroke-dasharray", false);
    let fill_id = lookup(b"fill", false);
    let stroke_id = lookup(b"stroke", false);
    let mut g = Globals { nodes: n as u64, ..Globals::default() };
    // 1 度目：文書の全体の量（受け継いだ d・塗りの最大、深さ、use の数、宣言の量）
    let mut depth: MVec<'_, u32> = MVec::with_capacity(budget, n)?;
    let mut max_d = 0u64;
    let mut sheet_decls: MVec<'_, SheetDecl> = MVec::new(budget);
    for (i, node) in nodes.iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        let dp = if node.parent == NONE { 0 } else { depth.get(node.parent as usize).copied().unwrap_or(0) };
        let me = dp.saturating_add(1);
        depth.push(me)?;
        if node.kind != KIND_ELEMENT {
            continue;
        }
        g.max_depth = g.max_depth.max(u64::from(me));
        let svg = node.ns == NS_SVG;
        let local = node.local.of(input);
        if svg && local == b"use" {
            g.uses = g.uses.saturating_add(1);
        }
        for s in styles.of(i as u32) {
            budget.spend(cost::OP_STEP)?;
            g.specs = g.specs.saturating_add(1);
            let p = Some(s.prop);
            let kind = if p == fill_id || p == stroke_id {
                DECL_PAINT
            } else if s.wide != WIDE_NONE {
                continue;
            } else if p == d_id {
                DECL_D
            } else if p == dash_id {
                DECL_DASH
            } else {
                continue;
            };
            // 規則の並びの宣言は当たる要素で共有するので、宣言ごとに 1 度だけ数える（並べて重なりを除いてから。単位 3 の
            // レビュー）
            if s.src == SRC_SHEET {
                sheet_decls.push(SheetDecl { start: s.value.start, end: s.value.end, kind, pad: [0; 7], verbs: 0 })?;
                continue;
            }
            decl(&mut g, &mut max_d, bufs.of(s.src), s.value, s.origin != ORIGIN_ATTR, kind, budget)?;
        }
        if svg && matches!(local, b"polyline" | b"polygon") {
            g.poly_attrs = g.poly_attrs.saturating_add(node.attrs.len() as u64);
            if let Some(v) = doc.attr(input, node, NS_NONE, b"points", budget)? {
                let bytes = v.len() as u64;
                g.keys = g.keys.saturating_add(1);
                g.max_points = g.max_points.max(bytes);
                g.decl_work =
                    g.decl_work.saturating_add(bytes.saturating_mul(cost::DECL_BYTE)).saturating_add(cost::BUILD_SPEC);
                g.decl_verbs = g.decl_verbs.saturating_add(points_verbs(bytes));
                g.decl_coords = g.decl_coords.saturating_add(points_coords(bytes));
            }
        }
    }
    drop(depth);
    // 規則の並びの宣言を 1 度ずつ
    stable_sort(budget, sheet_decls.as_mut_slice(), |a, b| (a.start, a.kind) < (b.start, b.kind))?;
    let mut keep = 0usize;
    for k in 0..sheet_decls.len() {
        budget.spend(cost::OP_STEP)?;
        let Some(mut x) = sheet_decls.get(k).copied() else { break };
        if keep > 0 && sheet_decls.get(keep.saturating_sub(1)).is_some_and(|y| (y.start, y.kind) == (x.start, x.kind)) {
            continue;
        }
        x.verbs = decl(&mut g, &mut max_d, sheet, Span { start: x.start, end: x.end }, true, x.kind, budget)?;
        if let Some(y) = sheet_decls.get_mut(keep) {
            *y = x;
        }
        keep = keep.saturating_add(1);
    }
    sheet_decls.truncate(keep);
    // 2 度目：要素ごとの重み
    let paint =
        if g.max_paint == 0 { cost::BUILD_PAINT_COLOR } else { cost::build_paint(g.max_paint, g.paint_escaped, ids) };
    let lookups = cost::search(g.keys).saturating_mul(2);
    let node_push = cost::OP_STEP.saturating_add(cost::copy(PATH_NODE_BYTES));
    let mut w: MVec<'_, u64> = MVec::with_capacity(budget, n)?;
    for (i, node) in nodes.iter().enumerate() {
        budget.spend(cost::OP_STEP)?;
        if node.kind != KIND_ELEMENT {
            w.push(0)?;
            continue;
        }
        let mut children = 0u64;
        let mut c = node.first_child;
        while c != NONE {
            budget.spend(cost::OP_STEP)?;
            children = children.saturating_add(1);
            c = nodes.get(c as usize).map_or(NONE, |x| x.next_sibling);
        }
        let svg = node.ns == NS_SVG;
        let local = if svg { node.local.of(input) } else { &[][..] };
        // 名前で引く属性（引く回数と、引いて読む値のバイト数）
        let looked = looked_up(local);
        let attrs = doc.attrs_of(node);
        let mut attr_bytes = 0u64;
        if !looked.is_empty() {
            for a in attrs {
                budget.spend(cost::OP_STEP)?;
                if a.ns == NS_NONE && looked.contains(&a.local.of(input)) {
                    attr_bytes = attr_bytes.saturating_add(a.value.len() as u64);
                }
            }
        }
        let mut specs = 0u64;
        let mut spec_bytes = 0u64;
        let mut own_d: Option<u64> = None;
        for s in styles.of(i as u32) {
            budget.spend(cost::OP_STEP)?;
            specs = specs.saturating_add(1);
            // d と stroke-dasharray は宣言ごとに 1 度だけ読む（計算した値は頭の字句を 1 つ見るだけ）
            if Some(s.prop) != d_id && Some(s.prop) != dash_id {
                spec_bytes = spec_bytes.saturating_add(s.value.len() as u64);
            }
            if Some(s.prop) == d_id {
                own_d = Some(if s.wide != WIDE_NONE {
                    max_d
                } else if s.src == SRC_SHEET {
                    let j = lower_bound_try(budget, sheet_decls.as_slice(), |x| {
                        Ok((x.start, x.kind) < (s.value.start, DECL_D))
                    })?;
                    sheet_decls
                        .get(j)
                        .filter(|x| x.start == s.value.start && x.kind == DECL_D)
                        .map_or(max_d, |x| x.verbs)
                } else {
                    d_bound(bufs.of(s.src), s.value, s.origin != ORIGIN_ATTR, budget)?.0
                });
            }
        }
        let a = start.get(i).copied().unwrap_or(0);
        let b = start.get(i.saturating_add(1)).copied().unwrap_or(a);
        let edges = u64::from(b.saturating_sub(a));
        // polyline・polygon は points も引く（値は宣言ごとに 1 度だけ読む）
        let extra = u64::from(matches!(local, b"polyline" | b"polygon"));
        let probes = (attrs.len() as u64).saturating_mul((looked.len() as u64).saturating_add(extra));
        let mut x = cost::build_node(children, probes, attr_bytes, specs, spec_bytes, edges);
        if shape(local) {
            let segs = if basic_shape(local) {
                cost::SHAPE_VERBS
            } else if local == b"path" {
                own_d.unwrap_or(0)
            } else {
                points_verbs(doc.attr(input, node, NS_NONE, b"points", budget)?.map_or(0, |v| v.len() as u64))
            };
            let write = if basic_shape(local) { cost::SHAPE_WRITE } else { 0 };
            x = x
                .saturating_add(paint.saturating_mul(2))
                .saturating_add(segs.saturating_mul(cost::BOUNDS_SEGMENT))
                .saturating_add(lookups)
                .saturating_add(write)
                .saturating_add(node_push);
        }
        w.push(x)?;
    }
    Ok((w, g))
}

/// 根から展開したあとの和（`refs::count_instances` の根の数え）。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Expanded {
    pub(crate) instances: u64,
    pub(crate) work: u64,
    /// 図形の節の数の上限（塗る要素の数）と、節ごとに書く図形の数
    pub(crate) shapes: u64,
    pub(crate) basic: u64,
    /// たどる段の積み上げの深さの上限（use の中身は use の段と参照先の段で 2 つ深くなる）
    pub(crate) depth: u64,
}

/// 置き場を伸ばす手間の上限（`n` 個まで 1 つずつ足す。伸ばすたびの確保と写し）。
const fn grow_work(n: u64, size: u64) -> u64 {
    let steps = cost::ceil_log2(n.saturating_add(1)).saturating_add(1);
    n.saturating_mul(cost::OP_STEP.saturating_add(cost::copy(size)))
        .saturating_add(steps.saturating_mul(cost::ALLOC_STEP))
        .saturating_add(cost::copy(n.saturating_mul(size)).saturating_mul(2))
}

/// 1 つずつ足して `n` 個まで伸ばす置き場の確保の山（倍々で伸ばすので、伸ばす途中は古い領域と新しい領域が生きる）。
const fn grow_bytes(n: u64, size: u64) -> u64 {
    n.saturating_add(4).saturating_mul(size).saturating_mul(3)
}

/// 見積もりを閉じる（`report` は文書の報告の量。木の報告はその写し）。
pub(crate) fn close_weights(g: &Globals, e: &Expanded, report_bytes: u64) -> BuildWeights {
    let node_size = PATH_NODE_BYTES;
    let frame = crate::doc::walk::FRAME_BYTES as u64;
    let ctx = crate::doc::walk::CONTEXT_BYTES as u64;
    let shapes = e.shapes;
    let verbs = g.decl_verbs.saturating_add(e.basic.saturating_mul(cost::SHAPE_VERBS));
    let coords = g.decl_coords.saturating_add(e.basic.saturating_mul(cost::SHAPE_COORDS));
    let dashes = g.dashes;
    let length_size = size_of::<crate::values::Length>() as u64;
    let out_bytes = shapes
        .saturating_mul(node_size)
        .saturating_add(verbs)
        .saturating_add(coords.saturating_mul(8))
        .saturating_add(dashes.saturating_mul(length_size));
    // たどる段の積み上げ：展開の深さまで（use の文脈の積み上げはその内）
    let frames = e.depth.saturating_add(1);
    let ctxs = e.depth.saturating_add(1);
    let key_size = KEY_BYTES;
    let slot_size = SLOT_BYTES;
    let keys = g.keys;
    let work = e
        .work
        .saturating_add(g.decl_work)
        // 鍵を集める・並べる・重なりを除く
        .saturating_add(g.nodes.saturating_add(g.specs).saturating_add(g.poly_attrs).saturating_mul(cost::OP_STEP))
        .saturating_add(keys.saturating_mul(cost::OP_STEP.saturating_add(cost::copy(key_size))))
        .saturating_add(cost::stable_sort(keys, key_size))
        .saturating_add(keys.saturating_mul(cost::OP_STEP))
        .saturating_add(cost::fill(keys.saturating_mul(slot_size)))
        // 積み上げにある数の表・たどる段と use の文脈の積み上げ・一時の置き場
        .saturating_add(cost::fill(g.nodes.saturating_mul(4)))
        .saturating_add(grow_work(frames, frame))
        .saturating_add(grow_work(ctxs, ctx))
        .saturating_add(grow_work(g.max_css_d, 1))
        .saturating_add(grow_work(g.max_points, 16))
        // 置き場を詰めて写す
        .saturating_add(cost::copy(out_bytes))
        .saturating_add(cost::ALLOC_STEP.saturating_mul(32))
        .saturating_add(cost::copy(report_bytes));
    let peak = keys
        .saturating_mul(key_size)
        .saturating_add(cost::stable_sort_bytes(keys, key_size))
        .saturating_add(keys.saturating_mul(slot_size))
        .saturating_add(g.nodes.saturating_mul(4))
        .saturating_add(grow_bytes(frames, frame))
        .saturating_add(grow_bytes(ctxs, ctx))
        .saturating_add(grow_bytes(g.max_css_d, 1))
        .saturating_add(grow_bytes(g.max_points, 16))
        .saturating_add(grow_bytes(g.max_paint.saturating_mul(2), 1))
        .saturating_add(out_bytes.saturating_mul(2))
        .saturating_add(report_bytes);
    BuildWeights {
        work,
        peak_bytes: peak,
        result_bytes: out_bytes.saturating_add(report_bytes),
        shapes,
        verbs,
        coords,
        dashes,
        keys,
    }
}

#[allow(dead_code, reason = "Plain の確かめ")]
const fn _plain<T: Plain>() {}
