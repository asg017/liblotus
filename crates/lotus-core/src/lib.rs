pub mod types;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod functions;
pub mod eval;
pub mod names;
pub mod complete;
pub mod custom;
pub mod dag;
pub mod refs;
pub mod range;
pub mod tokens;

// Re-export the main public API.
pub use complete::{
    complete, complete_with_registry, list_functions, list_functions_with_registry,
    signature_help, signature_help_with_registry, CompletionItem, CompletionKind, CompletionList,
    ExampleInfo, FunctionInfo, ParamInfo, SignatureHelp,
};
pub use custom::{CustomFunction, CustomTypeHandler, Registry, RegistryError};
pub use dag::{CellInput, CellMap, NameMap, Sheet, SpillError, SpillRegion};
pub use error::{FormulaError, SheetError};
pub use eval::Evaluator;
pub use names::resolve_names;
pub use range::{
    cell_id, col_to_index, format_cell_id, format_range, index_to_col, is_unbounded_range,
    parse_cell_id, parse_range, CellCoord, ParsedRange, RangeParseError,
};
pub use refs::{
    adjust_refs_for_column_block_move, adjust_refs_for_column_block_move_data_following,
    adjust_refs_for_deletion, adjust_refs_for_insertion, adjust_refs_for_row_block_move,
    adjust_refs_for_row_block_move_data_following, extract_refs, rewrite_refs_for_deletion,
    shift_formula_refs, Deletion, FormulaRef, Insertion, RefKind,
};
pub use tokens::{formula_tokens, FormulaToken, TokenKind};
pub use types::{ArrayValue, BinaryOp, CellValue, CompareOp, CustomValue, MAX_RANGE_CELLS};
