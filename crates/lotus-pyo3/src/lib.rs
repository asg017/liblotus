use std::collections::HashMap;
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use lotus_core::{
    adjust_refs_for_column_block_move as core_adjust_refs_for_column_block_move,
    adjust_refs_for_column_block_move_data_following
        as core_adjust_refs_for_column_block_move_data_following,
    adjust_refs_for_deletion as core_adjust_refs_for_deletion,
    adjust_refs_for_insertion as core_adjust_refs_for_insertion,
    adjust_refs_for_row_block_move as core_adjust_refs_for_row_block_move,
    adjust_refs_for_row_block_move_data_following
        as core_adjust_refs_for_row_block_move_data_following,
    cell_id as core_cell_id, col_to_index as core_col_to_index, complete as core_complete,
    extract_refs as core_extract_refs, formula_tokens as core_formula_tokens,
    index_to_col as core_index_to_col, is_unbounded_range as core_is_unbounded_range,
    list_functions as core_list_functions, parse_cell_id as core_parse_cell_id,
    parse_range as core_parse_range, rewrite_refs_for_deletion as core_rewrite_refs_for_deletion,
    shift_formula_refs as core_shift_formula_refs, signature_help as core_signature_help,
    BinaryOp, CellInput, CellValue, CompareOp, CompletionKind, CompletionList, CustomFunction,
    CustomTypeHandler, CustomValue, Deletion, Evaluator, FunctionInfo, Insertion, ParamInfo,
    RefKind, Sheet as CoreSheet, SignatureHelp, SpillError,
};

/// Python-visible spreadsheet backed by lotus-core.
#[pyclass]
struct Sheet {
    inner: CoreSheet,
}

#[pymethods]
impl Sheet {
    #[new]
    fn new() -> Self {
        Sheet {
            inner: CoreSheet::new(),
        }
    }

