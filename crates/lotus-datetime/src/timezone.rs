use jiff::tz::{Offset, TimeZone};
use lotus_core::{CellValue, CompareOp, CustomTypeHandler, CustomValue};

use crate::tags;

/// Custom-type handler for `jtimezone` (a `jiff::tz::TimeZone`).
///
/// `data` is either an IANA name (`America/New_York`, `UTC`) or a fixed
/// offset rendered `+HH:MM` / `-HH:MM`. The handler does NOT define
/// `parse_literal` — strings like `"UTC"` should not be silently
/// reinterpreted as a time zone if the user just wanted text. Use the
/// `TIMEZONE("…")` formula function to construct one explicitly.
pub struct TimeZoneHandler;

pub(crate) fn pack_timezone_iana(name: &str) -> CustomValue {
    CustomValue { type_tag: tags::TIMEZONE.into(), data: name.into() }
}

/// Canonical `+HH:MM` rendering for a fixed offset. Used when ZONED /
/// IN_TZ accept a user-typed offset string and we want the stored
/// representation to be normalised (`+0530` and `+05:30` round-trip the
/// same way).
pub(crate) fn pack_timezone_offset(offset: Offset) -> CustomValue {
    CustomValue { type_tag: tags::TIMEZONE.into(), data: format_offset(offset) }
}

/// Parse a `+HH:MM` / `-HH:MM` / IANA-name string into a `TimeZone`.
/// `UTC` and `Z` short-circuit to `TimeZone::UTC`.
pub(crate) fn parse_timezone(s: &str) -> Result<TimeZone, String> {
    if s == "UTC" || s == "Z" {
        return Ok(TimeZone::UTC);
    }
    if let Some(offset) = parse_fixed_offset(s) {
        return Ok(TimeZone::fixed(offset));
    }
    TimeZone::get(s).map_err(|e| e.to_string())
}

/// Parse `+HH:MM` / `-HH:MM` (also `+HHMM` and `+HH`) into an `Offset`.
/// Returns `None` if the shape doesn't match — caller falls back to
/// IANA lookup.
fn parse_fixed_offset(s: &str) -> Option<Offset> {
    let (sign, rest) = match s.as_bytes().first()? {
        b'+' => (1i32, &s[1..]),
        b'-' => (-1i32, &s[1..]),
        _ => return None,
    };
    let (h, m) = match rest.len() {
        2 => (rest.parse::<u8>().ok()?, 0u8),
        4 => (rest.get(..2)?.parse::<u8>().ok()?, rest.get(2..)?.parse::<u8>().ok()?),
        5 if rest.as_bytes()[2] == b':' => (
            rest.get(..2)?.parse::<u8>().ok()?,
            rest.get(3..)?.parse::<u8>().ok()?,
        ),
        _ => return None,
    };
    let total_seconds = sign * (i32::from(h) * 3600 + i32::from(m) * 60);
    Offset::from_seconds(total_seconds).ok()
}

/// Render an `Offset` as `+HH:MM` / `-HH:MM`.
fn format_offset(offset: Offset) -> String {
    let total = offset.seconds();
    let sign = if total < 0 { '-' } else { '+' };
    let abs = total.unsigned_abs();
    let h = abs / 3600;
    let m = (abs % 3600) / 60;
    format!("{sign}{h:02}:{m:02}")
}

impl CustomTypeHandler for TimeZoneHandler {
    fn type_tag(&self) -> &str {
        tags::TIMEZONE
    }

    // No `parse_literal` — see struct doc.

    fn display(&self, v: &CustomValue) -> String {
        v.data.clone()
    }

    fn edit_repr(&self, v: &CustomValue) -> String {
        v.data.clone()
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
        if a.type_tag != tags::TIMEZONE || b.type_tag != tags::TIMEZONE {
            return None;
        }
        // Equality only — ordering of time zones isn't a meaningful
        // operation; return None for the ordering comparators so the
        // engine falls through to the default (which currently treats
        // unorderable Custom pairs as `false`).
        match op {
            CompareOp::Eq => Some(Ok(a.data == b.data)),
            CompareOp::Ne => Some(Ok(a.data != b.data)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_utc_short_form() {
        let tz = parse_timezone("UTC").unwrap();
        assert_eq!(tz.iana_name(), Some("UTC"));
    }

    #[test]
    fn parse_fixed_offsets() {
        assert!(parse_timezone("+00:00").is_ok());
        assert!(parse_timezone("-05:00").is_ok());
        assert!(parse_timezone("+0530").is_ok());
        assert!(parse_timezone("+05").is_ok());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_timezone("not-a-zone").is_err());
        assert!(parse_timezone("Mars/Olympus_Mons").is_err());
    }

    #[test]
    fn format_offset_pads() {
        assert_eq!(format_offset(Offset::from_seconds(-5 * 3600).unwrap()), "-05:00");
        assert_eq!(format_offset(Offset::from_seconds(5 * 3600 + 30 * 60).unwrap()), "+05:30");
        assert_eq!(format_offset(Offset::from_seconds(0).unwrap()), "+00:00");
    }

    #[test]
    fn equality_compares_by_data_string() {
        let h = TimeZoneHandler;
        let a = CellValue::Custom(pack_timezone_iana("America/New_York"));
        let b = CellValue::Custom(pack_timezone_iana("America/New_York"));
        let c = CellValue::Custom(pack_timezone_iana("UTC"));
        assert!(h.compare(CompareOp::Eq, &a, &b).unwrap().unwrap());
        assert!(h.compare(CompareOp::Ne, &a, &c).unwrap().unwrap());
        // Ordering returns None → engine falls through.
        assert!(h.compare(CompareOp::Lt, &a, &c).is_none());
    }
}
