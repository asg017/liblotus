//! Formula token stream — a higher-level classifier on top of the lexer.
//!
//! Unlike [`crate::refs::extract_refs`] which filters to reference tokens,
//! [`formula_tokens`] surfaces every token in a formula string with its
//! source span. It exists so downstream consumers (e.g. the frontend's
//! formula-segment builder and in-string caret detection) don't have to
//! roll their own scanners.

use crate::lexer::Lexer;
use crate::types::{Token, TokenType};

/// Classification of a single formula token.
///
/// Ranges (`A1:B2`, `A:C`, `1:5`) are aggregated into a single [`Range`]
/// kind rather than surfaced as separate endpoint+colon tokens.
/// Identifiers are classified as [`Function`] when immediately followed
/// by `(`, else [`Name`] (bare identifier / named-range).
///
/// [`Range`]: TokenKind::Range
/// [`Function`]: TokenKind::Function
/// [`Name`]: TokenKind::Name
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Number,
    String,
    Boolean,
    CellRef,
    Range,
    Name,
    Function,
    Operator,
    Paren,
    Comma,
    Whitespace,
    Unknown,
}

impl TokenKind {
    /// Stable snake_case string used by the pyo3 / wasm bindings.
    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Number => "number",
            TokenKind::String => "string",
            TokenKind::Boolean => "boolean",
            TokenKind::CellRef => "cell_ref",
            TokenKind::Range => "range",
            TokenKind::Name => "name",
            TokenKind::Function => "function",
            TokenKind::Operator => "operator",
            TokenKind::Paren => "paren",
            TokenKind::Comma => "comma",
            TokenKind::Whitespace => "whitespace",
            TokenKind::Unknown => "unknown",
        }
    }
}

/// A single token with source span. Byte offsets are relative to the
/// original formula string including its leading `=`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaToken {
    pub start: u32,
    pub end: u32,
    pub kind: TokenKind,
}

/// Classify every token of a formula string.
///
/// Returns an empty list for non-formula input (no leading `=`). On a
/// lexer error, returns whatever tokens were produced before the error
/// plus an [`Unknown`](TokenKind::Unknown) span covering the remainder
/// of the body.
///
/// Whitespace between tokens is emitted as [`Whitespace`](TokenKind::Whitespace)
/// tokens so callers can reconstruct the full formula span from the token list.
pub fn formula_tokens(formula: &str) -> Vec<FormulaToken> {
    if !formula.starts_with('=') {
        return vec![];
    }

    let (raw, err_pos) = Lexer::new(formula).tokenize_partial();
    let body_len = formula.len() - 1; // lexer strips the leading '='

    // Lexer positions are 0-based within the body; formula coords add +1
    // to account for the stripped leading '='.
    let to_formula = |body_pos: usize| -> u32 { (body_pos + 1) as u32 };

    let mut out: Vec<FormulaToken> = Vec::with_capacity(raw.len());
    let mut cursor: usize = 0;

    let mut i = 0;
    while i < raw.len() {
        let t = &raw[i];

        // Fill any whitespace gap before this token.
        if t.position > cursor {
            out.push(FormulaToken {
                start: to_formula(cursor),
                end: to_formula(t.position),
                kind: TokenKind::Whitespace,
            });
        }

        // Aggregate ranges: CellRef : CellRef, or two endpoint tokens
        // joined by `:` for whole-column (Function : Function with pure
        // letters) and whole-row (Number : Number) shapes.
        if let Some((range_end_pos, advance)) = try_range_span(&raw, i) {
            out.push(FormulaToken {
                start: to_formula(t.position),
                end: to_formula(range_end_pos),
                kind: TokenKind::Range,
            });
            cursor = range_end_pos;
            i += advance;
            continue;
        }

        let kind = match t.token_type {
            TokenType::Number => TokenKind::Number,
            TokenType::String => TokenKind::String,
            TokenType::Boolean => TokenKind::Boolean,
            TokenType::CellRef => TokenKind::CellRef,
            TokenType::Function => {
                if next_nonspace_is_lparen(&raw, i) {
                    TokenKind::Function
                } else {
                    TokenKind::Name
                }
            }
            TokenType::Operator => TokenKind::Operator,
            TokenType::LParen | TokenType::RParen | TokenType::LBrace | TokenType::RBrace => {
                TokenKind::Paren
            }
            TokenType::Comma | TokenType::Semicolon => TokenKind::Comma,
            // `:` and `#` outside of range/spill aggregation — surface as
            // operators so callers still see them as a classified token.
            TokenType::Colon | TokenType::Hash => TokenKind::Operator,
            TokenType::Eof => {
                i += 1;
                continue;
            }
        };

        let end_pos = t.position + t.source_len;
        out.push(FormulaToken {
            start: to_formula(t.position),
            end: to_formula(end_pos),
            kind,
        });
        cursor = end_pos;
        i += 1;
    }

    // Tail: whitespace then Unknown (if the lexer stopped early), or just
    // trailing whitespace otherwise.
    if let Some(err) = err_pos {
        if err > cursor {
            out.push(FormulaToken {
                start: to_formula(cursor),
                end: to_formula(err),
                kind: TokenKind::Whitespace,
            });
        }
        if body_len > err {
            out.push(FormulaToken {
                start: to_formula(err),
                end: to_formula(body_len),
                kind: TokenKind::Unknown,
            });
        }
    } else if body_len > cursor {
        out.push(FormulaToken {
            start: to_formula(cursor),
            end: to_formula(body_len),
            kind: TokenKind::Whitespace,
        });
    }

    out
}