    /// Set one or more cells and recalculate.
    /// `changes` is a list of (cell_id, raw_value) tuples.
    fn set_cells(&mut self, changes: Vec<(String, String)>) -> PyResult<()> {
        self.inner
            .set_cells(&changes)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Set one or more cells, with each cell either raw text (engine
    /// parses) or a pre-typed value (engine stores as-is).
    ///
    /// `changes` is a list of `(cell_id, payload)` tuples where each
    /// payload is a dict with a `kind` key:
    ///
    /// ```python
    /// sheet.set_cells_typed([
    ///     ("A1", {"kind": "raw", "value": "=B1+1"}),
    ///     ("B1", {"kind": "number", "value": 42.5}),
    ///     ("C1", {"kind": "string", "value": "2/4"}),    # forces literal text
    ///     ("D1", {"kind": "boolean", "value": True}),
    ///     ("E1", {"kind": "empty"}),                      # delete
    ///     ("F1", {"kind": "custom", "type_tag": "jdate", "data": "2026-04-02"}),
    /// ])
    /// ```
    ///
    /// `kind: "raw"` matches `set_cells` exactly (empty `value` deletes
    /// the cell). The other kinds bypass the parse pipeline; the typed
    /// value persists across recalcs until a `kind: "raw"` write to the
    /// same cell opts back into auto-classification.
    fn set_cells_typed(
        &mut self,
        py: Python<'_>,
        changes: Vec<(String, Py<PyAny>)>,
    ) -> PyResult<()> {
        let mut inputs: Vec<(String, CellInput)> = Vec::with_capacity(changes.len());
        for (cell_id, payload) in changes {
            let bound = payload.bind(py);
            let input = py_to_cell_input(bound).map_err(|e| {
                pyo3::exceptions::PyValueError::new_err(format!("cell {cell_id}: {e}"))
            })?;
            inputs.push((cell_id, input));
        }
        self.inner
            .set_cells_typed(&inputs)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Probe a registered custom-type handler with handler-specific
    /// `options`. Returns `{"type_tag": str, "data": str}` if the
    /// handler claims the input, else `None`.
    ///
    /// Use this to disambiguate ambiguous input — e.g.
    /// `sheet.try_parse("jdate", "4/2", "%d/%m")` asks the date handler
    /// to parse `"4/2"` as a day/month shorthand. Pass the result
    /// straight to `set_cells_typed` to write it.
    fn try_parse(
        &self,
        py: Python<'_>,
        type_tag: &str,
        raw: &str,
        options: &str,
    ) -> PyResult<Option<PyObject>> {
        let Some(cv) = self.inner.try_parse(type_tag, raw, options) else {
            return Ok(None);
        };
        let d = PyDict::new(py);
        d.set_item("type_tag", cv.type_tag)?;
        d.set_item("data", cv.data)?;
        Ok(Some(d.into_any().unbind()))
    }

    /// Get the computed value of a cell as a string, or None if empty.
    fn get(&self, cell_id: &str) -> Option<String> {
        match self.inner.get(cell_id) {
            CellValue::Empty => None,
            v => Some(v.to_string()),
        }
    }

    /// Return all computed values as a dict of {cell_id: value_string}.
    fn get_all(&self) -> HashMap<String, String> {
        self.inner
            .get_all()
            .iter()
            .filter_map(|(id, v)| match v {
                CellValue::Empty => None,
                _ => Some((id.clone(), v.to_string())),
            })
            .collect()
    }

    /// Get the typed computed value: `int`, `float`, `str`, or `None`.
    ///
    /// Numbers that are finite, integral, and within JS-safe-integer range
    /// come back as `int`; everything else as `float`. Strings (including
    /// error values like `"#DIV/0!"`) stay strings. Empty cells → `None`.
    fn get_typed(&self, py: Python<'_>, cell_id: &str) -> PyResult<Option<PyObject>> {
        cell_value_to_py(py, &self.inner.get(cell_id))
    }

    /// Return all computed values as a dict of {cell_id: typed_value}.
    /// See `get_typed` for the type-mapping rules. Empty cells are omitted.
    fn get_all_typed(&self, py: Python<'_>) -> PyResult<HashMap<String, PyObject>> {
        let mut out = HashMap::with_capacity(self.inner.get_all().len());
        for (id, v) in self.inner.get_all() {
            if let Some(obj) = cell_value_to_py(py, v)? {
                out.insert(id.clone(), obj);
            }
        }
        Ok(out)
    }

    /// If `cell_id` is an anchor, return its full array as a nested list
    /// of typed values. Otherwise None. See `get_typed` for type mapping.
    fn get_array_typed(
        &self,
        py: Python<'_>,
        cell_id: &str,
    ) -> PyResult<Option<Vec<Vec<PyObject>>>> {
        let Some(arr) = self.inner.spill_array(cell_id) else {
            return Ok(None);
        };
        let mut rows = Vec::with_capacity(arr.rows as usize);
        for r in 0..arr.rows {
            let mut row = Vec::with_capacity(arr.cols as usize);
            for c in 0..arr.cols {
                let obj = match cell_value_to_py(py, arr.get(r, c))? {
                    Some(o) => o,
                    None => py.None(),
                };
                row.push(obj);
            }
            rows.push(row);
        }
        Ok(Some(rows))
    }

    /// Define (or overwrite) a workbook-global named range and recalculate.
    ///
    /// `definition` is the raw text — `"=B1"`, `"=A1:A10"`,
    /// `"=SUM(A1:A10)"`, or a literal like `"0.05"`. Raises ValueError
    /// for invalid names (cell-ref-shaped, builtin function names,
    /// malformed) or unparseable definitions.
    fn set_name(&mut self, name: &str, definition: &str) -> PyResult<()> {
        self.inner
            .set_name(name, definition)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Remove a named range and recalculate. No-op if the name is undefined.
    fn remove_name(&mut self, name: &str) -> PyResult<()> {
        self.inner
            .remove_name(name)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Look up a name's raw definition, or None if undefined.
    fn get_name(&self, name: &str) -> Option<String> {
        self.inner.get_name(name).map(|s| s.to_string())
    }

    /// Return all defined names as a dict mapping uppercased name → definition.
    /// Iteration order matches the order names were defined.
    fn names(&self) -> HashMap<String, String> {
        self.inner
            .names()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// If `cell_id` is the anchor of a spill region, return a dict
    /// {"rows": int, "cols": int}. Otherwise None.
    fn spill_at(&self, py: Python<'_>, cell_id: &str) -> PyResult<Option<PyObject>> {
        match self.inner.spill_at(cell_id) {
            Some(region) => {
                let dict = PyDict::new(py);
                dict.set_item("rows", region.rows)?;
                dict.set_item("cols", region.cols)?;
                Ok(Some(dict.into()))
            }
            None => Ok(None),
        }
    }

    /// If `cell_id` was populated by a spill from another cell's formula,
    /// return the anchor id. Otherwise None.
    fn owner_of(&self, cell_id: &str) -> Option<String> {
        self.inner.owner_of(cell_id).cloned()
    }

    /// If `cell_id` is an anchor, return its full array as a nested list
    /// of strings (row-major, empty cells as ""). Otherwise None.
    fn get_array(&self, cell_id: &str) -> Option<Vec<Vec<String>>> {
        let arr = self.inner.spill_array(cell_id)?;
        let mut rows = Vec::with_capacity(arr.rows as usize);
        for r in 0..arr.rows {
            let mut row = Vec::with_capacity(arr.cols as usize);
            for c in 0..arr.cols {
                row.push(arr.get(r, c).to_string());
            }
            rows.push(row);
        }
        Some(rows)
    }

    /// Pin a 2-D value at an anchor cell. The pin overrides whatever the
    /// cell's formula would evaluate to and spills like a native array
    /// result (>1×1 → anchor + members, 1×1 → scalar). Blockers flip the
    /// anchor to `#SPILL!`.
    ///
    /// `rows` is a list of same-length lists of strings; every value is
    /// stored as-is (the engine treats all pinned cells as strings).
    /// Raises `ValueError` when the array is empty or ragged.
    fn pin_value(&mut self, cell_id: &str, rows: Vec<Vec<String>>) -> PyResult<()> {
        let converted: Vec<Vec<CellValue>> = rows
            .into_iter()
            .map(|r| r.into_iter().map(CellValue::String).collect())
            .collect();
        self.inner
            .pin_value(cell_id, converted)
            .map_err(|e: SpillError| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Remove a pin. On the next recalc the cell reverts to evaluating
    /// its own formula (or to empty if it has none). No-op if not pinned.
    fn unpin_value(&mut self, cell_id: &str) {
        self.inner.unpin_value(cell_id);
    }

    /// All currently-pinned anchor cell IDs, sorted.
    fn pinned_cells(&self) -> Vec<String> {
        self.inner.pinned_cells()
    }

    /// True if the cell's value comes from a pin rather than its own formula.
    fn is_pinned(&self, cell_id: &str) -> bool {
        self.inner.is_pinned(cell_id)
    }

    /// Register a Python custom-type handler. `handler` is any object
    /// with a `type_tag` string attribute and (optionally) these
    /// callable attributes:
    ///   - `parse_literal(raw: str) -> dict|None`
    ///   - `display(v: dict) -> str`
    ///   - `edit_repr(v: dict) -> str`
    ///   - `binary_op(op: str, lhs, rhs) -> value|None`
    ///   - `compare(op: str, lhs, rhs) -> bool|None`
    ///   - `as_number(v: dict) -> float|None`
    ///
    /// Cell values across the boundary are Python primitives or, for
    /// custom values, dicts `{"type_tag": str, "data": str}`. Raises
    /// ValueError on duplicate `type_tag`.
    fn register_type(&mut self, py: Python<'_>, handler: Py<PyAny>) -> PyResult<()> {
        let h = PyHandler::new(py, handler)?;
        self.inner
            .register_type(Arc::new(h))
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Register a Python function callable as a custom spreadsheet
    /// function. `func` is called with a single argument — a list of
    /// pre-flattened arg values — and should return a value in the
    /// same vocabulary described on `register_type`. Raising an
    /// exception surfaces as a cell error.
    fn register_function(&mut self, name: &str, func: Py<PyAny>) -> PyResult<()> {
        if name.is_empty() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "register_function: name must be non-empty",
            ));
        }
        let f = PyFunc {
            name: name.to_string(),
            func,
        };
        self.inner
            .register_function(Arc::new(f))
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Completion suggestions for the cursor position in `formula`.
    ///
    /// Returns a dict:
    ///   {"replace_start": int, "replace_end": int, "items": [
    ///     {"kind": "function"|"name", "label": str, "insert": str,
    ///      "detail": Optional[str], "documentation": Optional[str]}, ...]}
    fn complete(&self, py: Python<'_>, formula: &str, cursor: usize) -> PyResult<PyObject> {
        let names: Vec<&str> = self.inner.names().keys().map(|s| s.as_str()).collect();
        let list = core_complete(formula, cursor, &names);
        Ok(completion_list_to_py(py, &list)?.into())
    }

    /// Register the bundled `lotus-datetime` extension on this sheet.
    /// Adds the six `j*` datetime cell types (`jdate`, `jtime`,
    /// `jdatetime`, `jzoned`, `jtimezone`, `jspan`) and ~40 formula
    /// functions (`DATE`, `NOW`, `YEAR`, `DAYS_BETWEEN`, …) that operate
    /// on them. Custom values cross the Python boundary as
    /// `{"type_tag": str, "data": str}` dicts.
    ///
    /// Compiled in only when `lotus-pyo3` was built with the `datetime`
    /// cargo feature; otherwise this method is absent. Raises
    /// `ValueError` on duplicate registration.
    #[cfg(feature = "datetime")]
    fn register_datetime(&mut self) -> PyResult<()> {
        lotus_datetime::register_on_sheet(&mut self.inner)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Register the bundled `lotus-url` extension on this sheet. Adds
    /// scalar URL functions (`URL_SCHEME`, `URL_HOST`, `URL_PORT`,
    /// `URL_PATH`, `URL_PATH_SEGMENT`, `URL_QUERY`, `URL_FRAGMENT`,
    /// `URL_PARAM`, `URL_VALID`, `URL_ENCODE`, `URL_DECODE`,
    /// `URL_JOIN`). No new cell types — URLs are plain text.
    ///
    /// Compiled in only when `lotus-pyo3` was built with the `url`
    /// cargo feature; otherwise this method is absent. Raises
    /// `ValueError` on duplicate registration.
    #[cfg(feature = "url")]
    fn register_url(&mut self) -> PyResult<()> {
        lotus_url::register_on_sheet(&mut self.inner)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Metadata for every function this sheet's evaluator can dispatch
    /// on: builtins followed by runtime-registered customs (e.g. anything
    /// added by `register_function`, `register_datetime`, or
    /// `register_url`). Returns a
    /// list of dicts shaped like `lotus.list_functions()`. Custom
    /// functions surface a minimal entry (name + open-ended variadic).
    fn list_functions(&self, py: Python<'_>) -> PyResult<PyObject> {
        let list = PyList::empty(py);
        for info in self.inner.list_functions() {
            list.append(function_info_to_py(py, &info)?)?;
        }
        Ok(list.into())
    }

    /// Cursor-aware signature help that resolves names through both the
    /// builtin table and this sheet's registry, so registered customs
    /// (YEAR, host extensions, …) light up alongside SUM / IF / etc.
    /// Returns `None` when the cursor isn't inside any function-call
    /// parens or the name doesn't resolve.
    fn signature_help(
        &self,
        py: Python<'_>,
        formula: &str,
        cursor: usize,
    ) -> PyResult<Option<PyObject>> {
        match self.inner.signature_help(formula, cursor) {
            Some(sig) => Ok(Some(signature_help_to_py(py, &sig)?.into())),
            None => Ok(None),
        }
    }
}

/// One-shot formula evaluation (no sheet state).
#[pyfunction]
fn evaluate(formula: &str) -> PyResult<Option<String>> {
    let eval = Evaluator::new(|_| CellValue::Empty);
    match eval.evaluate(formula) {
        Ok(CellValue::Empty) => Ok(None),
        Ok(v) => Ok(Some(v.to_string())),
        Err(e) => Err(pyo3::exceptions::PyValueError::new_err(e.to_string())),
    }
}

/// Extract cell references from a formula with source positions.
/// Returns a list of dicts: [{"start": int, "end": int, "text": str,
/// "kind": str, "cells": [str]}] where `kind` is one of
/// "cell", "range", "whole_column", "whole_row", "name".
#[pyfunction]
fn extract_refs(formula: &str) -> Vec<HashMap<String, PyObject>> {
    Python::with_gil(|py| {
        core_extract_refs(formula)
            .into_iter()
            .map(|r| {
                let kind = match r.kind {
                    RefKind::Cell => "cell",
                    RefKind::Range => "range",
                    RefKind::WholeColumn => "whole_column",
                    RefKind::WholeRow => "whole_row",
                    RefKind::Name => "name",
                    RefKind::SpillRef => "spill",
                };
                let mut m = HashMap::new();
                m.insert("start".into(), r.start.into_pyobject(py).unwrap().into_any().unbind());
                m.insert("end".into(), r.end.into_pyobject(py).unwrap().into_any().unbind());
                m.insert("text".into(), r.text.into_pyobject(py).unwrap().into_any().unbind());
                m.insert("kind".into(), kind.into_pyobject(py).unwrap().into_any().unbind());
                // CellCoord → "A1"-style strings at the binding boundary so
                // the Python API surface stays identical.
                let cells: Vec<String> = r
                    .cells
                    .iter()
                    .map(|c| lotus_core::format_cell_id(*c))
                    .collect();
                m.insert("cells".into(), cells.into_pyobject(py).unwrap().into_any().unbind());
                m
            })
            .collect()
    })
}

/// Classify every token of a formula string with its source span.
///
/// Returns a list of dicts: `[{"start": int, "end": int, "kind": str}]`
/// where `kind` is one of `"number"`, `"string"`, `"cell_ref"`, `"range"`,
/// `"name"`, `"function"`, `"operator"`, `"paren"`, `"comma"`,
/// `"whitespace"`, `"unknown"`. Non-formula input (no leading `=`)
/// returns an empty list. Lexer errors produce whatever tokens were
/// recognized plus an `"unknown"` span covering the remainder.
#[pyfunction]
fn formula_tokens(formula: &str) -> Vec<HashMap<String, PyObject>> {
    Python::with_gil(|py| {
        core_formula_tokens(formula)
            .into_iter()
            .map(|t| {
                let mut m = HashMap::new();
                m.insert("start".into(), t.start.into_pyobject(py).unwrap().into_any().unbind());
                m.insert("end".into(), t.end.into_pyobject(py).unwrap().into_any().unbind());
                m.insert(
                    "kind".into(),
                    t.kind.as_str().into_pyobject(py).unwrap().into_any().unbind(),
                );
                m
            })
            .collect()
    })
}

/// Rewrite every ref in `formula` that points into the deleted rows or
/// columns, replacing the affected portion with `#REF!`. Row and column
/// indices are 0-based. Non-formula input passes through unchanged.
#[pyfunction]
#[pyo3(signature = (formula, deleted_cols = None, deleted_rows = None))]
fn rewrite_refs_for_deletion(
    formula: &str,
    deleted_cols: Option<Vec<u32>>,
    deleted_rows: Option<Vec<u32>>,
) -> String {
    let deletion = Deletion {
        cols: deleted_cols.unwrap_or_default(),
        rows: deleted_rows.unwrap_or_default(),
    };
    core_rewrite_refs_for_deletion(formula, &deletion).into_owned()
}

/// Adjust every ref in `formula` to reflect the formula's *new* text
/// after the given rows/cols have been deleted. Surviving refs shift
/// to their new index, ranges get trimmed to the surviving extent,
/// and refs with no survivors become `#REF!`. Row and column indices
/// are 0-based. Non-formula input passes through unchanged.
#[pyfunction]
#[pyo3(signature = (formula, deleted_cols = None, deleted_rows = None))]
fn adjust_refs_for_deletion(
    formula: &str,
    deleted_cols: Option<Vec<u32>>,
    deleted_rows: Option<Vec<u32>>,
) -> String {
    let deletion = Deletion {
        cols: deleted_cols.unwrap_or_default(),
        rows: deleted_rows.unwrap_or_default(),
    };
    core_adjust_refs_for_deletion(formula, &deletion).into_owned()
}

/// Adjust every ref in `formula` to reflect the formula's *new* text
/// after the given rows/cols have been inserted. Refs at or past the
/// insertion point shift outward by the count of insertions
/// at-or-before their index; refs before every insertion are
/// untouched. Absolute components (`$`-prefixed) keep their markers
/// but still shift positionally. Ranges whose endpoints straddle an
/// insertion grow to include the new blank row/col. Row and column
/// indices are 0-based. Non-formula input passes through unchanged.
#[pyfunction]
#[pyo3(signature = (formula, inserted_cols = None, inserted_rows = None))]
fn adjust_refs_for_insertion(
    formula: &str,
    inserted_cols: Option<Vec<u32>>,
    inserted_rows: Option<Vec<u32>>,
) -> String {
    let insertion = Insertion {
        cols: inserted_cols.unwrap_or_default(),
        rows: inserted_rows.unwrap_or_default(),
    };
    core_adjust_refs_for_insertion(formula, &insertion).into_owned()
}

/// Adjust every column-bearing ref in `formula` to reflect a
/// contiguous block of columns `[src_start, src_end]` (0-based,
/// inclusive) moving to land starting at `final_start` in the
/// post-move layout. Block move is a permutation — no `#REF!` is
/// produced. Bounded ranges (`A1:D5`) stay positional; whole-column
/// ranges (`B:D`) follow their data via interior-bbox semantics.
/// Non-formula input passes through unchanged.
#[pyfunction]
fn adjust_refs_for_column_block_move(
    formula: &str,
    src_start: u32,
    src_end: u32,
    final_start: u32,
) -> String {
    core_adjust_refs_for_column_block_move(formula, src_start, src_end, final_start).into_owned()
}

/// Same shape as `adjust_refs_for_column_block_move` BUT bounded
/// ranges (`A1:D5`) are rewritten via interior-bbox semantics
/// rather than left in place. Use this on text where bounded
/// ranges denote *named cells* (named-range definitions) rather
/// than *positional rectangles* (cell formulas).
#[pyfunction]
fn adjust_refs_for_column_block_move_data_following(
    formula: &str,
    src_start: u32,
    src_end: u32,
    final_start: u32,
) -> String {
    core_adjust_refs_for_column_block_move_data_following(
        formula,
        src_start,
        src_end,
        final_start,
    )
    .into_owned()
}

/// Adjust every row-bearing ref in `formula` to reflect a
/// contiguous block of rows `[src_start, src_end]` (0-based,
/// inclusive) moving to land starting at `final_start` in the
/// post-move layout. Block move is a permutation — no `#REF!` is
/// produced. Bounded ranges (`A1:D5`) stay positional; whole-row
/// ranges (`3:5`) follow their data via interior-bbox semantics.
/// Whole-column ranges (`A:C`) are unaffected by a row move.
/// Non-formula input passes through unchanged. Mirror of
/// `adjust_refs_for_column_block_move` on the row axis.
#[pyfunction]
fn adjust_refs_for_row_block_move(
    formula: &str,
    src_start: u32,
    src_end: u32,
    final_start: u32,
) -> String {
    core_adjust_refs_for_row_block_move(formula, src_start, src_end, final_start).into_owned()
}

/// Same shape as `adjust_refs_for_row_block_move` BUT bounded
/// ranges (`A1:D5`) are rewritten via interior-bbox semantics
/// rather than left in place. Use this on text where bounded
/// ranges denote *named cells* (named-range definitions) rather
/// than *positional rectangles* (cell formulas). Mirror of
/// `adjust_refs_for_column_block_move_data_following` on the row
/// axis.
#[pyfunction]
fn adjust_refs_for_row_block_move_data_following(
    formula: &str,
    src_start: u32,
    src_end: u32,
    final_start: u32,
) -> String {
    core_adjust_refs_for_row_block_move_data_following(formula, src_start, src_end, final_start)
        .into_owned()
}

/// Shift every relative cell/range reference in `formula` by
/// `(d_row, d_col)`. Absolute components (those preceded by `$`) are
/// unchanged. References that would land outside `1..=max_row` /
/// `1..=max_col` become `#REF!` (a range with any off-grid endpoint
/// collapses entirely). Non-formula input passes through unchanged.
///
/// Primary use case: copy/paste of formulas — compute the paste
/// target's delta from the source anchor and rewrite each copied
/// formula so its relative refs point at the equivalent destination
/// cells.
#[pyfunction]
fn shift_formula_refs(
    formula: &str,
    d_row: i32,
    d_col: i32,
    max_row: u32,
    max_col: u32,
) -> String {
    core_shift_formula_refs(formula, d_row, d_col, max_row, max_col)
}

/// Parse an A1-style range into a dict describing its coordinates.
///
/// Returns `{"start": {"row": u32, "col": u32}, "end_col": u32,
/// "end_row": Option<u32>, "unbounded": bool, "normalized": str}`.
#[pyfunction]
fn parse_range(py: Python<'_>, input: &str) -> PyResult<PyObject> {
    let r = core_parse_range(input)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let dict = PyDict::new(py);
    let start = PyDict::new(py);
    start.set_item("row", r.start.row)?;
    start.set_item("col", r.start.col)?;
    dict.set_item("start", start)?;
    dict.set_item("end_col", r.end_col)?;
    dict.set_item("end_row", r.end_row)?;
    dict.set_item("unbounded", r.is_unbounded_rows())?;
    dict.set_item("normalized", r.normalized)?;
    Ok(dict.into())
}

/// Return True if the range string is unbounded downward. Garbage input
/// returns False (no exception).
#[pyfunction]
fn is_unbounded_range(input: &str) -> bool {
    core_is_unbounded_range(input)
}

/// Parse a single A1-style cell id into `{"row": u32, "col": u32}`.
#[pyfunction]
fn parse_cell_id(py: Python<'_>, input: &str) -> PyResult<PyObject> {
    let c = core_parse_cell_id(input)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let dict = PyDict::new(py);
    dict.set_item("row", c.row)?;
    dict.set_item("col", c.col)?;
    Ok(dict.into())
}

/// Convert column letters to a zero-based column index.
///
/// `"A"` → `0`, `"Z"` → `25`, `"AA"` → `26`. Accepts lowercase too.
/// Raises `ValueError` on empty input or non-letter characters.
#[pyfunction]
fn col_to_index(letters: &str) -> PyResult<u32> {
    core_col_to_index(letters)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

/// Convert a zero-based column index back to uppercase letters.
///
/// `0` → `"A"`, `25` → `"Z"`, `26` → `"AA"`.
#[pyfunction]
fn index_to_col(index: u32) -> String {
    core_index_to_col(index)
}

/// Build a canonical A1-style cell id from zero-based `(row, col)`.
///
/// `(0, 0)` → `"A1"`, `(11, 1)` → `"B12"`, `(0, 26)` → `"AA1"`.
#[pyfunction]
fn cell_id(row: u32, col: u32) -> String {
    core_cell_id(row, col)
}

/// Metadata for every builtin function. Useful for help panels and docs sites.
#[pyfunction]
fn list_functions(py: Python<'_>) -> PyResult<PyObject> {
    let list = PyList::empty(py);
    for info in core_list_functions() {
        list.append(function_info_to_py(py, &info)?)?;
    }
    Ok(list.into())
}

/// If the cursor sits inside a function call's parens, return the function's
/// metadata and which 0-based parameter index is being typed. Otherwise None.
#[pyfunction]
fn signature_help(py: Python<'_>, formula: &str, cursor: usize) -> PyResult<Option<PyObject>> {
    match core_signature_help(formula, cursor) {
        Some(sig) => Ok(Some(signature_help_to_py(py, &sig)?.into())),
        None => Ok(None),
    }
}

// ── conversion helpers ──

/// Map a `CellValue` to a Python object. `Empty` → `None` (via `Ok(None)`);
/// numbers that are finite, integral, and within `2^53` come back as
/// Python `int`, otherwise as `float`; strings as `str`; custom values
/// as a dict `{"type_tag": str, "data": str}`.
fn cell_value_to_py(py: Python<'_>, v: &CellValue) -> PyResult<Option<PyObject>> {
    const SAFE_INT: f64 = 9_007_199_254_740_992.0; // 2^53
    match v {
        CellValue::Empty => Ok(None),
        CellValue::String(s) => Ok(Some(s.into_pyobject(py)?.into_any().unbind())),
        CellValue::Number(n) => {
            if n.is_finite() && n.fract() == 0.0 && n.abs() < SAFE_INT {
                Ok(Some((*n as i64).into_pyobject(py)?.into_any().unbind()))
            } else {
                Ok(Some(n.into_pyobject(py)?.into_any().unbind()))
            }
        }
        CellValue::Boolean(b) => Ok(Some(b.into_pyobject(py)?.to_owned().into_any().unbind())),
        // Errors marshal as their sentinel string ("#DIV/0!" etc.) — keeps
        // the existing Python API surface; new callers can use
        // Sheet.is_error() / value_kind() if/when those land.
        CellValue::Error(e) => Ok(Some(e.to_string().into_pyobject(py)?.into_any().unbind())),
        CellValue::Custom(cv) => {
            let d = PyDict::new(py);
            d.set_item("type_tag", &cv.type_tag)?;
            d.set_item("data", &cv.data)?;
            Ok(Some(d.into_any().unbind()))
        }
    }
}

/// Decode the `kind`-discriminated dict accepted by
/// [`Sheet::set_cells_typed`] into a [`CellInput`]. The shape mirrors
/// the JSON used in the lotus-wasm binding for cross-runtime parity.
fn py_to_cell_input(payload: &Bound<'_, PyAny>) -> PyResult<CellInput> {
    let dict = payload.downcast::<PyDict>().map_err(|_| {
        pyo3::exceptions::PyValueError::new_err("payload must be a dict")
    })?;
    let kind: String = dict
        .get_item("kind")?
        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("missing key `kind`"))?
        .extract()
        .map_err(|_| pyo3::exceptions::PyValueError::new_err("`kind` must be str"))?;

    let get_str = |key: &str| -> PyResult<String> {
        dict.get_item(key)?
            .ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "{kind}: missing key `{key}`"
                ))
            })?
            .extract::<String>()
            .map_err(|_| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "{kind}: `{key}` must be str"
                ))
            })
    };

    match kind.as_str() {
        "raw" => Ok(CellInput::Raw(get_str("value")?)),
        "string" => Ok(CellInput::Typed(CellValue::String(get_str("value")?))),
        "number" => {
            let n: f64 = dict
                .get_item("value")?
                .ok_or_else(|| {
                    pyo3::exceptions::PyValueError::new_err("number: missing key `value`")
                })?
                .extract()
                .map_err(|_| {
                    pyo3::exceptions::PyValueError::new_err("number: `value` must be numeric")
                })?;
            Ok(CellInput::Typed(CellValue::Number(n)))
        }
        "boolean" => {
            let b: bool = dict
                .get_item("value")?
                .ok_or_else(|| {
                    pyo3::exceptions::PyValueError::new_err("boolean: missing key `value`")
                })?
                .extract()
                .map_err(|_| {
                    pyo3::exceptions::PyValueError::new_err("boolean: `value` must be bool")
                })?;
            Ok(CellInput::Typed(CellValue::Boolean(b)))
        }
        "empty" => Ok(CellInput::Raw(String::new())),
        "custom" => Ok(CellInput::Typed(CellValue::Custom(CustomValue {
            type_tag: get_str("type_tag")?,
            data: get_str("data")?,
        }))),
        other => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "unknown kind `{other}`"
        ))),
    }
}

