# lotus-text

A line-based text format (`.sheet`), snapshot format (`.sheet.snap`), linter,
and interactive REPL for [`lotus-core`](../../crates/lotus-core). Built for quick
hand-editing, test fixtures, and LLM-friendly round-trips.

## The `.sheet` format

One cell per line, `CELL_ID: raw_value`. Comments start with `#`, blank lines
are ignored, duplicate cells are last-wins, and an empty RHS clears the cell.

```
# revenue model
A1: =SUM(B1:B3)
A2: =AVERAGE(B1:B3)
B1: 10
B2: 20
B3: 30
```

- Cell IDs are uppercase letters + digits (`A1`, `AA12`, `Z99`).
- A single leading space after `:` is stripped; trailing whitespace is trimmed.
- Formulas start with `=`; anything else is a literal (number or string —
  the engine decides at eval time).

Cells are emitted sorted by column then row, so diffs are stable.

## The `.sheet.snap` format

A `.sheet` with a ` => computed` suffix on each cell line. The delimiter is
the literal sequence space-arrow-space, which never clashes with `>=` in a
formula.

```
A1: =SUM(B1:B3) => 60
B1: 10 => 10
B2: 20 => 20
B3: 30 => 30
```

Any `.sheet.snap` also parses as a `.sheet` (the tails are ignored).
`verify_snapshot` re-evaluates every raw value and reports mismatches —
handy for pinning engine behavior in tests.

## Library usage

```rust
use lotus_text::{parse, format_snapshot, lint, verify_snapshot};

let sheet = parse("A1: =1+1\nB1: =A1*3\n")?;
assert_eq!(sheet.get("B1").to_string(), "6");

let snap = format_snapshot(&sheet);
verify_snapshot(&snap)?;

for diag in lint("A1: =NOPE(1)\n") {
    println!("{diag}");
}
```

Public surface:

| Function | Purpose |
|---|---|
| `parse(&str) -> Result<TextSheet, ParseError>` | Parse + evaluate |
| `format_compact(&TextSheet) -> String` | Canonical `.sheet` output |
| `format_snapshot(&TextSheet) -> String` | Canonical `.sheet.snap` output |
| `verify_snapshot(&str) -> Result<(), VerifyError>` | Check snapshot tails |
| `lint(&str) -> Vec<Diagnostic>` | Diagnose a source string |
| `lint_sheet(&TextSheet) -> Vec<Diagnostic>` | Diagnose an in-memory sheet |

## REPL

```
cargo run -p lotus-text --bin lotus-repl [file.sheet]
```

Starts empty, or loads the given file.

Backed by rustyline, so the usual line-editing keys work:

- **Up / Down** — scroll through command history (persisted across sessions at `~/.cache/lotus-repl/history`)
- **Ctrl-R** — reverse-incremental search
- **Ctrl-A / Ctrl-E** — jump to line start / end
- **Ctrl-C** — discard the current line and prompt again
- **Ctrl-D** — exit

Input is syntax-highlighted as you type: cell refs in cyan, numbers in
magenta, function names in yellow, strings in green, operators in red. The
same palette is applied to `:show`, `:snap`, `?`, and `:why` output when
stdout is a TTY. Piping to a non-TTY (redirect, pipeline) suppresses colors
automatically.

| Input | Effect |
|---|---|
| `A1 = 10` | Set a cell (also accepts `A1: 10`) |
| `A1 = =SUM(B1:B3)` | Set a formula |
| `?A1` | Print raw + computed value |
| `:show` | Dump the sheet as `.sheet` |
| `:snap` | Dump the sheet as `.sheet.snap` |
| `:lint` | Run diagnostics on the current sheet |
| `:why A1` | Show the references in `A1` and their values |
| `:load path` | Replace state with a file |
| `:save path` | Write current state to a file |
| `:clear` | Reset to an empty sheet |
| `:help` | List commands |
| `:quit` | Exit (also `:q`, `:exit`, ctrl-D) |

Use the `=` form (`A1 = =SUM(B1:B3)`) to avoid the colon-in-range ambiguity
that the `A1: =SUM(B1:B3)` form otherwise exposes on input.

Example session:

```
> A1 = 10
A1 = 10
> B1 = =A1*2
B1 = 20
> ?B1
B1: =A1*2  =>  20
> :snap
A1: 10 => 10
B1: =A1*2 => 20
> :quit
```

## Linter rules

| Severity | Rule |
|---|---|
| Error | Syntax error (bad cell id, missing `:`, leading whitespace) |
| Error | Unknown function name |
| Error | Circular dependency (engine returns `#CIRCULAR!`) |
| Error | Evaluation error (`#DIV/0!`, etc. as a computed value) |
| Warning | Formula references a cell that is not authored in the sheet |

The function-name check probes the engine's table at lint time, so new
functions in `lotus-core` are picked up automatically.

## Snapshot testing pattern

Pin engine behavior by storing a `.sheet.snap` file next to your test and
asserting that it re-verifies:

```rust
#[test]
fn revenue_model_unchanged() {
    let snap = include_str!("fixtures/revenue.sheet.snap");
    lotus_text::verify_snapshot(snap).unwrap();
}
```

To regenerate, parse the raw `.sheet`, call `format_snapshot`, and write it
back.
