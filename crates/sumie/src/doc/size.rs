//! 文書の大きさ（FR-OUT-01・ADR-58 (1)・ADR-63 (3)・ADR-64 (8)・ADR-74 (7)）。
//!
//! 外側の `svg` の `width`・`height`（絶対の長さか単位のない数。em・ex などは文字の大きさで解く）と `viewBox` から決める：
//!
//! - 幅と高さが絶対の長さなら、それ
//! - viewBox があれば、% は viewBox の大きさに対する割合、片方（か壊れた値）は viewBox の縦横の比で補い、どちらもなければ
//!   viewBox の大きさ（壊れた viewBox（幅か高さが 0 以下）は無いもの）
//! - viewBox がなく、どちらかが欠ける・%・壊れた値なら、大きさは中身の外接矩形で決まる（木が要る）
//! - 決まった幅か高さが 0 か負なら `Invalid`（理由 `Size`。位置は外側の svg 要素）

use crate::budget::Budget;
use crate::error::{Error, Invalid, InvalidReason};
use crate::values::{Length, Unit, Values, view_box};
use crate::xml::{self, KIND_ELEMENT, NS_NONE, NS_SVG, XmlDoc};

/// ユーザー単位の大きさ。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// 画素の大きさ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

/// 文書の大きさの決まり方。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SizeRule {
    /// 属性で決まる
    Known(Size),
    /// 中身の外接矩形で決まる（木が要る）
    Content,
}

impl SizeRule {
    pub(crate) fn known(self) -> Option<Size> {
        match self {
            SizeRule::Known(s) => Some(s),
            SizeRule::Content => None,
        }
    }
}

/// 幅か高さの属性：絶対の大きさ、割合、なしか壊れた値。
enum Dim {
    Abs(f64),
    Pct(f64),
    No,
}

/// 文書の大きさを決める。`font_size` は em・ex などを解く文字の大きさ（px）。
pub(crate) fn size_rule(doc: &XmlDoc, input: &[u8], font_size: f64, budget: &Budget<'_>) -> Result<SizeRule, Error> {
    let budget = budget_ref(budget);
    let Some(root) = doc.nodes.as_slice().first().filter(|n| n.kind == KIND_ELEMENT) else {
        return Ok(SizeRule::Content);
    };
    if root.ns != NS_SVG || root.local.of(input) != b"svg" {
        return Ok(SizeRule::Content);
    }
    let len = |name: &[u8]| -> Result<Dim, Error> {
        let Some(a) = doc.find_attr(input, root, NS_NONE, name, budget)? else { return Ok(Dim::No) };
        let (buf, span) = attr_buf(doc, input, a);
        let mut v = Values::new(buf, span, budget);
        let l: Option<Length> = v.length(true)?;
        if !v.at_end()? {
            return Ok(Dim::No);
        }
        let Some(l) = l else { return Ok(Dim::No) };
        let px = match l.unit {
            Unit::Percent => return Ok(Dim::Pct(l.value)),
            Unit::Em | Unit::Rem | Unit::Ic => Some(l.value * font_size),
            Unit::Ex | Unit::Ch => Some(l.value * font_size * 0.5),
            Unit::Cap => Some(l.value * font_size * 0.7),
            _ => l.absolute_px(),
        };
        Ok(px.filter(|x| x.is_finite()).map_or(Dim::No, Dim::Abs))
    };
    let w = len(b"width")?;
    let h = len(b"height")?;
    let vb = match doc.find_attr(input, root, NS_NONE, b"viewBox", budget)? {
        Some(a) => {
            let (buf, span) = attr_buf(doc, input, a);
            view_box(buf, span, budget)?.filter(|v| v.w > 0.0 && v.h > 0.0)
        }
        None => None,
    };
    let size = match (w, h, vb) {
        (Dim::Abs(w), Dim::Abs(h), _) => (w, h),
        (w, h, Some(v)) => {
            let resolve = |d: Dim, full: f64| match d {
                Dim::Abs(x) => Some(x),
                Dim::Pct(p) => Some(full * p / 100.0),
                Dim::No => None,
            };
            match (resolve(w, v.w), resolve(h, v.h)) {
                (Some(w), Some(h)) => (w, h),
                (Some(w), None) => (w, w * v.h / v.w),
                (None, Some(h)) => (h * v.w / v.h, h),
                (None, None) => (v.w, v.h),
            }
        }
        _ => return Ok(SizeRule::Content),
    };
    if !(size.0.is_finite() && size.1.is_finite() && size.0 > 0.0 && size.1 > 0.0) {
        return Err(Error::Invalid(Invalid { reason: InvalidReason::Size, position: Some(u64::from(root.pos)) }));
    }
    Ok(SizeRule::Known(Size { width: size.0, height: size.1 }))
}

/// 画素の大きさ（倍率を掛けて切り上げる。10.5 は 11・0.3 は 1）。倍率が有限で正でないか、画素の数が u32 を超えれば
/// `Invalid`（`Argument`）。
pub(crate) fn pixel_size(size: Size, scale: f64) -> Result<PixelSize, Error> {
    let arg = || Error::Invalid(Invalid { reason: InvalidReason::Argument, position: None });
    if !(scale.is_finite() && scale > 0.0) {
        return Err(arg());
    }
    let px = |v: f64| -> Result<u32, Error> {
        let x = (v * scale).ceil();
        if !(x.is_finite() && x >= 1.0 && x <= f64::from(u32::MAX)) {
            // 0 より大きく 1 未満は 1 にする
            if x.is_finite() && x > 0.0 && x < 1.0 {
                return Ok(1);
            }
            return Err(arg());
        }
        Ok(x as u32)
    };
    Ok(PixelSize { width: px(size.width)?, height: px(size.height)? })
}

/// 属性の値の置き場と範囲。
fn attr_buf<'a>(doc: &'a XmlDoc, input: &'a [u8], a: &xml::Attr) -> (&'a [u8], crate::xml::lex::Span) {
    if a.flags & xml::FLAG_IN_STORE != 0 { (doc.store.as_slice(), a.value) } else { (input, a.value) }
}

fn budget_ref<'a>(b: &'a Budget<'_>) -> &'a Budget<'a> {
    b
}

#[cfg(test)]
mod tests;
