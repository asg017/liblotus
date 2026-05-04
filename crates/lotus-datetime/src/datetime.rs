use std::str::FromStr;

use jiff::civil::DateTime;
use lotus_core::{BinaryOp, CellValue, CompareOp, CustomTypeHandler, CustomValue};

use crate::span::unpack_span;
use crate::tags;

/// Custom-type handler for `jdatetime` (a `jiff::civil::DateTime`).
///
/// `data` is the canonical ISO 8601 form (`YYYY-MM-DDTHH:MM:SS[.fff…]`).
/// `display` swaps the `T` for a space for human readability — the edit
/// repr keeps the canonical form for round-trip safety.
///
/// Owns: `jdatetime ± jspan` → `jdatetime`. `jdatetime - jdatetime` is
/// owned by `SpanHandler`.
pub struct DateTimeHandler;

const NANOS_PER_DAY: f64 = 86_400.0 * 1_000_000_000.0;

pub(crate) fn pack_datetime(dt: DateTime) -> CustomValue {
    CustomValue { type_tag: tags::DATETIME.into(), data: dt.to_string() }
}

pub(crate) fn unpack_datetime(cv: &CustomValue) -> Option<Result<DateTime, String>> {
    if cv.type_tag != tags::DATETIME {
        return None;
    }
    Some(DateTime::from_str(&cv.data).map_err(|e| e.to_string()))
}

/// Cheap shape check: `YYYY-MM-DDT…` (16+ bytes, T at index 10). Crucially
/// rejects strings that contain `[` (a zoned datetime, claimed first by
/// the higher-priority ZonedHandler) so we don't double-claim.
fn looks_like_datetime(raw: &str) -> bool {
    let b = raw.as_bytes();
    if b.len() < 16 {
        return false;
    }
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return false;
    }
    if b.contains(&b'[') {
        return false;
    }
    let leading_digits = [0, 1, 2, 3, 5, 6, 8, 9];
    leading_digits.iter().all(|&i| b[i].is_ascii_digit())
}

impl CustomTypeHandler for DateTimeHandler {
    fn type_tag(&self) -> &str {
        tags::DATETIME
    }

    fn parse_literal(&self, raw: &str) -> Option<CustomValue> {
        if !looks_like_datetime(raw) {
            return None;
        }
        DateTime::from_str(raw).ok().map(pack_datetime)
    }

    /// Probe API: empty options falls back to `parse_literal`, non-empty
    /// options is treated as a jiff `strptime` pattern. See
    /// [`crate::DateHandler::parse_with`] for the design.
    fn parse_with(&self, raw: &str, options: &str) -> Option<CustomValue> {
        if options.is_empty() {
            return self.parse_literal(raw);
        }
        DateTime::strptime(options, raw).ok().map(pack_datetime)
    }

    fn display(&self, v: &CustomValue) -> String {
        // Replace the canonical 'T' separator with a space — purely
        // cosmetic. Only the first occurrence (the date/time joiner)
        // matters; later 'T' characters can't appear in valid ISO data.
        v.data.replacen('T', " ", 1)
    }

    fn edit_repr(&self, v: &CustomValue) -> String {
        v.data.clone()
    }

