//! 描く木を作るのに要るもの（architecture.md §3.1・FR-IMG-01・ADR-37 (3)）。
//!
//! - 文字：`text` 要素があるか（Kura と同じに要素の有無で決める）
//! - 画像：描く木に入りうる `<image>`・`<feImage>` の画像を、根から描く道（子と、中身を作る参照の辺）でたどって数える。
//!   描かない入れ物（defs・symbol・clipPath・mask・pattern・marker・グラデーション・filter・知らない要素など）の子は、参照の
//!   辺で入ったときだけたどる。`display: none` の部分木・条件が偽の要素・選ばれない switch の子は除く。clipPath の中の画像は
//!   描かないので数えない
//! - 画像の種類：`data:image/svg+xml` と `.svg` は SVG の画像（読めない）、ほかの `data:` と外の画像はラスター（呼び手の
//!   デコーダが要る）

use crate::budget::Budget;
use crate::cost;
use crate::css::sheet::Sheet;
use crate::doc::refs::{
    EDGE_CLIP, EDGE_HREF, EDGE_TEXT, EDGE_USE, HIDDEN_SELF, Refs, fits, not_rendered, renders_children,
};
use crate::error::Error;
use crate::mem::MVec;
use crate::style::cascade::{SRC_SHEET, SRC_STORE, Styles, WIDE_NONE};
use crate::style::props::lookup;
use crate::values::Values;
use crate::xml::{self, KIND_ELEMENT, NONE, NS_NONE, NS_SVG, NS_XLINK, XmlDoc};

/// 描く木を作るのに要るもの。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Needs {
    /// 文字がある（`text` 要素がある）
    pub text: bool,
    /// 描く木に入りうるラスターの画像がある（呼び手の画像のデコーダが要る）
    pub raster_images: bool,
    /// 描く木に入りうる SVG の画像がある（入れ子の SVG は読まない：木を作る段は `Unsupported`）
    pub svg_images: bool,
}

/// 知っている要素（知らない要素は子ごと描かない。FR-ST-02）。
pub(crate) fn known(local: &[u8]) -> bool {
    not_rendered(local)
        || matches!(
            local,
            b"svg"
                | b"g"
                | b"a"
                | b"switch"
                | b"use"
                | b"image"
                | b"text"
                | b"tspan"
                | b"textPath"
                | b"tref"
                | b"rect"
                | b"circle"
                | b"ellipse"
                | b"line"
                | b"polyline"
                | b"polygon"
                | b"path"
                | b"stop"
        )
        || local.starts_with(b"fe")
}

