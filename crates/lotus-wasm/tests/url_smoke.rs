//! Build-and-call smoke tests for the optional `url` feature.
//!
//! Mirrors `datetime_smoke.rs`: exercises the same code paths a browser
//! would hit (the `WasmSheet` methods themselves), but runs as native
//! Rust tests so we don't need a JS test harness for CI.
//!
//! Compiled out unless built with `--features url`.

#![cfg(feature = "url")]

use lotus_wasm::WasmSheet;

#[test]
fn register_url_then_use_path_segment() {
    let mut s = WasmSheet::new();
    s.register_url().expect("registration succeeds on a fresh sheet");
    s.set_cells(
        r#"[["A1","https://www.instagram.com/localfixture/"],["A2","=URL_PATH_SEGMENT(A1, -1)"]]"#,
    )
    .expect("set_cells succeeds");
    assert_eq!(s.get("A2"), "localfixture");
}

#[test]
fn list_functions_includes_url_after_register() {
    let mut s = WasmSheet::new();
    let baseline = s.list_functions();
    s.register_url().unwrap();
    let after = s.list_functions();
    assert!(after.len() > baseline.len(), "expected url fns to extend the list");
    // URL_HOST / URL_ENCODE are part of the url extension and should be
    // visible after register_url — not before.
    assert!(after.contains(r#""URL_HOST""#), "URL_HOST missing from list_functions: {after}");
    assert!(after.contains(r#""URL_ENCODE""#), "URL_ENCODE missing from list_functions: {after}");
    assert!(!baseline.contains(r#""URL_HOST""#), "URL_HOST leaked into builtin-only baseline");
}

#[test]
fn signature_help_resolves_url_function() {
    let mut s = WasmSheet::new();
    s.register_url().unwrap();
    let sig = s.signature_help("=URL_HOST(", 10);
    assert!(sig.contains(r#""name":"URL_HOST""#), "expected URL_HOST signature, got: {sig}");
    assert!(sig.contains(r#""active_param":0"#), "expected first param active, got: {sig}");
}

// Note: duplicate-registration error testing is skipped from native
// Rust tests because constructing `JsError` calls a wasm-imported
// function that panics on non-wasm targets. Browser-side tests would
// cover that path.

#[test]
fn url_param_extracts_query_value() {
    let mut s = WasmSheet::new();
    s.register_url().unwrap();
    s.set_cells(
        r#"[["A1","https://x.com/?q=hi%20there&ref=hn"],["A2","=URL_PARAM(A1, \"q\")"]]"#,
    )
    .unwrap();
    assert_eq!(s.get("A2"), "hi there");
}

#[test]
fn url_join_resolves_relative_against_base() {
    let mut s = WasmSheet::new();
    s.register_url().unwrap();
    s.set_cells(
        r#"[["A1","https://docs.rs/url/latest/url/"],["A2","struct.Url.html"],["A3","=URL_JOIN(A1, A2)"]]"#,
    )
    .unwrap();
    assert_eq!(s.get("A3"), "https://docs.rs/url/latest/url/struct.Url.html");
}

#[test]
fn url_encode_decode_roundtrip() {
    let mut s = WasmSheet::new();
    s.register_url().unwrap();
    s.set_cells(
        r#"[["A1","hello world & friends?"],["A2","=URL_ENCODE(A1)"],["A3","=URL_DECODE(A2)"]]"#,
    )
    .unwrap();
    assert_eq!(s.get("A2"), "hello%20world%20%26%20friends%3F");
    assert_eq!(s.get("A3"), "hello world & friends?");
}
