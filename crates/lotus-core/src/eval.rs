use std::sync::Arc;

use crate::custom::Registry;
use crate::functions::call_function;
use crate::parser::Parser;
use crate::error::FormulaError;
use crate::types::{
    cell_id, col_to_index, index_to_col, ArrayValue, AstNode, BinaryOp, CellValue, CompareOp,
    MAX_RANGE_CELLS,
};

/// Resolves a spill-anchor cell id (e.g. "A1") to the array it spilled.
/// The lifetime lets the resolver borrow from outside the `Evaluator` —
/// `Sheet`'s recalc loop uses this to hand the evaluator a reference to
/// its own `spill_arrays` map without cloning it per cell.
type SpillResolver<'a> = Box<dyn Fn(&str) -> Option<ArrayValue> + 'a>;

/// Evaluates formula strings and literal values.
///
/// Requires a `cell_resolver` closure that returns the current value of any
/// cell by its ID (e.g. "A1").
///
/// `max_row` and `max_col` bound whole-column (`A:A`) and whole-row (`1:1`)
/// ranges so they don't iterate infinitely.
///
/// The `'a` lifetime defaults to `'static` for callers that don't attach a
/// borrowing spill resolver — so `Evaluator::new(...)` still works without
/// any lifetime annotation.
pub struct Evaluator<'a, F>
where
    F: Fn(&str) -> CellValue,
{
    cell_resolver: F,
    /// Optional spill-array resolver. Called when evaluating a `SpillRef`
    /// node (`A1#`). Returns `None` if the target isn't an anchor; eval
    /// then yields `#REF!`. Boxed to avoid a second generic parameter.
    spill_resolver: Option<SpillResolver<'a>>,
    /// Custom-type + custom-function registry. Defaults to empty.
    registry: Arc<Registry>,
    /// 1-based max row for column ranges (default 1000).
    max_row: u32,
    /// 1-based max column index for row ranges (default 26 = Z).
    max_col: u32,
}

impl<F> Evaluator<'static, F>
where
    F: Fn(&str) -> CellValue,
{
    #[must_use]
    pub fn new(cell_resolver: F) -> Self {
        Evaluator {
            cell_resolver,
            spill_resolver: None,
            registry: Arc::new(Registry::default()),
            max_row: 1000,
            max_col: 26,
        }
    }

    #[must_use]
    pub fn with_bounds(cell_resolver: F, max_row: u32, max_col: u32) -> Self {
        Evaluator {
            cell_resolver,
            spill_resolver: None,
            registry: Arc::new(Registry::default()),
            max_row,
            max_col,
        }
    }
}

