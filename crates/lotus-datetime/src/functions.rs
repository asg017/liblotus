use std::str::FromStr;
use std::sync::Arc;

use jiff::{
    civil::{Date, DateTime, Time},
    tz::{Disambiguation, TimeZone},
    Span, Zoned,
};
use lotus_core::{CellValue, CustomFunction, FormulaError, Registry, RegistryError};

use crate::date::pack_date;
use crate::datetime::pack_datetime;
use crate::error::map_jiff_err;
use crate::span::{pack_span, EPOCH};
use crate::tags;
use crate::time::pack_time;
use crate::timezone::{pack_timezone_iana, pack_timezone_offset, parse_timezone};
use crate::zoned::pack_zoned;

pub(crate) fn register_date_functions(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register_function(Arc::new(DateFn))?;
    registry.register_function(Arc::new(TodayFn))?;
    registry.register_function(Arc::new(ParseDateFn))?;
    Ok(())
}

pub(crate) fn register_time_functions(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register_function(Arc::new(TimeFn))?;
    registry.register_function(Arc::new(ParseTimeFn))?;
    Ok(())
}

pub(crate) fn register_datetime_functions(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register_function(Arc::new(DateTimeFn))?;
    registry.register_function(Arc::new(ParseDateTimeFn))?;
    Ok(())
}

pub(crate) fn register_zoned_functions(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register_function(Arc::new(TimezoneFn))?;
    registry.register_function(Arc::new(ZonedFn))?;
    registry.register_function(Arc::new(ToZonedFn))?;
    registry.register_function(Arc::new(ParseZonedFn))?;
    registry.register_function(Arc::new(InTzFn))?;
    registry.register_function(Arc::new(ToUtcFn))?;
    registry.register_function(Arc::new(NowFn))?;
    registry.register_function(Arc::new(UtcNowFn))?;
    Ok(())
}

pub(crate) fn register_span_functions(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register_function(Arc::new(UnitFn { name: "YEARS", builder: |s, n| s.years(n) }))?;
    registry.register_function(Arc::new(UnitFn { name: "MONTHS", builder: |s, n| s.months(n) }))?;
    registry.register_function(Arc::new(UnitFn { name: "WEEKS", builder: |s, n| s.weeks(n) }))?;
    registry.register_function(Arc::new(UnitFn { name: "DAYS", builder: |s, n| s.days(n) }))?;
    registry.register_function(Arc::new(UnitFn { name: "HOURS", builder: |s, n| s.hours(n) }))?;
    registry.register_function(Arc::new(UnitFn { name: "MINUTES", builder: |s, n| s.minutes(n) }))?;
    registry.register_function(Arc::new(UnitFn { name: "SECONDS", builder: |s, n| s.seconds(n) }))?;
    registry.register_function(Arc::new(SpanFn))?;
    registry.register_function(Arc::new(ParseSpanFn))?;
    registry.register_function(Arc::new(SpanToSecondsFn))?;
    registry.register_function(Arc::new(SpanToDaysFn))?;
    Ok(())
}

/// Pull a numeric arg, rejecting cell values that don't carry a number.
/// `position` is 1-based (matches what the user typed).
fn arg_number(args: &[CellValue], position: usize, fn_name: &str) -> Result<f64, String> {
    let v = args
        .get(position - 1)
        .ok_or_else(|| FormulaError::value(format!("{fn_name}: missing arg #{position}")).to_string())?;
    v.as_number().ok_or_else(|| {
        FormulaError::value(format!("{fn_name}: arg #{position} must be a number")).to_string()
    })
}

// `arg_date` (jdate-only) was used by the original YEAR/MONTH/DAY/
// DAYS_BETWEEN; those moved to `accessors.rs` and grew to accept
// jdatetime/jzoned via `arg_date_like`.

/// Pull a `Span` arg from a `jspan` cell value.
fn arg_span(args: &[CellValue], position: usize, fn_name: &str) -> Result<Span, String> {
    let v = args
        .get(position - 1)
        .ok_or_else(|| FormulaError::value(format!("{fn_name}: missing arg #{position}")).to_string())?;
    match v {
        CellValue::Custom(cv) if cv.type_tag == tags::SPAN => {
            Span::from_str(&cv.data).map_err(|e| map_jiff_err(e).to_string())
        }
        _ => Err(FormulaError::value(format!(
            "{fn_name}: arg #{position} must be a {} value",
            tags::SPAN,
        ))
        .to_string()),
    }
}

