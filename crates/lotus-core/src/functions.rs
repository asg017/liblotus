use crate::custom::Registry;
use crate::error::FormulaError;
use crate::types::{ArrayValue, CellValue};

/// Function categories — used by clients to group entries in help panels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Math,
    Statistical,
    Logical,
    Text,
    Array,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Math => "Math",
            Category::Statistical => "Statistical",
            Category::Logical => "Logical",
            Category::Text => "Text",
            Category::Array => "Array",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub name: &'static str,
    pub optional: bool,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct Example {
    pub formula: &'static str,
    pub result: &'static str,
}

/// How a function interacts with array inputs.
///
/// - `Aggregate`: flatten all args into a single `Vec<CellValue>` and call
///   the scalar impl. Output is a 1×1 array. Example: `SUM`, `AVERAGE`.
/// - `Elementwise`: output shape is the first arg's shape. The scalar impl
///   is called once per element of the first arg with the remaining args
///   passed through as scalars (top-left of each). Example: `ABS`, `UPPER`.
/// - `ArrayReturning`: the impl handles arrays natively and can produce
///   an output shape unrelated to the inputs. Example: `TRANSPOSE`, `FILTER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnKind {
    Aggregate,
    Elementwise,
    ArrayReturning,
}

/// Implementation pointer. `Scalar` is used for `Aggregate` and `Elementwise`;
/// `Array` is used for `ArrayReturning`.
///
/// Implementations receive `&Registry` so they can stringify / numerically
/// coerce [`CellValue::Custom`] values correctly. Most built-ins ignore it.
#[derive(Clone, Copy)]
pub enum FnImpl {
    Scalar(fn(&[CellValue], &Registry) -> Result<CellValue, FormulaError>),
    Array(fn(&[ArrayValue], &Registry) -> Result<ArrayValue, FormulaError>),
}

/// Static metadata + impl for a single builtin function.
pub struct FunctionDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub category: Category,
    pub params: &'static [Param],
    pub variadic: Option<Param>,
    pub kind: FnKind,
    pub description: &'static str,
    pub examples: &'static [Example],
    pub eval: FnImpl,
}

