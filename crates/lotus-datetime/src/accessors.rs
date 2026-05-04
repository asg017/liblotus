//! Polymorphic accessor and conversion functions that operate on any
//! "date-like" or "time-like" cell value.
//!
//! - **Date-like**: `jdate`, `jdatetime`, `jzoned` — anything with a calendar component.
//! - **Time-like**: `jtime`, `jdatetime`, `jzoned` — anything with a clock component.
//!
//! Each accessor extracts a unified view (`DateLike`, `TimeLike`) and
//! delegates to the appropriate jiff accessor, so YEAR/MONTH/DAY etc.
//! work uniformly across the three calendar-bearing types.

use std::str::FromStr;
use std::sync::Arc;

use jiff::{
    civil::{Date, DateTime, Time, Weekday},
    Zoned,
};
use lotus_core::{CellValue, CustomFunction, FormulaError, Registry, RegistryError};

use crate::date::pack_date;
use crate::datetime::pack_datetime;
use crate::error::map_jiff_err;
use crate::tags;
use crate::time::pack_time;

pub(crate) fn register_accessors(registry: &mut Registry) -> Result<(), RegistryError> {
    // Calendar accessors
    registry.register_function(Arc::new(YearFn))?;
    registry.register_function(Arc::new(MonthFn))?;
    registry.register_function(Arc::new(DayFn))?;
    registry.register_function(Arc::new(WeekdayFn))?;
    registry.register_function(Arc::new(DayOfYearFn))?;
    registry.register_function(Arc::new(WeekOfYearFn))?;
    registry.register_function(Arc::new(QuarterFn))?;
    registry.register_function(Arc::new(IsLeapYearFn))?;
    // Clock accessors
    registry.register_function(Arc::new(HourFn))?;
    registry.register_function(Arc::new(MinuteFn))?;
    registry.register_function(Arc::new(SecondFn))?;
    registry.register_function(Arc::new(NanosecondFn))?;
    // Zoned-only accessors
    registry.register_function(Arc::new(TzNameFn))?;
    registry.register_function(Arc::new(TzOffsetSecondsFn))?;
    registry.register_function(Arc::new(IsDstFn))?;
    // Conversions
    registry.register_function(Arc::new(ToDateFn))?;
    registry.register_function(Arc::new(ToTimeFn))?;
    registry.register_function(Arc::new(ToDateTimeFn))?;
    // Formatting
    registry.register_function(Arc::new(FormatFn))?;
    registry.register_function(Arc::new(IsoFn))?;
    // Span "between" helpers (broadened from T-DT-2's jdate-only DAYS_BETWEEN)
    registry.register_function(Arc::new(DaysBetweenFn))?;
    registry.register_function(Arc::new(HoursBetweenFn))?;
    registry.register_function(Arc::new(MinutesBetweenFn))?;
    registry.register_function(Arc::new(SecondsBetweenFn))?;
    // Sugar over the +/- operators
    registry.register_function(Arc::new(DateaddFn))?;
    registry.register_function(Arc::new(DatesubFn))?;
    Ok(())
}

/// Anything with a calendar component (date, datetime, zoned).
enum DateLike {
    Date(Date),
    DateTime(DateTime),
    Zoned(Zoned),
}