/// Inverse of [`cell_value_to_py`]. Accepts `None`, `bool`, `int`/`float`,
/// `str`, or a dict with `type_tag` + `data`; anything else is stringified.
fn py_to_cell_value(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<CellValue> {
    if obj.is_none() {
        return Ok(CellValue::Empty);
    }
    // bool comes before f64 because Python bool is an int subclass.
    if let Ok(b) = obj.extract::<bool>() {
        return Ok(CellValue::Boolean(b));
    }
    if let Ok(n) = obj.extract::<f64>() {
        return Ok(CellValue::Number(n));
    }
    if let Ok(s) = obj.extract::<String>() {
        return Ok(CellValue::String(s));
    }
    if let Ok(d) = obj.downcast::<PyDict>() {
        let tag = d.get_item("type_tag")?.and_then(|v| v.extract::<String>().ok());
        let data = d.get_item("data")?.and_then(|v| v.extract::<String>().ok());
        if let (Some(type_tag), Some(data)) = (tag, data) {
            return Ok(CellValue::Custom(CustomValue { type_tag, data }));
        }
    }
    // Fallback: stringify.
    let _ = py;
    Ok(CellValue::String(obj.str()?.extract::<String>()?))
}

/// Trampoline: presents a Python handler object as a Rust
/// `CustomTypeHandler`. Every method acquires the GIL before calling
/// back into Python. Method attributes are resolved once at
/// construction so hot-path dispatch is a single `Py<PyAny>` clone and
/// `call1`.
struct PyHandler {
    tag: String,
    parse_literal: Option<Py<PyAny>>,
    parse_with: Option<Py<PyAny>>,
    display: Option<Py<PyAny>>,
    edit_repr: Option<Py<PyAny>>,
    binary_op: Option<Py<PyAny>>,
    compare: Option<Py<PyAny>>,
    as_number: Option<Py<PyAny>>,
}

impl PyHandler {
    fn new(py: Python<'_>, obj: Py<PyAny>) -> PyResult<Self> {
        let bound = obj.bind(py);
        let tag = bound
            .getattr("type_tag")
            .map_err(|_| pyo3::exceptions::PyValueError::new_err("handler: missing `type_tag`"))?
            .extract::<String>()
            .map_err(|_| {
                pyo3::exceptions::PyValueError::new_err("handler: `type_tag` must be str")
            })?;
        if tag.is_empty() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "handler: `type_tag` must be non-empty",
            ));
        }
        let resolve = |name: &str| -> Option<Py<PyAny>> {
            bound.getattr(name).ok().map(|m| m.unbind())
        };
        Ok(PyHandler {
            tag,
            parse_literal: resolve("parse_literal"),
            parse_with: resolve("parse_with"),
            display: resolve("display"),
            edit_repr: resolve("edit_repr"),
            binary_op: resolve("binary_op"),
            compare: resolve("compare"),
            as_number: resolve("as_number"),
        })
    }
}

