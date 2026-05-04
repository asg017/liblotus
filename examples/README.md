# Examples

Reference implementations built on top of [`lotus-core`](../crates/lotus-core).
They exist to show idiomatic ways to embed the formula engine, and to exercise
it against real workloads (CLI, TUI, GUI, persistence).

| Example | Kind | Run |
|---------|------|-----|
| [`lotus-sqlite`](./lotus-sqlite) | Rust library — SQLite-backed cell store with recalc | `cargo test -p lotus-sqlite` |
| [`lotus-text`](./lotus-text) | Rust library + REPL — `.sheet` text format, linter, snapshot tests | `cargo run -p lotus-text --bin lotus-repl` |
| [`macos`](./macos) | SwiftUI menu-bar app via C ABI (standalone, not in workspace) | `cd examples/macos && ./build.sh && open build/LotusGrid.app` |

`lotus-sqlite` and `lotus-text` are regular members of the Cargo workspace, so
they build and test alongside `lotus-core` via `cargo build` / `cargo test`
from the repo root. The `macos/` example is its own build — see its README.

`vlotus` (vim-style terminal spreadsheet) used to live here but has been split
out to `../vlotus` as a sibling project with a path dep on `lotus-core`.
