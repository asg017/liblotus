/// Token types produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenType {
    Number,
    String,
    Boolean,     // `TRUE` / `FALSE` — case-insensitive bare identifier
    CellRef,
    Function,
    Operator,
    LParen,
    RParen,
    LBrace,      // `{` — array literal start
    RBrace,      // `}` — array literal end
    Comma,
    Colon,
    Semicolon,   // `;` — array-literal row separator (only inside `{}`)
    Hash,        // `#` — postfix spill operator (e.g. `A1#`)
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub token_type: TokenType,
    pub value: String,
    pub position: usize,
    /// Number of source bytes this token occupies starting at `position`.
    /// Usually equal to `value.len()`, but diverges for cell refs whose
    /// source form included `$` markers that were stripped from `value`.
    /// Callers computing span ends should use `position + source_len`
    /// rather than `position + value.len()`.
    pub source_len: usize,
}

/// Comparison operators (spreadsheet `=`, `<>`, `<`, `<=`, `>`, `>=`).
/// Evaluates to 1.0 (TRUE) or 0.0 (FALSE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CompareOp {
    pub fn as_str(self) -> &'static str {
        match self {
            CompareOp::Eq => "=",
            CompareOp::Ne => "<>",
            CompareOp::Lt => "<",
            CompareOp::Le => "<=",
            CompareOp::Gt => ">",
            CompareOp::Ge => ">=",
        }
    }
}

/// Binary arithmetic / concatenation operators. Used at the
/// [`crate::custom::CustomTypeHandler::binary_op`] boundary so handlers
/// can pattern-match on a typed value instead of a `char`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Mod,
    Concat,
}

impl BinaryOp {
    pub fn from_char(c: char) -> Option<Self> {
        Some(match c {
            '+' => BinaryOp::Add,
            '-' => BinaryOp::Sub,
            '*' => BinaryOp::Mul,
            '/' => BinaryOp::Div,
            '^' => BinaryOp::Pow,
            '%' => BinaryOp::Mod,
            '&' => BinaryOp::Concat,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Pow => "^",
            BinaryOp::Mod => "%",
            BinaryOp::Concat => "&",
        }
    }
}

/// AST nodes produced by the parser.
#[derive(Debug, Clone, PartialEq)]
pub enum AstNode {
    Number(f64),
    String(String),
    Boolean(bool),
    CellRef {
        col: String,
        row: u32,
    },
    Range {
        start_col: String,
        start_row: u32,
        end_col: String,
        end_row: u32,
    },
    /// Whole-column range like A:A or A:C.
    ColumnRange {
        start_col: String,
        end_col: String,
    },
    /// Whole-row range like 1:1 or 1:5.
    RowRange {
        start_row: u32,
        end_row: u32,
    },
    /// Named-range reference, e.g. `TaxRate`. Resolved before evaluation.
    Name(String),
    BinaryOp {
        operator: char,
        left: Box<AstNode>,
        right: Box<AstNode>,
    },
    UnaryOp {
        operator: char,
        operand: Box<AstNode>,
    },
    FunctionCall {
        name: String,
        args: Vec<AstNode>,
    },
    /// Inline 2-D array literal: `{1,2;3,4}` → 2 rows × 2 cols.
    /// All rows must have the same length (enforced at parse time).
    ArrayLiteral {
        rows: Vec<Vec<AstNode>>,
    },
    /// Spill operator: `A1#` resolves to the full ArrayValue at anchor A1.
    /// If the target cell isn't an anchor, evaluation yields a 1×1 array
    /// containing `#REF!`.
    SpillRef {
        col: String,
        row: u32,
    },
    /// Comparison: `A1 > 5`, `A1:A10 = "x"`, etc. Evaluates to 1/0 with
    /// broadcasting (like other binary ops).
    Compare {
        op: CompareOp,
        left: Box<AstNode>,
        right: Box<AstNode>,
    },
}

/// A runtime-extensible value carried inside [`CellValue::Custom`].
///
/// `type_tag` routes the value to a [`crate::custom::CustomTypeHandler`].
/// `data` is a handler-defined string repr (JSON, canonical literal,
/// file path, …) that crosses WASM/PyO3 boundaries for free.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomValue {
    pub type_tag: String,
    pub data: String,
}

