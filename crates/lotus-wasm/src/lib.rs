use std::sync::Arc;

use lotus_core::{
    adjust_refs_for_column_block_move as core_adjust_refs_for_column_block_move,
    adjust_refs_for_column_block_move_data_following
        as core_adjust_refs_for_column_block_move_data_following,
    adjust_refs_for_deletion as core_adjust_refs_for_deletion,
    adjust_refs_for_insertion as core_adjust_refs_for_insertion,
    adjust_refs_for_row_block_move as core_adjust_refs_for_row_block_move,
    adjust_refs_for_row_block_move_data_following
        as core_adjust_refs_for_row_block_move_data_following,
    cell_id as core_cell_id, col_to_index as core_col_to_index,
    complete_with_registry as core_complete_with_registry,
    extract_refs, formula_tokens as core_formula_tokens, index_to_col as core_index_to_col,
    is_unbounded_range as core_is_unbounded_range, parse_cell_id as core_parse_cell_id,
    parse_range as core_parse_range,
    rewrite_refs_for_deletion as core_rewrite_refs_for_deletion,
    shift_formula_refs as core_shift_formula_refs, CellInput, CellValue, CompletionKind,
    CompletionList, CustomValue, Deletion, Evaluator, FunctionInfo, Insertion, ParamInfo, RefKind,
    Sheet, SignatureHelp,
};
use wasm_bindgen::prelude::*;

mod custom_bridge;

use custom_bridge::{JsFunc, JsHandler};

/// A spreadsheet exposed to JavaScript via wasm-bindgen.
#[wasm_bindgen]
pub struct WasmSheet {
    sheet: Sheet,
}

