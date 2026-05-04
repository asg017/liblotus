//! End-to-end smoke test: register the datetime extension and exercise
//! the public formula surface (`DATE`, `YEAR`, `MONTH`, `DAY`,
//! `Date + Span`, comparisons) through a real `Sheet`.

use std::sync::Arc;

use lotus_core::{CellValue, CustomValue, Registry, Sheet};

fn sheet() -> Sheet {
    let mut reg = Registry::new();
    lotus_datetime::register(&mut reg).unwrap();
    Sheet::new_with_registry(Arc::new(reg))
}

#[test]
fn date_literal_parsed_as_jdate() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "2025-04-27".into())]).unwrap();
    match s.get("A1") {
        CellValue::Custom(cv) => {
            assert_eq!(cv.type_tag, "jdate");
            assert_eq!(cv.data, "2025-04-27");
        }
        other => panic!("expected jdate, got {other:?}"),
    }
}

#[test]
fn date_function_constructs_jdate() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "=DATE(2025, 4, 27)".into())]).unwrap();
    assert_eq!(
        s.get("A1"),
        CellValue::Custom(CustomValue { type_tag: "jdate".into(), data: "2025-04-27".into() })
    );
}

#[test]
fn invalid_calendar_date_surfaces_value_error() {
    let mut s = sheet();
    s.set_cells(&[("A1".into(), "=DATE(2025, 2, 30)".into())]).unwrap();
    let v = s.get("A1");
    assert!(v.is_error(), "expected error, got {v:?}");
    assert!(v.to_string().starts_with("#VALUE!"), "got {v}");
}

#[test]
fn year_month_day_round_trip_through_a_sheet() {
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

// Note: Date+Span operator dispatch is covered by unit tests in
// `src/date.rs`. We can't exercise it through `Sheet::set_cells` here
// in T-DT-1 because there's no way to *parse* a jspan literal yet —
// the SpanHandler arrives in T-DT-2 and will own that.

#[test]
fn date_comparisons() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "2025-04-20".into()),
        ("A2".into(), "2025-04-27".into()),
        ("B1".into(), "=A1<A2".into()),
        ("B2".into(), "=A1=A2".into()),
        ("B3".into(), "=A1<>A2".into()),
        ("B4".into(), "=A1<=A1".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Boolean(true));
    assert_eq!(s.get("B2"), CellValue::Boolean(false));
    assert_eq!(s.get("B3"), CellValue::Boolean(true));
    assert_eq!(s.get("B4"), CellValue::Boolean(true));
}

#[test]
fn date_aggregates_via_as_number() {
    // SUM should coerce jdate → days-since-epoch via `as_number`.
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "1970-01-01".into()),
        ("A2".into(), "1970-01-02".into()),
        ("A3".into(), "=SUM(A1:A2)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("A3"), CellValue::Number(1.0));
}