/// A cell value: number, string, boolean, empty, formula error, or a
/// runtime-extensible custom type. Booleans coerce numerically to 1.0/0.0
/// (Excel convention). Errors carry a typed [`crate::FormulaError`] so
/// downstream code (an `IFERROR` impl, bindings) can dispatch on the
/// variant; `Display` reproduces the legacy `#FOO!` strings.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    Number(f64),
    String(String),
    Boolean(bool),
    Empty,
    Error(crate::error::FormulaError),
    Custom(CustomValue),
}

impl CellValue {
    /// Try to coerce to f64 using only core knowledge (no registry).
    ///
    /// Returns `None` for empty, non-numeric strings, and custom values —
    /// custom types that want numeric coercion go through
    /// [`crate::custom::Registry::as_number`].
    pub fn as_number(&self) -> Option<f64> {
        match self {
            CellValue::Number(n) => Some(*n),
            CellValue::String(s) => s.parse::<f64>().ok(),
            CellValue::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
            CellValue::Empty => None,
            // Errors don't coerce numerically — propagation happens via
            // the evaluator's Result chain, not by silently producing 0.
            CellValue::Error(_) => None,
            CellValue::Custom(_) => None,
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            CellValue::Number(n) => *n != 0.0,
            CellValue::String(s) => !s.is_empty(),
            CellValue::Boolean(b) => *b,
            CellValue::Empty => false,
            // Errors are never truthy. An `IF(error, …)` evaluates the
            // false branch (the error itself still propagates separately).
            CellValue::Error(_) => false,
            // Any custom value is truthy — handlers that need nuance can
            // intercept comparisons via `CustomTypeHandler::compare`.
            CellValue::Custom(_) => true,
        }
    }

    /// True if this cell holds a [`crate::FormulaError`].
    pub fn is_error(&self) -> bool {
        matches!(self, CellValue::Error(_))
    }
}

/// Low-level stringification with **no registry knowledge**.
///
/// - `Number` → standard `f64` `Display` (e.g. `42`, `3.14`, `-0.5`).
/// - `String` → the inner text verbatim.
/// - `Boolean` → `"TRUE"` / `"FALSE"` (Excel convention).
/// - `Empty` → empty string.
/// - `Custom(cv)` → `cv.data` verbatim — the handler's structured
///   representation, *not* the user-facing rendering.
///
/// **Use [`crate::Registry::render`] for user-facing display** (CONCAT,
/// grid cell text, JSON output). It dispatches `Custom` values through
/// the registered [`crate::custom::CustomTypeHandler::display`] and
/// falls back to this impl for the built-in variants. `CellValue`'s own
/// `Display` is intentionally registry-free so it can be used in
/// contexts (logging, debug output, error messages) that don't have a
/// registry handle.
impl std::fmt::Display for CellValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CellValue::Number(n) => write!(f, "{n}"),
            CellValue::String(s) => write!(f, "{s}"),
            CellValue::Boolean(b) => write!(f, "{}", if *b { "TRUE" } else { "FALSE" }),
            CellValue::Empty => write!(f, ""),
            // Delegates to FormulaError's Display, which reproduces the
            // legacy "#FOO!" sentinel strings byte-for-byte.
            CellValue::Error(e) => write!(f, "{e}"),
            CellValue::Custom(cv) => write!(f, "{}", cv.data),
        }
    }
}

/// A cell ID like "A1", "B2", "AA10".
pub type CellId = String;

/// Hard cap on how many cells a single range can expand to.
/// Protects against `=A1:A9999999`-style typos that would otherwise
/// allocate millions of cell IDs and freeze the browser tab.
pub const MAX_RANGE_CELLS: usize = 100_000;

/// A rectangular 2-D block of cell values produced by a single formula
/// evaluation. Row-major: `data[r * cols + c]` is the value at (r, c).
///
/// All formula evaluation returns an `ArrayValue` internally — a scalar
/// is just a 1×1 array. `Sheet::get` collapses to the top-left element
/// for backwards compatibility; `get_array` exposes the full shape.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayValue {
    pub rows: u32,
    pub cols: u32,
    pub data: Vec<CellValue>,
}