/// 文書の要るものを決める。
pub(crate) fn needs(
    doc: &XmlDoc,
    input: &[u8],
    sheet: &Sheet,
    styles: &Styles,
    refs: &Refs,
    budget: &Budget<'_>,
) -> Result<Needs, Error> {
    let budget = budget_ref(budget);
    let nodes = doc.nodes.as_slice();
    let mut out = Needs::default();
    // 文字は要素の有無
    for n in nodes {
        budget.spend(cost::OP_STEP)?;
        if n.kind == KIND_ELEMENT && n.ns == NS_SVG && n.local.of(input) == b"text" {
            out.text = true;
            break;
        }
    }
    // 根が svg の要素でなければ何も描かない
    if !nodes.first().is_some_and(|n| n.kind == KIND_ELEMENT && n.ns == NS_SVG && n.local.of(input) == b"svg") {
        return Ok(out);
    }
    let display = lookup(b"display", false);
    let content_visibility = lookup(b"content-visibility", false);
    // 節（XML の節と、受け継がない参照の並びの節）
    let total = refs.start.len().saturating_sub(1);
    // 中をたどった印（文脈ごと）：1 子の辺で文字の外・4 子の辺で文字の中・2 参照の辺。印は確かめを通ったときだけ付ける
    // （子の辺で隠れた要素も参照の辺で来たら見る。結果が文書の順に依らない。文書の 2 回目のレビュー）
    let mut seen: MVec<'_, u8> = MVec::with_capacity(budget, total)?;
    seen.resize_to(total, 0)?;
    // 行く節の積み上げ（節の番号 << 3 ＋ filter の中・文字の中・参照の辺で入ったかの印）
    let mut stack: MVec<'_, u64> = MVec::new(budget);
    stack.push(0)?;
    while let Some(item) = stack.pop() {
        let (i, via_ref) = ((item >> 3) as u32, item & 1 != 0);
        let (in_text, in_filter) = (item & 2 != 0, item & 4 != 0);
        budget.spend(cost::OP_STEP)?;
        let bit = if via_ref {
            2
        } else if in_text {
            4
        } else {
            1
        };
        let prev = seen.get(i as usize).copied().unwrap_or(0);
        if prev & bit != 0 {
            continue;
        }
        let Some(n) = nodes.get(i as usize) else {
            // 並びの節：参照先へ（参照の辺で入ったものとして）
            if let Some(x) = seen.get_mut(i as usize) {
                *x = prev | bit;
            }
            // 輪として取り除いた辺もたどる（M2 は取り除いた参照の先を 1 段だけ描き直す。ADR-86 (1)）
            for e in refs.of(i) {
                budget.spend(cost::OP_STEP)?;
                stack.push((u64::from(e.target) << 3) | 1)?;
            }
            continue;
        };
        if n.kind != KIND_ELEMENT {
            continue;
        }
        let local = n.local.of(input);
        // 参照の辺で入った描かない入れ物（pattern・marker・mask・symbol・filter など）には、display と自分の条件が効かない
        // （SVG 1.1・2。resvg と同じ。文書の 2 回目のレビュー）
        let referenced_container = via_ref && not_rendered(local);
        // 自分の条件が偽ならどの文脈でも描かない。switch が選ばないのは、参照の辺で入れば効かない
        let hidden = refs.hidden.as_slice().get(i as usize).copied().unwrap_or(0);
        // 根は display: none と偽の条件でも描く（Chrome と同じ。ADR-102 (1)。単位 3 の 5 回目のレビュー）
        let root = i == 0;
        let hidden = if referenced_container || root {
            0
        } else if via_ref {
            hidden & HIDDEN_SELF
        } else {
            hidden
        };
        if n.ns != NS_SVG || hidden != 0 || !known(local) {
            continue;
        }
        // 親の文脈で描かれうるか（参照の辺で入った要素は、その種類の中身として見る。filter の中は fe… だけ）
        if !via_ref && in_filter && !local.starts_with(b"fe") {
            continue;
        }
        if !via_ref && !in_filter && !fits(local, in_text) {
            continue;
        }
        // display: none の部分木は描かない（値は walk と同じ読み方：escape を解いて大文字小文字を問わない）
        if !referenced_container && !root {
            let spec = styles.of(i).iter().find(|s| Some(s.prop) == display).copied();
            if let Some(s) = spec
                && s.wide == WIDE_NONE
            {
                let buf = match s.src {
                    SRC_SHEET => sheet.text.as_slice(),
                    SRC_STORE => doc.store.as_slice(),
                    _ => input,
                };
                if Values::new(buf, s.value, budget).eat_ident(b"none")? {
                    continue;
                }
            }
        }
        // content-visibility: hidden の入れ物・use・参照される symbol の中身は描かない（walk と同じ。単位 3 の 6・7 回目の
        // レビュー）
        let mut cv_hidden = false;
        {
            let spec = styles.of(i).iter().find(|s| Some(s.prop) == content_visibility).copied();
            if let Some(s) = spec
                && s.wide == WIDE_NONE
            {
                let buf = match s.src {
                    SRC_SHEET => sheet.text.as_slice(),
                    SRC_STORE => doc.store.as_slice(),
                    _ => input,
                };
                cv_hidden = Values::new(buf, s.value, budget).eat_ident(b"hidden")?;
            }
        }
        if let Some(x) = seen.get_mut(i as usize) {
            *x = prev | bit;
        }
        if matches!(local, b"image" | b"feImage") {
            classify(doc, input, n, local, &mut out, budget)?;
        }
        // 子（描かない入れ物は参照の辺で入ったときだけ。clipPath の中の画像は描かないので入らない。図形などの子は描かない）
        let into = if not_rendered(local) {
            via_ref && local != b"clipPath" && !(local == b"symbol" && cv_hidden)
        } else {
            renders_children(local) && !cv_hidden
        };
        if into {
            let child_text = in_text || local == b"text";
            let child_filter = local == b"filter";
            let flags = (u64::from(child_filter) << 2) | (u64::from(child_text) << 1);
            let mut c = n.first_child;
            while c != NONE {
                budget.spend(cost::OP_STEP)?;
                stack.push((u64::from(c) << 3) | flags)?;
                c = nodes.get(c as usize).map_or(NONE, |x| x.next_sibling);
            }
        }
        // 中身を作る参照の辺（clip-path の中身は画像を描かない）。輪として取り除いた辺もたどる（ADR-86 (1)）
        for e in refs.of(i) {
            budget.spend(cost::OP_STEP)?;
            if e.kind == EDGE_HREF || e.kind == EDGE_TEXT || e.kind == EDGE_CLIP || (cv_hidden && e.kind == EDGE_USE) {
                continue;
            }
            stack.push((u64::from(e.target) << 3) | 1)?;
        }
    }
    Ok(out)
}

/// 画像の種類を決める（`#` の参照は要素への辺で数えるので、ここでは数えない）。
fn classify(
    doc: &XmlDoc,
    input: &[u8],
    n: &xml::Node,
    local: &[u8],
    out: &mut Needs,
    budget: &Budget<'_>,
) -> Result<(), Error> {
    let a = match doc.attr(input, n, NS_NONE, b"href", budget)? {
        Some(v) => Some(v),
        None => doc.attr(input, n, NS_XLINK, b"href", budget)?,
    };
    let Some(v) = a else { return Ok(()) };
    let v = super::trim_ws(v, budget)?;
    if v.is_empty() || (local == b"feImage" && v.starts_with(b"#")) {
        return Ok(());
    }
    let lower_starts = |p: &[u8]| v.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p));
    if lower_starts(b"data:image/svg+xml") {
        out.svg_images = true;
    } else if lower_starts(b"data:") {
        out.raster_images = true;
    } else {
        // 外の画像：拡張子 .svg・.svgz は SVG の画像（`?`・`#` の前まで。読むバイトを払う）
        let mut meter = crate::xml::lex::ScanMeter::new(0, v.len());
        let mut end = v.len();
        for (k, &c) in v.iter().enumerate() {
            meter.pay(budget, k)?;
            if c == b'?' || c == b'#' {
                end = k;
                break;
            }
        }
        let path = v.get(..end).unwrap_or(&[]);
        let svg = path.len() >= 4
            && (path.get(path.len().saturating_sub(4)..).is_some_and(|e| e.eq_ignore_ascii_case(b".svg"))
                || path.get(path.len().saturating_sub(5)..).is_some_and(|e| e.eq_ignore_ascii_case(b".svgz")));
        if svg {
            out.svg_images = true;
        } else {
            out.raster_images = true;
        }
    }
    Ok(())
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
