#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use super::*;

#[test]
fn spaces_and_chars() {
    assert!(is_space(b' ') && is_space(b'\t') && is_space(b'\r') && is_space(b'\n'));
    assert!(!is_space(0x0B) && !is_space(0x0C) && !is_space(0xA0));
    assert!(is_char(0x9) && is_char(0x20) && is_char(0x10FFFF));
    assert!(!is_char(0x0) && !is_char(0x1) && !is_char(0xFFFE) && !is_char(0xD800));
}

#[test]
fn names() {
    for c in ['a', 'Z', '_', ':', 'あ', '\u{10000}'] {
        assert!(is_name_start(c as u32), "{c}");
    }
    for c in ['-', '.', '0', '\u{B7}', '\u{300}'] {
        assert!(!is_name_start(c as u32) && is_name_char(c as u32), "{c}");
    }
    for c in [' ', '<', '&', '\u{D7}', '\u{F7}'] {
        assert!(!is_name_char(c as u32), "{c}");
    }
}

#[test]
fn decoding() {
    let s = "aé鞍😀".as_bytes();
    assert_eq!(decode(s, 0), Some(('a' as u32, 1)));
    assert_eq!(decode(s, 1), Some(('é' as u32, 2)));
    assert_eq!(decode(s, 3), Some(('鞍' as u32, 3)));
    assert_eq!(decode(s, 6), Some(('😀' as u32, 4)));
    assert_eq!(decode(s, 10), None);
    // 途中で切れた文字
    assert_eq!(decode(&s[..5], 3), None);
}
