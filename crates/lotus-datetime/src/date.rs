use std::str::FromStr;

use jiff::civil::Date;
use lotus_core::{BinaryOp, CellValue, CompareOp, CustomTypeHandler, CustomValue};

use crate::span::unpack_span;
use crate::tags;

/// Custom-type handler for `jdate` (a `jiff::civil::Date`).
///
/// `data` is the canonical ISO 8601 form `YYYY-MM-DD`. The handler
/// re-parses on every dispatch — cheap (10-byte string, no allocation
/// needed inside jiff's parser) and avoids any in-memory caching layer.
pub struct DateHandler;

/// Days in 86_400 seconds — used to convert `as_number` output from
/// "seconds since 1970-01-01" to "days since 1970-01-01".
const SECONDS_PER_DAY: f64 = 86_400.0;

/// Wrap `Date` in a `CustomValue` with the canonical ISO repr.
pub(crate) fn pack_date(d: Date) -> CustomValue {
    CustomValue { type_tag: tags::DATE.into(), data: d.to_string() }
}

/// Try to extract a `Date` from a `CustomValue` whose tag is `jdate`.
/// Returns `Err` with the parse error string if the tag matches but the
/// data is malformed; returns `Ok(None)` if the tag isn't `jdate`.
pub(crate) fn unpack_date(cv: &CustomValue) -> Option<Result<Date, String>> {
    if cv.type_tag != tags::DATE {
        return None;
    }
    Some(Date::from_str(&cv.data).map_err(|e| e.to_string()))
}

/// Cheap shape check: exactly 10 bytes, digits/hyphens only, `Y-M-D`
/// hyphen positions. Avoids invoking jiff's parser on inputs that
/// clearly aren't dates (`"42"`, `"hello"`, `"2025-04-27T12:00"`).
fn looks_like_iso_date(raw: &str) -> bool {
    let b = raw.as_bytes();
    if b.len() != 10 {
        return false;
    }
    if b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = [0, 1, 2, 3, 5, 6, 8, 9];
    digits.iter().all(|&i| b[i].is_ascii_digit())
}

impl CustomTypeHandler for DateHandler {
    fn type_tag(&self) -> &str {
        tags::DATE
    }

    fn parse_literal(&self, raw: &str) -> Option<CustomValue> {
        if !looks_like_iso_date(raw) {
            return None;
        }
        Date::from_str(raw).ok().map(pack_date)
    }

    /// Probe API: with an empty `options` string, behaves identically to
    /// `parse_literal` (the ingestion-pipeline fast path). With a
    /// non-empty `options`, treats it as a `strptime` pattern and tries
    /// to parse `raw` against it — e.g. `parse_with("4/2", "%d/%m")`
    /// yields a date for Feb 4. Embedders use this to disambiguate
    /// shorthand input (`"2/4"`) before writing the result via
    /// `set_cells_typed`.
    fn parse_with(&self, raw: &str, options: &str) -> Option<CustomValue> {
        if options.is_empty() {
            return self.parse_literal(raw);
        }
        Date::strptime(options, raw).ok().map(pack_date)
    }

    fn display(&self, v: &CustomValue) -> String {
        v.data.clone()
    }

    fn edit_repr(&self, v: &CustomValue) -> String {
        v.data.clone()
    }

    fn as_number(&self, v: &CustomValue) -> Option<f64> {
        // Days since the Unix epoch (1970-01-01). Negative for earlier
        // dates. Excel users beware: this is *not* the Excel serial
        // (which counts from 1900-01-00 with a fictional 1900-02-29).
        const EPOCH: Date = Date::constant(1970, 1, 1);
        let date = Date::from_str(&v.data).ok()?;
        Some(date.duration_since(EPOCH).as_secs() as f64 / SECONDS_PER_DAY)
    }