impl CustomTypeHandler for PyHandler {
    fn type_tag(&self) -> &str {
        &self.tag
    }

    fn parse_literal(&self, raw: &str) -> Option<CustomValue> {
        let method = self.parse_literal.as_ref()?;
        Python::with_gil(|py| {
            let result = method.bind(py).call1((raw,)).ok()?;
            if result.is_none() {
                return None;
            }
            let d = result.downcast_into::<PyDict>().ok()?;
            let data = d.get_item("data").ok()??.extract::<String>().ok()?;
            Some(CustomValue {
                type_tag: self.tag.clone(),
                data,
            })
        })
    }

    fn parse_with(&self, raw: &str, options: &str) -> Option<CustomValue> {
        // Route to the Python-side `parse_with(raw, options)` if defined;
        // otherwise fall back to the trait default (delegating to
        // parse_literal). Mirrors the JS handler bridge.
        let Some(method) = self.parse_with.as_ref() else {
            return self.parse_literal(raw);
        };
        Python::with_gil(|py| {
            let result = method.bind(py).call1((raw, options)).ok()?;
            if result.is_none() {
                return None;
            }
            let d = result.downcast_into::<PyDict>().ok()?;
            let data = d.get_item("data").ok()??.extract::<String>().ok()?;
            Some(CustomValue {
                type_tag: self.tag.clone(),
                data,
            })
        })
    }

