use std::str::FromStr;

use jiff::{civil::Time, Span};
use lotus_core::{BinaryOp, CellValue, CompareOp, CustomTypeHandler, CustomValue, FormulaError};

use crate::span::unpack_span;
use crate::tags;

/// Custom-type handler for `jtime` (a `jiff::civil::Time`).
///
/// `data` is ISO 8601 (`HH:MM:SS` or `HH:MM:SS.fffffffff`). Subsecond
/// digits round-trip losslessly; jiff trims trailing zeros for us.
///
/// Owns: `jtime ± jspan` → `jtime` (via `wrapping_add`/`wrapping_sub`,
/// so 23:00 + 2h wraps to 01:00). Spans containing calendar units are
/// rejected with `#VALUE!`. `jtime - jtime` is owned by `SpanHandler`.
pub struct TimeHandler;

const NANOS_PER_DAY: f64 = 86_400.0 * 1_000_000_000.0;

pub(crate) fn pack_time(t: Time) -> CustomValue {
    CustomValue { type_tag: tags::TIME.into(), data: t.to_string() }
}

pub(crate) fn unpack_time(cv: &CustomValue) -> Option<Result<Time, String>> {
    if cv.type_tag != tags::TIME {
        return None;
    }
    Some(Time::from_str(&cv.data).map_err(|e| e.to_string()))
}

/// Cheap shape check: 5–18 bytes, `HH:MM` or `HH:MM:SS(.fff…)`,
/// digits/colons/period only. Crucially rejects strings with hyphens
/// (a date) or `T` (a datetime) so the higher-claim-priority handlers
/// keep their slot.
fn looks_like_time(raw: &str) -> bool {
    let b = raw.as_bytes();
    if b.len() < 5 || b.len() > 18 {
        return false;
    }
    if b[2] != b':' {
        return false;
    }
    let allowed = |c: u8| c.is_ascii_digit() || c == b':' || c == b'.';
    b.iter().all(|&c| allowed(c))
}

/// True iff the span has any year/month/week/day component. Time has
/// no calendar context, so adding such a span is meaningless and we
/// reject it instead of silently dropping units.
fn span_has_calendar_units(s: &Span) -> bool {
    s.get_years() != 0 || s.get_months() != 0 || s.get_weeks() != 0 || s.get_days() != 0
}

impl CustomTypeHandler for TimeHandler {
    fn type_tag(&self) -> &str {
        tags::TIME
    }

    fn parse_literal(&self, raw: &str) -> Option<CustomValue> {
        if !looks_like_time(raw) {
            return None;
        }
        Time::from_str(raw).ok().map(pack_time)
    }

    /// Probe API: empty options falls back to `parse_literal`, non-empty
    /// options is treated as a jiff `strptime` pattern. See
    /// [`crate::DateHandler::parse_with`] for the design.
    fn parse_with(&self, raw: &str, options: &str) -> Option<CustomValue> {
        if options.is_empty() {
            return self.parse_literal(raw);
        }
        Time::strptime(options, raw).ok().map(pack_time)
    }

    fn display(&self, v: &CustomValue) -> String {
        v.data.clone()
    }

    fn edit_repr(&self, v: &CustomValue) -> String {
        v.data.clone()
    }

    fn as_number(&self, v: &CustomValue) -> Option<f64> {
        // Fraction of day in [0, 1). 12:00:00 → 0.5.
        let t = Time::from_str(&v.data).ok()?;
        let total_nanos = i64::from(t.hour()) * 3_600_000_000_000
            + i64::from(t.minute()) * 60_000_000_000
            + i64::from(t.second()) * 1_000_000_000
            + i64::from(t.subsec_nanosecond());
        Some(total_nanos as f64 / NANOS_PER_DAY)
    }

