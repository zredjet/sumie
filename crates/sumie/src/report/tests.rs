#![allow(clippy::disallowed_macros, clippy::disallowed_methods, clippy::disallowed_types)]

use super::*;
use crate::cancel::Never;
use crate::cost;
use crate::error::Stage;

#[test]
fn sorted_by_position_and_stable() {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut r = ReportBuilder::with_capacity(&budget, 4, 2, 6).unwrap();
    r.warn(WarningKind::AtRule, Some(30)).unwrap();
    r.warn(WarningKind::InvalidValue, None).unwrap();
    r.warn(WarningKind::UnknownElement, Some(10)).unwrap();
    r.warn(WarningKind::ReferenceCycle, Some(10)).unwrap();
    r.unread(b"b.svg", 20).unwrap();
    r.unread(b"#", 5).unwrap();
    let report = r.finish().unwrap();
    let w: Vec<Warning> = report.warnings().collect();
    assert_eq!(
        w,
        [
            Warning { kind: WarningKind::UnknownElement, position: Some(10) },
            Warning { kind: WarningKind::ReferenceCycle, position: Some(10) },
            Warning { kind: WarningKind::AtRule, position: Some(30) },
            Warning { kind: WarningKind::InvalidValue, position: None },
        ]
    );
    let u: Vec<UnreadRef<'_>> = report.unread_refs().collect();
    assert_eq!(u, [UnreadRef { href: b"#", position: 5 }, UnreadRef { href: b"b.svg", position: 20 }]);
    // 持ち続ける量は頼んだ容量の分
    assert_eq!(report.bytes(), (4 * size_of::<WarningEntry>() + 2 * size_of::<UnreadEntry>() + 6) as u64);
    // 容量を取る手間（3 つの入れ物）は勘定に入る
    assert!(budget.work() >= cost::ALLOC_STEP * 3);
}

#[test]
fn empty_report() {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let report = ReportBuilder::with_capacity(&budget, 0, 0, 0).unwrap().finish().unwrap();
    assert_eq!(report.warnings().len(), 0);
    assert_eq!(report.unread_refs().len(), 0);
    assert_eq!(report.bytes(), 0);
}

/// 数えた容量を超えて足すのは見積もりの不具合（伸ばさずに誤り）。
#[test]
fn capacity_is_not_exceeded() {
    let budget = Budget::new(Stage::Parse, u64::MAX, u64::MAX, &Never);
    let mut r = ReportBuilder::with_capacity(&budget, 1, 1, 2).unwrap();
    r.warn(WarningKind::AtRule, None).unwrap();
    assert!(matches!(r.warn(WarningKind::AtRule, None), Err(Error::Limit(l)) if l.stage == Stage::Parse));
    assert!(r.unread(b"abc", 0).is_err());
    r.unread(b"ab", 0).unwrap();
    assert!(r.unread(b"", 1).is_err());
    let report = r.finish().unwrap();
    assert_eq!(report.bytes(), (size_of::<WarningEntry>() + size_of::<UnreadEntry>() + 2) as u64);
}
