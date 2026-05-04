//! End-to-end coverage for jzoned + jtimezone — including the DST
//! gauntlet pinned to US 2024 transitions in America/New_York.
//!
//! Gated on `system-tz` (and implicitly the bundled tzdb feature on
//! platforms without one). Without it, IANA name lookups fail.

#![cfg(feature = "system-tz")]

use std::sync::Arc;

use lotus_core::{CellValue, Registry, Sheet};

fn sheet() -> Sheet {
    let mut reg = Registry::new();
    lotus_datetime::register(&mut reg).unwrap();
    Sheet::new_with_registry(Arc::new(reg))
}

#[test]
fn timezone_constructor() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=TIMEZONE(\"America/New_York\")".into()),
        ("A2".into(), "=TIMEZONE(\"+05:30\")".into()),
        ("A3".into(), "=TIMEZONE(\"+0530\")".into()),
    ])
    .unwrap();
    match s.get("A1") {
        CellValue::Custom(cv) => {
            assert_eq!(cv.type_tag, "jtimezone");
            assert_eq!(cv.data, "America/New_York");
        }
        other => panic!("expected jtimezone, got {other:?}"),
    }
    // Both `+05:30` and `+0530` should canonicalise to `+05:30`.
    let CellValue::Custom(a2) = s.get("A2") else {
        panic!()
    };
    let CellValue::Custom(a3) = s.get("A3") else {
        panic!()
    };
    assert_eq!(a2.data, "+05:30");
    assert_eq!(a2.data, a3.data);
}

#[test]
fn timezone_rejects_garbage() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "=TIMEZONE(\"Mars/Olympus\")".into())]).unwrap();
    let v = s.get("A1");
    assert!(v.is_error(), "expected error, got {v:?}");
    assert!(v.to_string().starts_with("#VALUE!"), "got {v}");
}

#[test]
fn zoned_literal_parsed() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "2024-06-15T12:00:00-04:00[America/New_York]".into())]).unwrap();
    match s.get("A1") {
        CellValue::Custom(cv) => assert_eq!(cv.type_tag, "jzoned"),
        other => panic!("expected jzoned, got {other:?}"),
    }
}

#[test]
fn zoned_constructor_compatible_default() {
    let mut s = sheet();
    s.set_cells(&[(
        "A1".into(),
        "=ZONED(DATETIME(2024, 6, 15, 12, 0, 0), \"America/New_York\")".into(),
    )])
    .unwrap();
    let CellValue::Custom(cv) = s.get("A1") else {
        panic!()
    };
    // June 15 → DST in effect (-04:00).
    assert!(cv.data.contains("-04:00[America/New_York]"), "got {}", cv.data);
}

#[test]
fn in_tz_preserves_instant() {
    let mut s = sheet();
    s.set_cells(&[
        (
            "A1".into(),
            "=ZONED(DATETIME(2024, 6, 15, 12, 0, 0), \"America/New_York\")".into(),
        ),
        ("A2".into(), "=IN_TZ(A1, \"UTC\")".into()),
        // Equality compares instants — if IN_TZ preserves the instant
        // these must compare equal even though the wall clocks differ.
        ("B1".into(), "=A1=A2".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Boolean(true));
    // And the UTC wall clock should be 16:00 (NYC noon DST = UTC 16:00).
    let CellValue::Custom(a2) = s.get("A2") else {
        panic!()
    };
    assert!(a2.data.starts_with("2024-06-15T16:00:00"), "got {}", a2.data);
}

#[test]
fn to_utc_sugar_matches_in_tz_utc() {
    let mut s = sheet();
    s.set_cells(&[
        (
            "A1".into(),
            "=ZONED(DATETIME(2024, 6, 15, 12, 0, 0), \"America/New_York\")".into(),
        ),
        ("A2".into(), "=TO_UTC(A1)".into()),
        ("A3".into(), "=IN_TZ(A1, \"UTC\")".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A2"), s.get("A3"));
}

#[test]
fn utcnow_returns_jzoned() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "=UTCNOW()".into())]).unwrap();
    match s.get("A1") {
        CellValue::Custom(cv) => {
            assert_eq!(cv.type_tag, "jzoned");
            assert!(cv.data.contains("[UTC]"), "got {}", cv.data);
        }
        other => panic!("expected jzoned, got {other:?}"),
    }
}

#[test]
fn now_returns_jzoned() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "=NOW()".into())]).unwrap();
    match s.get("A1") {
        CellValue::Custom(cv) => assert_eq!(cv.type_tag, "jzoned"),
        other => panic!("expected jzoned, got {other:?}"),
    }
}

// === DST gauntlet ===
// US 2024 spring forward: 2024-03-10 02:00 → 03:00 (gap; 02:30 doesn't exist)
// US 2024 fall back:      2024-11-03 02:00 → 01:00 (fold; 01:30 occurs twice)
// All assertions pinned to America/New_York.

