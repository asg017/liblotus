//! Extract cell references from a formula string and rewrite them when
//! rows or columns are deleted.

use std::borrow::Cow;
use std::collections::HashSet;

use crate::lexer::Lexer;
use crate::range::CellCoord;
use crate::types::{col_to_index, index_to_col, Token, TokenType, MAX_RANGE_CELLS};

/// What shape of reference a [`FormulaRef`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// Single cell, e.g. `A1`.
    Cell,
    /// Bounded rectangular range, e.g. `A1:B2`.
    Range,
    /// Whole-column range, e.g. `B:B` or `A:C`.
    WholeColumn,
    /// Whole-row range, e.g. `1:1` or `5:7`.
    WholeRow,
    /// Named-range reference, e.g. `TaxRate`. Resolved separately by callers
    /// against the sheet's name table; `cells` is empty.
    Name,
    /// Spill operator `A1#` — references the full spill region anchored at
    /// a cell. `cells` contains the anchor id only; callers resolve the
    /// region through `Sheet::spill_at`.
    SpillRef,
}

/// A cell reference or range found in a formula, with source positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaRef {
    /// Byte offset of the start of this reference in the original formula string (including the `=` prefix).
    pub start: usize,
    /// Byte offset of the end (exclusive) in the original formula string.
    pub end: usize,
    /// The raw text of the reference, e.g. "A1" or "A1:B3".
    pub text: String,
    /// What shape of reference this is.
    pub kind: RefKind,
    /// All cells covered by this reference, as zero-based [`CellCoord`]s.
    /// Bounded ranges are expanded into the full rectangle.
    /// **Empty for [`RefKind::WholeColumn`] and [`RefKind::WholeRow`]** —
    /// callers should not expand these.
    ///
    /// Coordinates instead of strings: a 1000-row range used to allocate
    /// 1000 [`String`]s on every keystroke (the WASM editor calls
    /// `extract_refs` on every keystroke); 8 bytes per coord vs ~24 bytes
    /// per heap-allocated short string. Format with
    /// [`crate::format_cell_id`] when stringifying at a boundary.
    pub cells: Vec<CellCoord>,
}

/// A set of deleted rows/columns. Indices are 0-based.
#[derive(Debug, Default, Clone)]
pub struct Deletion {
    pub cols: Vec<u32>,
    pub rows: Vec<u32>,
}

/// A set of inserted rows/columns. Indices are 0-based. Each index
/// represents a new blank row/column inserted **at** that index,
/// shifting every row/col at-or-after the index by +1. Duplicates
/// mean multiple insertions at the same index (e.g. `cols: [3, 3]`
/// inserts two columns at index 3).
#[derive(Debug, Default, Clone)]
pub struct Insertion {
    pub cols: Vec<u32>,
    pub rows: Vec<u32>,
}

const REF_ERROR: &str = "#REF!";

/// Extract all cell references and ranges from a formula string.
///
/// Returns an empty vec for non-formula strings (those not starting with `=`).
/// Silently returns an empty vec if tokenization fails (partial/invalid formulas).
///
/// Includes whole-column (`B:B`) and whole-row (`1:1`) ranges in the
/// result; their `cells` field is empty because callers shouldn't
/// expand them into concrete cell ids.
pub fn extract_refs(formula: &str) -> Vec<FormulaRef> {
    if !formula.starts_with('=') {
        return vec![];
    }

    let tokens = match Lexer::new(formula).tokenize() {
        Ok(t) => t,
        Err(_) => return vec![],
    };

    let mut refs = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let t = &tokens[i];

        // A1-style cell ref or range.
        if t.token_type == TokenType::CellRef {
            if i + 2 < tokens.len()
                && tokens[i + 1].token_type == TokenType::Colon
                && tokens[i + 2].token_type == TokenType::CellRef
            {
                let end_tok = &tokens[i + 2];
                let start = t.position + 1;
                let end = end_tok.position + end_tok.source_len + 1;
                let text = formula[start..end].to_string();
                let cells = expand_range(&t.value, &end_tok.value);
                refs.push(FormulaRef {
                    start,
                    end,
                    text,
                    kind: RefKind::Range,
                    cells,
                });
                i += 3;
                continue;
            }
            // Spill operator A1#
            if matches!(tokens.get(i + 1), Some(n) if n.token_type == TokenType::Hash) {
                let hash_tok = &tokens[i + 1];
                let start = t.position + 1;
                let end = hash_tok.position + 1 + 1;
                let text = formula[start..end].to_string();
                refs.push(FormulaRef {
                    start,
                    end,
                    text,
                    kind: RefKind::SpillRef,
                    cells: coord_of(&t.value).map(|c| vec![c]).unwrap_or_default(),
                });
                i += 2;
                continue;
            }
            let start = t.position + 1;
            let end = start + t.source_len;
            // Preserve source bytes (including any `$`) in `text`; `cells`
            // stays canonical so callers can match against sheet cell IDs.
            refs.push(FormulaRef {
                start,
                end,
                text: formula[start..end].to_string(),
                kind: RefKind::Cell,
                cells: coord_of(&t.value).map(|c| vec![c]).unwrap_or_default(),
            });
            i += 1;
            continue;
        }

        // Whole-column range: Function(letters) COLON Function(letters).
        // Reject when the Function token is actually a function call (followed by `(`).
        if t.token_type == TokenType::Function
            && is_pure_letters(&t.value)
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Function
            && is_pure_letters(&tokens[i + 2].value)
        {
            let end_tok = &tokens[i + 2];
            let start = t.position + 1;
            let end = end_tok.position + end_tok.source_len + 1;
            let text = formula[start..end].to_string();
            refs.push(FormulaRef {
                start,
                end,
                text,
                kind: RefKind::WholeColumn,
                cells: vec![],
            });
            i += 3;
            continue;
        }

        // Whole-row range: Number COLON Number.
        if t.token_type == TokenType::Number
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Number
        {
            let end_tok = &tokens[i + 2];
            let start = t.position + 1;
            let end = end_tok.position + end_tok.source_len + 1;
            let text = formula[start..end].to_string();
            refs.push(FormulaRef {
                start,
                end,
                text,
                kind: RefKind::WholeRow,
                cells: vec![],
            });
            i += 3;
            continue;
        }

        // Named-range reference: a bare Function token that isn't a
        // function call (no `(` follows) and isn't part of a column-range
        // (already matched above). Callers resolve the name separately.
        if t.token_type == TokenType::Function
            && !matches!(
                tokens.get(i + 1),
                Some(next) if next.token_type == TokenType::LParen
            )
        {
            let start = t.position + 1;
            let end = start + t.source_len;
            refs.push(FormulaRef {
                start,
                end,
                text: t.value.clone(),
                kind: RefKind::Name,
                cells: vec![],
            });
            i += 1;
            continue;
        }

        i += 1;
    }

    refs
}

/// Rewrite every ref in `formula` that points into the deleted
/// row/column set, replacing the affected portion with `#REF!`.
///
/// Rules, matching Google Sheets:
/// - Single cell ref whose col or row is deleted  → `#REF!`
/// - Range where both endpoints are deleted       → `#REF!`
/// - Range where one endpoint is deleted          → replace that half
///   with `#REF!`, keep the other: `A1:B2` + del col B → `A1:#REF!`
/// - Whole-column range (`B:B`) + col B deleted   → `#REF!`
/// - Whole-row range (`1:1`) + row 1 deleted      → `#REF!`
/// - Refs unaffected by the deletion              → unchanged bytes
///
/// Non-formula input (doesn't start with `=`) is returned as-is. If
/// tokenization fails, the input is returned unchanged; this makes
/// the function idempotent even though `#REF!` is not itself a
/// tokenizable expression.
pub fn rewrite_refs_for_deletion<'a>(
    formula: &'a str,
    deletion: &Deletion,
) -> Cow<'a, str> {
    if !formula.starts_with('=') {
        return Cow::Borrowed(formula);
    }
    if deletion.cols.is_empty() && deletion.rows.is_empty() {
        return Cow::Borrowed(formula);
    }

    let tokens = match Lexer::new(formula).tokenize() {
        Ok(t) => t,
        Err(_) => return Cow::Borrowed(formula),
    };

    let cols: HashSet<u32> = deletion.cols.iter().copied().collect();
    let rows: HashSet<u32> = deletion.rows.iter().copied().collect();

    // (start, end, replacement) into the original `formula` string.
    let mut edits: Vec<(usize, usize, &'static str)> = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let t = &tokens[i];

        // A1-range or single cell.
        if t.token_type == TokenType::CellRef {
            if i + 2 < tokens.len()
                && tokens[i + 1].token_type == TokenType::Colon
                && tokens[i + 2].token_type == TokenType::CellRef
            {
                let end_tok = &tokens[i + 2];
                let start_deleted = cell_ref_is_deleted(&t.value, &cols, &rows);
                let end_deleted = cell_ref_is_deleted(&end_tok.value, &cols, &rows);
                push_range_edit(&mut edits, t, end_tok, start_deleted, end_deleted);
                i += 3;
                continue;
            }
            if cell_ref_is_deleted(&t.value, &cols, &rows) {
                let start = t.position + 1;
                let end = start + t.source_len;
                edits.push((start, end, REF_ERROR));
            }
            i += 1;
            continue;
        }

        // Whole-column range.
        if t.token_type == TokenType::Function
            && is_pure_letters(&t.value)
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Function
            && is_pure_letters(&tokens[i + 2].value)
        {
            let end_tok = &tokens[i + 2];
            let start_deleted = col_letters_deleted(&t.value, &cols);
            let end_deleted = col_letters_deleted(&end_tok.value, &cols);
            push_range_edit(&mut edits, t, end_tok, start_deleted, end_deleted);
            i += 3;
            continue;
        }

        // Whole-row range.
        if t.token_type == TokenType::Number
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Number
        {
            let end_tok = &tokens[i + 2];
            let start_deleted = row_number_deleted(&t.value, &rows);
            let end_deleted = row_number_deleted(&end_tok.value, &rows);
            push_range_edit(&mut edits, t, end_tok, start_deleted, end_deleted);
            i += 3;
            continue;
        }

        i += 1;
    }

    if edits.is_empty() {
        return Cow::Borrowed(formula);
    }

    let mut out = String::with_capacity(formula.len());
    let mut cursor = 0;
    for (s, e, rep) in edits {
        out.push_str(&formula[cursor..s]);
        out.push_str(rep);
        cursor = e;
    }
    out.push_str(&formula[cursor..]);
    Cow::Owned(out)
}

/// Push an edit for a two-endpoint range (A1:B2, A:C, 1:5). The
/// byte spans are computed in terms of the original formula (with
/// the leading `=`), so tokens' lexer-relative positions are shifted
/// by `+1`.
fn push_range_edit(
    edits: &mut Vec<(usize, usize, &'static str)>,
    start_tok: &Token,
    end_tok: &Token,
    start_deleted: bool,
    end_deleted: bool,
) {
    let full_start = start_tok.position + 1;
    let full_end = end_tok.position + end_tok.source_len + 1;
    match (start_deleted, end_deleted) {
        (false, false) => {}
        (true, true) => edits.push((full_start, full_end, REF_ERROR)),
        (true, false) => {
            let start_end = start_tok.position + start_tok.source_len + 1;
            edits.push((full_start, start_end, REF_ERROR));
        }
        (false, true) => {
            let end_start = end_tok.position + 1;
            edits.push((end_start, full_end, REF_ERROR));
        }
    }
}

fn is_pure_letters(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic())
}

fn cell_ref_is_deleted(cell: &str, cols: &HashSet<u32>, rows: &HashSet<u32>) -> bool {
    let Some((col_str, row)) = parse_ref(cell) else {
        return false;
    };
    if row == 0 {
        return false;
    }
    let col_idx = col_to_index(col_str.as_ref()) - 1;
    let row_idx = row - 1;
    cols.contains(&col_idx) || rows.contains(&row_idx)
}

fn col_letters_deleted(letters: &str, cols: &HashSet<u32>) -> bool {
    if !is_pure_letters(letters) {
        return false;
    }
    let idx = col_to_index(letters).saturating_sub(1);
    cols.contains(&idx)
}

fn row_number_deleted(n: &str, rows: &HashSet<u32>) -> bool {
    let row: u32 = match n.parse() {
        Ok(r) if r > 0 => r,
        _ => return false,
    };
    rows.contains(&(row - 1))
}