impl DateLike {
    fn year(&self) -> i16 {
        match self {
            DateLike::Date(d) => d.year(),
            DateLike::DateTime(d) => d.year(),
            DateLike::Zoned(z) => z.year(),
        }
    }
    fn month(&self) -> i8 {
        match self {
            DateLike::Date(d) => d.month(),
            DateLike::DateTime(d) => d.month(),
            DateLike::Zoned(z) => z.month(),
        }
    }
    fn day(&self) -> i8 {
        match self {
            DateLike::Date(d) => d.day(),
            DateLike::DateTime(d) => d.day(),
            DateLike::Zoned(z) => z.day(),
        }
    }
    fn weekday(&self) -> Weekday {
        match self {
            DateLike::Date(d) => d.weekday(),
            DateLike::DateTime(d) => d.weekday(),
            DateLike::Zoned(z) => z.weekday(),
        }
    }
    fn day_of_year(&self) -> i16 {
        match self {
            DateLike::Date(d) => d.day_of_year(),
            DateLike::DateTime(d) => d.day_of_year(),
            DateLike::Zoned(z) => z.day_of_year(),
        }
    }
    fn iso_week(&self) -> i8 {
        // jiff's `iso_week_date` takes `self` by value on `Zoned` (which
        // isn't `Copy`), so route through the civil date — same answer.
        match self {
            DateLike::Date(d) => d.iso_week_date().week(),
            DateLike::DateTime(d) => d.iso_week_date().week(),
            DateLike::Zoned(z) => z.date().iso_week_date().week(),
        }
    }
    fn in_leap_year(&self) -> bool {
        match self {
            DateLike::Date(d) => d.in_leap_year(),
            DateLike::DateTime(d) => d.in_leap_year(),
            DateLike::Zoned(z) => z.in_leap_year(),
        }
    }
    fn quarter(&self) -> i8 {
        // Jan-Mar = 1, Apr-Jun = 2, Jul-Sep = 3, Oct-Dec = 4.
        (self.month() - 1) / 3 + 1
    }

    fn to_date(&self) -> Date {
        match self {
            DateLike::Date(d) => *d,
            DateLike::DateTime(d) => d.date(),
            DateLike::Zoned(z) => z.date(),
        }
    }
}

/// Anything with a clock component (time, datetime, zoned).
enum TimeLike {
    Time(Time),
    DateTime(DateTime),
    Zoned(Zoned),
}

impl TimeLike {
    fn hour(&self) -> i8 {
        match self {
            TimeLike::Time(t) => t.hour(),
            TimeLike::DateTime(d) => d.hour(),
            TimeLike::Zoned(z) => z.hour(),
        }
    }
    fn minute(&self) -> i8 {
        match self {
            TimeLike::Time(t) => t.minute(),
            TimeLike::DateTime(d) => d.minute(),
            TimeLike::Zoned(z) => z.minute(),
        }
    }
    fn second(&self) -> i8 {
        match self {
            TimeLike::Time(t) => t.second(),
            TimeLike::DateTime(d) => d.second(),
            TimeLike::Zoned(z) => z.second(),
        }
    }
    fn subsec_nanosecond(&self) -> i32 {
        match self {
            TimeLike::Time(t) => t.subsec_nanosecond(),
            TimeLike::DateTime(d) => d.subsec_nanosecond(),
            TimeLike::Zoned(z) => z.subsec_nanosecond(),
        }
    }

    fn to_time(&self) -> Time {
        match self {
            TimeLike::Time(t) => *t,
            TimeLike::DateTime(d) => d.time(),
            TimeLike::Zoned(z) => z.time(),
        }
    }
}

fn arg_date_like(
    args: &[CellValue],
    position: usize,
    fn_name: &str,
) -> Result<DateLike, String> {
    let v = args.get(position - 1).ok_or_else(|| {
        FormulaError::value(format!("{fn_name}: missing arg #{position}")).to_string()
    })?;
    let cv = match v {
        CellValue::Custom(cv) => cv,
        _ => {
            return Err(FormulaError::value(format!(
                "{fn_name}: arg #{position} must be a jdate, jdatetime, or jzoned value"
            ))
            .to_string())
        }
    };
    match cv.type_tag.as_str() {
        tags::DATE => Date::from_str(&cv.data)
            .map(DateLike::Date)
            .map_err(|e| map_jiff_err(e).to_string()),
        tags::DATETIME => DateTime::from_str(&cv.data)
            .map(DateLike::DateTime)
            .map_err(|e| map_jiff_err(e).to_string()),
        tags::ZONED => Zoned::from_str(&cv.data)
            .map(DateLike::Zoned)
            .map_err(|e| map_jiff_err(e).to_string()),
        other => Err(FormulaError::value(format!(
            "{fn_name}: arg #{position} must be a jdate, jdatetime, or jzoned value (got {other})"
        ))
        .to_string()),
    }
}