/// Optional jdate arg: returns `None` if the arg is missing or `Empty`,
/// otherwise enforces it's a jdate. Used by `SPAN_TO_*` for the
/// optional `relative` parameter.
fn arg_optional_date(
    args: &[CellValue],
    position: usize,
    fn_name: &str,
) -> Result<Option<Date>, String> {
    match args.get(position - 1) {
        None | Some(CellValue::Empty) => Ok(None),
        Some(CellValue::Custom(cv)) if cv.type_tag == tags::DATE => Date::from_str(&cv.data)
            .map(Some)
            .map_err(|e| map_jiff_err(e).to_string()),
        _ => Err(FormulaError::value(format!(
            "{fn_name}: arg #{position} must be a {} value or omitted",
            tags::DATE,
        ))
        .to_string()),
    }
}

/// `DATE(year, month, day) -> jdate`
struct DateFn;

impl CustomFunction for DateFn {
    fn name(&self) -> &str {
        "DATE"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.len() != 3 {
            return Err(FormulaError::value("DATE: expected 3 args (year, month, day)").to_string());
        }
        let year = arg_number(args, 1, "DATE")?;
        let month = arg_number(args, 2, "DATE")?;
        let day = arg_number(args, 3, "DATE")?;
        // jiff's Date::new takes i16/i8/i8. Anything outside those ranges
        // (or fractional) is a user error → #VALUE!.
        let year = ranged_int::<i16>(year, "DATE", "year")?;
        let month = ranged_int::<i8>(month, "DATE", "month")?;
        let day = ranged_int::<i8>(day, "DATE", "day")?;
        let date = Date::new(year, month, day).map_err(|e| map_jiff_err(e).to_string())?;
        Ok(CellValue::Custom(pack_date(date)))
    }
}

/// `TODAY() -> jdate` (system-zone calendar date).
struct TodayFn;

impl CustomFunction for TodayFn {
    fn name(&self) -> &str {
        "TODAY"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if !args.is_empty() {
            return Err(FormulaError::value("TODAY: expected no args").to_string());
        }
        today_impl()
    }
}

#[cfg(feature = "_has-now")]
fn today_impl() -> Result<CellValue, String> {
    Ok(CellValue::Custom(pack_date(jiff::Zoned::now().date())))
}

#[cfg(not(feature = "_has-now"))]
fn today_impl() -> Result<CellValue, String> {
    // We deliberately error rather than fall back to UTC — without a
    // system tz lookup, "today" could be off by one calendar day from
    // what the user expects.
    Err(FormulaError::Custom(
        "TODAY() requires the lotus-datetime `system-tz` (native) or `js-now` (wasm) feature"
            .into(),
    )
    .to_string())
}

/// `PARSE_DATE(str, [fmt]) -> jdate`. With `fmt`, uses jiff strptime.
struct ParseDateFn;

impl CustomFunction for ParseDateFn {
    fn name(&self) -> &str {
        "PARSE_DATE"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        let (s, fmt) = parse_args("PARSE_DATE", args)?;
        let d = match fmt {
            Some(f) => Date::strptime(f, s).map_err(|e| map_jiff_err(e).to_string())?,
            None => Date::from_str(s).map_err(|e| map_jiff_err(e).to_string())?,
        };
        Ok(CellValue::Custom(pack_date(d)))
    }
}

// YEAR / MONTH / DAY moved to `accessors.rs` when they became polymorphic
// over jdate / jdatetime / jzoned in T-DT-5.

/// `YEARS(n)` / `MONTHS(n)` / … → a single-unit `jspan`. One generic
/// struct backs all seven; `name` and `builder` differ.
struct UnitFn {
    name: &'static str,
    builder: fn(Span, i64) -> Span,
}

impl CustomFunction for UnitFn {
    fn name(&self) -> &str {
        self.name
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.len() != 1 {
            return Err(
                FormulaError::value(format!("{}: expected 1 arg", self.name)).to_string()
            );
        }
        let n = arg_number(args, 1, self.name)?;
        if !n.is_finite() || n.fract() != 0.0 {
            return Err(
                FormulaError::value(format!("{}: arg must be a whole number", self.name))
                    .to_string(),
            );
        }
        let s = (self.builder)(Span::new(), n as i64);
        Ok(CellValue::Custom(pack_span(s)))
    }
}

/// `SPAN(years, months, weeks, days, hours, minutes, seconds) -> jspan`.
struct SpanFn;