    fn display(&self, v: &CustomValue) -> String {
        let Some(method) = self.display.as_ref() else {
            return v.data.clone();
        };
        Python::with_gil(|py| {
            let Ok(arg) = custom_value_to_py(py, v) else {
                return v.data.clone();
            };
            method
                .bind(py)
                .call1((arg,))
                .ok()
                .and_then(|r| r.extract::<String>().ok())
                .unwrap_or_else(|| v.data.clone())
        })
    }

    fn edit_repr(&self, v: &CustomValue) -> String {
        let Some(method) = self.edit_repr.as_ref() else {
            return v.data.clone();
        };
        Python::with_gil(|py| {
            let Ok(arg) = custom_value_to_py(py, v) else {
                return v.data.clone();
            };
            method
                .bind(py)
                .call1((arg,))
                .ok()
                .and_then(|r| r.extract::<String>().ok())
                .unwrap_or_else(|| v.data.clone())
        })
    }

    fn binary_op(
        &self,
        op: BinaryOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<CellValue, String>> {
        let method = self.binary_op.as_ref()?;
        Python::with_gil(|py| {
            let l = cell_value_to_py(py, lhs).ok()?.unwrap_or_else(|| py.None());
            let r = cell_value_to_py(py, rhs).ok()?.unwrap_or_else(|| py.None());
            match method.bind(py).call1((op.as_str(), l, r)) {
                Err(e) => Some(Err(e.to_string())),
                Ok(result) if result.is_none() => None,
                Ok(result) => Some(py_to_cell_value(py, &result).map_err(|e| e.to_string())),
            }
        })
    }

    fn compare(
        &self,
        op: CompareOp,
        lhs: &CellValue,
        rhs: &CellValue,
    ) -> Option<Result<bool, String>> {
        let method = self.compare.as_ref()?;
        Python::with_gil(|py| {
            let l = cell_value_to_py(py, lhs).ok()?.unwrap_or_else(|| py.None());
            let r = cell_value_to_py(py, rhs).ok()?.unwrap_or_else(|| py.None());
            match method.bind(py).call1((op.as_str(), l, r)) {
                Err(e) => Some(Err(e.to_string())),
                Ok(result) if result.is_none() => None,
                Ok(result) => Some(result.extract::<bool>().map_err(|e| e.to_string())),
            }
        })
    }

    fn as_number(&self, v: &CustomValue) -> Option<f64> {
        let method = self.as_number.as_ref()?;
        Python::with_gil(|py| {
            let arg = custom_value_to_py(py, v).ok()?;
            let result = method.bind(py).call1((arg,)).ok()?;
            if result.is_none() {
                return None;
            }
            result.extract::<f64>().ok()
        })
    }
}

fn custom_value_to_py(py: Python<'_>, v: &CustomValue) -> PyResult<PyObject> {
    let d = PyDict::new(py);
    d.set_item("type_tag", &v.type_tag)?;
    d.set_item("data", &v.data)?;
    Ok(d.into_any().unbind())
}

/// Trampoline for a Python callable registered as a custom function.
struct PyFunc {
    name: String,
    func: Py<PyAny>,
}

impl CustomFunction for PyFunc {
    fn name(&self) -> &str {
        &self.name
    }

