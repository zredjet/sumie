//! カスケード（FR-ST-09・ADR-66 (3)・architecture.md §4.2）。要素ごとに、プロパティごとの指定の値（勝った宣言の値の
//! 字句の範囲）を決める。
//!
//! 弱い順：プレゼンテーション属性 ＜ 規則（層の順位・詳細度・規則の順・宣言の順） ＜ `style` 属性 ＜ 規則の `!important`
//! （層は逆の順。層のない規則がいちばん弱い） ＜ `style` 属性の `!important`。壊れた値の宣言は勝たない（無視して警告）。
//! 規則の宣言は、照合の前に 1 度だけ確かめる。値は計算した値を求めるとき（木を作る段）に文脈で読み直す。
//!
//! 幾何のプロパティ（x・y・width・height・r・rx・ry・cx・cy・d）のプレゼンテーション属性は、それを幾何に使う要素でだけ
//! 読む（`text` の `x` などは別の意味の属性）。SVG の名前空間の要素だけに指定の値を持つ。

use crate::budget::Budget;
use crate::cost;
use crate::css::decl::{Decl, parse_declarations};
use crate::css::lex::Lexer;
use crate::css::matcher::{MatchSink, match_rules};
use crate::css::select::NONE;
use crate::css::sheet::{COND_MEDIA, Sheet};
use crate::error::Error;
use crate::mem::{Frozen, MVec, Plain};
use crate::report::WarningKind;
use crate::style::cond::{Env, media, supports};
use crate::style::props::{PROPS, PropertyId, lookup, prop};
use crate::style::valid::{Wide, valid, wide};
use crate::xml::lex::Span;
use crate::xml::{FLAG_IN_STORE, NS_NONE, NS_SVG, Node, XmlDoc};

/// 値の置き場。
pub(crate) const SRC_INPUT: u8 = 0;
pub(crate) const SRC_STORE: u8 = 1;
pub(crate) const SRC_SHEET: u8 = 2;

/// どこから来た宣言か（`transform` の書き方が違う）。
pub(crate) const ORIGIN_ATTR: u8 = 0;
pub(crate) const ORIGIN_RULE: u8 = 1;
pub(crate) const ORIGIN_STYLE: u8 = 2;

/// 指定の値（勝った宣言）。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Specified {
    pub(crate) node: u32,
    /// 値の字句の範囲（`src` の置き場の中）
    pub(crate) value: Span,
    pub(crate) prop: u16,
    pub(crate) src: u8,
    pub(crate) origin: u8,
    /// CSS の全体の言葉（0 なし、1 inherit、2 initial、3 unset）
    pub(crate) wide: u8,
    pub(crate) pad: [u8; 3],
}

impl Plain for Specified {}

/// カスケードの結果。
#[derive(Debug)]
pub(crate) struct Styles {
    /// 節の番号の順、同じ節はプロパティの番号の順
    pub(crate) specs: Frozen<Specified>,
    /// 節ごとの `specs` の始まり（節の数 ＋ 1）
    pub(crate) index: Frozen<u32>,
    /// 警告（種類と位置）
    pub(crate) warnings: Frozen<Warn>,
}

impl Styles {
    pub(crate) fn bytes(&self) -> u64 {
        self.specs.bytes().saturating_add(self.index.bytes()).saturating_add(self.warnings.bytes())
    }

    /// 節の指定の値。
    pub(crate) fn of(&self, node: u32) -> &[Specified] {
        let ix = self.index.as_slice();
        let a = ix.get(node as usize).copied().unwrap_or(0) as usize;
        let b = ix.get((node as usize).saturating_add(1)).copied().unwrap_or(0) as usize;
        self.specs.as_slice().get(a..b).unwrap_or(&[])
    }
}

/// 警告。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Warn {
    pub(crate) pos: u32,
    pub(crate) kind: WarningKind,
    pub(crate) pad: u16,
}

impl Plain for Warn {}

/// 規則の宣言を前に確かめた結果。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DeclInfo {
    prop: u16,
    wide: u8,
    ok: u8,
}

impl Plain for DeclInfo {}

