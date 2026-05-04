//! ANSI coloring for formulas, `.sheet` lines, and REPL input.
//!
//! Uses `lotus_core::lexer` to tokenize formulas, then re-emits the original
//! text with escape sequences wrapped around each token. If tokenization
//! fails (partial or invalid input — common while typing), the input is
//! returned unchanged rather than partially colored.

use lotus_core::lexer::Lexer;
use lotus_core::types::TokenType;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";

// Token palette (16-color codes for broad terminal support):
const C_CELL: &str = "\x1b[36m"; // cyan
const C_RANGE: &str = "\x1b[1;36m"; // bold cyan — ranges like A1:B3 or A:A
const C_NUMBER: &str = "\x1b[35m"; // magenta
const C_STRING: &str = "\x1b[32m"; // green
const C_FUNCTION: &str = "\x1b[33m"; // yellow
const C_OPERATOR: &str = "\x1b[31m"; // red
const C_EQ: &str = "\x1b[91m"; // bright red (the leading `=`)
const C_PUNCT: &str = "\x1b[2m"; // dim (parens/commas/colons)

// Line-level palette:
const C_CELL_ID: &str = "\x1b[1;36m"; // bold cyan for the LHS cell id
const C_COMMENT: &str = "\x1b[90m"; // bright black
const C_TAIL_ARROW: &str = "\x1b[90m"; // bright black (the ` => ` delimiter)
const C_COMPUTED: &str = "\x1b[2;37m"; // dim white
const C_CMD: &str = "\x1b[1;34m"; // bold blue (REPL colon commands)

/// Colorize a formula string (with or without a leading `=`).
///
/// Returns the input unchanged if lexing fails.
pub fn highlight_formula(input: &str) -> String {
    let (has_eq, body) = match input.strip_prefix('=') {
        Some(rest) => (true, rest),
        None => (false, input),
    };

    let tokens = match Lexer::new(input).tokenize() {
        Ok(t) => t,
        Err(_) => return input.to_string(),
    };

    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    if has_eq {
        out.push_str(C_EQ);
        out.push('=');
        out.push_str(RESET);
    }

    let mut cursor = 0usize;
    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        if matches!(tok.token_type, TokenType::Eof) {
            break;
        }

        // Three-token lookahead: ident `:` ident → single range span. This
        // covers `A1:B3`, `A:A`, `1:5`. Column ranges use Function tokens on
        // either side (bare letters are lexed as functions), so include them.
        if is_range_endpoint(&tok.token_type)
            && tokens
                .get(i + 1)
                .map(|t| t.token_type == TokenType::Colon)
                .unwrap_or(false)
            && tokens
                .get(i + 2)
                .map(|t| is_range_endpoint(&t.token_type))
                .unwrap_or(false)
        {
            let third = &tokens[i + 2];
            let start = tok.position;
            let end = third
                .position
                .saturating_add(span_len(third))
                .min(chars.len());

            if cursor < start {
                out.extend(chars[cursor..start].iter());
            }
            let slice: String = chars[start..end].iter().collect();
            out.push_str(C_RANGE);
            out.push_str(&slice);
            out.push_str(RESET);
            cursor = end;
            i += 3;
            continue;
        }

        let start = tok.position;
        let len = span_len(tok);
        let end = start.saturating_add(len).min(chars.len());

        if cursor < start {
            out.extend(chars[cursor..start].iter());
        }
        let slice: String = chars[start..end].iter().collect();
        let color = color_for(&tok.token_type);
        out.push_str(color);
        out.push_str(&slice);
        out.push_str(RESET);
        cursor = end;
        i += 1;
    }
    if cursor < chars.len() {
        out.extend(chars[cursor..].iter());
    }
    out
}

fn is_range_endpoint(ty: &TokenType) -> bool {
    // `A1:B3` → CellRef on both sides. `A:A` → Function on both sides (bare
    // letters lex as functions). `1:5` → Number on both sides. Anything else
    // isn't a range.
    matches!(
        ty,
        TokenType::CellRef | TokenType::Function | TokenType::Number
    )
}