/// All builtin functions, in display order.
pub static FUNCTIONS: &[FunctionDef] = &[
    FunctionDef {
        name: "SUM",
        aliases: &[],
        category: Category::Math,
        params: &[],
        variadic: Some(Param {
            name: "value",
            optional: false,
            description: "A number, cell reference, or range to add.",
        }),
        kind: FnKind::Aggregate,
        description: "Returns the sum of a series of numbers and/or cells.",
        examples: &[
            Example { formula: "=SUM(1, 2, 3)", result: "6" },
            Example { formula: "=SUM(A1:A10)", result: "(sum of A1 through A10)" },
        ],
        eval: FnImpl::Scalar(fn_sum),
    },
    FunctionDef {
        name: "AVERAGE",
        aliases: &["MEAN", "AVG"],
        category: Category::Statistical,
        params: &[],
        variadic: Some(Param {
            name: "value",
            optional: false,
            description: "A number, cell reference, or range to average.",
        }),
        kind: FnKind::Aggregate,
        description: "Returns the arithmetic mean of the supplied numbers.",
        examples: &[
            Example { formula: "=AVERAGE(2, 4, 6)", result: "4" },
            Example { formula: "=AVERAGE(A1:A10)", result: "(mean of A1:A10)" },
        ],
        eval: FnImpl::Scalar(fn_average),
    },
    FunctionDef {
        name: "MIN",
        aliases: &[],
        category: Category::Statistical,
        params: &[],
        variadic: Some(Param {
            name: "value",
            optional: false,
            description: "A number, cell reference, or range.",
        }),
        kind: FnKind::Aggregate,
        description: "Returns the smallest of the supplied numbers.",
        examples: &[Example { formula: "=MIN(3, 1, 5)", result: "1" }],
        eval: FnImpl::Scalar(fn_min),
    },
    FunctionDef {
        name: "MAX",
        aliases: &[],
        category: Category::Statistical,
        params: &[],
        variadic: Some(Param {
            name: "value",
            optional: false,
            description: "A number, cell reference, or range.",
        }),
        kind: FnKind::Aggregate,
        description: "Returns the largest of the supplied numbers.",
        examples: &[Example { formula: "=MAX(3, 1, 5)", result: "5" }],
        eval: FnImpl::Scalar(fn_max),
    },
    FunctionDef {
        name: "COUNT",
        aliases: &[],
        category: Category::Statistical,
        params: &[],
        variadic: Some(Param {
            name: "value",
            optional: false,
            description: "A value, cell, or range. Non-numeric values are ignored.",
        }),
        kind: FnKind::Aggregate,
        description: "Counts how many of the supplied values are numeric.",
        examples: &[Example { formula: "=COUNT(1, \"x\", 2)", result: "2" }],
        eval: FnImpl::Scalar(fn_count),
    },
    FunctionDef {
        name: "ABS",
        aliases: &[],
        category: Category::Math,
        params: &[Param {
            name: "value",
            optional: false,
            description: "The number to take the absolute value of.",
        }],
        variadic: None,
        kind: FnKind::Elementwise,
        description: "Returns the absolute value of a number. Elementwise over arrays.",
        examples: &[Example { formula: "=ABS(-5)", result: "5" }],
        eval: FnImpl::Scalar(fn_abs),
    },
    FunctionDef {
        name: "ROUND",
        aliases: &[],
        category: Category::Math,
        params: &[
            Param {
                name: "value",
                optional: false,
                description: "The number to round.",
            },
            Param {
                name: "decimals",
                optional: true,
                description: "Number of decimal places (default 0).",
            },
        ],
        variadic: None,
        kind: FnKind::Elementwise,
        description: "Rounds a number to a fixed number of decimal places. Elementwise.",
        examples: &[
            Example { formula: "=ROUND(3.456, 2)", result: "3.46" },
            Example { formula: "=ROUND(3.456)", result: "3" },
        ],
        eval: FnImpl::Scalar(fn_round),
    },
    FunctionDef {
        name: "IF",
        aliases: &[],
        category: Category::Logical,
        params: &[
            Param {
                name: "condition",
                optional: false,
                description: "Truthy: nonzero number or non-empty string. May be an array.",
            },
            Param {
                name: "then_value",
                optional: true,
                description: "Returned when the condition is truthy.",
            },
            Param {
                name: "else_value",
                optional: true,
                description: "Returned when the condition is falsy.",
            },
        ],
        variadic: None,
        kind: FnKind::ArrayReturning,
        description: "Returns one of two values depending on a condition. Broadcasts across arrays.",
        examples: &[
            Example { formula: "=IF(A1>0, \"yes\", \"no\")", result: "\"yes\" or \"no\"" },
        ],
        eval: FnImpl::Array(fn_if_array),
    },
    FunctionDef {
        name: "CONCAT",
        aliases: &[],
        category: Category::Text,
        params: &[],
        variadic: Some(Param {
            name: "value",
            optional: false,
            description: "A value to convert to text and concatenate.",
        }),
        kind: FnKind::Aggregate,
        description: "Concatenates all arguments into a single string.",
        examples: &[
            Example { formula: "=CONCAT(\"hello\", \" \", \"world\")", result: "\"hello world\"" },
        ],
        eval: FnImpl::Scalar(fn_concat),
    },
    FunctionDef {
        name: "LEN",
        aliases: &[],
        category: Category::Text,
        params: &[Param {
            name: "text",
            optional: false,
            description: "The string to measure.",
        }],
        variadic: None,
        kind: FnKind::Elementwise,
        description: "Returns the number of characters in a string. Elementwise.",
        examples: &[Example { formula: "=LEN(\"hello\")", result: "5" }],
        eval: FnImpl::Scalar(fn_len),
    },
    FunctionDef {
        name: "UPPER",
        aliases: &[],
        category: Category::Text,
        params: &[Param {
            name: "text",
            optional: false,
            description: "The string to convert.",
        }],
        variadic: None,
        kind: FnKind::Elementwise,
        description: "Converts a string to upper case. Elementwise.",
        examples: &[Example { formula: "=UPPER(\"hello\")", result: "\"HELLO\"" }],
        eval: FnImpl::Scalar(fn_upper),
    },
    FunctionDef {
        name: "LOWER",
        aliases: &[],
        category: Category::Text,
        params: &[Param {
            name: "text",
            optional: false,
            description: "The string to convert.",
        }],
        variadic: None,
        kind: FnKind::Elementwise,
        description: "Converts a string to lower case. Elementwise.",
        examples: &[Example { formula: "=LOWER(\"HELLO\")", result: "\"hello\"" }],
        eval: FnImpl::Scalar(fn_lower),
    },
    FunctionDef {
        name: "SEQUENCE",
        aliases: &[],
        category: Category::Array,
        params: &[
            Param { name: "rows", optional: false, description: "Number of rows to generate." },
            Param { name: "cols", optional: true, description: "Number of columns (default 1)." },
            Param { name: "start", optional: true, description: "Starting value (default 1)." },
            Param { name: "step", optional: true, description: "Increment (default 1)." },
        ],
        variadic: None,
        kind: FnKind::ArrayReturning,
        description: "Generates a rows×cols array of sequential numbers.",
        examples: &[
            Example { formula: "=SEQUENCE(5)", result: "{1; 2; 3; 4; 5}" },
            Example { formula: "=SEQUENCE(2, 3)", result: "{1, 2, 3; 4, 5, 6}" },
            Example { formula: "=SEQUENCE(3, 1, 10, 2)", result: "{10; 12; 14}" },
        ],
        eval: FnImpl::Array(fn_sequence),
    },
    FunctionDef {
        name: "TRANSPOSE",
        aliases: &[],
        category: Category::Array,
        params: &[Param { name: "array", optional: false, description: "Array to transpose." }],
        variadic: None,
        kind: FnKind::ArrayReturning,
        description: "Swaps the rows and columns of an array.",
        examples: &[
            Example { formula: "=TRANSPOSE(A1:C1)", result: "(column of 3 values)" },
        ],
        eval: FnImpl::Array(fn_transpose),
    },
    FunctionDef {
        name: "FILTER",
        aliases: &[],
        category: Category::Array,
        params: &[
            Param { name: "array", optional: false, description: "Array to filter." },
            Param {
                name: "condition",
                optional: false,
                description: "Same-shape boolean array. Rows (or cells) where condition is truthy survive.",
            },
        ],
        variadic: None,
        kind: FnKind::ArrayReturning,
        description: "Returns the rows of `array` where the matching element of `condition` is truthy.",
        examples: &[
            Example { formula: "=FILTER(A1:A5, A1:A5>2)", result: "(values greater than 2)" },
        ],
        eval: FnImpl::Array(fn_filter),
    },
    FunctionDef {
        name: "SORT",
        aliases: &[],
        category: Category::Array,
        params: &[
            Param { name: "array", optional: false, description: "Array to sort." },
            Param {
                name: "sort_col",
                optional: true,
                description: "1-based column index to sort by (default 1).",
            },
            Param {
                name: "ascending",
                optional: true,
                description: "TRUE (nonzero) for ascending (default), FALSE (zero) for descending.",
            },
        ],
        variadic: None,
        kind: FnKind::ArrayReturning,
        description: "Returns the array sorted by the given column.",
        examples: &[
            Example { formula: "=SORT(A1:A5)", result: "(sorted ascending)" },
            Example { formula: "=SORT(A1:B5, 2, 0)", result: "(sort by col 2 descending)" },
        ],
        eval: FnImpl::Array(fn_sort),
    },
    FunctionDef {
        name: "UNIQUE",
        aliases: &[],
        category: Category::Array,
        params: &[Param {
            name: "array",
            optional: false,
            description: "Array whose unique rows (or values) to return, in first-seen order.",
        }],
        variadic: None,
        kind: FnKind::ArrayReturning,
        description: "Removes duplicate rows (or values) from an array.",
        examples: &[
            Example { formula: "=UNIQUE(A1:A10)", result: "(unique values in first-seen order)" },
        ],
        eval: FnImpl::Array(fn_unique),
    },
];

