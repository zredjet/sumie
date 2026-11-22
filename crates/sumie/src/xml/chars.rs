//! XML 1.0（第 5 版）の文字の分類。表は仕様の範囲をそのまま書いたもので、Unicode の版に依らない（determinism.md §4.3）。
//! 入力は UTF-8 として正しいと確かめてから読む（xml の前置き）。

/// XML の空白（S ::= (#x20 | #x9 | #xD | #xA)+）。std の `is_whitespace` は使わない。
pub(crate) const fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

/// UTF-8 の 1 文字を読む（`b` は正しい UTF-8 と確かめたもの）。符号位置とバイト数。範囲の外・途中なら None。
pub(crate) fn decode(b: &[u8], i: usize) -> Option<(u32, usize)> {
    let c0 = u32::from(*b.get(i)?);
    if c0 < 0x80 {
        return Some((c0, 1));
    }
    let cont = |k: usize| b.get(i.checked_add(k)?).map(|&x| u32::from(x) & 0x3F);
    if c0 >= 0xF0 {
        let c = ((c0 & 0x07) << 18) | (cont(1)? << 12) | (cont(2)? << 6) | cont(3)?;
        Some((c, 4))
    } else if c0 >= 0xE0 {
        let c = ((c0 & 0x0F) << 12) | (cont(1)? << 6) | cont(2)?;
        Some((c, 3))
    } else {
        let c = ((c0 & 0x1F) << 6) | cont(1)?;
        Some((c, 2))
    }
}

/// NameStartChar。
pub(crate) const fn is_name_start(c: u32) -> bool {
    matches!(c,
        0x3A | 0x41..=0x5A | 0x5F | 0x61..=0x7A | 0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x2FF | 0x370..=0x37D
        | 0x37F..=0x1FFF | 0x200C..=0x200D | 0x2070..=0x218F | 0x2C00..=0x2FEF | 0x3001..=0xD7FF
        | 0xF900..=0xFDCF | 0xFDF0..=0xFFFD | 0x10000..=0xEFFFF)
}

/// NameChar。
pub(crate) const fn is_name_char(c: u32) -> bool {
    is_name_start(c) || matches!(c, 0x2D | 0x2E | 0x30..=0x39 | 0xB7 | 0x300..=0x36F | 0x203F..=0x2040)
}

/// Char（XML の文書に書いてよい文字）。
pub(crate) const fn is_char(c: u32) -> bool {
    matches!(c, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

#[cfg(test)]
mod tests;