/// `(field-name, builder)` pair used by both `SpanFn` and `UnitFn`.
type SpanFieldBuilder = (&'static str, fn(Span, i64) -> Span);

impl CustomFunction for SpanFn {
    fn name(&self) -> &str {
        "SPAN"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.len() != 7 {
            return Err(FormulaError::value(
                "SPAN: expected 7 args (years, months, weeks, days, hours, minutes, seconds)",
            )
            .to_string());
        }
        let mut s = Span::new();
        let setters: [SpanFieldBuilder; 7] = [
            ("years", |s, n| s.years(n)),
            ("months", |s, n| s.months(n)),
            ("weeks", |s, n| s.weeks(n)),
            ("days", |s, n| s.days(n)),
            ("hours", |s, n| s.hours(n)),
            ("minutes", |s, n| s.minutes(n)),
            ("seconds", |s, n| s.seconds(n)),
        ];
        for (i, (field, build)) in setters.iter().enumerate() {
            let n = arg_number(args, i + 1, "SPAN")?;
            if !n.is_finite() || n.fract() != 0.0 {
                return Err(FormulaError::value(format!(
                    "SPAN: {field} must be a whole number"
                ))
                .to_string());
            }
            s = build(s, n as i64);
        }
        Ok(CellValue::Custom(pack_span(s)))
    }
}

/// `PARSE_SPAN(str) -> jspan`. Accepts ISO 8601 (`P1Y2M`) and the friendly
/// form (`5h 30m`). Useful when the literal-parse path is occluded
/// (e.g. the value came from a string concat).
struct ParseSpanFn;

impl CustomFunction for ParseSpanFn {
    fn name(&self) -> &str {
        "PARSE_SPAN"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.len() != 1 {
            return Err(FormulaError::value("PARSE_SPAN: expected 1 string arg").to_string());
        }
        let s = match &args[0] {
            CellValue::String(s) => s.as_str(),
            _ => {
                return Err(
                    FormulaError::value("PARSE_SPAN: arg must be a string").to_string()
                )
            }
        };
        let span = Span::from_str(s).map_err(|e| map_jiff_err(e).to_string())?;
        Ok(CellValue::Custom(pack_span(span)))
    }
}

// DAYS_BETWEEN moved to accessors.rs in T-DT-5 when it was broadened to
// accept jdatetime/jzoned/jtime in addition to jdate, and joined by
// HOURS/MINUTES/SECONDS_BETWEEN.

/// `SPAN_TO_SECONDS(span, [relative]) -> Number`. `relative` defaults to
/// the Unix epoch. Required arg becomes load-bearing only for spans
/// containing months/years (where the second-count varies by anchor).
struct SpanToSecondsFn;

impl CustomFunction for SpanToSecondsFn {
    fn name(&self) -> &str {
        "SPAN_TO_SECONDS"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.is_empty() || args.len() > 2 {
            return Err(FormulaError::value(
                "SPAN_TO_SECONDS: expected 1 or 2 args (span, [relative jdate])",
            )
            .to_string());
        }
        let span = arg_span(args, 1, "SPAN_TO_SECONDS")?;
        let relative = arg_optional_date(args, 2, "SPAN_TO_SECONDS")?.unwrap_or(EPOCH);
        let dur = span.to_duration(relative).map_err(|e| map_jiff_err(e).to_string())?;
        Ok(CellValue::Number(dur.as_secs_f64()))
    }
}

/// Pull a `DateTime` from a `jdatetime` cell value.
fn arg_datetime(args: &[CellValue], position: usize, fn_name: &str) -> Result<DateTime, String> {
    let v = args
        .get(position - 1)
        .ok_or_else(|| FormulaError::value(format!("{fn_name}: missing arg #{position}")).to_string())?;
    match v {
        CellValue::Custom(cv) if cv.type_tag == tags::DATETIME => {
            DateTime::from_str(&cv.data).map_err(|e| map_jiff_err(e).to_string())
        }
        _ => Err(FormulaError::value(format!(
            "{fn_name}: arg #{position} must be a {} value",
            tags::DATETIME,
        ))
        .to_string()),
    }
}

/// Pull a `Zoned` from a `jzoned` cell value.
fn arg_zoned(args: &[CellValue], position: usize, fn_name: &str) -> Result<Zoned, String> {
    let v = args
        .get(position - 1)
        .ok_or_else(|| FormulaError::value(format!("{fn_name}: missing arg #{position}")).to_string())?;
    match v {
        CellValue::Custom(cv) if cv.type_tag == tags::ZONED => {
            Zoned::from_str(&cv.data).map_err(|e| map_jiff_err(e).to_string())
        }
        _ => Err(FormulaError::value(format!(
            "{fn_name}: arg #{position} must be a {} value",
            tags::ZONED,
        ))
        .to_string()),
    }
}

/// Pull a `TimeZone` arg, accepting either a `jtimezone` cell value
/// or a raw string (IANA name or `+HH:MM`). Strings let users write
/// `IN_TZ(z, "UTC")` without first wrapping in `TIMEZONE("UTC")`.
fn arg_timezone(args: &[CellValue], position: usize, fn_name: &str) -> Result<TimeZone, String> {
    let v = args
        .get(position - 1)
        .ok_or_else(|| FormulaError::value(format!("{fn_name}: missing arg #{position}")).to_string())?;
    let s = match v {
        CellValue::Custom(cv) if cv.type_tag == tags::TIMEZONE => cv.data.as_str(),
        CellValue::String(s) => s.as_str(),
        _ => {
            return Err(FormulaError::value(format!(
                "{fn_name}: arg #{position} must be a {} value or a string",
                tags::TIMEZONE,
            ))
            .to_string())
        }
    };
    parse_timezone(s).map_err(|e| FormulaError::value(format!("{fn_name}: {e}")).to_string())
}

/// Map an optional disambiguation-string arg to jiff's `Disambiguation`.
/// Defaults to `Compatible` (jiff's default — picks the later instant
/// for folds, shifts forward for gaps). `Reject` surfaces ambiguity as
/// `#VALUE!`.
fn arg_disambiguation(
    args: &[CellValue],
    position: usize,
    fn_name: &str,
) -> Result<Disambiguation, String> {
    let raw = match args.get(position - 1) {
        None | Some(CellValue::Empty) => return Ok(Disambiguation::Compatible),
        Some(CellValue::String(s)) if s.is_empty() => return Ok(Disambiguation::Compatible),
        Some(CellValue::String(s)) => s.as_str(),
        _ => {
            return Err(FormulaError::value(format!(
                "{fn_name}: disambiguation must be a string or omitted"
            ))
            .to_string())
        }
    };
    Ok(match raw.to_ascii_lowercase().as_str() {
        "compatible" => Disambiguation::Compatible,
        "earlier" => Disambiguation::Earlier,
        "later" => Disambiguation::Later,
        "reject" => Disambiguation::Reject,
        other => {
            return Err(FormulaError::value(format!(
                "{fn_name}: disambiguation must be 'compatible', 'earlier', 'later', or 'reject' (got {other:?})"
            ))
            .to_string())
        }
    })
}

/// Convert a civil DateTime to a Zoned in `tz` honouring the requested
/// disambiguation. Shared between `ZONED`, `TO_ZONED`, and used in tests.
fn datetime_to_zoned(
    dt: DateTime,
    tz: TimeZone,
    disambig: Disambiguation,
    fn_name: &str,
) -> Result<Zoned, String> {
    let ambiguous = tz.to_ambiguous_zoned(dt);
    // `Disambiguation` is `#[non_exhaustive]` upstream; treat any future
    // variant as `Compatible` so a jiff bump doesn't break us.
    let z = match disambig {
        Disambiguation::Earlier => ambiguous.earlier(),
        Disambiguation::Later => ambiguous.later(),
        Disambiguation::Reject => ambiguous.unambiguous(),
        Disambiguation::Compatible | _ => ambiguous.compatible(),
    };
    z.map_err(|e| {
        if matches!(disambig, Disambiguation::Reject) {
            FormulaError::value(format!("{fn_name}: ambiguous local time ({e})")).to_string()
        } else {
            map_jiff_err(e).to_string()
        }
    })
}

/// `TIMEZONE(name) -> jtimezone`. Accepts IANA names, `UTC`, `Z`, and
/// `+HH:MM` / `-HH:MM` fixed offsets.
struct TimezoneFn;

impl CustomFunction for TimezoneFn {
    fn name(&self) -> &str {
        "TIMEZONE"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.len() != 1 {
            return Err(FormulaError::value("TIMEZONE: expected 1 string arg").to_string());
        }
        let s = match &args[0] {
            CellValue::String(s) => s.as_str(),
            _ => {
                return Err(FormulaError::value("TIMEZONE: arg must be a string").to_string())
            }
        };
        // Validate it parses (so we don't store bogus data), then store
        // canonically — IANA names verbatim, fixed offsets normalised
        // to `+HH:MM` so user input like `+0530` and `+05:30` round-trip
        // identically.
        let tz = parse_timezone(s)
            .map_err(|e| FormulaError::value(format!("TIMEZONE: {e}")).to_string())?;
        let cv = match tz.iana_name() {
            Some(name) => pack_timezone_iana(name),
            None => {
                // Fixed-offset zone. Pull the offset by asking the zone
                // for the offset at the Unix epoch — fixed zones return
                // the same offset for any instant.
                let offset = tz.to_offset(jiff::Timestamp::UNIX_EPOCH);
                pack_timezone_offset(offset)
            }
        };
        Ok(CellValue::Custom(cv))
    }
}

/// `ZONED(datetime, tz, [disambig='compatible']) -> jzoned`.
struct ZonedFn;

impl CustomFunction for ZonedFn {
    fn name(&self) -> &str {
        "ZONED"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.len() < 2 || args.len() > 3 {
            return Err(FormulaError::value(
                "ZONED: expected 2 or 3 args (datetime, tz, [disambiguation])",
            )
            .to_string());
        }
        let dt = arg_datetime(args, 1, "ZONED")?;
        let tz = arg_timezone(args, 2, "ZONED")?;
        let disambig = arg_disambiguation(args, 3, "ZONED")?;
        let z = datetime_to_zoned(dt, tz, disambig, "ZONED")?;
        Ok(CellValue::Custom(pack_zoned(&z)))
    }
}

/// `TO_ZONED(datetime, tz, [disambig])` — alias for `ZONED`. Some users
/// reach for the `TO_*` form when explicitly converting between types.
struct ToZonedFn;

impl CustomFunction for ToZonedFn {
    fn name(&self) -> &str {
        "TO_ZONED"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        ZonedFn.call(args)
    }
}

/// `PARSE_ZONED(str, [fmt]) -> jzoned`.
struct ParseZonedFn;

impl CustomFunction for ParseZonedFn {
    fn name(&self) -> &str {
        "PARSE_ZONED"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        let (s, fmt) = parse_args("PARSE_ZONED", args)?;
        let z = match fmt {
            Some(f) => Zoned::strptime(f, s).map_err(|e| map_jiff_err(e).to_string())?,
            None => Zoned::from_str(s).map_err(|e| map_jiff_err(e).to_string())?,
        };
        Ok(CellValue::Custom(pack_zoned(&z)))
    }
}

/// `IN_TZ(zoned, tz) -> jzoned`. Same instant, different zone.
struct InTzFn;

impl CustomFunction for InTzFn {
    fn name(&self) -> &str {
        "IN_TZ"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.len() != 2 {
            return Err(FormulaError::value("IN_TZ: expected 2 args (zoned, tz)").to_string());
        }
        let z = arg_zoned(args, 1, "IN_TZ")?;
        let tz = arg_timezone(args, 2, "IN_TZ")?;
        let out = z.with_time_zone(tz);
        Ok(CellValue::Custom(pack_zoned(&out)))
    }
}

/// `TO_UTC(zoned) -> jzoned` — sugar for `IN_TZ(z, "UTC")`.
struct ToUtcFn;

impl CustomFunction for ToUtcFn {
    fn name(&self) -> &str {
        "TO_UTC"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.len() != 1 {
            return Err(FormulaError::value("TO_UTC: expected 1 jzoned arg").to_string());
        }
        let z = arg_zoned(args, 1, "TO_UTC")?;
        Ok(CellValue::Custom(pack_zoned(&z.with_time_zone(TimeZone::UTC))))
    }
}

/// `NOW() -> jzoned` in the system time zone.
struct NowFn;

impl CustomFunction for NowFn {
    fn name(&self) -> &str {
        "NOW"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if !args.is_empty() {
            return Err(FormulaError::value("NOW: expected no args").to_string());
        }
        now_impl()
    }
}

#[cfg(feature = "_has-now")]
fn now_impl() -> Result<CellValue, String> {
    Ok(CellValue::Custom(pack_zoned(&Zoned::now())))
}

#[cfg(not(feature = "_has-now"))]
fn now_impl() -> Result<CellValue, String> {
    Err(FormulaError::Custom(
        "NOW() requires the lotus-datetime `system-tz` (native) or `js-now` (wasm) feature".into(),
    )
    .to_string())
}

/// `UTCNOW() -> jzoned` in UTC. Works without `system-tz` (uses jiff's
/// fallback path); on browser WASM it requires the `js-now` feature so
/// jiff has a clock.
struct UtcNowFn;

impl CustomFunction for UtcNowFn {
    fn name(&self) -> &str {
        "UTCNOW"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if !args.is_empty() {
            return Err(FormulaError::value("UTCNOW: expected no args").to_string());
        }
        let ts = jiff::Timestamp::now();
        Ok(CellValue::Custom(pack_zoned(&ts.to_zoned(TimeZone::UTC))))
    }
}

/// `TIME(hour, minute, second, [nanos=0]) -> jtime`.
struct TimeFn;

impl CustomFunction for TimeFn {
    fn name(&self) -> &str {
        "TIME"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.len() < 3 || args.len() > 4 {
            return Err(FormulaError::value(
                "TIME: expected 3 or 4 args (hour, minute, second, [nanos])",
            )
            .to_string());
        }
        let hour = ranged_int::<i8>(arg_number(args, 1, "TIME")?, "TIME", "hour")?;
        let minute = ranged_int::<i8>(arg_number(args, 2, "TIME")?, "TIME", "minute")?;
        let second = ranged_int::<i8>(arg_number(args, 3, "TIME")?, "TIME", "second")?;
        let nanos = if args.len() == 4 {
            ranged_int::<i32>(arg_number(args, 4, "TIME")?, "TIME", "nanos")?
        } else {
            0
        };
        let t = Time::new(hour, minute, second, nanos).map_err(|e| map_jiff_err(e).to_string())?;
        Ok(CellValue::Custom(pack_time(t)))
    }
}

/// `PARSE_TIME(str, [fmt]) -> jtime`. With `fmt`, uses jiff strptime;
/// without, ISO 8601.
struct ParseTimeFn;

impl CustomFunction for ParseTimeFn {
    fn name(&self) -> &str {
        "PARSE_TIME"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        let (s, fmt) = parse_args("PARSE_TIME", args)?;
        let t = match fmt {
            Some(f) => Time::strptime(f, s).map_err(|e| map_jiff_err(e).to_string())?,
            None => Time::from_str(s).map_err(|e| map_jiff_err(e).to_string())?,
        };
        Ok(CellValue::Custom(pack_time(t)))
    }
}

/// `DATETIME(year, month, day, hour, minute, second, [nanos=0]) -> jdatetime`.
struct DateTimeFn;

impl CustomFunction for DateTimeFn {
    fn name(&self) -> &str {
        "DATETIME"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.len() < 6 || args.len() > 7 {
            return Err(FormulaError::value(
                "DATETIME: expected 6 or 7 args (year, month, day, hour, minute, second, [nanos])",
            )
            .to_string());
        }
        let year = ranged_int::<i16>(arg_number(args, 1, "DATETIME")?, "DATETIME", "year")?;
        let month = ranged_int::<i8>(arg_number(args, 2, "DATETIME")?, "DATETIME", "month")?;
        let day = ranged_int::<i8>(arg_number(args, 3, "DATETIME")?, "DATETIME", "day")?;
        let hour = ranged_int::<i8>(arg_number(args, 4, "DATETIME")?, "DATETIME", "hour")?;
        let minute = ranged_int::<i8>(arg_number(args, 5, "DATETIME")?, "DATETIME", "minute")?;
        let second = ranged_int::<i8>(arg_number(args, 6, "DATETIME")?, "DATETIME", "second")?;
        let nanos = if args.len() == 7 {
            ranged_int::<i32>(arg_number(args, 7, "DATETIME")?, "DATETIME", "nanos")?
        } else {
            0
        };
        let dt = DateTime::new(year, month, day, hour, minute, second, nanos)
            .map_err(|e| map_jiff_err(e).to_string())?;
        Ok(CellValue::Custom(pack_datetime(dt)))
    }
}

/// `PARSE_DATETIME(str, [fmt]) -> jdatetime`.
struct ParseDateTimeFn;

impl CustomFunction for ParseDateTimeFn {
    fn name(&self) -> &str {
        "PARSE_DATETIME"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        let (s, fmt) = parse_args("PARSE_DATETIME", args)?;
        let dt = match fmt {
            Some(f) => DateTime::strptime(f, s).map_err(|e| map_jiff_err(e).to_string())?,
            None => DateTime::from_str(s).map_err(|e| map_jiff_err(e).to_string())?,
        };
        Ok(CellValue::Custom(pack_datetime(dt)))
    }
}

/// Shared arg validator for the `PARSE_*` family: 1-2 string args
/// (`value, [fmt]`). The fmt arg is optional / `Empty`-skippable so
/// formulas can defensively pass through `IF(condition, fmt, "")`.
fn parse_args<'a>(
    fn_name: &str,
    args: &'a [CellValue],
) -> Result<(&'a str, Option<&'a str>), String> {
    if args.is_empty() || args.len() > 2 {
        return Err(FormulaError::value(format!(
            "{fn_name}: expected 1 or 2 string args (value, [fmt])"
        ))
        .to_string());
    }
    let s = match &args[0] {
        CellValue::String(s) => s.as_str(),
        _ => {
            return Err(FormulaError::value(format!("{fn_name}: arg #1 must be a string"))
                .to_string())
        }
    };
    let fmt = match args.get(1) {
        None | Some(CellValue::Empty) => None,
        Some(CellValue::String(s)) if s.is_empty() => None,
        Some(CellValue::String(s)) => Some(s.as_str()),
        _ => {
            return Err(FormulaError::value(format!(
                "{fn_name}: arg #2 (fmt) must be a string or omitted"
            ))
            .to_string())
        }
    };
    Ok((s, fmt))
}