    fn binary_op(
        &self,
        op: BinaryOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<CellValue, String>> {
        // We own:
        //   jdate + jspan = jdate
        //   jspan + jdate = jdate    (commutative `+`)
        //   jdate - jspan = jdate
        // We do NOT own jdate - jdate (that's a span, owned by SpanHandler in T-DT-2).
        let (CellValue::Custom(a), CellValue::Custom(b)) = (lhs, rhs) else {
            return None;
        };
        match op {
            BinaryOp::Add => match (a.type_tag.as_str(), b.type_tag.as_str()) {
                (tags::DATE, tags::SPAN) => Some(date_plus_span(a, b)),
                (tags::SPAN, tags::DATE) => Some(date_plus_span(b, a)),
                _ => None,
            },
            BinaryOp::Sub if a.type_tag == tags::DATE && b.type_tag == tags::SPAN => {
                Some(date_minus_span(a, b))
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
        if a.type_tag != tags::DATE || b.type_tag != tags::DATE {
            return None;
        }
        let da = match unpack_date(a)? {
            Ok(d) => d,
            Err(e) => return Some(Err(e)),
        };
        let db = match unpack_date(b)? {
            Ok(d) => d,
            Err(e) => return Some(Err(e)),
        };
        Some(Ok(match op {
            CompareOp::Eq => da == db,
            CompareOp::Ne => da != db,
            CompareOp::Lt => da < db,
            CompareOp::Le => da <= db,
            CompareOp::Gt => da > db,
            CompareOp::Ge => da >= db,
        }))
    }
}

fn date_plus_span(date_cv: &CustomValue, span_cv: &CustomValue) -> Result<CellValue, String> {
    let date = unpack_date(date_cv).expect("checked tag")?;
    let span = unpack_span(span_cv).expect("checked tag")?;
    let out = date.checked_add(span).map_err(|e| e.to_string())?;
    Ok(CellValue::Custom(pack_date(out)))
}

fn date_minus_span(date_cv: &CustomValue, span_cv: &CustomValue) -> Result<CellValue, String> {
    let date = unpack_date(date_cv).expect("checked tag")?;
    let span = unpack_span(span_cv).expect("checked tag")?;
    let out = date.checked_sub(span).map_err(|e| e.to_string())?;
    Ok(CellValue::Custom(pack_date(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jdate(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::DATE.into(), data: s.into() })
    }

    fn jspan(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::SPAN.into(), data: s.into() })
    }

    #[test]
    fn parse_literal_accepts_iso_date() {
        let cv = DateHandler.parse_literal("2025-04-27").unwrap();
        assert_eq!(cv.type_tag, "jdate");
        assert_eq!(cv.data, "2025-04-27");
    }

    #[test]
    fn parse_literal_rejects_non_dates() {
        assert!(DateHandler.parse_literal("hello").is_none());
        assert!(DateHandler.parse_literal("42").is_none());
        assert!(DateHandler.parse_literal("2025-04-27T12:00").is_none());
        assert!(DateHandler.parse_literal("2025/04/27").is_none());
    }

    #[test]
    fn parse_with_strptime_pattern() {
        // The driving use case: "4/2" is ambiguous, but with a strftime
        // pattern the embedder can disambiguate before writing the cell.
        let cv = DateHandler.parse_with("4/2/2026", "%d/%m/%Y").unwrap();
        assert_eq!(cv.type_tag, "jdate");
        assert_eq!(cv.data, "2026-02-04");

        // Same string, US-style → April 2nd.
        let cv = DateHandler.parse_with("4/2/2026", "%m/%d/%Y").unwrap();
        assert_eq!(cv.data, "2026-04-02");
    }

    #[test]
    fn parse_with_empty_options_matches_parse_literal() {
        // Empty options ⇒ same gate as parse_literal (ISO-only).
        assert_eq!(
            DateHandler.parse_with("2025-04-27", "").map(|cv| cv.data),
            Some("2025-04-27".into())
        );
        assert!(DateHandler.parse_with("4/2", "").is_none());
    }

    #[test]
    fn parse_with_pattern_mismatch_returns_none() {
        // Pattern doesn't match the input — handler declines, no panic.
        assert!(DateHandler.parse_with("hello", "%d/%m/%Y").is_none());
        assert!(DateHandler.parse_with("4/2/2026", "%Y-%m-%d").is_none());
    }

    #[test]
    fn parse_literal_rejects_invalid_calendar_date() {
        // Looks like an ISO date but Feb 30 doesn't exist — jiff's parser
        // rejects it, so we decline rather than misparse.
        assert!(DateHandler.parse_literal("2025-02-30").is_none());
    }

    #[test]
    fn as_number_unix_epoch_days() {
        assert_eq!(DateHandler.as_number(&pack_date(Date::constant(1970, 1, 1))), Some(0.0));
        assert_eq!(DateHandler.as_number(&pack_date(Date::constant(1970, 1, 2))), Some(1.0));
        assert_eq!(DateHandler.as_number(&pack_date(Date::constant(1969, 12, 31))), Some(-1.0));
    }

    #[test]
    fn date_plus_span_adds_calendar_units() {
        let CellValue::Custom(out) = DateHandler
            .binary_op(BinaryOp::Add, &jdate("2025-01-31"), &jspan("P1M"))
            .unwrap()
            .unwrap()
        else {
            panic!("expected custom");
        };
        // jiff clamps Jan 31 + 1 month to Feb 28 (or 29 in leap years).
        assert_eq!(out.data, "2025-02-28");
    }

    #[test]
    fn date_plus_span_commutes() {
        let lhs = DateHandler
            .binary_op(BinaryOp::Add, &jdate("2025-04-27"), &jspan("P3D"))
            .unwrap()
            .unwrap();
        let rhs = DateHandler
            .binary_op(BinaryOp::Add, &jspan("P3D"), &jdate("2025-04-27"))
            .unwrap()
            .unwrap();
        assert_eq!(lhs, rhs);
    }

    #[test]
    fn date_minus_span_subtracts() {
        let CellValue::Custom(out) = DateHandler
            .binary_op(BinaryOp::Sub, &jdate("2025-04-27"), &jspan("P10D"))
            .unwrap()
            .unwrap()
        else {
            panic!("expected custom");
        };
        assert_eq!(out.data, "2025-04-17");
    }

    #[test]
    fn date_minus_date_declined() {
        // SpanHandler will own this in T-DT-2.
        assert!(DateHandler
            .binary_op(BinaryOp::Sub, &jdate("2025-04-27"), &jdate("2025-04-20"))
            .is_none());
    }

    #[test]
    fn compare_orders_chronologically() {
        let h = DateHandler;
        let earlier = jdate("2025-04-20");
        let later = jdate("2025-04-27");
        let earlier_dup = earlier.clone();
        assert!(h.compare(CompareOp::Lt, &earlier, &later).unwrap().unwrap());
        assert!(!h.compare(CompareOp::Gt, &earlier, &later).unwrap().unwrap());
        assert!(h.compare(CompareOp::Eq, &earlier, &earlier_dup).unwrap().unwrap());
        assert!(h.compare(CompareOp::Ne, &earlier, &later).unwrap().unwrap());
        assert!(h.compare(CompareOp::Le, &earlier, &earlier_dup).unwrap().unwrap());
        assert!(h.compare(CompareOp::Ge, &later, &earlier).unwrap().unwrap());
    }

    #[test]
    fn compare_declines_cross_type() {
        let h = DateHandler;
        // Comparing a date to anything that isn't a date — fall through.
        assert!(h
            .compare(CompareOp::Eq, &jdate("2025-04-27"), &CellValue::Number(45774.0))
            .is_none());
        assert!(h
            .compare(CompareOp::Eq, &jdate("2025-04-27"), &jspan("P1D"))
            .is_none());
    }
}