fn arg_time_like(
    args: &[CellValue],
    position: usize,
    fn_name: &str,
) -> Result<TimeLike, String> {
    let v = args.get(position - 1).ok_or_else(|| {
        FormulaError::value(format!("{fn_name}: missing arg #{position}")).to_string()
    })?;
    let cv = match v {
        CellValue::Custom(cv) => cv,
        _ => {
            return Err(FormulaError::value(format!(
                "{fn_name}: arg #{position} must be a jtime, jdatetime, or jzoned value"
            ))
            .to_string())
        }
    };
    match cv.type_tag.as_str() {
        tags::TIME => Time::from_str(&cv.data)
            .map(TimeLike::Time)
            .map_err(|e| map_jiff_err(e).to_string()),
        tags::DATETIME => DateTime::from_str(&cv.data)
            .map(TimeLike::DateTime)
            .map_err(|e| map_jiff_err(e).to_string()),
        tags::ZONED => Zoned::from_str(&cv.data)
            .map(TimeLike::Zoned)
            .map_err(|e| map_jiff_err(e).to_string()),
        other => Err(FormulaError::value(format!(
            "{fn_name}: arg #{position} must be a jtime, jdatetime, or jzoned value (got {other})"
        ))
        .to_string()),
    }
}

fn arg_zoned_only(
    args: &[CellValue],
    position: usize,
    fn_name: &str,
) -> Result<Zoned, String> {
    let v = args.get(position - 1).ok_or_else(|| {
        FormulaError::value(format!("{fn_name}: missing arg #{position}")).to_string()
    })?;
    match v {
        CellValue::Custom(cv) if cv.type_tag == tags::ZONED => {
            Zoned::from_str(&cv.data).map_err(|e| map_jiff_err(e).to_string())
        }
        _ => Err(FormulaError::value(format!(
            "{fn_name}: arg #{position} must be a jzoned value"
        ))
        .to_string()),
    }
}

fn enforce_arity(args: &[CellValue], n: usize, fn_name: &str) -> Result<(), String> {
    if args.len() != n {
        Err(FormulaError::value(format!("{fn_name}: expected {n} arg(s), got {}", args.len()))
            .to_string())
    } else {
        Ok(())
    }
}

// === Calendar accessors ===

struct YearFn;
impl CustomFunction for YearFn {
    fn name(&self) -> &str { "YEAR" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "YEAR")?;
        Ok(CellValue::Number(f64::from(arg_date_like(args, 1, "YEAR")?.year())))
    }
}

struct MonthFn;
impl CustomFunction for MonthFn {
    fn name(&self) -> &str { "MONTH" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "MONTH")?;
        Ok(CellValue::Number(f64::from(arg_date_like(args, 1, "MONTH")?.month())))
    }
}

struct DayFn;
impl CustomFunction for DayFn {
    fn name(&self) -> &str { "DAY" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "DAY")?;
        Ok(CellValue::Number(f64::from(arg_date_like(args, 1, "DAY")?.day())))
    }
}

/// `WEEKDAY(x, [iso=true])` → 1-7. Default ISO: 1=Mon, 7=Sun.
/// `iso=false` → 1=Sun, 7=Sat (the common "spreadsheet Sunday-first" form).
struct WeekdayFn;
impl CustomFunction for WeekdayFn {
    fn name(&self) -> &str { "WEEKDAY" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.is_empty() || args.len() > 2 {
            return Err(FormulaError::value("WEEKDAY: expected 1 or 2 args (date, [iso=true])").to_string());
        }
        let dl = arg_date_like(args, 1, "WEEKDAY")?;
        let iso = match args.get(1) {
            None | Some(CellValue::Empty) => true,
            Some(CellValue::Boolean(b)) => *b,
            Some(CellValue::Number(n)) => *n != 0.0,
            _ => return Err(FormulaError::value("WEEKDAY: iso arg must be a boolean").to_string()),
        };
        let n = if iso {
            dl.weekday().to_monday_one_offset()
        } else {
            dl.weekday().to_sunday_one_offset()
        };
        Ok(CellValue::Number(f64::from(n)))
    }
}

struct DayOfYearFn;
impl CustomFunction for DayOfYearFn {
    fn name(&self) -> &str { "DAYOFYEAR" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "DAYOFYEAR")?;
        Ok(CellValue::Number(f64::from(arg_date_like(args, 1, "DAYOFYEAR")?.day_of_year())))
    }
}

