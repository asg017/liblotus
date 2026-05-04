# LotusGrid — a tiny macOS spreadsheet using `lotus-core`

A menu-bar app that summons an 8×20 grid on ⌘⇧Space and calculates formulas
through the `lotus-core` Rust engine via a C ABI bridge.

## Layout

```
examples/macos/
├── lotus-ffi/              # Rust crate: C ABI around lotus-core
│   ├── Cargo.toml           #   (standalone, not part of the workspace)
│   └── src/lib.rs
├── LotusGrid/               # SwiftUI menu-bar app
│   ├── lotus_ffi.h         #   bridging header, imported with -import-objc-header
│   ├── SheetEngine.swift    #   thin Swift wrapper over the C ABI
│   ├── GridViewModel.swift  #   raw inputs canonical in Swift, computed on demand
│   ├── ContentView.swift    #   8×20 Grid with click-to-edit cells
│   ├── HotKey.swift         #   Carbon-based global hotkey
│   ├── Persistence.swift    #   JSON in ~/Library/Application Support/LotusGrid/
│   └── LotusGridApp.swift   #   @main, NSPanel, NSStatusItem, hotkey wiring
├── Info.plist               # LSUIElement=true → no dock icon
└── build.sh                 # one-shot build into build/LotusGrid.app
```

## Build & run

```bash
./build.sh
open build/LotusGrid.app
```

Press **⌘⇧Space** to summon/dismiss the grid. The status-bar `⊞` icon works too.

## How it fits together

- `cargo build --release` in `lotus-ffi/` produces `liblotus_ffi.a`, which
  statically links `lotus-core`.
- `swiftc` compiles all `LotusGrid/*.swift`, uses `lotus_ffi.h` as an
  Objective-C bridging header (pure C is fine — it is a subset), and links
  the static lib directly (`-L … -llotus_ffi`).
- `build.sh` assembles an `.app` bundle with `Info.plist` and ad-hoc signs it
  so Gatekeeper lets you launch it locally.

## Type your first formula

1. Hit ⌘⇧Space.
2. Click `A1`, type `10`, press Return.
3. Click `A2`, type `20`, press Return.
4. Click `A3`, type `=SUM(A1:A2)`, press Return → shows `30`.

State lives at `~/Library/Application Support/LotusGrid/cells.json`.

## Extending

- **Supported functions** (from `lotus-core`): SUM, AVERAGE, MIN, MAX,
  COUNT, ABS, ROUND, IF, CONCAT, LEN, UPPER, LOWER.
- **Add a function**: edit `crates/lotus-core/src/functions.rs`, rebuild,
  rerun `./build.sh`.
- **Change the hotkey**: tweak the `keyCode` / `modifiers` defaults in
  `HotKey.swift` (constants live in `Carbon.HIToolbox.Events`).
- **Resize the grid**: change `cols` / `rows` in `GridViewModel.swift`.
