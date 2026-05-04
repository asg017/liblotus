//! A1-notation range parsing.
//!
//! The formula engine's lexer/parser already handles cell refs and ranges
//! inside expressions, but external callers (the Python backend, the
//! browser frontend) need to parse standalone range strings for UI and
//! storage concerns. This module exposes a small, forgiving parser so
//! those callers don't have to reimplement it.


/// Zero-based, inclusive cell coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellCoord {
    pub row: u32,
    pub col: u32,
}

/// A parsed A1-style range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRange {
    /// Inclusive top-left corner.
    pub start: CellCoord,
    /// Inclusive end row. `None` means the range is unbounded downward
    /// (`A:F`, `A1:F`, `B5:D`); callers substitute their own max.
    pub end_row: Option<u32>,
    /// Inclusive end column.
    pub end_col: u32,
    /// Input text, uppercased with whitespace and `$` stripped.
    pub normalized: String,
}

impl ParsedRange {
    pub fn is_unbounded_rows(&self) -> bool {
        self.end_row.is_none()
    }

    pub fn resolve_end_row(&self, fallback: u32) -> u32 {
        self.end_row.unwrap_or(fallback)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeParseError {
    Empty,
    UnknownShape,
    InvalidColumn(String),
    InvalidRow(String),
}

impl std::fmt::Display for RangeParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RangeParseError::Empty => write!(f, "Empty: range string is empty"),
            RangeParseError::UnknownShape => {
                write!(f, "UnknownShape: input does not match any supported range form")
            }
            RangeParseError::InvalidColumn(c) => write!(f, "InvalidColumn: {c:?}"),
            RangeParseError::InvalidRow(r) => write!(f, "InvalidRow: {r:?}"),
        }
    }
}

impl std::error::Error for RangeParseError {}

fn normalize(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '$')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Split a token into leading letters and trailing digits. Returns `None`
/// if there are no leading letters.
fn split_letters_digits(s: &str) -> Option<(&str, &str)> {
    let split = s
        .char_indices()
        .find(|(_, c)| !c.is_ascii_alphabetic())
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    if split == 0 {
        return None;
    }
    Some(s.split_at(split))
}

/// Convert column letters to a zero-based column index.
///
/// `"A"` → `0`, `"Z"` → `25`, `"AA"` → `26`. Accepts lowercase too
/// (matching `parse_cell_id`'s tolerance). Errors on empty input,
/// non-letter characters, or overflow past `u32::MAX`.
pub fn col_to_index(letters: &str) -> Result<u32, RangeParseError> {
    if letters.is_empty() || !letters.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(RangeParseError::InvalidColumn(letters.to_string()));
    }
    let mut idx: u32 = 0;
    for b in letters.bytes() {
        let shift = (b.to_ascii_uppercase() - b'A') as u32 + 1;
        idx = idx
            .checked_mul(26)
            .and_then(|n| n.checked_add(shift))
            .ok_or_else(|| RangeParseError::InvalidColumn(letters.to_string()))?;
    }
    Ok(idx - 1)
}

/// Convert a zero-based column index back to uppercase letters.
///
/// `0` → `"A"`, `25` → `"Z"`, `26` → `"AA"`. The inverse of
/// [`col_to_index`]. Saturates at `u32::MAX` (matching the core's
/// unchecked 1-based helper); never panics.
pub fn index_to_col(index: u32) -> String {
    crate::types::index_to_col(index.saturating_add(1))
}

/// Build a canonical A1-style cell id from zero-based `(row, col)`.
///
/// `(0, 0)` → `"A1"`, `(11, 1)` → `"B12"`, `(0, 26)` → `"AA1"`.
pub fn cell_id(row: u32, col: u32) -> String {
    format_cell_id(CellCoord { row, col })
}

/// Parse an optional trailing row portion. Returns `Ok(None)` if empty.
fn parse_row_opt(rest: &str) -> Result<Option<u32>, RangeParseError> {
    if rest.is_empty() {
        return Ok(None);
    }
    if !rest.bytes().all(|b| b.is_ascii_digit()) {
        return Err(RangeParseError::UnknownShape);
    }
    let row: u32 = rest
        .parse()
        .map_err(|_| RangeParseError::InvalidRow(rest.to_string()))?;
    if row == 0 {
        return Err(RangeParseError::InvalidRow(rest.to_string()));
    }
    Ok(Some(row))
}

