use std::cmp::Ordering;
use std::str::FromStr;

use jiff::{civil::Date, Span};
use lotus_core::{BinaryOp, CellValue, CompareOp, CustomTypeHandler, CustomValue};

use crate::datetime::unpack_datetime;
use crate::tags;
use crate::time::unpack_time;
use crate::zoned::unpack_zoned;

/// Custom-type handler for `jspan` (a `jiff::Span`).
///
/// `data` is jiff's canonical ISO 8601 duration form (`P1Y2M3DT4H5M6S`)
/// — what `Span::Display` produces and `Span::from_str` accepts. Friendly
/// rendering happens in `display`.
///
/// Owns these binary ops (per the result-type-owns-dispatch convention):
///
/// - `jspan ± jspan` → `jspan`
/// - `jspan * Number` → `jspan` (integer scalar; non-integer is `#VALUE!`)
/// - `jdate - jdate` → `jspan`         (cross-type)
/// - `jtime - jtime` → `jspan`         (cross-type)
/// - `jdatetime - jdatetime` → `jspan` (cross-type)
/// - `jzoned - jzoned` → `jspan`       (cross-type, DST-aware)
pub struct SpanHandler;

/// Reference date for span↔SignedDuration conversion. The Unix epoch is
/// arbitrary but consistent — comparing two spans always uses the same
/// relative anchor so equality/ordering is deterministic.
pub(crate) const EPOCH: Date = Date::constant(1970, 1, 1);

/// Wrap a `Span` in a `CustomValue` with the canonical ISO repr.
pub(crate) fn pack_span(s: Span) -> CustomValue {
    CustomValue { type_tag: tags::SPAN.into(), data: s.to_string() }
}

/// Try to extract a `Span` from a `CustomValue` whose tag is `jspan`.
pub(crate) fn unpack_span(cv: &CustomValue) -> Option<Result<Span, String>> {
    if cv.type_tag != tags::SPAN {
        return None;
    }
    Some(Span::from_str(&cv.data).map_err(|e| e.to_string()))
}

/// Cheap shape check before invoking jiff's parser. Accepts:
///   - ISO 8601 form: starts with `P` or `-P` / `+P`
///   - jiff "friendly" form: starts with a digit followed by a unit token
///     (`1d`, `30 minutes`, `5h 30m`)
fn looks_like_span(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    let Some(&first) = bytes.first() else { return false };
    if matches!(first, b'P' | b'+' | b'-' | b'p') {
        // ISO form. Cheap accept; jiff will reject malformed bodies.
        return true;
    }
    // Friendly form: digit, optional digits/spaces, then a letter unit.
    if !first.is_ascii_digit() {
        return false;
    }
    let has_unit_letter = bytes
        .iter()
        .skip(1)
        .any(|b| matches!(b, b'y' | b'w' | b'd' | b'h' | b'm' | b's'));
    has_unit_letter
}

impl CustomTypeHandler for SpanHandler {
    fn type_tag(&self) -> &str {
        tags::SPAN
    }

    fn parse_literal(&self, raw: &str) -> Option<CustomValue> {
        if !looks_like_span(raw) {
            return None;
        }
        Span::from_str(raw).ok().map(pack_span)
    }

    fn display(&self, v: &CustomValue) -> String {
        // Jiff's friendly format renders as `1y 2mo 3d 4h 5m`. Falls back
        // to ISO if the data didn't parse — that shouldn't happen if the
        // value was constructed via this handler, but a hand-built
        // `CustomValue` with garbage in `data` won't panic.
        match Span::from_str(&v.data) {
            Ok(s) => format!("{s:#}"),
            Err(_) => v.data.clone(),
        }
    }

    fn edit_repr(&self, v: &CustomValue) -> String {
        // Canonical ISO. Round-trips cleanly through `parse_literal`.
        v.data.clone()
    }

    fn as_number(&self, _v: &CustomValue) -> Option<f64> {
        // Calendar units (months, years) don't have a fixed second-count
        // — converting requires a relative date. We don't have one here,
        // so we force the user to call `SPAN_TO_SECONDS(span, [relative])`
        // explicitly instead of silently picking a possibly-wrong anchor.
        None
    }

