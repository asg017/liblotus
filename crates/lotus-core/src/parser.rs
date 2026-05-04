use crate::lexer::Lexer;
use crate::types::{AstNode, CompareOp, Token, TokenType};

/// Recursive-descent parser that builds an AST from a formula string.
///
/// Operator precedence (lowest → highest), matching Excel/Sheets:
///   1. Comparison:      = <> < <= > >=
///   2. Concatenation:   &
///   3. Additive:        + -
///   4. Multiplicative:  * / %
///   5. Power:           ^
///   6. Unary:           + -
///   7. Primary:         number, string, cell ref, range, function call, parens, array literal
pub struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    pub fn parse(input: &str) -> Result<AstNode, String> {
        let tokens = Lexer::new(input).tokenize()?;
        let mut parser = Parser {
            tokens,
            position: 0,
        };
        let result = parser.parse_expression()?;

        if parser.current().token_type != TokenType::Eof {
            return Err(format!(
                "Unexpected token: {}",
                parser.current().value
            ));
        }

        Ok(result)
    }

    fn current(&self) -> &Token {
        static EOF: Token = Token {
            token_type: TokenType::Eof,
            value: String::new(),
            position: 0,
            source_len: 0,
        };
        self.tokens.get(self.position).unwrap_or(&EOF)
    }

    fn consume(&mut self, expected: Option<TokenType>) -> Result<Token, String> {
        let token = self.current().clone();
        if let Some(exp) = expected {
            if token.token_type != exp {
                return Err(format!(
                    "Expected {exp:?} but got {:?}",
                    token.token_type
                ));
            }
        }
        self.position += 1;
        Ok(token)
    }

    // ── precedence levels ───────────────────────────────────────────────

    fn parse_expression(&mut self) -> Result<AstNode, String> {
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<AstNode, String> {
        let mut left = self.parse_concat()?;

        while self.current().token_type == TokenType::Operator {
            let op = match self.current().value.as_str() {
                "=" => CompareOp::Eq,
                "<>" => CompareOp::Ne,
                "<" => CompareOp::Lt,
                "<=" => CompareOp::Le,
                ">" => CompareOp::Gt,
                ">=" => CompareOp::Ge,
                _ => break,
            };
            self.consume(None)?;
            let right = self.parse_concat()?;
            left = AstNode::Compare {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_concat(&mut self) -> Result<AstNode, String> {
        let mut left = self.parse_additive()?;

        while self.current().token_type == TokenType::Operator
            && self.current().value == "&"
        {
            let op = self.consume(None)?.value.chars().next().unwrap();
            let right = self.parse_additive()?;
            left = AstNode::BinaryOp {
                operator: op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<AstNode, String> {
        let mut left = self.parse_multiplicative()?;

        while self.current().token_type == TokenType::Operator
            && (self.current().value == "+" || self.current().value == "-")
        {
            let op = self.consume(None)?.value.chars().next().unwrap();
            let right = self.parse_multiplicative()?;
            left = AstNode::BinaryOp {
                operator: op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<AstNode, String> {
        let mut left = self.parse_power()?;

        while self.current().token_type == TokenType::Operator
            && (self.current().value == "*"
                || self.current().value == "/"
                || self.current().value == "%")
        {
            let op = self.consume(None)?.value.chars().next().unwrap();
            let right = self.parse_power()?;
            left = AstNode::BinaryOp {
                operator: op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_power(&mut self) -> Result<AstNode, String> {
        let mut left = self.parse_unary()?;

        while self.current().token_type == TokenType::Operator && self.current().value == "^" {
            let op = self.consume(None)?.value.chars().next().unwrap();
            let right = self.parse_unary()?;
            left = AstNode::BinaryOp {
                operator: op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<AstNode, String> {
        if self.current().token_type == TokenType::Operator
            && (self.current().value == "+" || self.current().value == "-")
        {
            let op = self.consume(None)?.value.chars().next().unwrap();
            let operand = self.parse_unary()?;
            return Ok(AstNode::UnaryOp {
                operator: op,
                operand: Box::new(operand),
            });
        }

        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<AstNode, String> {
        let token = self.current().clone();

        match token.token_type {
            TokenType::Number => {
                self.consume(None)?;

                // Check for row range like 1:5
                if self.current().token_type == TokenType::Colon
                    && self.tokens.get(self.position + 1)
                        .is_some_and(|t| t.token_type == TokenType::Number)
                {
                    let start_row: u32 = token.value.parse()
                        .map_err(|_| format!("Invalid row number: {}", token.value))?;
                    self.consume(None)?; // consume ':'
                    let end_token = self.consume(Some(TokenType::Number))?;
                    let end_row: u32 = end_token.value.parse()
                        .map_err(|_| format!("Invalid row number: {}", end_token.value))?;
                    return Ok(AstNode::RowRange { start_row, end_row });
                }

                let value: f64 = token
                    .value
                    .parse()
                    .map_err(|_| format!("Invalid number: {}", token.value))?;
                Ok(AstNode::Number(value))
            }

            TokenType::String => {
                self.consume(None)?;
                Ok(AstNode::String(token.value.clone()))
            }

            TokenType::Boolean => {
                self.consume(None)?;
                // Lexer canonicalises to upper-case "TRUE" / "FALSE" — anything
                // else here would be a bug in the lexer.
                Ok(AstNode::Boolean(token.value == "TRUE"))
            }

            TokenType::CellRef => {
                self.consume(None)?;
                let (col, row) = parse_cell_ref(&token.value)?;

                // Check for spill operator A1#
                if self.current().token_type == TokenType::Hash {
                    self.consume(None)?;
                    return Ok(AstNode::SpillRef { col, row });
                }

                // Check for range A1:B2
                if self.current().token_type == TokenType::Colon {
                    self.consume(None)?; // consume ':'
                    let end_token = self.consume(Some(TokenType::CellRef))?;
                    let (end_col, end_row) = parse_cell_ref(&end_token.value)?;
                    Ok(AstNode::Range {
                        start_col: col,
                        start_row: row,
                        end_col,
                        end_row,
                    })
                } else {
                    Ok(AstNode::CellRef { col, row })
                }
            }

            TokenType::Function => {
                let func_name = self.consume(None)?.value;

                // Check for column range like A:C — Function followed by : and Function
                if self.current().token_type == TokenType::Colon
                    && self.tokens.get(self.position + 1)
                        .is_some_and(|t| t.token_type == TokenType::Function)
                {
                    // Validate both sides are pure letters (column names, not function names)
                    if func_name.chars().all(|c| c.is_ascii_alphabetic()) {
                        self.consume(None)?; // consume ':'
                        let end_token = self.consume(Some(TokenType::Function))?;
                        if end_token.value.chars().all(|c| c.is_ascii_alphabetic()) {
                            return Ok(AstNode::ColumnRange {
                                start_col: func_name,
                                end_col: end_token.value,
                            });
                        }
                        return Err(format!("Invalid column range end: {}", end_token.value));
                    }
                }

                // Bare identifier (no `(` follows) → named-range reference.
                if self.current().token_type != TokenType::LParen {
                    return Ok(AstNode::Name(func_name));
                }
                self.consume(Some(TokenType::LParen))?;

                let mut args = Vec::new();
                if self.current().token_type != TokenType::RParen {
                    args.push(self.parse_expression()?);
                    while self.current().token_type == TokenType::Comma {
                        self.consume(None)?;
                        args.push(self.parse_expression()?);
                    }
                }

                self.consume(Some(TokenType::RParen))?;

                Ok(AstNode::FunctionCall {
                    name: func_name,
                    args,
                })
            }

            TokenType::LParen => {
                self.consume(None)?;
                let expr = self.parse_expression()?;
                self.consume(Some(TokenType::RParen))?;
                Ok(expr)
            }

            TokenType::LBrace => self.parse_array_literal(),

            _ => Err(format!(
                "Unexpected token: {:?} ({})",
                token.token_type, token.value
            )),
        }
    }

    /// Parse `{row (; row)*}` where each row is `expr (, expr)*`.
    /// All rows must have equal width.
    fn parse_array_literal(&mut self) -> Result<AstNode, String> {
        self.consume(Some(TokenType::LBrace))?;

        let mut rows: Vec<Vec<AstNode>> = Vec::new();
        // Reject empty `{}`.
        if self.current().token_type == TokenType::RBrace {
            return Err("Array literal must contain at least one element".into());
        }

        loop {
            let mut row = Vec::new();
            row.push(self.parse_expression()?);
            while self.current().token_type == TokenType::Comma {
                self.consume(None)?;
                row.push(self.parse_expression()?);
            }
            rows.push(row);

            match self.current().token_type {
                TokenType::Semicolon => {
                    self.consume(None)?;
                    continue;
                }
                TokenType::RBrace => break,
                _ => {
                    return Err(format!(
                        "Expected `,`, `;`, or `}}` in array literal; got {}",
                        self.current().value
                    ));
                }
            }
        }
        self.consume(Some(TokenType::RBrace))?;

        let width = rows[0].len();
        for (i, r) in rows.iter().enumerate() {
            if r.len() != width {
                return Err(format!(
                    "Array literal rows have inconsistent widths (row {} has {}, row 0 has {})",
                    i, r.len(), width
                ));
            }
        }
        Ok(AstNode::ArrayLiteral { rows })
    }
}

/// Parse "A1" → ("A", 1), "AA10" → ("AA", 10)
fn parse_cell_ref(value: &str) -> Result<(String, u32), String> {
    let col_end = value
        .find(|c: char| c.is_ascii_digit())
        .ok_or_else(|| format!("Invalid cell reference: {value}"))?;
    let col = &value[..col_end];
    let row: u32 = value[col_end..]
        .parse()
        .map_err(|_| format!("Invalid cell reference: {value}"))?;
    Ok((col.to_string(), row))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::approx_constant)]
    fn parse_number() {
        assert_eq!(Parser::parse("=42").unwrap(), AstNode::Number(42.0));
        assert_eq!(Parser::parse("=3.14").unwrap(), AstNode::Number(3.14));
    }

    #[test]
    fn parse_string() {
        assert_eq!(
            Parser::parse(r#"="hello""#).unwrap(),
            AstNode::String("hello".into())
        );
    }

    #[test]
    fn parse_cell_ref_simple() {
        assert_eq!(
            Parser::parse("=A1").unwrap(),
            AstNode::CellRef {
                col: "A".into(),
                row: 1
            }
        );
        assert_eq!(
            Parser::parse("=AA10").unwrap(),
            AstNode::CellRef {
                col: "AA".into(),
                row: 10
            }
        );
    }

    #[test]
    fn parse_range() {
        assert_eq!(
            Parser::parse("=A1:B3").unwrap(),
            AstNode::Range {
                start_col: "A".into(),
                start_row: 1,
                end_col: "B".into(),
                end_row: 3,
            }
        );
    }

    #[test]
    fn parse_binary_ops_precedence() {
        // 1 + 2 * 3  →  1 + (2 * 3)
        let ast = Parser::parse("=1 + 2 * 3").unwrap();
        match ast {
            AstNode::BinaryOp {
                operator: '+',
                left,
                right,
            } => {
                assert_eq!(*left, AstNode::Number(1.0));
                match *right {
                    AstNode::BinaryOp {
                        operator: '*',
                        left: l,
                        right: r,
                    } => {
                        assert_eq!(*l, AstNode::Number(2.0));
                        assert_eq!(*r, AstNode::Number(3.0));
                    }
                    _ => panic!("Expected BinaryOp *"),
                }
            }
            _ => panic!("Expected BinaryOp +"),
        }
    }

    #[test]
    fn parse_unary_negation() {
        let ast = Parser::parse("=-5").unwrap();
        match ast {
            AstNode::UnaryOp {
                operator: '-',
                operand,
            } => {
                assert_eq!(*operand, AstNode::Number(5.0));
            }
            _ => panic!("Expected UnaryOp"),
        }
    }

    #[test]
    fn parse_function_call() {
        let ast = Parser::parse("=SUM(A1, 5)").unwrap();
        match ast {
            AstNode::FunctionCall { name, args } => {
                assert_eq!(name, "SUM");
                assert_eq!(args.len(), 2);
                assert_eq!(
                    args[0],
                    AstNode::CellRef {
                        col: "A".into(),
                        row: 1
                    }
                );
                assert_eq!(args[1], AstNode::Number(5.0));
            }
            _ => panic!("Expected FunctionCall"),
        }
    }

    #[test]
    fn parse_function_with_range() {
        let ast = Parser::parse("=SUM(A1:B2)").unwrap();
        match ast {
            AstNode::FunctionCall { name, args } => {
                assert_eq!(name, "SUM");
                assert_eq!(args.len(), 1);
                assert_eq!(
                    args[0],
                    AstNode::Range {
                        start_col: "A".into(),
                        start_row: 1,
                        end_col: "B".into(),
                        end_row: 2,
                    }
                );
            }
            _ => panic!("Expected FunctionCall"),
        }
    }

    #[test]
    fn parse_parenthesized() {
        // (1 + 2) * 3
        let ast = Parser::parse("=(1 + 2) * 3").unwrap();
        match ast {
            AstNode::BinaryOp {
                operator: '*',
                left,
                right,
            } => {
                match *left {
                    AstNode::BinaryOp {
                        operator: '+', ..
                    } => {}
                    _ => panic!("Expected BinaryOp +"),
                }
                assert_eq!(*right, AstNode::Number(3.0));
            }
            _ => panic!("Expected BinaryOp *"),
        }
    }

    #[test]
    fn parse_nested_functions() {
        let ast = Parser::parse("=SUM(A1, MAX(B1, B2))").unwrap();
        match ast {
            AstNode::FunctionCall { name, args } => {
                assert_eq!(name, "SUM");
                assert_eq!(args.len(), 2);
                match &args[1] {
                    AstNode::FunctionCall { name, .. } => assert_eq!(name, "MAX"),
                    _ => panic!("Expected nested FunctionCall"),
                }
            }
            _ => panic!("Expected FunctionCall"),
        }
    }

    #[test]
    fn parse_column_range() {
        assert_eq!(
            Parser::parse("=A:A").unwrap(),
            AstNode::ColumnRange {
                start_col: "A".into(),
                end_col: "A".into(),
            }
        );
        assert_eq!(
            Parser::parse("=A:C").unwrap(),
            AstNode::ColumnRange {
                start_col: "A".into(),
                end_col: "C".into(),
            }
        );
    }

    #[test]
    fn parse_row_range() {
        assert_eq!(
            Parser::parse("=1:1").unwrap(),
            AstNode::RowRange {
                start_row: 1,
                end_row: 1,
            }
        );
        assert_eq!(
            Parser::parse("=1:5").unwrap(),
            AstNode::RowRange {
                start_row: 1,
                end_row: 5,
            }
        );
    }

    #[test]
    fn parse_sum_column_range() {
        let ast = Parser::parse("=SUM(A:A)").unwrap();
        match ast {
            AstNode::FunctionCall { name, args } => {
                assert_eq!(name, "SUM");
                assert_eq!(args.len(), 1);
                assert_eq!(
                    args[0],
                    AstNode::ColumnRange {
                        start_col: "A".into(),
                        end_col: "A".into(),
                    }
                );
            }
            _ => panic!("Expected FunctionCall"),
        }
    }

    #[test]
    fn parse_error_unexpected_token() {
        assert!(Parser::parse("=1 + + +").is_err());
    }

    #[test]
    fn parse_empty_function_args() {
        // e.g. =NOW() — zero args
        let ast = Parser::parse("=NOW()").unwrap();
        match ast {
            AstNode::FunctionCall { name, args } => {
                assert_eq!(name, "NOW");
                assert!(args.is_empty());
            }
            _ => panic!("Expected FunctionCall"),
        }
    }

    #[test]
    fn parse_name_bare_identifier() {
        assert_eq!(Parser::parse("=TaxRate").unwrap(), AstNode::Name("TAXRATE".into()));
    }

    #[test]
    fn parse_name_in_expression() {
        // TaxRate * 100  →  Name("TAXRATE") * 100
        let ast = Parser::parse("=TaxRate * 100").unwrap();
        match ast {
            AstNode::BinaryOp { operator: '*', left, right } => {
                assert_eq!(*left, AstNode::Name("TAXRATE".into()));
                assert_eq!(*right, AstNode::Number(100.0));
            }
            _ => panic!("Expected BinaryOp *"),
        }
    }

    #[test]
    fn parse_name_as_function_arg() {
        let ast = Parser::parse("=SUM(Revenue, A1)").unwrap();
        match ast {
            AstNode::FunctionCall { name, args } => {
                assert_eq!(name, "SUM");
                assert_eq!(args[0], AstNode::Name("REVENUE".into()));
                assert_eq!(args[1], AstNode::CellRef { col: "A".into(), row: 1 });
            }
            _ => panic!("Expected FunctionCall"),
        }
    }

    #[test]
    fn parse_function_name_without_parens_is_name() {
        // `=SUM` (no parens) parses as a name reference, not a call.
        // Evaluator will fail to resolve it (returns #NAME?), matching Excel.
        assert_eq!(Parser::parse("=SUM").unwrap(), AstNode::Name("SUM".into()));
    }

    #[test]
    fn parse_function_call_still_works() {
        // Regression: function-call path unaffected by the new bare-name path.
        let ast = Parser::parse("=SUM(A1)").unwrap();
        assert!(matches!(ast, AstNode::FunctionCall { .. }));
    }

    #[test]
    fn parse_column_range_still_works() {
        // Regression: A:C still parses as ColumnRange, not Name(A) : Name(C).
        assert_eq!(
            Parser::parse("=A:C").unwrap(),
            AstNode::ColumnRange { start_col: "A".into(), end_col: "C".into() }
        );
    }

    #[test]
    fn parse_string_concat() {
        // ="a" & "b"  →  & with two String children
        let ast = Parser::parse(r#"="a" & "b""#).unwrap();
        match ast {
            AstNode::BinaryOp { operator: '&', left, right } => {
                assert_eq!(*left, AstNode::String("a".into()));
                assert_eq!(*right, AstNode::String("b".into()));
            }
            _ => panic!("expected BinaryOp &, got {ast:?}"),
        }
    }

    #[test]
    fn parse_concat_lower_precedence_than_addition() {
        // ="a" & 1 + 2  →  "a" & (1 + 2)  (Excel: & is lower than +/-)
        let ast = Parser::parse(r#"="a" & 1 + 2"#).unwrap();
        match ast {
            AstNode::BinaryOp { operator: '&', left, right } => {
                assert_eq!(*left, AstNode::String("a".into()));
                match *right {
                    AstNode::BinaryOp { operator: '+', .. } => {}
                    other => panic!("expected RHS to be +, got {other:?}"),
                }
            }
            _ => panic!("expected top-level BinaryOp &, got {ast:?}"),
        }
    }

    #[test]
    fn parse_array_literal_single_row() {
        let ast = Parser::parse("={1, 2, 3}").unwrap();
        match ast {
            AstNode::ArrayLiteral { rows } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].len(), 3);
                assert_eq!(rows[0][0], AstNode::Number(1.0));
                assert_eq!(rows[0][2], AstNode::Number(3.0));
            }
            _ => panic!("expected ArrayLiteral, got {ast:?}"),
        }
    }

    #[test]
    fn parse_array_literal_2d() {
        let ast = Parser::parse("={1, 2; 3, 4}").unwrap();
        match ast {
            AstNode::ArrayLiteral { rows } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
                assert_eq!(rows[1].len(), 2);
            }
            _ => panic!("expected ArrayLiteral"),
        }
    }

    #[test]
    fn parse_array_literal_with_expressions() {
        let ast = Parser::parse("={1+1, 2*3}").unwrap();
        match ast {
            AstNode::ArrayLiteral { rows } => {
                assert!(matches!(rows[0][0], AstNode::BinaryOp { operator: '+', .. }));
                assert!(matches!(rows[0][1], AstNode::BinaryOp { operator: '*', .. }));
            }
            _ => panic!("expected ArrayLiteral"),
        }
    }

    #[test]
    fn parse_array_literal_inconsistent_row_width_errors() {
        let err = Parser::parse("={1, 2; 3, 4, 5}").unwrap_err();
        assert!(err.contains("inconsistent widths"), "got: {err}");
    }

    #[test]
    fn parse_array_literal_empty_errors() {
        let err = Parser::parse("={}").unwrap_err();
        assert!(err.contains("at least one element"), "got: {err}");
    }

    #[test]
    fn parse_comparison_eq() {
        let ast = Parser::parse("=A1 = B1").unwrap();
        match ast {
            AstNode::Compare { op: CompareOp::Eq, left, right } => {
                assert_eq!(*left, AstNode::CellRef { col: "A".into(), row: 1 });
                assert_eq!(*right, AstNode::CellRef { col: "B".into(), row: 1 });
            }
            _ => panic!("expected Compare Eq, got {ast:?}"),
        }
    }

    #[test]
    fn parse_comparison_all_operators() {
        let ops = [
            ("=", CompareOp::Eq),
            ("<>", CompareOp::Ne),
            ("<", CompareOp::Lt),
            ("<=", CompareOp::Le),
            (">", CompareOp::Gt),
            (">=", CompareOp::Ge),
        ];
        for (text, expected) in ops {
            let input = format!("=1 {text} 2");
            let ast = Parser::parse(&input).unwrap();
            match ast {
                AstNode::Compare { op, .. } => assert_eq!(op, expected, "op {text}"),
                _ => panic!("expected Compare for {text}"),
            }
        }
    }

    #[test]
    fn parse_comparison_lowest_precedence() {
        // 1 + 2 = 3  →  (1 + 2) = 3  (comparison is lower than +)
        let ast = Parser::parse("=1 + 2 = 3").unwrap();
        match ast {
            AstNode::Compare { op: CompareOp::Eq, left, right } => {
                assert!(matches!(*left, AstNode::BinaryOp { operator: '+', .. }));
                assert_eq!(*right, AstNode::Number(3.0));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_comparison_below_concat() {
        // "a" & "b" = "ab"  →  ("a" & "b") = "ab"  (& is higher than =)
        let ast = Parser::parse(r#"="a" & "b" = "ab""#).unwrap();
        match ast {
            AstNode::Compare { op: CompareOp::Eq, left, .. } => {
                assert!(matches!(*left, AstNode::BinaryOp { operator: '&', .. }));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_spill_ref() {
        assert_eq!(
            Parser::parse("=A1#").unwrap(),
            AstNode::SpillRef { col: "A".into(), row: 1 }
        );
    }

    #[test]
    fn parse_spill_ref_in_expression() {
        let ast = Parser::parse("=SUM(B2#)").unwrap();
        match ast {
            AstNode::FunctionCall { name, args } => {
                assert_eq!(name, "SUM");
                assert_eq!(args[0], AstNode::SpillRef { col: "B".into(), row: 2 });
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_spill_takes_precedence_over_range() {
        // A1# stops at the hash — next token is not in range grammar
        let ast = Parser::parse("=A1# + 1").unwrap();
        match ast {
            AstNode::BinaryOp { operator: '+', left, .. } => {
                assert_eq!(*left, AstNode::SpillRef { col: "A".into(), row: 1 });
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_array_literal_with_strings() {
        let ast = Parser::parse(r#"={"a", "b"; "c", "d"}"#).unwrap();
        match ast {
            AstNode::ArrayLiteral { rows } => {
                assert_eq!(rows[0][0], AstNode::String("a".into()));
                assert_eq!(rows[1][1], AstNode::String("d".into()));
            }
            _ => panic!(),
        }
    }

    // ── `$` absoluteness passes through parser unchanged ────────────

    #[test]
    fn parse_absolute_cell_ref_strips_dollar_in_ast() {
        // AST carries canonical col/row — evaluator doesn't care about `$`.
        assert_eq!(
            Parser::parse("=$A$1").unwrap(),
            AstNode::CellRef { col: "A".into(), row: 1 }
        );
        assert_eq!(
            Parser::parse("=$A1").unwrap(),
            AstNode::CellRef { col: "A".into(), row: 1 }
        );
        assert_eq!(
            Parser::parse("=A$1").unwrap(),
            AstNode::CellRef { col: "A".into(), row: 1 }
        );
    }

    #[test]
    fn parse_absolute_range() {
        assert_eq!(
            Parser::parse("=$A$1:$B$2").unwrap(),
            AstNode::Range {
                start_col: "A".into(),
                start_row: 1,
                end_col: "B".into(),
                end_row: 2,
            }
        );
    }

    #[test]
    fn parse_absolute_whole_col_range() {
        assert_eq!(
            Parser::parse("=$A:$A").unwrap(),
            AstNode::ColumnRange { start_col: "A".into(), end_col: "A".into() }
        );
    }

    #[test]
    fn parse_absolute_whole_row_range() {
        assert_eq!(
            Parser::parse("=$1:$5").unwrap(),
            AstNode::RowRange { start_row: 1, end_row: 5 }
        );
    }

    #[test]
    fn parse_absolute_spill_ref() {
        assert_eq!(
            Parser::parse("=$A$1#").unwrap(),
            AstNode::SpillRef { col: "A".into(), row: 1 }
        );
    }

    #[test]
    fn parse_boolean_literals() {
        assert_eq!(Parser::parse("=TRUE").unwrap(), AstNode::Boolean(true));
        assert_eq!(Parser::parse("=FALSE").unwrap(), AstNode::Boolean(false));
        // Case-insensitive at the lexer.
        assert_eq!(Parser::parse("=true").unwrap(), AstNode::Boolean(true));
        assert_eq!(Parser::parse("=False").unwrap(), AstNode::Boolean(false));
    }

    #[test]
    fn parse_boolean_in_expression() {
        // =IF(TRUE, 1, 0) — Boolean as a function arg.
        let ast = Parser::parse("=IF(TRUE, 1, 0)").unwrap();
        match ast {
            AstNode::FunctionCall { name, args } => {
                assert_eq!(name, "IF");
                assert_eq!(args[0], AstNode::Boolean(true));
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn parse_concat_left_associative() {
        // ="a" & "b" & "c"  →  ("a" & "b") & "c"
        let ast = Parser::parse(r#"="a" & "b" & "c""#).unwrap();
        match ast {
            AstNode::BinaryOp { operator: '&', left, right } => {
                match *left {
                    AstNode::BinaryOp { operator: '&', .. } => {}
                    other => panic!("expected LHS to be &, got {other:?}"),
                }
                assert_eq!(*right, AstNode::String("c".into()));
            }
            _ => panic!("expected top-level BinaryOp &"),
        }
    }
}
