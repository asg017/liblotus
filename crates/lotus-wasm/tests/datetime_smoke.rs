//! Build-and-call smoke tests for the optional `datetime` feature.
//!
//! These exercise the same code paths a browser would hit (the
//! `WasmSheet` methods themselves), but run as native Rust tests so we
//! don't need a JS test harness for CI. The wasm-bindgen attributes
//! don't prevent direct Rust invocation.
//!
//! Compiled out unless built with `--features datetime`.

#![cfg(feature = "datetime")]

use lotus_wasm::WasmSheet;

#[test]
fn register_datetime_then_use_date_formula() {
    let mut s = WasmSheet::new();
    s.register_datetime().expect("registration succeeds on a fresh sheet");
    s.set_cells(r#"[["A1","=DATE(2025, 4, 27)"],["A2","=YEAR(A1)"]]"#)
        .expect("set_cells succeeds");
    // Date renders via the registry path (Display swaps T for space etc.).
    assert_eq!(s.get("A2"), "2025");
}

#[test]
fn list_functions_includes_datetime_after_register() {
    // The repro case from TODO-liblotus-list-functions-instance.md: a
    // datetime user typing `=YEAR(...)` should be able to discover the
    // function via list_functions on the sheet that has it registered.
    let mut s = WasmSheet::new();
    let baseline = s.list_functions();
    s.register_datetime().unwrap();
    let after = s.list_functions();
    assert!(after.len() > baseline.len(), "expected datetime fns to extend the list");
    // YEAR / NOW / TODAY are part of the datetime extension and should be
    // visible after register_datetime — not before.
    assert!(after.contains(r#""YEAR""#), "YEAR missing from list_functions: {after}");
    assert!(after.contains(r#""TODAY""#), "TODAY missing from list_functions: {after}");
    assert!(!baseline.contains(r#""YEAR""#), "YEAR leaked into builtin-only baseline");
}

#[test]
fn signature_help_resolves_datetime_function() {
    let mut s = WasmSheet::new();
    s.register_datetime().unwrap();
    let sig = s.signature_help("=YEAR(", 6);
    assert!(sig.contains(r#""name":"YEAR""#), "expected YEAR signature, got: {sig}");
    assert!(sig.contains(r#""active_param":0"#), "expected first param active, got: {sig}");
}

// Note: duplicate-registration error testing is skipped from native
// Rust tests because constructing `JsError` calls a wasm-imported
// function that panics on non-wasm targets. Browser-side tests in
// `tests/integration.ts` cover that path.

#[test]
fn span_formula_round_trip() {
    let mut s = WasmSheet::new();
    s.register_datetime().unwrap();
    s.set_cells(r#"[["A1","=DATE(2025, 4, 27)"],["A2","=A1 + DAYS(10)"]]"#).unwrap();
    assert_eq!(s.get("A2"), "2025-05-07");
}

#[test]
fn set_cells_typed_with_custom_jdate_via_strptime() {
    // The end-to-end JS-flow this feature was designed for: the embedder
    // reads "4/2", asks the date handler to parse it with %d/%m/%Y, then
    // writes the resulting jdate into a cell. Round-trip inside Rust
    // since constructing JsValue panics off-wasm; we read computed back
    // through the registry-rendered display string.
    let mut s = WasmSheet::new();
    s.register_datetime().unwrap();

    // The probe API would produce {type_tag: "jdate", data: "2026-02-04"}
    // for "4/2/2026" with "%d/%m/%Y"; here we just hand-build that
    // payload and verify set_cells_typed accepts it.
    let payload = r#"[
        ["A1", { "kind": "custom", "type_tag": "jdate", "data": "2026-02-04" }]
    ]"#;
    s.set_cells_typed(payload).unwrap();
    assert_eq!(s.get("A1"), "2026-02-04");
}

#[test]
fn set_cells_typed_string_kind_forces_literal_text() {
    // "42" would auto-classify as a number through set_cells; the typed
    // string kind keeps it as text.
    let mut s = WasmSheet::new();
    let payload = r#"[["A1", { "kind": "string", "value": "42" }]]"#;
    s.set_cells_typed(payload).unwrap();
    // Display of a string is the string itself.
    assert_eq!(s.get("A1"), "42");
    // And it really is a string — concatenation would fail on a Number.
    s.set_cells(r#"[["B1","=A1 & \" suffix\""]]"#).unwrap();
    assert_eq!(s.get("B1"), "42 suffix");
}

#[test]
fn set_cells_typed_kind_raw_matches_set_cells() {
    let mut s = WasmSheet::new();
    let payload = r#"[
        ["A1", { "kind": "raw", "value": "10" }],
        ["B1", { "kind": "raw", "value": "=A1*2" }]
    ]"#;
    s.set_cells_typed(payload).unwrap();
    assert_eq!(s.get("A1"), "10");
    assert_eq!(s.get("B1"), "20");
}

#[test]
fn set_cells_typed_kind_empty_deletes() {
    let mut s = WasmSheet::new();
    s.set_cells(r#"[["A1","42"]]"#).unwrap();
    assert_eq!(s.get("A1"), "42");
    s.set_cells_typed(r#"[["A1", { "kind": "empty" }]]"#).unwrap();
    assert_eq!(s.get("A1"), "");
}