/// Rewrite every ref in `formula` to reflect what the formula should
/// look like **after** the given rows/cols have been deleted. This is
/// the Google-Sheets-matching behavior:
///
/// - Refs into deleted cells become `#REF!`.
/// - Refs to surviving cells *shift* to their new index (e.g. `D1` +
///   del col B → `C1`).
/// - Ranges get *trimmed* to cover only surviving cells (e.g.
///   `A1:D1` + del col B → `A1:C1`; `A1:D1` + del col D → `A1:C1`).
/// - Ranges where no cell survives become `#REF!`.
/// - Refs unaffected by the deletion keep their original bytes
///   (preserving case).
///
/// This is strictly more powerful than [`rewrite_refs_for_deletion`];
/// callers who only want the conservative `#REF!`-stamping behavior
/// should use that function instead.
///
/// Non-formula input passes through unchanged. Tokenization failure
/// (e.g. the output of a previous rewrite containing `#REF!`) also
/// passes through unchanged.
///
/// Note that this function is **not** idempotent when a second
/// application of the same deletion would further shift survivors —
/// running it twice semantically represents two sequential deletions.
pub fn adjust_refs_for_deletion<'a>(
    formula: &'a str,
    deletion: &Deletion,
) -> Cow<'a, str> {
    if !formula.starts_with('=') {
        return Cow::Borrowed(formula);
    }
    if deletion.cols.is_empty() && deletion.rows.is_empty() {
        return Cow::Borrowed(formula);
    }

    let tokens = match Lexer::new(formula).tokenize() {
        Ok(t) => t,
        Err(_) => return Cow::Borrowed(formula),
    };

    let cols: HashSet<u32> = deletion.cols.iter().copied().collect();
    let rows: HashSet<u32> = deletion.rows.iter().copied().collect();

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let t = &tokens[i];

        // A1-range or single cell.
        if t.token_type == TokenType::CellRef {
            if i + 2 < tokens.len()
                && tokens[i + 1].token_type == TokenType::Colon
                && tokens[i + 2].token_type == TokenType::CellRef
            {
                let end_tok = &tokens[i + 2];
                if let (Some((scol, srow)), Some((ecol, erow))) =
                    (parse_ref_idx(&t.value), parse_ref_idx(&end_tok.value))
                {
                    let (min_c, max_c) = (scol.min(ecol), scol.max(ecol));
                    let (min_r, max_r) = (srow.min(erow), srow.max(erow));
                    let col_trim = trim_and_shift_range(min_c, max_c, &cols);
                    let row_trim = trim_and_shift_range(min_r, max_r, &rows);
                    let full_start = t.position + 1;
                    let full_end = end_tok.position + end_tok.source_len + 1;
                    match (col_trim, row_trim) {
                        (Some((cs, ce)), Some((rs, re))) => {
                            if (cs, ce) != (min_c, max_c) || (rs, re) != (min_r, max_r) {
                                let rep = format!(
                                    "{}:{}",
                                    format_cell(cs, rs),
                                    format_cell(ce, re)
                                );
                                edits.push((full_start, full_end, rep));
                            }
                        }
                        _ => edits.push((full_start, full_end, REF_ERROR.to_string())),
                    }
                }
                i += 3;
                continue;
            }
            // Single cell.
            if let Some((col, row)) = parse_ref_idx(&t.value) {
                let start = t.position + 1;
                let end = start + t.source_len;
                if cols.contains(&col) || rows.contains(&row) {
                    edits.push((start, end, REF_ERROR.to_string()));
                } else {
                    let col_shift = cols.iter().filter(|&&d| d < col).count() as u32;
                    let row_shift = rows.iter().filter(|&&d| d < row).count() as u32;
                    if col_shift > 0 || row_shift > 0 {
                        edits.push((start, end, format_cell(col - col_shift, row - row_shift)));
                    }
                }
            }
            i += 1;
            continue;
        }

        // Whole-column range.
        if t.token_type == TokenType::Function
            && is_pure_letters(&t.value)
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Function
            && is_pure_letters(&tokens[i + 2].value)
        {
            let end_tok = &tokens[i + 2];
            let scol = col_to_index(&t.value).saturating_sub(1);
            let ecol = col_to_index(&end_tok.value).saturating_sub(1);
            let (min_c, max_c) = (scol.min(ecol), scol.max(ecol));
            let full_start = t.position + 1;
            let full_end = end_tok.position + end_tok.source_len + 1;
            match trim_and_shift_range(min_c, max_c, &cols) {
                Some((cs, ce)) if (cs, ce) != (min_c, max_c) => {
                    let rep = format!("{}:{}", index_to_col(cs + 1), index_to_col(ce + 1));
                    edits.push((full_start, full_end, rep));
                }
                None => edits.push((full_start, full_end, REF_ERROR.to_string())),
                _ => {}
            }
            i += 3;
            continue;
        }

        // Whole-row range.
        if t.token_type == TokenType::Number
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Number
        {
            let end_tok = &tokens[i + 2];
            if let (Ok(sr), Ok(er)) = (t.value.parse::<u32>(), end_tok.value.parse::<u32>()) {
                if sr > 0 && er > 0 {
                    let srow = sr - 1;
                    let erow = er - 1;
                    let (min_r, max_r) = (srow.min(erow), srow.max(erow));
                    let full_start = t.position + 1;
                    let full_end = end_tok.position + end_tok.source_len + 1;
                    match trim_and_shift_range(min_r, max_r, &rows) {
                        Some((rs, re)) if (rs, re) != (min_r, max_r) => {
                            let rep = format!("{}:{}", rs + 1, re + 1);
                            edits.push((full_start, full_end, rep));
                        }
                        None => edits.push((full_start, full_end, REF_ERROR.to_string())),
                        _ => {}
                    }
                }
            }
            i += 3;
            continue;
        }

        i += 1;
    }

    if edits.is_empty() {
        return Cow::Borrowed(formula);
    }

    let mut out = String::with_capacity(formula.len());
    let mut cursor = 0;
    for (s, e, rep) in &edits {
        out.push_str(&formula[cursor..*s]);
        out.push_str(rep);
        cursor = *e;
    }
    out.push_str(&formula[cursor..]);
    Cow::Owned(out)
}

/// Rewrite every ref in `formula` to reflect what the formula should
/// look like **after** the given rows/cols have been inserted. This
/// is the mirror of [`adjust_refs_for_deletion`]:
///
/// - Refs at or past the insertion point shift outward by the count
///   of insertions at-or-before their index. Refs before every
///   insertion are untouched.
/// - Absolute components (`$A`, `A$1`, `$1`, `$A:$C`) follow the
///   same positional shift and keep their `$` markers — the `$` pins
///   the ref to a cell, not to a literal index.
/// - Ranges whose endpoints straddle an insertion grow to include
///   the new blank col/row (matches GSheets "insert inside a range
///   expands the range").
/// - Whole-column / whole-row refs (`A:A`, `1:1`) shift the bounded
///   axis and leave the unbounded axis alone.
/// - Refs unaffected by the insertion keep their original bytes
///   (preserving case).
///
/// Insertion only shifts refs forward, never off the grid, so no
/// `#REF!` case arises.
///
/// Non-formula input (doesn't start with `=`) passes through
/// unchanged. Tokenization failure also passes through unchanged.
///
/// Applying the same insertion twice semantically represents two
/// sequential insertions (refs shift twice).
pub fn adjust_refs_for_insertion<'a>(
    formula: &'a str,
    insertion: &Insertion,
) -> Cow<'a, str> {
    if !formula.starts_with('=') {
        return Cow::Borrowed(formula);
    }
    if insertion.cols.is_empty() && insertion.rows.is_empty() {
        return Cow::Borrowed(formula);
    }

    let tokens = match Lexer::new(formula).tokenize() {
        Ok(t) => t,
        Err(_) => return Cow::Borrowed(formula),
    };

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let t = &tokens[i];

        // A1-range or single cell.
        if t.token_type == TokenType::CellRef {
            if i + 2 < tokens.len()
                && tokens[i + 1].token_type == TokenType::Colon
                && tokens[i + 2].token_type == TokenType::CellRef
            {
                let end_tok = &tokens[i + 2];
                if let (Some((scol, srow)), Some((ecol, erow))) =
                    (parse_ref_idx(&t.value), parse_ref_idx(&end_tok.value))
                {
                    let full_start = t.position + 1;
                    let full_end = end_tok.position + end_tok.source_len + 1;
                    let s_src = &formula[full_start..t.position + 1 + t.source_len];
                    let e_src = &formula[end_tok.position + 1..full_end];
                    let (s_col_abs, s_row_abs) = cell_abs_from_source(s_src);
                    let (e_col_abs, e_row_abs) = cell_abs_from_source(e_src);
                    let new_scol = shift_for_insertion(scol, &insertion.cols);
                    let new_srow = shift_for_insertion(srow, &insertion.rows);
                    let new_ecol = shift_for_insertion(ecol, &insertion.cols);
                    let new_erow = shift_for_insertion(erow, &insertion.rows);
                    if (new_scol, new_srow, new_ecol, new_erow) != (scol, srow, ecol, erow) {
                        let mut rep = String::new();
                        write_cell_indexed(
                            s_col_abs, new_scol, s_row_abs, new_srow, &mut rep,
                        );
                        rep.push(':');
                        write_cell_indexed(
                            e_col_abs, new_ecol, e_row_abs, new_erow, &mut rep,
                        );
                        edits.push((full_start, full_end, rep));
                    }
                }
                i += 3;
                continue;
            }
            // Single cell.
            if let Some((col, row)) = parse_ref_idx(&t.value) {
                let start = t.position + 1;
                let end = start + t.source_len;
                let (col_abs, row_abs) = cell_abs_from_source(&formula[start..end]);
                let new_col = shift_for_insertion(col, &insertion.cols);
                let new_row = shift_for_insertion(row, &insertion.rows);
                if new_col != col || new_row != row {
                    let mut rep = String::new();
                    write_cell_indexed(col_abs, new_col, row_abs, new_row, &mut rep);
                    edits.push((start, end, rep));
                }
            }
            i += 1;
            continue;
        }

        // Whole-column range.
        if t.token_type == TokenType::Function
            && is_pure_letters(&t.value)
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Function
            && is_pure_letters(&tokens[i + 2].value)
        {
            let end_tok = &tokens[i + 2];
            let scol = col_to_index(&t.value).saturating_sub(1);
            let ecol = col_to_index(&end_tok.value).saturating_sub(1);
            let full_start = t.position + 1;
            let full_end = end_tok.position + end_tok.source_len + 1;
            let s_src = &formula[full_start..t.position + 1 + t.source_len];
            let e_src = &formula[end_tok.position + 1..full_end];
            let s_abs = s_src.starts_with('$');
            let e_abs = e_src.starts_with('$');
            let new_scol = shift_for_insertion(scol, &insertion.cols);
            let new_ecol = shift_for_insertion(ecol, &insertion.cols);
            if (new_scol, new_ecol) != (scol, ecol) {
                let mut rep = String::new();
                if s_abs {
                    rep.push('$');
                }
                rep.push_str(&index_to_col(new_scol + 1));
                rep.push(':');
                if e_abs {
                    rep.push('$');
                }
                rep.push_str(&index_to_col(new_ecol + 1));
                edits.push((full_start, full_end, rep));
            }
            i += 3;
            continue;
        }

        // Whole-row range.
        if t.token_type == TokenType::Number
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Number
        {
            let end_tok = &tokens[i + 2];
            if let (Ok(sr), Ok(er)) = (t.value.parse::<u32>(), end_tok.value.parse::<u32>()) {
                if sr > 0 && er > 0 {
                    let srow = sr - 1;
                    let erow = er - 1;
                    let full_start = t.position + 1;
                    let full_end = end_tok.position + end_tok.source_len + 1;
                    let s_src = &formula[full_start..t.position + 1 + t.source_len];
                    let e_src = &formula[end_tok.position + 1..full_end];
                    let s_abs = s_src.starts_with('$');
                    let e_abs = e_src.starts_with('$');
                    let new_srow = shift_for_insertion(srow, &insertion.rows);
                    let new_erow = shift_for_insertion(erow, &insertion.rows);
                    if (new_srow, new_erow) != (srow, erow) {
                        let mut rep = String::new();
                        if s_abs {
                            rep.push('$');
                        }
                        rep.push_str(&(new_srow + 1).to_string());
                        rep.push(':');
                        if e_abs {
                            rep.push('$');
                        }
                        rep.push_str(&(new_erow + 1).to_string());
                        edits.push((full_start, full_end, rep));
                    }
                }
            }
            i += 3;
            continue;
        }

        i += 1;
    }

    if edits.is_empty() {
        return Cow::Borrowed(formula);
    }

    let mut out = String::with_capacity(formula.len());
    let mut cursor = 0;
    for (s, e, rep) in &edits {
        out.push_str(&formula[cursor..*s]);
        out.push_str(rep);
        cursor = *e;
    }
    out.push_str(&formula[cursor..]);
    Cow::Owned(out)
}

/// Return the new 0-based index for a ref at `idx` after `inserted`
/// blank rows/cols have been inserted. Insertion indices at-or-before
/// `idx` each push it forward by one.
fn shift_for_insertion(idx: u32, inserted: &[u32]) -> u32 {
    let shift = inserted.iter().filter(|&&i| i <= idx).count() as u32;
    idx + shift
}

/// How a column- or row-block-move should treat bounded ranges
/// (`CellRef:CellRef`, e.g. `A1:D5`).
#[derive(Copy, Clone, PartialEq, Eq)]
enum BoundedRangeStrategy {
    /// Leave bounded ranges unchanged. The rectangle stays put even
    /// though its data permutes inside. Used by cell formulas like
    /// `=SUM(A1:D5)` where the range denotes a positional rectangle.
    Positional,
    /// Rewrite bounded ranges via the same interior-bbox semantics
    /// the whole-column / whole-row branch uses. Used by named-range
    /// definitions where the range denotes the named cells, which
    /// should follow the data when columns or rows move.
    DataFollowing,
}

/// Rewrite every column-bearing ref in `formula` to reflect a
/// contiguous block of columns `[src_start, src_end]` (0-based,
/// inclusive) moving to land starting at `final_start` in the
/// post-move layout. Width = `src_end - src_start + 1`. Handles both
/// single-column moves (`src_start == src_end`) and multi-column
/// block moves.
///
/// Block move is a permutation — no `#REF!` is ever produced.
/// Non-formula input passes through unchanged. Tokenization failure,
/// `final_start == src_start`, and `src_end < src_start` all
/// short-circuit to `Cow::Borrowed(formula)` (defensive parity with
/// `adjust_refs_for_insertion` / `adjust_refs_for_deletion`).
///
/// **Bounded vs. whole-col asymmetry** — bounded ranges (`A1:D5`)
/// are positional and stay put; the rectangle keeps its corners
/// even though its data permutes inside. Whole-column ranges
/// (`B:D`) follow their data: the new range is the bounding box of
/// every interior column's image under the forward map. See the
/// crate-level TODO for the worked-example justification — endpoint
/// mapping alone gives wrong answers when the move straddles the
/// range.
///
/// For a variant that *also* rewrites bounded ranges via interior
/// bbox (used for named-range definitions, where the range denotes
/// named cells rather than a positional rectangle), see
/// [`adjust_refs_for_column_block_move_data_following`].
pub fn adjust_refs_for_column_block_move<'a>(
    formula: &'a str,
    src_start: u32,
    src_end: u32,
    final_start: u32,
) -> Cow<'a, str> {
    column_block_move_impl(
        formula,
        src_start,
        src_end,
        final_start,
        BoundedRangeStrategy::Positional,
    )
}

/// Same shape as [`adjust_refs_for_column_block_move`] BUT bounded
/// ranges (`CellRef:CellRef`, e.g. `A1:D5`) are rewritten using the
/// same interior-bbox semantics applied to whole-column ranges,
/// rather than left in place.
///
/// Use this on text where bounded ranges denote *named cells*
/// (named-range definitions) rather than *positional rectangles*
/// (cell formulas). Single-cell, whole-column, whole-row,
/// absolute-marker, and spill-anchor handling is identical to
/// [`adjust_refs_for_column_block_move`]. No `#REF!` case — block
/// move is a permutation.
///
/// Don't merge this with the sibling function: cell formulas and
/// named-range definitions need different bounded-range semantics
/// and the call sites should be explicit about which they want.
pub fn adjust_refs_for_column_block_move_data_following<'a>(
    formula: &'a str,
    src_start: u32,
    src_end: u32,
    final_start: u32,
) -> Cow<'a, str> {
    column_block_move_impl(
        formula,
        src_start,
        src_end,
        final_start,
        BoundedRangeStrategy::DataFollowing,
    )
}