struct WeekOfYearFn;
impl CustomFunction for WeekOfYearFn {
    fn name(&self) -> &str { "WEEKOFYEAR" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "WEEKOFYEAR")?;
        Ok(CellValue::Number(f64::from(arg_date_like(args, 1, "WEEKOFYEAR")?.iso_week())))
    }
}

struct QuarterFn;
impl CustomFunction for QuarterFn {
    fn name(&self) -> &str { "QUARTER" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "QUARTER")?;
        Ok(CellValue::Number(f64::from(arg_date_like(args, 1, "QUARTER")?.quarter())))
    }
}

struct IsLeapYearFn;
impl CustomFunction for IsLeapYearFn {
    fn name(&self) -> &str { "IS_LEAP_YEAR" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "IS_LEAP_YEAR")?;
        Ok(CellValue::Boolean(arg_date_like(args, 1, "IS_LEAP_YEAR")?.in_leap_year()))
    }
}

// === Clock accessors ===

struct HourFn;
impl CustomFunction for HourFn {
    fn name(&self) -> &str { "HOUR" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "HOUR")?;
        Ok(CellValue::Number(f64::from(arg_time_like(args, 1, "HOUR")?.hour())))
    }
}

struct MinuteFn;
impl CustomFunction for MinuteFn {
    fn name(&self) -> &str { "MINUTE" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "MINUTE")?;
        Ok(CellValue::Number(f64::from(arg_time_like(args, 1, "MINUTE")?.minute())))
    }
}

struct SecondFn;
impl CustomFunction for SecondFn {
    fn name(&self) -> &str { "SECOND" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "SECOND")?;
        Ok(CellValue::Number(f64::from(arg_time_like(args, 1, "SECOND")?.second())))
    }
}

struct NanosecondFn;
impl CustomFunction for NanosecondFn {
    fn name(&self) -> &str { "NANOSECOND" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "NANOSECOND")?;
        Ok(CellValue::Number(f64::from(arg_time_like(args, 1, "NANOSECOND")?.subsec_nanosecond())))
    }
}

// === Zoned-only accessors ===

struct TzNameFn;
impl CustomFunction for TzNameFn {
    fn name(&self) -> &str { "TZ_NAME" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "TZ_NAME")?;
        let z = arg_zoned_only(args, 1, "TZ_NAME")?;
        // For fixed offsets there's no IANA name — fall back to the
        // canonical offset string the zone parses back from.
        let name = z
            .time_zone()
            .iana_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| z.offset().to_string());
        Ok(CellValue::String(name))
    }
}

struct TzOffsetSecondsFn;
impl CustomFunction for TzOffsetSecondsFn {
    fn name(&self) -> &str { "TZ_OFFSET_SECONDS" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "TZ_OFFSET_SECONDS")?;
        let z = arg_zoned_only(args, 1, "TZ_OFFSET_SECONDS")?;
        Ok(CellValue::Number(f64::from(z.offset().seconds())))
    }
}

/// `IS_DST(jzoned) → Boolean`. True iff the zone's *current* offset for
/// this instant differs from its non-DST baseline. For fixed offsets and
/// zones that don't observe DST, returns `false`.
struct IsDstFn;
impl CustomFunction for IsDstFn {
    fn name(&self) -> &str { "IS_DST" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "IS_DST")?;
        let z = arg_zoned_only(args, 1, "IS_DST")?;
        // jiff exposes DST status via the offset info struct.
        let info = z.time_zone().to_offset_info(z.timestamp());
        Ok(CellValue::Boolean(info.dst().is_dst()))
    }
}

// === Conversions ===

/// `TO_DATE(date_like) → jdate`. Drops time and zone.
struct ToDateFn;
impl CustomFunction for ToDateFn {
    fn name(&self) -> &str { "TO_DATE" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "TO_DATE")?;
        Ok(CellValue::Custom(pack_date(arg_date_like(args, 1, "TO_DATE")?.to_date())))
    }
}

