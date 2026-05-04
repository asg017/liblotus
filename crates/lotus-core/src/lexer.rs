use crate::types::{Token, TokenType};

/// Tokenizes a formula string into a sequence of tokens.
pub struct Lexer {
    input: Vec<char>,
    position: usize,
}

impl Lexer {
    #[must_use]
    pub fn new(input: &str) -> Self {
        // Strip leading '=' if present (formula indicator)
        let s = input.strip_prefix('=').unwrap_or(input);
        Lexer {
            input: s.chars().collect(),
            position: 0,
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        self.position = 0;

        while let Some(t) = self.next_token()? {
            tokens.push(t);
        }

        tokens.push(Token {
            token_type: TokenType::Eof,
            value: String::new(),
            position: self.position,
            source_len: 0,
        });

        Ok(tokens)
    }

    /// Like [`tokenize`], but on error returns the tokens produced so far
    /// plus the byte position (within the lexer body) where scanning
    /// stopped. No `Eof` marker is appended. Used by callers that need to
    /// classify partial/invalid formulas — e.g. to colour a half-typed
    /// cell before the closing quote is entered.
    pub fn tokenize_partial(&mut self) -> (Vec<Token>, Option<usize>) {
        let mut tokens = Vec::new();
        self.position = 0;

        loop {
            self.skip_whitespace();
            if self.position >= self.input.len() {
                return (tokens, None);
            }
            let checkpoint = self.position;
            match self.next_token() {
                Ok(Some(t)) => tokens.push(t),
                Ok(None) => return (tokens, None),
                Err(_) => return (tokens, Some(checkpoint)),
            }
        }
    }

    /// Read a single token at the current position, or return `Ok(None)`
    /// if there are no more tokens. Skips leading whitespace.
    fn next_token(&mut self) -> Result<Option<Token>, String> {
        self.skip_whitespace();
        if self.position >= self.input.len() {
            return Ok(None);
        }

        let ch = self.input[self.position];

        // `$` is an absoluteness marker — legal as a prefix on a cell-ref
        // or whole-col/whole-row endpoint. It must be followed by a
        // letter (cell ref / col letters) or a digit (row number).
        if ch == '$' {
            let next = self.peek(1);
            if next.is_some_and(|c| c.is_ascii_alphabetic()) {
                return Ok(Some(self.read_identifier()?));
            }
            if next.is_some_and(|c| c.is_ascii_digit()) {
                return Ok(Some(self.read_number()));
            }
            return Err(format!(
                "Unexpected character: $ at position {}",
                self.position
            ));
        }

        if ch.is_ascii_digit() || (ch == '.' && self.peek(1).is_some_and(|c| c.is_ascii_digit())) {
            return Ok(Some(self.read_number()));
        }
        if ch == '"' || ch == '\'' {
            return Ok(Some(self.read_string(ch)?));
        }
        if ch.is_ascii_alphabetic() {
            return Ok(Some(self.read_identifier()?));
        }
        if ch == '<' || ch == '>' || ch == '=' {
            // Comparison operators, possibly two-char: <=, >=, <>.
            let start = self.position;
            let next = self.peek(1);
            let (value, n) = match (ch, next) {
                ('<', Some('=')) | ('<', Some('>')) | ('>', Some('=')) => {
                    self.position += 2;
                    (format!("{ch}{}", next.unwrap()), 2)
                }
                _ => {
                    self.position += 1;
                    (ch.to_string(), 1)
                }
            };
            return Ok(Some(Token {
                token_type: TokenType::Operator,
                value,
                position: start,
                source_len: n,
            }));
        }
        if is_operator(ch) {
            let t = single_char(TokenType::Operator, ch, self.position);
            self.position += 1;
            return Ok(Some(t));
        }
        let simple = match ch {
            '(' => Some(TokenType::LParen),
            ')' => Some(TokenType::RParen),
            ',' => Some(TokenType::Comma),
            '{' => Some(TokenType::LBrace),
            '}' => Some(TokenType::RBrace),
            ';' => Some(TokenType::Semicolon),
            '#' => Some(TokenType::Hash),
            ':' => Some(TokenType::Colon),
            _ => None,
        };
        if let Some(tt) = simple {
            let t = single_char(tt, ch, self.position);
            self.position += 1;
            return Ok(Some(t));
        }
        Err(format!(
            "Unexpected character: {ch} at position {}",
            self.position
        ))
    }

    fn skip_whitespace(&mut self) {
        while self.position < self.input.len() && self.input[self.position].is_ascii_whitespace() {
            self.position += 1;
        }
    }

    fn peek(&self, offset: usize) -> Option<char> {
        self.input.get(self.position + offset).copied()
    }

    fn read_number(&mut self) -> Token {
        let start = self.position;
        let mut has_decimal = false;

        // Optional leading `$` — acts as an absoluteness marker on a
        // whole-row endpoint (e.g. `$1:$1`). Stripped from `value`.
        if self.input[self.position] == '$' {
            self.position += 1;
        }

        let value_start = self.position;
        while self.position < self.input.len() {
            let ch = self.input[self.position];
            if ch.is_ascii_digit() {
                self.position += 1;
            } else if ch == '.' && !has_decimal {
                has_decimal = true;
                self.position += 1;
            } else {
                break;
            }
        }

        let value: String = self.input[value_start..self.position].iter().collect();
        Token {
            token_type: TokenType::Number,
            value,
            position: start,
            source_len: self.position - start,
        }
    }

    fn read_string(&mut self, quote: char) -> Result<Token, String> {
        let start = self.position;
        self.position += 1; // skip opening quote

        while self.position < self.input.len() && self.input[self.position] != quote {
            self.position += 1;
        }

        if self.position >= self.input.len() {
            return Err("Unterminated string".to_string());
        }

        self.position += 1; // skip closing quote

        let value: String = self.input[start + 1..self.position - 1].iter().collect();
        Ok(Token {
            token_type: TokenType::String,
            value,
            position: start,
            source_len: self.position - start,
        })
    }

    /// Read an identifier / cell ref, honouring `$` markers for absolute
    /// addressing. Recognized shapes:
    ///
    ///   - `$?letters$?digits`   → `CellRef`. `$` is stripped from `value`
    ///     so downstream dep tracking canonicalises absolute and relative
    ///     to the same id.
    ///   - `$?letters`           → `Function` token (whole-col endpoint or
    ///     named-range / function name). `$` allowed only on whole-col
    ///     endpoints, but the parser — not the lexer — enforces usage
    ///     context.
    ///   - `letters[digits]letters...` (mixed) → `Function` token, same as
    ///     before. `$` is rejected in this shape.
    ///
    /// Errors when `$` appears but the resulting token isn't a valid
    /// cell-ref or bare column-letter token.
    fn read_identifier(&mut self) -> Result<Token, String> {
        let start = self.position;
        let dollar_before_letters = self.input[self.position] == '$';
        if dollar_before_letters {
            self.position += 1;
        }

        // Phase 1: letters.
        let letters_start = self.position;
        while self.position < self.input.len() && self.input[self.position].is_ascii_alphabetic() {
            self.position += 1;
        }
        let letters_end = self.position;

        // Phase 2: optional `$` — only valid when followed by digits.
        let mut dollar_before_digits = false;
        if self.position < self.input.len()
            && self.input[self.position] == '$'
            && self.peek(1).is_some_and(|c| c.is_ascii_digit())
        {
            dollar_before_digits = true;
            self.position += 1;
        }

        // Phase 3: digits.
        let digits_start = self.position;
        while self.position < self.input.len() && self.input[self.position].is_ascii_digit() {
            self.position += 1;
        }
        let digits_end = self.position;

        // Phase 4: identifier tail (letters / digits / `_`). If anything
        // falls here it means we've got a non-cell-ref identifier like
        // `my_range` or `A1X` — the whole thing is one Function token.
        let tail_start = self.position;
        while self.position < self.input.len()
            && (self.input[self.position].is_ascii_alphanumeric()
                || self.input[self.position] == '_')
        {
            self.position += 1;
        }
        let has_tail = self.position > tail_start;

        let has_letters = letters_end > letters_start;
        let has_digits = digits_end > digits_start;
        let is_cell_ref = has_letters && has_digits && !has_tail;
        let is_bare_letters = has_letters && !has_digits && !has_tail;

        if (dollar_before_letters || dollar_before_digits) && !is_cell_ref && !is_bare_letters {
            return Err(format!(
                "Unexpected character: $ at position {start}"
            ));
        }
        if !has_letters {
            // We only reach here via the `$`-dispatch path (caller peeked a letter),
            // so this branch is defensive; no letters found means broken input.
            return Err(format!(
                "Unexpected character: $ at position {start}"
            ));
        }

        let value = if is_cell_ref {
            // Canonical form: uppercased letters + digits, without `$`.
            let letters: String = self.input[letters_start..letters_end].iter().collect();
            let digits: String = self.input[digits_start..digits_end].iter().collect();
            format!("{}{digits}", letters.to_uppercase())
        } else {
            // Function / named-range / bare col letters / boolean literal.
            let raw: String = self.input[letters_start..self.position].iter().collect();
            raw.to_uppercase()
        };

        // Excel-style boolean literal. Only the bare-letters shape (no `$`,
        // no digits, no tail) is eligible — `$TRUE`, `TRUE1`, and `TRUE_X`
        // stay as cell-ref / function tokens.
        let token_type = if is_cell_ref {
            TokenType::CellRef
        } else if is_bare_letters
            && !dollar_before_letters
            && (value == "TRUE" || value == "FALSE")
        {
            TokenType::Boolean
        } else {
            TokenType::Function
        };

        Ok(Token {
            token_type,
            value,
            position: start,
            source_len: self.position - start,
        })
    }
}

fn single_char(token_type: TokenType, ch: char, position: usize) -> Token {
    Token {
        token_type,
        value: ch.to_string(),
        position,
        source_len: 1,
    }
}

fn is_operator(ch: char) -> bool {
    matches!(ch, '+' | '-' | '*' | '/' | '^' | '%' | '&')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok_types(input: &str) -> Vec<TokenType> {
        Lexer::new(input)
            .tokenize()
            .unwrap()
            .into_iter()
            .map(|t| t.token_type)
            .collect()
    }

    fn tok_values(input: &str) -> Vec<String> {
        Lexer::new(input)
            .tokenize()
            .unwrap()
            .into_iter()
            .map(|t| t.value)
            .collect()
    }

    #[test]
    fn strips_leading_equals() {
        let values = tok_values("=42");
        assert_eq!(values, vec!["42", ""]);
    }

    #[test]
    fn number_tokens() {
        assert_eq!(
            tok_types("123"),
            vec![TokenType::Number, TokenType::Eof]
        );
        assert_eq!(
            tok_types("3.14"),
            vec![TokenType::Number, TokenType::Eof]
        );
        assert_eq!(tok_values(".5"), vec![".5", ""]);
    }

    #[test]
    fn string_tokens() {
        assert_eq!(tok_values(r#""hello""#), vec!["hello", ""]);
        assert_eq!(tok_values("'world'"), vec!["world", ""]);
    }

    #[test]
    fn cell_ref_and_function() {
        assert_eq!(
            tok_types("A1"),
            vec![TokenType::CellRef, TokenType::Eof]
        );
        assert_eq!(
            tok_types("AA10"),
            vec![TokenType::CellRef, TokenType::Eof]
        );
        assert_eq!(
            tok_types("SUM"),
            vec![TokenType::Function, TokenType::Eof]
        );
        // cell refs are uppercased
        assert_eq!(tok_values("a1"), vec!["A1", ""]);
    }

    #[test]
    fn operators_and_punctuation() {
        let types = tok_types("=A1 + B2 * 3");
        assert_eq!(
            types,
            vec![
                TokenType::CellRef,
                TokenType::Operator,
                TokenType::CellRef,
                TokenType::Operator,
                TokenType::Number,
                TokenType::Eof,
            ]
        );
    }

    #[test]
    fn full_formula() {
        let types = tok_types("=SUM(A1:B2, 5)");
        assert_eq!(
            types,
            vec![
                TokenType::Function,
                TokenType::LParen,
                TokenType::CellRef,
                TokenType::Colon,
                TokenType::CellRef,
                TokenType::Comma,
                TokenType::Number,
                TokenType::RParen,
                TokenType::Eof,
            ]
        );
    }

    #[test]
    fn unexpected_char_error() {
        let result = Lexer::new("=A1 @ B2").tokenize();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unexpected character"));
    }

    #[test]
    fn single_char_comparison_operators() {
        for op in ["=", "<", ">"] {
            let input = format!("=A1{op}B1");
            let toks = Lexer::new(&input).tokenize().unwrap();
            let found = toks.iter().find(|t| t.value == op).unwrap();
            assert_eq!(found.token_type, TokenType::Operator);
        }
    }

    #[test]
    fn two_char_comparison_operators() {
        for op in ["<=", ">=", "<>"] {
            let input = format!("=A1{op}B1");
            let toks = Lexer::new(&input).tokenize().unwrap();
            let found = toks.iter().find(|t| t.value == op).unwrap();
            assert_eq!(found.token_type, TokenType::Operator);
            assert_eq!(found.value, op);
        }
    }

    #[test]
    fn comparison_op_position_tracking() {
        // <= at position 2 (after the `=` prefix which gets stripped).
        let toks = Lexer::new("=A1<=B2").tokenize().unwrap();
        let op_tok = toks.iter().find(|t| t.value == "<=").unwrap();
        // Lexer strips the `=`, so its positions are 0-based within the body.
        assert_eq!(op_tok.position, 2);
    }

    #[test]
    fn ampersand_tokenized_as_operator() {
        let toks = Lexer::new(r#"="a" & B2"#).tokenize().unwrap();
        let amp = toks.iter().find(|t| t.value == "&").unwrap();
        assert_eq!(amp.token_type, TokenType::Operator);
    }

    #[test]
    fn unterminated_string_error() {
        let result = Lexer::new(r#"="hello"#).tokenize();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unterminated string"));
    }

    // ── `$` absoluteness markers ────────────────────────────────────

    fn cell_ref_token(input: &str) -> (String, usize) {
        let toks = Lexer::new(input).tokenize().unwrap();
        let t = toks
            .iter()
            .find(|t| t.token_type == TokenType::CellRef)
            .unwrap();
        (t.value.clone(), t.source_len)
    }

    #[test]
    fn dollar_full_absolute_cell_ref() {
        let (value, source_len) = cell_ref_token("=$A$1");
        assert_eq!(value, "A1");
        assert_eq!(source_len, 4);
    }

    #[test]
    fn dollar_col_absolute_cell_ref() {
        let (value, source_len) = cell_ref_token("=$A1");
        assert_eq!(value, "A1");
        assert_eq!(source_len, 3);
    }

    #[test]
    fn dollar_row_absolute_cell_ref() {
        let (value, source_len) = cell_ref_token("=A$1");
        assert_eq!(value, "A1");
        assert_eq!(source_len, 3);
    }

    #[test]
    fn dollar_whole_col_endpoint_token() {
        let toks = Lexer::new("=$A:$A").tokenize().unwrap();
        // Expect: Function("A", src=2), Colon, Function("A", src=2), EOF.
        assert_eq!(toks[0].token_type, TokenType::Function);
        assert_eq!(toks[0].value, "A");
        assert_eq!(toks[0].source_len, 2);
        assert_eq!(toks[1].token_type, TokenType::Colon);
        assert_eq!(toks[2].token_type, TokenType::Function);
        assert_eq!(toks[2].source_len, 2);
    }

    #[test]
    fn dollar_whole_row_endpoint_token() {
        let toks = Lexer::new("=$1:$5").tokenize().unwrap();
        assert_eq!(toks[0].token_type, TokenType::Number);
        assert_eq!(toks[0].value, "1");
        assert_eq!(toks[0].source_len, 2);
        assert_eq!(toks[2].token_type, TokenType::Number);
        assert_eq!(toks[2].value, "5");
        assert_eq!(toks[2].source_len, 2);
    }

    #[test]
    fn dollar_followed_by_invalid_is_error() {
        // `$` with no ref-forming continuation.
        assert!(Lexer::new("=$").tokenize().is_err());
        assert!(Lexer::new("=$ ").tokenize().is_err());
        assert!(Lexer::new("=$+1").tokenize().is_err());
        // `$` on a token that continues as an identifier — not a valid cell ref.
        assert!(Lexer::new("=$A1X").tokenize().is_err());
    }

    #[test]
    fn dollar_is_not_tokenized_inside_string_literals() {
        let toks = Lexer::new(r#"="$A$1""#).tokenize().unwrap();
        assert_eq!(toks[0].token_type, TokenType::String);
        assert_eq!(toks[0].value, "$A$1");
    }

    // ── boolean literals ────────────────────────────────────────────

    #[test]
    fn boolean_literal_uppercase() {
        let toks = Lexer::new("=TRUE").tokenize().unwrap();
        assert_eq!(toks[0].token_type, TokenType::Boolean);
        assert_eq!(toks[0].value, "TRUE");
        let toks = Lexer::new("=FALSE").tokenize().unwrap();
        assert_eq!(toks[0].token_type, TokenType::Boolean);
        assert_eq!(toks[0].value, "FALSE");
    }

    #[test]
    fn boolean_literal_case_insensitive() {
        for s in ["true", "True", "tRuE"] {
            let input = format!("={s}");
            let toks = Lexer::new(&input).tokenize().unwrap();
            assert_eq!(toks[0].token_type, TokenType::Boolean, "{s}");
            assert_eq!(toks[0].value, "TRUE");
        }
        for s in ["false", "False", "fAlSe"] {
            let input = format!("={s}");
            let toks = Lexer::new(&input).tokenize().unwrap();
            assert_eq!(toks[0].token_type, TokenType::Boolean, "{s}");
            assert_eq!(toks[0].value, "FALSE");
        }
    }

    #[test]
    fn boolean_only_for_bare_letters() {
        // A `$` prefix forces the cell-ref / whole-col path — not a boolean.
        // (`$TRUE` is currently a function-shaped token; it should not flip
        // to Boolean.)
        let toks = Lexer::new("=$TRUE").tokenize().unwrap();
        assert_ne!(toks[0].token_type, TokenType::Boolean);
        // Trailing identifier characters keep it as a function-shaped token.
        let toks = Lexer::new("=TRUE1").tokenize().unwrap();
        assert_ne!(toks[0].token_type, TokenType::Boolean);
        let toks = Lexer::new("=TRUEX").tokenize().unwrap();
        assert_ne!(toks[0].token_type, TokenType::Boolean);
    }
}
