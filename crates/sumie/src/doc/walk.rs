//! 文書の木をたどって、要素ごとの計算した値と根からの変形を求める（architecture.md §4.2・§4.3）。木を作る段（M2）は同じ
//! 求め方に use の展開を足して使う。M1 は照合の道具（`sumie-tools values`）と試験が使う。
//!
//! - 行きがけの順に、明示の積み上げで（深さごとの計算した値と、子に掛ける変形・表示の領域を積む）
//! - 根からの変形：根の viewBox の変形 × 祖先の変形 × 要素の変形。use は自分の変形のあとに x・y の平行移動を掛け、入れ子の
//!   svg は子に x・y の平行移動と viewBox の変形を掛ける（svg 自身の変形には含めない）。根の svg 自身の変形は viewBox の
//!   変形の外側に掛ける
//! - 表示の領域（% の基準）：根は文書の大きさ（viewBox があればその幅・高さ）。入れ子の svg は `width`・`height`（既定
//!   100%。属性だけを読む）と viewBox
//! - 描く木に入るか：SVG の名前空間の知っている要素で、子を描く入れ物（svg・g・a・switch と文字の要素）の子であり、内容の
//!   モデルに合い（文字の中は文字の中身の要素と a だけ、文字の外では tspan・stop・fe… を描かない。`refs::fits`）、描かない
//!   入れ物（defs など）の中でなく、条件が偽・switch で選ばれない要素の部分木（refs の `hidden`）でなく、`display: none` の
//!   部分木の中でない。変形が逆にできない（行列式が 0）要素と、幅か高さが 0 以下の入れ子の svg と、幅か高さが 0 の viewBox
//!   の svg（根なら文書の全体）は、部分木ごと描かない（CSS Transforms 1 §6・SVG 2 §8.2。ADR-78 (7)）

use crate::budget::Budget;
use crate::cost;
use crate::css::lex::decode_string;
use crate::doc::Document;
use crate::doc::needs::known;
use crate::doc::refs::{EDGE_FILL, EDGE_STROKE, fits, not_rendered, renders_children};
use crate::error::Error;
use crate::mem::{MVec, Plain};
use crate::style::computed::{
    Axis, Base, Bufs, CPaint, Computed, FALLBACK_COLOR, G_X, G_Y, PAINT_CONTEXT_FILL, PAINT_CONTEXT_STROKE, PAINT_NONE,
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
    /// 要素の長さの % の基準（最も近い表示の領域。決まらなければ None）
    pub(crate) viewport: Option<(f64, f64)>,
    /// 子を表示の領域で切る（入れ子の svg・use の中の symbol で、`overflow` が visible・auto でないもの。M2 は描かない。
    /// ADR-89 (1)）
    pub(crate) clips: bool,
    /// 最も近い use の文脈の塗り（use の中身でなければ None）
    pub(crate) context: Option<UseContext>,
}

/// たどり方。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WalkOptions {
    /// 木を作る段のたどり方：use の参照先を use の文脈で展開し（参照先が積み上げにあれば輪：展開しない。輪として取り除いた
    /// 参照の先は 1 段だけ描き直し、その中でまた取り除いた参照に当たったら展開しない。ADR-86 (1)）、描く木に入らない要素の
    /// 部分木に降りない（要素そのものは渡す）
    pub(crate) build: bool,
}

/// use の文脈の塗り（use の中身の `context-fill`・`context-stroke` は use の塗りを使う。SVG 2 §13.3）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UseContext {
    pub(crate) fill: CPaint,
    pub(crate) stroke: CPaint,
    /// それぞれの currentColor を解く色
    pub(crate) fill_color: Color,
    pub(crate) stroke_color: Color,
    /// それぞれの url の取り除いた辺を見る節（塗りを書いた use）
    pub(crate) fill_node: u32,
    pub(crate) stroke_node: u32,
}

impl Plain for UseContext {}

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
    /// 子の use の文脈（`Walker::ctx` の番号。なければ NONE）。use の参照先の段では自分が足した番号
    ctx: u32,
    has_vp: u8,
    /// 子が描く木に入りうる
    children_rendered: u8,
    known: u8,
    /// 子が文字の中（text の部分木）
    in_text: u8,
    /// use の参照先を 1 つだけ見る段（`child` だけを見て、兄弟を見ない。`node` は use）
    single: u8,
    /// 輪として取り除いた参照の先を描き直している中
    redraw: u8,
    pad: [u8; 2],
}

impl Plain for Frame {}