/// `TO_TIME(time_like) → jtime`. Drops date and zone.
struct ToTimeFn;
impl CustomFunction for ToTimeFn {
    fn name(&self) -> &str { "TO_TIME" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "TO_TIME")?;
        Ok(CellValue::Custom(pack_time(arg_time_like(args, 1, "TO_TIME")?.to_time())))
    }
}

/// `TO_DATETIME(date_like, [time]) → jdatetime`. Combines a date with
/// a time (default midnight). When the first arg is already a datetime
/// or zoned, the second arg replaces its time.
struct ToDateTimeFn;
impl CustomFunction for ToDateTimeFn {
    fn name(&self) -> &str { "TO_DATETIME" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.is_empty() || args.len() > 2 {
            return Err(FormulaError::value(
                "TO_DATETIME: expected 1 or 2 args (date_like, [time])",
            )
            .to_string());
        }
        let date = arg_date_like(args, 1, "TO_DATETIME")?.to_date();
        let time = match args.get(1) {
            None | Some(CellValue::Empty) => Time::midnight(),
            Some(CellValue::Custom(cv)) if cv.type_tag == tags::TIME => {
                Time::from_str(&cv.data).map_err(|e| map_jiff_err(e).to_string())?
            }
            _ => {
                return Err(FormulaError::value(
                    "TO_DATETIME: arg #2 must be a jtime value or omitted",
                )
                .to_string())
            }
        };
        Ok(CellValue::Custom(pack_datetime(date.to_datetime(time))))
    }
}

// === Formatting ===

/// strftime a date/time/datetime/zoned `CustomValue` with `fmt`. Returns
/// `Err` for unsupported tags (jspan / jtimezone) and for malformed
/// canonical data. Used by both the `FORMAT` formula function and by
/// host UIs that want a per-cell strftime axis (e.g. vlotus's
/// `:fmt date <pattern>`).
pub fn format_custom_value(
    cv: &lotus_core::CustomValue,
    fmt: &str,
) -> Result<String, String> {
    match cv.type_tag.as_str() {
        tags::DATE => Date::from_str(&cv.data)
            .map(|d| d.strftime(fmt).to_string())
            .map_err(|e| map_jiff_err(e).to_string()),
        tags::TIME => Time::from_str(&cv.data)
            .map(|t| t.strftime(fmt).to_string())
            .map_err(|e| map_jiff_err(e).to_string()),
        tags::DATETIME => DateTime::from_str(&cv.data)
            .map(|d| d.strftime(fmt).to_string())
            .map_err(|e| map_jiff_err(e).to_string()),
        tags::ZONED => Zoned::from_str(&cv.data)
            .map(|z| z.strftime(fmt).to_string())
            .map_err(|e| map_jiff_err(e).to_string()),
        other => Err(format!(
            "format_custom_value: unsupported type {other} (need jdate, jtime, jdatetime, or jzoned)"
        )),
    }
}

/// `FORMAT(temporal, fmt_str) → String`. Polymorphic over the four
/// temporal types; uses jiff's strftime. The fmt string is the standard
/// C-family format with jiff extensions like `%Z` (zone abbrev) and
/// `%Q` (IANA name).
struct FormatFn;
impl CustomFunction for FormatFn {
    fn name(&self) -> &str { "FORMAT" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 2, "FORMAT")?;
        let fmt = match &args[1] {
            CellValue::String(s) => s.as_str(),
            _ => {
                return Err(
                    FormulaError::value("FORMAT: arg #2 (fmt) must be a string").to_string()
                )
            }
        };
        let CellValue::Custom(cv) = &args[0] else {
            return Err(FormulaError::value(
                "FORMAT: arg #1 must be a jdate, jtime, jdatetime, or jzoned value",
            )
            .to_string());
        };
        let rendered = format_custom_value(cv, fmt).map_err(|msg| {
            // Re-route the host-style error message through #VALUE! so
            // the cell shows the standard error sigil; the formula
            // contract is FormulaError::value, not a raw string.
            if msg.starts_with("format_custom_value: unsupported type") {
                FormulaError::value(format!(
                    "FORMAT: arg #1 must be a date/time/datetime/zoned ({msg})"
                ))
                .to_string()
            } else {
                msg
            }
        })?;
        Ok(CellValue::String(rendered))
    }
}

