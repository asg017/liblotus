//! End-to-end coverage for jtime + jdatetime through a real `Sheet`.

use std::sync::Arc;

use lotus_core::{CellValue, CustomValue, Registry, Sheet};

fn sheet() -> Sheet {
    let mut reg = Registry::new();
    lotus_datetime::register(&mut reg).unwrap();
    Sheet::new_with_registry(Arc::new(reg))
}

fn jtime(data: &str) -> CellValue {
    CellValue::Custom(CustomValue { type_tag: "jtime".into(), data: data.into() })
}

fn jdatetime(data: &str) -> CellValue {
    CellValue::Custom(CustomValue { type_tag: "jdatetime".into(), data: data.into() })
}

#[test]
fn time_literal_parsed() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "14:30:00".into())]).unwrap();
    assert_eq!(s.get("A1"), jtime("14:30:00"));
}

#[test]
fn datetime_literal_parsed() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "2025-04-27T14:30:00".into())]).unwrap();
    assert_eq!(s.get("A1"), jdatetime("2025-04-27T14:30:00"));
}

#[test]
fn time_function_constructs_jtime() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "=TIME(14, 30, 0)".into())]).unwrap();
    assert_eq!(s.get("A1"), jtime("14:30:00"));
}

#[test]
fn datetime_function_constructs_jdatetime() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "=DATETIME(2025, 4, 27, 14, 30, 0)".into())]).unwrap();
    assert_eq!(s.get("A1"), jdatetime("2025-04-27T14:30:00"));
}

#[test]
fn time_plus_span_via_operator() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=TIME(14, 30, 0)".into()),
        ("A2".into(), "=A1 + HOURS(1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A2"), jtime("15:30:00"));
}

#[test]
fn time_plus_calendar_span_errors() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=TIME(14, 30, 0)".into()),
        ("A2".into(), "=A1 + DAYS(1)".into()),
    ])
    .unwrap();
    let v = s.get("A2");
    assert!(v.is_error(), "expected error, got {v:?}");
    assert!(v.to_string().contains("calendar units"), "got {v}");
}

#[test]
fn datetime_plus_span() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATETIME(2025, 4, 27, 12, 0, 0)".into()),
        ("A2".into(), "=A1 + HOURS(3)".into()),
        ("A3".into(), "=A1 + DAYS(1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A2"), jdatetime("2025-04-27T15:00:00"));
    assert_eq!(s.get("A3"), jdatetime("2025-04-28T12:00:00"));
}

#[test]
fn datetime_minus_datetime() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATETIME(2025, 4, 28, 12, 0, 0)".into()),
        ("A2".into(), "=DATETIME(2025, 4, 27, 12, 0, 0)".into()),
        ("A3".into(), "=SPAN_TO_DAYS(A1 - A2)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A3"), CellValue::Number(1.0));
}

#[test]
fn time_minus_time_yields_span() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=TIME(15, 0, 0)".into()),
        ("A2".into(), "=TIME(14, 0, 0)".into()),
        ("A3".into(), "=SPAN_TO_SECONDS(A1 - A2)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A3"), CellValue::Number(3600.0));
}

#[test]
fn parse_datetime_with_format() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "=PARSE_DATETIME(\"04/27/2025 02:30 PM\", \"%m/%d/%Y %I:%M %p\")".into())])
        .unwrap();
    assert_eq!(s.get("A1"), jdatetime("2025-04-27T14:30:00"));
}

#[test]
fn datetime_display_uses_space() {
    // The cell renders via the registry path (CONCAT goes through display).
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "=DATETIME(2025, 4, 27, 14, 30, 0)".into()),
        ("A2".into(), "=CONCAT(\"at \", A1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A2"), CellValue::String("at 2025-04-27 14:30:00".into()));
}
