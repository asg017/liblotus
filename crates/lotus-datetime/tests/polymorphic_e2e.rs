//! End-to-end coverage for T-DT-5's polymorphic accessors,
//! conversions, between-helpers, and formatting functions.

use std::sync::Arc;

use lotus_core::{CellValue, CustomValue, Registry, Sheet};

fn sheet() -> Sheet {
    let mut reg = Registry::new();
    lotus_datetime::register(&mut reg).unwrap();
    Sheet::new_with_registry(Arc::new(reg))
}

// === Polymorphic calendar accessors ===

#[test]
fn year_month_day_work_on_jdate() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 4, 27)".into()),
        ("B1".into(), "=YEAR(A1)".into()),
        ("C1".into(), "=MONTH(A1)".into()),
        ("D1".into(), "=DAY(A1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Number(2025.0));
    assert_eq!(s.get("C1"), CellValue::Number(4.0));
    assert_eq!(s.get("D1"), CellValue::Number(27.0));
}

#[test]
fn year_month_day_work_on_jdatetime() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATETIME(2025, 4, 27, 14, 30, 0)".into()),
        ("B1".into(), "=YEAR(A1)".into()),
        ("C1".into(), "=MONTH(A1)".into()),
        ("D1".into(), "=DAY(A1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Number(2025.0));
    assert_eq!(s.get("C1"), CellValue::Number(4.0));
    assert_eq!(s.get("D1"), CellValue::Number(27.0));
}

#[cfg(feature = "system-tz")]
#[test]
fn year_month_day_work_on_jzoned() {
    let mut s = sheet();
    s.set_cells(&[
        (
            "A1".into(),
            "=ZONED(DATETIME(2025, 4, 27, 14, 30, 0), \"America/New_York\")".into(),
        ),
        ("B1".into(), "=YEAR(A1)".into()),
        ("B2".into(), "=MONTH(A1)".into()),
        ("B3".into(), "=DAY(A1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Number(2025.0));
    assert_eq!(s.get("B2"), CellValue::Number(4.0));
    assert_eq!(s.get("B3"), CellValue::Number(27.0));
}

#[test]
fn weekday_iso_default_and_sunday_first() {
    // 2025-04-27 is a Sunday.
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 4, 27)".into()),
        ("B1".into(), "=WEEKDAY(A1)".into()),
        ("B2".into(), "=WEEKDAY(A1, FALSE)".into()),
    ])
    .unwrap();
    // ISO: Mon=1 .. Sun=7
    assert_eq!(s.get("B1"), CellValue::Number(7.0));
    // Sunday-first: Sun=1
    assert_eq!(s.get("B2"), CellValue::Number(1.0));
}

#[test]
fn dayofyear_weekofyear_quarter_isleapyear() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2024, 12, 31)".into()),
        ("A2".into(), "=DATE(2025, 4, 27)".into()),
        ("B1".into(), "=DAYOFYEAR(A1)".into()),
        ("B2".into(), "=WEEKOFYEAR(A2)".into()),
        ("C1".into(), "=QUARTER(DATE(2025, 1, 1))".into()),
        ("C2".into(), "=QUARTER(DATE(2025, 12, 31))".into()),
        ("D1".into(), "=IS_LEAP_YEAR(A1)".into()),
        ("D2".into(), "=IS_LEAP_YEAR(A2)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Number(366.0)); // 2024 is leap, has 366 days
    // 2025-04-27 is in ISO week 17.
    assert_eq!(s.get("B2"), CellValue::Number(17.0));
    assert_eq!(s.get("C1"), CellValue::Number(1.0));
    assert_eq!(s.get("C2"), CellValue::Number(4.0));
    assert_eq!(s.get("D1"), CellValue::Boolean(true));
    assert_eq!(s.get("D2"), CellValue::Boolean(false));
}

// === Polymorphic clock accessors ===