/// たどる段の積み上げの 1 段と use の文脈の大きさ（木を作る段の見積もり）。
pub(crate) const FRAME_BYTES: usize = size_of::<Frame>();
pub(crate) const CONTEXT_BYTES: usize = size_of::<UseContext>();

/// 文書を行きがけの順にたどり、要素ごとに `f` を呼ぶ（描く木に入らない要素も、計算した値を求めて渡す。use は展開しない）。
pub(crate) fn walk(
    d: &Document<'_>,
    budget: &Budget<'_>,
    f: &mut dyn FnMut(&WalkItem) -> Result<(), Error>,
) -> Result<(), Error> {
    walk_with(d, WalkOptions::default(), budget, f)
}

/// [`walk`] に、たどり方を選べるようにしたもの。
pub(crate) fn walk_with(
    d: &Document<'_>,
    opts: WalkOptions,
    budget: &Budget<'_>,
    f: &mut dyn FnMut(&WalkItem) -> Result<(), Error>,
) -> Result<(), Error> {
    let budget = budget_ref(budget);
    let Some(mut w) = Walker::start(d, opts, budget, f)? else { return Ok(()) };
    while w.step(f)? {}
    Ok(())
}

/// たどる途中の状態。
struct Walker<'a, 'd, 'b> {
    d: &'a Document<'d>,
    opts: WalkOptions,
    budget: &'b Budget<'b>,
    ids: PropIds,
    bufs: Bufs<'a>,
    base: Base,
    stack: MVec<'b, Frame>,
    /// 要素ごとの、積み上げにある数（use の輪を見つける。木を作るたどり方だけ）
    on_stack: MVec<'b, u32>,
    /// use の文脈の積み上げ
    ctx: MVec<'b, UseContext>,
    overflow_id: Option<crate::style::props::PropertyId>,
}

