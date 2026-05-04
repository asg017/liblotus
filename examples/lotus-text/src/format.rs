//! Line-based `.sheet` format: `CELL_ID: raw_value`.
//!
//! Grammar (one rule per line of input):
//!
//! - Blank lines and lines whose first non-whitespace char is `#` are ignored.
//! - Every other line must match `<CELL_ID>:<sp?><raw>` where `CELL_ID` is
//!   one or more uppercase ASCII letters followed by one or more digits.
//! - The separator is the first `:` on the line. A single leading space after
//!   the colon is stripped; trailing whitespace is stripped.
//! - Duplicate cell IDs: last assignment wins.
//! - Empty raw (`A1:`) clears the cell.

use std::collections::BTreeMap;

use lotus_core::{CellValue, Sheet};

/// A parsed `.sheet` paired with an evaluated engine sheet.
///
/// `raw` preserves every authored cell in sorted (column, row) order so that
/// formatting round-trips stably. `sheet` holds the computed values and is
/// what you call for recalculation.
#[derive(Debug, Clone, Default)]
pub struct TextSheet {
    raw: BTreeMap<CellKey, String>,
    sheet: Sheet,
}

/// Sort key for cell IDs: column index first, then row.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct CellKey {
    col: u32,
    row: u32,
    id: String,
}

impl CellKey {
    fn from_id(id: &str) -> Option<Self> {
        let (col_str, row) = split_cell_id(id)?;
        Some(CellKey {
            col: col_to_index(col_str),
            row,
            id: id.to_string(),
        })
    }
}

/// A failure to parse a `.sheet` source. Line numbers are 1-based.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Outcome of a line-level classification pass.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Blank,
    Comment,
    Cell {
        cell: String,
        raw: String,
        /// The computed-value tail for snapshot files (after ` => `).
        expected: Option<String>,
    },
}

impl TextSheet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a `.sheet` (or `.sheet.snap`) source and evaluate it.
    ///
    /// The computed-value tails in a snapshot file are ignored here — use
    /// `verify_snapshot` in the `snapshot` module to check them.
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        let mut raw: BTreeMap<CellKey, String> = BTreeMap::new();

        for (i, line) in source.lines().enumerate() {
            let line_no = i + 1;
            match classify_line(line, line_no)? {
                Line::Blank | Line::Comment => continue,
                Line::Cell { cell, raw: r, .. } => {
                    let key = CellKey::from_id(&cell).ok_or_else(|| ParseError {
                        line: line_no,
                        message: format!("invalid cell id: {cell}"),
                    })?;
                    if r.is_empty() {
                        raw.remove(&key);
                    } else {
                        raw.insert(key, r);
                    }
                }
            }
        }

        let mut ts = TextSheet {
            raw,
            sheet: Sheet::new(),
        };
        ts.recalculate().map_err(|e| ParseError {
            line: 0,
            message: e,
        })?;
        Ok(ts)
    }

    /// Set one cell's raw value and recalculate.
    pub fn set(&mut self, cell: &str, raw: &str) -> Result<(), String> {
        let key = CellKey::from_id(cell).ok_or_else(|| format!("invalid cell id: {cell}"))?;
        if raw.is_empty() {
            self.raw.remove(&key);
        } else {
            self.raw.insert(key, raw.to_string());
        }
        self.recalculate()
    }

    fn recalculate(&mut self) -> Result<(), String> {
        let mut fresh = Sheet::new();
        let changes: Vec<(String, String)> = self
            .raw
            .iter()
            .map(|(k, v)| (k.id.clone(), v.clone()))
            .collect();
        fresh.set_cells(&changes).map_err(|e| e.to_string())?;
        self.sheet = fresh;
        Ok(())
    }

    pub fn get(&self, cell: &str) -> CellValue {
        self.sheet.get(cell)
    }

    pub fn get_raw(&self, cell: &str) -> Option<&str> {
        let key = CellKey::from_id(cell)?;
        self.raw.get(&key).map(|s| s.as_str())
    }

    /// Borrow the underlying evaluated sheet.
    pub fn sheet(&self) -> &Sheet {
        &self.sheet
    }

    /// Iterate authored cells in (column, row) order.
    pub fn cells(&self) -> impl Iterator<Item = (&str, &str)> {
        self.raw.iter().map(|(k, v)| (k.id.as_str(), v.as_str()))
    }

    /// Number of authored cells.
    pub fn len(&self) -> usize {
        self.raw.len()
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// Render in canonical `.sheet` form.
    pub fn format_compact(&self) -> String {
        let mut out = String::new();
        for (id, raw) in self.cells() {
            out.push_str(id);
            out.push_str(": ");
            out.push_str(raw);
            out.push('\n');
        }
        out
    }
}

/// Free function mirror of `TextSheet::parse`.
pub fn parse(source: &str) -> Result<TextSheet, ParseError> {
    TextSheet::parse(source)
}

/// Free function mirror of `TextSheet::format_compact`.
pub fn format_compact(sheet: &TextSheet) -> String {
    sheet.format_compact()
}