impl Default for WasmSheet {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmSheet {
    /// Create a new empty spreadsheet.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        WasmSheet {
            sheet: Sheet::new(),
        }
    }

    /// Apply a batch of cell changes.
    ///
    /// `changes_json` must be a JSON array of `[cell_id, raw_value]` pairs, e.g.:
    /// `[["A1", "42"], ["A2", "=A1*2"]]`
    pub fn set_cells(&mut self, changes_json: &str) -> Result<(), JsError> {
        let pairs: Vec<(String, String)> = serde_json::from_str(changes_json)
            .map_err(|e| JsError::new(&format!("Invalid JSON: {e}")))?;

        self.sheet
            .set_cells(&pairs)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Apply a batch of cell changes, with each cell either raw text
    /// (engine parses) or a pre-typed value (engine stores as-is).
    ///
    /// `changes_json` is a JSON array of `[cell_id, payload]` pairs.
    /// `payload` is a `kind`-discriminated object:
    ///
    /// ```js
    /// [
    ///   ["A1", { kind: "raw", value: "=B1+1" }],
    ///   ["B1", { kind: "number", value: 42.5 }],
    ///   ["C1", { kind: "string", value: "2/4" }],   // forces literal text
    ///   ["D1", { kind: "boolean", value: true }],
    ///   ["E1", { kind: "empty" }],                   // delete cell
    ///   ["F1", { kind: "custom", type_tag: "jdate", data: "2026-04-02" }],
    /// ]
    /// ```
    ///
    /// `kind: "raw"` matches `set_cells` exactly: empty `value` deletes
    /// the cell. The other kinds bypass the parse pipeline; the typed
    /// value persists across recalcs until a `kind: "raw"` write to the
    /// same cell opts back into auto-classification.
    pub fn set_cells_typed(&mut self, changes_json: &str) -> Result<(), JsError> {
        let parsed: Vec<(String, serde_json::Value)> = serde_json::from_str(changes_json)
            .map_err(|e| JsError::new(&format!("Invalid JSON: {e}")))?;

        let mut inputs: Vec<(String, CellInput)> = Vec::with_capacity(parsed.len());
        for (cell_id, payload) in parsed {
            let input = json_to_cell_input(&payload)
                .map_err(|e| JsError::new(&format!("cell {cell_id}: {e}")))?;
            inputs.push((cell_id, input));
        }

        self.sheet
            .set_cells_typed(&inputs)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Probe a registered custom-type handler with handler-specific
    /// options. Returns `{ type_tag, data }` if the handler claims the
    /// input, or `null` if no such handler is registered or the handler
    /// declines.
    ///
    /// Use this to disambiguate input on a per-cell basis — e.g.
    /// `try_parse("jdate", "4/2", "%d/%m")` asks the date handler to
    /// interpret `"4/2"` as a day/month shorthand. Pass the returned
    /// object to `set_cells_typed` to write the typed value into a cell.
    pub fn try_parse(&self, type_tag: &str, raw: &str, options: &str) -> JsValue {
        match self.sheet.try_parse(type_tag, raw, options) {
            Some(cv) => custom_bridge::cell_value_to_js(&CellValue::Custom(cv)),
            None => JsValue::NULL,
        }
    }

    /// Get the computed value of a single cell, returned as a string.
    /// Returns an empty string for cells that have not been set.
    pub fn get(&self, cell_id: &str) -> String {
        self.sheet.get(cell_id).to_string()
    }

    /// Get all computed values as a JSON object mapping cell IDs to string values.
    /// Empty cells are omitted.
    pub fn get_all(&self) -> String {
        let mut map = serde_json::Map::new();
        for (key, val) in self.sheet.get_all() {
            if *val != CellValue::Empty {
                map.insert(key.clone(), serde_json::Value::String(val.to_string()));
            }
        }
        serde_json::Value::Object(map).to_string()
    }

    /// Get the typed computed value of a cell. Numbers come back as JS
    /// `number`, strings (including error values like `"#DIV/0!"`) as
    /// strings, and empty cells as `null`. Unlike `get`, this preserves
    /// the engine's internal number/string distinction.
    pub fn get_typed(&self, cell_id: &str) -> JsValue {
        cell_value_to_js(&self.sheet.get(cell_id))
    }

    /// All computed values as a JS object `{cellId: typedValue}`. Empty
    /// cells are omitted; numbers stay numbers, strings stay strings.
    pub fn get_all_typed(&self) -> JsValue {
        let obj = js_sys::Object::new();
        for (key, val) in self.sheet.get_all() {
            if !matches!(val, CellValue::Empty) {
                let _ = js_sys::Reflect::set(&obj, &JsValue::from_str(key), &cell_value_to_js(val));
            }
        }
        obj.into()
    }

    /// If `cell_id` is an anchor, returns the full array as a JS 2-D
    /// array with typed values (row-major). Otherwise `null`.
    pub fn get_array_typed(&self, cell_id: &str) -> JsValue {
        let Some(arr) = self.sheet.spill_array(cell_id) else {
            return JsValue::NULL;
        };
        let outer = js_sys::Array::new_with_length(arr.rows);
        for r in 0..arr.rows {
            let row = js_sys::Array::new_with_length(arr.cols);
            for c in 0..arr.cols {
                row.set(c, cell_value_to_js(arr.get(r, c)));
            }
            outer.set(r, row.into());
        }
        outer.into()
    }

    /// Define (or overwrite) a workbook-global named range.
    ///
    /// `definition` is the raw text — `"=B1"`, `"=A1:A10"`,
    /// `"=SUM(A1:A10)"`, or a literal like `"0.05"`. Throws on invalid
    /// names (cell-ref-shaped, builtin function names, malformed) or
    /// unparseable definitions. Triggers a recalculation.
    pub fn set_name(&mut self, name: &str, definition: &str) -> Result<(), JsError> {
        self.sheet
            .set_name(name, definition)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Remove a named range and recalculate. No-op if the name is undefined.
    pub fn remove_name(&mut self, name: &str) -> Result<(), JsError> {
        self.sheet
            .remove_name(name)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Look up a name's raw definition. Returns `undefined` if the name
    /// is not defined.
    pub fn get_name(&self, name: &str) -> Option<String> {
        self.sheet.get_name(name).map(|s| s.to_string())
    }

    /// Return all defined names as a JSON object mapping uppercased name → definition.
    pub fn names(&self) -> String {
        let mut map = serde_json::Map::new();
        for (k, v) in self.sheet.names() {
            map.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        serde_json::Value::Object(map).to_string()
    }

    /// Evaluate a formula string without any cell context.
    /// Useful for one-shot calculations like `evaluate("=1+2")`.
    pub fn evaluate(formula: &str) -> String {
        let eval = Evaluator::new(|_| CellValue::Empty);
        match eval.evaluate(formula) {
            Ok(val) => val.to_string(),
            Err(e) => e.to_string(),
        }
    }

    /// Extract all cell references from a formula, with source positions.
    ///
    /// Returns a JSON array of objects:
    /// `[{start, end, text, kind, cells}, ...]` where `kind` is one of
    /// `"cell"`, `"range"`, `"whole_column"`, `"whole_row"`, `"name"`.
    pub fn extract_refs(formula: &str) -> String {
        let refs = extract_refs(formula);
        let arr: Vec<serde_json::Value> = refs
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
                // CellCoord → "A1"-style strings at the binding boundary so
                // the JSON API surface stays identical for the JS frontend.
                let cells: Vec<String> = r
                    .cells
                    .iter()
                    .map(|c| lotus_core::format_cell_id(*c))
                    .collect();
                serde_json::json!({
                    "start": r.start,
                    "end": r.end,
                    "text": r.text,
                    "kind": kind,
                    "cells": cells,
                })
            })
            .collect();
        serde_json::Value::Array(arr).to_string()
    }

    /// Classify every token of a formula string with its source span.
    ///
    /// Returns a JSON array of objects:
    /// `[{start, end, kind}, ...]` where `kind` is one of `"number"`,
    /// `"string"`, `"cell_ref"`, `"range"`, `"name"`, `"function"`,
    /// `"operator"`, `"paren"`, `"comma"`, `"whitespace"`, `"unknown"`.
    /// Non-formula input returns `[]`. Lexer errors produce whatever
    /// tokens were recognized plus an `"unknown"` span covering the tail.
    pub fn formula_tokens(formula: &str) -> String {
        let toks = core_formula_tokens(formula);
        let arr: Vec<serde_json::Value> = toks
            .into_iter()
            .map(|t| {
                serde_json::json!({
                    "start": t.start,
                    "end": t.end,
                    "kind": t.kind.as_str(),
                })
            })
            .collect();
        serde_json::Value::Array(arr).to_string()
    }

    /// Rewrite every ref in `formula` that points into the given
    /// deleted rows or columns, replacing the affected portion with
    /// `#REF!`. `deletion_json` must be a JSON object of the shape
    /// `{"cols": number[], "rows": number[]}` (indices are 0-based).
    pub fn rewrite_refs_for_deletion(
        formula: &str,
        deletion_json: &str,
    ) -> Result<String, JsError> {
        let deletion = parse_deletion_json(deletion_json)?;
        Ok(core_rewrite_refs_for_deletion(formula, &deletion).into_owned())
    }

    /// Adjust every ref in `formula` to reflect the formula's *new*
    /// text after the given rows/cols have been deleted: surviving
    /// refs shift left/up, ranges trim to the surviving extent, and
    /// fully-deleted refs become `#REF!`. `deletion_json` shape is
    /// `{"cols": number[], "rows": number[]}` (indices 0-based).
    pub fn adjust_refs_for_deletion(
        formula: &str,
        deletion_json: &str,
    ) -> Result<String, JsError> {
        let deletion = parse_deletion_json(deletion_json)?;
        Ok(core_adjust_refs_for_deletion(formula, &deletion).into_owned())
    }

    /// Adjust every ref in `formula` to reflect the formula's *new*
    /// text after the given rows/cols have been inserted: refs at or
    /// past the insertion point shift outward, and ranges straddling
    /// an insertion grow to include the new blank row/col. Absolute
    /// components (`$`-prefixed) keep their markers but still shift
    /// positionally. `insertion_json` shape is
    /// `{"cols": number[], "rows": number[]}` (indices 0-based;
    /// duplicates mean multiple insertions at the same index).
    pub fn adjust_refs_for_insertion(
        formula: &str,
        insertion_json: &str,
    ) -> Result<String, JsError> {
        let insertion = parse_insertion_json(insertion_json)?;
        Ok(core_adjust_refs_for_insertion(formula, &insertion).into_owned())
    }

    /// Adjust every column-bearing ref in `formula` to reflect a
    /// contiguous block of columns `[src_start, src_end]` (0-based,
    /// inclusive) moving to land starting at `final_start` in the
    /// post-move layout. Block move is a permutation — no `#REF!`
    /// is produced. Bounded ranges (`A1:D5`) stay positional;
    /// whole-column ranges (`B:D`) follow their data via
    /// interior-bbox semantics. See lotus-core docs.
    pub fn adjust_refs_for_column_block_move(
        formula: &str,
        src_start: u32,
        src_end: u32,
        final_start: u32,
    ) -> String {
        core_adjust_refs_for_column_block_move(formula, src_start, src_end, final_start)
            .into_owned()
    }

    /// Same shape as `adjust_refs_for_column_block_move` BUT bounded
    /// ranges (`A1:D5`) are rewritten via interior-bbox semantics
    /// rather than left in place. Use this on text where bounded
    /// ranges denote *named cells* (named-range definitions) rather
    /// than *positional rectangles* (cell formulas). See lotus-core
    /// docs.
    pub fn adjust_refs_for_column_block_move_data_following(
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
    /// post-move layout. Block move is a permutation — no `#REF!`
    /// is produced. Bounded ranges (`A1:D5`) stay positional;
    /// whole-row ranges (`3:5`) follow their data via interior-bbox
    /// semantics. Whole-column ranges (`A:C`) are unaffected.
    /// Mirror of `adjust_refs_for_column_block_move` on the row
    /// axis. See lotus-core docs.
    pub fn adjust_refs_for_row_block_move(
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
    /// `adjust_refs_for_column_block_move_data_following` on the
    /// row axis. See lotus-core docs.
    pub fn adjust_refs_for_row_block_move_data_following(
        formula: &str,
        src_start: u32,
        src_end: u32,
        final_start: u32,
    ) -> String {
        core_adjust_refs_for_row_block_move_data_following(
            formula,
            src_start,
            src_end,
            final_start,
        )
        .into_owned()
    }

    /// Shift every relative cell/range reference in `formula` by
    /// `(d_row, d_col)`. Absolute components (`$`-prefixed) stay put.
    /// References that would land outside `1..=max_row` / `1..=max_col`
    /// become `#REF!` (a range with any off-grid endpoint collapses
    /// entirely). Non-formula input passes through unchanged.
    pub fn shift_formula_refs(
        formula: &str,
        d_row: i32,
        d_col: i32,
        max_row: u32,
        max_col: u32,
    ) -> String {
        core_shift_formula_refs(formula, d_row, d_col, max_row, max_col)
    }

    /// Parse an A1-style range into a JSON object describing its
    /// coordinates. Throws a `JsError` for unparseable input.
    ///
    /// Shape: `{"start": {"row": u32, "col": u32}, "end_col": u32,
    /// "end_row": u32 | null, "unbounded": bool, "normalized": str}`
    pub fn parse_range(input: &str) -> Result<String, JsError> {
        let r = core_parse_range(input).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(serde_json::json!({
            "start": { "row": r.start.row, "col": r.start.col },
            "end_col": r.end_col,
            "end_row": r.end_row,
            "unbounded": r.is_unbounded_rows(),
            "normalized": r.normalized,
        })
        .to_string())
    }

    /// Return `true` if the range string is unbounded downward. Garbage
    /// input returns `false` (no throw).
    pub fn is_unbounded_range(input: &str) -> bool {
        core_is_unbounded_range(input)
    }

    /// Parse a single A1-style cell id into `{"row": u32, "col": u32}`.
    pub fn parse_cell_id(input: &str) -> Result<String, JsError> {
        let c = core_parse_cell_id(input).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(serde_json::json!({ "row": c.row, "col": c.col }).to_string())
    }

    /// Convert column letters to a zero-based column index.
    ///
    /// `"A"` → `0`, `"Z"` → `25`, `"AA"` → `26`. Accepts lowercase too.
    /// Throws on empty input or non-letter characters.
    pub fn col_to_index(letters: &str) -> Result<u32, JsError> {
        core_col_to_index(letters).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Convert a zero-based column index back to uppercase letters.
    ///
    /// `0` → `"A"`, `25` → `"Z"`, `26` → `"AA"`.
    pub fn index_to_col(index: u32) -> String {
        core_index_to_col(index)
    }

    /// Build a canonical A1-style cell id from zero-based `(row, col)`.
    ///
    /// `(0, 0)` → `"A1"`, `(11, 1)` → `"B12"`, `(0, 26)` → `"AA1"`.
    pub fn cell_id(row: u32, col: u32) -> String {
        core_cell_id(row, col)
    }

    /// If `cell_id` is the anchor of a spill region, returns JSON
    /// `{"rows": N, "cols": M}`. Otherwise returns `"null"`.
    pub fn spill_at(&self, cell_id: &str) -> String {
        match self.sheet.spill_at(cell_id) {
            Some(region) => serde_json::json!({
                "rows": region.rows,
                "cols": region.cols,
            })
            .to_string(),
            None => serde_json::Value::Null.to_string(),
        }
    }

    /// If `cell_id` was populated by a spill, returns the anchor id as
    /// a string. Otherwise returns `"null"` (as JSON).
    pub fn owned_by(&self, cell_id: &str) -> String {
        match self.sheet.owner_of(cell_id) {
            Some(anchor) => serde_json::Value::String(anchor.clone()).to_string(),
            None => serde_json::Value::Null.to_string(),
        }
    }

    /// Pin a 2-D value at an anchor cell. The pin overrides whatever
    /// the cell's formula would evaluate to and spills like a native
    /// array result. Blockers flip the anchor to `#SPILL!`.
    ///
    /// `array_json` must be a JSON 2-D string array, e.g. `[["1","2"],["3","4"]]`.
    /// Throws on ragged/empty arrays or malformed JSON.
    pub fn pin_value(&mut self, cell_id: &str, array_json: &str) -> Result<(), JsError> {
        let parsed: Vec<Vec<String>> = serde_json::from_str(array_json)
            .map_err(|e| JsError::new(&format!("Invalid JSON: {e}")))?;
        let rows: Vec<Vec<CellValue>> = parsed
            .into_iter()
            .map(|r| r.into_iter().map(CellValue::String).collect())
            .collect();
        self.sheet
            .pin_value(cell_id, rows)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Remove a pin. On the next recalc the cell reverts to evaluating
    /// its own formula (or to empty if it has none). No-op if not pinned.
    pub fn unpin_value(&mut self, cell_id: &str) {
        self.sheet.unpin_value(cell_id);
    }

    /// All currently-pinned anchor cell IDs as JSON array of strings.
    pub fn pinned_cells(&self) -> String {
        serde_json::Value::Array(
            self.sheet
                .pinned_cells()
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        )
        .to_string()
    }

    /// True if the cell's value comes from a pin rather than its own formula.
    pub fn is_pinned(&self, cell_id: &str) -> bool {
        self.sheet.is_pinned(cell_id)
    }

    /// If `cell_id` is an anchor, returns the full array as JSON —
    /// a 2-D array of strings (row-major, empty cells as ""). Otherwise `"null"`.
    pub fn get_array(&self, cell_id: &str) -> String {
        let Some(arr) = self.sheet.spill_array(cell_id) else {
            return serde_json::Value::Null.to_string();
        };
        let rows: Vec<serde_json::Value> = (0..arr.rows)
            .map(|r| {
                let row: Vec<serde_json::Value> = (0..arr.cols)
                    .map(|c| serde_json::Value::String(arr.get(r, c).to_string()))
                    .collect();
                serde_json::Value::Array(row)
            })
            .collect();
        serde_json::Value::Array(rows).to_string()
    }

    /// Completion suggestions at a cursor position. Returns a JSON object:
    /// `{replace_start, replace_end, items: [{kind, label, insert, detail, documentation}]}`.
    /// `kind` is `"function"` or `"name"`. Includes runtime-registered
    /// custom functions (e.g. anything added via `register_function` /
    /// `register_datetime` / `register_url`) so they show up in the
    /// browser's autocomplete popup.
    pub fn complete(&self, formula: &str, cursor: usize) -> String {
        let names: Vec<&str> = self.sheet.names().keys().map(|s| s.as_str()).collect();
        let list = core_complete_with_registry(self.sheet.registry(), formula, cursor, &names);
        completion_list_to_json(&list).to_string()
    }

    /// Metadata for every function this sheet's evaluator can dispatch
    /// on: builtins followed by runtime-registered customs (e.g. anything
    /// added by `register_function`, `register_datetime`, or
    /// `register_url`). Returns a
    /// JSON array. Custom functions surface a minimal entry (name +
    /// open-ended variadic) since the host doesn't supply parameter
    /// docs at registration time.
    pub fn list_functions(&self) -> String {
        let arr: Vec<serde_json::Value> = self
            .sheet
            .list_functions()
            .iter()
            .map(function_info_to_json)
            .collect();
        serde_json::Value::Array(arr).to_string()
    }

    /// If the cursor sits inside a function call's parens, returns
    /// `{function: FunctionInfo, active_param: int}`. Otherwise `"null"`.
    /// Resolves names through both the builtin table and this sheet's
    /// registry, so registered customs (YEAR, host extensions, …) light
    /// up alongside SUM / IF / etc.
    pub fn signature_help(&self, formula: &str, cursor: usize) -> String {
        match self.sheet.signature_help(formula, cursor) {
            Some(sig) => signature_help_to_json(&sig).to_string(),
            None => serde_json::Value::Null.to_string(),
        }
    }

    /// Register a JS-defined custom type handler. The `handler` argument
    /// is a plain JS object; see [`custom_bridge`] for the expected
    /// method contract. Throws on duplicate `typeTag` or malformed input.
    ///
    /// Must be called before any `set_cells` that should use the handler —
    /// registering mid-session won't re-ingest already-stored cells.
    pub fn register_type(&mut self, handler: JsValue) -> Result<(), JsError> {
        let h = JsHandler::new(handler)?;
        self.sheet
            .register_type(Arc::new(h))
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Register a JS-defined custom function. `func` receives a flat
    /// array of arg values (numbers, strings, nulls, or
    /// `{type_tag, data}` objects) and returns a value in the same
    /// vocabulary, or `{error: "..."}` to raise a spreadsheet error.
    /// Throws on collision with a built-in or previously-registered name.
    pub fn register_function(&mut self, name: &str, func: JsValue) -> Result<(), JsError> {
        let f = func
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsError::new("register_function: `func` must be a JS function"))?;
        let wrapped = JsFunc::new(name, f)?;
        self.sheet
            .register_function(Arc::new(wrapped))
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Register the bundled `lotus-datetime` extension on this sheet.
    /// Adds the six `j*` datetime cell types (`jdate`, `jtime`,
    /// `jdatetime`, `jzoned`, `jtimezone`, `jspan`) and ~40 formula
    /// functions (`DATE`, `NOW`, `YEAR`, `DAYS_BETWEEN`, …) operating
    /// on them. Values cross the JS boundary as `{type_tag, data}`
    /// objects via the existing custom-value marshalling.
    ///
    /// Compiled in only when `lotus-wasm` was built with the `datetime`
    /// cargo feature; otherwise this method is absent. Throws on
    /// duplicate registration (calling twice on the same sheet, or
    /// having pre-registered a colliding type tag).
    #[cfg(feature = "datetime")]
    pub fn register_datetime(&mut self) -> Result<(), JsError> {
        lotus_datetime::register_on_sheet(&mut self.sheet)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Register the bundled `lotus-url` extension on this sheet. Adds
    /// scalar URL functions (`URL_SCHEME`, `URL_HOST`, `URL_PORT`,
    /// `URL_PATH`, `URL_PATH_SEGMENT`, `URL_QUERY`, `URL_FRAGMENT`,
    /// `URL_PARAM`, `URL_VALID`, `URL_ENCODE`, `URL_DECODE`,
    /// `URL_JOIN`). No new cell types — URLs are plain text.
    ///
    /// Compiled in only when `lotus-wasm` was built with the `url`
    /// cargo feature; otherwise this method is absent. Throws on
    /// duplicate registration.
    #[cfg(feature = "url")]
    pub fn register_url(&mut self) -> Result<(), JsError> {
        lotus_url::register_on_sheet(&mut self.sheet)
            .map_err(|e| JsError::new(&e.to_string()))
    }
}

fn cell_value_to_js(v: &CellValue) -> JsValue {
    custom_bridge::cell_value_to_js(v)
}

/// Decode the `kind`-discriminated payload object accepted by
/// `set_cells_typed` into a [`CellInput`]. See the method doc on
/// [`WasmSheet::set_cells_typed`] for the JSON shape.
fn json_to_cell_input(v: &serde_json::Value) -> Result<CellInput, String> {
    let obj = v
        .as_object()
        .ok_or_else(|| "payload must be an object".to_string())?;
    let kind = obj
        .get("kind")
        .and_then(|k| k.as_str())
        .ok_or_else(|| "missing string field `kind`".to_string())?;

    match kind {
        "raw" => {
            let value = obj
                .get("value")
                .and_then(|s| s.as_str())
                .ok_or_else(|| "raw: missing string field `value`".to_string())?;
            Ok(CellInput::Raw(value.to_string()))
        }
        "number" => {
            let value = obj
                .get("value")
                .and_then(|n| n.as_f64())
                .ok_or_else(|| "number: missing numeric field `value`".to_string())?;
            Ok(CellInput::Typed(CellValue::Number(value)))
        }
        "string" => {
            let value = obj
                .get("value")
                .and_then(|s| s.as_str())
                .ok_or_else(|| "string: missing string field `value`".to_string())?;
            Ok(CellInput::Typed(CellValue::String(value.to_string())))
        }
        "boolean" => {
            let value = obj
                .get("value")
                .and_then(|b| b.as_bool())
                .ok_or_else(|| "boolean: missing bool field `value`".to_string())?;
            Ok(CellInput::Typed(CellValue::Boolean(value)))
        }
        "empty" => Ok(CellInput::Raw(String::new())),
        "custom" => {
            let type_tag = obj
                .get("type_tag")
                .and_then(|s| s.as_str())
                .ok_or_else(|| "custom: missing string field `type_tag`".to_string())?;
            let data = obj
                .get("data")
                .and_then(|s| s.as_str())
                .ok_or_else(|| "custom: missing string field `data`".to_string())?;
            Ok(CellInput::Typed(CellValue::Custom(CustomValue {
                type_tag: type_tag.to_string(),
                data: data.to_string(),
            })))
        }
        other => Err(format!("unknown kind `{other}`")),
    }
}

fn completion_list_to_json(list: &CompletionList) -> serde_json::Value {
    let items: Vec<serde_json::Value> = list
        .items
        .iter()
        .map(|i| {
            serde_json::json!({
                "kind": match i.kind {
                    CompletionKind::Function => "function",
                    CompletionKind::Name => "name",
                },
                "label": i.label,
                "insert": i.insert,
                "detail": i.detail,
                "documentation": i.documentation,
            })
        })
        .collect();
    serde_json::json!({
        "replace_start": list.replace_start,
        "replace_end": list.replace_end,
        "items": items,
    })
}

fn function_info_to_json(info: &FunctionInfo) -> serde_json::Value {
    serde_json::json!({
        "name": info.name,
        "aliases": info.aliases,
        "category": info.category,
        "params": info.params.iter().map(param_info_to_json).collect::<Vec<_>>(),
        "variadic": info.variadic.as_ref().map(param_info_to_json),
        "description": info.description,
        "examples": info.examples.iter().map(|e| serde_json::json!({
            "formula": e.formula,
            "result": e.result,
        })).collect::<Vec<_>>(),
    })
}

fn param_info_to_json(p: &ParamInfo) -> serde_json::Value {
    serde_json::json!({
        "name": p.name,
        "optional": p.optional,
        "description": p.description,
    })
}

fn signature_help_to_json(sig: &SignatureHelp) -> serde_json::Value {
    serde_json::json!({
        "function": function_info_to_json(&sig.function),
        "active_param": sig.active_param,
    })
}

fn parse_deletion_json(s: &str) -> Result<Deletion, JsError> {
    let payload: serde_json::Value =
        serde_json::from_str(s).map_err(|e| JsError::new(&format!("Invalid JSON: {e}")))?;
    let cols = extract_u32_array(&payload, "cols")?;
    let rows = extract_u32_array(&payload, "rows")?;
    Ok(Deletion { cols, rows })
}

fn parse_insertion_json(s: &str) -> Result<Insertion, JsError> {
    let payload: serde_json::Value =
        serde_json::from_str(s).map_err(|e| JsError::new(&format!("Invalid JSON: {e}")))?;
    let cols = extract_u32_array(&payload, "cols")?;
    let rows = extract_u32_array(&payload, "rows")?;
    Ok(Insertion { cols, rows })
}

fn extract_u32_array(v: &serde_json::Value, key: &str) -> Result<Vec<u32>, JsError> {
    match v.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| JsError::new(&format!("Expected non-negative u32 in `{key}`")))
            })
            .collect(),
        Some(_) => Err(JsError::new(&format!("Expected array for `{key}`"))),
    }
}