impl<'a, 'd, 'b> Walker<'a, 'd, 'b> {
    /// 根を計算して `f` に渡し、根の段を積む（根が要素でなければ None）。
    fn start(
        d: &'a Document<'d>,
        opts: WalkOptions,
        budget: &'b Budget<'b>,
        f: &mut dyn FnMut(&WalkItem) -> Result<(), Error>,
    ) -> Result<Option<Walker<'a, 'd, 'b>>, Error> {
        let doc = &d.xml;
        let input = d.input;
        let nodes = doc.nodes.as_slice();
        let Some(root) = nodes.first().filter(|n| n.kind == KIND_ELEMENT) else { return Ok(None) };
        let ids = PropIds::new();
        let bufs = Bufs { input, store: doc.store.as_slice(), sheet: d.sheet.text.as_slice() };
        let size = d.size.known().map(|s| (s.width, s.height));
        let initial = Computed::initial(d.font_size);
        let mut base = Base {
            default_font: d.font_size,
            root_font: d.font_size,
            root_line: initial.line_height,
            viewport: size,
            // vw などは `@media` と同じ文書の大きさ（既定の文字の大きさで解いたもの。ADR-74 (7)）
            doc: d.media_size,
            outermost: true,
        };
        budget.spend(cost::OP_STEP)?;
        let c0 = compute(&initial, d.styles.of(0), &ids, &bufs, &base, budget)?;
        base.root_font = c0.font_size;
        base.root_line = c0.line_height;
        base.outermost = false;
        // 根の viewBox の変形と、子の表示の領域
        // 幅か高さが 0 の viewBox の根は描かない（大きさは viewBox がないものとして決める。ADR-78 (7)）
        let mut root_drawn = true;
        let (root_ctm, root_vp) = match (size, view_box_of(doc, input, root, budget)?) {
            (Some((w, h)), Some((vb, aspect))) => match view_box_transform(&vb, aspect, w, h) {
                Some(t) => (t, Some((vb.w, vb.h))),
                None => {
                    root_drawn = false;
                    (Transform::IDENTITY, size)
                }
            },
            (None, Some((vb, _))) if vb.w <= 0.0 || vb.h <= 0.0 => {
                root_drawn = false;
                (Transform::IDENTITY, size)
            }
            _ => (Transform::IDENTITY, size),
        };
        let hidden = d.refs.hidden.as_slice().first().copied().unwrap_or(0) != 0;
        let root_ok =
            root.ns == NS_SVG && root.local.of(input) == b"svg" && c0.display_none == 0 && !hidden && root_drawn;
        // 根の svg の変形は viewBox の変形の外側に掛ける（usvg・Chrome と同じ。文書の 1 回目のレビュー）。原点の既定は表示の
        // 領域の真ん中（Chrome と同じ。usvg は原点。ADR-85 (1)）
        let root_own = c0.transform;
        let root_known = c0.transform_known != 0;
        let root_child = root_own.then(&root_ctm);
        let root_ok = root_ok && root_child.finite() && {
            let det = root_own.a * root_own.d - root_own.b * root_own.c;
            det != 0.0 && det.is_finite()
        };
        let root_child = if root_child.finite() { root_child } else { Transform::IDENTITY };
        f(&WalkItem {
            node: 0,
            parent: NONE,
            computed: c0,
            ctm: root_own,
            ctm_known: root_known,
            rendered: root_ok,
            viewport: size,
            clips: false,
            context: None,
        })?;
        let mut w = Walker {
            d,
            opts,
            budget,
            ids,
            bufs,
            base,
            stack: MVec::new(budget),
            on_stack: MVec::new(budget),
            ctx: MVec::new(budget),
            overflow_id: crate::style::props::lookup(b"overflow", false),
        };
        if opts.build {
            if !root_ok {
                return Ok(None);
            }
            w.on_stack.resize_to(nodes.len(), 0)?;
            if let Some(x) = w.on_stack.get_mut(0) {
                *x = 1;
            }
        }
        let (vp_w, vp_h, has_vp) = match root_vp {
            Some((w, h)) => (w, h, 1),
            None => (0.0, 0.0, 0),
        };
        w.stack.push(Frame {
            computed: c0,
            ctm_children: root_child,
            vp_w,
            vp_h,
            node: 0,
            child: root.first_child,
            ctx: NONE,
            has_vp,
            children_rendered: u8::from(root_ok),
            known: u8::from(root_known),
            in_text: 0,
            single: 0,
            redraw: 0,
            pad: [0; 2],
        })?;
        Ok(Some(w))
    }

    /// 次の要素を 1 つ見る（積み上げが空なら偽）。
    fn step(&mut self, f: &mut dyn FnMut(&WalkItem) -> Result<(), Error>) -> Result<bool, Error> {
        let budget = self.budget;
        let d = self.d;
        let doc = &d.xml;
        let input = d.input;
        let nodes = doc.nodes.as_slice();
        let Some(top) = self.stack.last().copied() else { return Ok(false) };
        budget.spend(cost::OP_STEP)?;
        // 次の要素の子（use の参照先の段は参照先だけ）
        let mut c = top.child;
        let mut next = NONE;
        if top.single != 0 {
            next = c;
            c = NONE;
        } else {
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
        }
        if let Some(t) = self.stack.last_mut() {
            t.child = c;
        }
        if next == NONE {
            if let Some(done) = self.stack.pop() {
                if done.single == 0 {
                    if let Some(x) = self.on_stack.get_mut(done.node as usize) {
                        *x = x.saturating_sub(1);
                    }
                } else {
                    self.ctx.truncate(done.ctx as usize);
                }
            }
            return Ok(true);
        }
        let Some(n) = nodes.get(next as usize) else { return Ok(true) };
        let hidden = d.refs.hidden.as_slice().get(next as usize).copied().unwrap_or(0) != 0;
        let vp = (top.has_vp != 0).then_some((top.vp_w, top.vp_h));
        let b = Base { viewport: vp, ..self.base };
        let comp = compute(&top.computed, d.styles.of(next), &self.ids, &self.bufs, &b, budget)?;
        let local = n.local.of(input);
        let svg_ns = n.ns == NS_SVG;
        let t = &comp.transform;
        let det = t.a * t.d - t.b * t.c;
        // use の参照先の symbol は表示の領域として描く（SVG 2 §5.5）
        let symbol_instance = top.single != 0 && svg_ns && local == b"symbol";
        let mut rendered = top.children_rendered != 0
            && svg_ns
            && known(local)
            && (!not_rendered(local) || symbol_instance)
            && fits(local, top.in_text != 0)
            && !hidden
            && comp.display_none == 0
            && det != 0.0
            && det.is_finite();
        let mut ctm = top.ctm_children.then(&comp.transform);
        let mut known_ctm = top.known != 0 && comp.transform_known != 0;
        let mut child_ctm = ctm;
        let mut child_vp = vp;
        // 属性の長さ（書いていない・壊れた値は 0。基準がなく解けない % は 0 にして「解けない」印を付ける）
        let mut len0 = |name: &[u8], axis: Axis| -> Result<f64, Error> {
            Ok(match attr_len(doc, input, n, name, &comp, &b, axis, budget)? {
                Some(Some(v)) => v,
                Some(None) => {
                    known_ctm = false;
                    0.0
                }
                None => 0.0,
            })
        };
        if svg_ns && local == b"use" {
            // 木を作るたどり方は x・y を CSS のプロパティ（計算した値）として読む（ADR-84 (6)。M1 の照合の道具は属性だけ）
            let (x, y) = if self.opts.build {
                let gx = comp.geo.used(G_X, vp.map(|(w, _)| w));
                let gy = comp.geo.used(G_Y, vp.map(|(_, h)| h));
                // 基準のない % は 0 にして「解けない」印を付ける
                if gx.is_none() || gy.is_none() {
                    known_ctm = false;
                }
                (gx.unwrap_or(0.0), gy.unwrap_or(0.0))
            } else {
                (len0(b"x", Axis::X)?, len0(b"y", Axis::Y)?)
            };
            ctm = ctm.then(&Transform { e: x, f: y, ..Transform::IDENTITY });
            child_ctm = ctm;
        } else if svg_ns && (local == b"svg" || symbol_instance) {
            // 入れ子の svg：x・y・width・height（既定 100%）と viewBox。use の中の symbol は use の width・height が勝つ
            let x = len0(b"x", Axis::X)?;
            let y = len0(b"y", Axis::Y)?;
            let full =
                |axis: Axis| resolve(Length::new(100.0, Unit::Percent), comp.font_size, comp.line_height, &b, axis);
            let use_node = if symbol_instance { nodes.get(top.node as usize) } else { None };
            let size_of = |name: &[u8], axis: Axis| -> Result<Option<f64>, Error> {
                if let Some(u) = use_node
                    && let Some(v) = attr_len(doc, input, u, name, &comp, &b, axis, budget)?
                {
                    return Ok(v);
                }
                Ok(match attr_len(doc, input, n, name, &comp, &b, axis, budget)? {
                    Some(v) => v,
                    None => full(axis),
                })
            };
            let w = size_of(b"width", Axis::X)?;
            let h = size_of(b"height", Axis::Y)?;
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
        // 根からの変形があふれたら（大きな変形の入れ子）、描かず、解けない印にして有限に保つ（文書の 4 回目のレビュー）
        if !ctm.finite() || !child_ctm.finite() {
            ctm = Transform::IDENTITY;
            child_ctm = Transform::IDENTITY;
            known_ctm = false;
            rendered = false;
        }
        // 表示の領域で切るか（入れ子の svg・use の中の symbol。overflow が visible・auto なら切らない）
        let clips = svg_ns
            && (local == b"svg" || symbol_instance)
            && !overflow_visible(d, next, self.overflow_id, &self.bufs, budget)?;
        let context = if top.ctx == NONE { None } else { self.ctx.get(top.ctx as usize).copied() };
        f(&WalkItem {
            node: next,
            parent: top.node,
            computed: comp,
            ctm,
            ctm_known: known_ctm,
            rendered,
            viewport: vp,
            clips,
            context,
        })?;
        // 木を作るたどり方は、描く木に入らない要素の部分木に降りない
        if self.opts.build && !rendered {
            return Ok(true);
        }
        let (vp_w, vp_h, has_vp) = match child_vp {
            Some((w, h)) => (w, h, 1),
            None => (0.0, 0.0, 0),
        };
        let frame = Frame {
            computed: comp,
            ctm_children: child_ctm,
            vp_w,
            vp_h,
            node: next,
            child: n.first_child,
            ctx: top.ctx,
            has_vp,
            // use の中の symbol は子を描く（SVG 2 §5.5。ほかの文脈の symbol は描く木に入らない）
            children_rendered: u8::from(rendered && (renders_children(local) || symbol_instance)),
            known: u8::from(known_ctm),
            in_text: u8::from(top.in_text != 0 || (svg_ns && local == b"text")),
            single: 0,
            redraw: top.redraw,
            pad: [0; 2],
        };
        self.stack.push(frame)?;
        if let Some(x) = self.on_stack.get_mut(next as usize) {
            *x = x.saturating_add(1);
        }
        if self.opts.build && svg_ns && local == b"use" {
            self.expand_use(&frame, top.redraw, context)?;
        }
        Ok(true)
    }

    /// use の参照先を use の文脈で展開する段を積む（参照先が積み上げにあれば輪：積まない。取り除いた参照は、描き直しの
    /// 中でなければ 1 段だけ描き直す）。
    fn expand_use(&mut self, frame: &Frame, redraw: u8, outer: Option<UseContext>) -> Result<(), Error> {
        let budget = self.budget;
        let nodes = self.d.xml.nodes.as_slice();
        let mut edge = None;
        for e in self.d.refs.of(frame.node) {
            budget.spend(cost::OP_STEP)?;
            if e.kind == crate::doc::refs::EDGE_USE {
                edge = Some(*e);
                break;
            }
        }
        let Some(e) = edge else { return Ok(()) };
        let removed = e.removed != 0;
        if !nodes.get(e.target as usize).is_some_and(|x| x.kind == KIND_ELEMENT)
            || self.on_stack.get(e.target as usize).copied() != Some(0)
            || (removed && redraw != 0)
        {
            return Ok(());
        }
        // use の中身の context-fill・context-stroke が使う塗り（use の塗りがまた文脈の塗りなら、外の use の文脈から）
        let c = &frame.computed;
        let pick = |p: CPaint| -> (CPaint, Color, u32) {
            match (p.kind, outer) {
                (PAINT_CONTEXT_FILL, Some(o)) => (o.fill, o.fill_color, o.fill_node),
                (PAINT_CONTEXT_STROKE, Some(o)) => (o.stroke, o.stroke_color, o.stroke_node),
                (PAINT_CONTEXT_FILL | PAINT_CONTEXT_STROKE, None) => (CPaint::NONE, c.color, frame.node),
                _ => (p, c.color, frame.node),
            }
        };
        let (fill, fill_color, fill_node) = pick(c.fill);
        let (stroke, stroke_color, stroke_node) = pick(c.stroke);
        let at = self.ctx.len();
        self.ctx.push(UseContext { fill, stroke, fill_color, stroke_color, fill_node, stroke_node })?;
        self.stack.push(Frame {
            child: e.target,
            ctx: at as u32,
            children_rendered: 1,
            single: 1,
            redraw: redraw | u8::from(removed),
            ..*frame
        })?;
        Ok(())
    }
}