/// `SPAN_TO_DAYS(span, [relative]) -> Number`. Same semantics as
/// `SPAN_TO_SECONDS` divided by 86_400.
struct SpanToDaysFn;

impl CustomFunction for SpanToDaysFn {
    fn name(&self) -> &str {
        "SPAN_TO_DAYS"
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        if args.is_empty() || args.len() > 2 {
            return Err(FormulaError::value(
                "SPAN_TO_DAYS: expected 1 or 2 args (span, [relative jdate])",
            )
            .to_string());
        }
        let span = arg_span(args, 1, "SPAN_TO_DAYS")?;
        let relative = arg_optional_date(args, 2, "SPAN_TO_DAYS")?.unwrap_or(EPOCH);
        let dur = span.to_duration(relative).map_err(|e| map_jiff_err(e).to_string())?;
        Ok(CellValue::Number(dur.as_secs_f64() / 86_400.0))
    }
}

/// Convert an `f64` arg to a small integer type, rejecting non-integer
/// values and out-of-range values with a `#VALUE!` carrying field-level
/// detail (e.g. "DATE: month must be a whole number in -128..=127").
fn ranged_int<T>(v: f64, fn_name: &str, field: &str) -> Result<T, String>
where
    T: TryFrom<i64> + RangedIntBounds,
{
    if !v.is_finite() || v.fract() != 0.0 {
        return Err(FormulaError::value(format!(
            "{fn_name}: {field} must be a whole number"
        ))
        .to_string());
    }
    let as_i64 = v as i64;
    T::try_from(as_i64).map_err(|_| {
        FormulaError::value(format!(
            "{fn_name}: {field} must be in {}..={}",
            T::MIN_DESC,
            T::MAX_DESC,
        ))
        .to_string()
    })
}

