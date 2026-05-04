//! End-to-end span and date×span integration through a real `Sheet`.
//! Exercises the dispatch contract that DateHandler owns `Date ± Span`
//! and SpanHandler owns `Date - Date`, `Span ± Span`, `Span * N`.

use std::sync::Arc;

use lotus_core::{CellValue, CustomValue, Registry, Sheet};

fn sheet() -> Sheet {
    let mut reg = Registry::new();
    lotus_datetime::register(&mut reg).unwrap();
    Sheet::new_with_registry(Arc::new(reg))
}

fn jdate(data: &str) -> CellValue {
    CellValue::Custom(CustomValue { type_tag: "jdate".into(), data: data.into() })
}

#[test]
fn date_plus_days_via_operator() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 4, 27)".into()),
        ("A2".into(), "=A1 + DAYS(10)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A2"), jdate("2025-05-07"));
}

#[test]
fn date_minus_days() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 4, 27)".into()),
        ("A2".into(), "=A1 - DAYS(10)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A2"), jdate("2025-04-17"));
}

#[test]
fn date_minus_date_yields_span() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 4, 27)".into()),
        ("A2".into(), "=DATE(2025, 4, 20)".into()),
        // Convert through SPAN_TO_DAYS so we can assert against a number.
        ("A3".into(), "=SPAN_TO_DAYS(A1 - A2)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A3"), CellValue::Number(7.0));
}

#[test]
fn days_between_signed() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 1, 1)".into()),
        ("A2".into(), "=DATE(2025, 1, 31)".into()),
        ("B1".into(), "=DAYS_BETWEEN(A1, A2)".into()),
        ("B2".into(), "=DAYS_BETWEEN(A2, A1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Number(30.0));
    assert_eq!(s.get("B2"), CellValue::Number(-30.0));
}

#[test]
fn months_add_clamp_to_end_of_month() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATE(2025, 1, 31)".into()),
        ("A2".into(), "=A1 + MONTHS(1)".into()),
    ])
    .unwrap();
    // Jan 31 + 1 month → Feb 28 (jiff clamps).
    assert_eq!(s.get("A2"), jdate("2025-02-28"));
}

#[test]
fn span_arithmetic_chains() {
    let mut s = sheet();
    s.set_cells(&[
        // A week + 3 days = 10 days, then × 2 = 20 days.
        ("A1".into(), "=(WEEKS(1) + DAYS(3)) * 2".into()),
        ("A2".into(), "=SPAN_TO_DAYS(A1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A2"), CellValue::Number(20.0));
}

#[test]
fn span_compare_orders_by_duration() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DAYS(7)".into()),
        ("A2".into(), "=WEEKS(1)".into()),
        ("A3".into(), "=DAYS(3)".into()),
        ("B1".into(), "=A1=A2".into()),  // 7 days == 1 week
        ("B2".into(), "=A3<A1".into()),  // 3 days < 7 days
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Boolean(true));
    assert_eq!(s.get("B2"), CellValue::Boolean(true));
}

#[test]
fn iso_span_literal_parsed_as_jspan() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "P10D".into())]).unwrap();
    match s.get("A1") {
        CellValue::Custom(cv) => assert_eq!(cv.type_tag, "jspan"),
        other => panic!("expected jspan, got {other:?}"),
    }
}

#[test]
fn span_to_seconds_calendar_requires_relative_for_correctness() {
    let mut s = sheet();
    // Without explicit relative, MONTHS(1) defaults to EPOCH (Jan 1970, 31 days).
    s.set_cells(&[
        ("A1".into(), "=SPAN_TO_DAYS(MONTHS(1))".into()),
        // With explicit Feb relative, MONTHS(1) = 28 days.
        ("A2".into(), "=SPAN_TO_DAYS(MONTHS(1), DATE(2025, 2, 1))".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A1"), CellValue::Number(31.0));
    assert_eq!(s.get("A2"), CellValue::Number(28.0));
}
