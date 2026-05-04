//! Snapshot format: a `.sheet` with ` => <computed>` tails on each cell line.
//!
//! ```text
//! A1: =SUM(B1:B3) => 60
//! B1: 10 => 10
//! B2: 20 => 20
//! B3: 30 => 30
//! ```
//!
//! Because the delimiter is the literal sequence ` => ` (space-arrow-space),
//! it does not collide with `>=` in formulas. Plain `.sheet` files parse fine
//! through the snapshot path — missing tails just mean "no expectation".

use crate::format::{classify_line, Line, ParseError, TextSheet};

/// Render a snapshot: every authored cell, one per line, with its computed
/// value as an ` => ` tail.
pub fn format_snapshot(sheet: &TextSheet) -> String {
    let mut out = String::new();
    for (id, raw) in sheet.cells() {
        let computed = sheet.get(id).to_string();
        out.push_str(id);
        out.push_str(": ");
        out.push_str(raw);
        out.push_str(" => ");
        out.push_str(&computed);
        out.push('\n');
    }
    out
}

/// A single cell where the snapshot's expected value did not match the
/// recomputed value.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotMismatch {
    pub cell: String,
    pub expected: String,
    pub actual: String,
}

impl std::fmt::Display for SnapshotMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: expected {:?}, got {:?}",
            self.cell, self.expected, self.actual
        )
    }
}

/// Either a parse failure or a list of expectation mismatches.
#[derive(Debug, Clone, PartialEq)]
pub enum VerifyError {
    Parse(ParseError),
    Mismatches(Vec<SnapshotMismatch>),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::Parse(e) => write!(f, "{e}"),
            VerifyError::Mismatches(ms) => {
                writeln!(f, "{} mismatch(es):", ms.len())?;
                for m in ms {
                    writeln!(f, "  {m}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for VerifyError {}

/// Evaluate the snapshot's raw values and compare against its ` => ` tails.
///
/// Lines without a tail are skipped (no expectation to check). Returns `Ok`
/// iff every expectation matched.
pub fn verify_snapshot(source: &str) -> Result<(), VerifyError> {
    let sheet = TextSheet::parse(source).map_err(VerifyError::Parse)?;

    let mut mismatches = Vec::new();
    for (i, raw_line) in source.lines().enumerate() {
        let line = classify_line(raw_line, i + 1).map_err(VerifyError::Parse)?;
        if let Line::Cell {
            cell,
            expected: Some(expected),
            ..
        } = line
        {
            let actual = sheet.get(&cell).to_string();
            if actual != expected {
                mismatches.push(SnapshotMismatch {
                    cell,
                    expected,
                    actual,
                });
            }
        }
    }

    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(VerifyError::Mismatches(mismatches))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::parse;

    #[test]
    fn snapshot_round_trips() {
        let src = "A1: =SUM(B1:B3)\nB1: 10\nB2: 20\nB3: 30\n";
        let ts = parse(src).unwrap();
        let snap = format_snapshot(&ts);
        assert_eq!(
            snap,
            "A1: =SUM(B1:B3) => 60\nB1: 10 => 10\nB2: 20 => 20\nB3: 30 => 30\n"
        );
        verify_snapshot(&snap).unwrap();
    }

    #[test]
    fn verify_flags_mismatch() {
        let snap = "A1: =1+1 => 3\n";
        let err = verify_snapshot(snap).unwrap_err();
        match err {
            VerifyError::Mismatches(ms) => {
                assert_eq!(ms.len(), 1);
                assert_eq!(ms[0].cell, "A1");
                assert_eq!(ms[0].expected, "3");
                assert_eq!(ms[0].actual, "2");
            }
            _ => panic!("expected mismatches"),
        }
    }

    #[test]
    fn verify_accepts_missing_tails() {
        // No tail = no expectation; should pass.
        let snap = "A1: =1+1\nB1: 5 => 5\n";
        verify_snapshot(snap).unwrap();
    }

    #[test]
    fn verify_detects_multiple_mismatches() {
        let snap = "A1: =1+1 => 3\nB1: =2*2 => 5\n";
        let err = verify_snapshot(snap).unwrap_err();
        match err {
            VerifyError::Mismatches(ms) => assert_eq!(ms.len(), 2),
            _ => panic!(),
        }
    }

    #[test]
    fn snapshot_captures_computed_errors() {
        // Division by zero lands in the computed value as `#DIV/0!`, so it
        // shows up in a snapshot. (Circular deps are rejected at set_cells
        // time, so those never produce a snapshot — parse itself errors.)
        let ts = parse("A1: =1/0\n").unwrap();
        let snap = format_snapshot(&ts);
        assert!(snap.contains("#DIV/0!"), "snap was: {snap}");
    }

    #[test]
    fn ge_operator_is_not_mistaken_for_tail() {
        // The ` => ` tail requires surrounding spaces, so `>=` alone is safe.
        // (We don't currently support `>=` in the evaluator, but the parser
        // contract is what we're checking here.)
        let src = "A1: =B1>=C1\n";
        let ts = parse(src).unwrap();
        assert_eq!(ts.get_raw("A1"), Some("=B1>=C1"));
    }
}