/// Tiny helper trait so `ranged_int` can render the bound description
/// for the type without hard-coding numbers.
trait RangedIntBounds {
    const MIN_DESC: &'static str;
    const MAX_DESC: &'static str;
}
impl RangedIntBounds for i8 {
    const MIN_DESC: &'static str = "-128";
    const MAX_DESC: &'static str = "127";
}
impl RangedIntBounds for i16 {
    const MIN_DESC: &'static str = "-32768";
    const MAX_DESC: &'static str = "32767";
}
impl RangedIntBounds for i32 {
    const MIN_DESC: &'static str = "-2147483648";
    const MAX_DESC: &'static str = "2147483647";
}

#[cfg(test)]
mod tests {
    use super::*;
    use lotus_core::CustomValue;

    fn jdate(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::DATE.into(), data: s.into() })
    }

    #[test]
    fn date_constructs_valid() {
        let v = DateFn
            .call(&[CellValue::Number(2025.0), CellValue::Number(4.0), CellValue::Number(27.0)])
            .unwrap();
        assert_eq!(v, jdate("2025-04-27"));
    }

    #[test]
    fn date_rejects_invalid_calendar_date() {
        let err = DateFn
            .call(&[CellValue::Number(2025.0), CellValue::Number(2.0), CellValue::Number(30.0)])
            .unwrap_err();
        assert!(err.starts_with("#VALUE!"), "got: {err}");
    }

    #[test]
    fn date_rejects_non_integer_args() {
        let err = DateFn
            .call(&[CellValue::Number(2025.5), CellValue::Number(4.0), CellValue::Number(27.0)])
            .unwrap_err();
        assert!(err.contains("whole number"), "got: {err}");
    }

    // YEAR / MONTH / DAY / DAYS_BETWEEN unit tests live in
    // tests/polymorphic_e2e.rs now that those functions take date-like
    // (not just jdate) arguments.

    #[cfg(not(feature = "_has-now"))]
    #[test]
    fn today_without_clock_source_errors() {
        let err = TodayFn.call(&[]).unwrap_err();
        // Message should point users at both the native and wasm features.
        assert!(err.contains("system-tz"), "got: {err}");
        assert!(err.contains("js-now"), "got: {err}");
    }

    #[cfg(feature = "_has-now")]
    #[test]
    fn today_with_clock_source_returns_jdate() {
        let v = TodayFn.call(&[]).unwrap();
        match v {
            CellValue::Custom(cv) => assert_eq!(cv.type_tag, tags::DATE),
            other => panic!("expected jdate, got {other:?}"),
        }
    }

    fn jspan(s: &str) -> CellValue {
        CellValue::Custom(CustomValue { type_tag: tags::SPAN.into(), data: s.into() })
    }

    #[test]
    fn unit_constructors_each_set_their_field() {
        let cases: &[(&dyn CustomFunction, &str)] = &[
            (&UnitFn { name: "YEARS", builder: |s, n| s.years(n) }, "P3Y"),
            (&UnitFn { name: "MONTHS", builder: |s, n| s.months(n) }, "P3M"),
            (&UnitFn { name: "WEEKS", builder: |s, n| s.weeks(n) }, "P3W"),
            (&UnitFn { name: "DAYS", builder: |s, n| s.days(n) }, "P3D"),
            (&UnitFn { name: "HOURS", builder: |s, n| s.hours(n) }, "PT3H"),
            (&UnitFn { name: "MINUTES", builder: |s, n| s.minutes(n) }, "PT3M"),
            (&UnitFn { name: "SECONDS", builder: |s, n| s.seconds(n) }, "PT3S"),
        ];
        for (f, expected_iso) in cases {
            let CellValue::Custom(out) = f.call(&[CellValue::Number(3.0)]).unwrap() else {
                panic!("expected jspan from {}", f.name());
            };
            assert_eq!(out.data, *expected_iso, "{}", f.name());
        }
    }

    #[test]
    fn unit_constructor_rejects_fractional() {
        let f = UnitFn { name: "DAYS", builder: |s, n| s.days(n) };
        let err = f.call(&[CellValue::Number(2.5)]).unwrap_err();
        assert!(err.contains("whole number"), "got: {err}");
    }

    #[test]
    fn span_constructor_combines_units() {
        let CellValue::Custom(out) = SpanFn
            .call(&[
                CellValue::Number(1.0),
                CellValue::Number(2.0),
                CellValue::Number(0.0),
                CellValue::Number(4.0),
                CellValue::Number(5.0),
                CellValue::Number(0.0),
                CellValue::Number(7.0),
            ])
            .unwrap()
        else {
            panic!("expected jspan");
        };
        let parsed = Span::from_str(&out.data).unwrap();
        assert_eq!(parsed.get_years(), 1);
        assert_eq!(parsed.get_months(), 2);
        assert_eq!(parsed.get_days(), 4);
        assert_eq!(parsed.get_hours(), 5);
        assert_eq!(parsed.get_seconds(), 7);
    }

    #[test]
    fn parse_span_round_trips() {
        let CellValue::Custom(out) = ParseSpanFn
            .call(&[CellValue::String("P10D".into())])
            .unwrap()
        else {
            panic!("expected jspan");
        };
        assert_eq!(Span::from_str(&out.data).unwrap().get_days(), 10);
    }

    #[test]
    fn span_to_seconds_pure_time_units() {
        // 1 hour = 3600 seconds, no relative date needed.
        assert_eq!(
            SpanToSecondsFn.call(&[jspan("PT1H")]).unwrap(),
            CellValue::Number(3600.0)
        );
    }

    #[test]
    fn span_to_seconds_calendar_uses_default_epoch() {
        // 1 month at the Unix epoch (Jan 1970) = 31 days = 2_678_400s.
        // The test pins this exact number to lock in the EPOCH-default
        // behaviour; if we ever change the default anchor this assertion
        // will flag the breakage.
        assert_eq!(
            SpanToSecondsFn.call(&[jspan("P1M")]).unwrap(),
            CellValue::Number(31.0 * 86_400.0)
        );
    }

    #[test]
    fn span_to_days_with_explicit_relative() {
        // 1 month at March 1, 2025 = 31 days (March has 31).
        let relative = jdate("2025-03-01");
        assert_eq!(
            SpanToDaysFn.call(&[jspan("P1M"), relative]).unwrap(),
            CellValue::Number(31.0)
        );
        // 1 month at Feb 1, 2025 = 28 days (non-leap Feb).
        let relative_feb = jdate("2025-02-01");
        assert_eq!(
            SpanToDaysFn.call(&[jspan("P1M"), relative_feb]).unwrap(),
            CellValue::Number(28.0)
        );
    }
}

