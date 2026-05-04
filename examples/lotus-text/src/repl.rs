//! Line-oriented REPL core. The binary in `src/bin/lotus-repl.rs` is a thin
//! stdin/stdout wrapper; all behavior lives here so it can be unit-tested.

use std::fs;

use lotus_core::parser::Parser;
use lotus_core::types::AstNode;
use lotus_core::{extract_refs, format_cell_id};

use crate::format::TextSheet;
use crate::lint;
use crate::snapshot;

/// Result of handling one line of input.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub output: String,
    pub should_quit: bool,
}

impl Response {
    fn text(s: impl Into<String>) -> Self {
        Response {
            output: s.into(),
            should_quit: false,
        }
    }
    fn quit() -> Self {
        Response {
            output: String::new(),
            should_quit: true,
        }
    }
    fn empty() -> Self {
        Response::text("")
    }
}

pub struct Session {
    sheet: TextSheet,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Session {
            sheet: TextSheet::new(),
        }
    }

    pub fn sheet(&self) -> &TextSheet {
        &self.sheet
    }

    /// Replace the session's sheet with one parsed from `source`.
    pub fn load_source(&mut self, source: &str) -> Result<usize, crate::format::ParseError> {
        let new = TextSheet::parse(source)?;
        let n = new.len();
        self.sheet = new;
        Ok(n)
    }

    /// Dispatch a single input line. The binary calls this for every line
    /// read from stdin.
    pub fn handle(&mut self, input: &str) -> Response {
        let trimmed = input.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return Response::empty();
        }

        if let Some(rest) = trimmed.strip_prefix('?') {
            return self.cmd_query(rest.trim());
        }

        if let Some(rest) = trimmed.strip_prefix(':') {
            return self.cmd_colon(rest.trim());
        }

        // Bare cell id (`A1`) → treat as a query shortcut.
        if is_cell_id(trimmed) {
            return self.cmd_query(trimmed);
        }

        // Assignment: `CELL = raw` or `CELL: raw`. The RHS is auto-wrapped as
        // a formula if it parses as one (so `B1 = SUM(A:A)` and
        // `B1 = sum(A:A)` both do what the user meant).
        match parse_assignment(trimmed) {
            Some((cell, raw)) => {
                let raw = auto_wrap_formula(&raw);
                match self.sheet.set(&cell, &raw) {
                    Ok(()) => {
                        let val = self.sheet.get(&cell);
                        Response::text(format!("{cell} = {val}"))
                    }
                    Err(e) => Response::text(format!("error: {e}")),
                }
            }
            None => Response::text(format!(
                "error: expected `CELL = value`, `?CELL`, or `:command` (got {trimmed:?})"
            )),
        }
    }

    fn cmd_query(&self, cell: &str) -> Response {
        if cell.is_empty() {
            return Response::text("error: `?` needs a cell id, e.g. `?A1`");
        }
        match self.sheet.get_raw(cell) {
            Some(raw) => {
                let computed = self.sheet.get(cell);
                Response::text(format!("{cell}: {raw}  =>  {computed}"))
            }
            None => Response::text(format!("{cell}: (empty)")),
        }
    }

    fn cmd_colon(&mut self, rest: &str) -> Response {
        let (name, arg) = split_once_ws(rest);
        match name {
            "q" | "quit" | "exit" => Response::quit(),
            "help" | "h" | "?" => Response::text(HELP.trim_end()),
            "show" => Response::text(self.sheet.format_compact().trim_end()),
            "snap" => {
                let snap = snapshot::format_snapshot(&self.sheet);
                Response::text(snap.trim_end())
            }
            "lint" => {
                let diags = lint::lint_sheet(&self.sheet);
                if diags.is_empty() {
                    Response::text("lint: clean")
                } else {
                    let mut out = String::new();
                    for d in diags {
                        out.push_str(&format!("{d}\n"));
                    }
                    Response::text(out.trim_end())
                }
            }
            "clear" => {
                self.sheet = TextSheet::new();
                Response::text("cleared")
            }
            "load" => match arg {
                Some(path) => match fs::read_to_string(path) {
                    Ok(src) => match TextSheet::parse(&src) {
                        Ok(new) => {
                            let n = new.len();
                            self.sheet = new;
                            Response::text(format!("loaded {n} cell(s) from {path}"))
                        }
                        Err(e) => Response::text(format!("error: {e}")),
                    },
                    Err(e) => Response::text(format!("error: {e}")),
                },
                None => Response::text("usage: :load <path>"),
            },
            "save" => match arg {
                Some(path) => {
                    let out = self.sheet.format_compact();
                    match fs::write(path, out) {
                        Ok(()) => Response::text(format!("saved to {path}")),
                        Err(e) => Response::text(format!("error: {e}")),
                    }
                }
                None => Response::text("usage: :save <path>"),
            },
            "why" => match arg {
                Some(cell) => Response::text(explain_cell(&self.sheet, cell)),
                None => Response::text("usage: :why <cell>"),
            },
            other => Response::text(format!("unknown command: :{other}  (try :help)")),
        }
    }
}