    fn as_number(&self, v: &CustomValue) -> Option<f64> {
        // Fractional days since 1970-01-01T00:00:00 (UTC interpretation,
        // i.e. wall clock pretending to be UTC; jzoned does the actual
        // tz-aware version).
        const EPOCH: DateTime = DateTime::constant(1970, 1, 1, 0, 0, 0, 0);
        let dt = DateTime::from_str(&v.data).ok()?;
        let dur = dt.duration_since(EPOCH);
        let nanos = i128::from(dur.as_secs()) * 1_000_000_000 + i128::from(dur.subsec_nanos());
        Some(nanos as f64 / NANOS_PER_DAY)
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
                (tags::DATETIME, tags::SPAN) => Some(dt_plus_span(a, b)),
                (tags::SPAN, tags::DATETIME) => Some(dt_plus_span(b, a)),
                _ => None,
            },
            BinaryOp::Sub if a.type_tag == tags::DATETIME && b.type_tag == tags::SPAN => {
                Some(dt_minus_span(a, b))
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
        if a.type_tag != tags::DATETIME || b.type_tag != tags::DATETIME {
            return None;
        }
        let da = match unpack_datetime(a)? {
            Ok(d) => d,
            Err(e) => return Some(Err(e)),
        };
        let db = match unpack_datetime(b)? {
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

fn dt_plus_span(dt_cv: &CustomValue, span_cv: &CustomValue) -> Result<CellValue, String> {
    let dt = unpack_datetime(dt_cv).expect("checked tag")?;
    let s = unpack_span(span_cv).expect("checked tag")?;
    let out = dt.checked_add(s).map_err(|e| e.to_string())?;
    Ok(CellValue::Custom(pack_datetime(out)))
}

fn dt_minus_span(dt_cv: &CustomValue, span_cv: &CustomValue) -> Result<CellValue, String> {
    let dt = unpack_datetime(dt_cv).expect("checked tag")?;
    let s = unpack_span(span_cv).expect("checked tag")?;
    let out = dt.checked_sub(s).map_err(|e| e.to_string())?;
    Ok(CellValue::Custom(pack_datetime(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jdt_cv(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::DATETIME.into(), data: s.into() })
    }

    fn jspan_cv(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::SPAN.into(), data: s.into() })
    }

    #[test]
    fn parse_iso_datetime() {
        let cv = DateTimeHandler.parse_literal("2025-04-27T14:30:00").unwrap();
        assert_eq!(cv.type_tag, "jdatetime");
        assert_eq!(cv.data, "2025-04-27T14:30:00");
    }

    #[test]
    fn parse_rejects_dates_zoneds_etc() {
        assert!(DateTimeHandler.parse_literal("2025-04-27").is_none());
        assert!(DateTimeHandler
            .parse_literal("2025-04-27T14:30:00-04:00[America/New_York]")
            .is_none());
        assert!(DateTimeHandler.parse_literal("hello").is_none());
        assert!(DateTimeHandler.parse_literal("14:30:00").is_none());
    }

    #[test]
    fn display_uses_space_separator() {
        let cv = pack_datetime(DateTime::constant(2025, 4, 27, 14, 30, 0, 0));
        assert_eq!(DateTimeHandler.display(&cv), "2025-04-27 14:30:00");
    }

    #[test]
    fn edit_repr_keeps_canonical_t() {
        let cv = pack_datetime(DateTime::constant(2025, 4, 27, 14, 30, 0, 0));
        assert_eq!(DateTimeHandler.edit_repr(&cv), "2025-04-27T14:30:00");
    }

    #[test]
    fn as_number_unix_epoch_days_fractional() {
        // Epoch noon → 0.5.
        assert_eq!(
            DateTimeHandler.as_number(&pack_datetime(DateTime::constant(1970, 1, 1, 12, 0, 0, 0))),
            Some(0.5)
        );
        // Epoch + 1 day → 1.0.
        assert_eq!(
            DateTimeHandler.as_number(&pack_datetime(DateTime::constant(1970, 1, 2, 0, 0, 0, 0))),
            Some(1.0)
        );
        // Day before epoch noon → -0.5.
        assert_eq!(
            DateTimeHandler.as_number(&pack_datetime(DateTime::constant(1969, 12, 31, 12, 0, 0, 0))),
            Some(-0.5)
        );
    }

    #[test]
    fn datetime_plus_span() {
        let CellValue::Custom(out) = DateTimeHandler
            .binary_op(BinaryOp::Add, &jdt_cv("2025-04-27T12:00:00"), &jspan_cv("PT3H"))
            .unwrap()
            .unwrap()
        else {
            panic!("expected jdatetime");
        };
        assert_eq!(out.data, "2025-04-27T15:00:00");
    }

    #[test]
    fn datetime_plus_calendar_span() {
        let CellValue::Custom(out) = DateTimeHandler
            .binary_op(BinaryOp::Add, &jdt_cv("2025-01-31T12:00:00"), &jspan_cv("P1M"))
            .unwrap()
            .unwrap()
        else {
            panic!("expected jdatetime");
        };
        // jiff clamps Jan 31 + 1 month → Feb 28 (non-leap), preserves time.
        assert_eq!(out.data, "2025-02-28T12:00:00");
    }

    #[test]
    fn compare_orders_chronologically() {
        let h = DateTimeHandler;
        let a = jdt_cv("2025-04-27T12:00:00");
        let b = jdt_cv("2025-04-27T13:00:00");
        let a_dup = a.clone();
        assert!(h.compare(CompareOp::Lt, &a, &b).unwrap().unwrap());
        assert!(h.compare(CompareOp::Eq, &a, &a_dup).unwrap().unwrap());
    }
}
