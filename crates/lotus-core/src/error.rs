//! Errors produced by `lotus-core`.
//!
//! Two distinct shapes:
//!
//! - [`SheetError`] — top-level errors from the public [`crate::Sheet`]
//!   mutation API (`set_cells`, `set_name`, `remove_name`). A real Rust
//!   error: callers handle it as `Err(SheetError::...)`.
//! - [`FormulaError`] — per-cell formula errors that show up *as cell
//!   values* (`#DIV/0!`, `#REF!`, etc.). Internally the evaluator
//!   bubbles these up via `Result<_, FormulaError>`, but the recalc loop
//!   catches them and stores them in the cell as
//!   [`crate::CellValue::Error`] — they're a value-vehicle pattern, not
//!   an error condition for the Rust caller.

use crate::types::CellId;

/// Errors returned by the public [`crate::Sheet`] mutation API
/// (`set_cells`, `set_name`, `remove_name`, …).
///
/// `Display` reproduces the strings these methods used to return when
/// they were typed as `Result<(), String>`, so binding crates that
/// surface the message directly (lotus-pyo3, lotus-wasm) keep their
/// observable behaviour. New Rust callers can match on the variant.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum SheetError {
    /// A circular dependency was detected during recalculation.
    /// `cells` lists every cell id the topo-sort failed to visit,
    /// sorted so the rendered message is reproducible across runs.
    #[error("#CIRCULAR! Cycle detected involving: {}", .cells.join(", "))]
    Circular { cells: Vec<CellId> },

    /// A name failed validation (empty, lexes as a cell ref, shadows
    /// a builtin, …) — surfaced from `set_name`.
    #[error("{0}")]
    InvalidName(String),

    /// A name's definition failed to parse — surfaced from `set_name`.
    #[error("{0}")]
    InvalidDefinition(String),
}

/// Per-cell formula errors. The evaluator bubbles these up via
/// `Result<_, FormulaError>`; the recalc loop catches them and stores
/// the cell's value as [`crate::CellValue::Error`], so downstream
/// consumers (other formulas, bindings, the UI) see them as cell values.
///
/// `Display` reproduces the legacy sentinel strings byte-for-byte
/// (`#DIV/0!`, `#REF!`, `#VALUE!`, `#VALUE! IF: incompatible cols`, …),
/// so any consumer that calls `.to_string()` on the cell value sees the
/// same text it always did. New Rust callers can match on the variant
/// for typed dispatch (e.g. an `IFERROR` implementation).
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum FormulaError {
    /// `#DIV/0!` — division by zero.
    #[error("#DIV/0!")]
    DivByZero,

    /// `#REF!` — reference to a non-existent or invalid cell.
    #[error("#REF!")]
    Ref,

    /// `#NAME?` — unknown function or named range.
    #[error("#NAME?")]
    Name,

    /// `#SPILL!` — spill blocked by another value occupying the target.
    #[error("#SPILL!")]
    Spill,

    /// `#LOADING!` — host-injected sentinel for asynchronously-fetched
    /// values that aren't ready yet.
    #[error("#LOADING!")]
    Loading,

    /// `#N/A` — value not available.
    #[error("#N/A")]
    NotAvailable,

    /// `#CIRCULAR!` — only used internally when a per-cell evaluation
    /// dependency loop is detected; top-level circular dependencies are
    /// surfaced as [`SheetError::Circular`].
    #[error("#CIRCULAR!")]
    Circular,

    /// `#VALUE!` with an optional detail message
    /// (e.g. `"IF: incompatible cols"`).
    #[error("{}", format_with_detail("#VALUE!", .0.as_deref()))]
    Value(Option<String>),

    /// `#SIZE!` with an optional detail
    /// (e.g. `"range exceeds 1048576 cells"`).
    #[error("{}", format_with_detail("#SIZE!", .0.as_deref()))]
    Size(Option<String>),

    /// Free-form error message from a custom function or unrecognised
    /// engine error string. Displayed verbatim.
    #[error("{0}")]
    Custom(String),
}

impl FormulaError {
    /// Constructor for `#VALUE!` with a `'static` detail string —
    /// shorthand for `FormulaError::Value(Some(detail.into()))`.
    pub fn value(detail: impl Into<String>) -> Self {
        FormulaError::Value(Some(detail.into()))
    }

    /// Constructor for `#SIZE!` with a detail string.
    pub fn size(detail: impl Into<String>) -> Self {
        FormulaError::Size(Some(detail.into()))
    }
}

/// Converts an untyped error string into a [`FormulaError`]. Recognises
/// the legacy `#FOO!` sentinels (with or without a detail tail) so that
/// existing code paths (parser errors, custom-function errors,
/// string-typed bubbling) round-trip without losing the variant.
/// Anything that doesn't look like a known sentinel becomes
/// [`FormulaError::Custom`].
impl From<String> for FormulaError {
    fn from(s: String) -> Self {
        // Helper: pull off "#PREFIX!" / "#PREFIX! detail" → (rest after prefix, opt detail)
        fn strip_prefix<'a>(s: &'a str, prefix: &str) -> Option<Option<&'a str>> {
            let rest = s.strip_prefix(prefix)?;
            if rest.is_empty() {
                Some(None)
            } else {
                Some(Some(rest.trim_start()))
            }
        }

        if s == "#DIV/0!" {
            return FormulaError::DivByZero;
        }
        if s == "#REF!" {
            return FormulaError::Ref;
        }
        if s == "#NAME?" {
            return FormulaError::Name;
        }
        if s == "#SPILL!" {
            return FormulaError::Spill;
        }
        if s == "#LOADING!" {
            return FormulaError::Loading;
        }
        if s == "#N/A" {
            return FormulaError::NotAvailable;
        }
        if let Some(rest) = strip_prefix(&s, "#CIRCULAR!") {
            return match rest {
                None | Some("") => FormulaError::Circular,
                _ => FormulaError::Custom(s),
            };
        }
        if let Some(detail) = strip_prefix(&s, "#VALUE!") {
            return FormulaError::Value(detail.map(str::to_string));
        }
        if let Some(detail) = strip_prefix(&s, "#SIZE!") {
            return FormulaError::Size(detail.map(str::to_string));
        }
        FormulaError::Custom(s)
    }
}

impl From<&str> for FormulaError {
    fn from(s: &str) -> Self {
        FormulaError::from(s.to_string())
    }
}

fn format_with_detail(prefix: &str, detail: Option<&str>) -> String {
    match detail {
        Some(d) if !d.is_empty() => format!("{prefix} {d}"),
        _ => prefix.to_string(),
    }
}