// === Span "between" family ===
//
// All four take a matched pair of temporal cell values and return the
// signed elapsed time in the named unit (`end - start`). Mixing types
// (jdate vs jzoned, etc.) is rejected with #VALUE!. Within a unit's
// granularity the divisions are exact integer-second math, so e.g.
// SECONDS_BETWEEN never loses precision below the f64 mantissa limit.

fn between_seconds(args: &[CellValue], fn_name: &str) -> Result<f64, String> {
    if args.len() != 2 {
        return Err(FormulaError::value(format!("{fn_name}: expected 2 temporal args")).to_string());
    }
    let (CellValue::Custom(a), CellValue::Custom(b)) = (&args[0], &args[1]) else {
        return Err(FormulaError::value(format!(
            "{fn_name}: both args must be matching temporal values (jdate, jdatetime, jzoned, or jtime)"
        ))
        .to_string());
    };
    if a.type_tag != b.type_tag {
        return Err(FormulaError::value(format!(
            "{fn_name}: both args must be the same type (got {} and {})",
            a.type_tag, b.type_tag
        ))
        .to_string());
    }
    let secs = match a.type_tag.as_str() {
        tags::DATE => {
            let start = Date::from_str(&a.data).map_err(|e| map_jiff_err(e).to_string())?;
            let end = Date::from_str(&b.data).map_err(|e| map_jiff_err(e).to_string())?;
            end.duration_since(start).as_secs_f64()
        }
        tags::DATETIME => {
            let start = DateTime::from_str(&a.data).map_err(|e| map_jiff_err(e).to_string())?;
            let end = DateTime::from_str(&b.data).map_err(|e| map_jiff_err(e).to_string())?;
            end.duration_since(start).as_secs_f64()
        }
        tags::ZONED => {
            let start = Zoned::from_str(&a.data).map_err(|e| map_jiff_err(e).to_string())?;
            let end = Zoned::from_str(&b.data).map_err(|e| map_jiff_err(e).to_string())?;
            end.duration_since(&start).as_secs_f64()
        }
        tags::TIME => {
            let start = Time::from_str(&a.data).map_err(|e| map_jiff_err(e).to_string())?;
            let end = Time::from_str(&b.data).map_err(|e| map_jiff_err(e).to_string())?;
            // Time::since returns a Span; for pure clock units we can
            // round-trip through SignedDuration with no relative anchor.
            let span = end.since(start).map_err(|e| e.to_string())?;
            span.to_duration(jiff::SpanRelativeTo::days_are_24_hours())
                .map_err(|e| map_jiff_err(e).to_string())?
                .as_secs_f64()
        }
        other => {
            return Err(FormulaError::value(format!(
                "{fn_name}: unsupported temporal type {other}"
            ))
            .to_string())
        }
    };
    Ok(secs)
}

struct DaysBetweenFn;
impl CustomFunction for DaysBetweenFn {
    fn name(&self) -> &str { "DAYS_BETWEEN" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        Ok(CellValue::Number(between_seconds(args, "DAYS_BETWEEN")? / 86_400.0))
    }
}

struct HoursBetweenFn;
impl CustomFunction for HoursBetweenFn {
    fn name(&self) -> &str { "HOURS_BETWEEN" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        Ok(CellValue::Number(between_seconds(args, "HOURS_BETWEEN")? / 3_600.0))
    }
}

struct MinutesBetweenFn;
impl CustomFunction for MinutesBetweenFn {
    fn name(&self) -> &str { "MINUTES_BETWEEN" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        Ok(CellValue::Number(between_seconds(args, "MINUTES_BETWEEN")? / 60.0))
    }
}

struct SecondsBetweenFn;
impl CustomFunction for SecondsBetweenFn {
    fn name(&self) -> &str { "SECONDS_BETWEEN" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        Ok(CellValue::Number(between_seconds(args, "SECONDS_BETWEEN")?))
    }
}

// === Sugar over operators ===
//
// `DATEADD(x, span)` is exactly `x + span` for any temporal type that
// supports + with a span (jdate / jdatetime / jzoned / jtime). Same
// for DATESUB. We dispatch by type_tag and call the existing handler
// arithmetic so the "calendar units forbidden on jtime" constraint and
// DST-aware Zoned addition come along for free.

