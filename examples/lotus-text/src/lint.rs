//! Diagnostics for a `.sheet` source: parse errors, unknown functions,
//! dangling references, evaluation errors, circular dependencies.

use lotus_core::lexer::Lexer;
use lotus_core::types::TokenType;
use lotus_core::{extract_refs, format_cell_id, CellValue};

use crate::format::{classify_line, Line, TextSheet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Error => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// 1-based source line, or 0 if the diagnostic is sheet-wide.
    pub line: usize,
    pub cell: Option<String>,
    pub severity: Severity,
    pub message: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(f, "{}: {}", self.severity, self.message)
        } else if let Some(cell) = &self.cell {
            write!(f, "line {} [{cell}] {}: {}", self.line, self.severity, self.message)
        } else {
            write!(f, "line {} {}: {}", self.line, self.severity, self.message)
        }
    }
}

/// Lint a `.sheet` source and return every diagnostic we can produce.
pub fn lint(source: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // Pass 1: syntactic — per line.
    let mut cell_lines: Vec<(usize, String, String)> = Vec::new();
    for (i, raw_line) in source.lines().enumerate() {
        let line_no = i + 1;
        match classify_line(raw_line, line_no) {
            Ok(Line::Cell { cell, raw, .. }) => {
                cell_lines.push((line_no, cell, raw));
            }
            Ok(Line::Blank) | Ok(Line::Comment) => {}
            Err(e) => diags.push(Diagnostic {
                line: e.line,
                cell: None,
                severity: Severity::Error,
                message: e.message,
            }),
        }
    }

    // Pass 2: unknown functions in each formula.
    for (line_no, cell, raw) in &cell_lines {
        if !raw.starts_with('=') {
            continue;
        }
        for name in function_names(raw) {
            if !is_known_function(&name) {
                diags.push(Diagnostic {
                    line: *line_no,
                    cell: Some(cell.clone()),
                    severity: Severity::Error,
                    message: format!("unknown function: {name}"),
                });
            }
        }
    }

    // Pass 3: evaluation errors. Only run if pass 1 found no syntactic
    // problems — otherwise the engine would re-report the same failure.
    let had_parse_errors = diags.iter().any(|d| d.severity == Severity::Error);
    if had_parse_errors {
        return diags;
    }

    match TextSheet::parse(source) {
        Err(e) => diags.push(Diagnostic {
            line: e.line,
            cell: None,
            severity: Severity::Error,
            message: e.message,
        }),
        Ok(sheet) => {
            // Dangling refs: any formula reference to a cell not authored.
            for (line_no, cell, raw) in &cell_lines {
                if !raw.starts_with('=') {
                    continue;
                }
                for r in extract_refs(raw) {
                    for coord in &r.cells {
                        let target = format_cell_id(*coord);
                        if sheet.get_raw(&target).is_none() {
                            diags.push(Diagnostic {
                                line: *line_no,
                                cell: Some(cell.clone()),
                                severity: Severity::Warning,
                                message: format!(
                                    "reference to empty cell: {target}"
                                ),
                            });
                        }
                    }
                }
            }

            // Computed errors: anything that landed as a CellValue::Error.
            for (line_no, cell, _) in &cell_lines {
                if let CellValue::Error(e) = sheet.get(cell) {
                    diags.push(Diagnostic {
                        line: *line_no,
                        cell: Some(cell.clone()),
                        severity: Severity::Error,
                        message: format!("evaluation error: {e}"),
                    });
                }
            }
        }
    }

    diags
}

/// Lint an already-loaded sheet for computed errors only. Useful for the REPL
/// where we've been mutating in memory and don't have source text.
pub fn lint_sheet(sheet: &TextSheet) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for (id, raw) in sheet.cells() {
        if raw.starts_with('=') {
            for name in function_names(raw) {
                if !is_known_function(&name) {
                    diags.push(Diagnostic {
                        line: 0,
                        cell: Some(id.to_string()),
                        severity: Severity::Error,
                        message: format!("unknown function: {name}"),
                    });
                }
            }
            for r in extract_refs(raw) {
                for coord in &r.cells {
                    let target = format_cell_id(*coord);
                    if sheet.get_raw(&target).is_none() {
                        diags.push(Diagnostic {
                            line: 0,
                            cell: Some(id.to_string()),
                            severity: Severity::Warning,
                            message: format!("reference to empty cell: {target}"),
                        });
                    }
                }
            }
        }
        if let CellValue::Error(e) = sheet.get(id) {
            diags.push(Diagnostic {
                line: 0,
                cell: Some(id.to_string()),
                severity: Severity::Error,
                message: format!("evaluation error: {e}"),
            });
        }
    }
    diags
}

/// Collect every `Function` token name in a formula.
///
/// Silently returns an empty list if tokenization fails — the evaluation pass
/// will surface the parse error elsewhere.
fn function_names(formula: &str) -> Vec<String> {
    let mut lexer = Lexer::new(formula);
    let tokens = match lexer.tokenize() {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    tokens
        .into_iter()
        .filter(|t| t.token_type == TokenType::Function)
        .map(|t| t.value)
        .collect()
}

/// Probe the engine: a name is known iff calling it with zero args does not
/// yield `FormulaError::Name`. This keeps us in sync with the engine's
/// actual function table without duplicating it here.
fn is_known_function(name: &str) -> bool {
    let reg = lotus_core::Registry::default();
    !matches!(
        lotus_core::functions::call_function(name, &[], &reg),
        Err(lotus_core::FormulaError::Name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(src: &str) -> Vec<Diagnostic> {
        lint(src)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect()
    }

    #[test]
    fn clean_source_has_no_errors() {
        assert!(errors("A1: =SUM(B1:B3)\nB1: 1\nB2: 2\nB3: 3\n").is_empty());
    }

    #[test]
    fn flags_unknown_function() {
        let d = errors("A1: =FOOBAR(1)\n");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("FOOBAR"));
        assert_eq!(d[0].line, 1);
        assert_eq!(d[0].cell.as_deref(), Some("A1"));
    }

    #[test]
    fn flags_parse_error() {
        let d = errors("not a cell line\n");
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn flags_circular_dep() {
        // Engine returns #CIRCULAR! as a computed value; we flag that.
        let d = errors("A1: =B1\nB1: =A1\n");
        assert!(d.iter().any(|d| d.message.contains("#CIRCULAR!")));
    }

    #[test]
    fn flags_div_by_zero() {
        let d = errors("A1: =1/0\n");
        assert!(d.iter().any(|d| d.message.contains("#DIV/0!")));
    }

    #[test]
    fn warns_on_dangling_ref() {
        let diags = lint("A1: =B1+1\n");
        let warns: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Warning).collect();
        assert_eq!(warns.len(), 1);
        assert!(warns[0].message.contains("B1"));
    }

    #[test]
    fn no_warning_when_ref_exists() {
        let diags = lint("A1: =B1+1\nB1: 10\n");
        assert!(diags.iter().all(|d| d.severity != Severity::Warning));
    }

    #[test]
    fn known_functions_are_not_flagged() {
        // Exercise a function that returns Err on empty args (ABS returns Empty,
        // but CONCAT returns Ok). Confirms the probe handles both paths.
        let diags = lint("A1: =SUM(1,2)\nB1: =ABS(-3)\nC1: =CONCAT(\"x\",\"y\")\n");
        assert!(diags.iter().all(|d| d.severity != Severity::Error));
    }
}