/// Colorize a single `.sheet` or `.sheet.snap` line.
///
/// Handles comments (`# ...`), cell assignments (`A1: raw`), and the
/// snapshot tail (` => computed`). Blank input passes through untouched.
pub fn highlight_sheet_line(line: &str) -> String {
    if line.trim().is_empty() {
        return line.to_string();
    }
    if line.trim_start().starts_with('#') {
        return format!("{C_COMMENT}{line}{RESET}");
    }

    let Some(colon) = line.find(':') else {
        return line.to_string();
    };
    let (cell, rest) = line.split_at(colon);
    if !is_cell_id(cell) {
        return line.to_string();
    }
    let rest = &rest[1..]; // drop the colon
    let leading_space = rest.starts_with(' ');
    let body_start = if leading_space { 1 } else { 0 };
    let body = &rest[body_start..];

    let (raw, tail) = split_snapshot_tail(body);

    let mut out = String::new();
    out.push_str(C_CELL_ID);
    out.push_str(cell);
    out.push_str(RESET);
    out.push_str(DIM);
    out.push(':');
    out.push_str(RESET);
    if leading_space {
        out.push(' ');
    }

    if raw.starts_with('=') {
        out.push_str(&highlight_formula(raw));
    } else {
        out.push_str(raw);
    }

    if let Some(computed) = tail {
        out.push_str(C_TAIL_ARROW);
        out.push_str(" => ");
        out.push_str(RESET);
        out.push_str(C_COMPUTED);
        out.push_str(computed);
        out.push_str(RESET);
    }

    out
}

/// Colorize a multi-line block of `.sheet` text.
pub fn highlight_sheet(source: &str) -> String {
    let mut out = String::new();
    for (i, line) in source.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&highlight_sheet_line(line));
    }
    out
}

/// Colorize a REPL input line (for use by rustyline's `Highlighter`).
///
/// Recognizes: blank lines, `# comments`, `:commands`, `?queries`, and
/// assignments (`A1 = raw` or `A1: raw`).
pub fn highlight_input(line: &str) -> String {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return line.to_string();
    }
    let lead_ws = &line[..line.len() - trimmed.len()];

    if trimmed.starts_with('#') {
        return format!("{lead_ws}{C_COMMENT}{trimmed}{RESET}");
    }

    if let Some(rest) = trimmed.strip_prefix(':') {
        return format!("{lead_ws}{C_CMD}:{rest}{RESET}");
    }

    if let Some(rest) = trimmed.strip_prefix('?') {
        return format!("{lead_ws}{C_CMD}?{RESET}{C_CELL_ID}{rest}{RESET}");
    }

    // Assignment. Prefer ` = ` (mirrors the REPL parser in `repl.rs`).
    let (cell, sep, raw) = if let Some(idx) = trimmed.find(" = ") {
        (&trimmed[..idx], " = ", &trimmed[idx + 3..])
    } else if let Some(idx) = trimmed.find(':') {
        let rhs = &trimmed[idx + 1..];
        let has_space = rhs.starts_with(' ');
        let sep = if has_space { ": " } else { ":" };
        let raw = if has_space { &rhs[1..] } else { rhs };
        (&trimmed[..idx], sep, raw)
    } else {
        return line.to_string();
    };

    let raw_colored = if raw.starts_with('=') {
        highlight_formula(raw)
    } else {
        raw.to_string()
    };

    format!(
        "{lead_ws}{C_CELL_ID}{cell}{RESET}{DIM}{sep}{RESET}{raw_colored}"
    )
}

fn span_len(tok: &lotus_core::types::Token) -> usize {
    match tok.token_type {
        // String values exclude their quotes; span includes them.
        TokenType::String => tok.value.chars().count() + 2,
        _ => tok.value.chars().count(),
    }
}

fn color_for(ty: &TokenType) -> &'static str {
    match ty {
        TokenType::CellRef => C_CELL,
        TokenType::Number => C_NUMBER,
        TokenType::String => C_STRING,
        // Booleans render with the number palette — they coerce numerically
        // (1/0) and read as "value-like" rather than "name-like".
        TokenType::Boolean => C_NUMBER,
        TokenType::Function => C_FUNCTION,
        TokenType::Operator => C_OPERATOR,
        TokenType::LParen
        | TokenType::RParen
        | TokenType::LBrace
        | TokenType::RBrace
        | TokenType::Comma
        | TokenType::Colon
        | TokenType::Semicolon
        | TokenType::Hash => C_PUNCT,
        TokenType::Eof => "",
    }
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

fn split_snapshot_tail(s: &str) -> (&str, Option<&str>) {
    match s.find(" => ") {
        Some(i) => (&s[..i], Some(&s[i + 4..])),
        None => (s, None),
    }
}