fn column_block_move_impl<'a>(
    formula: &'a str,
    src_start: u32,
    src_end: u32,
    final_start: u32,
    bounded: BoundedRangeStrategy,
) -> Cow<'a, str> {
    if !formula.starts_with('=') {
        return Cow::Borrowed(formula);
    }
    if src_end < src_start || final_start == src_start {
        return Cow::Borrowed(formula);
    }

    let tokens = match Lexer::new(formula).tokenize() {
        Ok(t) => t,
        Err(_) => return Cow::Borrowed(formula),
    };

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let t = &tokens[i];

        // Bounded A1-range. Skip past it (no edit) under Positional;
        // rewrite via interior bbox under DataFollowing. Either way
        // we advance past the three tokens so the lone-CellRef branch
        // doesn't fire on the start cell.
        if t.token_type == TokenType::CellRef
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::CellRef
        {
            if bounded == BoundedRangeStrategy::DataFollowing {
                let end_tok = &tokens[i + 2];
                if let (Some((scol, srow)), Some((ecol, erow))) =
                    (parse_ref_idx(&t.value), parse_ref_idx(&end_tok.value))
                {
                    let full_start = t.position + 1;
                    let full_end = end_tok.position + end_tok.source_len + 1;
                    let s_src = &formula[full_start..t.position + 1 + t.source_len];
                    let e_src = &formula[end_tok.position + 1..full_end];
                    let (s_col_abs, s_row_abs) = cell_abs_from_source(s_src);
                    let (e_col_abs, e_row_abs) = cell_abs_from_source(e_src);
                    let lo_in = scol.min(ecol);
                    let hi_in = scol.max(ecol);
                    let (lo_col, hi_col) = (lo_in..=hi_in)
                        .map(|c| forward_col(c, src_start, src_end, final_start))
                        .fold((u32::MAX, 0_u32), |(lo, hi), c| (lo.min(c), hi.max(c)));
                    if (lo_col, hi_col) != (lo_in, hi_in) {
                        let mut rep = String::new();
                        write_cell_indexed(s_col_abs, lo_col, s_row_abs, srow, &mut rep);
                        rep.push(':');
                        write_cell_indexed(e_col_abs, hi_col, e_row_abs, erow, &mut rep);
                        edits.push((full_start, full_end, rep));
                    }
                }
            }
            i += 3;
            continue;
        }

        // Single cell. Spill anchor `A1#` rides through naturally —
        // the trailing `Hash` token is left untouched on the next pass.
        if t.token_type == TokenType::CellRef {
            if let Some((col, row)) = parse_ref_idx(&t.value) {
                let start = t.position + 1;
                let end = start + t.source_len;
                let (col_abs, row_abs) = cell_abs_from_source(&formula[start..end]);
                let new_col = forward_col(col, src_start, src_end, final_start);
                if new_col != col {
                    let mut rep = String::new();
                    write_cell_indexed(col_abs, new_col, row_abs, row, &mut rep);
                    edits.push((start, end, rep));
                }
            }
            i += 1;
            continue;
        }

        // Whole-column range (e.g. `B:D`). Interior bbox — see doc note.
        if t.token_type == TokenType::Function
            && is_pure_letters(&t.value)
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Function
            && is_pure_letters(&tokens[i + 2].value)
        {
            let end_tok = &tokens[i + 2];
            let scol = col_to_index(&t.value).saturating_sub(1);
            let ecol = col_to_index(&end_tok.value).saturating_sub(1);
            let full_start = t.position + 1;
            let full_end = end_tok.position + end_tok.source_len + 1;
            let s_src = &formula[full_start..t.position + 1 + t.source_len];
            let e_src = &formula[end_tok.position + 1..full_end];
            let s_abs = s_src.starts_with('$');
            let e_abs = e_src.starts_with('$');
            let (lo_col, hi_col) = (scol.min(ecol)..=scol.max(ecol))
                .map(|c| forward_col(c, src_start, src_end, final_start))
                .fold((u32::MAX, 0_u32), |(lo, hi), c| (lo.min(c), hi.max(c)));
            if (lo_col, hi_col) != (scol.min(ecol), scol.max(ecol)) {
                let mut rep = String::new();
                if s_abs {
                    rep.push('$');
                }
                rep.push_str(&index_to_col(lo_col + 1));
                rep.push(':');
                if e_abs {
                    rep.push('$');
                }
                rep.push_str(&index_to_col(hi_col + 1));
                edits.push((full_start, full_end, rep));
            }
            i += 3;
            continue;
        }

        // Whole-row range — column move doesn't affect rows. Skip past
        // so the Number tokens don't trigger any other branch.
        if t.token_type == TokenType::Number
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Number
        {
            i += 3;
            continue;
        }

        i += 1;
    }

    if edits.is_empty() {
        return Cow::Borrowed(formula);
    }

    let mut out = String::with_capacity(formula.len());
    let mut cursor = 0;
    for (s, e, rep) in &edits {
        out.push_str(&formula[cursor..*s]);
        out.push_str(rep);
        cursor = *e;
    }
    out.push_str(&formula[cursor..]);
    Cow::Owned(out)
}

/// Forward map for a column-block move. See
/// [`adjust_refs_for_column_block_move`].
fn forward_col(c: u32, src_start: u32, src_end: u32, final_start: u32) -> u32 {
    let width = src_end - src_start + 1;
    if c >= src_start && c <= src_end {
        return c - src_start + final_start;
    }
    if final_start < src_start {
        if c >= final_start && c < src_start {
            c + width
        } else {
            c
        }
    } else if c > src_end && c < final_start + width {
        c - width
    } else {
        c
    }
}

/// Rewrite every row-bearing ref in `formula` to reflect a
/// contiguous block of rows `[src_start, src_end]` (0-based,
/// inclusive) moving to land starting at `final_start` in the
/// post-move layout. Width = `src_end - src_start + 1`. Handles both
/// single-row moves (`src_start == src_end`) and multi-row block
/// moves.
///
/// Block move is a permutation — no `#REF!` is ever produced.
/// Non-formula input passes through unchanged. Tokenization failure,
/// `final_start == src_start`, and `src_end < src_start` all
/// short-circuit to `Cow::Borrowed(formula)` (defensive parity with
/// the column-axis siblings).
///
/// **Bounded vs. whole-row asymmetry** — bounded ranges (`A1:D5`)
/// are positional and stay put; the rectangle keeps its corners
/// even though its data permutes inside. Whole-row ranges (`3:5`)
/// follow their data: the new range is the bounding box of every
/// interior row's image under the forward map. Mirror of
/// [`adjust_refs_for_column_block_move`] on the row axis;
/// whole-column ranges (`A:C`) are unaffected by a row move.
///
/// For a variant that *also* rewrites bounded ranges via interior
/// bbox (used for named-range definitions, where the range denotes
/// named cells rather than a positional rectangle), see
/// [`adjust_refs_for_row_block_move_data_following`].
pub fn adjust_refs_for_row_block_move<'a>(
    formula: &'a str,
    src_start: u32,
    src_end: u32,
    final_start: u32,
) -> Cow<'a, str> {
    row_block_move_impl(
        formula,
        src_start,
        src_end,
        final_start,
        BoundedRangeStrategy::Positional,
    )
}

/// Same shape as [`adjust_refs_for_row_block_move`] BUT bounded
/// ranges (`CellRef:CellRef`, e.g. `A1:D5`) are rewritten using the
/// same interior-bbox semantics applied to whole-row ranges, rather
/// than left in place.
///
/// Use this on text where bounded ranges denote *named cells*
/// (named-range definitions) rather than *positional rectangles*
/// (cell formulas). Single-cell, whole-column, whole-row,
/// absolute-marker, and spill-anchor handling is identical to
/// [`adjust_refs_for_row_block_move`]. No `#REF!` case — block move
/// is a permutation. Mirror of
/// [`adjust_refs_for_column_block_move_data_following`] on the row
/// axis.
///
/// Don't merge this with the sibling function: cell formulas and
/// named-range definitions need different bounded-range semantics
/// and the call sites should be explicit about which they want.
pub fn adjust_refs_for_row_block_move_data_following<'a>(
    formula: &'a str,
    src_start: u32,
    src_end: u32,
    final_start: u32,
) -> Cow<'a, str> {
    row_block_move_impl(
        formula,
        src_start,
        src_end,
        final_start,
        BoundedRangeStrategy::DataFollowing,
    )
}

fn row_block_move_impl<'a>(
    formula: &'a str,
    src_start: u32,
    src_end: u32,
    final_start: u32,
    bounded: BoundedRangeStrategy,
) -> Cow<'a, str> {
    if !formula.starts_with('=') {
        return Cow::Borrowed(formula);
    }
    if src_end < src_start || final_start == src_start {
        return Cow::Borrowed(formula);
    }

    let tokens = match Lexer::new(formula).tokenize() {
        Ok(t) => t,
        Err(_) => return Cow::Borrowed(formula),
    };

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let t = &tokens[i];

        // Bounded A1-range. Skip past it (no edit) under Positional;
        // rewrite via interior bbox under DataFollowing. Either way
        // we advance past the three tokens so the lone-CellRef branch
        // doesn't fire on the start cell.
        if t.token_type == TokenType::CellRef
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::CellRef
        {
            if bounded == BoundedRangeStrategy::DataFollowing {
                let end_tok = &tokens[i + 2];
                if let (Some((scol, srow)), Some((ecol, erow))) =
                    (parse_ref_idx(&t.value), parse_ref_idx(&end_tok.value))
                {
                    let full_start = t.position + 1;
                    let full_end = end_tok.position + end_tok.source_len + 1;
                    let s_src = &formula[full_start..t.position + 1 + t.source_len];
                    let e_src = &formula[end_tok.position + 1..full_end];
                    let (s_col_abs, s_row_abs) = cell_abs_from_source(s_src);
                    let (e_col_abs, e_row_abs) = cell_abs_from_source(e_src);
                    let lo_in = srow.min(erow);
                    let hi_in = srow.max(erow);
                    let (lo_row, hi_row) = (lo_in..=hi_in)
                        .map(|r| forward_row(r, src_start, src_end, final_start))
                        .fold((u32::MAX, 0_u32), |(lo, hi), r| (lo.min(r), hi.max(r)));
                    if (lo_row, hi_row) != (lo_in, hi_in) {
                        let mut rep = String::new();
                        write_cell_indexed(s_col_abs, scol, s_row_abs, lo_row, &mut rep);
                        rep.push(':');
                        write_cell_indexed(e_col_abs, ecol, e_row_abs, hi_row, &mut rep);
                        edits.push((full_start, full_end, rep));
                    }
                }
            }
            i += 3;
            continue;
        }

        // Single cell. Spill anchor `A1#` rides through naturally —
        // the trailing `Hash` token is left untouched on the next pass.
        if t.token_type == TokenType::CellRef {
            if let Some((col, row)) = parse_ref_idx(&t.value) {
                let start = t.position + 1;
                let end = start + t.source_len;
                let (col_abs, row_abs) = cell_abs_from_source(&formula[start..end]);
                let new_row = forward_row(row, src_start, src_end, final_start);
                if new_row != row {
                    let mut rep = String::new();
                    write_cell_indexed(col_abs, col, row_abs, new_row, &mut rep);
                    edits.push((start, end, rep));
                }
            }
            i += 1;
            continue;
        }

        // Whole-column range (e.g. `B:D`). Unaffected by a row move —
        // skip past so the Function tokens don't trigger any other
        // branch.
        if t.token_type == TokenType::Function
            && is_pure_letters(&t.value)
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Function
            && is_pure_letters(&tokens[i + 2].value)
        {
            i += 3;
            continue;
        }

        // Whole-row range (e.g. `3:5`). Interior bbox — see doc note.
        if t.token_type == TokenType::Number
            && i + 2 < tokens.len()
            && tokens[i + 1].token_type == TokenType::Colon
            && tokens[i + 2].token_type == TokenType::Number
        {
            let end_tok = &tokens[i + 2];
            if let (Ok(srow1), Ok(erow1)) =
                (t.value.parse::<u32>(), end_tok.value.parse::<u32>())
            {
                if srow1 >= 1 && erow1 >= 1 {
                    let srow = srow1 - 1;
                    let erow = erow1 - 1;
                    let full_start = t.position + 1;
                    let full_end = end_tok.position + end_tok.source_len + 1;
                    let s_src = &formula[full_start..t.position + 1 + t.source_len];
                    let e_src = &formula[end_tok.position + 1..full_end];
                    let s_abs = s_src.starts_with('$');
                    let e_abs = e_src.starts_with('$');
                    let lo_in = srow.min(erow);
                    let hi_in = srow.max(erow);
                    let (lo_row, hi_row) = (lo_in..=hi_in)
                        .map(|r| forward_row(r, src_start, src_end, final_start))
                        .fold((u32::MAX, 0_u32), |(lo, hi), r| (lo.min(r), hi.max(r)));
                    if (lo_row, hi_row) != (lo_in, hi_in) {
                        let mut rep = String::new();
                        if s_abs {
                            rep.push('$');
                        }
                        rep.push_str(&(lo_row + 1).to_string());
                        rep.push(':');
                        if e_abs {
                            rep.push('$');
                        }
                        rep.push_str(&(hi_row + 1).to_string());
                        edits.push((full_start, full_end, rep));
                    }
                }
            }
            i += 3;
            continue;
        }

        i += 1;
    }

    if edits.is_empty() {
        return Cow::Borrowed(formula);
    }

    let mut out = String::with_capacity(formula.len());
    let mut cursor = 0;
    for (s, e, rep) in &edits {
        out.push_str(&formula[cursor..*s]);
        out.push_str(rep);
        cursor = *e;
    }
    out.push_str(&formula[cursor..]);
    Cow::Owned(out)
}

/// Forward map for a row-block move. See
/// [`adjust_refs_for_row_block_move`].
fn forward_row(r: u32, src_start: u32, src_end: u32, final_start: u32) -> u32 {
    let width = src_end - src_start + 1;
    if r >= src_start && r <= src_end {
        return r - src_start + final_start;
    }
    if final_start < src_start {
        if r >= final_start && r < src_start {
            r + width
        } else {
            r
        }
    } else if r > src_end && r < final_start + width {
        r - width
    } else {
        r
    }
}

/// Inspect a cell-ref source slice (e.g. `$A$1`, `B2`) and report
/// whether the column and row components carry `$` markers.
fn cell_abs_from_source(source: &str) -> (bool, bool) {
    let bytes = source.as_bytes();
    let col_abs = bytes.first() == Some(&b'$');
    let mut pos = if col_abs { 1 } else { 0 };
    while pos < bytes.len() && bytes[pos].is_ascii_alphabetic() {
        pos += 1;
    }
    let row_abs = bytes.get(pos) == Some(&b'$');
    (col_abs, row_abs)
}

/// Write a cell ref from 0-based indices, preserving absoluteness.
fn write_cell_indexed(
    col_abs: bool,
    col_idx: u32,
    row_abs: bool,
    row_idx: u32,
    out: &mut String,
) {
    if col_abs {
        out.push('$');
    }
    out.push_str(&index_to_col(col_idx + 1));
    if row_abs {
        out.push('$');
    }
    out.push_str(&(row_idx + 1).to_string());
}

/// Given a closed 0-based index range `[start, end]`, return the
/// `(new_start, new_end)` describing the surviving sub-range after
/// `deleted` indices have been removed and remaining indices shifted
/// to close the gaps. Returns `None` if every index in the range
/// is deleted.
fn trim_and_shift_range(
    start: u32,
    end: u32,
    deleted: &HashSet<u32>,
) -> Option<(u32, u32)> {
    debug_assert!(start <= end);
    // Advance start to the first surviving index in [start, end].
    let mut new_start = start;
    while new_start <= end && deleted.contains(&new_start) {
        if new_start == u32::MAX {
            return None;
        }
        new_start += 1;
    }
    if new_start > end {
        return None;
    }
    // Retreat end to the last surviving index in [new_start, end].
    let mut new_end = end;
    while new_end >= new_start && deleted.contains(&new_end) {
        if new_end == 0 {
            return None;
        }
        new_end -= 1;
    }
    if new_end < new_start {
        return None;
    }
    let shift_start = deleted.iter().filter(|&&d| d < new_start).count() as u32;
    let shift_end = deleted.iter().filter(|&&d| d < new_end).count() as u32;
    Some((new_start - shift_start, new_end - shift_end))
}

fn parse_ref_idx(cell: &str) -> Option<(u32, u32)> {
    let (col, row) = parse_ref(cell)?;
    if row == 0 {
        return None;
    }
    let col_idx = col_to_index(&col);
    if col_idx == u32::MAX {
        // Saturated — letters overflowed u32. Treat as no coord rather
        // than pointing at column 4 billion.
        return None;
    }
    Some((col_idx - 1, row - 1))
}

