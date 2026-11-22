#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::indexing_slicing)]

use super::*;

#[test]
fn table_is_sorted_and_lookup_works() {
    for w in PROPS.windows(2) {
        assert!(
            w[0].name < w[1].name,
            "{:?} と {:?}",
            String::from_utf8_lossy(w[0].name),
            String::from_utf8_lossy(w[1].name)
        );
    }
    let fill = lookup(b"fill", false).unwrap();
    assert_eq!(prop(fill).unwrap().name, b"fill");
    assert_eq!(lookup(b"FILL", true), Some(fill));
    assert_eq!(lookup(b"FILL", false), None);
    assert_eq!(lookup(b"bogus", true), None);
    assert!(prop(lookup(b"stroke", false).unwrap()).unwrap().inherited);
    assert!(!prop(lookup(b"opacity", false).unwrap()).unwrap().inherited);
    assert!(!prop(lookup(b"mix-blend-mode", false).unwrap()).unwrap().attr);
}
