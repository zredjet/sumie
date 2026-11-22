//! ID の表（architecture.md §4.3・FR-ST-13）。同じ id は文書の順で最初の要素を指す（DOM・Chrome と同じ。ADR-66 (5)）。
//!
//! 名前空間のない `id` 属性を持つ要素を、値のバイトの順に並べ（同じ値は文書の順のまま）、同じ値の 2 つめからを除く。引く
//! のは二分探索。比べるバイトは比べる中で塊ずつ払う。

use crate::budget::Budget;
use crate::cost;
use crate::error::Error;
use crate::mem::{Frozen, MVec, bytes_cmp, lower_bound_try, stable_sort_try};
use crate::xml::{KIND_ELEMENT, NS_NONE, XmlDoc};

/// ID の表：（id の値の属性の番号, 要素の番号）を値の順に並べたもの。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdEntry {
    /// `id` 属性（`XmlDoc::attrs` の番号）
    pub(crate) attr: u32,
    pub(crate) node: u32,
}

impl crate::mem::Plain for IdEntry {}

#[derive(Debug)]
pub(crate) struct Ids {
    pub(crate) entries: Frozen<IdEntry>,
}

impl Ids {
    /// 表を作る。
    pub(crate) fn build(doc: &XmlDoc, input: &[u8], budget: &Budget<'_>) -> Result<Ids, Error> {
        let budget = budget_ref(budget);
        let nodes = doc.nodes.as_slice();
        // 数える（要素ごとに id を探す。属性ごとに払う）
        let mut n = 0usize;
        for node in nodes {
            budget.spend(cost::OP_STEP)?;
            if node.kind == KIND_ELEMENT && doc.find_attr(input, node, NS_NONE, b"id", budget)?.is_some() {
                n = n.saturating_add(1);
            }
        }
        let mut v: MVec<'_, IdEntry> = MVec::with_capacity(budget, n)?;
        for (i, node) in nodes.iter().enumerate() {
            budget.spend(cost::OP_STEP)?;
            if node.kind != KIND_ELEMENT {
                continue;
            }
            for k in node.attrs.start..node.attrs.end {
                budget.spend(cost::OP_STEP)?;
                let Some(a) = doc.attrs.as_slice().get(k as usize) else { break };
                if a.ns == NS_NONE && a.local.of(input) == b"id" {
                    v.push(IdEntry { attr: k, node: i as u32 })?;
                    break;
                }
            }
        }
        let value = |e: &IdEntry| doc.attrs.as_slice().get(e.attr as usize).map_or(&[][..], |a| doc.value(input, a));
        // 値の順（同じ値は文書の順のまま。並べ替えは安定）
        stable_sort_try(budget, v.as_mut_slice(), |a, b| Ok(bytes_cmp(budget, value(a), value(b))?.is_lt()))?;
        // 同じ値の 2 つめからを除く
        let mut keep = 0usize;
        for k in 0..v.len() {
            budget.spend(cost::OP_STEP)?;
            let Some(&e) = v.get(k) else { continue };
            let dup = match keep.checked_sub(1).and_then(|p| v.get(p)) {
                Some(prev) => bytes_cmp(budget, value(prev), value(&e))?.is_eq(),
                None => false,
            };
            if dup {
                continue;
            }
            if let Some(x) = v.get_mut(keep) {
                *x = e;
            }
            keep = keep.saturating_add(1);
        }
        v.truncate(keep);
        let mut exact: MVec<'_, IdEntry> = MVec::with_capacity(budget, v.len())?;
        exact.append_slice(v.as_slice())?;
        Ok(Ids { entries: exact.freeze() })
    }

    /// id `name` の要素（なければ None）。
    pub(crate) fn find(
        &self,
        doc: &XmlDoc,
        input: &[u8],
        name: &[u8],
        budget: &Budget<'_>,
    ) -> Result<Option<u32>, Error> {
        let list = self.entries.as_slice();
        let value = |e: &IdEntry| doc.attrs.as_slice().get(e.attr as usize).map_or(&[][..], |a| doc.value(input, a));
        let i = lower_bound_try(budget, list, |e| Ok(bytes_cmp(budget, value(e), name)?.is_lt()))?;
        match list.get(i) {
            Some(e) if bytes_cmp(budget, value(e), name)?.is_eq() => Ok(Some(e.node)),
            _ => Ok(None),
        }
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.entries.bytes()
    }
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
