use std::cmp::Ordering;
use std::str::FromStr;

use jiff::Zoned;
use lotus_core::{BinaryOp, CellValue, CompareOp, CustomTypeHandler, CustomValue};

use crate::span::unpack_span;
use crate::tags;

/// Custom-type handler for `jzoned` (a `jiff::Zoned`).
///
/// `data` is RFC 9557, e.g. `2025-04-27T14:30:00-04:00[America/New_York]`,
/// which is what `Zoned::Display` produces and `Zoned::from_str` accepts.
/// Display swaps the canonical `T` for a space and shows the zone abbrev
/// for readability; edit_repr keeps the canonical form.
///
/// Owns: `jzoned ± jspan` → `jzoned` (DST-aware via jiff's checked add).
/// `jzoned - jzoned` is owned by `SpanHandler`.
///
/// **Round-trip caveat**: parsing a `jzoned` whose IANA name isn't in the
/// active tzdb will fail. The `system-tz` (native) or `bundled-tz`
/// (WASM/sandboxed) cargo features provide the tzdb; without either,
/// only fixed-offset zones round-trip.
pub struct ZonedHandler;

pub(crate) fn pack_zoned(z: &Zoned) -> CustomValue {
    CustomValue { type_tag: tags::ZONED.into(), data: z.to_string() }
}

pub(crate) fn unpack_zoned(cv: &CustomValue) -> Option<Result<Zoned, String>> {
    if cv.type_tag != tags::ZONED {
        return None;
    }
    Some(Zoned::from_str(&cv.data).map_err(|e| e.to_string()))
}

/// Cheap shape check: looks like a `jdatetime` *and* contains `[…]` for
/// the IANA suffix. The bracket distinguishes it from `jdatetime`,
/// letting us register `jzoned` first in `lib.rs::register` and have it
/// claim the right strings.
fn looks_like_zoned(raw: &str) -> bool {
    let b = raw.as_bytes();
    if b.len() < 20 {
        return false;
    }
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return false;
    }
    b.contains(&b'[') && b.last() == Some(&b']')
}

impl CustomTypeHandler for ZonedHandler {
    fn type_tag(&self) -> &str {
        tags::ZONED
    }

    fn parse_literal(&self, raw: &str) -> Option<CustomValue> {
        if !looks_like_zoned(raw) {
            return None;
        }
        Zoned::from_str(raw).ok().map(|z| pack_zoned(&z))
    }

    /// Probe API: empty options falls back to `parse_literal`, non-empty
    /// options is treated as a jiff `strptime` pattern. See
    /// [`crate::DateHandler::parse_with`] for the design.
    fn parse_with(&self, raw: &str, options: &str) -> Option<CustomValue> {
        if options.is_empty() {
            return self.parse_literal(raw);
        }
        Zoned::strptime(options, raw).ok().map(|z| pack_zoned(&z))
    }

    fn display(&self, v: &CustomValue) -> String {
        // Friendly: 'YYYY-MM-DD HH:MM:SS ZONE'. Falls back to canonical
        // RFC 9557 if strftime fails (shouldn't happen for valid data,
        // but a corrupted `data` field shouldn't panic the renderer).
        match Zoned::from_str(&v.data) {
            Ok(z) => z.strftime("%Y-%m-%d %H:%M:%S %Z").to_string(),
            Err(_) => v.data.clone(),
        }
    }

    fn edit_repr(&self, v: &CustomValue) -> String {
        v.data.clone()
    }