/// Look up a function by name (case-sensitive — pass uppercase). Aliases match.
pub fn lookup(name: &str) -> Option<&'static FunctionDef> {
    FUNCTIONS
        .iter()
        .find(|d| d.name == name || d.aliases.contains(&name))
}

/// True if `name` (uppercased) is the name of a built-in function.
/// Used by `Sheet::set_name` to reject named ranges that would shadow a builtin.
pub fn is_builtin_function(name: &str) -> bool {
    lookup(name).is_some()
}

/// Dispatch a builtin call, honoring its [`FnKind`].
///
/// - `Aggregate`: flatten every arg's cells into one `Vec`, call the scalar
///   impl, wrap the result in a 1×1 array.
/// - `Elementwise`: iterate the first arg's shape. For each cell, call the
///   scalar impl with `[cell, first(arg1), first(arg2), …]`. Per-element
///   errors are stamped in-place rather than aborting the whole call.
/// - `ArrayReturning`: pass all args through to the array impl.
pub fn call_function(
    name: &str,
    args: &[ArrayValue],
    reg: &Registry,
) -> Result<ArrayValue, FormulaError> {
    // Registry rejects collisions with built-in names at register time,
    // so this lookup can never shadow a built-in.
    if let Some(func) = reg.lookup_function(name) {
        let flat: Vec<CellValue> = args.iter().flat_map(|a| a.iter().cloned()).collect();
        // Custom functions still produce String errors per their public
        // trait — convert to FormulaError at the boundary.
        return func
            .call(&flat)
            .map(ArrayValue::scalar)
            .map_err(FormulaError::from);
    }

    let def = lookup(name).ok_or(FormulaError::Name)?;
    match def.kind {
        FnKind::Aggregate => {
            let flat: Vec<CellValue> = args.iter().flat_map(|a| a.iter().cloned()).collect();
            let FnImpl::Scalar(f) = def.eval else {
                return Err(FormulaError::Custom(format!(
                    "{name}: Aggregate requires Scalar eval"
                )));
            };
            f(&flat, reg).map(ArrayValue::scalar)
        }
        FnKind::Elementwise => {
            if args.is_empty() {
                return Ok(ArrayValue::scalar(CellValue::Empty));
            }
            let first = &args[0];
            let extras: Vec<CellValue> = args[1..].iter().map(|a| a.first()).collect();
            let FnImpl::Scalar(f) = def.eval else {
                return Err(FormulaError::Custom(format!(
                    "{name}: Elementwise requires Scalar eval"
                )));
            };
            let mut data = Vec::with_capacity(first.len());
            let mut scalar_err: Option<FormulaError> = None;
            let single = first.is_scalar();
            for cell in first.iter() {
                let mut call_args = Vec::with_capacity(extras.len() + 1);
                call_args.push(cell.clone());
                call_args.extend(extras.iter().cloned());
                match f(&call_args, reg) {
                    Ok(v) => data.push(v),
                    Err(e) => {
                        if single && scalar_err.is_none() {
                            scalar_err = Some(e.clone());
                        }
                        data.push(CellValue::Error(e));
                    }
                }
            }
            if let Some(e) = scalar_err {
                return Err(e);
            }
            Ok(ArrayValue::new(first.rows, first.cols, data))
        }
        FnKind::ArrayReturning => {
            let FnImpl::Array(f) = def.eval else {
                return Err(FormulaError::Custom(format!(
                    "{name}: ArrayReturning requires Array eval"
                )));
            };
            f(args, reg)
        }
    }
}

// ── Scalar implementations (used by Aggregate + Elementwise) ──

fn fn_sum(args: &[CellValue], reg: &Registry) -> Result<CellValue, FormulaError> {
    Ok(CellValue::Number(get_numbers(args, reg).iter().sum()))
}