fn parse_assignment(line: &str) -> Option<(String, String)> {
    // Try each delimiter; take the first one whose LHS is a valid cell id.
    // This lets `B1: = SUM(A:A)` split at the first `:` rather than eating
    // through to the ` = ` inside the formula.
    let candidates: &[(&str, usize)] = &[(" = ", 3), (":", 1)];
    for &(delim, delim_len) in candidates {
        if let Some(idx) = line.find(delim) {
            let lhs = line[..idx].trim();
            if !is_cell_id(lhs) {
                continue;
            }
            let rhs = &line[idx + delim_len..];
            let rhs = rhs.strip_prefix(' ').unwrap_or(rhs);
            let rhs = rhs.trim_end();
            return Some((lhs.to_string(), rhs.to_string()));
        }
    }
    None
}

/// If `raw` isn't already a formula but parses as one when prefixed with `=`,
/// add the prefix. Plain numbers and quoted strings are left alone so
/// `A1 = 42` and `A1 = "hi"` still store literals.
fn auto_wrap_formula(raw: &str) -> String {
    if raw.starts_with('=') || raw.is_empty() {
        return raw.to_string();
    }
    let probe = format!("={raw}");
    match Parser::parse(&probe) {
        Ok(ast) if is_formula_ast(&ast) => probe,
        _ => raw.to_string(),
    }
}

fn is_formula_ast(ast: &AstNode) -> bool {
    // Plain literals and bare identifiers are not formulas — they stay
    // as raw strings. A bare Name (e.g. "hello") would otherwise auto-
    // wrap into "=hello" and try to resolve as a named range.
    !matches!(
        ast,
        AstNode::Number(_) | AstNode::String(_) | AstNode::Name(_)
    )
}

fn is_cell_id(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let digit_start = match s.find(|c: char| c.is_ascii_digit()) {
        Some(i) if i > 0 => i,
        _ => return false,
    };
    s[..digit_start].bytes().all(|b| b.is_ascii_uppercase())
        && s[digit_start..].bytes().all(|b| b.is_ascii_digit())
}

fn split_once_ws(s: &str) -> (&str, Option<&str>) {
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], Some(s[i..].trim())),
        None => (s, None),
    }
}

fn explain_cell(sheet: &TextSheet, cell: &str) -> String {
    let raw = match sheet.get_raw(cell) {
        Some(r) => r,
        None => return format!("{cell}: (empty)"),
    };
    let computed = sheet.get(cell);
    let mut out = format!("{cell}: {raw}\n  computed: {computed}");
    if raw.starts_with('=') {
        let refs = extract_refs(raw);
        if refs.is_empty() {
            out.push_str("\n  refs: (none)");
        } else {
            out.push_str("\n  refs:");
            for r in refs {
                let kind = if r.cells.len() == 1 {
                    "cell".to_string()
                } else {
                    format!("range of {}", r.cells.len())
                };
                out.push_str(&format!("\n    {} ({kind})", r.text));
                for coord in r.cells.iter().take(8) {
                    let id = format_cell_id(*coord);
                    let val = sheet.get(&id);
                    out.push_str(&format!("\n      {id} = {val}"));
                }
                if r.cells.len() > 8 {
                    out.push_str(&format!("\n      … {} more", r.cells.len() - 8));
                }
            }
        }
    }
    out
}

const HELP: &str = "\
commands:
  A1 = <value>       set cell (also: `A1: <value>`)
  ?A1                print raw and computed value
  :show              dump sheet in .sheet format
  :snap              dump .sheet.snap (raw => computed)
  :lint              run diagnostics on current sheet
  :why A1            show refs and their values for A1
  :load <path>       replace sheet with contents of file
  :save <path>       write sheet to file in .sheet format
  :clear             start a new empty sheet
  :help              this list
  :quit              exit (also :q, :exit, ctrl-D)