/// 要素の `overflow` が visible か auto か（受け継がない。書いていなければ偽：UA の規則で入れ子の svg と symbol は hidden）。
fn overflow_visible(
    d: &Document<'_>,
    node: u32,
    id: Option<crate::style::props::PropertyId>,
    bufs: &Bufs<'_>,
    budget: &Budget<'_>,
) -> Result<bool, Error> {
    let budget = budget_ref(budget);
    for s in d.styles.of(node) {
        budget.spend(cost::OP_STEP)?;
        if Some(s.prop) != id || s.wide != crate::style::cascade::WIDE_NONE {
            continue;
        }
        let mut v = Values::new(bufs.of(s.src), s.value, budget);
        return Ok(v.eat_ident(b"visible")? || v.eat_ident(b"auto")?);
    }
    Ok(false)
}

/// 属性の長さ（単位のない数も受ける）をユーザー単位に解く。書いていない・`auto`・壊れた値は None、基準がなく解けない
/// （表示の領域のない % など）なら `Some(None)`。
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
) -> Result<Option<Option<f64>>, Error> {
    let budget = budget_ref(budget);
    let Some(v) = doc.attr(input, n, NS_NONE, name, budget)? else { return Ok(None) };
    let mut vals = Values::new(v, Span::new(0, v.len()), budget);
    let Some(l) = vals.length(true)? else { return Ok(None) };
    if !vals.at_end()? {
        return Ok(None);
    }
    Ok(Some(resolve(l, c.font_size, c.line_height, base, axis)))
}