fn fn_average(args: &[CellValue], reg: &Registry) -> Result<CellValue, FormulaError> {
    let nums = get_numbers(args, reg);
    if nums.is_empty() {
        Ok(CellValue::Empty)
    } else {
        let sum: f64 = nums.iter().sum();
        Ok(CellValue::Number(sum / nums.len() as f64))
    }
}

fn fn_min(args: &[CellValue], reg: &Registry) -> Result<CellValue, FormulaError> {
    let nums = get_numbers(args, reg);
    if nums.is_empty() {
        Ok(CellValue::Empty)
    } else {
        Ok(CellValue::Number(
            nums.iter().copied().fold(f64::INFINITY, f64::min),
        ))
    }
}

fn fn_max(args: &[CellValue], reg: &Registry) -> Result<CellValue, FormulaError> {
    let nums = get_numbers(args, reg);
    if nums.is_empty() {
        Ok(CellValue::Empty)
    } else {
        Ok(CellValue::Number(
            nums.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ))
    }
}

fn fn_count(args: &[CellValue], reg: &Registry) -> Result<CellValue, FormulaError> {
    Ok(CellValue::Number(get_numbers(args, reg).len() as f64))
}

fn fn_abs(args: &[CellValue], reg: &Registry) -> Result<CellValue, FormulaError> {
    // Elementwise: called per cell, so args is [cell_value].
    let nums = get_numbers(args, reg);
    if nums.is_empty() {
        Ok(CellValue::Empty)
    } else {
        Ok(CellValue::Number(nums[0].abs()))
    }
}

fn fn_round(args: &[CellValue], reg: &Registry) -> Result<CellValue, FormulaError> {
    let nums = get_numbers(args, reg);
    if nums.is_empty() {
        Ok(CellValue::Empty)
    } else {
        let value = nums[0];
        let decimals = nums.get(1).copied().unwrap_or(0.0) as i32;
        let factor = 10f64.powi(decimals);
        Ok(CellValue::Number((value * factor).round() / factor))
    }
}

fn fn_concat(args: &[CellValue], reg: &Registry) -> Result<CellValue, FormulaError> {
    let result: String = args.iter().map(|v| reg.render(v)).collect();
    Ok(CellValue::String(result))
}

fn fn_len(args: &[CellValue], reg: &Registry) -> Result<CellValue, FormulaError> {
    let s = args.first().map(|v| reg.render(v)).unwrap_or_default();
    Ok(CellValue::Number(s.len() as f64))
}

fn fn_upper(args: &[CellValue], reg: &Registry) -> Result<CellValue, FormulaError> {
    let s = args.first().map(|v| reg.render(v)).unwrap_or_default();
    Ok(CellValue::String(s.to_uppercase()))
}

fn fn_lower(args: &[CellValue], reg: &Registry) -> Result<CellValue, FormulaError> {
    let s = args.first().map(|v| reg.render(v)).unwrap_or_default();
    Ok(CellValue::String(s.to_lowercase()))
}

// ── Array implementations ─────────────────────────────────────────────

fn fn_if_array(args: &[ArrayValue], _reg: &Registry) -> Result<ArrayValue, FormulaError> {
    // Broadcast condition + then + else over a common output shape.
    let empty = ArrayValue::scalar(CellValue::Empty);
    let cond = args.first().unwrap_or(&empty);
    let then_v = args.get(1).unwrap_or(&empty);
    let else_v = args.get(2).unwrap_or(&empty);

    let out_rows = max3(cond.rows, then_v.rows, else_v.rows);
    let out_cols = max3(cond.cols, then_v.cols, else_v.cols);
    // Compatibility: each dim must be either equal to out-dim or 1.
    for (got, dim) in [
        (cond.rows, "rows"),
        (then_v.rows, "rows"),
        (else_v.rows, "rows"),
    ] {
        if got != 1 && got != out_rows {
            return Err(FormulaError::value(format!("IF: incompatible {dim}")));
        }
    }
    for got in [cond.cols, then_v.cols, else_v.cols] {
        if got != 1 && got != out_cols {
            return Err(FormulaError::value("IF: incompatible cols"));
        }
    }

    let mut data = Vec::with_capacity((out_rows as usize) * (out_cols as usize));
    for r in 0..out_rows {
        for c in 0..out_cols {
            let cr = if cond.rows == 1 { 0 } else { r };
            let cc = if cond.cols == 1 { 0 } else { c };
            let tr = if then_v.rows == 1 { 0 } else { r };
            let tc = if then_v.cols == 1 { 0 } else { c };
            let er = if else_v.rows == 1 { 0 } else { r };
            let ec = if else_v.cols == 1 { 0 } else { c };
            let condition = cond.get(cr, cc).is_truthy();
            data.push(if condition {
                then_v.get(tr, tc).clone()
            } else {
                else_v.get(er, ec).clone()
            });
        }
    }
    Ok(ArrayValue::new(out_rows, out_cols, data))
}