/// Strip ANSI escape sequences from a string. Helper for tests.
#[cfg(test)]
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            // Skip until 'm'
            i += 2;
            while i < bytes.len() && bytes[i] != b'm' {
                i += 1;
            }
            i += 1; // skip 'm'
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula_preserves_text() {
        let input = "=SUM(A1:B3, 10)";
        let out = highlight_formula(input);
        assert_eq!(strip_ansi(&out), input);
    }

    #[test]
    fn formula_colors_tokens() {
        let out = highlight_formula("=SUM(A1)");
        assert!(out.contains(C_FUNCTION));
        assert!(out.contains(C_CELL));
        assert!(out.contains(C_EQ));
    }

    #[test]
    fn formula_highlights_cell_range_as_unit() {
        let out = highlight_formula("=SUM(A1:B3)");
        assert_eq!(strip_ansi(&out), "=SUM(A1:B3)");
        assert!(out.contains(C_RANGE), "range color missing: {out:?}");
        // The range color should cover the whole `A1:B3` substring — no
        // RESET or other color escape inside it.
        let range_start = out.find(C_RANGE).unwrap() + C_RANGE.len();
        let after = &out[range_start..];
        let reset_idx = after.find(RESET).unwrap();
        assert_eq!(&after[..reset_idx], "A1:B3");
    }

    #[test]
    fn formula_highlights_column_range_as_unit() {
        let out = highlight_formula("=SUM(A:A)");
        assert_eq!(strip_ansi(&out), "=SUM(A:A)");
        assert!(out.contains(C_RANGE));
        let range_start = out.find(C_RANGE).unwrap() + C_RANGE.len();
        let after = &out[range_start..];
        let reset_idx = after.find(RESET).unwrap();
        assert_eq!(&after[..reset_idx], "A:A");
    }

    #[test]
    fn formula_highlights_row_range_as_unit() {
        let out = highlight_formula("=SUM(1:5)");
        assert_eq!(strip_ansi(&out), "=SUM(1:5)");
        assert!(out.contains(C_RANGE));
    }

    #[test]
    fn formula_with_string_literal() {
        let input = r#"=CONCAT("hi", A1)"#;
        let out = highlight_formula(input);
        assert_eq!(strip_ansi(&out), input);
        assert!(out.contains(C_STRING));
    }

    #[test]
    fn formula_with_invalid_input_returns_unchanged() {
        // `@` isn't a recognized character — lexer fails, we pass through.
        let input = "=A1@B1";
        let out = highlight_formula(input);
        assert_eq!(out, input);
    }

    #[test]
    fn sheet_line_highlights_cell_and_raw() {
        let out = highlight_sheet_line("A1: =SUM(B1)");
        assert_eq!(strip_ansi(&out), "A1: =SUM(B1)");
        assert!(out.contains(C_CELL_ID));
    }

    #[test]
    fn sheet_line_handles_snapshot_tail() {
        let out = highlight_sheet_line("A1: =1+1 => 2");
        assert_eq!(strip_ansi(&out), "A1: =1+1 => 2");
        assert!(out.contains(C_TAIL_ARROW));
        assert!(out.contains(C_COMPUTED));
    }

    #[test]
    fn sheet_line_passes_comments() {
        let out = highlight_sheet_line("# hello");
        assert_eq!(strip_ansi(&out), "# hello");
        assert!(out.contains(C_COMMENT));
    }

    #[test]
    fn sheet_line_literal_value() {
        let out = highlight_sheet_line("A1: hello");
        assert_eq!(strip_ansi(&out), "A1: hello");
    }

    #[test]
    fn sheet_line_passes_non_cell_prefix() {
        // REPL status messages like "error: ..." or "loaded: ..." must not be
        // mistaken for cell assignments.
        let cases = ["error: bad thing", "loaded 3 cells", "line 4 error: oops"];
        for case in cases {
            assert_eq!(strip_ansi(&highlight_sheet_line(case)), case);
        }
    }

    #[test]
    fn sheet_block_preserves_newlines() {
        let input = "A1: 1\nB1: =A1+1\n";
        let out = highlight_sheet(input);
        // `highlight_sheet` processes each line and rejoins; trailing newline
        // from `lines()` isn't preserved, so compare line counts.
        assert_eq!(strip_ansi(&out).lines().count(), 2);
    }

    #[test]
    fn repl_input_assignment_eq_form() {
        let out = highlight_input("A1 = =SUM(B1:B3)");
        assert_eq!(strip_ansi(&out), "A1 = =SUM(B1:B3)");
        assert!(out.contains(C_CELL_ID));
        assert!(out.contains(C_FUNCTION));
    }

    #[test]
    fn repl_input_colon_command() {
        let out = highlight_input(":show");
        assert_eq!(strip_ansi(&out), ":show");
        assert!(out.contains(C_CMD));
    }

    #[test]
    fn repl_input_query() {
        let out = highlight_input("?A1");
        assert_eq!(strip_ansi(&out), "?A1");
        assert!(out.contains(C_CELL_ID));
    }

    #[test]
    fn repl_input_blank_passes_through() {
        assert_eq!(highlight_input(""), "");
        assert_eq!(highlight_input("   "), "   ");
    }

    #[test]
    fn repl_input_partial_formula_preserves_text() {
        // User is mid-typing a bad character; output still matches input text.
        let input = "A1 = =A1>B1";
        let out = highlight_input(input);
        assert_eq!(strip_ansi(&out), input);
    }
}
