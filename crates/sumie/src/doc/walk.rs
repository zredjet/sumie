//! 文書の木をたどって、要素ごとの計算した値と根からの変形を求める（architecture.md §4.2・§4.3）。木を作る段（M2）は同じ
//! 求め方に use の展開を足して使う。M1 は照合の道具（`sumie-tools values`）と試験が使う。
//!
//! - 行きがけの順に、明示の積み上げで（深さごとの計算した値と、子に掛ける変形・表示の領域を積む）
//! - 根からの変形：根の viewBox の変形 × 祖先の変形 × 要素の変形。use は自分の変形のあとに x・y の平行移動を掛け、入れ子の
//!   svg は子に x・y の平行移動と viewBox の変形を掛ける（svg 自身の変形には含めない）。根の svg 自身の変形は読まない
//! - 表示の領域（% の基準）：根は文書の大きさ（viewBox があればその幅・高さ）。入れ子の svg は `width`・`height`（既定
//!   100%。属性だけを読む）と viewBox
//! - 描く木に入るか：SVG の名前空間の知っている要素で、子を描く入れ物（svg・g・a・switch と文字の要素）の子であり、描かない
//!   入れ物（defs など）の中でなく、条件が偽・switch で選ばれない要素の部分木（refs の `hidden`）でなく、`display: none` の
//!   部分木の中でない。変形が逆にできない（行列式が 0）要素と、幅か高さが 0 以下の入れ子の svg は、部分木ごと描かない
//!   （CSS Transforms 1 §6・SVG 2 §8.2）

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::decode_string;
use crate::doc::Document;
use crate::doc::needs::known;
use crate::doc::refs::{EDGE_FILL, EDGE_STROKE, not_rendered};
use crate::error::Error;
use crate::mem::{MVec, Plain};
use crate::style::computed::{
    Axis, Base, Bufs, CPaint, Computed, FALLBACK_COLOR, PAINT_CONTEXT_FILL, PAINT_CONTEXT_STROKE, PAINT_NONE,
    PAINT_URL, PropIds, compute, resolve,
};
use crate::values::{self, Aspect, Color, Length, Transform, Unit, Values, ViewBox, view_box_transform};
use crate::xml::lex::Span;
use crate::xml::{KIND_ELEMENT, NONE, NS_NONE, NS_SVG, Node, XmlDoc};

/// たどった 1 つの要素。
#[derive(Debug, Clone, Copy)]
pub(crate) struct WalkItem {
    pub(crate) node: u32,
    /// 親の要素の節（根は NONE）
    pub(crate) parent: u32,
    pub(crate) computed: Computed,
    /// 根からの変形（自分の変形まで。use は x・y を含み、svg は表示の領域の変形を含まない）
    pub(crate) ctm: Transform,
    /// 変形が解けた（外接矩形の要る `transform-box` の変形が途中になければ真）
    pub(crate) ctm_known: bool,
    /// 描く木に入る
    pub(crate) rendered: bool,
}

/// 積み上げの段。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Frame {
    computed: Computed,
    /// 子に掛ける変形
    ctm_children: Transform,
    /// 子の表示の領域
    vp_w: f64,
    vp_h: f64,
    node: u32,
    /// 次に見る子
    child: u32,
    has_vp: u8,
    /// 子が描く木に入りうる
    children_rendered: u8,
    known: u8,
    pad: [u8; 5],
}

impl Plain for Frame {}