    fn as_number(&self, v: &CustomValue) -> Option<f64> {
        // Seconds since the Unix epoch (instant in time, ignores zone).
        let z = Zoned::from_str(&v.data).ok()?;
        Some(z.timestamp().as_second() as f64)
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
                (tags::ZONED, tags::SPAN) => Some(zoned_plus_span(a, b)),
                (tags::SPAN, tags::ZONED) => Some(zoned_plus_span(b, a)),
                _ => None,
            },
            BinaryOp::Sub if a.type_tag == tags::ZONED && b.type_tag == tags::SPAN => {
                Some(zoned_minus_span(a, b))
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
        if a.type_tag != tags::ZONED || b.type_tag != tags::ZONED {
            return None;
        }
        let za = match unpack_zoned(a)? {
            Ok(z) => z,
            Err(e) => return Some(Err(e)),
        };
        let zb = match unpack_zoned(b)? {
            Ok(z) => z,
            Err(e) => return Some(Err(e)),
        };
        // Compare by instant (.timestamp()) — ignores how each side
        // happens to render its wall clock. Two zoneds at the same
        // instant in different zones compare equal.
        let ord = za.timestamp().cmp(&zb.timestamp());
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

fn zoned_plus_span(zoned_cv: &CustomValue, span_cv: &CustomValue) -> Result<CellValue, String> {
    let z = unpack_zoned(zoned_cv).expect("checked tag")?;
    let s = unpack_span(span_cv).expect("checked tag")?;
    let out = z.checked_add(s).map_err(|e| e.to_string())?;
    Ok(CellValue::Custom(pack_zoned(&out)))
}

fn zoned_minus_span(zoned_cv: &CustomValue, span_cv: &CustomValue) -> Result<CellValue, String> {
    let z = unpack_zoned(zoned_cv).expect("checked tag")?;
    let s = unpack_span(span_cv).expect("checked tag")?;
    let out = z.checked_sub(s).map_err(|e| e.to_string())?;
    Ok(CellValue::Custom(pack_zoned(&out)))
}

#[cfg(all(test, feature = "system-tz"))]
mod tests {
    use super::*;

    fn jzoned_cv(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::ZONED.into(), data: s.into() })
    }

    fn jspan_cv(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::SPAN.into(), data: s.into() })
    }

    #[test]
    fn parse_iso_zoned() {
        let cv = ZonedHandler
            .parse_literal("2025-04-27T14:30:00-04:00[America/New_York]")
            .unwrap();
        assert_eq!(cv.type_tag, "jzoned");
    }

    #[test]
    fn parse_rejects_plain_datetime() {
        // No '[…]' suffix → don't claim. DateTimeHandler will get it.
        assert!(ZonedHandler.parse_literal("2025-04-27T14:30:00").is_none());
        assert!(ZonedHandler.parse_literal("hello").is_none());
    }

    #[test]
    fn as_number_unix_seconds() {
        let z = Zoned::from_str("1970-01-01T00:00:00+00:00[UTC]").unwrap();
        assert_eq!(ZonedHandler.as_number(&pack_zoned(&z)), Some(0.0));
        let z = Zoned::from_str("1970-01-01T00:01:00+00:00[UTC]").unwrap();
        assert_eq!(ZonedHandler.as_number(&pack_zoned(&z)), Some(60.0));
    }

    #[test]
    fn compare_ignores_zone_when_instants_match() {
        // Same instant rendered in different zones → equal under `=`.
        let utc = jzoned_cv("2025-04-27T18:30:00+00:00[UTC]");
        let nyc = jzoned_cv("2025-04-27T14:30:00-04:00[America/New_York]");
        assert!(ZonedHandler
            .compare(CompareOp::Eq, &utc, &nyc)
            .unwrap()
            .unwrap());
    }

    #[test]
    fn zoned_plus_one_day_dst_aware() {
        // March 9 → March 10 in America/New_York: 1 calendar day, but
        // 23 wall-clock hours (DST gap on March 10 02:00 → 03:00).
        let before_dst = jzoned_cv("2024-03-09T12:00:00-05:00[America/New_York]");
        let CellValue::Custom(out) = ZonedHandler
            .binary_op(BinaryOp::Add, &before_dst, &jspan_cv("P1D"))
            .unwrap()
            .unwrap()
        else {
            panic!("expected jzoned");
        };
        // The result is March 10 12:00 in NYC — same wall time, different
        // offset (-04:00 because DST started). The IMPORTANT property is
        // that we land on the next civil day, not 24 hours later.
        let parsed = Zoned::from_str(&out.data).unwrap();
        assert_eq!(parsed.date().day(), 10);
        assert_eq!(parsed.hour(), 12);
        assert_eq!(parsed.offset().seconds(), -4 * 3600);
    }
}