#[test]
fn hour_minute_second_on_jtime_jdatetime() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=TIME(14, 30, 45)".into()),
        ("A2".into(), "=DATETIME(2025, 4, 27, 14, 30, 45)".into()),
        ("B1".into(), "=HOUR(A1)".into()),
        ("B2".into(), "=HOUR(A2)".into()),
        ("C1".into(), "=MINUTE(A1)".into()),
        ("C2".into(), "=MINUTE(A2)".into()),
        ("D1".into(), "=SECOND(A1)".into()),
        ("D2".into(), "=SECOND(A2)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Number(14.0));
    assert_eq!(s.get("B2"), CellValue::Number(14.0));
    assert_eq!(s.get("C1"), CellValue::Number(30.0));
    assert_eq!(s.get("C2"), CellValue::Number(30.0));
    assert_eq!(s.get("D1"), CellValue::Number(45.0));
    assert_eq!(s.get("D2"), CellValue::Number(45.0));
}

#[test]
fn nanosecond_extracts_subsecond() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=TIME(0, 0, 0, 123456789)".into()),
        ("B1".into(), "=NANOSECOND(A1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Number(123_456_789.0));
}

#[test]
fn hour_rejects_jdate() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "=HOUR(DATE(2025, 4, 27))".into())]).unwrap();
    let v = s.get("A1");
    assert!(v.is_error(), "expected error, got {v:?}");
}

// === Zoned-only accessors ===

#[cfg(feature = "system-tz")]
#[test]
fn tz_name_offset_isdst() {
    let mut s = sheet();
    s.set_cells(&[
        // June: DST in effect.
        (
            "A1".into(),
            "=ZONED(DATETIME(2024, 6, 15, 12, 0, 0), \"America/New_York\")".into(),
        ),
        ("B1".into(), "=TZ_NAME(A1)".into()),
        ("B2".into(), "=TZ_OFFSET_SECONDS(A1)".into()),
        ("B3".into(), "=IS_DST(A1)".into()),
        // January: no DST.
        (
            "A2".into(),
            "=ZONED(DATETIME(2024, 1, 15, 12, 0, 0), \"America/New_York\")".into(),
        ),
        ("C1".into(), "=IS_DST(A2)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::String("America/New_York".into()));
    assert_eq!(s.get("B2"), CellValue::Number(-4.0 * 3600.0));
    assert_eq!(s.get("B3"), CellValue::Boolean(true));
    assert_eq!(s.get("C1"), CellValue::Boolean(false));
}

// === Conversions ===

#[test]
fn to_date_from_datetime() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATETIME(2025, 4, 27, 14, 30, 0)".into()),
        ("B1".into(), "=TO_DATE(A1)".into()),
    ])
    .unwrap();
    assert_eq!(
        s.get("B1"),
        CellValue::Custom(CustomValue { type_tag: "jdate".into(), data: "2025-04-27".into() })
    );
}

#[test]
fn to_time_from_datetime() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATETIME(2025, 4, 27, 14, 30, 45)".into()),
        ("B1".into(), "=TO_TIME(A1)".into()),
    ])
    .unwrap();
    assert_eq!(
        s.get("B1"),
        CellValue::Custom(CustomValue { type_tag: "jtime".into(), data: "14:30:45".into() })
    );
}

#[test]
fn to_datetime_combines_date_and_time() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 4, 27)".into()),
        ("A2".into(), "=TIME(14, 30, 0)".into()),
        ("B1".into(), "=TO_DATETIME(A1, A2)".into()),
        ("B2".into(), "=TO_DATETIME(A1)".into()),
    ])
    .unwrap();
    assert_eq!(
        s.get("B1"),
        CellValue::Custom(CustomValue { type_tag: "jdatetime".into(), data: "2025-04-27T14:30:00".into() })
    );
    assert_eq!(
        s.get("B2"),
        CellValue::Custom(CustomValue { type_tag: "jdatetime".into(), data: "2025-04-27T00:00:00".into() })
    );
}