/// Parse a single A1-style cell id like `"A1"` or `"AB42"`.
pub fn parse_cell_id(input: &str) -> Result<CellCoord, RangeParseError> {
    let n = normalize(input);
    if n.is_empty() {
        return Err(RangeParseError::Empty);
    }
    if n.contains(':') {
        return Err(RangeParseError::UnknownShape);
    }
    let (letters, rest) = split_letters_digits(&n).ok_or(RangeParseError::UnknownShape)?;
    let row_1based = parse_row_opt(rest)?.ok_or(RangeParseError::UnknownShape)?;
    let col = col_to_index(letters)?;
    Ok(CellCoord {
        row: row_1based - 1,
        col,
    })
}

/// Parse an A1-style range. Supports the bounded form (`A1:F10`), the
/// whole-column form (`A:F`), and the partially-unbounded form
/// (`A1:F`, `B5:D`). Reversed corners are normalized so `start` is
/// always the top-left.
pub fn parse_range(input: &str) -> Result<ParsedRange, RangeParseError> {
    let normalized = normalize(input);
    if normalized.is_empty() {
        return Err(RangeParseError::Empty);
    }

    let parts: Vec<&str> = normalized.split(':').collect();
    if parts.len() != 2 {
        return Err(RangeParseError::UnknownShape);
    }
    let (a, b) = (parts[0], parts[1]);
    if a.is_empty() || b.is_empty() {
        return Err(RangeParseError::UnknownShape);
    }

    let (a_letters, a_rest) = split_letters_digits(a).ok_or(RangeParseError::UnknownShape)?;
    let (b_letters, b_rest) = split_letters_digits(b).ok_or(RangeParseError::UnknownShape)?;

    let a_row = parse_row_opt(a_rest)?;
    let b_row = parse_row_opt(b_rest)?;

    let a_col = col_to_index(a_letters)?;
    let b_col = col_to_index(b_letters)?;

    // Row semantics map the four (Some/None) combinations to our three
    // supported shapes. `A:F10` (start missing row, end has one) is the
    // one combination we reject.
    let (start_row_1based, end_row_1based) = match (a_row, b_row) {
        (Some(ar), Some(br)) => {
            let (s, e) = if ar <= br { (ar, br) } else { (br, ar) };
            (s, Some(e))
        }
        (None, None) => (1, None),
        (Some(ar), None) => (ar, None),
        (None, Some(_)) => return Err(RangeParseError::UnknownShape),
    };

    let (start_col, end_col) = if a_col <= b_col {
        (a_col, b_col)
    } else {
        (b_col, a_col)
    };

    Ok(ParsedRange {
        start: CellCoord {
            row: start_row_1based - 1,
            col: start_col,
        },
        end_col,
        end_row: end_row_1based.map(|r| r - 1),
        normalized,
    })
}

/// Lightweight predicate for callers that only care whether a range can
/// grow downward. Garbage input returns `false` rather than erroring.
pub fn is_unbounded_range(input: &str) -> bool {
    parse_range(input)
        .map(|r| r.is_unbounded_rows())
        .unwrap_or(false)
}

pub fn format_cell_id(coord: CellCoord) -> String {
    format!("{}{}", crate::types::index_to_col(coord.col + 1), coord.row + 1)
}