/// Parse a cell-ref token's value (e.g. `"A1"`) into a zero-based coord.
/// Returns `None` only for malformed input — by lexer invariant a
/// `CellRef` token always produces `Some`.
fn coord_of(value: &str) -> Option<CellCoord> {
    parse_ref_idx(value).map(|(col, row)| CellCoord { row, col })
}

fn format_cell(col_idx: u32, row_idx: u32) -> String {
    format!("{}{}", index_to_col(col_idx + 1), row_idx + 1)
}

/// Expand a range like "A1":"B3" into the full rectangle of zero-based
/// [`CellCoord`]s. Returns an empty vec if either endpoint fails to parse
/// or if the rectangle exceeds `MAX_RANGE_CELLS`.
fn expand_range(start_ref: &str, end_ref: &str) -> Vec<CellCoord> {
    let Some((s_col, s_row)) = parse_ref(start_ref).map(|(c, r)| (col_to_index(&c), r)) else {
        return vec![];
    };
    let Some((e_col, e_row)) = parse_ref(end_ref).map(|(c, r)| (col_to_index(&c), r)) else {
        return vec![];
    };

    let min_col = s_col.min(e_col);
    let max_col = s_col.max(e_col);
    let min_row = s_row.min(e_row);
    let max_row = s_row.max(e_row);

    let rows = (max_row - min_row + 1) as usize;
    let cols = (max_col - min_col + 1) as usize;
    let total = rows.saturating_mul(cols);
    if total > MAX_RANGE_CELLS {
        return vec![];
    }

    let mut cells = Vec::with_capacity(total);
    for r in min_row..=max_row {
        for c in min_col..=max_col {
            // parse_ref returned 1-based; CellCoord is 0-based.
            cells.push(CellCoord { row: r - 1, col: c - 1 });
        }
    }
    cells
}

/// Parse "A1" into ("A".to_string(), 1).
fn parse_ref(r: &str) -> Option<(String, u32)> {
    let col_end = r.find(|c: char| c.is_ascii_digit())?;
    let col = &r[..col_end];
    if col.is_empty() || !col.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let row: u32 = r[col_end..].parse().ok()?;
    Some((col.to_string(), row))
}

/// Shift every *relative* cell/range reference in `formula` by
/// `(d_row, d_col)`. Components preceded by `$` in the source are
/// absolute and do not shift. References that would land outside the
/// `1..=max_row` / `1..=max_col` grid are replaced with `#REF!`.
///
/// For a cell range or whole-column/whole-row range, if *any* endpoint
/// would land off-grid the entire range collapses to a single `#REF!`
/// token (matches the test-table choice of "full collapse").
///
/// Non-formula input (no leading `=`) is returned unchanged. A zero
/// delta is a no-op. Named ranges, function names, numeric literals,
/// and other tokens are passed through verbatim, preserving their
/// original case/whitespace.
pub fn shift_formula_refs(
    formula: &str,
    d_row: i32,
    d_col: i32,
    max_row: u32,
    max_col: u32,
) -> String {
    if !formula.starts_with('=') {
        return formula.to_string();
    }
    if d_row == 0 && d_col == 0 {
        return formula.to_string();
    }
    if max_row == 0 || max_col == 0 {
        return formula.to_string();
    }

    let shift = Shift {
        d_row,
        d_col,
        max_row,
        max_col,
    };

    let body = &formula[1..];
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(formula.len() + 4);
    out.push('=');
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];

        // String literal: copy the full quoted span, including quotes.
        if b == b'"' || b == b'\'' {
            let quote = b;
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            out.push_str(&body[start..i]);
            continue;
        }

        // A ref-like span starts with `$`, a letter, or a digit.
        if b == b'$' || b.is_ascii_alphabetic() || b.is_ascii_digit() {
            if let Some(consumed) = try_emit_ref(body, i, &shift, &mut out) {
                i += consumed;
                continue;
            }
            // Not a recognized ref — consume the whole token verbatim so
            // we don't retry matching from the middle of it.
            let end = if b == b'$' {
                i + 1
            } else {
                skip_word(body, i)
            };
            out.push_str(&body[i..end]);
            i = end;
            continue;
        }

        // Operators / punctuation / whitespace pass through.
        out.push(b as char);
        i += 1;
    }

    out
}

/// Attempt to recognize and emit a ref pattern starting at `body[start]`.
/// On success writes the rewritten (or pass-through) text to `out` and
/// returns the number of bytes consumed from `start`.
fn try_emit_ref(body: &str, start: usize, shift: &Shift, out: &mut String) -> Option<usize> {
    let bytes = body.as_bytes();
    let first = bytes[start];

    if first == b'$' || first.is_ascii_alphabetic() {
        // Cell ref or cell-cell range or spill ref.
        if let Some(cell) = parse_cell(body, start) {
            let after = start + cell.total_len;

            // cell : cell → range
            if bytes.get(after) == Some(&b':') {
                if let Some(cell2) = parse_cell(body, after + 1) {
                    let total = after + 1 + cell2.total_len - start;
                    emit_cell_range(body, start, total, &cell, &cell2, shift, out);
                    return Some(total);
                }
            }

            // cell # → spill ref
            if bytes.get(after) == Some(&b'#') {
                let total = after + 1 - start;
                emit_spill_ref(body, start, total, &cell, shift, out);
                return Some(total);
            }

            // plain cell
            let total = cell.total_len;
            emit_cell(body, start, total, &cell, shift, out);
            return Some(total);
        }

        // Whole-column range: $?letters : $?letters (no digits on either side).
        if let Some(cols) = parse_col_letters(body, start) {
            let after = start + cols.total_len;
            if bytes.get(after) == Some(&b':') {
                if let Some(cols2) = parse_col_letters(body, after + 1) {
                    let total = after + 1 + cols2.total_len - start;
                    emit_col_range(body, start, total, &cols, &cols2, shift, out);
                    return Some(total);
                }
            }
        }
        return None;
    }

    if first.is_ascii_digit() {
        // Whole-row range: digits : digits.
        if let Some(rn) = parse_row_num(body, start) {
            let after = start + rn.total_len;
            if bytes.get(after) == Some(&b':') {
                if let Some(rn2) = parse_row_num(body, after + 1) {
                    let total = after + 1 + rn2.total_len - start;
                    emit_row_range(body, start, total, &rn, &rn2, shift, out);
                    return Some(total);
                }
            }
        }
    }

    None
}

/// A `$?letters$?digits` cell ref.
struct CellParse {
    col_abs: bool,
    col: u32, // 1-based
    row_abs: bool,
    row: u32, // 1-based
    total_len: usize,
}

/// `$?letters` (not followed by digits) — used for whole-column endpoints.
struct ColLetters {
    abs: bool,
    col: u32, // 1-based
    total_len: usize,
}

/// Row number token — whole-row endpoint.
struct RowNum {
    row: u32, // 1-based
    total_len: usize,
}

fn parse_cell(body: &str, start: usize) -> Option<CellParse> {
    let bytes = body.as_bytes();
    let mut pos = start;

    let col_abs = bytes.get(pos) == Some(&b'$');
    if col_abs {
        pos += 1;
    }

    let col_start = pos;
    while pos < bytes.len() && bytes[pos].is_ascii_alphabetic() {
        pos += 1;
    }
    if pos == col_start {
        return None;
    }
    let col_end = pos;

    let row_abs = bytes.get(pos) == Some(&b'$');
    if row_abs {
        pos += 1;
    }

    let row_start = pos;
    while pos < bytes.len() && bytes[pos].is_ascii_digit() {
        pos += 1;
    }
    if pos == row_start {
        return None;
    }

    // Reject when the token continues as an identifier (e.g. `A1B` — not a cell).
    if let Some(&next) = bytes.get(pos) {
        if next.is_ascii_alphanumeric() || next == b'_' {
            return None;
        }
    }

    let col_letters = std::str::from_utf8(&bytes[col_start..col_end]).ok()?;
    let row_digits = std::str::from_utf8(&bytes[row_start..pos]).ok()?;
    let col = col_letters_to_index(col_letters)?;
    let row: u32 = row_digits.parse().ok()?;
    if row == 0 {
        return None;
    }

    Some(CellParse {
        col_abs,
        col,
        row_abs,
        row,
        total_len: pos - start,
    })
}

fn parse_col_letters(body: &str, start: usize) -> Option<ColLetters> {
    let bytes = body.as_bytes();
    let mut pos = start;

    let abs = bytes.get(pos) == Some(&b'$');
    if abs {
        pos += 1;
    }

    let letters_start = pos;
    while pos < bytes.len() && bytes[pos].is_ascii_alphabetic() {
        pos += 1;
    }
    if pos == letters_start {
        return None;
    }

    // Must not continue as an identifier (else the token is a name, not just col letters).
    if let Some(&next) = bytes.get(pos) {
        if next.is_ascii_alphanumeric() || next == b'_' {
            return None;
        }
    }

    let letters = std::str::from_utf8(&bytes[letters_start..pos]).ok()?;
    let col = col_letters_to_index(letters)?;
    Some(ColLetters {
        abs,
        col,
        total_len: pos - start,
    })
}

fn parse_row_num(body: &str, start: usize) -> Option<RowNum> {
    let bytes = body.as_bytes();
    let mut pos = start;
    while pos < bytes.len() && bytes[pos].is_ascii_digit() {
        pos += 1;
    }
    if pos == start {
        return None;
    }
    // Reject numbers that bleed into an identifier or decimal (e.g. `3.14`, `5abc`) —
    // those aren't row indices.
    if let Some(&next) = bytes.get(pos) {
        if next == b'.' || next.is_ascii_alphabetic() || next == b'_' {
            return None;
        }
    }
    let row: u32 = std::str::from_utf8(&bytes[start..pos]).ok()?.parse().ok()?;
    if row == 0 {
        return None;
    }
    Some(RowNum {
        row,
        total_len: pos - start,
    })
}

fn col_letters_to_index(letters: &str) -> Option<u32> {
    if letters.is_empty() {
        return None;
    }
    let mut idx: u32 = 0;
    for c in letters.chars() {
        let upper = c.to_ascii_uppercase();
        if !upper.is_ascii_alphabetic() {
            return None;
        }
        let digit = (upper as u32) - b'A' as u32 + 1;
        idx = idx.checked_mul(26)?.checked_add(digit)?;
    }
    Some(idx)
}

/// Apply `d` to `v` (1-based) if not absolute; return `None` if the
/// result falls outside `1..=max`.
fn shift_component(v: u32, d: i32, max: u32, absolute: bool) -> Option<u32> {
    if absolute {
        return Some(v);
    }
    let shifted = v as i64 + d as i64;
    if shifted >= 1 && shifted <= max as i64 {
        Some(shifted as u32)
    } else {
        None
    }
}

fn write_cell(col_abs: bool, col: u32, row_abs: bool, row: u32, out: &mut String) {
    if col_abs {
        out.push('$');
    }
    out.push_str(&index_to_col(col));
    if row_abs {
        out.push('$');
    }
    out.push_str(&row.to_string());
}

/// Bundled shift parameters for the `emit_*` helpers — passed as a single
/// reference so each helper stays under clippy's `too_many_arguments` cap.
struct Shift {
    d_row: i32,
    d_col: i32,
    max_row: u32,
    max_col: u32,
}

fn emit_cell(
    body: &str,
    start: usize,
    total: usize,
    cp: &CellParse,
    shift: &Shift,
    out: &mut String,
) {
    let new_col = shift_component(cp.col, shift.d_col, shift.max_col, cp.col_abs);
    let new_row = shift_component(cp.row, shift.d_row, shift.max_row, cp.row_abs);
    match (new_col, new_row) {
        (Some(nc), Some(nr)) if nc == cp.col && nr == cp.row => {
            // Unchanged — keep original bytes (preserves case).
            out.push_str(&body[start..start + total]);
        }
        (Some(nc), Some(nr)) => write_cell(cp.col_abs, nc, cp.row_abs, nr, out),
        _ => out.push_str(REF_ERROR),
    }
}

fn emit_cell_range(
    body: &str,
    start: usize,
    total: usize,
    a: &CellParse,
    b: &CellParse,
    shift: &Shift,
    out: &mut String,
) {
    let a_col = shift_component(a.col, shift.d_col, shift.max_col, a.col_abs);
    let a_row = shift_component(a.row, shift.d_row, shift.max_row, a.row_abs);
    let b_col = shift_component(b.col, shift.d_col, shift.max_col, b.col_abs);
    let b_row = shift_component(b.row, shift.d_row, shift.max_row, b.row_abs);
    match (a_col, a_row, b_col, b_row) {
        (Some(ac), Some(ar), Some(bc), Some(br)) => {
            if ac == a.col && ar == a.row && bc == b.col && br == b.row {
                out.push_str(&body[start..start + total]);
            } else {
                write_cell(a.col_abs, ac, a.row_abs, ar, out);
                out.push(':');
                write_cell(b.col_abs, bc, b.row_abs, br, out);
            }
        }
        _ => out.push_str(REF_ERROR),
    }
}

fn emit_spill_ref(
    body: &str,
    start: usize,
    total: usize,
    cp: &CellParse,
    shift: &Shift,
    out: &mut String,
) {
    let new_col = shift_component(cp.col, shift.d_col, shift.max_col, cp.col_abs);
    let new_row = shift_component(cp.row, shift.d_row, shift.max_row, cp.row_abs);
    match (new_col, new_row) {
        (Some(nc), Some(nr)) if nc == cp.col && nr == cp.row => {
            out.push_str(&body[start..start + total]);
        }
        (Some(nc), Some(nr)) => {
            write_cell(cp.col_abs, nc, cp.row_abs, nr, out);
            out.push('#');
        }
        _ => out.push_str(REF_ERROR),
    }
}

fn emit_col_range(
    body: &str,
    start: usize,
    total: usize,
    a: &ColLetters,
    b: &ColLetters,
    shift: &Shift,
    out: &mut String,
) {
    let ac = shift_component(a.col, shift.d_col, shift.max_col, a.abs);
    let bc = shift_component(b.col, shift.d_col, shift.max_col, b.abs);
    match (ac, bc) {
        (Some(nac), Some(nbc)) => {
            if nac == a.col && nbc == b.col {
                out.push_str(&body[start..start + total]);
            } else {
                if a.abs {
                    out.push('$');
                }
                out.push_str(&index_to_col(nac));
                out.push(':');
                if b.abs {
                    out.push('$');
                }
                out.push_str(&index_to_col(nbc));
            }
        }
        _ => out.push_str(REF_ERROR),
    }
}

fn emit_row_range(
    body: &str,
    start: usize,
    total: usize,
    a: &RowNum,
    b: &RowNum,
    shift: &Shift,
    out: &mut String,
) {
    // Whole-row refs have no `$` markers in our grammar (we don't
    // tokenize `$1`), so both endpoints are treated as relative.
    let ar = shift_component(a.row, shift.d_row, shift.max_row, false);
    let br = shift_component(b.row, shift.d_row, shift.max_row, false);
    match (ar, br) {
        (Some(nar), Some(nbr)) => {
            if nar == a.row && nbr == b.row {
                out.push_str(&body[start..start + total]);
            } else {
                out.push_str(&nar.to_string());
                out.push(':');
                out.push_str(&nbr.to_string());
            }
        }
        _ => out.push_str(REF_ERROR),
    }
}