// === Between helpers ===

#[test]
fn hours_minutes_seconds_between_jdatetime() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATETIME(2025, 4, 27, 12, 0, 0)".into()),
        ("A2".into(), "=DATETIME(2025, 4, 27, 13, 30, 45)".into()),
        ("B1".into(), "=HOURS_BETWEEN(A1, A2)".into()),
        ("B2".into(), "=MINUTES_BETWEEN(A1, A2)".into()),
        ("B3".into(), "=SECONDS_BETWEEN(A1, A2)".into()),
    ])
    .unwrap();
    // 1h 30m 45s = 5445s = 90.75min = 1.5125h
    assert_eq!(s.get("B3"), CellValue::Number(5445.0));
    assert_eq!(s.get("B2"), CellValue::Number(90.75));
    assert_eq!(s.get("B1"), CellValue::Number(5445.0 / 3600.0));
}

#[test]
fn between_rejects_mixed_types() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 4, 27)".into()),
        ("A2".into(), "=DATETIME(2025, 4, 27, 14, 30, 0)".into()),
        ("B1".into(), "=DAYS_BETWEEN(A1, A2)".into()),
    ])
    .unwrap();
    let v = s.get("B1");
    assert!(v.is_error(), "expected error for mixed types, got {v:?}");
    assert!(v.to_string().contains("same type"), "got {v}");
}

// === DATEADD / DATESUB ===

#[test]
fn dateadd_datesub_match_operators() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 4, 27)".into()),
        ("B1".into(), "=A1 + DAYS(7)".into()),
        ("B2".into(), "=DATEADD(A1, DAYS(7))".into()),
        ("C1".into(), "=A1 - DAYS(7)".into()),
        ("C2".into(), "=DATESUB(A1, DAYS(7))".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), s.get("B2"));
    assert_eq!(s.get("C1"), s.get("C2"));
}

#[test]
fn dateadd_works_on_jdatetime_jtime() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATETIME(2025, 4, 27, 14, 30, 0)".into()),
        ("B1".into(), "=DATEADD(A1, HOURS(3))".into()),
        ("A2".into(), "=TIME(23, 0, 0)".into()),
        ("B2".into(), "=DATEADD(A2, HOURS(2))".into()),
    ])
    .unwrap();
    assert_eq!(
        s.get("B1"),
        CellValue::Custom(CustomValue { type_tag: "jdatetime".into(), data: "2025-04-27T17:30:00".into() })
    );
    assert_eq!(
        s.get("B2"),
        CellValue::Custom(CustomValue { type_tag: "jtime".into(), data: "01:00:00".into() })
    );
}

// === FORMAT / ISO ===

#[test]
fn format_strftime_polymorphic() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 4, 27)".into()),
        ("B1".into(), "=FORMAT(A1, \"%B %d, %Y\")".into()),
        ("B2".into(), "=FORMAT(DATETIME(2025, 4, 27, 14, 30, 0), \"%Y-%m-%d %H:%M\")".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::String("April 27, 2025".into()));
    assert_eq!(s.get("B2"), CellValue::String("2025-04-27 14:30".into()));
}

#[test]
fn iso_returns_canonical() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 4, 27)".into()),
        ("A2".into(), "=DATETIME(2025, 4, 27, 14, 30, 0)".into()),
        ("B1".into(), "=ISO(A1)".into()),
        ("B2".into(), "=ISO(A2)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::String("2025-04-27".into()));
    // ISO returns the canonical form (with T), not the friendlier display form.
    assert_eq!(s.get("B2"), CellValue::String("2025-04-27T14:30:00".into()));
}

#[test]
fn parse_date_with_format() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "=PARSE_DATE(\"04/27/2025\", \"%m/%d/%Y\")".into())]).unwrap();
    assert_eq!(
        s.get("A1"),
        CellValue::Custom(CustomValue { type_tag: "jdate".into(), data: "2025-04-27".into() })
    );
}
