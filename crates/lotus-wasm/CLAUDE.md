# lotus-wasm — WebAssembly Bindings

Thin wasm-bindgen wrapper around `lotus-core`. Compiled to WASM for browser-side formula evaluation.

## API (exposed to JavaScript)

```typescript
class WasmSheet {
  constructor();
  set_cells(changes_json: string): void;  // JSON: [["A1","42"],["A2","=A1*2"]]
  get(cell_id: string): string;
  get_all(): string;                       // JSON: {"A1":"42","A2":"84"}
  static evaluate(formula: string): string;
  static extract_refs(formula: string): string; // JSON: [{start,end,text,cells}]
}
```

All complex data passes as JSON strings (wasm-bindgen limitation for complex types).

## Build

```bash
wasm-pack build --target bundler --out-dir pkg
```

Output in `pkg/`: `.wasm` binary, `.js` glue, `.d.ts` types, `package.json`.

The frontend imports from `../../rust/crates/lotus-wasm/pkg/lotus_wasm` and Vite bundles the WASM as a static asset.

## Error Handling

`set_cells()` throws `JsError` on circular dependencies. `evaluate()` returns error strings (e.g. `"#DIV/0!"`) as the result value.