pub fn format_range(r: &ParsedRange) -> String {
    let start_col = crate::types::index_to_col(r.start.col + 1);
    let end_col = crate::types::index_to_col(r.end_col + 1);
    match r.end_row {
        Some(er) => format!("{}{}:{}{}", start_col, r.start.row + 1, end_col, er + 1),
        None => format!("{}{}:{}", start_col, r.start.row + 1, end_col),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coord(row: u32, col: u32) -> CellCoord {
        CellCoord { row, col }
    }

    #[test]
    fn bounded_rectangle() {
        let r = parse_range("A1:F10").unwrap();
        assert_eq!(r.start, coord(0, 0));
        assert_eq!(r.end_col, 5);
        assert_eq!(r.end_row, Some(9));
        assert!(!r.is_unbounded_rows());
    }

    #[test]
    fn single_cell_range() {
        let r = parse_range("A1:A1").unwrap();
        assert_eq!(r.start, coord(0, 0));
        assert_eq!(r.end_col, 0);
        assert_eq!(r.end_row, Some(0));
    }

    #[test]
    fn whole_column_range() {
        let r = parse_range("A:F").unwrap();
        assert_eq!(r.start, coord(0, 0));
        assert_eq!(r.end_col, 5);
        assert_eq!(r.end_row, None);
        assert!(r.is_unbounded_rows());
    }

    #[test]
    fn start_cell_to_column_end() {
        let r = parse_range("A1:F").unwrap();
        assert_eq!(r.start, coord(0, 0));
        assert_eq!(r.end_col, 5);
        assert_eq!(r.end_row, None);
        assert!(r.is_unbounded_rows());
    }

    #[test]
    fn offset_cell_to_column_end() {
        let r = parse_range("B5:D").unwrap();
        assert_eq!(r.start, coord(4, 1));
        assert_eq!(r.end_col, 3);
        assert_eq!(r.end_row, None);
    }

    #[test]
    fn lower_case_matches_upper_case() {
        let lower = parse_range("a1:f").unwrap();
        let upper = parse_range("A1:F").unwrap();
        assert_eq!(lower, upper);
        assert_eq!(lower.normalized, "A1:F");
    }

    #[test]
    fn dollar_signs_stripped() {
        let r = parse_range("$A$1:$F$10").unwrap();
        assert_eq!(r.start, coord(0, 0));
        assert_eq!(r.end_col, 5);
        assert_eq!(r.end_row, Some(9));
        assert_eq!(r.normalized, "A1:F10");
    }

    #[test]
    fn reversed_rectangle_normalized() {
        let r = parse_range("F10:A1").unwrap();
        assert_eq!(r.start, coord(0, 0));
        assert_eq!(r.end_col, 5);
        assert_eq!(r.end_row, Some(9));
    }

    #[test]
    fn reversed_columns_normalized() {
        let r = parse_range("Z:A").unwrap();
        assert_eq!(r.start, coord(0, 0));
        assert_eq!(r.end_col, 25);
        assert_eq!(r.end_row, None);
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(parse_range(""), Err(RangeParseError::Empty));
        assert_eq!(parse_range("   "), Err(RangeParseError::Empty));
    }

    #[test]
    fn rejects_single_cell() {
        assert_eq!(parse_range("A1"), Err(RangeParseError::UnknownShape));
    }

    #[test]
    fn rejects_missing_end() {
        assert_eq!(parse_range("A1:"), Err(RangeParseError::UnknownShape));
    }

    #[test]
    fn rejects_missing_start() {
        assert_eq!(parse_range(":F10"), Err(RangeParseError::UnknownShape));
    }

    #[test]
    fn rejects_three_parts() {
        assert_eq!(
            parse_range("A1:F10:Z20"),
            Err(RangeParseError::UnknownShape)
        );
    }

    #[test]
    fn rejects_no_columns() {
        assert_eq!(parse_range("1:10"), Err(RangeParseError::UnknownShape));
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse_range("A1:F10 extra junk").is_err());
    }

    #[test]
    fn rejects_row_zero() {
        assert!(matches!(
            parse_range("A0:B1"),
            Err(RangeParseError::InvalidRow(_))
        ));
    }

    #[test]
    fn rejects_bare_column_to_row_cell() {
        // `A:F10` is not one of the supported shapes.
        assert_eq!(parse_range("A:F10"), Err(RangeParseError::UnknownShape));
    }

    #[test]
    fn overflowing_column_errors() {
        // 8 letters of A overflows u32.
        let res = parse_range("AAAAAAAA1:B2");
        assert!(matches!(res, Err(RangeParseError::InvalidColumn(_))));
    }

    #[test]
    fn is_unbounded_helper() {
        assert!(is_unbounded_range("A:F"));
        assert!(is_unbounded_range("A1:F"));
        assert!(is_unbounded_range("B5:D"));
        assert!(!is_unbounded_range("A1:F10"));
        assert!(!is_unbounded_range("garbage"));
        assert!(!is_unbounded_range(""));
    }

    #[test]
    fn parse_cell_id_basic() {
        assert_eq!(parse_cell_id("A1").unwrap(), coord(0, 0));
        assert_eq!(parse_cell_id("AB42").unwrap(), coord(41, 27));
        assert_eq!(parse_cell_id("$A$1").unwrap(), coord(0, 0));
    }

    #[test]
    fn parse_cell_id_rejects_ranges_and_junk() {
        assert!(matches!(
            parse_cell_id("A1:B2"),
            Err(RangeParseError::UnknownShape)
        ));
        assert!(parse_cell_id("").is_err());
        assert!(parse_cell_id("1").is_err());
        assert!(parse_cell_id("A").is_err());
        assert!(parse_cell_id("A1X").is_err());
    }

    #[test]
    fn format_round_trips() {
        // `format_range` emits a canonical form, so compare coordinates
        // rather than the full struct (the `normalized` input differs).
        for input in ["A1:F10", "A1:A1", "A:F", "A1:F", "B5:D"] {
            let parsed = parse_range(input).unwrap();
            let formatted = format_range(&parsed);
            let reparsed = parse_range(&formatted).unwrap();
            assert_eq!(parsed.start, reparsed.start, "start differs for {input}");
            assert_eq!(parsed.end_col, reparsed.end_col, "end_col differs for {input}");
            assert_eq!(parsed.end_row, reparsed.end_row, "end_row differs for {input}");
        }
    }

    #[test]
    fn format_cell_id_round_trips() {
        for input in ["A1", "AB42", "Z99"] {
            let c = parse_cell_id(input).unwrap();
            assert_eq!(format_cell_id(c), input);
        }
    }

    #[test]
    fn resolve_end_row_applies_fallback() {
        let bounded = parse_range("A1:F10").unwrap();
        assert_eq!(bounded.resolve_end_row(999), 9);
        let unbounded = parse_range("A1:F").unwrap();
        assert_eq!(unbounded.resolve_end_row(999), 999);
    }

    #[test]
    fn index_to_col_basic() {
        assert_eq!(index_to_col(0), "A");
        assert_eq!(index_to_col(25), "Z");
        assert_eq!(index_to_col(26), "AA");
        assert_eq!(index_to_col(701), "ZZ");
        assert_eq!(index_to_col(702), "AAA");
    }

    #[test]
    fn col_to_index_basic() {
        assert_eq!(col_to_index("A").unwrap(), 0);
        assert_eq!(col_to_index("Z").unwrap(), 25);
        assert_eq!(col_to_index("AA").unwrap(), 26);
        assert_eq!(col_to_index("ZZ").unwrap(), 701);
        assert_eq!(col_to_index("AAA").unwrap(), 702);
    }

    #[test]
    fn col_to_index_accepts_lowercase() {
        assert_eq!(col_to_index("a").unwrap(), 0);
        assert_eq!(col_to_index("aa").unwrap(), 26);
        assert_eq!(col_to_index("Ab").unwrap(), 27);
    }

    #[test]
    fn col_to_index_rejects_non_letters() {
        assert!(matches!(
            col_to_index(""),
            Err(RangeParseError::InvalidColumn(_))
        ));
        assert!(matches!(
            col_to_index("A1"),
            Err(RangeParseError::InvalidColumn(_))
        ));
        assert!(matches!(
            col_to_index(" A"),
            Err(RangeParseError::InvalidColumn(_))
        ));
    }

    #[test]
    fn col_to_index_overflows_on_huge_input() {
        assert!(matches!(
            col_to_index("AAAAAAAA"),
            Err(RangeParseError::InvalidColumn(_))
        ));
    }

    #[test]
    fn cell_id_basic() {
        assert_eq!(cell_id(0, 0), "A1");
        assert_eq!(cell_id(11, 1), "B12");
        assert_eq!(cell_id(0, 26), "AA1");
        assert_eq!(cell_id(99, 14), "O100");
    }

    #[test]
    fn index_col_round_trips() {
        for idx in [0u32, 1, 25, 26, 27, 701, 702, 10_000] {
            assert_eq!(col_to_index(&index_to_col(idx)).unwrap(), idx);
        }
    }
}