/// Classify one source line. Exposed for the linter and the snapshot parser.
pub fn classify_line(line: &str, line_no: usize) -> Result<Line, ParseError> {
    let trimmed_start = line.trim_start();
    if trimmed_start.is_empty() {
        return Ok(Line::Blank);
    }
    if trimmed_start.starts_with('#') {
        return Ok(Line::Comment);
    }
    if trimmed_start.len() != line.len() {
        return Err(ParseError {
            line: line_no,
            message: "unexpected leading whitespace".into(),
        });
    }

    let colon = line.find(':').ok_or_else(|| ParseError {
        line: line_no,
        message: "expected `CELL: value`".into(),
    })?;
    let cell = &line[..colon];
    if !is_valid_cell_id(cell) {
        return Err(ParseError {
            line: line_no,
            message: format!("invalid cell id: {cell:?}"),
        });
    }

    let rest = &line[colon + 1..];
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    let rest = rest.trim_end();

    let (raw, expected) = match split_snapshot_tail(rest) {
        Some((raw, tail)) => (raw.to_string(), Some(tail.to_string())),
        None => (rest.to_string(), None),
    };

    Ok(Line::Cell {
        cell: cell.to_string(),
        raw,
        expected,
    })
}

/// Split an ` => computed` tail off a raw value, if one is present.
///
/// We look for ` => ` as a delimited sequence so that `>=` in formulas is
/// never misinterpreted.
fn split_snapshot_tail(s: &str) -> Option<(&str, &str)> {
    let idx = s.find(" => ")?;
    Some((&s[..idx], &s[idx + 4..]))
}

fn is_valid_cell_id(s: &str) -> bool {
    split_cell_id(s).is_some()
}

/// Split `"AA12"` into `("AA", 12)`.
fn split_cell_id(s: &str) -> Option<(&str, u32)> {
    if s.is_empty() {
        return None;
    }
    let digit_start = s.find(|c: char| c.is_ascii_digit())?;
    if digit_start == 0 {
        return None;
    }
    let col = &s[..digit_start];
    if !col.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let row: u32 = s[digit_start..].parse().ok()?;
    if row == 0 {
        return None;
    }
    Some((col, row))
}

fn col_to_index(col: &str) -> u32 {
    let mut idx = 0u32;
    for b in col.bytes() {
        idx = idx * 26 + (b - b'A') as u32 + 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let src = "A1: =SUM(B1:B3)\nB1: 10\nB2: 20\nB3: 30\n";
        let ts = parse(src).unwrap();
        assert_eq!(ts.get("A1"), CellValue::Number(60.0));
        assert_eq!(ts.get_raw("A1"), Some("=SUM(B1:B3)"));
        assert_eq!(ts.len(), 4);
    }

    #[test]
    fn parse_ignores_blank_and_comments() {
        let src = "\n# a comment\nA1: 1\n\n# another\nB1: 2\n";
        let ts = parse(src).unwrap();
        assert_eq!(ts.len(), 2);
    }

    #[test]
    fn parse_strips_snapshot_tail() {
        let src = "A1: =1+1 => 2\n";
        let ts = parse(src).unwrap();
        assert_eq!(ts.get_raw("A1"), Some("=1+1"));
        assert_eq!(ts.get("A1"), CellValue::Number(2.0));
    }

    #[test]
    fn parse_empty_raw_clears() {
        let src = "A1: 1\nA1:\n";
        let ts = parse(src).unwrap();
        assert_eq!(ts.get_raw("A1"), None);
        assert_eq!(ts.get("A1"), CellValue::Empty);
    }

    #[test]
    fn parse_duplicate_last_wins() {
        let ts = parse("A1: 1\nA1: 2\n").unwrap();
        assert_eq!(ts.get_raw("A1"), Some("2"));
    }

    #[test]
    fn parse_rejects_leading_whitespace() {
        let err = parse("  A1: 1\n").unwrap_err();
        assert_eq!(err.line, 1);
    }

    #[test]
    fn parse_rejects_bad_cell_id() {
        let err = parse("1A: 1\n").unwrap_err();
        assert_eq!(err.line, 1);
    }

    #[test]
    fn parse_rejects_missing_colon() {
        let err = parse("A1 = 1\n").unwrap_err();
        assert_eq!(err.line, 1);
    }

    #[test]
    fn format_compact_sorts_column_major() {
        let ts = parse("B2: 4\nA1: 1\nB1: 3\nA2: 2\n").unwrap();
        assert_eq!(ts.format_compact(), "A1: 1\nA2: 2\nB1: 3\nB2: 4\n");
    }

    #[test]
    fn format_compact_handles_aa_correctly() {
        let ts = parse("AA1: x\nZ1: y\n").unwrap();
        // Z (col 26) sorts before AA (col 27).
        assert_eq!(ts.format_compact(), "Z1: y\nAA1: x\n");
    }

    #[test]
    fn round_trip_preserves_authored_cells() {
        let src = "A1: =SUM(B1:B3)\nB1: 10\nB2: 20\nB3: 30\n";
        let ts = parse(src).unwrap();
        let round = parse(&ts.format_compact()).unwrap();
        assert_eq!(ts.format_compact(), round.format_compact());
        assert_eq!(round.get("A1"), CellValue::Number(60.0));
    }

    #[test]
    fn set_and_clear() {
        let mut ts = TextSheet::new();
        ts.set("A1", "10").unwrap();
        ts.set("B1", "=A1*2").unwrap();
        assert_eq!(ts.get("B1"), CellValue::Number(20.0));
        ts.set("A1", "").unwrap();
        assert_eq!(ts.get_raw("A1"), None);
    }

    #[test]
    fn does_not_strip_trailing_space_inside_string_value() {
        // Trailing whitespace is trimmed by design. Documented here.
        let ts = parse("A1: hello   \n").unwrap();
        assert_eq!(ts.get_raw("A1"), Some("hello"));
    }
}