    fn binary_op(
        &self,
        op: BinaryOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<CellValue, String>> {
        let (CellValue::Custom(a), CellValue::Custom(b)) = (lhs, rhs) else {
            return None;
        };
        match op {
            BinaryOp::Add => match (a.type_tag.as_str(), b.type_tag.as_str()) {
                (tags::TIME, tags::SPAN) => Some(time_plus_span(a, b)),
                (tags::SPAN, tags::TIME) => Some(time_plus_span(b, a)),
                _ => None,
            },
            BinaryOp::Sub if a.type_tag == tags::TIME && b.type_tag == tags::SPAN => {
                Some(time_minus_span(a, b))
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
        if a.type_tag != tags::TIME || b.type_tag != tags::TIME {
            return None;
        }
        let ta = match unpack_time(a)? {
            Ok(t) => t,
            Err(e) => return Some(Err(e)),
        };
        let tb = match unpack_time(b)? {
            Ok(t) => t,
            Err(e) => return Some(Err(e)),
        };
        Some(Ok(match op {
            CompareOp::Eq => ta == tb,
            CompareOp::Ne => ta != tb,
            CompareOp::Lt => ta < tb,
            CompareOp::Le => ta <= tb,
            CompareOp::Gt => ta > tb,
            CompareOp::Ge => ta >= tb,
        }))
    }
}

fn time_plus_span(time_cv: &CustomValue, span_cv: &CustomValue) -> Result<CellValue, String> {
    let t = unpack_time(time_cv).expect("checked tag")?;
    let s = unpack_span(span_cv).expect("checked tag")?;
    if span_has_calendar_units(&s) {
        return Err(FormulaError::value(
            "jtime + jspan: span has calendar units (years/months/weeks/days), which time has no anchor for",
        )
        .to_string());
    }
    Ok(CellValue::Custom(pack_time(t.wrapping_add(s))))
}

fn time_minus_span(time_cv: &CustomValue, span_cv: &CustomValue) -> Result<CellValue, String> {
    let t = unpack_time(time_cv).expect("checked tag")?;
    let s = unpack_span(span_cv).expect("checked tag")?;
    if span_has_calendar_units(&s) {
        return Err(FormulaError::value(
            "jtime - jspan: span has calendar units (years/months/weeks/days), which time has no anchor for",
        )
        .to_string());
    }
    Ok(CellValue::Custom(pack_time(t.wrapping_sub(s))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jtime_cv(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::TIME.into(), data: s.into() })
    }

    fn jspan_cv(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::SPAN.into(), data: s.into() })
    }

    #[test]
    fn parse_iso_time() {
        let cv = TimeHandler.parse_literal("14:30:00").unwrap();
        assert_eq!(cv.type_tag, "jtime");
        assert_eq!(cv.data, "14:30:00");
    }

    #[test]
    fn parse_rejects_dates_and_datetimes() {
        assert!(TimeHandler.parse_literal("2025-04-27").is_none());
        assert!(TimeHandler.parse_literal("2025-04-27T14:30:00").is_none());
        assert!(TimeHandler.parse_literal("hello").is_none());
    }

    #[test]
    fn parse_handles_subsecond() {
        let cv = TimeHandler.parse_literal("14:30:00.5").unwrap();
        assert_eq!(cv.type_tag, "jtime");
        // Round-trip via jiff to canonicalise the data string.
        let t = Time::from_str(&cv.data).unwrap();
        assert_eq!(t.subsec_nanosecond(), 500_000_000);
    }

    #[test]
    fn as_number_fraction_of_day() {
        // Noon = 0.5.
        assert_eq!(TimeHandler.as_number(&pack_time(Time::constant(12, 0, 0, 0))), Some(0.5));
        // Midnight = 0.
        assert_eq!(TimeHandler.as_number(&pack_time(Time::constant(0, 0, 0, 0))), Some(0.0));
        // 06:00 = 0.25.
        assert_eq!(TimeHandler.as_number(&pack_time(Time::constant(6, 0, 0, 0))), Some(0.25));
    }

    #[test]
    fn time_plus_hours_wraps_at_midnight() {
        let CellValue::Custom(out) = TimeHandler
            .binary_op(BinaryOp::Add, &jtime_cv("23:00:00"), &jspan_cv("PT2H"))
            .unwrap()
            .unwrap()
        else {
            panic!("expected jtime");
        };
        assert_eq!(out.data, "01:00:00");
    }

    #[test]
    fn time_plus_calendar_span_rejected() {
        let err = TimeHandler
            .binary_op(BinaryOp::Add, &jtime_cv("12:00:00"), &jspan_cv("P1D"))
            .unwrap()
            .unwrap_err();
        assert!(err.contains("calendar units"), "got: {err}");
    }

    #[test]
    fn time_minus_span() {
        let CellValue::Custom(out) = TimeHandler
            .binary_op(BinaryOp::Sub, &jtime_cv("14:30:00"), &jspan_cv("PT30M"))
            .unwrap()
            .unwrap()
        else {
            panic!("expected jtime");
        };
        assert_eq!(out.data, "14:00:00");
    }

    #[test]
    fn compare_orders_chronologically() {
        let h = TimeHandler;
        let early = jtime_cv("06:00:00");
        let early_dup = early.clone();
        let late = jtime_cv("23:00:00");
        assert!(h.compare(CompareOp::Lt, &early, &late).unwrap().unwrap());
        assert!(h.compare(CompareOp::Eq, &early, &early_dup).unwrap().unwrap());
        assert!(h.compare(CompareOp::Ge, &late, &early).unwrap().unwrap());
    }
}
