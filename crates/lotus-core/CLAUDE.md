# lotus-core — Formula Engine

Minimal-dependency Rust crate. The single source of truth for all formula evaluation across WASM (browser) and PyO3 (Python backend).

Current third-party deps (kept deliberately small — `lotus-core` ships everywhere the engine ships):

- **`indexmap`** — `IndexMap`-backed per-cell maps so `get_all()` / `names()` / `spills()` and the `#CIRCULAR!` error text iterate in insertion order. Net win on recalc benches.
- **`thiserror`** — proc-macro for `SheetError` (zero runtime / WASM bundle weight).

New deps need bundle-size + supply-chain justification, ideally with bench evidence. See root `CLAUDE.md` for the policy.

## Pipeline

```
Raw input ("=SUM(A1:B3)+10")
  → Lexer (lexer.rs)     → Vec<Token>
  → Parser (parser.rs)    → ASTNode tree
  → Evaluator (eval.rs)   → CellValue (number/string/empty)
```

For full-sheet recalculation, `Sheet` (dag.rs) wraps this pipeline with dependency tracking:

```
Sheet::set_cells(changes)
  → Build dependency graph (collect_refs from all formulas)
  → Topological sort (Kahn's algorithm)
  → Detect circular dependencies (fail-fast with #CIRCULAR!)
  → Evaluate in dependency order
  → Store computed values
```

## Modules

### `types.rs` — Type Definitions

- `TokenType`: NUMBER, STRING, CELL_REF, FUNCTION, OPERATOR, LPAREN, RPAREN, COMMA, COLON, EOF
- `Token { token_type, value, position }` — position tracks byte offset for reference extraction
- `AstNode`: Number, String, CellRef, Range, ColumnRange, RowRange, BinaryOp, UnaryOp, FunctionCall
- `CellValue`: Number(f64), String(String), Empty
- 1-based coordinate helpers: `types::col_to_index("AA")` → `27`, `types::index_to_col(27)` → `"AA"`, `types::cell_id("AA", 1)` → `"AA1"`. Internal engine use; both helpers saturate at `u32::MAX` (overflow returns the sentinel rather than panicking — see the comment on `col_to_index`).

⚠ **Two `col_to_index` functions exist** — pick the right one:

| Function | Indexing | Behavior on overflow / bad input | Use when |
|---|---|---|---|
| `types::col_to_index` | 1-based (`A`=1) | Saturates to `u32::MAX`, never panics. Doesn't validate input. | Inside the engine, on tokens already validated by the lexer. |
| `range::col_to_index` | 0-based (`A`=0) | Returns `Result<u32, RangeParseError>`; rejects empty / non-letter / overflow. | Anywhere taking untrusted strings, or when you need errors. |

`types::index_to_col` and `range::index_to_col` mirror the indexing split. Picking the wrong one is the source of an entire class of off-by-one and panic bugs.

### `lexer.rs` — Tokenizer

Strips leading `=`. Character-by-character scanning, no regex. Distinguishes cell refs (letters+digits like "A1") from function names (pure letters like "SUM") at tokenization time. Tracks `position` on each token for reference extraction.

### `parser.rs` — Recursive Descent Parser

Operator precedence (lowest to highest): additive → multiplicative → power → unary → primary.

Handles:
- Cell references: `A1`, `AA100`
- Ranges: `A1:B3` (CellRef COLON CellRef)
- Column ranges: `A:C` (two Function tokens that look like column letters)
- Row ranges: `1:5` (Number COLON Number)
- Function calls: `SUM(A1:A3, 10)`
- Parenthesized expressions

### `eval.rs` — AST Evaluator

Takes a `cell_resolver: Fn(&str) -> CellValue` closure. Evaluates the AST recursively. Ranges expand into flat `Vec<CellValue>`. Binary ops coerce to numbers; `+` falls back to string concatenation for non-numbers. Division by zero → `#DIV/0!` error string.

### `functions.rs` — Built-in Functions

12 functions: SUM, AVERAGE, MIN, MAX, COUNT, ABS, ROUND, IF, CONCAT, LEN, UPPER, LOWER.

All receive `&[CellValue]` (pre-flattened from ranges). `get_numbers()` helper filters non-numeric values.

### `dag.rs` — Sheet + Dependency Graph

`Sheet` struct holds `raw: CellMap<String>` and `computed: CellMap<CellValue>` (where `CellMap<V>` is `IndexMap<CellId, V>` — see `error.rs` and `dag.rs` type aliases).

`set_cells()` triggers full recalculation:
1. Merge changes into raw values
2. Parse all formulas, collect cell references → build adjacency list
3. Topological sort → evaluation order (Kahn's algorithm)
4. If sort doesn't visit all nodes → circular dependency → return error
5. Evaluate non-formulas first, then formulas in topo order

### `refs.rs` — Reference Extraction

`extract_refs(formula)` → `Vec<FormulaRef>` with source positions. Used by:
- Frontend (via WASM) for formula reference coloring while editing
- Backend (via PyO3) for potential future dependency analysis

Each `FormulaRef` has `start`, `end` (byte offsets in the formula string), `text`, and `cells` (expanded list for ranges).

## Adding a New Function

1. Add to `functions.rs`: `"MYFUNC" => |args| { ... }`
2. That's it — the lexer recognizes any all-caps identifier as a function name, the parser handles `FUNC(args)`, and the evaluator looks it up in the function table.

## Error Handling

Two error families with different shapes:

- **Top-level Sheet API** (`set_cells`, `set_name`, `remove_name`) returns `Result<(), SheetError>` — a `thiserror`-derived enum (`Circular { cells }`, `InvalidName`, `InvalidDefinition`). `Display` matches the prior strings (`"#CIRCULAR! Cycle detected involving: ..."`) so binding crates that surface the message keep their behaviour.
- **Per-cell formula errors** (`#DIV/0!`, `#REF!`, `#VALUE!`, `#NAME?`, `#SIZE!`, `#SPILL!`) still flow as `Result<_, String>` internally and end up as `CellValue::String("#FOO!")` in the cell. This is a value-vehicle pattern, not a Rust error — callers see the error in the cell value, not as `Err`. Typing this is on the backlog (dex `T14`).