impl<'a, F> Evaluator<'a, F>
where
    F: Fn(&str) -> CellValue,
{
    /// Builder: attach a closure that resolves a spill-anchor cell id
    /// to its full ArrayValue. Used by `Sheet` to make `A1#` work.
    ///
    /// The resolver may borrow data with lifetime `'a`, which lets callers
    /// avoid cloning their backing store per evaluation.
    #[must_use]
    pub fn with_spill_resolver(
        mut self,
        resolver: impl Fn(&str) -> Option<ArrayValue> + 'a,
    ) -> Self {
        self.spill_resolver = Some(Box::new(resolver));
        self
    }

    /// Builder: attach a custom-type / custom-function registry.
    #[must_use]
    pub fn with_registry(mut self, registry: Arc<Registry>) -> Self {
        self.registry = registry;
        self
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// Evaluate an input string. Non-formula inputs are returned as literals.
    ///
    /// Ingestion order for non-formula input: registered custom-type
    /// handlers (in registration order) → numeric coercion → boolean
    /// literal → empty → string. A handler can therefore claim a literal
    /// that `f64::parse` would also accept (e.g. `"3.14 rad"` or even
    /// bare `"3.14"`). Boolean literals (`TRUE`/`FALSE`, case-insensitive
    /// — Excel convention) match exactly with no surrounding whitespace
    /// or punctuation.
    pub fn evaluate(&self, input: &str) -> Result<CellValue, FormulaError> {
        if !input.starts_with('=') {
            if let Some(cv) = self.registry.parse_literal(input) {
                return Ok(CellValue::Custom(cv));
            }
            if let Ok(n) = input.parse::<f64>() {
                if n.is_finite() {
                    return Ok(CellValue::Number(n));
                }
            }
            if let Some(b) = parse_bool_literal(input) {
                return Ok(CellValue::Boolean(b));
            }
            return Ok(if input.is_empty() {
                CellValue::Empty
            } else {
                CellValue::String(input.to_string())
            });
        }

        let ast = Parser::parse(input)?;
        self.eval_ast(&ast)
    }

    /// Evaluate a pre-parsed AST, returning the top-left value if the result
    /// is an array. Used by callers that want legacy scalar behavior.
    pub fn eval_ast(&self, ast: &AstNode) -> Result<CellValue, FormulaError> {
        Ok(self.eval_ast_array(ast)?.first())
    }

    /// Evaluate a pre-parsed AST, returning the full shaped result.
    /// A scalar comes back as a 1×1 array.
    pub fn eval_ast_array(&self, ast: &AstNode) -> Result<ArrayValue, FormulaError> {
        self.eval_node(ast)
    }

    fn eval_node(&self, node: &AstNode) -> Result<ArrayValue, FormulaError> {
        match node {
            AstNode::Number(v) => Ok(ArrayValue::scalar(CellValue::Number(*v))),

            AstNode::String(v) => Ok(ArrayValue::scalar(CellValue::String(v.clone()))),

            AstNode::Boolean(b) => Ok(ArrayValue::scalar(CellValue::Boolean(*b))),

            AstNode::CellRef { col, row } => {
                let id = cell_id(col, *row);
                Ok(ArrayValue::scalar((self.cell_resolver)(&id)))
            }

            AstNode::Range {
                start_col,
                start_row,
                end_col,
                end_row,
            } => self.expand_range(start_col, *start_row, end_col, *end_row),

            AstNode::ColumnRange { start_col, end_col } => {
                self.expand_range(start_col, 1, end_col, self.max_row)
            }

            AstNode::RowRange { start_row, end_row } => {
                let start_col = index_to_col(1);
                let end_col = index_to_col(self.max_col);
                self.expand_range(&start_col, *start_row, &end_col, *end_row)
            }

            AstNode::BinaryOp {
                operator,
                left,
                right,
            } => {
                let op = BinaryOp::from_char(*operator)
                    .ok_or_else(|| format!("Unknown operator: {operator}"))?;
                let l = self.eval_node(left)?;
                let r = self.eval_node(right)?;
                broadcast_binop(op, &l, &r, &self.registry)
            }

            AstNode::UnaryOp { operator, operand } => {
                let v = self.eval_node(operand)?;
                let mut data = Vec::with_capacity(v.data.len());
                for cell in v.iter() {
                    data.push(eval_unary_op(*operator, cell)?);
                }
                Ok(ArrayValue::new(v.rows, v.cols, data))
            }

            AstNode::FunctionCall { name, args } => {
                // Each arg preserves its shape; the function's FnKind
                // decides how to consume arrays (flatten / elementwise /
                // pass-through).
                let mut evaluated = Vec::with_capacity(args.len());
                for arg in args {
                    evaluated.push(self.eval_node(arg)?);
                }
                call_function(name, &evaluated, &self.registry)
            }

            AstNode::Compare { op, left, right } => {
                let l = self.eval_node(left)?;
                let r = self.eval_node(right)?;
                broadcast_compare(*op, &l, &r, &self.registry)
            }

            AstNode::SpillRef { col, row } => {
                let id = cell_id(col, *row);
                if let Some(resolver) = &self.spill_resolver {
                    if let Some(arr) = resolver(&id) {
                        return Ok(arr);
                    }
                }
                // Not an anchor (or no resolver wired up).
                Ok(ArrayValue::scalar(CellValue::Error(FormulaError::Ref)))
            }

            AstNode::ArrayLiteral { rows } => {
                let row_count = rows.len() as u32;
                let col_count = rows.first().map(|r| r.len()).unwrap_or(0) as u32;
                let total = (row_count as usize) * (col_count as usize);
                if total > MAX_RANGE_CELLS {
                    return Err(FormulaError::size(format!(
                        "array literal exceeds {MAX_RANGE_CELLS} cells"
                    )));
                }
                let mut data = Vec::with_capacity(total);
                for row in rows {
                    for cell in row {
                        // Each entry must collapse to a scalar — nested
                        // arrays in literals aren't supported (would lose
                        // dimensional information).
                        let v = self.eval_node(cell)?.first();
                        data.push(v);
                    }
                }
                Ok(ArrayValue::new(row_count, col_count, data))
            }

            // Safety net: callers should run `resolve_names` before evaluation,
            // so a bare Name here means no resolver was wired up (or the name
            // wasn't defined). Mirrors Excel's `#NAME?` for unresolved refs.
            AstNode::Name(_) => Err(FormulaError::Name),
        }
    }

    /// Expand a rectangular range into a 2-D `ArrayValue` shaped to match.
    fn expand_range(
        &self,
        start_col: &str,
        start_row: u32,
        end_col: &str,
        end_row: u32,
    ) -> Result<ArrayValue, FormulaError> {
        let sc = col_to_index(start_col);
        let ec = col_to_index(end_col);
        let sr = start_row.min(end_row);
        let er = start_row.max(end_row);
        let c_lo = sc.min(ec);
        let c_hi = sc.max(ec);

        let rows = (er - sr + 1) as usize;
        let cols = (c_hi - c_lo + 1) as usize;
        if rows.saturating_mul(cols) > MAX_RANGE_CELLS {
            return Err(FormulaError::size(format!(
                "range exceeds {MAX_RANGE_CELLS} cells"
            )));
        }

        let mut data = Vec::with_capacity(rows * cols);
        for row in sr..=er {
            for col in c_lo..=c_hi {
                let id = cell_id(&index_to_col(col), row);
                data.push((self.cell_resolver)(&id));
            }
        }
        Ok(ArrayValue::new(rows as u32, cols as u32, data))
    }
}

/// Negotiate a NumPy-style output shape for elementwise ops: dims must
/// either match or be 1. Shared by `broadcast_binop`/`broadcast_compare`.
fn broadcast_shape(l: &ArrayValue, r: &ArrayValue, label: &str) -> Result<(u32, u32), FormulaError> {
    let out_rows = match (l.rows, r.rows) {
        (a, b) if a == b => a,
        (1, b) => b,
        (a, 1) => a,
        _ => return Err(FormulaError::value("incompatible array shapes")),
    };
    let out_cols = match (l.cols, r.cols) {
        (a, b) if a == b => a,
        (1, b) => b,
        (a, 1) => a,
        _ => return Err(FormulaError::value("incompatible array shapes")),
    };
    let total = (out_rows as usize).saturating_mul(out_cols as usize);
    if total > MAX_RANGE_CELLS {
        return Err(FormulaError::size(format!(
            "{label} result exceeds {MAX_RANGE_CELLS} cells"
        )));
    }
    Ok((out_rows, out_cols))
}

/// For a broadcast output cell at `(r, c)`, pick the corresponding cell
/// from `a`, honoring dim-1 broadcasting.
fn pick(a: &ArrayValue, r: u32, c: u32) -> &CellValue {
    let ar = if a.rows == 1 { 0 } else { r };
    let ac = if a.cols == 1 { 0 } else { c };
    a.get(ar, ac)
}

/// Apply a binary operator elementwise across two arrays. Output shape
/// is `(max(rows_l, rows_r), max(cols_l, cols_r))`. Per-cell errors
/// in non-scalar broadcasts are stamped in-place; scalar errors
/// propagate so `evaluate("=1/0")` still returns `Err`.
fn broadcast_binop(
    op: BinaryOp,
    l: &ArrayValue,
    r: &ArrayValue,
    reg: &Registry,
) -> Result<ArrayValue, FormulaError> {
    let (out_rows, out_cols) = broadcast_shape(l, r, "broadcast")?;
    let total = (out_rows as usize) * (out_cols as usize);
    let mut data = Vec::with_capacity(total);
    let mut scalar_err: Option<FormulaError> = None;
    for r_idx in 0..out_rows {
        for c_idx in 0..out_cols {
            let lv = pick(l, r_idx, c_idx);
            let rv = pick(r, r_idx, c_idx);
            match eval_binary_op(op, lv, rv, reg) {
                Ok(v) => data.push(v),
                Err(e) => {
                    if total == 1 && scalar_err.is_none() {
                        scalar_err = Some(e.clone());
                    }
                    data.push(CellValue::Error(e));
                }
            }
        }
    }
    if let Some(e) = scalar_err {
        return Err(e);
    }
    Ok(ArrayValue::new(out_rows, out_cols, data))
}

/// Broadcast a comparison across two arrays. Result cells are
/// `Boolean(true/false)` — they coerce to 1/0 numerically (via
/// `as_number`) for IF / FILTER / arithmetic, so this is type-safe
/// without breaking existing consumers.
fn broadcast_compare(
    op: CompareOp,
    l: &ArrayValue,
    r: &ArrayValue,
    reg: &Registry,
) -> Result<ArrayValue, FormulaError> {
    let (out_rows, out_cols) = broadcast_shape(l, r, "comparison")?;
    let total = (out_rows as usize) * (out_cols as usize);
    let mut data = Vec::with_capacity(total);
    let mut scalar_err: Option<FormulaError> = None;
    for r_idx in 0..out_rows {
        for c_idx in 0..out_cols {
            match compare_cells(op, pick(l, r_idx, c_idx), pick(r, r_idx, c_idx), reg) {
                Ok(out) => data.push(CellValue::Boolean(out)),
                Err(e) => {
                    // Mirror broadcast_binop: stamp the error in-place for
                    // arrays, propagate as Err only if the result is scalar.
                    if total == 1 && scalar_err.is_none() {
                        scalar_err = Some(e.clone());
                    }
                    data.push(CellValue::Error(e));
                }
            }
        }
    }
    if let Some(e) = scalar_err {
        return Err(e);
    }
    Ok(ArrayValue::new(out_rows, out_cols, data))
}

/// Evaluate a single comparison. Rules:
/// - Custom values (on either side): registry dispatches first; if no
///   handler claims the op, falls through to a conservative default —
///   equal iff both sides are `Custom` with identical `type_tag` + `data`,
///   otherwise non-equal and non-ordered (any ordering compare → false).
/// - Number vs Number: numeric.
/// - String vs String: lexicographic.
/// - Empty is equal to Empty; unequal to everything else (for `=`).
/// - Mixed types: false for `=`, true for `<>`; other comparisons fall
///   back to a total order (numbers < strings < empty) to keep SORT/
///   FILTER consistent with the rest of the engine.
fn compare_cells(
    op: CompareOp,
    a: &CellValue,
    b: &CellValue,
    reg: &Registry,
) -> Result<bool, FormulaError> {
    use std::cmp::Ordering;
    let bf = |b: bool| if b { 1.0_f64 } else { 0.0 };
    let ord = match (a, b) {
        // Comparing with a formula error propagates the error (Excel parity).
        (CellValue::Error(e), _) | (_, CellValue::Error(e)) => return Err(e.clone()),
        (CellValue::Custom(_), _) | (_, CellValue::Custom(_)) => {
            if let Some(result) = reg.dispatch_compare(op, a, b) {
                return result;
            }
            let equal = matches!(
                (a, b),
                (CellValue::Custom(x), CellValue::Custom(y)) if x == y
            );
            return Ok(match op {
                CompareOp::Eq => equal,
                CompareOp::Ne => !equal,
                _ => false,
            });
        }
        (CellValue::Number(x), CellValue::Number(y)) => {
            x.partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        // Booleans coerce to 1/0 against any numeric-like value.
        (CellValue::Boolean(x), CellValue::Boolean(y)) => x.cmp(y),
        (CellValue::Number(x), CellValue::Boolean(y)) => {
            x.partial_cmp(&bf(*y)).unwrap_or(Ordering::Equal)
        }
        (CellValue::Boolean(x), CellValue::Number(y)) => {
            bf(*x).partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (CellValue::String(x), CellValue::String(y)) => x.cmp(y),
        (CellValue::Empty, CellValue::Empty) => Ordering::Equal,
        // Excel treats empty as 0 for numeric comparisons.
        (CellValue::Number(x), CellValue::Empty) => x.partial_cmp(&0.0).unwrap_or(Ordering::Equal),
        (CellValue::Empty, CellValue::Number(y)) => 0.0_f64.partial_cmp(y).unwrap_or(Ordering::Equal),
        (CellValue::Boolean(x), CellValue::Empty) => bf(*x).partial_cmp(&0.0).unwrap_or(Ordering::Equal),
        (CellValue::Empty, CellValue::Boolean(y)) => 0.0_f64.partial_cmp(&bf(*y)).unwrap_or(Ordering::Equal),
        (CellValue::String(x), CellValue::Empty) => x.as_str().cmp(""),
        (CellValue::Empty, CellValue::String(y)) => "".cmp(y.as_str()),
        (CellValue::Number(_) | CellValue::Boolean(_), CellValue::String(_)) => Ordering::Less,
        (CellValue::String(_), CellValue::Number(_) | CellValue::Boolean(_)) => Ordering::Greater,
    };
    Ok(match op {
        CompareOp::Eq => ord == Ordering::Equal,
        CompareOp::Ne => ord != Ordering::Equal,
        CompareOp::Lt => ord == Ordering::Less,
        CompareOp::Le => ord != Ordering::Greater,
        CompareOp::Gt => ord == Ordering::Greater,
        CompareOp::Ge => ord != Ordering::Less,
    })
}

fn eval_binary_op(
    op: BinaryOp,
    left: &CellValue,
    right: &CellValue,
    reg: &Registry,
) -> Result<CellValue, FormulaError> {
    // Custom-type handlers get first crack at any op involving a Custom.
    // A `qty` handler claims `km + mi`; a `polygon` handler declines arithmetic.
    if matches!(left, CellValue::Custom(_)) || matches!(right, CellValue::Custom(_)) {
        if let Some(result) = reg.dispatch_binary_op(op, left, right) {
            return result;
        }
    }

    // `&` always concatenates as strings. Empty cells render as "";
    // Custom values go through the registry's `display`.
    if op == BinaryOp::Concat {
        return Ok(CellValue::String(format!(
            "{}{}",
            reg.render(left),
            reg.render(right)
        )));
    }

    match (reg.coerce_number(left), reg.coerce_number(right)) {
        (Some(l), Some(r)) => match op {
            BinaryOp::Add => Ok(CellValue::Number(l + r)),
            BinaryOp::Sub => Ok(CellValue::Number(l - r)),
            BinaryOp::Mul => Ok(CellValue::Number(l * r)),
            BinaryOp::Div => {
                if r == 0.0 {
                    Err(FormulaError::DivByZero)
                } else {
                    Ok(CellValue::Number(l / r))
                }
            }
            BinaryOp::Pow => Ok(CellValue::Number(l.powf(r))),
            BinaryOp::Mod => Ok(CellValue::Number(l % r)),
            BinaryOp::Concat => unreachable!("handled above"),
        },
        // `+` falls back to string concat when either side isn't numeric;
        // other ops silently drop to Empty (matches legacy behavior).
        _ if op == BinaryOp::Add => Ok(CellValue::String(format!(
            "{}{}",
            reg.render(left),
            reg.render(right)
        ))),
        _ => Ok(CellValue::Empty),
    }
}

/// Recognise an Excel-style boolean literal. Case-insensitive on the four
/// ASCII letters (`tRuE` → `Some(true)`); rejects any surrounding
/// whitespace or trailing characters so `"TRUE "` and `"TRUE!"` stay
/// strings.
pub(crate) fn parse_bool_literal(input: &str) -> Option<bool> {
    if input.eq_ignore_ascii_case("TRUE") {
        Some(true)
    } else if input.eq_ignore_ascii_case("FALSE") {
        Some(false)
    } else {
        None
    }
}

fn eval_unary_op(operator: char, operand: &CellValue) -> Result<CellValue, FormulaError> {
    match operand.as_number() {
        Some(n) => match operator {
            '+' => Ok(CellValue::Number(n)),
            '-' => Ok(CellValue::Number(-n)),
            _ => Err(FormulaError::Custom(format!(
                "Unknown unary operator: {operator}"
            ))),
        },
        None => Ok(CellValue::Empty),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_eval(cells: HashMap<&str, CellValue>) -> Evaluator<'static, impl Fn(&str) -> CellValue> {
        let map: HashMap<String, CellValue> = cells
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        Evaluator::new(move |id: &str| {
            map.get(id).cloned().unwrap_or(CellValue::Empty)
        })
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn literal_number() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("42").unwrap(), CellValue::Number(42.0));
        assert_eq!(eval.evaluate("3.14").unwrap(), CellValue::Number(3.14));
    }

    #[test]
    fn literal_string() {
        let eval = make_eval(HashMap::new());
        assert_eq!(
            eval.evaluate("hello").unwrap(),
            CellValue::String("hello".into())
        );
    }

    #[test]
    fn empty_input() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("").unwrap(), CellValue::Empty);
    }

    #[test]
    fn literal_boolean() {
        let eval = make_eval(HashMap::new());
        for s in ["TRUE", "True", "true", "tRuE"] {
            assert_eq!(eval.evaluate(s).unwrap(), CellValue::Boolean(true), "{s}");
        }
        for s in ["FALSE", "False", "false", "fAlSe"] {
            assert_eq!(eval.evaluate(s).unwrap(), CellValue::Boolean(false), "{s}");
        }
    }

    #[test]
    fn boolean_literal_requires_exact_match() {
        // Whitespace and trailing punctuation keep the value as a string.
        let eval = make_eval(HashMap::new());
        for s in ["TRUE ", " TRUE", "TRUE!", "TRUEX", "FALSE."] {
            assert_eq!(
                eval.evaluate(s).unwrap(),
                CellValue::String(s.into()),
                "{s}"
            );
        }
    }

    #[test]
    fn simple_addition() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=1 + 2").unwrap(), CellValue::Number(3.0));
    }

    #[test]
    fn operator_precedence() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=2 + 3 * 4").unwrap(), CellValue::Number(14.0));
        assert_eq!(eval.evaluate("=(2 + 3) * 4").unwrap(), CellValue::Number(20.0));
    }

    #[test]
    fn cell_references() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(10.0));
        cells.insert("A2", CellValue::Number(20.0));
        let eval = make_eval(cells);
        assert_eq!(eval.evaluate("=A1 + A2").unwrap(), CellValue::Number(30.0));
    }

    #[test]
    fn sum_function_with_range() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        cells.insert("A2", CellValue::Number(2.0));
        cells.insert("A3", CellValue::Number(3.0));
        let eval = make_eval(cells);
        assert_eq!(eval.evaluate("=SUM(A1:A3)").unwrap(), CellValue::Number(6.0));
    }

    #[test]
    fn nested_functions() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(5.0));
        cells.insert("A2", CellValue::Number(3.0));
        let eval = make_eval(cells);
        assert_eq!(eval.evaluate("=SUM(A1, MAX(A1, A2))").unwrap(), CellValue::Number(10.0));
    }

    #[test]
    fn div_by_zero() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=1/0").unwrap_err(), FormulaError::DivByZero);
    }

    #[test]
    fn string_concatenation_with_plus() {
        let eval = make_eval(HashMap::new());
        assert_eq!(
            eval.evaluate("=\"a\"+\"b\"").unwrap(),
            CellValue::String("ab".into())
        );
    }

    #[test]
    fn unary_negation() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=-5 + 3").unwrap(), CellValue::Number(-2.0));
    }

    #[test]
    fn if_function() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        let eval = make_eval(cells);
        assert_eq!(
            eval.evaluate("=IF(A1, \"yes\", \"no\")").unwrap(),
            CellValue::String("yes".into())
        );
    }

    #[test]
    fn empty_cell_in_formula() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=A1").unwrap(), CellValue::Empty);
    }

    #[test]
    fn oversized_range_errors_with_size() {
        let eval = make_eval(HashMap::new());
        let err = eval.evaluate("=SUM(A1:A200000)").unwrap_err();
        assert!(matches!(err, FormulaError::Size(_)), "got: {err:?}");
    }

    #[test]
    fn modest_range_still_sums() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=SUM(A1:A100)").unwrap(), CellValue::Number(0.0));
    }

    #[test]
    fn concat_function() {
        let eval = make_eval(HashMap::new());
        assert_eq!(
            eval.evaluate(r#"=CONCAT("a", "b", "c")"#).unwrap(),
            CellValue::String("abc".into())
        );
    }

    #[test]
    fn unresolved_name_yields_name_error() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=Foo").unwrap_err(), FormulaError::Name);
    }

    // ── string concatenation with & ─────────────────────────────────────

    #[test]
    fn concat_two_strings() {
        let eval = make_eval(HashMap::new());
        assert_eq!(
            eval.evaluate(r#"="hello" & " " & "world""#).unwrap(),
            CellValue::String("hello world".into())
        );
    }

    #[test]
    fn concat_string_with_number() {
        let eval = make_eval(HashMap::new());
        assert_eq!(
            eval.evaluate(r#"="count: " & 42"#).unwrap(),
            CellValue::String("count: 42".into())
        );
    }

    #[test]
    fn concat_with_cell_ref() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::String("world".into()));
        let eval = make_eval(cells);
        assert_eq!(
            eval.evaluate(r#"="hello " & A1"#).unwrap(),
            CellValue::String("hello world".into())
        );
    }

    #[test]
    fn concat_with_empty_cell_renders_empty() {
        let eval = make_eval(HashMap::new());
        assert_eq!(
            eval.evaluate(r#"="x" & A1 & "y""#).unwrap(),
            CellValue::String("xy".into())
        );
    }

    #[test]
    fn concat_lower_precedence_than_addition() {
        let eval = make_eval(HashMap::new());
        assert_eq!(
            eval.evaluate(r#"="a" & 1 + 2"#).unwrap(),
            CellValue::String("a3".into())
        );
    }

    #[test]
    fn concat_two_numbers_produces_string() {
        let eval = make_eval(HashMap::new());
        assert_eq!(
            eval.evaluate("=12 & 34").unwrap(),
            CellValue::String("1234".into())
        );
    }

    // ── ArrayValue shape via eval_ast_array ────────────────────────────

    #[test]
    fn scalar_result_is_1x1_array() {
        let eval = make_eval(HashMap::new());
        let ast = Parser::parse("=1 + 2").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (1, 1));
        assert_eq!(arr.first(), CellValue::Number(3.0));
    }

    #[test]
    fn range_result_is_2d_shaped() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        cells.insert("A2", CellValue::Number(2.0));
        cells.insert("A3", CellValue::Number(3.0));
        cells.insert("B1", CellValue::Number(10.0));
        cells.insert("B2", CellValue::Number(20.0));
        cells.insert("B3", CellValue::Number(30.0));
        let eval = make_eval(cells);
        let ast = Parser::parse("=A1:B3").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 2));
        assert_eq!(*arr.get(0, 0), CellValue::Number(1.0));
        assert_eq!(*arr.get(0, 1), CellValue::Number(10.0));
        assert_eq!(*arr.get(2, 1), CellValue::Number(30.0));
    }

    #[test]
    fn single_column_range_is_n_by_1() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        cells.insert("A2", CellValue::Number(2.0));
        let eval = make_eval(cells);
        let ast = Parser::parse("=A1:A2").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (2, 1));
    }

    #[test]
    fn single_row_range_is_1_by_n() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        cells.insert("B1", CellValue::Number(2.0));
        cells.insert("C1", CellValue::Number(3.0));
        let eval = make_eval(cells);
        let ast = Parser::parse("=A1:C1").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (1, 3));
    }

    #[test]
    fn eval_ast_collapses_array_to_first() {
        // Backwards-compat: eval_ast returns the top-left element.
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        cells.insert("A2", CellValue::Number(2.0));
        cells.insert("A3", CellValue::Number(3.0));
        let eval = make_eval(cells);
        let ast = Parser::parse("=A1:A3").unwrap();
        assert_eq!(eval.eval_ast(&ast).unwrap(), CellValue::Number(1.0));
    }

    // ── broadcasting ───────────────────────────────────────────────────

    fn range_cells() -> HashMap<&'static str, CellValue> {
        let mut c = HashMap::new();
        c.insert("A1", CellValue::Number(1.0));
        c.insert("A2", CellValue::Number(2.0));
        c.insert("A3", CellValue::Number(3.0));
        c.insert("B1", CellValue::Number(10.0));
        c.insert("B2", CellValue::Number(20.0));
        c.insert("B3", CellValue::Number(30.0));
        c
    }

    #[test]
    fn broadcast_scalar_times_column() {
        let eval = make_eval(range_cells());
        let ast = Parser::parse("=A1:A3 * 2").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 1));
        assert_eq!(*arr.get(0, 0), CellValue::Number(2.0));
        assert_eq!(*arr.get(1, 0), CellValue::Number(4.0));
        assert_eq!(*arr.get(2, 0), CellValue::Number(6.0));
    }

    #[test]
    fn broadcast_scalar_on_left() {
        let eval = make_eval(range_cells());
        let ast = Parser::parse("=10 - A1:A3").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 1));
        assert_eq!(*arr.get(0, 0), CellValue::Number(9.0));
        assert_eq!(*arr.get(1, 0), CellValue::Number(8.0));
        assert_eq!(*arr.get(2, 0), CellValue::Number(7.0));
    }

    #[test]
    fn broadcast_same_shape_elementwise() {
        let eval = make_eval(range_cells());
        let ast = Parser::parse("=A1:A3 + B1:B3").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 1));
        assert_eq!(*arr.get(0, 0), CellValue::Number(11.0));
        assert_eq!(*arr.get(1, 0), CellValue::Number(22.0));
        assert_eq!(*arr.get(2, 0), CellValue::Number(33.0));
    }

    #[test]
    fn broadcast_row_times_column_cross_product() {
        // (3, 1) * (1, 2) → (3, 2)
        // A1:A3 is 3×1, A1:B1 is 1×2.
        let eval = make_eval(range_cells());
        let ast = Parser::parse("=A1:A3 * A1:B1").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 2));
        assert_eq!(*arr.get(0, 0), CellValue::Number(1.0));   // 1 * 1
        assert_eq!(*arr.get(0, 1), CellValue::Number(10.0));  // 1 * 10
        assert_eq!(*arr.get(1, 0), CellValue::Number(2.0));   // 2 * 1
        assert_eq!(*arr.get(1, 1), CellValue::Number(20.0));  // 2 * 10
        assert_eq!(*arr.get(2, 1), CellValue::Number(30.0));  // 3 * 10
    }

    #[test]
    fn broadcast_incompatible_shapes_errors() {
        let mut cells = range_cells();
        cells.insert("A4", CellValue::Number(4.0));
        cells.insert("A5", CellValue::Number(5.0));
        let eval = make_eval(cells);
        // 3×1 vs 5×1 — rows don't match and neither is 1
        let ast = Parser::parse("=A1:A3 + A1:A5").unwrap();
        let err = eval.eval_ast_array(&ast).unwrap_err();
        assert!(matches!(err, FormulaError::Value(_)), "got: {err:?}");
    }

    #[test]
    fn broadcast_per_cell_error_does_not_abort_array() {
        // =A1:A3 / {1, 0, 1} would need array literals; fake it with
        // two ranges where one divisor is 0.
        let mut cells = range_cells();
        cells.insert("B2", CellValue::Number(0.0)); // override: make B2 a zero divisor
        let eval = make_eval(cells);
        let ast = Parser::parse("=A1:A3 / B1:B3").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 1));
        // A1/B1 = 1/10 = 0.1
        assert_eq!(*arr.get(0, 0), CellValue::Number(0.1));
        // A2/B2 = 2/0 = #DIV/0! (in-place, not aborting)
        assert_eq!(*arr.get(1, 0), CellValue::Error(FormulaError::DivByZero));
        // A3/B3 = 3/30 = 0.1
        assert_eq!(*arr.get(2, 0), CellValue::Number(0.1));
    }

    #[test]
    fn scalar_div_by_zero_still_errors() {
        // Preserve legacy Err behavior for the scalar case so
        // `evaluate("=1/0")` raises at the Python boundary.
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=1/0").unwrap_err(), FormulaError::DivByZero);
    }

    #[test]
    fn unary_negation_on_array() {
        let eval = make_eval(range_cells());
        let ast = Parser::parse("=-A1:A3").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 1));
        assert_eq!(*arr.get(0, 0), CellValue::Number(-1.0));
        assert_eq!(*arr.get(1, 0), CellValue::Number(-2.0));
        assert_eq!(*arr.get(2, 0), CellValue::Number(-3.0));
    }

    // ── array functions through eval ──

    #[test]
    fn eval_sequence_produces_shaped_array() {
        let eval = make_eval(HashMap::new());
        let ast = Parser::parse("=SEQUENCE(3, 2)").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 2));
        assert_eq!(*arr.get(0, 0), CellValue::Number(1.0));
        assert_eq!(*arr.get(2, 1), CellValue::Number(6.0));
    }

    #[test]
    fn eval_transpose_range() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        cells.insert("B1", CellValue::Number(2.0));
        cells.insert("C1", CellValue::Number(3.0));
        let eval = make_eval(cells);
        let ast = Parser::parse("=TRANSPOSE(A1:C1)").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 1));
        assert_eq!(*arr.get(0, 0), CellValue::Number(1.0));
        assert_eq!(*arr.get(2, 0), CellValue::Number(3.0));
    }

    #[test]
    fn eval_filter_with_comparison_broadcast() {
        // This also exercises the future story once we have comparisons.
        // For now FILTER takes a pre-computed boolean column, which we
        // simulate with a range of 0/1.
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        cells.insert("A2", CellValue::Number(2.0));
        cells.insert("A3", CellValue::Number(3.0));
        cells.insert("B1", CellValue::Number(0.0));
        cells.insert("B2", CellValue::Number(1.0));
        cells.insert("B3", CellValue::Number(1.0));
        let eval = make_eval(cells);
        let ast = Parser::parse("=FILTER(A1:A3, B1:B3)").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (2, 1));
        assert_eq!(*arr.get(0, 0), CellValue::Number(2.0));
        assert_eq!(*arr.get(1, 0), CellValue::Number(3.0));
    }

    #[test]
    fn eval_array_literal_1d() {
        let eval = make_eval(HashMap::new());
        let ast = Parser::parse("={10, 20, 30}").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (1, 3));
        assert_eq!(*arr.get(0, 0), CellValue::Number(10.0));
        assert_eq!(*arr.get(0, 2), CellValue::Number(30.0));
    }

    #[test]
    fn eval_array_literal_2d() {
        let eval = make_eval(HashMap::new());
        let ast = Parser::parse("={10, 20; 30, 40}").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (2, 2));
        assert_eq!(*arr.get(0, 0), CellValue::Number(10.0));
        assert_eq!(*arr.get(0, 1), CellValue::Number(20.0));
        assert_eq!(*arr.get(1, 0), CellValue::Number(30.0));
        assert_eq!(*arr.get(1, 1), CellValue::Number(40.0));
    }

    #[test]
    fn eval_array_literal_with_expressions() {
        let eval = make_eval(HashMap::new());
        let ast = Parser::parse("={1+2, 3*4; 5-1, 6/2}").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (2, 2));
        assert_eq!(*arr.get(0, 0), CellValue::Number(3.0));
        assert_eq!(*arr.get(0, 1), CellValue::Number(12.0));
        assert_eq!(*arr.get(1, 0), CellValue::Number(4.0));
        assert_eq!(*arr.get(1, 1), CellValue::Number(3.0));
    }

    #[test]
    fn eval_array_literal_broadcasts_with_binary_op() {
        let eval = make_eval(HashMap::new());
        let ast = Parser::parse("={1, 2, 3} * 10").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (1, 3));
        assert_eq!(*arr.get(0, 0), CellValue::Number(10.0));
        assert_eq!(*arr.get(0, 2), CellValue::Number(30.0));
    }

    #[test]
    fn eval_array_literal_feeds_sum() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=SUM({1, 2, 3; 4, 5, 6})").unwrap(), CellValue::Number(21.0));
    }

    #[test]
    fn eval_array_literal_with_mixed_types() {
        let eval = make_eval(HashMap::new());
        let ast = Parser::parse(r#"={1, "hello"; 3, "world"}"#).unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (2, 2));
        assert_eq!(*arr.get(0, 1), CellValue::String("hello".into()));
    }

    #[test]
    fn eval_abs_elementwise_over_range_collapses_for_scalar_caller() {
        // eval_ast returns top-left: ABS of a range gives |A1|.
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(-5.0));
        cells.insert("A2", CellValue::Number(-7.0));
        let eval = make_eval(cells);
        assert_eq!(eval.evaluate("=ABS(A1:A2)").unwrap(), CellValue::Number(5.0));
        // But eval_ast_array gives the full shape.
        let ast = Parser::parse("=ABS(A1:A2)").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (2, 1));
        assert_eq!(*arr.get(0, 0), CellValue::Number(5.0));
        assert_eq!(*arr.get(1, 0), CellValue::Number(7.0));
    }

    // ── comparisons ────────────────────────────────────────────────────

    #[test]
    fn eval_eq_true_and_false() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=1 = 1").unwrap(), CellValue::Boolean(true));
        assert_eq!(eval.evaluate("=1 = 2").unwrap(), CellValue::Boolean(false));
    }

    #[test]
    fn eval_ne() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=1 <> 2").unwrap(), CellValue::Boolean(true));
        assert_eq!(eval.evaluate("=1 <> 1").unwrap(), CellValue::Boolean(false));
    }

    #[test]
    fn eval_lt_le_gt_ge() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=1 < 2").unwrap(), CellValue::Boolean(true));
        assert_eq!(eval.evaluate("=2 < 1").unwrap(), CellValue::Boolean(false));
        assert_eq!(eval.evaluate("=1 <= 1").unwrap(), CellValue::Boolean(true));
        assert_eq!(eval.evaluate("=2 > 1").unwrap(), CellValue::Boolean(true));
        assert_eq!(eval.evaluate("=1 >= 1").unwrap(), CellValue::Boolean(true));
        assert_eq!(eval.evaluate("=0 >= 1").unwrap(), CellValue::Boolean(false));
    }

    #[test]
    fn eval_compare_strings() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate(r#"="abc" = "abc""#).unwrap(), CellValue::Boolean(true));
        assert_eq!(eval.evaluate(r#"="a" < "b""#).unwrap(), CellValue::Boolean(true));
    }

    #[test]
    fn eval_compare_empty_cell_treated_as_zero() {
        let eval = make_eval(HashMap::new());
        // A1 is empty → 0 for numeric compare.
        assert_eq!(eval.evaluate("=A1 = 0").unwrap(), CellValue::Boolean(true));
        assert_eq!(eval.evaluate("=A1 < 1").unwrap(), CellValue::Boolean(true));
    }

    #[test]
    fn eval_compare_mixed_types_never_equal() {
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate(r#"=1 = "1""#).unwrap(), CellValue::Boolean(false));
        assert_eq!(eval.evaluate(r#"=1 <> "1""#).unwrap(), CellValue::Boolean(true));
    }

    #[test]
    fn eval_compare_broadcasts_scalar_vs_column() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        cells.insert("A2", CellValue::Number(5.0));
        cells.insert("A3", CellValue::Number(3.0));
        let eval = make_eval(cells);
        let ast = Parser::parse("=A1:A3 > 2").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 1));
        assert_eq!(*arr.get(0, 0), CellValue::Boolean(false)); // 1 > 2
        assert_eq!(*arr.get(1, 0), CellValue::Boolean(true));  // 5 > 2
        assert_eq!(*arr.get(2, 0), CellValue::Boolean(true));  // 3 > 2
    }

    #[test]
    fn eval_compare_feeds_filter() {
        // The motivating case: =FILTER(A1:A5, A1:A5 > 2)
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        cells.insert("A2", CellValue::Number(2.0));
        cells.insert("A3", CellValue::Number(3.0));
        cells.insert("A4", CellValue::Number(4.0));
        cells.insert("A5", CellValue::Number(5.0));
        let eval = make_eval(cells);
        let ast = Parser::parse("=FILTER(A1:A5, A1:A5 > 2)").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 1));
        assert_eq!(*arr.get(0, 0), CellValue::Number(3.0));
        assert_eq!(*arr.get(1, 0), CellValue::Number(4.0));
        assert_eq!(*arr.get(2, 0), CellValue::Number(5.0));
    }

    #[test]
    fn eval_compare_feeds_if() {
        // Array condition into IF: ={"pos", "neg", "pos"} for [1, -2, 3].
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        cells.insert("A2", CellValue::Number(-2.0));
        cells.insert("A3", CellValue::Number(3.0));
        let eval = make_eval(cells);
        let ast = Parser::parse(r#"=IF(A1:A3 > 0, "pos", "neg")"#).unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 1));
        assert_eq!(*arr.get(0, 0), CellValue::String("pos".into()));
        assert_eq!(*arr.get(1, 0), CellValue::String("neg".into()));
        assert_eq!(*arr.get(2, 0), CellValue::String("pos".into()));
    }

    #[test]
    fn eval_compare_precedence_lower_than_plus() {
        // 1 + 2 = 3  →  true (3 = 3)
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=1 + 2 = 3").unwrap(), CellValue::Boolean(true));
    }

    #[test]
    fn eval_chained_comparisons_left_associative() {
        // (1 < 2) < 3  →  TRUE < 3 → TRUE (TRUE coerces to 1, 1 < 3).
        // Parser treats comparisons as left-associative, matching most
        // spreadsheet engines which don't chain.
        let eval = make_eval(HashMap::new());
        assert_eq!(eval.evaluate("=1 < 2 < 3").unwrap(), CellValue::Boolean(true));
    }

    #[test]
    fn eval_array_literal_compared_to_scalar() {
        let eval = make_eval(HashMap::new());
        let ast = Parser::parse("={1, 2, 3, 4} >= 3").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (1, 4));
        assert_eq!(*arr.get(0, 0), CellValue::Boolean(false));
        assert_eq!(*arr.get(0, 1), CellValue::Boolean(false));
        assert_eq!(*arr.get(0, 2), CellValue::Boolean(true));
        assert_eq!(*arr.get(0, 3), CellValue::Boolean(true));
    }

    #[test]
    fn eval_compare_incompatible_shapes() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::Number(1.0));
        cells.insert("A2", CellValue::Number(2.0));
        cells.insert("A3", CellValue::Number(3.0));
        cells.insert("B1", CellValue::Number(1.0));
        cells.insert("B2", CellValue::Number(2.0));
        let eval = make_eval(cells);
        // 3×1 vs 2×1 — error.
        let ast = Parser::parse("=A1:A3 = B1:B2").unwrap();
        assert!(matches!(
            eval.eval_ast_array(&ast).unwrap_err(),
            FormulaError::Value(_)
        ));
    }

    #[test]
    fn concat_operator_broadcasts() {
        let mut cells = HashMap::new();
        cells.insert("A1", CellValue::String("x".into()));
        cells.insert("A2", CellValue::String("y".into()));
        cells.insert("A3", CellValue::String("z".into()));
        let eval = make_eval(cells);
        let ast = Parser::parse("=A1:A3 & \"!\"").unwrap();
        let arr = eval.eval_ast_array(&ast).unwrap();
        assert_eq!(arr.shape(), (3, 1));
        assert_eq!(*arr.get(0, 0), CellValue::String("x!".into()));
        assert_eq!(*arr.get(1, 0), CellValue::String("y!".into()));
        assert_eq!(*arr.get(2, 0), CellValue::String("z!".into()));
    }
}