    fn call(&self, args: &[CellValue]) -> Result<CellValue, String> {
        Python::with_gil(|py| {
            let list = PyList::empty(py);
            for a in args {
                let obj = cell_value_to_py(py, a)
                    .map_err(|e| e.to_string())?
                    .unwrap_or_else(|| py.None());
                list.append(obj).map_err(|e| e.to_string())?;
            }
            let result = self
                .func
                .bind(py)
                .call1((list,))
                .map_err(|e| e.to_string())?;
            py_to_cell_value(py, &result).map_err(|e| e.to_string())
        })
    }
}

fn completion_list_to_py<'py>(
    py: Python<'py>,
    list: &CompletionList,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("replace_start", list.replace_start)?;
    dict.set_item("replace_end", list.replace_end)?;
    let items = PyList::empty(py);
    for item in &list.items {
        let m = PyDict::new(py);
        m.set_item(
            "kind",
            match item.kind {
                CompletionKind::Function => "function",
                CompletionKind::Name => "name",
            },
        )?;
        m.set_item("label", &item.label)?;
        m.set_item("insert", &item.insert)?;
        m.set_item("detail", item.detail.as_deref())?;
        m.set_item("documentation", item.documentation.as_deref())?;
        items.append(m)?;
    }
    dict.set_item("items", items)?;
    Ok(dict)
}