impl ArrayValue {
    /// Wrap a single value in a 1×1 array.
    #[must_use]
    pub fn scalar(v: CellValue) -> Self {
        ArrayValue { rows: 1, cols: 1, data: vec![v] }
    }

    /// New array from row-major `data`.
    ///
    /// # Panics
    /// Panics if `data.len() != rows * cols`.
    #[must_use]
    pub fn new(rows: u32, cols: u32, data: Vec<CellValue>) -> Self {
        assert_eq!(data.len(), (rows as usize) * (cols as usize), "shape/data mismatch");
        ArrayValue { rows, cols, data }
    }

    pub fn shape(&self) -> (u32, u32) {
        (self.rows, self.cols)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn is_scalar(&self) -> bool {
        self.rows == 1 && self.cols == 1
    }

    /// Get the element at `(row, col)`. Returns [`CellValue::Empty`] out of bounds.
    pub fn get(&self, row: u32, col: u32) -> &CellValue {
        if row >= self.rows || col >= self.cols {
            return &CellValue::Empty;
        }
        &self.data[(row * self.cols + col) as usize]
    }

    /// Top-left element (or `Empty` for a 0-size array).
    pub fn first(&self) -> CellValue {
        self.data.first().cloned().unwrap_or(CellValue::Empty)
    }

    /// Flattened iterator in row-major order.
    pub fn iter(&self) -> impl Iterator<Item = &CellValue> {
        self.data.iter()
    }

    /// Consume into a flat `Vec<CellValue>` in row-major order.
    pub fn into_flat(self) -> Vec<CellValue> {
        self.data
    }
}

/// Convert column letters to 1-based index: A=1, Z=26, AA=27. Saturates
/// at `u32::MAX` for inputs longer than the u32 range can represent
/// (8+ letters), so callers passing untrusted input — e.g. lexer
/// tokens like `=AAAAAAAA1` — get a sentinel rather than a panic.
pub fn col_to_index(col: &str) -> u32 {
    let mut index = 0u32;
    for b in col.bytes() {
        index = index
            .saturating_mul(26)
            .saturating_add((b - b'A') as u32 + 1);
    }
    index
}

/// Convert 1-based column index back to letters: 1=A, 26=Z, 27=AA.
pub fn index_to_col(mut index: u32) -> String {
    let mut col = String::new();
    while index > 0 {
        let remainder = (index - 1) % 26;
        col.insert(0, (b'A' + remainder as u8) as char);
        index = (index - 1) / 26;
    }
    col
}

/// Build a CellId from column string and row number.
pub fn cell_id(col: &str, row: u32) -> CellId {
    format!("{col}{row}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_has_1x1_shape() {
        let a = ArrayValue::scalar(CellValue::Number(7.0));
        assert_eq!(a.shape(), (1, 1));
        assert!(a.is_scalar());
        assert_eq!(a.first(), CellValue::Number(7.0));
    }

    #[test]
    fn get_at_row_col() {
        let a = ArrayValue::new(
            2,
            3,
            vec![
                CellValue::Number(1.0), CellValue::Number(2.0), CellValue::Number(3.0),
                CellValue::Number(4.0), CellValue::Number(5.0), CellValue::Number(6.0),
            ],
        );
        assert_eq!(*a.get(0, 0), CellValue::Number(1.0));
        assert_eq!(*a.get(0, 2), CellValue::Number(3.0));
        assert_eq!(*a.get(1, 0), CellValue::Number(4.0));
        assert_eq!(*a.get(1, 2), CellValue::Number(6.0));
    }

    #[test]
    fn get_out_of_bounds_returns_empty() {
        let a = ArrayValue::scalar(CellValue::Number(1.0));
        assert_eq!(*a.get(5, 5), CellValue::Empty);
    }

    #[test]
    #[should_panic(expected = "shape/data mismatch")]
    fn new_panics_on_mismatched_data_length() {
        let _ = ArrayValue::new(2, 3, vec![CellValue::Number(1.0)]);
    }
}