/// 要素の `viewBox`（負の幅・高さの壊れたものは None。幅か高さが 0 のものは返し、呼び手が描かない）と
/// `preserveAspectRatio`。
fn view_box_of(doc: &XmlDoc, input: &[u8], n: &Node, budget: &Budget<'_>) -> Result<Option<(ViewBox, Aspect)>, Error> {
    let budget = budget_ref(budget);
    let Some(v) = doc.attr(input, n, NS_NONE, b"viewBox", budget)? else { return Ok(None) };
    let Some(vb) = values::view_box(v, Span::new(0, v.len()), budget)? else {
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
    let v = super::trim_ws(scratch.as_slice(), budget)?;
    let Some(id) = v.strip_prefix(b"#") else { return Ok(fallback(p)) };
    let Some(t) = d.ids.find(doc, d.input, id, budget)? else { return Ok(fallback(p)) };
    let server = doc.node(t).is_some_and(|x| {
        x.ns == NS_SVG && matches!(x.local.of(d.input), b"linearGradient" | b"radialGradient" | b"pattern")
    });
    if !server {
        return Ok(fallback(p));
    }
    let kind = if is_fill { EDGE_FILL } else { EDGE_STROKE };
    let mut cut = false;
    for e in d.refs.of(node) {
        budget.spend(cost::OP_STEP)?;
        if e.kind == kind && e.target == t && e.removed != 0 {
            cut = true;
            break;
        }
    }
    Ok(if cut { Used::Cut } else { Used::Server(t) })
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