/// 勝ち負けを決める鍵（大きい方が勝つ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
struct Key {
    tier: u8,
    layer: u32,
    spec: u32,
    order: u32,
    idx: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Slot {
    set: u8,
    pad: [u8; 3],
    key_tier: u32,
    key_layer: u32,
    key_spec: u32,
    key_order: u32,
    key_idx: u32,
    spec: Specified,
}

impl Plain for Slot {}

impl Slot {
    fn key(&self) -> Key {
        Key {
            tier: self.key_tier as u8,
            layer: self.key_layer,
            spec: self.key_spec,
            order: self.key_order,
            idx: self.key_idx,
        }
    }
}

const fn wide_code(w: Option<Wide>) -> u8 {
    match w {
        None => 0,
        Some(Wide::Inherit) => 1,
        Some(Wide::Initial) => 2,
        Some(Wide::Unset) => 3,
    }
}

/// 幾何のプロパティのプレゼンテーション属性を、この要素で読むか。
fn geometry_ok(local: &[u8], name: &[u8]) -> bool {
    match name {
        b"x" | b"y" => matches!(local, b"rect" | b"image" | b"foreignObject" | b"svg" | b"use" | b"symbol"),
        b"width" | b"height" => matches!(local, b"rect" | b"image" | b"foreignObject" | b"svg" | b"use" | b"symbol"),
        b"r" => local == b"circle",
        b"cx" | b"cy" => matches!(local, b"circle" | b"ellipse"),
        b"rx" | b"ry" => matches!(local, b"rect" | b"ellipse"),
        b"d" => local == b"path",
        _ => true,
    }
}

/// カスケードする。`env` は条件を照らす環境（明暗と文書の大きさ）。
pub(crate) fn cascade(
    doc: &XmlDoc,
    input: &[u8],
    sheet: &Sheet,
    env: &Env,
    budget: &Budget<'_>,
) -> Result<Styles, Error> {
    let budget = budget_ref(budget);
    let text = sheet.text.as_slice();
    // 条件を照らし、規則ごとに当てるか
    let nconds = sheet.conds.len();
    let mut cond_ok: MVec<'_, u8> = MVec::with_capacity(budget, nconds)?;
    for c in sheet.conds.as_slice() {
        budget.spend(cost::OP_STEP)?;
        let ok = if c.kind == COND_MEDIA { media(text, c.text, env, budget)? } else { supports(text, c.text, budget)? };
        cond_ok.push(u8::from(ok))?;
    }
    let mut active: MVec<'_, u8> = MVec::with_capacity(budget, sheet.rules.len())?;
    for r in sheet.rules.as_slice() {
        budget.spend(cost::OP_STEP)?;
        let mut c = r.cond;
        let mut ok = true;
        while c != NONE && ok {
            budget.spend(cost::OP_STEP)?;
            ok = cond_ok.get(c as usize).copied() == Some(1);
            c = sheet.conds.as_slice().get(c as usize).map_or(NONE, |x| x.parent);
        }
        active.push(u8::from(ok))?;
    }
    // 規則の宣言を 1 度だけ確かめる（宣言は規則の順に続いて並ぶ。規則の外の宣言はない）
    let mut warns: MVec<'_, Warn> = MVec::new(budget);
    let mut infos: MVec<'_, DeclInfo> = MVec::with_capacity(budget, sheet.decls.len())?;
    infos.resize_to(sheet.decls.len(), DeclInfo { prop: u16::MAX, wide: 0, ok: 0 })?;
    for r in sheet.rules.as_slice() {
        for k in r.decls.start..r.decls.end {
            budget.spend(cost::OP_STEP)?;
            let Some(d) = sheet.decls.as_slice().get(k as usize) else { continue };
            let info = check_decl(text, d, budget)?;
            if info.prop != u16::MAX && info.ok == 0 {
                warns.push(Warn { pos: r.pos, kind: WarningKind::InvalidValue, pad: 0 })?;
            }
            if let Some(x) = infos.get_mut(k as usize) {
                *x = info;
            }
        }
    }
    // 照合しながら要素ごとに決める
    let nodes = doc.nodes.len();
    let mut out = Out {
        doc,
        input,
        sheet,
        infos: infos.as_slice(),
        budget,
        slots: MVec::with_capacity(budget, PROPS.len())?,
        cur: NONE,
        specs: MVec::new(budget),
        index: MVec::with_capacity(budget, nodes.saturating_add(1))?,
        warns,
    };
    out.slots.resize_to(PROPS.len(), Slot::empty())?;
    match_rules(doc, input, sheet, active.as_slice(), budget, &mut out)?;
    out.finish_element()?;
    // 残りの節の始まり（終わりの印を含めて節の数 ＋ 1）
    let n = out.specs.len() as u32;
    while out.index.len() <= nodes {
        budget.spend(cost::OP_STEP)?;
        out.index.push(n)?;
    }
    // 指定の値をぴったりの容量に写す（伸ばしながら集めたので）
    let mut exact: MVec<'_, Specified> = MVec::with_capacity(budget, out.specs.len())?;
    exact.append_slice(out.specs.as_slice())?;
    let mut w: MVec<'_, Warn> = MVec::with_capacity(budget, out.warns.len())?;
    w.append_slice(out.warns.as_slice())?;
    let index = core::mem::replace(&mut out.index, MVec::new(budget));
    Ok(Styles { specs: exact.freeze(), index: index.freeze(), warnings: w.freeze() })
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

/// 宣言の名前と値を確かめる（知らない名前は prop が u16::MAX）。
fn check_decl(text: &[u8], d: &Decl, budget: &Budget<'_>) -> Result<DeclInfo, Error> {
    let name = d.name.of(text);
    let id = if d.name_escaped != 0 { None } else { lookup(name, true) };
    let Some(id) = id else { return Ok(DeclInfo { prop: u16::MAX, wide: 0, ok: 0 }) };
    let w = wide(text, d.value, budget)?;
    let ok = match (w, prop(id)) {
        (Some(_), _) => true,
        (None, Some(p)) => valid(p.kind, text, d.value, false, budget)?,
        (None, None) => false,
    };
    Ok(DeclInfo { prop: id, wide: wide_code(w), ok: u8::from(ok) })
}

impl Slot {
    const fn empty() -> Slot {
        Slot {
            set: 0,
            pad: [0; 3],
            key_tier: 0,
            key_layer: 0,
            key_spec: 0,
            key_order: 0,
            key_idx: 0,
            spec: Specified {
                node: 0,
                value: Span { start: 0, end: 0 },
                prop: 0,
                src: 0,
                origin: 0,
                wide: 0,
                pad: [0; 3],
            },
        }
    }
}

struct Out<'a, 'b> {
    doc: &'a XmlDoc,
    input: &'a [u8],
    sheet: &'a Sheet,
    infos: &'a [DeclInfo],
    budget: &'b Budget<'b>,
    slots: MVec<'b, Slot>,
    /// 決めている要素（なければ NONE）
    cur: u32,
    specs: MVec<'b, Specified>,
    index: MVec<'b, u32>,
    warns: MVec<'b, Warn>,
}

impl Out<'_, '_> {
    fn node(&self) -> Option<&Node> {
        self.doc.node(self.cur)
    }

    /// 候補を置く（強ければ入れ替える）。
    fn offer(&mut self, prop: PropertyId, key: Key, spec: Specified) -> Result<(), Error> {
        self.budget.spend(cost::OP_STEP)?;
        // marker のまとめは 3 つに分ける
        if PROPS.get(usize::from(prop)).is_some_and(|p| p.name == b"marker") {
            for name in [b"marker-start".as_slice(), b"marker-mid", b"marker-end"] {
                if let Some(id) = lookup(name, false) {
                    self.put(id, key, Specified { prop: id, ..spec });
                }
            }
            return Ok(());
        }
        self.put(prop, key, spec);
        Ok(())
    }

    fn put(&mut self, prop: PropertyId, key: Key, spec: Specified) {
        if let Some(s) = self.slots.get_mut(usize::from(prop))
            && (s.set == 0 || key >= s.key())
        {
            *s = Slot {
                set: 1,
                pad: [0; 3],
                key_tier: u32::from(key.tier),
                key_layer: key.layer,
                key_spec: key.spec,
                key_order: key.order,
                key_idx: key.idx,
                spec,
            };
        }
    }

    /// 要素の決めた指定の値を書き出し、次の要素のために空にする。プレゼンテーション属性と style 属性をここで足す。
    fn finish_element(&mut self) -> Result<(), Error> {
        if self.cur == NONE {
            return Ok(());
        }
        let e = self.cur;
        let Some(&n) = self.node() else { return Ok(()) };
        if n.ns == NS_SVG {
            self.attributes(e, &n)?;
        }
        // 節の始まりを書く（飛ばした文字の節を含めて、この要素まで）
        let start = self.specs.len() as u32;
        while self.index.len() <= e as usize {
            self.budget.spend(cost::OP_STEP)?;
            self.index.push(start)?;
        }
        for k in 0..self.slots.len() {
            self.budget.spend(cost::OP_STEP)?;
            let Some(s) = self.slots.get_mut(k) else { continue };
            if s.set == 0 {
                continue;
            }
            s.set = 0;
            let spec = Specified { node: e, ..s.spec };
            if n.ns == NS_SVG {
                self.specs.push(spec)?;
            }
        }
        self.cur = NONE;
        Ok(())
    }

    /// プレゼンテーション属性と style 属性を候補にする。
    fn attributes(&mut self, e: u32, n: &Node) -> Result<(), Error> {
        let doc = self.doc;
        let input = self.input;
        let local = n.local.of(input);
        for (k, a) in doc.attrs_of(n).iter().enumerate() {
            self.budget.spend(cost::OP_STEP)?;
            if a.ns != NS_NONE {
                continue;
            }
            let name = a.local.of(input);
            let src = if a.flags & FLAG_IN_STORE != 0 { SRC_STORE } else { SRC_INPUT };
            let buf = if src == SRC_STORE { doc.store.as_slice() } else { input };
            if name == b"style" {
                self.style_attribute(e, n, buf, a.value, src)?;
                continue;
            }
            // transform 属性は transform プロパティのプレゼンテーション属性（SVG の書き方）
            let id = if name == b"transform" { lookup(b"transform", false) } else { lookup(name, false) };
            let Some(id) = id else { continue };
            let Some(p) = prop(id) else { continue };
            if !(p.attr || name == b"transform") || !geometry_ok(local, name) {
                continue;
            }
            let w = wide(buf, a.value, self.budget)?;
            if w.is_none() && !valid(p.kind, buf, a.value, true, self.budget)? {
                self.warns.push(Warn { pos: a.pos, kind: WarningKind::InvalidValue, pad: 0 })?;
                continue;
            }
            let spec = Specified {
                node: e,
                value: a.value,
                prop: id,
                src,
                origin: ORIGIN_ATTR,
                wide: wide_code(w),
                pad: [0; 3],
            };
            self.offer(id, Key { tier: 0, idx: k as u32, ..Key::default() }, spec)?;
        }
        Ok(())
    }

    fn style_attribute(&mut self, e: u32, n: &Node, buf: &[u8], value: Span, src: u8) -> Result<(), Error> {
        let budget = self.budget;
        let mut lex = Lexer::over(buf, value, budget);
        let mut list: MVec<'_, Decl> = MVec::new(budget);
        parse_declarations(&mut lex, buf, false, budget, &mut |d| list.push(d))?;
        for (k, d) in list.as_slice().iter().enumerate() {
            budget.spend(cost::OP_STEP)?;
            let info = check_decl(buf, d, budget)?;
            if info.prop == u16::MAX {
                continue;
            }
            if info.ok == 0 {
                self.warns.push(Warn { pos: n.pos, kind: WarningKind::InvalidValue, pad: 0 })?;
                continue;
            }
            let tier = if d.important != 0 { 4 } else { 2 };
            let spec = Specified {
                node: e,
                value: d.value,
                prop: info.prop,
                src,
                origin: ORIGIN_STYLE,
                wide: info.wide,
                pad: [0; 3],
            };
            self.offer(info.prop, Key { tier, idx: k as u32, ..Key::default() }, spec)?;
        }
        Ok(())
    }
}

impl MatchSink for Out<'_, '_> {
    fn element(&mut self, node: u32) -> Result<(), Error> {
        self.finish_element()?;
        self.cur = node;
        Ok(())
    }

    fn rule(&mut self, rule: u32, spec: u32) -> Result<(), Error> {
        let sheet = self.sheet;
        let Some(r) = sheet.rules.as_slice().get(rule as usize) else { return Ok(()) };
        let rank = sheet.rank(r);
        for k in r.decls.start..r.decls.end {
            self.budget.spend(cost::OP_STEP)?;
            let Some(info) = self.infos.get(k as usize).copied() else { continue };
            let Some(d) = sheet.decls.as_slice().get(k as usize) else { continue };
            if info.prop == u16::MAX || info.ok == 0 {
                continue;
            }
            let important = d.important != 0;
            let key = Key {
                tier: if important { 3 } else { 1 },
                layer: if important { u32::MAX.saturating_sub(rank) } else { rank },
                spec,
                order: r.order,
                idx: k,
            };
            let s = Specified {
                node: self.cur,
                value: d.value,
                prop: info.prop,
                src: SRC_SHEET,
                origin: ORIGIN_RULE,
                wide: info.wide,
                pad: [0; 3],
            };
            self.offer(info.prop, key, s)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
