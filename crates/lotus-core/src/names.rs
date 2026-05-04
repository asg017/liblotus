//! Resolve [`AstNode::Name`] references by substituting their definitions.
//!
//! Run as an AST→AST pass between parsing and evaluation. After resolution
//! the AST is free of `Name` nodes, so dependency collection and evaluation
//! both operate on a fully-expanded tree without any per-node name handling.

use std::collections::HashSet;

use crate::parser::Parser;
use crate::types::AstNode;

const NAME_ERROR: &str = "#NAME?";
const CIRCULAR_ERROR: &str = "#CIRCULAR!";

/// Substitute every [`AstNode::Name`] with the AST of its definition.
///
/// `resolver` returns the raw definition string for a name (uppercased).
/// Returns `Err("#NAME?")` if a name is unknown, `Err("#CIRCULAR!")` if
/// names recursively reference each other.
pub fn resolve_names<F>(ast: AstNode, resolver: &F) -> Result<AstNode, String>
where
    F: Fn(&str) -> Option<String>,
{
    let mut visiting = HashSet::new();
    resolve_inner(ast, resolver, &mut visiting)
}

fn resolve_inner<F>(
    ast: AstNode,
    resolver: &F,
    visiting: &mut HashSet<String>,
) -> Result<AstNode, String>
where
    F: Fn(&str) -> Option<String>,
{
    match ast {
        AstNode::Name(name) => {
            let def = resolver(&name).ok_or_else(|| NAME_ERROR.to_string())?;
            if !visiting.insert(name.clone()) {
                return Err(CIRCULAR_ERROR.to_string());
            }
            let resolved = parse_definition(&def)?;
            let result = resolve_inner(resolved, resolver, visiting)?;
            visiting.remove(&name);
            Ok(result)
        }
        AstNode::BinaryOp { operator, left, right } => Ok(AstNode::BinaryOp {
            operator,
            left: Box::new(resolve_inner(*left, resolver, visiting)?),
            right: Box::new(resolve_inner(*right, resolver, visiting)?),
        }),
        AstNode::Compare { op, left, right } => Ok(AstNode::Compare {
            op,
            left: Box::new(resolve_inner(*left, resolver, visiting)?),
            right: Box::new(resolve_inner(*right, resolver, visiting)?),
        }),
        AstNode::UnaryOp { operator, operand } => Ok(AstNode::UnaryOp {
            operator,
            operand: Box::new(resolve_inner(*operand, resolver, visiting)?),
        }),
        AstNode::FunctionCall { name, args } => {
            let mut resolved_args = Vec::with_capacity(args.len());
            for arg in args {
                resolved_args.push(resolve_inner(arg, resolver, visiting)?);
            }
            Ok(AstNode::FunctionCall { name, args: resolved_args })
        }
        AstNode::ArrayLiteral { rows } => {
            let mut resolved_rows = Vec::with_capacity(rows.len());
            for row in rows {
                let mut resolved_row = Vec::with_capacity(row.len());
                for cell in row {
                    resolved_row.push(resolve_inner(cell, resolver, visiting)?);
                }
                resolved_rows.push(resolved_row);
            }
            Ok(AstNode::ArrayLiteral { rows: resolved_rows })
        }
        leaf @ (AstNode::Number(_)
        | AstNode::String(_)
        | AstNode::Boolean(_)
        | AstNode::CellRef { .. }
        | AstNode::Range { .. }
        | AstNode::ColumnRange { .. }
        | AstNode::RowRange { .. }
        | AstNode::SpillRef { .. }) => Ok(leaf),
    }
}

