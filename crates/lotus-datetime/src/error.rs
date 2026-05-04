use lotus_core::FormulaError;

/// Map a `jiff::Error` into the `FormulaError` shape expected by the
/// engine. Range overflow surfaces as `#VALUE! out of range: ...`;
/// every other jiff error (invalid parameter, parse failure, …) becomes
/// a `#VALUE!` carrying jiff's native message — these are all "the cell
/// holds a value jiff can't make sense of" and Excel-family tools spell
/// that exact failure mode `#VALUE!`.
pub fn map_jiff_err(e: jiff::Error) -> FormulaError {
    if e.is_range() {
        FormulaError::value(format!("out of range: {e}"))
    } else {
        FormulaError::value(e.to_string())
    }
}