fn add_or_sub(args: &[CellValue], fn_name: &str, sub: bool) -> Result<CellValue, String> {
    if args.len() != 2 {
        return Err(FormulaError::value(format!("{fn_name}: expected 2 args (temporal, jspan)")).to_string());
    }
    use jiff::Span;
    let CellValue::Custom(a) = &args[0] else {
        return Err(FormulaError::value(format!(
            "{fn_name}: arg #1 must be a jdate/jdatetime/jzoned/jtime value"
        )).to_string());
    };
    let CellValue::Custom(b) = &args[1] else {
        return Err(FormulaError::value(format!(
            "{fn_name}: arg #2 must be a jspan value"
        )).to_string());
    };
    if b.type_tag != tags::SPAN {
        return Err(FormulaError::value(format!(
            "{fn_name}: arg #2 must be a jspan value (got {})",
            b.type_tag
        )).to_string());
    }
    let span = Span::from_str(&b.data).map_err(|e| map_jiff_err(e).to_string())?;
    match a.type_tag.as_str() {
        tags::DATE => {
            let d = Date::from_str(&a.data).map_err(|e| map_jiff_err(e).to_string())?;
            let out = if sub { d.checked_sub(span) } else { d.checked_add(span) };
            Ok(CellValue::Custom(pack_date(out.map_err(|e| map_jiff_err(e).to_string())?)))
        }
        tags::DATETIME => {
            let d = DateTime::from_str(&a.data).map_err(|e| map_jiff_err(e).to_string())?;
            let out = if sub { d.checked_sub(span) } else { d.checked_add(span) };
            Ok(CellValue::Custom(pack_datetime(out.map_err(|e| map_jiff_err(e).to_string())?)))
        }
        tags::ZONED => {
            let z = Zoned::from_str(&a.data).map_err(|e| map_jiff_err(e).to_string())?;
            let out = if sub { z.checked_sub(span) } else { z.checked_add(span) };
            Ok(CellValue::Custom(crate::zoned::pack_zoned(&out.map_err(|e| map_jiff_err(e).to_string())?)))
        }
        tags::TIME => {
            // Forbid calendar units on time, matching TimeHandler's behaviour.
            if span.get_years() != 0 || span.get_months() != 0
                || span.get_weeks() != 0 || span.get_days() != 0
            {
                return Err(FormulaError::value(format!(
                    "{fn_name}: span has calendar units; jtime has no calendar anchor"
                )).to_string());
            }
            let t = Time::from_str(&a.data).map_err(|e| map_jiff_err(e).to_string())?;
            let out = if sub { t.wrapping_sub(span) } else { t.wrapping_add(span) };
            Ok(CellValue::Custom(pack_time(out)))
        }
        other => Err(FormulaError::value(format!(
            "{fn_name}: arg #1 must be a jdate/jdatetime/jzoned/jtime value (got {other})"
        )).to_string()),
    }
}

struct DateaddFn;
impl CustomFunction for DateaddFn {
    fn name(&self) -> &str { "DATEADD" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        add_or_sub(args, "DATEADD", false)
    }
}

struct DatesubFn;
impl CustomFunction for DatesubFn {
    fn name(&self) -> &str { "DATESUB" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        add_or_sub(args, "DATESUB", true)
    }
}

/// `ISO(temporal) → String`. Returns the canonical text rep — same as
/// `edit_repr`. Useful for serialising to JSON or CSV without going
/// through the friendlier `display`.
struct IsoFn;
impl CustomFunction for IsoFn {
    fn name(&self) -> &str { "ISO" }
    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        enforce_arity(args, 1, "ISO")?;
        let CellValue::Custom(cv) = &args[0] else {
            return Err(FormulaError::value(
                "ISO: arg #1 must be a jdate, jtime, jdatetime, jzoned, or jspan value",
            )
            .to_string());
        };
        // For all our types the canonical form is what we already store
        // in `data`. Re-emit verbatim.
        Ok(CellValue::String(cv.data.clone()))
    }
}