fn fn_sequence(args: &[ArrayValue], _reg: &Registry) -> Result<ArrayValue, FormulaError> {
    let rows = args.first().map(|a| a.first()).and_then(|v| v.as_number())
        .ok_or_else(|| FormulaError::value("SEQUENCE: rows required"))?;
    let cols = args.get(1).map(|a| a.first()).and_then(|v| v.as_number()).unwrap_or(1.0);
    let start = args.get(2).map(|a| a.first()).and_then(|v| v.as_number()).unwrap_or(1.0);
    let step = args.get(3).map(|a| a.first()).and_then(|v| v.as_number()).unwrap_or(1.0);

    if rows < 1.0 || cols < 1.0 {
        return Err(FormulaError::value("SEQUENCE: rows and cols must be >= 1"));
    }
    let rows_u = rows as u32;
    let cols_u = cols as u32;
    let total = (rows_u as usize).saturating_mul(cols_u as usize);
    if total > crate::types::MAX_RANGE_CELLS {
        return Err(FormulaError::size(format!(
            "SEQUENCE exceeds {} cells",
            crate::types::MAX_RANGE_CELLS
        )));
    }

    let mut data = Vec::with_capacity(total);
    for i in 0..total {
        data.push(CellValue::Number(start + (i as f64) * step));
    }
    Ok(ArrayValue::new(rows_u, cols_u, data))
}

fn fn_transpose(args: &[ArrayValue], _reg: &Registry) -> Result<ArrayValue, FormulaError> {
    let a = args.first().ok_or_else(|| FormulaError::value("TRANSPOSE: array required"))?;
    let rows = a.cols;
    let cols = a.rows;
    let mut data = Vec::with_capacity(a.len());
    for r in 0..rows {
        for c in 0..cols {
            data.push(a.get(c, r).clone());
        }
    }
    Ok(ArrayValue::new(rows, cols, data))
}

fn fn_filter(args: &[ArrayValue], _reg: &Registry) -> Result<ArrayValue, FormulaError> {
    let a = args.first().ok_or_else(|| FormulaError::value("FILTER: array required"))?;
    let cond = args.get(1).ok_or_else(|| FormulaError::value("FILTER: condition required"))?;

    // Condition may be either (rows, cols) matching `a`, or a column vector
    // (rows, 1) selecting whole rows. Everything else is an error.
    let row_wise = cond.rows == a.rows && cond.cols == 1;
    let elementwise = cond.rows == a.rows && cond.cols == a.cols;
    if !row_wise && !elementwise {
        return Err(FormulaError::value(
            "FILTER: condition shape must match array or be a column vector",
        ));
    }

    if row_wise {
        let mut data = Vec::new();
        let mut kept = 0u32;
        for r in 0..a.rows {
            if cond.get(r, 0).is_truthy() {
                for c in 0..a.cols {
                    data.push(a.get(r, c).clone());
                }
                kept += 1;
            }
        }
        if kept == 0 {
            return Ok(ArrayValue::scalar(CellValue::Error(FormulaError::NotAvailable)));
        }
        Ok(ArrayValue::new(kept, a.cols, data))
    } else {
        // Elementwise: keep only cells where the condition is truthy,
        // collapsed into a column vector (first-truthy-first order).
        let mut data = Vec::new();
        for r in 0..a.rows {
            for c in 0..a.cols {
                if cond.get(r, c).is_truthy() {
                    data.push(a.get(r, c).clone());
                }
            }
        }
        if data.is_empty() {
            return Ok(ArrayValue::scalar(CellValue::Error(FormulaError::NotAvailable)));
        }
        let rows = data.len() as u32;
        Ok(ArrayValue::new(rows, 1, data))
    }
}

fn fn_sort(args: &[ArrayValue], _reg: &Registry) -> Result<ArrayValue, FormulaError> {
    let a = args.first().ok_or_else(|| FormulaError::value("SORT: array required"))?;
    let sort_col_1b = args
        .get(1)
        .map(|x| x.first())
        .and_then(|v| v.as_number())
        .unwrap_or(1.0) as u32;
    if sort_col_1b < 1 || sort_col_1b > a.cols {
        return Err(FormulaError::value("SORT: sort_col out of range"));
    }
    let ascending = args
        .get(2)
        .map(|x| x.first())
        .map(|v| v.is_truthy())
        .unwrap_or(true);

    // Build row indices and sort by the chosen column.
    let mut indices: Vec<u32> = (0..a.rows).collect();
    indices.sort_by(|&i, &j| {
        let vi = a.get(i, sort_col_1b - 1);
        let vj = a.get(j, sort_col_1b - 1);
        let ord = compare_cell_values(vi, vj);
        if ascending { ord } else { ord.reverse() }
    });

    let mut data = Vec::with_capacity(a.len());
    for i in indices {
        for c in 0..a.cols {
            data.push(a.get(i, c).clone());
        }
    }
    Ok(ArrayValue::new(a.rows, a.cols, data))
}

fn fn_unique(args: &[ArrayValue], _reg: &Registry) -> Result<ArrayValue, FormulaError> {
    let a = args.first().ok_or_else(|| FormulaError::value("UNIQUE: array required"))?;
    let mut seen: Vec<Vec<CellValue>> = Vec::new();
    let mut out_rows: Vec<Vec<CellValue>> = Vec::new();
    for r in 0..a.rows {
        let mut row = Vec::with_capacity(a.cols as usize);
        for c in 0..a.cols {
            row.push(a.get(r, c).clone());
        }
        if !seen.iter().any(|existing| existing == &row) {
            seen.push(row.clone());
            out_rows.push(row);
        }
    }
    let rows = out_rows.len() as u32;
    let cols = a.cols;
    let data: Vec<CellValue> = out_rows.into_iter().flatten().collect();
    Ok(ArrayValue::new(rows, cols, data))
}

// ── helpers ───────────────────────────────────────────────────────────