fn skip_word(body: &str, start: usize) -> usize {
    let bytes = body.as_bytes();
    let mut end = start;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::range::format_cell_id;

    /// Render `cells` as A1-style strings — keeps existing test asserts
    /// readable now that the field is `Vec<CellCoord>` instead of `Vec<String>`.
    fn render_cells(cells: &[CellCoord]) -> Vec<String> {
        cells.iter().map(|c| format_cell_id(*c)).collect()
    }

    #[test]
    fn test_single_ref() {
        let refs = extract_refs("=A1+B2");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].text, "A1");
        assert_eq!(refs[0].start, 1);
        assert_eq!(refs[0].end, 3);
        assert_eq!(refs[0].kind, RefKind::Cell);
        assert_eq!(render_cells(&refs[0].cells), vec!["A1"]);
        assert_eq!(refs[1].text, "B2");
        assert_eq!(refs[1].start, 4);
        assert_eq!(refs[1].end, 6);
    }

    #[test]
    fn test_range_ref() {
        let refs = extract_refs("=SUM(A1:B2)");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "A1:B2");
        assert_eq!(refs[0].kind, RefKind::Range);
        assert_eq!(render_cells(&refs[0].cells), vec!["A1", "B1", "A2", "B2"]);
    }

    #[test]
    fn test_no_formula() {
        assert_eq!(extract_refs("hello").len(), 0);
        assert_eq!(extract_refs("42").len(), 0);
    }

    #[test]
    fn test_positions_account_for_equals() {
        let refs = extract_refs("=A1");
        assert_eq!(refs[0].start, 1);
        assert_eq!(refs[0].end, 3);
    }

    #[test]
    fn test_mixed_refs_and_literals() {
        let refs = extract_refs("=A1+100+B3*C4");
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].text, "A1");
        assert_eq!(refs[1].text, "B3");
        assert_eq!(refs[2].text, "C4");
    }

    #[test]
    fn test_function_with_range() {
        let refs = extract_refs("=SUM(A1:A3)+B1");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].text, "A1:A3");
        assert_eq!(render_cells(&refs[0].cells), vec!["A1", "A2", "A3"]);
        assert_eq!(refs[1].text, "B1");
    }

    #[test]
    fn test_extract_whole_column() {
        let refs = extract_refs("=SUM(B:B)");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "B:B");
        assert_eq!(refs[0].kind, RefKind::WholeColumn);
        assert!(refs[0].cells.is_empty());
    }

    #[test]
    fn test_extract_whole_row() {
        let refs = extract_refs("=SUM(1:5)");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "1:5");
        assert_eq!(refs[0].kind, RefKind::WholeRow);
        assert!(refs[0].cells.is_empty());
    }

    #[test]
    fn test_oversized_range_has_empty_cells() {
        // A1:A200000 would expand to 200k cells — above the cap, so
        // callers get an empty expansion (like whole-col/row refs).
        let refs = extract_refs("=A1:A200000");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, RefKind::Range);
        assert_eq!(refs[0].text, "A1:A200000");
        assert!(refs[0].cells.is_empty());
    }

    #[test]
    fn test_modest_range_still_expands() {
        let refs = extract_refs("=A1:A10");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].cells.len(), 10);
        assert_eq!(format_cell_id(refs[0].cells[0]), "A1");
        assert_eq!(format_cell_id(refs[0].cells[9]), "A10");
    }

    #[test]
    fn test_extract_ignores_strings_with_refs() {
        let refs = extract_refs(r#"=IF(A1, "B1", "B:B")"#);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "A1");
    }

    #[test]
    fn test_extract_name_alongside_cell() {
        let refs = extract_refs("=TaxRate+A1");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].kind, RefKind::Name);
        assert_eq!(refs[0].text, "TAXRATE"); // lexer uppercases identifiers
        assert!(refs[0].cells.is_empty());
        assert_eq!(refs[1].kind, RefKind::Cell);
        assert_eq!(refs[1].text, "A1");
    }

    #[test]
    fn test_extract_does_not_emit_name_for_function_call() {
        // SUM is a function call, not a name reference.
        let refs = extract_refs("=SUM(A1)");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, RefKind::Cell);
        assert_eq!(refs[0].text, "A1");
    }

    #[test]
    fn test_extract_does_not_emit_name_for_column_range() {
        // A:C should still be WholeColumn, not Name(A) + Name(C).
        let refs = extract_refs("=SUM(A:C)");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, RefKind::WholeColumn);
    }

    #[test]
    fn test_extract_bare_name_only() {
        let refs = extract_refs("=Revenue");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, RefKind::Name);
        assert_eq!(refs[0].start, 1);
        assert_eq!(refs[0].end, 8); // "Revenue" is 7 chars after the leading '='
    }

    // ── rewrite_refs_for_deletion ────────────────────────────────────

    fn del(cols: &[u32], rows: &[u32]) -> Deletion {
        Deletion {
            cols: cols.to_vec(),
            rows: rows.to_vec(),
        }
    }

    fn rewrite(f: &str, cols: &[u32], rows: &[u32]) -> String {
        rewrite_refs_for_deletion(f, &del(cols, rows)).into_owned()
    }

    #[test]
    fn rewrite_single_cell_unaffected() {
        assert_eq!(rewrite("=A1", &[], &[]), "=A1");
    }

    #[test]
    fn rewrite_single_cell_column_deleted() {
        assert_eq!(rewrite("=B1", &[1], &[]), "=#REF!");
    }

    #[test]
    fn rewrite_two_cells_one_deleted() {
        assert_eq!(rewrite("=A1+B1", &[1], &[]), "=A1+#REF!");
    }

    #[test]
    fn rewrite_several_cells_multiple_cols_deleted() {
        assert_eq!(rewrite("=A1+B2+C3", &[1, 2], &[]), "=A1+#REF!+#REF!");
    }

    #[test]
    fn rewrite_whole_column_both_deleted() {
        assert_eq!(rewrite("=SUM(B:B)", &[1], &[]), "=SUM(#REF!)");
    }

    #[test]
    fn rewrite_whole_column_partial() {
        assert_eq!(rewrite("=SUM(A:B)", &[1], &[]), "=SUM(A:#REF!)");
    }

    #[test]
    fn rewrite_whole_column_both_cols_deleted() {
        assert_eq!(rewrite("=SUM(A:B)", &[0, 1], &[]), "=SUM(#REF!)");
    }

    #[test]
    fn rewrite_a1_range_partial() {
        assert_eq!(rewrite("=SUM(A1:B2)", &[1], &[]), "=SUM(A1:#REF!)");
    }

    #[test]
    fn rewrite_a1_range_both_deleted() {
        assert_eq!(rewrite("=SUM(A1:B2)", &[0, 1], &[]), "=SUM(#REF!)");
    }

    #[test]
    fn rewrite_whole_row_deleted() {
        assert_eq!(rewrite("=SUM(1:1)", &[], &[0]), "=SUM(#REF!)");
    }

    #[test]
    fn rewrite_a1_range_end_row_deleted() {
        assert_eq!(rewrite("=A1:A5", &[], &[4]), "=A1:#REF!");
    }

    #[test]
    fn rewrite_a1_range_all_rows_deleted() {
        assert_eq!(rewrite("=A1:A5", &[], &[0, 1, 2, 3, 4]), "=#REF!");
    }

    #[test]
    fn rewrite_preserves_string_literal_refs() {
        assert_eq!(
            rewrite(r#"=IF(A1, "B1", "B:B")"#, &[1], &[]),
            r#"=IF(A1, "B1", "B:B")"#
        );
    }

    #[test]
    fn rewrite_non_formula_passes_through() {
        assert_eq!(rewrite("hello", &[1], &[]), "hello");
    }

    #[test]
    fn rewrite_returns_borrowed_on_no_change() {
        let f = "=A1+C3";
        let out = rewrite_refs_for_deletion(f, &del(&[1], &[]));
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "=A1+C3");
    }

    #[test]
    fn rewrite_is_idempotent() {
        let cases: &[(&str, &[u32], &[u32])] = &[
            ("=A1", &[], &[]),
            ("=B1", &[1], &[]),
            ("=A1+B1", &[1], &[]),
            ("=SUM(B:B)", &[1], &[]),
            ("=SUM(A:B)", &[1], &[]),
            ("=SUM(A1:B2)", &[1], &[]),
            ("=SUM(1:1)", &[], &[0]),
            ("=A1:A5", &[], &[4]),
            ("=A1:A5", &[], &[0, 1, 2, 3, 4]),
            (r#"=IF(A1, "B1", "B:B")"#, &[1], &[]),
        ];
        for (f, cols, rows) in cases {
            let once = rewrite(f, cols, rows);
            let twice = rewrite(&once, cols, rows);
            assert_eq!(once, twice, "not idempotent for {f:?}");
        }
    }

    #[test]
    fn rewrite_function_call_not_confused_with_column_range() {
        // SUM( …) is a function call, not a whole-col range — make sure
        // we don't accidentally match on the Function token.
        assert_eq!(rewrite("=SUM(A1)", &[], &[]), "=SUM(A1)");
    }

    // ── adjust_refs_for_deletion (shifting + trimming) ───────────────

    fn adjust(f: &str, cols: &[u32], rows: &[u32]) -> String {
        adjust_refs_for_deletion(f, &del(cols, rows)).into_owned()
    }

    // ── shifting single cell refs ──

    #[test]
    fn adjust_cell_below_deletion_unaffected() {
        // A (idx 0) is before any deletion at idx 1+, so stays A.
        assert_eq!(adjust("=A1", &[1], &[]), "=A1");
    }

    #[test]
    fn adjust_cell_deleted_becomes_ref_error() {
        assert_eq!(adjust("=B1", &[1], &[]), "=#REF!");
    }

    #[test]
    fn adjust_cell_after_deletion_shifts_left() {
        // D (idx 3) after del col B (idx 1) → idx 2 = C.
        assert_eq!(adjust("=D1", &[1], &[]), "=C1");
    }

    #[test]
    fn adjust_cell_after_multiple_deletions_shifts_by_count() {
        // D (idx 3) after del B,C (idx 1,2) → idx 1 = B.
        assert_eq!(adjust("=D1", &[1, 2], &[]), "=B1");
    }

    #[test]
    fn adjust_cell_shifts_both_row_and_col() {
        // B2 (col 1, row 1) + del col A (0), row 1 (0) → col 0 = A, row 0 = 1.
        assert_eq!(adjust("=B2", &[0], &[0]), "=A1");
    }

    #[test]
    fn adjust_cell_row_shift_only() {
        // A5 (row idx 4) + del row 2 (idx 1) → row idx 3 = row 4.
        assert_eq!(adjust("=A5", &[], &[1]), "=A4");
    }

    #[test]
    fn adjust_cell_deleted_either_row_or_col_becomes_ref_error() {
        // Col alive but row deleted.
        assert_eq!(adjust("=A5", &[], &[4]), "=#REF!");
        // Row alive but col deleted.
        assert_eq!(adjust("=B1", &[1], &[]), "=#REF!");
    }

    #[test]
    fn adjust_multiple_cells_mixed() {
        // A1 stays, B2 deleted (#REF!), D3 shifts C3.
        assert_eq!(adjust("=A1+B2+D3", &[1], &[]), "=A1+#REF!+C3");
    }

    // ── trimming A1-style ranges (surviving endpoints) ──

    #[test]
    fn adjust_range_inner_column_deleted_trims_right() {
        // A1:C1 with col B deleted → surviving A,C; C shifts to B.
        assert_eq!(adjust("=A1:C1", &[1], &[]), "=A1:B1");
    }

    #[test]
    fn adjust_range_inner_two_cols_deleted_trims() {
        assert_eq!(adjust("=A1:D1", &[1, 2], &[]), "=A1:B1");
    }

    #[test]
    fn adjust_range_last_col_deleted_trims_to_survivor() {
        // A1:D1 + del D → surviving A,B,C; range shrinks to A1:C1.
        assert_eq!(adjust("=A1:D1", &[3], &[]), "=A1:C1");
    }

    #[test]
    fn adjust_range_first_col_deleted_trims_to_survivor() {
        // A1:D1 + del A → surviving B,C,D; B shifts to A, D shifts to C.
        assert_eq!(adjust("=A1:D1", &[0], &[]), "=A1:C1");
    }

    #[test]
    fn adjust_range_both_endpoints_deleted_interior_survives() {
        // A1:D1 + del A,D → surviving B,C → shift to A,B.
        assert_eq!(adjust("=A1:D1", &[0, 3], &[]), "=A1:B1");
    }

    #[test]
    fn adjust_range_all_cols_deleted_becomes_ref_error() {
        assert_eq!(adjust("=B1:C1", &[1, 2], &[]), "=#REF!");
    }

    #[test]
    fn adjust_range_row_deleted_inside_trims() {
        // A1:A5 + del row 3 (idx 2) → surviving rows 1,2,4,5; 4→3, 5→4.
        assert_eq!(adjust("=A1:A5", &[], &[2]), "=A1:A4");
    }

    #[test]
    fn adjust_range_first_row_deleted_shifts() {
        // A1:A5 + del row 1 (idx 0) → surviving 2..5 → shift to 1..4.
        assert_eq!(adjust("=A1:A5", &[], &[0]), "=A1:A4");
    }

    #[test]
    fn adjust_range_last_row_deleted_trims() {
        assert_eq!(adjust("=A1:A5", &[], &[4]), "=A1:A4");
    }

    #[test]
    fn adjust_range_all_rows_deleted_becomes_ref_error() {
        assert_eq!(adjust("=A1:A5", &[], &[0, 1, 2, 3, 4]), "=#REF!");
    }

    #[test]
    fn adjust_range_2d_mixed_col_and_row() {
        // A1:C3 + del col B (idx 1) + del row 2 (idx 1)
        //   cols: surviving A,C → shifted A,B
        //   rows: surviving 1,3 → shifted 1,2
        //   → A1:B2
        assert_eq!(adjust("=A1:C3", &[1], &[1]), "=A1:B2");
    }

    #[test]
    fn adjust_range_2d_all_cols_deleted_even_though_rows_survive() {
        // If every col in the range is deleted, the whole range is #REF!
        // even when rows are untouched.
        assert_eq!(adjust("=B1:B3", &[1], &[]), "=#REF!");
    }

    #[test]
    fn adjust_range_unchanged_when_deletion_is_outside() {
        // Deleting something past the range shouldn't touch it.
        assert_eq!(adjust("=A1:B2", &[5], &[5]), "=A1:B2");
    }

    #[test]
    fn adjust_range_backwards_canonicalizes_on_change() {
        // When the user wrote the range "backwards" (B1:A2) and a
        // deletion forces an edit, we emit it in canonical min:max form.
        assert_eq!(adjust("=B1:A2", &[0], &[]), "=A1:A2");
    }

    #[test]
    fn adjust_range_backwards_unchanged_if_untouched() {
        // But if nothing would change, keep the original bytes.
        assert_eq!(adjust("=B1:A2", &[5], &[]), "=B1:A2");
    }

    // ── whole-column ranges ──

    #[test]
    fn adjust_whole_column_inner_col_deleted_trims() {
        assert_eq!(adjust("=SUM(A:C)", &[1], &[]), "=SUM(A:B)");
    }

    #[test]
    fn adjust_whole_column_both_endpoints_deleted_interior_survives() {
        // A:C + del A, C → surviving B (idx 1) → shift to A (idx 0).
        assert_eq!(adjust("=SUM(A:C)", &[0, 2], &[]), "=SUM(A:A)");
    }

    #[test]
    fn adjust_whole_column_entirely_deleted_becomes_ref_error() {
        assert_eq!(adjust("=SUM(B:B)", &[1], &[]), "=SUM(#REF!)");
    }

    #[test]
    fn adjust_whole_column_shifted() {
        // C:D + del col A (idx 0) → C,D survive and shift by -1 → B:C.
        assert_eq!(adjust("=SUM(C:D)", &[0], &[]), "=SUM(B:C)");
    }

    #[test]
    fn adjust_whole_column_ignores_row_deletions() {
        // Deleting rows shouldn't change a whole-column range.
        assert_eq!(adjust("=SUM(A:C)", &[], &[0, 1, 2]), "=SUM(A:C)");
    }

    // ── whole-row ranges ──

    #[test]
    fn adjust_whole_row_inner_row_deleted_trims() {
        // 1:3 + del row 2 (idx 1) → surviving 1,3; 3 shifts to 2 → 1:2.
        assert_eq!(adjust("=SUM(1:3)", &[], &[1]), "=SUM(1:2)");
    }

    #[test]
    fn adjust_whole_row_first_row_deleted_shifts() {
        // 2:4 + del row 1 (idx 0) → surviving 2,3,4 shift to 1,2,3 → 1:3.
        assert_eq!(adjust("=SUM(2:4)", &[], &[0]), "=SUM(1:3)");
    }

    #[test]
    fn adjust_whole_row_entirely_deleted_becomes_ref_error() {
        assert_eq!(adjust("=SUM(1:1)", &[], &[0]), "=SUM(#REF!)");
    }

    #[test]
    fn adjust_whole_row_ignores_col_deletions() {
        assert_eq!(adjust("=SUM(1:5)", &[0, 1, 2], &[]), "=SUM(1:5)");
    }

    // ── no-ops / pass-through ──

    #[test]
    fn adjust_non_formula_passes_through() {
        assert_eq!(adjust("hello", &[1], &[]), "hello");
    }

    #[test]
    fn adjust_preserves_strings_with_refs() {
        // Ref-like text inside string literals must be left alone.
        assert_eq!(
            adjust(r#"=IF(A1, "B1", "B:B")"#, &[1], &[]),
            r#"=IF(A1, "B1", "B:B")"#
        );
    }

    #[test]
    fn adjust_borrowed_when_no_edits() {
        let out = adjust_refs_for_deletion("=A1+A2", &del(&[5], &[5]));
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn adjust_borrowed_when_no_deletions() {
        let out = adjust_refs_for_deletion("=A1+D5", &del(&[], &[]));
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn adjust_preserves_case_when_unchanged() {
        // Lowercase ref that isn't logically changed should keep
        // its original bytes (we only rewrite when semantics change).
        // Lexer can tokenize lowercase; if no shift is needed, no edit.
        let out = adjust_refs_for_deletion("=a1", &del(&[5], &[]));
        assert_eq!(&*out, "=a1");
    }

    // ── parity with simpler rewrite_refs_for_deletion ──

    #[test]
    fn adjust_matches_rewrite_when_no_survivor_can_shift_or_trim() {
        // The two functions agree when every affected ref is either
        // entirely deleted or sits entirely before the deletion point
        // (no opportunity to shift or trim).
        let cases: &[(&str, &[u32], &[u32])] = &[
            ("=A1", &[], &[]),
            ("=B1", &[1], &[]),
            ("=A1+B1", &[1], &[]),
            ("=SUM(B:B)", &[1], &[]),
            ("=SUM(1:1)", &[], &[0]),
            (r#"=IF(A1, "B1", "B:B")"#, &[1], &[]),
            ("hello", &[1], &[]),
        ];
        for (f, cols, rows) in cases {
            assert_eq!(adjust(f, cols, rows), rewrite(f, cols, rows), "{f:?}");
        }
    }

    #[test]
    fn adjust_and_rewrite_diverge_where_trimming_matters() {
        // Partial range deletions: adjust trims, rewrite stamps #REF!
        // on the deleted half. This test is here to document the
        // intentional divergence.
        assert_eq!(rewrite("=SUM(A:B)", &[1], &[]), "=SUM(A:#REF!)");
        assert_eq!(adjust("=SUM(A:B)", &[1], &[]), "=SUM(A:A)");
        assert_eq!(rewrite("=SUM(A1:B2)", &[1], &[]), "=SUM(A1:#REF!)");
        assert_eq!(adjust("=SUM(A1:B2)", &[1], &[]), "=SUM(A1:A2)");
    }

    // ── double-deletion represents two sequential deletes ──

    #[test]
    fn adjust_applied_twice_means_two_deletions() {
        // After first pass with del col B (idx 1): C1 → B1.
        // After second pass with the same del B: B1 → #REF!.
        let once = adjust("=C1", &[1], &[]);
        assert_eq!(once, "=B1");
        assert_eq!(adjust(&once, &[1], &[]), "=#REF!");
    }

    #[test]
    fn adjust_idempotent_when_no_surviving_shift() {
        // Starting formula's only affected ref becomes #REF!, which
        // can't be re-tokenized, so a second pass is a no-op.
        let once = adjust("=B1", &[1], &[]);
        assert_eq!(once, "=#REF!");
        assert_eq!(adjust(&once, &[1], &[]), "=#REF!");
    }

    // ── trim_and_shift_range unit checks ──

    #[test]
    fn trim_and_shift_all_deleted() {
        let deleted: HashSet<u32> = [0, 1, 2, 3].into();
        assert_eq!(trim_and_shift_range(0, 3, &deleted), None);
    }

    #[test]
    fn trim_and_shift_interior_survives() {
        let deleted: HashSet<u32> = [0, 3].into();
        assert_eq!(trim_and_shift_range(0, 3, &deleted), Some((0, 1)));
    }

    #[test]
    fn trim_and_shift_endpoints_survive() {
        let deleted: HashSet<u32> = [1, 2].into();
        assert_eq!(trim_and_shift_range(0, 3, &deleted), Some((0, 1)));
    }

    #[test]
    fn trim_and_shift_no_deletions_in_range() {
        let deleted: HashSet<u32> = [10].into();
        assert_eq!(trim_and_shift_range(0, 5, &deleted), Some((0, 5)));
    }

    #[test]
    fn trim_and_shift_deletions_only_before_range() {
        let deleted: HashSet<u32> = [0, 1].into();
        assert_eq!(trim_and_shift_range(5, 10, &deleted), Some((3, 8)));
    }

    // ── extract_refs for `$`-bearing inputs ───────────────────────────

    #[test]
    fn extract_refs_does_not_panic_on_oversized_column_letters() {
        // 8+ letters overflow u32 in `col_to_index`. The token still
        // tokenizes as a CellRef, but the coord must come back empty
        // rather than panicking the live-highlight pipeline.
        let refs = extract_refs("=AAAAAAAA1");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, RefKind::Cell);
        assert!(refs[0].cells.is_empty());
    }

    #[test]
    fn extract_refs_single_cell_preserves_dollar_in_text() {
        let refs = extract_refs("=$A$1+1");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, RefKind::Cell);
        assert_eq!(refs[0].text, "$A$1");
        // `cells` stays canonical for dep tracking.
        assert_eq!(render_cells(&refs[0].cells), vec!["A1"]);
        // Span covers the full `$A$1`.
        assert_eq!(&"=$A$1+1"[refs[0].start..refs[0].end], "$A$1");
    }

    #[test]
    fn extract_refs_absolute_range_preserves_dollars() {
        let refs = extract_refs("=SUM($A$1:$B$2)");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, RefKind::Range);
        assert_eq!(refs[0].text, "$A$1:$B$2");
        assert_eq!(render_cells(&refs[0].cells), vec!["A1", "B1", "A2", "B2"]);
    }

    #[test]
    fn extract_refs_absolute_spill_ref_preserves_dollars() {
        let refs = extract_refs("=$A$1#");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, RefKind::SpillRef);
        assert_eq!(refs[0].text, "$A$1#");
        assert_eq!(render_cells(&refs[0].cells), vec!["A1"]);
    }

    #[test]
    fn extract_refs_absolute_whole_col_and_row() {
        let col = extract_refs("=SUM($A:$A)");
        assert_eq!(col.len(), 1);
        assert_eq!(col[0].kind, RefKind::WholeColumn);
        assert_eq!(col[0].text, "$A:$A");

        let row = extract_refs("=SUM($1:$5)");
        assert_eq!(row.len(), 1);
        assert_eq!(row[0].kind, RefKind::WholeRow);
        assert_eq!(row[0].text, "$1:$5");
    }

    // ── shift_formula_refs ────────────────────────────────────────────

    fn shift(f: &str, d_row: i32, d_col: i32) -> String {
        shift_formula_refs(f, d_row, d_col, 1000, 26)
    }

    #[test]
    fn shift_relative_cells_down() {
        assert_eq!(shift("=A2*B1*4", 1, 0), "=A3*B2*4");
    }

    #[test]
    fn shift_relative_cells_right() {
        assert_eq!(shift("=A2*B1*4", 0, 1), "=B2*C1*4");
    }

    #[test]
    fn shift_off_grid_upward_becomes_ref_error() {
        // B1 shifts to row 0 → off-grid → #REF!.
        assert_eq!(shift("=A2*B1*4", -1, 0), "=A1*#REF!*4");
    }

    #[test]
    fn shift_preserves_full_absolute() {
        assert_eq!(shift("=$A$2*B1", 1, 1), "=$A$2*C2");
    }

    #[test]
    fn shift_mixed_absolute() {
        assert_eq!(shift("=$A2*A$1", 1, 1), "=$A3*B$1");
    }

    #[test]
    fn shift_mixed_absolute_with_trailing_literal() {
        // $A2 (col-abs) → $A3; B$1 (row-abs) → C$1; *4 untouched.
        assert_eq!(shift("=$A2*B$1*4", 1, 1), "=$A3*C$1*4");
        // Absolute axis stays pinned even under large deltas.
        assert_eq!(shift("=$A2*B$1*4", 5, 10), "=$A7*L$1*4");
        // Relative column on B$1 falling off-grid → #REF! for that ref.
        assert_eq!(shift("=$A2*B$1*4", 0, 25), "=$A2*#REF!*4");
    }

    #[test]
    fn shift_range_both_endpoints() {
        assert_eq!(shift("=A1:B5", 2, 0), "=A3:B7");
    }

    #[test]
    fn shift_range_off_grid_collapses() {
        // B5 would shift to col 27 (past max_col=26) → whole range is #REF!.
        assert_eq!(shift("=A1:B5", 0, 25), "=#REF!");
    }

    #[test]
    fn shift_range_with_absolute_start() {
        assert_eq!(shift("=$A$1:B5", 1, 0), "=$A$1:B6");
    }

    #[test]
    fn shift_whole_col_range_shifts_col() {
        assert_eq!(shift("=A:A", 5, 1), "=B:B");
    }

    #[test]
    fn shift_whole_col_range_off_grid_ref_error() {
        assert_eq!(shift("=A:A", 0, 26), "=#REF!");
    }

    #[test]
    fn shift_absolute_whole_col_range_unchanged() {
        assert_eq!(shift("=$A:$A", 0, 5), "=$A:$A");
    }

    #[test]
    fn shift_whole_row_range() {
        assert_eq!(shift("=1:1", 3, 5), "=4:4");
    }

    #[test]
    fn shift_spill_ref() {
        assert_eq!(shift("=A1#", 2, 1), "=B3#");
    }

    #[test]
    fn shift_absolute_spill_ref_unchanged() {
        assert_eq!(shift("=$A$1#", 2, 1), "=$A$1#");
    }

    #[test]
    fn shift_function_call_with_mixed_refs() {
        assert_eq!(
            shift("=SUM(A1, $B$2, C:C)", 1, 1),
            "=SUM(B2, $B$2, D:D)"
        );
    }

    #[test]
    fn shift_preserves_named_range() {
        assert_eq!(shift("=my_range + 1", 5, 5), "=my_range + 1");
    }

    #[test]
    fn shift_pass_through_function_without_refs() {
        assert_eq!(shift("=SEQUENCE(5)", 1, 1), "=SEQUENCE(5)");
    }

    #[test]
    fn shift_non_formula_number_passes_through() {
        assert_eq!(shift("123", 1, 0), "123");
    }

    #[test]
    fn shift_non_formula_string_passes_through() {
        assert_eq!(shift("hello", 1, 0), "hello");
    }

    #[test]
    fn shift_empty_string_passes_through() {
        assert_eq!(shift("", 1, 0), "");
    }

    #[test]
    fn shift_zero_delta_is_identity() {
        assert_eq!(shift("=A1", 0, 0), "=A1");
    }

    // ── additional edge cases ──

    #[test]
    fn shift_preserves_ref_inside_string_literal() {
        assert_eq!(
            shift(r#"=IF(A1, "A1", "B:B")"#, 1, 0),
            r#"=IF(A2, "A1", "B:B")"#
        );
    }

    #[test]
    fn shift_handles_lowercase_cell_ref() {
        // Lowercase shifts to canonical uppercase form.
        assert_eq!(shift("=a1", 1, 1), "=B2");
    }

    #[test]
    fn shift_cell_that_would_fall_to_row_zero_is_ref_error() {
        assert_eq!(shift("=A1", -1, 0), "=#REF!");
    }

    #[test]
    fn shift_cell_that_would_fall_to_col_zero_is_ref_error() {
        assert_eq!(shift("=A1", 0, -1), "=#REF!");
    }

    #[test]
    fn shift_non_formula_empty_after_equals_is_stable() {
        assert_eq!(shift("=", 1, 0), "=");
    }

    #[test]
    fn shift_whole_row_partial_off_grid_collapses() {
        // 3+996=999 (in-grid), 5+996=1001 (off-grid) → collapse.
        assert_eq!(shift_formula_refs("=3:5", 996, 0, 1000, 26), "=#REF!");
    }

    #[test]
    fn shift_negative_delta_within_range() {
        assert_eq!(shift("=C3", -1, -1), "=B2");
    }

    // ── adjust_refs_for_insertion ────────────────────────────────────

    fn ins(cols: &[u32], rows: &[u32]) -> Insertion {
        Insertion {
            cols: cols.to_vec(),
            rows: rows.to_vec(),
        }
    }

    fn adjust_insert(f: &str, cols: &[u32], rows: &[u32]) -> String {
        adjust_refs_for_insertion(f, &ins(cols, rows)).into_owned()
    }

    // ── single cell refs ──

    #[test]
    fn insert_single_cell_shifts_when_at_or_past_insertion_point() {
        // A (col 0) + insert col 0 → count(i<=0)=1 → col 1 = B.
        assert_eq!(adjust_insert("=A1", &[0], &[]), "=B1");
    }

    #[test]
    fn insert_single_cell_unaffected_when_before_insertion() {
        // A (col 0) + insert col 1 → count(i<=0)=0 → stays.
        assert_eq!(adjust_insert("=A1", &[1], &[]), "=A1");
    }

    #[test]
    fn insert_single_cell_shifts_at_insertion_index() {
        // B (col 1) + insert col 1 → count(i<=1)=1 → col 2 = C.
        assert_eq!(adjust_insert("=B2", &[1], &[]), "=C2");
    }

    #[test]
    fn insert_duplicate_indices_shift_by_count() {
        // B (col 1) + insert cols [1,1] → shift=2 → col 3 = D.
        assert_eq!(adjust_insert("=B2", &[1, 1], &[]), "=D2");
    }

    #[test]
    fn insert_row_shift_only() {
        // A1 (row 0) + insert row 0 → row 1 = 2.
        assert_eq!(adjust_insert("=A1", &[], &[0]), "=A2");
    }

    #[test]
    fn insert_shifts_both_axes_simultaneously() {
        // A1 + insert row 0, col 0 → B2.
        assert_eq!(adjust_insert("=A1", &[0], &[0]), "=B2");
    }

    #[test]
    fn insert_preserves_absolute_markers() {
        // $B$2 + insert col 1 → shift applies but `$` stays.
        assert_eq!(adjust_insert("=$B$2", &[1], &[]), "=$C$2");
    }

    #[test]
    fn insert_preserves_col_only_absolute() {
        // $A1 + insert col 0 → $B1. `$` on col pins the ref, still shifts.
        assert_eq!(adjust_insert("=$A1", &[0], &[]), "=$B1");
    }

    #[test]
    fn insert_preserves_row_only_absolute() {
        assert_eq!(adjust_insert("=A$1", &[], &[0]), "=A$2");
    }

    // ── A1 ranges ──

    #[test]
    fn insert_range_grows_when_insertion_is_inside() {
        // A1:C3 + insert col 1 → A unchanged (col 0), C (col 2) shifts
        // to col 3 (D). The range grows to include the new blank col.
        assert_eq!(adjust_insert("=A1:C3", &[1], &[]), "=A1:D3");
    }

    #[test]
    fn insert_range_grows_at_second_endpoint() {
        // A1:C3 + insert col 2 (at the endpoint) → C shifts to D.
        assert_eq!(adjust_insert("=A1:C3", &[2], &[]), "=A1:D3");
    }

    #[test]
    fn insert_range_unchanged_when_insertion_past_end() {
        assert_eq!(adjust_insert("=A1:C3", &[5], &[]), "=A1:C3");
    }

    #[test]
    fn insert_range_shifts_wholly_when_insertion_before_start() {
        // A1:C3 + insert col 0 → both endpoints shift by 1.
        assert_eq!(adjust_insert("=A1:C3", &[0], &[]), "=B1:D3");
    }

    #[test]
    fn insert_range_row_inside_grows() {
        // A1:A3 + insert row 1 → A1 unchanged, A3 → A4.
        assert_eq!(adjust_insert("=A1:A3", &[], &[1]), "=A1:A4");
    }

    // ── whole-column / whole-row ranges ──

    #[test]
    fn insert_whole_column_shifts() {
        assert_eq!(adjust_insert("=SUM(A:A)", &[0], &[]), "=SUM(B:B)");
    }

    #[test]
    fn insert_whole_column_range_grows() {
        // A:C + insert col 1 → A stays, C → D.
        assert_eq!(adjust_insert("=SUM(A:C)", &[1], &[]), "=SUM(A:D)");
    }

    #[test]
    fn insert_whole_column_ignores_row_insertions() {
        assert_eq!(adjust_insert("=SUM(A:C)", &[], &[0, 1, 2]), "=SUM(A:C)");
    }

    #[test]
    fn insert_whole_column_preserves_absolute() {
        // $A:$A + insert col 0 → $B:$B.
        assert_eq!(adjust_insert("=SUM($A:$A)", &[0], &[]), "=SUM($B:$B)");
    }

    #[test]
    fn insert_whole_row_shifts() {
        assert_eq!(adjust_insert("=SUM(1:1)", &[], &[0]), "=SUM(2:2)");
    }

    #[test]
    fn insert_whole_row_range_grows() {
        // 1:3 + insert row 1 → 1 stays, 3 → 4.
        assert_eq!(adjust_insert("=SUM(1:3)", &[], &[1]), "=SUM(1:4)");
    }

    #[test]
    fn insert_whole_row_ignores_col_insertions() {
        assert_eq!(adjust_insert("=SUM(1:5)", &[0, 1, 2], &[]), "=SUM(1:5)");
    }

    #[test]
    fn insert_whole_row_preserves_absolute() {
        assert_eq!(adjust_insert("=SUM($1:$1)", &[], &[0]), "=SUM($2:$2)");
    }

    // ── pass-through / no-op ──

    #[test]
    fn insert_non_formula_passes_through() {
        assert_eq!(adjust_insert("hello", &[0], &[]), "hello");
    }

    #[test]
    fn insert_preserves_string_literal_refs() {
        assert_eq!(
            adjust_insert(r#"=IF(A1, "B1", "B:B")"#, &[], &[0]),
            r#"=IF(A2, "B1", "B:B")"#
        );
    }

    #[test]
    fn insert_empty_insertion_is_borrowed() {
        let out = adjust_refs_for_insertion("=A1", &ins(&[], &[]));
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "=A1");
    }

    #[test]
    fn insert_borrowed_when_no_edits_apply() {
        // Insertion is past every ref — nothing to rewrite.
        let out = adjust_refs_for_insertion("=A1+B2", &ins(&[10], &[10]));
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn insert_mixed_formula_with_addition() {
        // =A1 + B2 + insert row 0 → =A2 + B3.
        assert_eq!(adjust_insert("=A1 + B2", &[], &[0]), "=A2 + B3");
    }

    #[test]
    fn insert_applied_twice_means_two_insertions() {
        // After first pass with insert col 0: A1 → B1.
        // After second pass with same insert col 0: B1 → C1.
        let once = adjust_insert("=A1", &[0], &[]);
        assert_eq!(once, "=B1");
        assert_eq!(adjust_insert(&once, &[0], &[]), "=C1");
    }

    #[test]
    fn insert_preserves_case_when_unchanged() {
        let out = adjust_refs_for_insertion("=a1", &ins(&[5], &[]));
        assert_eq!(&*out, "=a1");
    }

    // ── adjust_refs_for_column_block_move ────────────────────────────

    fn move_block(f: &str, src_start: u32, src_end: u32, final_start: u32) -> String {
        adjust_refs_for_column_block_move(f, src_start, src_end, final_start).into_owned()
    }

    // ── single-column move (D → after B, src 3..=3, final 2) ──

    #[test]
    fn move_single_cell_in_source_block() {
        assert_eq!(move_block("=D1", 3, 3, 2), "=C1");
    }

    #[test]
    fn move_single_cell_in_displaced_band() {
        // C is in [final_start, src_start) → shifts right by width.
        assert_eq!(move_block("=C1", 3, 3, 2), "=D1");
    }

    #[test]
    fn move_addition_outside_and_inside() {
        // B is outside the band (col 1 < final_start=2). D maps to C.
        assert_eq!(move_block("=B5+D5", 3, 3, 2), "=B5+C5");
    }

    #[test]
    fn move_preserves_both_abs_markers() {
        assert_eq!(move_block("=$D$1", 3, 3, 2), "=$C$1");
    }

    #[test]
    fn move_preserves_col_only_abs() {
        assert_eq!(move_block("=$D1", 3, 3, 2), "=$C1");
    }

    #[test]
    fn move_preserves_row_only_abs() {
        assert_eq!(move_block("=D$1", 3, 3, 2), "=C$1");
    }

    // ── bounded ranges are positional ──

    #[test]
    fn move_bounded_range_unchanged_outside() {
        assert_eq!(move_block("=A1:D10", 3, 3, 2), "=A1:D10");
    }

    #[test]
    fn move_bounded_range_unchanged_inside() {
        assert_eq!(move_block("=B1:C10", 3, 3, 2), "=B1:C10");
    }

    // ── whole-column ranges follow data via interior bbox ──

    #[test]
    fn move_whole_col_single_in_source() {
        assert_eq!(move_block("=SUM(D:D)", 3, 3, 2), "=SUM(C:C)");
    }

    #[test]
    fn move_whole_col_range_interior_bbox_unchanged() {
        // B:D → forward across {1,2,3} = {1, 3, 2} → bbox B:D.
        assert_eq!(move_block("=SUM(B:D)", 3, 3, 2), "=SUM(B:D)");
    }

    #[test]
    fn move_whole_col_range_interior_bbox_grows() {
        // B:C → forward across {1,2} = {1, 3} → bbox B:D.
        assert_eq!(move_block("=SUM(B:C)", 3, 3, 2), "=SUM(B:D)");
    }

    #[test]
    fn move_whole_col_range_preserves_abs_markers() {
        // Same shape as B:D (no bbox change), but `$` markers ride through.
        assert_eq!(move_block("=SUM($B:$D)", 3, 3, 2), "=SUM($B:$D)");
    }

    // ── whole-row ranges unaffected by column move ──

    #[test]
    fn move_whole_row_unchanged() {
        assert_eq!(move_block("=SUM(1:5)", 3, 3, 2), "=SUM(1:5)");
    }

    // ── spill anchor ──

    #[test]
    fn move_spill_anchor_follows() {
        // A1# with single-col move A→after-B: src=0..=0, final=2.
        // forward_col(0) = 2 → C1. The trailing `#` rides through.
        assert_eq!(move_block("=A1#", 0, 0, 2), "=C1#");
    }

    // ── pass-through ──

    #[test]
    fn move_non_formula_passes_through() {
        assert_eq!(move_block("not a formula", 3, 3, 2), "not a formula");
    }

    #[test]
    fn move_preserves_string_literal_refs() {
        assert_eq!(
            move_block(r#"=IF(A1, "B1", "B:B")"#, 0, 0, 2),
            r#"=IF(C1, "B1", "B:B")"#
        );
    }

    // ── multi-column block move (B:D → start at 4) ──

    #[test]
    fn move_block_swap_singles() {
        // B(1) → 1-1+4 = 4 = E. E(4) → 4-3 = 1 = B.
        assert_eq!(move_block("=B1+E1", 1, 3, 4), "=E1+B1");
    }

    #[test]
    fn move_block_whole_col_range_in_source() {
        // B:D → forward {1,2,3} = {4,5,6} → bbox E:G.
        assert_eq!(move_block("=SUM(B:D)", 1, 3, 4), "=SUM(E:G)");
    }

    #[test]
    fn move_block_whole_col_range_straddling() {
        // C:F → forward {2,3,4,5} = {5,6,1,2} → bbox B:G.
        assert_eq!(move_block("=SUM(C:F)", 1, 3, 4), "=SUM(B:G)");
    }

    // ── no-op shapes ──

    #[test]
    fn move_no_op_when_final_equals_src_start() {
        assert_eq!(move_block("=A1+B2", 3, 3, 3), "=A1+B2");
    }

    #[test]
    fn move_no_op_when_block_identity() {
        assert_eq!(move_block("=A1+B2", 1, 3, 1), "=A1+B2");
    }

    #[test]
    fn move_block_whole_col_range_block_outside() {
        // B:E with single-col D → after-B (src 3..=3, final 2):
        // forward {1,2,3,4} = {1,3,2,4} → bbox B:E (unchanged).
        assert_eq!(move_block("=SUM(B:E)", 3, 3, 2), "=SUM(B:E)");
    }

    // ── borrowed-vs-owned hygiene ──

    #[test]
    fn move_no_op_is_borrowed() {
        let out = adjust_refs_for_column_block_move("=A1", 3, 3, 3);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_borrowed_when_no_edits_apply() {
        // Move is in cols 5..=5; refs to A1/B2 are outside the band.
        let out = adjust_refs_for_column_block_move("=A1+B2", 5, 5, 8);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_non_formula_is_borrowed() {
        let out = adjust_refs_for_column_block_move("hello", 3, 3, 2);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_invalid_range_is_borrowed() {
        // src_end < src_start → defensive no-op.
        let out = adjust_refs_for_column_block_move("=A1", 5, 2, 0);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    // ── adjust_refs_for_column_block_move_data_following ────────────
    //
    // Differs from the sibling above only in how it treats bounded
    // ranges (CellRef:CellRef): they get rewritten via interior bbox
    // rather than left in place. Single-cell, whole-col, whole-row,
    // abs markers, and spill-anchor handling are identical, so the
    // tests below focus on (a) bounded-range cases that diverge and
    // (b) a few smoke checks that the unchanged branches still work.

    fn move_df(f: &str, src_start: u32, src_end: u32, final_start: u32) -> String {
        adjust_refs_for_column_block_move_data_following(f, src_start, src_end, final_start)
            .into_owned()
    }

    // ── single-cell + abs markers (smoke checks) ──

    #[test]
    fn move_df_single_cell_in_source_block() {
        assert_eq!(move_df("=D1", 3, 3, 2), "=C1");
    }

    #[test]
    fn move_df_preserves_both_abs_markers() {
        assert_eq!(move_df("=$D$1", 3, 3, 2), "=$C$1");
    }

    // ── bounded ranges follow data via interior bbox ──

    #[test]
    fn move_df_bounded_range_unchanged_when_bbox_unchanged() {
        // A1:D10 → forward {0,1,2,3} = {0,1,3,2} → bbox A:D (unchanged
        // because the move is fully inside the range).
        assert_eq!(move_df("=A1:D10", 3, 3, 2), "=A1:D10");
    }

    #[test]
    fn move_df_bounded_range_unchanged_when_band_inside_range() {
        // B1:D10 → forward {1,2,3} = {1,3,2} → bbox B:D (unchanged).
        assert_eq!(move_df("=B1:D10", 3, 3, 2), "=B1:D10");
    }

    #[test]
    fn move_df_bounded_range_grows_when_straddling() {
        // B1:C10 → forward {1,2} = {1,3} → bbox B:D. Differs from the
        // positional sibling, which leaves this as B1:C10.
        assert_eq!(move_df("=B1:C10", 3, 3, 2), "=B1:D10");
    }

    #[test]
    fn move_df_bounded_range_single_col_follows() {
        // D1:D10 → forward {3} = {2} → bbox C:C. Differs from sibling.
        assert_eq!(move_df("=D1:D10", 3, 3, 2), "=C1:C10");
    }

    #[test]
    fn move_df_bounded_range_preserves_abs_markers() {
        assert_eq!(move_df("=$D$1:$D$10", 3, 3, 2), "=$C$1:$C$10");
    }

    #[test]
    fn move_df_bounded_range_partial_abs_markers() {
        // Mixed `$` markers per endpoint preserved through the rewrite.
        assert_eq!(move_df("=$D1:D$10", 3, 3, 2), "=$C1:C$10");
    }

    #[test]
    fn move_df_bounded_range_grows_left() {
        // D1:E10 → forward {3,4} = {2,4} → bbox C:E.
        assert_eq!(move_df("=D1:E10", 3, 3, 2), "=C1:E10");
    }

    // ── whole-column ranges (identical to sibling) ──

    #[test]
    fn move_df_whole_col_single_in_source() {
        assert_eq!(move_df("=SUM(D:D)", 3, 3, 2), "=SUM(C:C)");
    }

    #[test]
    fn move_df_whole_col_range_interior_bbox_unchanged() {
        assert_eq!(move_df("=SUM(B:D)", 3, 3, 2), "=SUM(B:D)");
    }

    #[test]
    fn move_df_whole_col_range_interior_bbox_grows() {
        assert_eq!(move_df("=SUM(B:C)", 3, 3, 2), "=SUM(B:D)");
    }

    // ── whole-row + spill anchor + non-formula (identical to sibling) ──

    #[test]
    fn move_df_whole_row_unchanged() {
        assert_eq!(move_df("=SUM(1:5)", 3, 3, 2), "=SUM(1:5)");
    }

    #[test]
    fn move_df_spill_anchor_follows() {
        assert_eq!(move_df("=A1#", 0, 0, 2), "=C1#");
    }

    #[test]
    fn move_df_non_formula_passes_through() {
        assert_eq!(move_df("not a formula", 3, 3, 2), "not a formula");
    }

    #[test]
    fn move_df_preserves_string_literal_refs() {
        assert_eq!(move_df(r#"="hello"+A1"#, 0, 0, 2), r#"="hello"+C1"#);
    }

    // ── multi-column block move (B:D → start at 4) ──

    #[test]
    fn move_df_block_swap_singles() {
        assert_eq!(move_df("=B1+E1", 1, 3, 4), "=E1+B1");
    }

    #[test]
    fn move_df_block_bounded_range_in_source() {
        // B1:D10 → forward {1,2,3} = {4,5,6} → bbox E:G.
        assert_eq!(move_df("=B1:D10", 1, 3, 4), "=E1:G10");
    }

    #[test]
    fn move_df_block_bounded_range_straddling() {
        // C1:F10 → forward {2,3,4,5} = {5,6,1,2} → bbox B:G.
        assert_eq!(move_df("=C1:F10", 1, 3, 4), "=B1:G10");
    }

    // ── no-op + borrow hygiene ──

    #[test]
    fn move_df_no_op_when_final_equals_src_start() {
        assert_eq!(move_df("=A1:B5", 3, 3, 3), "=A1:B5");
    }

    #[test]
    fn move_df_no_op_is_borrowed() {
        let out = adjust_refs_for_column_block_move_data_following("=A1:D10", 3, 3, 3);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_df_borrowed_when_no_edits_apply() {
        let out = adjust_refs_for_column_block_move_data_following("=A1+B2", 5, 5, 8);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_df_non_formula_is_borrowed() {
        let out = adjust_refs_for_column_block_move_data_following("hello", 3, 3, 2);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_df_invalid_range_is_borrowed() {
        let out = adjust_refs_for_column_block_move_data_following("=A1", 5, 2, 0);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    // ── adjust_refs_for_row_block_move ───────────────────────────────

    fn move_row(f: &str, src_start: u32, src_end: u32, final_start: u32) -> String {
        adjust_refs_for_row_block_move(f, src_start, src_end, final_start).into_owned()
    }

    // ── single-row move (row 4 → land at row 2, src 4..=4, final 2) ──

    #[test]
    fn move_row_single_cell_in_source_block() {
        assert_eq!(move_row("=B5", 4, 4, 2), "=B3");
    }

    #[test]
    fn move_row_single_cell_in_displaced_band() {
        // Row 2 is in [final_start, src_start) → shifts down by width=1.
        assert_eq!(move_row("=B3", 4, 4, 2), "=B4");
    }

    #[test]
    fn move_row_addition_two_cells_in_source() {
        assert_eq!(move_row("=B5+C5", 4, 4, 2), "=B3+C3");
    }

    #[test]
    fn move_row_preserves_both_abs_markers() {
        assert_eq!(move_row("=$B$5", 4, 4, 2), "=$B$3");
    }

    #[test]
    fn move_row_preserves_col_only_abs() {
        assert_eq!(move_row("=$B5", 4, 4, 2), "=$B3");
    }

    #[test]
    fn move_row_preserves_row_only_abs() {
        assert_eq!(move_row("=B$5", 4, 4, 2), "=B$3");
    }

    // ── bounded ranges are positional ──

    #[test]
    fn move_row_bounded_range_unchanged_outside() {
        assert_eq!(move_row("=A1:D10", 4, 4, 2), "=A1:D10");
    }

    #[test]
    fn move_row_bounded_range_unchanged_inside() {
        assert_eq!(move_row("=B3:B5", 4, 4, 2), "=B3:B5");
    }

    // ── whole-row ranges follow data via interior bbox ──

    #[test]
    fn move_row_whole_row_single_in_source() {
        assert_eq!(move_row("=SUM(5:5)", 4, 4, 2), "=SUM(3:3)");
    }

    #[test]
    fn move_row_whole_row_range_interior_bbox_unchanged() {
        // 3:5 → forward across {2,3,4} = {3,4,2} → bbox 3..=5.
        assert_eq!(move_row("=SUM(3:5)", 4, 4, 2), "=SUM(3:5)");
    }

    #[test]
    fn move_row_whole_row_range_interior_bbox_grows() {
        // 3:4 → forward across {2,3} = {3,4} → bbox 4..=5.
        assert_eq!(move_row("=SUM(3:4)", 4, 4, 2), "=SUM(4:5)");
    }

    #[test]
    fn move_row_whole_row_range_straddling() {
        // 4:6 → forward across {3,4,5} = {4,2,5} → bbox 3..=6.
        assert_eq!(move_row("=SUM(4:6)", 4, 4, 2), "=SUM(3:6)");
    }

    #[test]
    fn move_row_whole_row_range_preserves_abs_markers() {
        // Same shape as 3:5 (no bbox change), but `$` markers ride through.
        assert_eq!(move_row("=SUM($3:$5)", 4, 4, 2), "=SUM($3:$5)");
    }

    // ── whole-column ranges unaffected by row move ──

    #[test]
    fn move_row_whole_col_unchanged() {
        assert_eq!(move_row("=SUM(A:C)", 4, 4, 2), "=SUM(A:C)");
    }

    // ── spill anchor ──

    #[test]
    fn move_row_spill_anchor_follows() {
        assert_eq!(move_row("=B5#", 4, 4, 2), "=B3#");
    }

    // ── pass-through ──

    #[test]
    fn move_row_non_formula_passes_through() {
        assert_eq!(move_row("not a formula", 4, 4, 2), "not a formula");
    }

    #[test]
    fn move_row_preserves_string_literal_refs() {
        assert_eq!(
            move_row(r#"=IF(B5, "B5", "5:5")"#, 4, 4, 2),
            r#"=IF(B3, "B5", "5:5")"#
        );
    }

    // ── multi-row block move (rows 1..=3 → start at 4) ──

    #[test]
    fn move_row_block_swap_singles() {
        // Row 1 → 1-1+4 = 4 = A5. Row 4 → 4-3 = 1 = A2.
        assert_eq!(move_row("=A2+A5", 1, 3, 4), "=A5+A2");
    }

    #[test]
    fn move_row_block_whole_row_range_in_source() {
        // 2:4 → forward {1,2,3} = {4,5,6} → bbox 5..=7.
        assert_eq!(move_row("=SUM(2:4)", 1, 3, 4), "=SUM(5:7)");
    }

    #[test]
    fn move_row_block_whole_row_range_straddling() {
        // 3:6 → forward {2,3,4,5} = {5,6,1,2} → bbox 2..=7.
        assert_eq!(move_row("=SUM(3:6)", 1, 3, 4), "=SUM(2:7)");
    }

    // ── no-op shapes ──

    #[test]
    fn move_row_no_op_when_final_equals_src_start() {
        assert_eq!(move_row("=A1+B2", 4, 4, 4), "=A1+B2");
    }

    #[test]
    fn move_row_no_op_when_block_identity() {
        assert_eq!(move_row("=A1+B2", 1, 3, 1), "=A1+B2");
    }

    // ── borrowed-vs-owned hygiene ──

    #[test]
    fn move_row_no_op_is_borrowed() {
        let out = adjust_refs_for_row_block_move("=A1", 3, 3, 3);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_row_borrowed_when_no_edits_apply() {
        // Move is in rows 5..=5; refs at row 1/2 are outside the band.
        let out = adjust_refs_for_row_block_move("=A1+B2", 5, 5, 8);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_row_non_formula_is_borrowed() {
        let out = adjust_refs_for_row_block_move("hello", 4, 4, 2);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_row_invalid_range_is_borrowed() {
        // src_end < src_start → defensive no-op.
        let out = adjust_refs_for_row_block_move("=A1", 5, 2, 0);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    // ── adjust_refs_for_row_block_move_data_following ────────────────
    //
    // Differs from the sibling above only in how it treats bounded
    // ranges (CellRef:CellRef): they get rewritten via interior bbox
    // rather than left in place. Single-cell, whole-row, whole-col,
    // abs markers, and spill-anchor handling are identical, so the
    // tests below focus on (a) bounded-range cases that diverge and
    // (b) a few smoke checks that the unchanged branches still work.

    fn move_row_df(f: &str, src_start: u32, src_end: u32, final_start: u32) -> String {
        adjust_refs_for_row_block_move_data_following(f, src_start, src_end, final_start)
            .into_owned()
    }

    // ── single-cell + abs markers (smoke checks) ──

    #[test]
    fn move_row_df_single_cell_in_source_block() {
        assert_eq!(move_row_df("=B5", 4, 4, 2), "=B3");
    }

    #[test]
    fn move_row_df_preserves_both_abs_markers() {
        assert_eq!(move_row_df("=$B$5", 4, 4, 2), "=$B$3");
    }

    // ── bounded ranges follow data via interior bbox ──

    #[test]
    fn move_row_df_bounded_range_unchanged_when_band_inside_range() {
        // A3:A5 → forward {2,3,4} = {3,4,2} → bbox 2..=4 (unchanged).
        assert_eq!(move_row_df("=A3:A5", 4, 4, 2), "=A3:A5");
    }

    #[test]
    fn move_row_df_bounded_range_grows_when_straddling() {
        // A3:A4 → forward {2,3} = {3,4} → bbox 3..=4 (rows 4..=5).
        assert_eq!(move_row_df("=A3:A4", 4, 4, 2), "=A4:A5");
    }

    #[test]
    fn move_row_df_bounded_range_grows_left() {
        // A4:A6 → forward {3,4,5} = {4,2,5} → bbox 2..=5 (rows 3..=6).
        assert_eq!(move_row_df("=A4:A6", 4, 4, 2), "=A3:A6");
    }

    #[test]
    fn move_row_df_bounded_range_single_row_follows() {
        // A5:A5 → forward {4} = {2} → bbox 2..=2 (rows 3..=3).
        assert_eq!(move_row_df("=A5:A5", 4, 4, 2), "=A3:A3");
    }

    #[test]
    fn move_row_df_bounded_range_unchanged_when_bbox_unchanged() {
        // A1:D5 → forward {0,1,2,3,4} = {0,1,3,4,2} → bbox 0..=4 (range
        // fully contains the affected band).
        assert_eq!(move_row_df("=A1:D5", 4, 4, 2), "=A1:D5");
    }

    #[test]
    fn move_row_df_bounded_range_grows_down_when_band_outside() {
        // A1:D3 → forward {0,1,2} = {0,1,3} → bbox 0..=3 (rows 1..=4).
        assert_eq!(move_row_df("=A1:D3", 4, 4, 2), "=A1:D4");
    }

    #[test]
    fn move_row_df_bounded_range_preserves_abs_markers() {
        // $A$3:$D$4 → rows 2..=3, forward {2,3} = {3,4} → bbox 3..=4.
        assert_eq!(move_row_df("=$A$3:$D$4", 4, 4, 2), "=$A$4:$D$5");
    }

    // ── whole-row ranges (identical to sibling) ──

    #[test]
    fn move_row_df_whole_row_single_in_source() {
        assert_eq!(move_row_df("=SUM(5:5)", 4, 4, 2), "=SUM(3:3)");
    }

    #[test]
    fn move_row_df_whole_row_range_interior_bbox_unchanged() {
        assert_eq!(move_row_df("=SUM(3:5)", 4, 4, 2), "=SUM(3:5)");
    }

    #[test]
    fn move_row_df_whole_row_range_interior_bbox_grows() {
        assert_eq!(move_row_df("=SUM(3:4)", 4, 4, 2), "=SUM(4:5)");
    }

    // ── whole-col + spill anchor + non-formula (identical to sibling) ──

    #[test]
    fn move_row_df_whole_col_unchanged() {
        assert_eq!(move_row_df("=SUM(A:C)", 4, 4, 2), "=SUM(A:C)");
    }

    #[test]
    fn move_row_df_spill_anchor_follows() {
        assert_eq!(move_row_df("=B5#", 4, 4, 2), "=B3#");
    }

    #[test]
    fn move_row_df_non_formula_passes_through() {
        assert_eq!(move_row_df("not a formula", 4, 4, 2), "not a formula");
    }

    #[test]
    fn move_row_df_preserves_string_literal_refs() {
        assert_eq!(move_row_df(r#"="hello"+B5"#, 4, 4, 2), r#"="hello"+B3"#);
    }

    // ── multi-row block move (rows 1..=3 → start at 4) ──

    #[test]
    fn move_row_df_block_swap_singles() {
        assert_eq!(move_row_df("=A2+A5", 1, 3, 4), "=A5+A2");
    }

    #[test]
    fn move_row_df_block_bounded_range_fully_contains_band() {
        // A1:D10 → forward across {0..=9} = {0,4,5,6,1,2,3,7,8,9}
        // → bbox 0..=9 (unchanged when range covers everything).
        assert_eq!(move_row_df("=A1:D10", 1, 3, 4), "=A1:D10");
    }

    #[test]
    fn move_row_df_block_bounded_range_in_source() {
        // A2:D4 → forward {1,2,3} = {4,5,6} → bbox 4..=6 (rows 5..=7).
        assert_eq!(move_row_df("=A2:D4", 1, 3, 4), "=A5:D7");
    }

    #[test]
    fn move_row_df_block_bounded_range_straddling() {
        // A2:D6 → forward {1,2,3,4,5} = {4,5,6,1,2} → bbox 1..=6.
        assert_eq!(move_row_df("=A2:D6", 1, 3, 4), "=A2:D7");
    }

    // ── no-op + borrow hygiene ──

    #[test]
    fn move_row_df_no_op_when_final_equals_src_start() {
        assert_eq!(move_row_df("=A1:B5", 3, 3, 3), "=A1:B5");
    }

    #[test]
    fn move_row_df_no_op_is_borrowed() {
        let out = adjust_refs_for_row_block_move_data_following("=A1", 3, 3, 3);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_row_df_borrowed_when_no_edits_apply() {
        let out = adjust_refs_for_row_block_move_data_following("=A1+B2", 5, 5, 8);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_row_df_non_formula_is_borrowed() {
        let out = adjust_refs_for_row_block_move_data_following("hello", 4, 4, 2);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn move_row_df_invalid_range_is_borrowed() {
        let out = adjust_refs_for_row_block_move_data_following("=A1", 5, 2, 0);
        assert!(matches!(out, Cow::Borrowed(_)));
    }
}