fn function_info_to_py<'py>(py: Python<'py>, info: &FunctionInfo) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("name", &info.name)?;
    dict.set_item("aliases", info.aliases.clone())?;
    dict.set_item("category", &info.category)?;
    let params = PyList::empty(py);
    for p in &info.params {
        params.append(param_info_to_py(py, p)?)?;
    }
    dict.set_item("params", params)?;
    dict.set_item(
        "variadic",
        match &info.variadic {
            Some(p) => Some(param_info_to_py(py, p)?),
            None => None,
        },
    )?;
    dict.set_item("description", &info.description)?;
    let examples = PyList::empty(py);
    for e in &info.examples {
        let em = PyDict::new(py);
        em.set_item("formula", &e.formula)?;
        em.set_item("result", &e.result)?;
        examples.append(em)?;
    }
    dict.set_item("examples", examples)?;
    Ok(dict)
}

fn param_info_to_py<'py>(py: Python<'py>, p: &ParamInfo) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("name", &p.name)?;
    dict.set_item("optional", p.optional)?;
    dict.set_item("description", &p.description)?;
    Ok(dict)
}

fn signature_help_to_py<'py>(
    py: Python<'py>,
    sig: &SignatureHelp,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("function", function_info_to_py(py, &sig.function)?)?;
    dict.set_item("active_param", sig.active_param)?;
    Ok(dict)
}