/// Parse a name definition. Formula definitions (`=…`) go through the
/// parser; literal numbers and strings become `Number` / `String` nodes
/// directly so a definition like `"0.05"` doesn't need a leading `=`.
fn parse_definition(def: &str) -> Result<AstNode, String> {
    if def.starts_with('=') {
        return Parser::parse(def);
    }
    if let Ok(n) = def.parse::<f64>() {
        if n.is_finite() {
            return Ok(AstNode::Number(n));
        }
    }
    if def.is_empty() {
        return Ok(AstNode::String(String::new()));
    }
    Ok(AstNode::String(def.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resolver(map: HashMap<&str, &str>) -> impl Fn(&str) -> Option<String> {
        let owned: HashMap<String, String> = map
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |n: &str| owned.get(n).cloned()
    }

    #[test]
    fn resolves_simple_name_to_cell_ref() {
        let r = resolver([("TAXRATE", "=B1")].into());
        let ast = Parser::parse("=TaxRate").unwrap();
        let out = resolve_names(ast, &r).unwrap();
        assert_eq!(out, AstNode::CellRef { col: "B".into(), row: 1 });
    }

    #[test]
    fn resolves_name_to_constant_number() {
        let r = resolver([("RATE", "0.05")].into());
        let ast = Parser::parse("=RATE * 100").unwrap();
        let out = resolve_names(ast, &r).unwrap();
        match out {
            AstNode::BinaryOp { operator: '*', left, right } => {
                assert_eq!(*left, AstNode::Number(0.05));
                assert_eq!(*right, AstNode::Number(100.0));
            }
            _ => panic!("Expected BinaryOp *"),
        }
    }

    #[test]
    fn resolves_nested_name() {
        // OUTER → =INNER + 1, INNER → 5  ⇒  expands fully.
        let r = resolver([("OUTER", "=INNER + 1"), ("INNER", "5")].into());
        let ast = Parser::parse("=OUTER").unwrap();
        let out = resolve_names(ast, &r).unwrap();
        match out {
            AstNode::BinaryOp { operator: '+', left, right } => {
                assert_eq!(*left, AstNode::Number(5.0));
                assert_eq!(*right, AstNode::Number(1.0));
            }
            _ => panic!("Expected BinaryOp +"),
        }
    }

    #[test]
    fn unknown_name_errors() {
        let r = resolver(HashMap::new());
        let ast = Parser::parse("=Missing").unwrap();
        assert_eq!(resolve_names(ast, &r).unwrap_err(), "#NAME?");
    }

    #[test]
    fn circular_name_errors() {
        // A → =B, B → =A
        let r = resolver([("A", "=B"), ("B", "=A")].into());
        let ast = Parser::parse("=A").unwrap();
        assert_eq!(resolve_names(ast, &r).unwrap_err(), "#CIRCULAR!");
    }

    #[test]
    fn self_referential_name_errors() {
        let r = resolver([("X", "=X + 1")].into());
        let ast = Parser::parse("=X").unwrap();
        assert_eq!(resolve_names(ast, &r).unwrap_err(), "#CIRCULAR!");
    }

    #[test]
    fn passthrough_when_no_names() {
        let r = resolver(HashMap::new());
        let ast = Parser::parse("=SUM(A1:B2) + 1").unwrap();
        let out = resolve_names(ast.clone(), &r).unwrap();
        assert_eq!(out, ast);
    }

    #[test]
    fn name_inside_function_call() {
        let r = resolver([("REV", "=A1:A3")].into());
        let ast = Parser::parse("=SUM(REV)").unwrap();
        let out = resolve_names(ast, &r).unwrap();
        match out {
            AstNode::FunctionCall { name, args } => {
                assert_eq!(name, "SUM");
                assert_eq!(
                    args[0],
                    AstNode::Range {
                        start_col: "A".into(),
                        start_row: 1,
                        end_col: "A".into(),
                        end_row: 3,
                    }
                );
            }
            _ => panic!("Expected FunctionCall"),
        }
    }

    #[test]
    fn sibling_uses_of_same_name_dont_falsely_trigger_cycle() {
        // Resolving a name in arg 0 must not block resolving the same name in arg 1.
        let r = resolver([("X", "1")].into());
        let ast = Parser::parse("=SUM(X, X)").unwrap();
        let out = resolve_names(ast, &r).unwrap();
        match out {
            AstNode::FunctionCall { args, .. } => {
                assert_eq!(args[0], AstNode::Number(1.0));
                assert_eq!(args[1], AstNode::Number(1.0));
            }
            _ => panic!("Expected FunctionCall"),
        }
    }
}