/// If `tokens[i..]` starts a range (`CellRef : CellRef`, pure-letters
/// `Function : Function`, or `Number : Number`), return `(end_byte_pos,
/// tokens_consumed)`. Otherwise `None`.
fn try_range_span(tokens: &[Token], i: usize) -> Option<(usize, usize)> {
    let start = tokens.get(i)?;
    let colon = tokens.get(i + 1)?;
    let end = tokens.get(i + 2)?;
    if colon.token_type != TokenType::Colon {
        return None;
    }
    let matches = match (&start.token_type, &end.token_type) {
        (TokenType::CellRef, TokenType::CellRef) => true,
        (TokenType::Number, TokenType::Number) => true,
        (TokenType::Function, TokenType::Function) => {
            is_pure_letters(&start.value) && is_pure_letters(&end.value)
        }
        _ => false,
    };
    if !matches {
        return None;
    }
    Some((end.position + end.source_len, 3))
}

fn next_nonspace_is_lparen(tokens: &[Token], i: usize) -> bool {
    matches!(
        tokens.get(i + 1),
        Some(t) if t.token_type == TokenType::LParen
    )
}

fn is_pure_letters(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(formula: &str) -> Vec<TokenKind> {
        formula_tokens(formula).into_iter().map(|t| t.kind).collect()
    }

    fn spans(formula: &str) -> Vec<(u32, u32, TokenKind)> {
        formula_tokens(formula)
            .into_iter()
            .map(|t| (t.start, t.end, t.kind))
            .collect()
    }

    #[test]
    fn non_formula_is_empty() {
        assert!(formula_tokens("hello").is_empty());
        assert!(formula_tokens("").is_empty());
        assert!(formula_tokens("42").is_empty());
    }

    #[test]
    fn simple_arithmetic() {
        assert_eq!(
            kinds("=1+2"),
            vec![TokenKind::Number, TokenKind::Operator, TokenKind::Number]
        );
    }

    #[test]
    fn cell_ref_and_range() {
        assert_eq!(
            kinds("=A1+B2:C3"),
            vec![
                TokenKind::CellRef,
                TokenKind::Operator,
                TokenKind::Range,
            ]
        );
    }

    #[test]
    fn whole_col_and_whole_row_ranges() {
        assert_eq!(kinds("=A:C"), vec![TokenKind::Range]);
        assert_eq!(kinds("=1:5"), vec![TokenKind::Range]);
    }

    #[test]
    fn function_vs_name() {
        assert_eq!(
            kinds("=SUM(A1)"),
            vec![
                TokenKind::Function,
                TokenKind::Paren,
                TokenKind::CellRef,
                TokenKind::Paren,
            ]
        );
        // Bare identifier — named range, not a call.
        assert_eq!(kinds("=TaxRate"), vec![TokenKind::Name]);
        assert_eq!(
            kinds("=TaxRate+1"),
            vec![TokenKind::Name, TokenKind::Operator, TokenKind::Number]
        );
    }

    #[test]
    fn string_token() {
        assert_eq!(kinds(r#"="hi""#), vec![TokenKind::String]);
        assert_eq!(
            kinds(r#"="a"&"b""#),
            vec![TokenKind::String, TokenKind::Operator, TokenKind::String]
        );
    }

    #[test]
    fn whitespace_fills_gaps() {
        // =  1 + 2
        // 0  123456
        let s = spans("=  1 + 2");
        assert_eq!(
            s,
            vec![
                (1, 3, TokenKind::Whitespace),
                (3, 4, TokenKind::Number),
                (4, 5, TokenKind::Whitespace),
                (5, 6, TokenKind::Operator),
                (6, 7, TokenKind::Whitespace),
                (7, 8, TokenKind::Number),
            ]
        );
    }

    #[test]
    fn trailing_whitespace() {
        let s = spans("=1  ");
        assert_eq!(
            s,
            vec![(1, 2, TokenKind::Number), (2, 4, TokenKind::Whitespace)]
        );
    }

    #[test]
    fn span_covers_dollar_markers() {
        // `$A$1` is a single CellRef token; its span covers the `$` bytes.
        let s = spans("=$A$1");
        assert_eq!(s, vec![(1, 5, TokenKind::CellRef)]);
    }

    #[test]
    fn unterminated_string_returns_partial_plus_unknown() {
        // `=1+"oops` — lexer stops at the `"` because the string never closes.
        let s = spans(r#"=1+"oops"#);
        assert_eq!(
            s,
            vec![
                (1, 2, TokenKind::Number),
                (2, 3, TokenKind::Operator),
                (3, 8, TokenKind::Unknown),
            ]
        );
    }

    #[test]
    fn unexpected_char_marks_tail_unknown() {
        let s = spans("=A1 @ B2");
        // Before the `@`: CellRef, whitespace. Then Unknown covers `@ B2`.
        assert_eq!(
            s,
            vec![
                (1, 3, TokenKind::CellRef),
                (3, 4, TokenKind::Whitespace),
                (4, 8, TokenKind::Unknown),
            ]
        );
    }

    #[test]
    fn colon_outside_range_is_operator() {
        // `:5` alone — the Number starts it, but no left endpoint, so no
        // range aggregation is possible. The bare colon surfaces as Operator.
        let s = spans("=:5");
        assert_eq!(
            s,
            vec![(1, 2, TokenKind::Operator), (2, 3, TokenKind::Number)]
        );
    }

    #[test]
    fn array_literal_braces_and_semicolons() {
        let k = kinds("={1,2;3,4}");
        assert_eq!(
            k,
            vec![
                TokenKind::Paren,
                TokenKind::Number,
                TokenKind::Comma,
                TokenKind::Number,
                TokenKind::Comma, // `;` surfaces as Comma
                TokenKind::Number,
                TokenKind::Comma,
                TokenKind::Number,
                TokenKind::Paren,
            ]
        );
    }

    #[test]
    fn spill_hash_is_operator() {
        let k = kinds("=A1#");
        assert_eq!(k, vec![TokenKind::CellRef, TokenKind::Operator]);
    }

    #[test]
    fn kind_as_str_is_snake_case() {
        assert_eq!(TokenKind::CellRef.as_str(), "cell_ref");
        assert_eq!(TokenKind::Whitespace.as_str(), "whitespace");
        assert_eq!(TokenKind::Function.as_str(), "function");
    }
}