#[test]
fn dst_gap_compatible_shifts_forward() {
    let mut s = sheet();
    s.set_cells(&[(
        "A1".into(),
        "=ZONED(DATETIME(2024, 3, 10, 2, 30, 0), \"America/New_York\", \"compatible\")".into(),
    )])
    .unwrap();
    // jiff's "compatible" strategy advances into the post-gap offset
    // (-04:00) while keeping the local clock — so 02:30 becomes 03:30.
    let CellValue::Custom(cv) = s.get("A1") else {
        panic!()
    };
    assert!(cv.data.contains("03:30:00-04:00"), "got {}", cv.data);
}

#[test]
fn dst_gap_reject_errors() {
    let mut s = sheet();
    s.set_cells(&[(
        "A1".into(),
        "=ZONED(DATETIME(2024, 3, 10, 2, 30, 0), \"America/New_York\", \"reject\")".into(),
    )])
    .unwrap();
    let v = s.get("A1");
    assert!(v.is_error(), "expected error, got {v:?}");
    assert!(v.to_string().contains("ambiguous"), "got {v}");
}

#[test]
fn dst_fold_earlier_picks_first_occurrence() {
    let mut s = sheet();
    s.set_cells(&[(
        "A1".into(),
        "=ZONED(DATETIME(2024, 11, 3, 1, 30, 0), \"America/New_York\", \"earlier\")".into(),
    )])
    .unwrap();
    let CellValue::Custom(cv) = s.get("A1") else {
        panic!()
    };
    // Earlier occurrence of 01:30 has the pre-transition offset (-04:00).
    assert!(cv.data.contains("01:30:00-04:00"), "got {}", cv.data);
}

#[test]
fn dst_fold_later_picks_second_occurrence() {
    let mut s = sheet();
    s.set_cells(&[(
        "A1".into(),
        "=ZONED(DATETIME(2024, 11, 3, 1, 30, 0), \"America/New_York\", \"later\")".into(),
    )])
    .unwrap();
    let CellValue::Custom(cv) = s.get("A1") else {
        panic!()
    };
    // Later occurrence has the post-transition offset (-05:00).
    assert!(cv.data.contains("01:30:00-05:00"), "got {}", cv.data);
}

#[test]
fn zoned_plus_one_day_lands_on_next_civil_day_not_24h() {
    // Crossing the spring-forward transition: March 9 noon + 1 day must
    // land on March 10 noon (same wall clock, different offset) — NOT
    // 24 hours later (which would put us past noon since the day has
    // only 23 hours of clock time).
    let mut s = sheet();
    s.set_cells(&[
        (
            "A1".into(),
            "=ZONED(DATETIME(2024, 3, 9, 12, 0, 0), \"America/New_York\")".into(),
        ),
        ("A2".into(), "=A1 + DAYS(1)".into()),
        ("A3".into(), "=SPAN_TO_SECONDS(A2 - A1)".into()),
    ])
    .unwrap();
    let CellValue::Custom(a2) = s.get("A2") else {
        panic!()
    };
    // Wall clock: noon next day, post-DST offset.
    assert!(a2.data.contains("2024-03-10T12:00:00-04:00"), "got {}", a2.data);
    // The actual elapsed seconds = 23 hours = 82_800s, NOT 86_400.
    assert_eq!(s.get("A3"), CellValue::Number(82_800.0));
}

#[test]
fn zoned_compare_uses_instant_not_wall_clock() {
    // Same instant, different zones — must compare equal.
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "2024-06-15T12:00:00-04:00[America/New_York]".into()),
        ("A2".into(), "2024-06-15T16:00:00+00:00[UTC]".into()),
        ("B1".into(), "=A1=A2".into()),
        ("B2".into(), "=A1<A2".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Boolean(true));
    assert_eq!(s.get("B2"), CellValue::Boolean(false));
}

#[test]
fn zoned_minus_zoned_yields_dst_aware_span() {
    // March 10 noon - March 9 noon (NYC) = 23 hours, not 24.
    let mut s = sheet();
    s.set_cells(&[
        (
            "A1".into(),
            "=ZONED(DATETIME(2024, 3, 10, 12, 0, 0), \"America/New_York\")".into(),
        ),
        (
            "A2".into(),
            "=ZONED(DATETIME(2024, 3, 9, 12, 0, 0), \"America/New_York\")".into(),
        ),
        ("A3".into(), "=SPAN_TO_SECONDS(A1 - A2)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A3"), CellValue::Number(82_800.0));
}

#[test]
fn zoned_aggregates_via_as_number() {
    // SUM coerces jzoned via `as_number` (Unix-epoch seconds).
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "1970-01-01T00:00:00+00:00[UTC]".into()),
        ("A2".into(), "1970-01-01T00:01:00+00:00[UTC]".into()),
        ("A3".into(), "=SUM(A1:A2)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A3"), CellValue::Number(60.0));
}