";

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &[&str]) -> (Session, Vec<String>) {
        let mut s = Session::new();
        let out = lines.iter().map(|l| s.handle(l).output).collect();
        (s, out)
    }

    #[test]
    fn assign_and_query() {
        let (_s, out) = run(&["A1 = 10", "?A1"]);
        assert_eq!(out[0], "A1 = 10");
        assert!(out[1].contains("10"));
    }

    #[test]
    fn formula_recomputes() {
        let (_s, out) = run(&[
            "A1 = 10",
            "B1 = =A1*2",
            "?B1",
        ]);
        assert!(out[2].contains("20"));
    }

    #[test]
    fn colon_assignment_preserves_colon_in_range() {
        let mut s = Session::new();
        s.handle("B1 = 1");
        s.handle("B2 = 2");
        s.handle("B3 = 3");
        // Use `=` form to avoid the colon-splitting ambiguity.
        let r = s.handle("A1 = =SUM(B1:B3)");
        assert!(r.output.contains("6"));
    }

    #[test]
    fn show_outputs_compact() {
        let mut s = Session::new();
        s.handle("A1 = 1");
        s.handle("B1 = 2");
        let r = s.handle(":show");
        assert_eq!(r.output, "A1: 1\nB1: 2");
    }

    #[test]
    fn snap_outputs_tails() {
        let mut s = Session::new();
        s.handle("A1 = 10");
        s.handle("B1 = =A1+5");
        let r = s.handle(":snap");
        assert!(r.output.contains("B1: =A1+5 => 15"));
    }

    #[test]
    fn lint_reports_unknown_function() {
        let mut s = Session::new();
        s.handle("A1 = =NOPE(1)");
        let r = s.handle(":lint");
        assert!(r.output.contains("NOPE"));
    }

    #[test]
    fn why_shows_refs() {
        let mut s = Session::new();
        s.handle("A1 = 5");
        s.handle("B1 = 7");
        s.handle("C1 = =A1+B1");
        let r = s.handle(":why C1");
        assert!(r.output.contains("A1"));
        assert!(r.output.contains("B1"));
        assert!(r.output.contains("12"));
    }

    #[test]
    fn clear_removes_cells() {
        let mut s = Session::new();
        s.handle("A1 = 1");
        s.handle(":clear");
        assert_eq!(s.sheet().len(), 0);
    }

    #[test]
    fn quit_signal() {
        let mut s = Session::new();
        assert!(s.handle(":quit").should_quit);
        assert!(s.handle(":q").should_quit);
        assert!(s.handle(":exit").should_quit);
    }

    #[test]
    fn blank_and_comment_lines_are_noops() {
        let mut s = Session::new();
        assert_eq!(s.handle("").output, "");
        assert_eq!(s.handle("   ").output, "");
        assert_eq!(s.handle("# hello").output, "");
    }

    #[test]
    fn unknown_command_is_reported() {
        let mut s = Session::new();
        let r = s.handle(":frobnicate");
        assert!(r.output.contains("unknown command"));
    }

    #[test]
    fn bare_cell_id_is_a_query() {
        let mut s = Session::new();
        s.handle("A1 = 42");
        let r = s.handle("A1");
        assert!(r.output.contains("42"));
        assert!(!r.output.contains("error"));
    }

    #[test]
    fn auto_wraps_uppercase_function() {
        let mut s = Session::new();
        s.handle("A1 = 1");
        s.handle("A2 = 2");
        s.handle("A3 = 3");
        let r = s.handle("B1 = SUM(A:A)");
        assert!(r.output.contains("6"), "got: {}", r.output);
        assert_eq!(s.sheet().get_raw("B1"), Some("=SUM(A:A)"));
    }

    #[test]
    fn auto_wraps_lowercase_function() {
        let mut s = Session::new();
        s.handle("A1 = 10");
        s.handle("A2 = 20");
        let r = s.handle("B1 = sum(A1:A2)");
        assert!(r.output.contains("30"), "got: {}", r.output);
    }

    #[test]
    fn auto_wraps_arithmetic() {
        let mut s = Session::new();
        s.handle("A1 = 10");
        let r = s.handle("B1 = A1*3");
        assert!(r.output.contains("30"));
        assert_eq!(s.sheet().get_raw("B1"), Some("=A1*3"));
    }

    #[test]
    fn does_not_wrap_plain_number() {
        let mut s = Session::new();
        s.handle("A1 = 42");
        assert_eq!(s.sheet().get_raw("A1"), Some("42"));
    }

    #[test]
    fn does_not_wrap_plain_string() {
        let mut s = Session::new();
        s.handle("A1 = hello");
        assert_eq!(s.sheet().get_raw("A1"), Some("hello"));
        // Two-word strings shouldn't wrap either.
        s.handle("A2 = hello world");
        assert_eq!(s.sheet().get_raw("A2"), Some("hello world"));
    }

    #[test]
    fn confused_colon_form_still_works() {
        // The original bug: `B1: = SUM(A:A)` split at the wrong delimiter.
        let mut s = Session::new();
        s.handle("A1 = 5");
        s.handle("A2 = 10");
        let r = s.handle("B1: = SUM(A1:A2)");
        assert!(!r.output.contains("error"), "got: {}", r.output);
        assert!(r.output.contains("15"));
    }

    #[test]
    fn explicit_equals_prefix_is_preserved() {
        // `A1 = =SUM(...)` should still work — the user-written `=` wins.
        let mut s = Session::new();
        s.handle("B1 = 7");
        let r = s.handle("A1 = =B1+1");
        assert!(r.output.contains("8"));
        assert_eq!(s.sheet().get_raw("A1"), Some("=B1+1"));
    }

    #[test]
    fn load_and_save_round_trip() {
        let tmp = std::env::temp_dir().join("lotus-text-repl-test.sheet");
        std::fs::write(&tmp, "A1: 1\nB1: =A1+1\n").unwrap();
        let mut s = Session::new();
        let r = s.handle(&format!(":load {}", tmp.display()));
        assert!(r.output.contains("loaded 2"));
        assert_eq!(s.sheet().get("B1"), lotus_core::CellValue::Number(2.0));

        let out_path = std::env::temp_dir().join("lotus-text-repl-test.out.sheet");
        s.handle(&format!(":save {}", out_path.display()));
        let content = std::fs::read_to_string(&out_path).unwrap();
        assert_eq!(content, "A1: 1\nB1: =A1+1\n");
    }
}