/// 文書を行きがけの順にたどり、要素ごとに `f` を呼ぶ（描く木に入らない要素も、計算した値を求めて渡す）。
pub(crate) fn walk(
    d: &Document<'_>,
    budget: &Budget<'_>,
    f: &mut dyn FnMut(&WalkItem) -> Result<(), Error>,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    let doc = &d.xml;
    let input = d.input;
    let nodes = doc.nodes.as_slice();
    let Some(root) = nodes.first().filter(|n| n.kind == KIND_ELEMENT) else { return Ok(()) };
    let ids = PropIds::new();
    let bufs = Bufs { input, store: doc.store.as_slice(), sheet: d.sheet.text.as_slice() };
    let size = d.size.known().map(|s| (s.width, s.height));
    let initial = Computed::initial(d.font_size);
    let mut base = Base {
        default_font: d.font_size,
        root_font: d.font_size,
        root_line: initial.line_height,
        viewport: size,
        doc: size,
    };
    budget.spend(cost::OP_STEP)?;
    let c0 = compute(&initial, d.styles.of(0), &ids, &bufs, &base, budget)?;
    base.root_font = c0.font_size;
    base.root_line = c0.line_height;
    // 根の viewBox の変形と、子の表示の領域
    let (root_ctm, root_vp) = match (size, view_box_of(doc, input, root, budget)?) {
        (Some((w, h)), Some((vb, aspect))) => match view_box_transform(&vb, aspect, w, h) {
            Some(t) => (t, Some((vb.w, vb.h))),
            None => (Transform::IDENTITY, size),
        },
        _ => (Transform::IDENTITY, size),
    };
    let hidden = d.refs.hidden.as_slice();
    let is_hidden = |i: u32| hidden.get(i as usize).copied() == Some(1);
    let root_ok = root.ns == NS_SVG && root.local.of(input) == b"svg" && c0.display_none == 0 && !is_hidden(0);
    f(&WalkItem { node: 0, parent: NONE, computed: c0, ctm: Transform::IDENTITY, ctm_known: true, rendered: root_ok })?;
    let mut stack: MVec<'_, Frame> = MVec::new(budget);
    let (vp_w, vp_h, has_vp) = match root_vp {
        Some((w, h)) => (w, h, 1),
        None => (0.0, 0.0, 0),
    };
    stack.push(Frame {
        computed: c0,
        ctm_children: root_ctm,
        vp_w,
        vp_h,
        node: 0,
        child: root.first_child,
        has_vp,
        children_rendered: u8::from(root_ok),
        known: 1,
        pad: [0; 5],
    })?;
    while let Some(top) = stack.last().copied() {
        budget.spend(cost::OP_STEP)?;
        // 次の要素の子
        let mut c = top.child;
        let mut next = NONE;
        while c != NONE {
            budget.spend(cost::OP_STEP)?;
            let Some(x) = nodes.get(c as usize) else { break };
            let after = x.next_sibling;
            if x.kind == KIND_ELEMENT {
                next = c;
                c = after;
                break;
            }
            c = after;
        }
        if let Some(t) = stack.last_mut() {
            t.child = c;
        }
        if next == NONE {
            stack.pop();
            continue;
        }
        let Some(n) = nodes.get(next as usize) else { continue };
        let vp = (top.has_vp != 0).then_some((top.vp_w, top.vp_h));
        let b = Base { viewport: vp, ..base };
        let comp = compute(&top.computed, d.styles.of(next), &ids, &bufs, &b, budget)?;
        let local = n.local.of(input);
        let svg_ns = n.ns == NS_SVG;
        let t = &comp.transform;
        let det = t.a * t.d - t.b * t.c;
        let mut rendered = top.children_rendered != 0
            && svg_ns
            && known(local)
            && !not_rendered(local)
            && !is_hidden(next)
            && comp.display_none == 0
            && det != 0.0
            && det.is_finite();
        let mut ctm = top.ctm_children.then(&comp.transform);
        let mut known_ctm = top.known != 0 && comp.transform_known != 0;
        let mut child_ctm = ctm;
        let mut child_vp = vp;
        if svg_ns && local == b"use" {
            let x = attr_len(doc, input, n, b"x", &comp, &b, Axis::X, budget)?.unwrap_or(0.0);
            let y = attr_len(doc, input, n, b"y", &comp, &b, Axis::Y, budget)?.unwrap_or(0.0);
            ctm = ctm.then(&Transform { e: x, f: y, ..Transform::IDENTITY });
            child_ctm = ctm;
        } else if svg_ns && local == b"svg" {
            // 入れ子の svg：x・y・width・height（既定 100%）と viewBox
            let x = attr_len(doc, input, n, b"x", &comp, &b, Axis::X, budget)?.unwrap_or(0.0);
            let y = attr_len(doc, input, n, b"y", &comp, &b, Axis::Y, budget)?.unwrap_or(0.0);
            let full =
                |axis: Axis| resolve(Length::new(100.0, Unit::Percent), comp.font_size, comp.line_height, &b, axis);
            let w = match attr_len(doc, input, n, b"width", &comp, &b, Axis::X, budget)? {
                Some(w) => Some(w),
                None => full(Axis::X),
            };
            let h = match attr_len(doc, input, n, b"height", &comp, &b, Axis::Y, budget)? {
                Some(h) => Some(h),
                None => full(Axis::Y),
            };
            let to = ctm.then(&Transform { e: x, f: y, ..Transform::IDENTITY });
            match (w, h) {
                (Some(w), Some(h)) if w > 0.0 && h > 0.0 => match view_box_of(doc, input, n, budget)? {
                    Some((vb, aspect)) => match view_box_transform(&vb, aspect, w, h) {
                        Some(t) => {
                            child_ctm = to.then(&t);
                            child_vp = Some((vb.w, vb.h));
                        }
                        None => rendered = false,
                    },
                    None => {
                        child_ctm = to;
                        child_vp = Some((w, h));
                    }
                },
                (Some(_), Some(_)) => rendered = false,
                // 表示の領域が決まらない（% の基準がない）
                _ => {
                    child_ctm = to;
                    child_vp = None;
                    known_ctm = false;
                }
            }
        }
        f(&WalkItem { node: next, parent: top.node, computed: comp, ctm, ctm_known: known_ctm, rendered })?;
        let (vp_w, vp_h, has_vp) = match child_vp {
            Some((w, h)) => (w, h, 1),
            None => (0.0, 0.0, 0),
        };
        stack.push(Frame {
            computed: comp,
            ctm_children: child_ctm,
            vp_w,
            vp_h,
            node: next,
            child: n.first_child,
            has_vp,
            children_rendered: u8::from(rendered && renders_children(local)),
            known: u8::from(known_ctm),
            pad: [0; 5],
        })?;
    }
    Ok(())
}