/// Python module exported as `lotus`.
#[pymodule]
fn lotus(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Sheet>()?;
    m.add_function(wrap_pyfunction!(evaluate, m)?)?;
    m.add_function(wrap_pyfunction!(extract_refs, m)?)?;
    m.add_function(wrap_pyfunction!(formula_tokens, m)?)?;
    m.add_function(wrap_pyfunction!(rewrite_refs_for_deletion, m)?)?;
    m.add_function(wrap_pyfunction!(adjust_refs_for_deletion, m)?)?;
    m.add_function(wrap_pyfunction!(adjust_refs_for_insertion, m)?)?;
    m.add_function(wrap_pyfunction!(adjust_refs_for_column_block_move, m)?)?;
    m.add_function(wrap_pyfunction!(
        adjust_refs_for_column_block_move_data_following,
        m
    )?)?;
    m.add_function(wrap_pyfunction!(adjust_refs_for_row_block_move, m)?)?;
    m.add_function(wrap_pyfunction!(
        adjust_refs_for_row_block_move_data_following,
        m
    )?)?;
    m.add_function(wrap_pyfunction!(shift_formula_refs, m)?)?;
    m.add_function(wrap_pyfunction!(parse_range, m)?)?;
    m.add_function(wrap_pyfunction!(is_unbounded_range, m)?)?;
    m.add_function(wrap_pyfunction!(parse_cell_id, m)?)?;
    m.add_function(wrap_pyfunction!(col_to_index, m)?)?;
    m.add_function(wrap_pyfunction!(index_to_col, m)?)?;
    m.add_function(wrap_pyfunction!(cell_id, m)?)?;
    m.add_function(wrap_pyfunction!(list_functions, m)?)?;
    m.add_function(wrap_pyfunction!(signature_help, m)?)?;
    Ok(())
}
