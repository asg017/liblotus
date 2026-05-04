# liblotus — Spreadsheet Engine Workspace

Cargo workspace split into library crates (`crates/`) and demo apps (`examples/`). `lotus-core` is the formula engine, kept to a deliberately minimal dependency footprint; `lotus-pyo3`/`lotus-wasm` are the first-party bindings. Everything under `examples/` is reference code showing how to build on top of the engine.

## Read these before grepping

Each large crate has its own `CLAUDE.md` with a module map and the load-bearing conventions. Read the relevant one *first* — it will save many grep/find rounds:

- **`crates/lotus-core/CLAUDE.md`** — pipeline (lexer → parser → eval → DAG), per-module roles, error families. Read for any engine work.
- `examples/lotus-text/` and `examples/macos/` have their own READMEs.

`vlotus` (the vim-style TUI) lives outside this workspace at `../vlotus`, with a
path dep on `lotus-core`. Read `../vlotus/CLAUDE.md` and `../vlotus/KEYMAP.md`
before touching any vlotus binding.

Common debugging entry points:

- **Panic on formula input** (e.g. `=AAAAAAAA1`) — call chain bottoms out at `lotus_core::extract_refs → parse_ref_idx → types::col_to_index`. Per-keystroke panics live on this path.
- **Reproducing engine bugs** — write `#[test]` inside `lotus-core` and run `cargo test -p lotus-core <name>`. Don't write `/tmp/*.rs` standalone files.

⚠ **Two `col_to_index` functions exist** in `lotus-core`. They behave differently — see `crates/lotus-core/CLAUDE.md` ("Modules → types.rs") for the disambiguation.

## Dependency policy for `lotus-core`

`lotus-core` ships everywhere the engine ships (browser via WASM, Python wheel, native FFI), so deps are scrutinised. Currently the only third-party deps are:

- **`indexmap`** — deterministic per-cell iteration; net perf win on recalc benches (5-9% across chain/grid/update/spill).
- **`thiserror`** — compile-time only (proc-macro), zero WASM bundle weight; powers `SheetError` for the public Sheet mutation API.

New deps should come with a justified bundle-size + supply-chain rationale, ideally with bench evidence. SmallVec was tried for AST arg vectors and reverted — bench evidence didn't support it (see dex `T12` for the reasoning).

## Library crates (`crates/`)

| Crate | Purpose | Dependencies |
|-------|---------|-------------|
| `lotus-core` | Formula engine: lexer, parser, evaluator, DAG recalc | indexmap, thiserror |
| `lotus-datetime` | Optional jiff-backed datetime extension: `jdate`/`jtime`/`jdatetime`/`jzoned`/`jtimezone`/`jspan` cell types and formula functions | lotus-core, jiff |
| `lotus-url` | Optional URL parsing extension: `URL_SCHEME`/`URL_HOST`/`URL_PATH_SEGMENT`/`URL_PARAM`/`URL_ENCODE`/`URL_JOIN` etc. No new cell types — URLs stay as text. | lotus-core, url, percent-encoding |
| `lotus-wasm` | Browser bindings via wasm-bindgen. Optional `datetime` and `url` features pull in the matching extension crates (`datetime` adds ~240 KB gz with IANA tzdb + `js_sys` clock; `url` adds ~80–120 KB gz with `idna` Unicode tables). | lotus-core, wasm-bindgen, serde_json, lotus-datetime (opt-in), lotus-url (opt-in) |
| `lotus-pyo3` | Python bindings via PyO3/maturin. Optional `datetime` and `url` features expose `Sheet.register_datetime()` / `Sheet.register_url()`. | lotus-core, pyo3 |

## Examples (`examples/`)

| Example | What it demonstrates | Dependencies |
|---------|----------------------|--------------|
| `lotus-text` | `.sheet` text format, linter, snapshot testing, REPL | lotus-core, rustyline |
| `macos/` | SwiftUI menu-bar spreadsheet via C ABI (standalone, not in workspace) | lotus-core via C FFI |

`vlotus` (vim-style terminal spreadsheet) lives at `../vlotus` as a sibling
project — see its own `Cargo.toml` and `CLAUDE.md`.

## Building

```bash
# Python module (for backend)
cd crates/lotus-pyo3 && maturin develop

# WASM module (for frontend)
wasm-pack build crates/lotus-wasm --target bundler --out-dir pkg

# WASM module + jiff datetime extension (adds ~240 KB gz to the bundle)
wasm-pack build crates/lotus-wasm --target bundler --out-dir pkg --features datetime

# WASM module + URL extension (adds ~80–120 KB gz to the bundle)
wasm-pack build crates/lotus-wasm --target bundler --out-dir pkg --features url

# Run all Rust tests
cargo test
```

The Justfile wraps these: `just engine` builds PyO3, and `just frontend` depends on the WASM being pre-built.

## Testing

```bash
cargo test                          # All crates
cargo test -p lotus-core           # Core engine only
cargo test -p lotus-core -- refs   # Specific module
```

lotus-pyo3 also has Python integration tests in `crates/lotus-pyo3/tests/test_integration.py` (run via `uv run pytest`).

## Linting

Run clippy across the whole workspace **before every commit** and fix any errors *and* warnings — the workspace is kept warning-free, so a new warning is treated as a regression:

```bash
cargo clippy --workspace --all-targets
```

`--all-targets` is required: it covers tests, examples, and benches, where lints often differ from the default lib build (e.g. `approx_constant` is `deny` in test code).

## Running examples

```bash
cargo run -p lotus-text --bin lotus-repl     # text-format REPL
# vlotus (terminal sheet) — sibling project: cd ../vlotus && cargo run
# macOS app: cd examples/macos && ./build.sh
```

## Working efficiently in this repo

Past sessions on small fixes (single-letter overflow panic, multi-column autofit, autocomplete prefix gate) burned 5–10× more tokens than the diff justified. The common waste patterns and the rules that prevent them:

- **Read the per-crate CLAUDE.md first** (see "Read these before grepping" above). The pointers there usually collapse a 10-grep search to a 1-read.
- **Skip subagent dispatch for single-file fixes <200 lines.** The subagent does the same exploration the parent will then re-do; you pay twice.
- **Skip TaskCreate for ≤3 linear steps.** Each TaskCreate/TaskUpdate re-caches the whole task list and dwarfs the work being tracked.
- **Read wide, not narrow.** One `Read` at offset N with `limit: 200` beats three reads at adjacent offsets. Don't re-read a file you've already read in the same conversation unless you've edited it.
- **Grep before you Read.** A `grep -n "<symbol>"` across the crate is 10× cheaper than reading large files at a guessed offset.
