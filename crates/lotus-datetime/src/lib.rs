//! jiff-backed datetime extension for `lotus-core`.
//!
//! Registers six custom cell types — `jdate`, `jtime`, `jdatetime`,
//! `jzoned`, `jtimezone`, `jspan` — and a catalogue of formula functions
//! (`DATE`, `NOW`, `YEAR`, `DAYS_BETWEEN`, …) that operate on them.
//!
//! ## Usage
//!
//! ```no_run
//! use std::sync::Arc;
//! use lotus_core::{Registry, Sheet};
//!
//! let mut reg = Registry::new();
//! lotus_datetime::register(&mut reg).expect("registry is empty");
//! let mut sheet = Sheet::new_with_registry(Arc::new(reg));
//! sheet.set_cells(&[("A1".into(), "=DATE(2025, 4, 27)".into())]).unwrap();
//! ```
//!
//! ## Type-tag prefix `j`
//!
//! Every type tag is prefixed `j` (`jdate`, `jspan`, …) to mark it as
//! jiff-handled, leaving room for alternative implementations on top of
//! other datetime libraries (`chrono_*`, `time_*`) without collision.
//!
//! ## Operator-dispatch ownership
//!
//! When two custom values meet a binary operator, the engine iterates
//! every registered handler and the first to claim the op wins. To keep
//! that orderly, the convention here is: **the handler whose type is the
//! *result* owns the dispatch**. So `DateHandler` owns `Date ± Span`
//! (result is `Date`), and `SpanHandler` owns `Date - Date` (result is
//! `Span`).

mod accessors;
mod date;
mod datetime;
mod error;
mod functions;
mod span;
mod time;
mod timezone;
mod zoned;

pub use accessors::format_custom_value;
pub use date::DateHandler;
pub use datetime::DateTimeHandler;
pub use error::map_jiff_err;
pub use span::SpanHandler;
pub use time::TimeHandler;
pub use timezone::TimeZoneHandler;
pub use zoned::ZonedHandler;

use std::sync::Arc;

use lotus_core::{Registry, RegistryError, Sheet};

/// Type-tag constants used by every handler. Centralised so handlers can
/// pattern-match on stable `&'static str` rather than duplicating literals.
pub mod tags {
    pub const DATE: &str = "jdate";
    pub const TIME: &str = "jtime";
    pub const DATETIME: &str = "jdatetime";
    pub const ZONED: &str = "jzoned";
    pub const TIMEZONE: &str = "jtimezone";
    pub const SPAN: &str = "jspan";
}

/// Today's date in the system zone, formatted as `YYYY-MM-DD`. Used by
/// host UIs that want to insert a literal "current date" into a cell
/// (e.g. vlotus's `:today` / `ctrl-;`). Requires the `system-tz` or
/// `js-now` feature on the underlying jiff dep — the registration
/// helpers in this crate already enforce that for `=NOW()`/`=TODAY()`.
#[cfg(feature = "_has-now")]
pub fn today_iso() -> String {
    jiff::Zoned::now().date().to_string()
}

/// Current local datetime as an ISO 8601 string with `T` separator and
/// second precision, e.g. `2026-05-01T13:42:33`. Companion to
/// [`today_iso`] for the `:now` / `ctrl-shift-;` flow.
#[cfg(feature = "_has-now")]
pub fn now_iso() -> String {
    let dt = jiff::Zoned::now().datetime();
    // jiff's Display for civil DateTime emits ISO with `T` and
    // second-precision; truncate any subsecond noise for stability.
    let mut s = dt.to_string();
    if let Some(dot) = s.find('.') {
        s.truncate(dot);
    }
    s
}

/// Register every datetime type and function on `registry`.
///
/// Errors if any of the type tags or function names collide with values
/// already registered (e.g. another extension claiming `jdate`).
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    // Registration order doubles as `parse_literal` priority — the first
    // handler to claim a string wins. Order from most specific shape
    // (the longest, most-anchored prefix) to least specific:
    //   - jzoned:    requires `[…]` IANA suffix (must come before jdatetime)
    //   - jdatetime: 16+ chars, requires `T` at index 10
    //   - jdate:     exactly 10 chars `YYYY-MM-DD`
    //   - jtime:     `HH:MM[:SS[.fff]]`
    //   - jspan:     starts with `P`/digit-then-unit
    //   - jtimezone: never auto-claims (would shadow strings like "UTC")
    registry.register_type(Arc::new(zoned::ZonedHandler))?;
    registry.register_type(Arc::new(datetime::DateTimeHandler))?;
    registry.register_type(Arc::new(date::DateHandler))?;
    registry.register_type(Arc::new(time::TimeHandler))?;
    registry.register_type(Arc::new(span::SpanHandler))?;
    registry.register_type(Arc::new(timezone::TimeZoneHandler))?;
    functions::register_date_functions(registry)?;
    functions::register_span_functions(registry)?;
    functions::register_time_functions(registry)?;
    functions::register_datetime_functions(registry)?;
    functions::register_zoned_functions(registry)?;
    accessors::register_accessors(registry)?;
    Ok(())
}

/// Convenience for binding crates that hold a `Sheet` instead of a
/// raw `Registry`. Equivalent to building a registry via [`register`]
/// and constructing a sheet around it, but works on an already-built
/// sheet — useful from [`lotus-wasm`] / [`lotus-pyo3`] which expose a
/// `Sheet` over their FFI boundary.
pub fn register_on_sheet(sheet: &mut Sheet) -> Result<(), RegistryError> {
    // Build the full registry first (this is the same code path as
    // `register`), then clone-on-write into the sheet's registry. The
    // `Arc::make_mut` inside `Sheet`'s register hooks keeps the
    // post-construction case allocation-light.
    for handler in handler_arcs() {
        sheet.register_type(handler)?;
    }
    for func in function_arcs() {
        sheet.register_function(func)?;
    }
    Ok(())
}

/// Every type handler this crate ships, in literal-priority order.
fn handler_arcs() -> Vec<Arc<dyn lotus_core::CustomTypeHandler>> {
    vec![
        Arc::new(zoned::ZonedHandler),
        Arc::new(datetime::DateTimeHandler),
        Arc::new(date::DateHandler),
        Arc::new(time::TimeHandler),
        Arc::new(span::SpanHandler),
        Arc::new(timezone::TimeZoneHandler),
    ]
}

/// Every formula function this crate ships, in registration order.
/// Materialised by routing through a temporary `Registry` so the
/// existing per-module `register_*_functions` helpers keep their
/// `&mut Registry` signature (they own collision detection internally).
fn function_arcs() -> Vec<Arc<dyn lotus_core::CustomFunction>> {
    let mut tmp = Registry::new();
    let _ = functions::register_date_functions(&mut tmp);
    let _ = functions::register_span_functions(&mut tmp);
    let _ = functions::register_time_functions(&mut tmp);
    let _ = functions::register_datetime_functions(&mut tmp);
    let _ = functions::register_zoned_functions(&mut tmp);
    let _ = accessors::register_accessors(&mut tmp);
    tmp.into_functions()
}