/// 子を描く入れ物（図形・use・image の子は描かない。文字の要素の子は文字の中身）。
fn renders_children(local: &[u8]) -> bool {
    matches!(local, b"svg" | b"g" | b"a" | b"switch" | b"text" | b"tspan" | b"textPath")
}

/// 属性の長さ（単位のない数も受ける。`auto` と壊れた値は None）をユーザー単位に解く。
#[allow(clippy::too_many_arguments, reason = "長さの文脈をまとめて渡す")]
fn attr_len(
    doc: &XmlDoc,
    input: &[u8],
    n: &Node,
    name: &[u8],
    c: &Computed,
    base: &Base,
    axis: Axis,
    budget: &Budget<'_>,
) -> Result<Option<f64>, Error> {
    let budget = budget_ref(budget);
    let Some(v) = doc.attr(input, n, NS_NONE, name, budget)? else { return Ok(None) };
    let mut vals = Values::new(v, Span::new(0, v.len()), budget);
    let Some(l) = vals.length(true)? else { return Ok(None) };
    if !vals.at_end()? {
        return Ok(None);
    }
    Ok(resolve(l, c.font_size, c.line_height, base, axis))
}

/// 要素の `viewBox`（幅か高さが 0 以下のものは None）と `preserveAspectRatio`。
fn view_box_of(doc: &XmlDoc, input: &[u8], n: &Node, budget: &Budget<'_>) -> Result<Option<(ViewBox, Aspect)>, Error> {
    let budget = budget_ref(budget);
    let Some(v) = doc.attr(input, n, NS_NONE, b"viewBox", budget)? else { return Ok(None) };
    let Some(vb) = values::view_box(v, Span::new(0, v.len()), budget)?.filter(|vb| vb.w > 0.0 && vb.h > 0.0) else {
        return Ok(None);
    };
    let aspect = match doc.attr(input, n, NS_NONE, b"preserveAspectRatio", budget)? {
        Some(a) => values::aspect(a, Span::new(0, a.len()), budget)?.unwrap_or(Aspect::DEFAULT),
        None => Aspect::DEFAULT,
    };
    Ok(Some((vb, aspect)))
}

/// 使う塗り。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Used {
    None,
    Color(Color),
    /// 塗りの参照先（グラデーションか pattern の要素）
    Server(u32),
    /// 参照の輪で取り除いた塗り
    Cut,
    ContextFill,
    ContextStroke,
}

/// 要素 `node` の塗り `p`（`fill` なら `is_fill`）を、参照先と代わりで解く（`color` は要素の `color`）。url の参照先が
/// グラデーション・pattern でないか見つからなければ代わり（書いていなければ none。SVG 2 §13.2）。
pub(crate) fn used_paint(
    d: &Document<'_>,
    node: u32,
    p: &CPaint,
    is_fill: bool,
    color: Color,
    budget: &Budget<'_>,
) -> Result<Used, Error> {
    let budget = budget_ref(budget);
    let fallback = |p: &CPaint| {
        if p.fallback == FALLBACK_COLOR { Used::Color(p.used_color(color)) } else { Used::None }
    };
    match p.kind {
        PAINT_NONE => return Ok(Used::None),
        PAINT_CONTEXT_FILL => return Ok(Used::ContextFill),
        PAINT_CONTEXT_STROKE => return Ok(Used::ContextStroke),
        PAINT_URL => {}
        _ => return Ok(Used::Color(p.used_color(color))),
    }
    let doc = &d.xml;
    let bufs = Bufs { input: d.input, store: doc.store.as_slice(), sheet: d.sheet.text.as_slice() };
    let buf = bufs.of(p.src);
    let mut scratch: MVec<'_, u8> = MVec::new(budget);
    if p.escaped != 0 {
        for c in decode_string(buf, p.href, budget) {
            let (b, n) = crate::css::select::utf8_pub(c?);
            scratch.append_slice(b.get(..n).unwrap_or(&[]))?;
        }
    } else {
        scratch.append_slice(p.href.of(buf))?;
    }
    let v = scratch.as_slice().trim_ascii();
    let Some(id) = v.strip_prefix(b"#") else { return Ok(fallback(p)) };
    let Some(t) = d.ids.find(doc, d.input, id, budget)? else { return Ok(fallback(p)) };
    let server = doc.node(t).is_some_and(|x| {
        x.ns == NS_SVG && matches!(x.local.of(d.input), b"linearGradient" | b"radialGradient" | b"pattern")
    });
    if !server {
        return Ok(fallback(p));
    }
    let kind = if is_fill { EDGE_FILL } else { EDGE_STROKE };
    let cut = d.refs.of(node).iter().any(|e| e.kind == kind && e.target == t && e.removed != 0);
    Ok(if cut { Used::Cut } else { Used::Server(t) })
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
