//! End-to-end smoke tests: register the URL extension and exercise
//! every public formula through a real `Sheet`.

use std::sync::Arc;

use lotus_core::{CellValue, Registry, Sheet};

fn sheet() -> Sheet {
    let mut reg = Registry::new();
    lotus_url::register(&mut reg).unwrap();
    Sheet::new_with_registry(Arc::new(reg))
}

fn text(s: &str) -> CellValue {
    CellValue::String(s.to_string())
}

const INSTAGRAM: &str = "https://www.instagram.com/localfixture/";
const COMPLEX: &str = "https://user@example.com:8443/blog/2025/post-slug?ref=hn&q=hi%20there#sec-2";

#[test]
fn scheme_host_port_path() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), COMPLEX.into()),
        ("B1".into(), "=URL_SCHEME(A1)".into()),
        ("B2".into(), "=URL_HOST(A1)".into()),
        ("B3".into(), "=URL_PORT(A1)".into()),
        ("B4".into(), "=URL_PATH(A1)".into()),
        ("B5".into(), "=URL_QUERY(A1)".into()),
        ("B6".into(), "=URL_FRAGMENT(A1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), text("https"));
    assert_eq!(s.get("B2"), text("example.com"));
    assert_eq!(s.get("B3"), CellValue::Number(8443.0));
    assert_eq!(s.get("B4"), text("/blog/2025/post-slug"));
    assert_eq!(s.get("B5"), text("ref=hn&q=hi%20there"));
    assert_eq!(s.get("B6"), text("sec-2"));
}

#[test]
fn port_returns_zero_when_implicit() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "https://x.com/".into()),
        ("B1".into(), "=URL_PORT(A1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Number(0.0));
}

#[test]
fn path_segment_extracts_instagram_username() {
    let mut s = sheet();
    s.set_cells(&[
        ("A2".into(), INSTAGRAM.into()),
        ("B1".into(), "=URL_PATH_SEGMENT(A2, 1)".into()),
        ("B2".into(), "=URL_PATH_SEGMENT(A2, -1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), text("localfixture"));
    assert_eq!(s.get("B2"), text("localfixture"));
}

#[test]
fn path_segment_indexing_forward_and_negative() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "https://example.com/blog/2025/post-slug".into()),
        ("B1".into(), "=URL_PATH_SEGMENT(A1, 1)".into()),
        ("B2".into(), "=URL_PATH_SEGMENT(A1, 2)".into()),
        ("B3".into(), "=URL_PATH_SEGMENT(A1, 3)".into()),
        ("B4".into(), "=URL_PATH_SEGMENT(A1, -1)".into()),
        ("B5".into(), "=URL_PATH_SEGMENT(A1, -2)".into()),
        // Out of range → empty
        ("B6".into(), "=URL_PATH_SEGMENT(A1, 4)".into()),
        ("B7".into(), "=URL_PATH_SEGMENT(A1, -4)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), text("blog"));
    assert_eq!(s.get("B2"), text("2025"));
    assert_eq!(s.get("B3"), text("post-slug"));
    assert_eq!(s.get("B4"), text("post-slug"));
    assert_eq!(s.get("B5"), text("2025"));
    assert_eq!(s.get("B6"), text(""));
    assert_eq!(s.get("B7"), text(""));
}

#[test]
fn path_segment_decodes_percent_encoded() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "https://x.com/hello%20world/foo".into()),
        ("B1".into(), "=URL_PATH_SEGMENT(A1, 1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), text("hello world"));
}

#[test]
fn path_segment_zero_index_is_value_error() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "https://x.com/a".into()),
        ("B1".into(), "=URL_PATH_SEGMENT(A1, 0)".into()),
    ])
    .unwrap();
    assert!(matches!(s.get("B1"), CellValue::Error(_)));
}

#[test]
fn param_returns_first_match_decoded() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "https://x.com/?a=1&a=2&q=hi%20there".into()),
        ("B1".into(), "=URL_PARAM(A1, \"a\")".into()),
        ("B2".into(), "=URL_PARAM(A1, \"q\")".into()),
        ("B3".into(), "=URL_PARAM(A1, \"missing\")".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), text("1"));
    assert_eq!(s.get("B2"), text("hi there"));
    assert_eq!(s.get("B3"), text(""));
}

#[test]
fn valid_distinguishes_good_and_bad() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "https://x.com/".into()),
        ("A2".into(), "not a url".into()),
        ("B1".into(), "=URL_VALID(A1)".into()),
        ("B2".into(), "=URL_VALID(A2)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), CellValue::Boolean(true));
    assert_eq!(s.get("B2"), CellValue::Boolean(false));
}

#[test]
fn extractor_errors_on_invalid_url() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "not a url".into()),
        ("B1".into(), "=URL_HOST(A1)".into()),
    ])
    .unwrap();
    assert!(matches!(s.get("B1"), CellValue::Error(_)));
}

#[test]
fn encode_decode_roundtrip() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "hello world & friends?".into()),
        ("B1".into(), "=URL_ENCODE(A1)".into()),
        ("B2".into(), "=URL_DECODE(B1)".into()),
    ])
    .unwrap();
    assert_eq!(
        s.get("B1"),
        text("hello%20world%20%26%20friends%3F"),
    );
    assert_eq!(s.get("B2"), text("hello world & friends?"));
}

#[test]
fn join_resolves_relative_against_base() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "https://docs.rs/url/latest/url/".into()),
        ("A2".into(), "struct.Url.html".into()),
        ("B1".into(), "=URL_JOIN(A1, A2)".into()),
        // Absolute relative wins over base path
        ("A3".into(), "/foo".into()),
        ("B2".into(), "=URL_JOIN(A1, A3)".into()),
    ])
    .unwrap();
    assert_eq!(
        s.get("B1"),
        text("https://docs.rs/url/latest/url/struct.Url.html"),
    );
    assert_eq!(s.get("B2"), text("https://docs.rs/foo"));
}

#[test]
fn cannot_be_base_url_yields_empty_segment() {
    let mut s = sheet();
    s.set_cells(&[
        ("A1".into(), "mailto:foo@example.com".into()),
        ("B1".into(), "=URL_PATH_SEGMENT(A1, 1)".into()),
    ])
    .unwrap();
    assert_eq!(s.get("B1"), text(""));
}