    fn binary_op(
        &self,
        op: BinaryOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<CellValue, String>> {
        match (lhs, rhs) {
            // Cross-type subtractions whose result is a span — we own these.
            (CellValue::Custom(a), CellValue::Custom(b))
                if op == BinaryOp::Sub
                    && a.type_tag == b.type_tag
                    && matches!(
                        a.type_tag.as_str(),
                        tags::DATE | tags::TIME | tags::DATETIME | tags::ZONED
                    ) =>
            {
                Some(temporal_minus_temporal(a, b))
            }
            // jspan + jspan, jspan - jspan
            (CellValue::Custom(a), CellValue::Custom(b))
                if a.type_tag == tags::SPAN && b.type_tag == tags::SPAN =>
            {
                match op {
                    BinaryOp::Add => Some(span_combine(a, b, /*sub=*/ false)),
                    BinaryOp::Sub => Some(span_combine(a, b, /*sub=*/ true)),
                    _ => None,
                }
            }
            // jspan * Number / Number * jspan
            (CellValue::Custom(a), b) if op == BinaryOp::Mul && a.type_tag == tags::SPAN => {
                Some(span_times_number(a, b))
            }
            (a, CellValue::Custom(b)) if op == BinaryOp::Mul && b.type_tag == tags::SPAN => {
                Some(span_times_number(b, a))
            }
            _ => None,
        }
    }

    fn compare(
        &self,
        op: CompareOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<bool, String>> {
        let (CellValue::Custom(a), CellValue::Custom(b)) = (lhs, rhs) else {
            return None;
        };
        if a.type_tag != tags::SPAN || b.type_tag != tags::SPAN {
            return None;
        }
        let sa = match unpack_span(a)? {
            Ok(s) => s,
            Err(e) => return Some(Err(e)),
        };
        let sb = match unpack_span(b)? {
            Ok(s) => s,
            Err(e) => return Some(Err(e)),
        };
        // Compare by total duration relative to a fixed anchor (the Unix
        // epoch). This makes `1.month() == 30.days()` *false* when the
        // relative date is Feb (28 days) but lets us give a deterministic
        // answer in every case.
        let ord = match sa.compare((sb, EPOCH)) {
            Ok(o) => o,
            Err(e) => return Some(Err(e.to_string())),
        };
        Some(Ok(match op {
            CompareOp::Eq => ord == Ordering::Equal,
            CompareOp::Ne => ord != Ordering::Equal,
            CompareOp::Lt => ord == Ordering::Less,
            CompareOp::Le => ord != Ordering::Greater,
            CompareOp::Gt => ord == Ordering::Greater,
            CompareOp::Ge => ord != Ordering::Less,
        }))
    }
}

/// Dispatches on the (matching) type tag of two temporal values to
/// produce a span via the appropriate jiff `since` call. Caller has
/// already validated `a.type_tag == b.type_tag` and that the tag is
/// one of `jdate` / `jtime` / `jdatetime`.
fn temporal_minus_temporal(
    later_cv: &CustomValue,
    earlier_cv: &CustomValue,
) -> Result<CellValue, String> {
    let span = match later_cv.type_tag.as_str() {
        tags::DATE => {
            let later = crate::date::unpack_date(later_cv).expect("checked tag")?;
            let earlier = crate::date::unpack_date(earlier_cv).expect("checked tag")?;
            later.since(earlier).map_err(|e| e.to_string())?
        }
        tags::TIME => {
            let later = unpack_time(later_cv).expect("checked tag")?;
            let earlier = unpack_time(earlier_cv).expect("checked tag")?;
            later.since(earlier).map_err(|e| e.to_string())?
        }
        tags::DATETIME => {
            let later = unpack_datetime(later_cv).expect("checked tag")?;
            let earlier = unpack_datetime(earlier_cv).expect("checked tag")?;
            later.since(earlier).map_err(|e| e.to_string())?
        }
        tags::ZONED => {
            let later = unpack_zoned(later_cv).expect("checked tag")?;
            let earlier = unpack_zoned(earlier_cv).expect("checked tag")?;
            // `Zoned::since` honours DST when computing the span.
            later.since(&earlier).map_err(|e| e.to_string())?
        }
        // Caller guards against this — `unreachable!` is the right answer
        // because reaching here means the dispatch matrix and the inner
        // arm have drifted apart.
        other => unreachable!("temporal_minus_temporal called with tag {other}"),
    };
    Ok(CellValue::Custom(pack_span(span)))
}

fn span_combine(a_cv: &CustomValue, b_cv: &CustomValue, sub: bool) -> Result<CellValue, String> {
    let a = unpack_span(a_cv).expect("checked tag")?;
    let b = unpack_span(b_cv).expect("checked tag")?;
    let out = if sub {
        a.checked_sub((b, EPOCH))
    } else {
        a.checked_add((b, EPOCH))
    };
    Ok(CellValue::Custom(pack_span(out.map_err(|e| e.to_string())?)))
}

fn span_times_number(span_cv: &CustomValue, num: &CellValue) -> Result<CellValue, String> {
    let n = num.as_number().ok_or_else(|| {
        lotus_core::FormulaError::value("span * value: scalar must be a number").to_string()
    })?;
    if !n.is_finite() || n.fract() != 0.0 {
        return Err(lotus_core::FormulaError::value(
            "span * value: scalar must be a whole number (fractional scaling not supported in v1)",
        )
        .to_string());
    }
    let n = n as i64;
    let s = unpack_span(span_cv).expect("checked tag")?;
    let out = s.checked_mul(n).map_err(|e| e.to_string())?;
    Ok(CellValue::Custom(pack_span(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jspan_cv(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::SPAN.into(), data: s.into() })
    }

    fn jdate_cv(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::DATE.into(), data: s.into() })
    }

    #[test]
    fn parse_iso_span() {
        let cv = SpanHandler.parse_literal("P1Y2M3DT4H").unwrap();
        assert_eq!(cv.type_tag, "jspan");
        // Re-parsed and re-serialised so we don't depend on capitalisation.
        let parsed = Span::from_str(&cv.data).unwrap();
        assert_eq!(parsed.get_years(), 1);
        assert_eq!(parsed.get_months(), 2);
        assert_eq!(parsed.get_days(), 3);
        assert_eq!(parsed.get_hours(), 4);
    }

    #[test]
    fn parse_friendly_span() {
        let cv = SpanHandler.parse_literal("5h 30m").unwrap();
        let parsed = Span::from_str(&cv.data).unwrap();
        assert_eq!(parsed.get_hours(), 5);
        assert_eq!(parsed.get_minutes(), 30);
    }

    #[test]
    fn parse_rejects_non_spans() {
        assert!(SpanHandler.parse_literal("hello").is_none());
        assert!(SpanHandler.parse_literal("42").is_none());
        assert!(SpanHandler.parse_literal("2025-04-27").is_none());
    }

    #[test]
    fn display_uses_friendly() {
        let cv = pack_span(Span::new().days(3).hours(4));
        let rendered = SpanHandler.display(&cv);
        // Friendly format uses single-letter / short tokens. We just check
        // it doesn't start with `P` (which would mean ISO).
        assert!(!rendered.starts_with('P'), "got {rendered}");
        assert!(rendered.contains('3'));
        assert!(rendered.contains('4'));
    }

    #[test]
    fn date_minus_date_returns_span() {
        let CellValue::Custom(out) = SpanHandler
            .binary_op(BinaryOp::Sub, &jdate_cv("2025-04-27"), &jdate_cv("2025-04-20"))
            .unwrap()
            .unwrap()
        else {
            panic!("expected custom");
        };
        let parsed = Span::from_str(&out.data).unwrap();
        // jiff's Date::since with default options yields a single-unit
        // span for two dates close together. Total duration is 7 days
        // either way.
        let total_days =
            parsed.to_duration(EPOCH).unwrap().as_secs() / 86_400;
        assert_eq!(total_days, 7);
    }

    #[test]
    fn span_plus_span() {
        let out = SpanHandler
            .binary_op(BinaryOp::Add, &jspan_cv("P3D"), &jspan_cv("P4D"))
            .unwrap()
            .unwrap();
        let CellValue::Custom(cv) = out else {
            panic!("expected custom");
        };
        let parsed = Span::from_str(&cv.data).unwrap();
        let total_days =
            parsed.to_duration(EPOCH).unwrap().as_secs() / 86_400;
        assert_eq!(total_days, 7);
    }

    #[test]
    fn span_minus_span() {
        let out = SpanHandler
            .binary_op(BinaryOp::Sub, &jspan_cv("P10D"), &jspan_cv("P3D"))
            .unwrap()
            .unwrap();
        let CellValue::Custom(cv) = out else {
            panic!("expected custom");
        };
        let parsed = Span::from_str(&cv.data).unwrap();
        let total_days =
            parsed.to_duration(EPOCH).unwrap().as_secs() / 86_400;
        assert_eq!(total_days, 7);
    }

    #[test]
    fn span_times_integer() {
        let out = SpanHandler
            .binary_op(BinaryOp::Mul, &jspan_cv("P3D"), &CellValue::Number(4.0))
            .unwrap()
            .unwrap();
        let CellValue::Custom(cv) = out else {
            panic!("expected custom");
        };
        let parsed = Span::from_str(&cv.data).unwrap();
        assert_eq!(parsed.get_days(), 12);
    }

    #[test]
    fn span_times_integer_commutes() {
        let lhs = SpanHandler
            .binary_op(BinaryOp::Mul, &jspan_cv("P3D"), &CellValue::Number(4.0))
            .unwrap()
            .unwrap();
        let rhs = SpanHandler
            .binary_op(BinaryOp::Mul, &CellValue::Number(4.0), &jspan_cv("P3D"))
            .unwrap()
            .unwrap();
        assert_eq!(lhs, rhs);
    }

    #[test]
    fn span_times_fractional_rejected() {
        let err = SpanHandler
            .binary_op(BinaryOp::Mul, &jspan_cv("P3D"), &CellValue::Number(2.5))
            .unwrap()
            .unwrap_err();
        assert!(err.contains("whole number"), "got: {err}");
    }

    #[test]
    fn compare_orders_by_duration() {
        let h = SpanHandler;
        let three = jspan_cv("P3D");
        let three_dup = three.clone();
        let seven = jspan_cv("P7D");
        assert!(h.compare(CompareOp::Lt, &three, &seven).unwrap().unwrap());
        assert!(h.compare(CompareOp::Eq, &three, &three_dup).unwrap().unwrap());
        assert!(h.compare(CompareOp::Gt, &seven, &three).unwrap().unwrap());
    }
}