/// Extract all numeric values from args (skipping non-numbers).
/// Custom values are coerced through the registry.
fn get_numbers(args: &[CellValue], reg: &Registry) -> Vec<f64> {
    args.iter().filter_map(|v| reg.coerce_number(v)).collect()
}

fn max3(a: u32, b: u32, c: u32) -> u32 {
    a.max(b).max(c)
}

/// Total order over CellValue for sorting.
/// Numbers (including booleans, coerced to 0/1) < strings < empty < custom
/// (by tag, then data). Stable within each. Note: sort doesn't consult
/// registry handlers — custom types always land after the built-in value
/// kinds, in tag/data order. Handlers that want custom ordering in SORT
/// should expose `as_number` so their values appear in the Number band.
fn compare_cell_values(a: &CellValue, b: &CellValue) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let bf = |b: bool| if b { 1.0_f64 } else { 0.0 };
    match (a, b) {
        (CellValue::Number(x), CellValue::Number(y)) => {
            x.partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (CellValue::Boolean(x), CellValue::Boolean(y)) => x.cmp(y),
        (CellValue::Number(x), CellValue::Boolean(y)) => {
            x.partial_cmp(&bf(*y)).unwrap_or(Ordering::Equal)
        }
        (CellValue::Boolean(x), CellValue::Number(y)) => {
            bf(*x).partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (CellValue::String(x), CellValue::String(y)) => x.cmp(y),
        (CellValue::Empty, CellValue::Empty) => Ordering::Equal,
        (CellValue::Custom(x), CellValue::Custom(y)) => x
            .type_tag
            .cmp(&y.type_tag)
            .then_with(|| x.data.cmp(&y.data)),
        // Errors compare by their Display string for stable, total ordering
        // within the Error band.
        (CellValue::Error(x), CellValue::Error(y)) => x.to_string().cmp(&y.to_string()),
        // Cross-band ordering: Number/Boolean < String < Empty < Error < Custom.
        (CellValue::Number(_) | CellValue::Boolean(_), _) => Ordering::Less,
        (_, CellValue::Number(_) | CellValue::Boolean(_)) => Ordering::Greater,
        (CellValue::String(_), _) => Ordering::Less,
        (_, CellValue::String(_)) => Ordering::Greater,
        (CellValue::Empty, _) => Ordering::Less,
        (_, CellValue::Empty) => Ordering::Greater,
        (CellValue::Error(_), _) => Ordering::Less,
        (_, CellValue::Error(_)) => Ordering::Greater,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: f64) -> CellValue {
        CellValue::Number(v)
    }
    fn s(v: &str) -> CellValue {
        CellValue::String(v.into())
    }
    fn scalar(v: CellValue) -> ArrayValue {
        ArrayValue::scalar(v)
    }
    /// Shim that calls `call_function` with an empty registry — every
    /// built-in test runs in the no-custom-types baseline.
    fn cf(name: &str, args: &[ArrayValue]) -> Result<ArrayValue, FormulaError> {
        call_function(name, args, &Registry::default())
    }

    // ── Aggregate kind ──

    #[test]
    fn test_sum() {
        let r = cf("SUM", &[scalar(n(1.0)), scalar(n(2.0)), scalar(n(3.0))]).unwrap();
        assert_eq!(r.first(), n(6.0));
        assert_eq!(r.shape(), (1, 1));
    }

    #[test]
    fn test_sum_with_range_arg() {
        let arr = ArrayValue::new(3, 1, vec![n(1.0), n(2.0), n(3.0)]);
        assert_eq!(cf("SUM", &[arr]).unwrap().first(), n(6.0));
    }

    #[test]
    fn test_average_aliases() {
        let a = [scalar(n(2.0)), scalar(n(4.0))];
        assert_eq!(cf("AVERAGE", &a).unwrap().first(), n(3.0));
        assert_eq!(cf("MEAN", &a).unwrap().first(), n(3.0));
        assert_eq!(cf("AVG", &a).unwrap().first(), n(3.0));
    }

    #[test]
    fn test_min_max() {
        let a = [scalar(n(3.0)), scalar(n(1.0)), scalar(n(5.0))];
        assert_eq!(cf("MIN", &a).unwrap().first(), n(1.0));
        assert_eq!(cf("MAX", &a).unwrap().first(), n(5.0));
    }

    #[test]
    fn test_count_ignores_strings() {
        let a = [scalar(n(1.0)), scalar(s("x")), scalar(n(2.0))];
        assert_eq!(cf("COUNT", &a).unwrap().first(), n(2.0));
    }

    // ── Elementwise kind ──

    #[test]
    fn test_abs_scalar() {
        assert_eq!(cf("ABS", &[scalar(n(-5.0))]).unwrap().first(), n(5.0));
    }

    #[test]
    fn test_abs_elementwise() {
        let arr = ArrayValue::new(3, 1, vec![n(-1.0), n(2.0), n(-3.0)]);
        let r = cf("ABS", &[arr]).unwrap();
        assert_eq!(r.shape(), (3, 1));
        assert_eq!(*r.get(0, 0), n(1.0));
        assert_eq!(*r.get(1, 0), n(2.0));
        assert_eq!(*r.get(2, 0), n(3.0));
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_round_elementwise_preserves_shape() {
        let arr = ArrayValue::new(2, 2, vec![n(1.234), n(2.567), n(3.14), n(4.999)]);
        let decimals = scalar(n(1.0));
        let r = cf("ROUND", &[arr, decimals]).unwrap();
        assert_eq!(r.shape(), (2, 2));
        assert_eq!(*r.get(0, 0), n(1.2));
        assert_eq!(*r.get(0, 1), n(2.6));
        assert_eq!(*r.get(1, 0), n(3.1));
        assert_eq!(*r.get(1, 1), n(5.0));
    }

    #[test]
    fn test_upper_elementwise() {
        let arr = ArrayValue::new(1, 3, vec![s("foo"), s("bar"), s("baz")]);
        let r = cf("UPPER", &[arr]).unwrap();
        assert_eq!(r.shape(), (1, 3));
        assert_eq!(*r.get(0, 0), s("FOO"));
        assert_eq!(*r.get(0, 2), s("BAZ"));
    }

    #[test]
    fn test_len_elementwise() {
        let arr = ArrayValue::new(2, 1, vec![s("hi"), s("world")]);
        let r = cf("LEN", &[arr]).unwrap();
        assert_eq!(*r.get(0, 0), n(2.0));
        assert_eq!(*r.get(1, 0), n(5.0));
    }

    // ── Array-returning ──

    #[test]
    fn test_sequence_default_single_column() {
        let r = cf("SEQUENCE", &[scalar(n(5.0))]).unwrap();
        assert_eq!(r.shape(), (5, 1));
        assert_eq!(*r.get(0, 0), n(1.0));
        assert_eq!(*r.get(4, 0), n(5.0));
    }

    #[test]
    fn test_sequence_rows_and_cols() {
        let r = cf("SEQUENCE", &[scalar(n(2.0)), scalar(n(3.0))]).unwrap();
        assert_eq!(r.shape(), (2, 3));
        assert_eq!(*r.get(0, 0), n(1.0));
        assert_eq!(*r.get(0, 2), n(3.0));
        assert_eq!(*r.get(1, 0), n(4.0));
        assert_eq!(*r.get(1, 2), n(6.0));
    }

    #[test]
    fn test_sequence_start_and_step() {
        let r = cf(
            "SEQUENCE",
            &[scalar(n(3.0)), scalar(n(1.0)), scalar(n(10.0)), scalar(n(2.0))],
        ).unwrap();
        assert_eq!(r.shape(), (3, 1));
        assert_eq!(*r.get(0, 0), n(10.0));
        assert_eq!(*r.get(1, 0), n(12.0));
        assert_eq!(*r.get(2, 0), n(14.0));
    }

    #[test]
    fn test_sequence_rejects_zero_rows() {
        let err = cf("SEQUENCE", &[scalar(n(0.0))]).unwrap_err();
        assert!(matches!(err, FormulaError::Value(_)), "got: {err:?}");
    }

    #[test]
    fn test_transpose_row_to_column() {
        let arr = ArrayValue::new(1, 3, vec![n(1.0), n(2.0), n(3.0)]);
        let r = cf("TRANSPOSE", &[arr]).unwrap();
        assert_eq!(r.shape(), (3, 1));
        assert_eq!(*r.get(0, 0), n(1.0));
        assert_eq!(*r.get(2, 0), n(3.0));
    }

    #[test]
    fn test_transpose_roundtrip() {
        let orig = ArrayValue::new(2, 3, vec![n(1.0), n(2.0), n(3.0), n(4.0), n(5.0), n(6.0)]);
        let t = cf("TRANSPOSE", std::slice::from_ref(&orig)).unwrap();
        let back = cf("TRANSPOSE", &[t]).unwrap();
        assert_eq!(back, orig);
    }

    #[test]
    fn test_filter_row_vector_condition() {
        let arr = ArrayValue::new(3, 1, vec![n(1.0), n(2.0), n(3.0)]);
        let cond = ArrayValue::new(3, 1, vec![n(0.0), n(1.0), n(1.0)]);
        let r = cf("FILTER", &[arr, cond]).unwrap();
        assert_eq!(r.shape(), (2, 1));
        assert_eq!(*r.get(0, 0), n(2.0));
        assert_eq!(*r.get(1, 0), n(3.0));
    }

    #[test]
    fn test_filter_nothing_matches_yields_na() {
        let arr = ArrayValue::new(3, 1, vec![n(1.0), n(2.0), n(3.0)]);
        let cond = ArrayValue::new(3, 1, vec![n(0.0), n(0.0), n(0.0)]);
        let r = cf("FILTER", &[arr, cond]).unwrap();
        assert_eq!(r.first(), CellValue::Error(FormulaError::NotAvailable));
    }

    #[test]
    fn test_filter_2d_row_wise() {
        let arr = ArrayValue::new(3, 2, vec![
            n(1.0), s("a"),
            n(2.0), s("b"),
            n(3.0), s("c"),
        ]);
        let cond = ArrayValue::new(3, 1, vec![n(0.0), n(1.0), n(1.0)]);
        let r = cf("FILTER", &[arr, cond]).unwrap();
        assert_eq!(r.shape(), (2, 2));
        assert_eq!(*r.get(0, 0), n(2.0));
        assert_eq!(*r.get(0, 1), s("b"));
        assert_eq!(*r.get(1, 0), n(3.0));
    }

    #[test]
    fn test_filter_shape_mismatch_errors() {
        let arr = ArrayValue::new(3, 1, vec![n(1.0), n(2.0), n(3.0)]);
        let cond = ArrayValue::new(2, 1, vec![n(1.0), n(1.0)]);
        assert!(matches!(
            cf("FILTER", &[arr, cond]).unwrap_err(),
            FormulaError::Value(_)
        ));
    }

    #[test]
    fn test_sort_ascending_default() {
        let arr = ArrayValue::new(3, 1, vec![n(3.0), n(1.0), n(2.0)]);
        let r = cf("SORT", &[arr]).unwrap();
        assert_eq!(*r.get(0, 0), n(1.0));
        assert_eq!(*r.get(1, 0), n(2.0));
        assert_eq!(*r.get(2, 0), n(3.0));
    }

    #[test]
    fn test_sort_descending() {
        let arr = ArrayValue::new(3, 1, vec![n(3.0), n(1.0), n(2.0)]);
        let r = cf("SORT", &[arr, scalar(n(1.0)), scalar(n(0.0))]).unwrap();
        assert_eq!(*r.get(0, 0), n(3.0));
        assert_eq!(*r.get(2, 0), n(1.0));
    }

    #[test]
    fn test_sort_by_column() {
        // 3×2, sorted by column 2 (1-based).
        let arr = ArrayValue::new(3, 2, vec![
            s("a"), n(3.0),
            s("b"), n(1.0),
            s("c"), n(2.0),
        ]);
        let r = cf("SORT", &[arr, scalar(n(2.0))]).unwrap();
        assert_eq!(r.shape(), (3, 2));
        // Rows reordered by col 2 ascending: (b,1), (c,2), (a,3)
        assert_eq!(*r.get(0, 0), s("b"));
        assert_eq!(*r.get(1, 0), s("c"));
        assert_eq!(*r.get(2, 0), s("a"));
    }

    #[test]
    fn test_sort_out_of_range_column_errors() {
        let arr = ArrayValue::new(3, 2, vec![n(1.0), n(2.0), n(3.0), n(4.0), n(5.0), n(6.0)]);
        assert!(matches!(
            cf("SORT", &[arr, scalar(n(5.0))]).unwrap_err(),
            FormulaError::Value(_)
        ));
    }

    #[test]
    fn test_unique_preserves_order() {
        let arr = ArrayValue::new(5, 1, vec![n(3.0), n(1.0), n(3.0), n(2.0), n(1.0)]);
        let r = cf("UNIQUE", &[arr]).unwrap();
        assert_eq!(r.shape(), (3, 1));
        assert_eq!(*r.get(0, 0), n(3.0));
        assert_eq!(*r.get(1, 0), n(1.0));
        assert_eq!(*r.get(2, 0), n(2.0));
    }

    #[test]
    fn test_unique_2d_by_row() {
        // Two duplicate rows; unique should strip the repeat.
        let arr = ArrayValue::new(3, 2, vec![
            n(1.0), s("a"),
            n(2.0), s("b"),
            n(1.0), s("a"),
        ]);
        let r = cf("UNIQUE", &[arr]).unwrap();
        assert_eq!(r.shape(), (2, 2));
    }

    // ── IF ──

    #[test]
    fn test_if_scalar_truthy() {
        assert_eq!(
            cf("IF", &[scalar(n(1.0)), scalar(s("yes")), scalar(s("no"))]).unwrap().first(),
            s("yes"),
        );
    }

    #[test]
    fn test_if_scalar_falsy() {
        assert_eq!(
            cf("IF", &[scalar(n(0.0)), scalar(s("yes")), scalar(s("no"))]).unwrap().first(),
            s("no"),
        );
    }

    #[test]
    fn test_if_empty_condition_is_falsy() {
        assert_eq!(
            cf("IF", &[scalar(CellValue::Empty), scalar(s("yes")), scalar(s("no"))]).unwrap().first(),
            s("no"),
        );
    }

    #[test]
    fn test_if_array_condition_broadcasts() {
        let cond = ArrayValue::new(3, 1, vec![n(1.0), n(0.0), n(1.0)]);
        let r = cf("IF", &[cond, scalar(s("Y")), scalar(s("N"))]).unwrap();
        assert_eq!(r.shape(), (3, 1));
        assert_eq!(*r.get(0, 0), s("Y"));
        assert_eq!(*r.get(1, 0), s("N"));
        assert_eq!(*r.get(2, 0), s("Y"));
    }

    // ── CONCAT ──

    #[test]
    fn test_concat_flattens_arrays() {
        let a = ArrayValue::new(1, 2, vec![s("foo"), s("bar")]);
        let b = scalar(s("!"));
        assert_eq!(
            cf("CONCAT", &[a, b]).unwrap().first(),
            s("foobar!"),
        );
    }

    // ── Dispatch/lookup ──

    #[test]
    fn test_unknown_function() {
        assert!(cf("FOOBAR", &[]).is_err());
    }

    #[test]
    fn lookup_finds_primary_and_aliases() {
        assert_eq!(lookup("SUM").unwrap().name, "SUM");
        assert_eq!(lookup("AVG").unwrap().name, "AVERAGE");
        assert_eq!(lookup("MEAN").unwrap().name, "AVERAGE");
        assert!(lookup("FOOBAR").is_none());
    }

    #[test]
    fn every_function_has_metadata() {
        for def in FUNCTIONS {
            assert!(!def.name.is_empty(), "empty name");
            assert!(!def.description.is_empty(), "{} missing description", def.name);
            assert!(
                !def.params.is_empty() || def.variadic.is_some(),
                "{} has no params and no variadic — looks like a misconfig",
                def.name,
            );
        }
    }
}
