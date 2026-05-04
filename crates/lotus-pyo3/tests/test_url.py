"""Integration tests for the optional `url` feature of the lotus module.

These tests are skipped when the wheel was built without `--features url`.
Build with: `maturin develop --features url`.
"""

import lotus
import pytest


def _has_url():
    """True if the wheel was built with the url feature."""
    return hasattr(lotus.Sheet(), "register_url")


pytestmark = pytest.mark.skipif(
    not _has_url(),
    reason="lotus wheel built without `url` feature",
)


def _sheet():
    s = lotus.Sheet()
    s.register_url()
    return s


# === Component extraction ===


def test_url_scheme_host_path_query_fragment():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "https://example.com:8443/blog/2025?ref=hn#sec-2"),
            ("B1", "=URL_SCHEME(A1)"),
            ("B2", "=URL_HOST(A1)"),
            ("B3", "=URL_PORT(A1)"),
            ("B4", "=URL_PATH(A1)"),
            ("B5", "=URL_QUERY(A1)"),
            ("B6", "=URL_FRAGMENT(A1)"),
        ]
    )
    assert s.get_typed("B1") == "https"
    assert s.get_typed("B2") == "example.com"
    assert s.get_typed("B3") == 8443
    assert isinstance(s.get_typed("B3"), int)
    assert s.get_typed("B4") == "/blog/2025"
    assert s.get_typed("B5") == "ref=hn"
    assert s.get_typed("B6") == "sec-2"


def test_url_port_returns_zero_when_implicit():
    s = _sheet()
    s.set_cells([("A1", "https://x.com/"), ("B1", "=URL_PORT(A1)")])
    assert s.get_typed("B1") == 0


# === Path segments (the Instagram-username case) ===


def test_url_path_segment_negative_index_extracts_last():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "https://www.instagram.com/localfixture/"),
            ("B1", "=URL_PATH_SEGMENT(A1, 1)"),
            ("B2", "=URL_PATH_SEGMENT(A1, -1)"),
        ]
    )
    assert s.get_typed("B1") == "localfixture"
    assert s.get_typed("B2") == "localfixture"


def test_url_path_segment_decodes_percent_encoded():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "https://x.com/hello%20world/foo"),
            ("B1", "=URL_PATH_SEGMENT(A1, 1)"),
        ]
    )
    assert s.get_typed("B1") == "hello world"


def test_url_path_segment_out_of_range_yields_empty():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "https://x.com/a"),
            ("B1", "=URL_PATH_SEGMENT(A1, 5)"),
        ]
    )
    assert s.get_typed("B1") == ""


# === Query parameters ===


def test_url_param_returns_first_match_decoded():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "https://x.com/?a=1&a=2&q=hi%20there"),
            ("B1", '=URL_PARAM(A1, "a")'),
            ("B2", '=URL_PARAM(A1, "q")'),
            ("B3", '=URL_PARAM(A1, "missing")'),
        ]
    )
    assert s.get_typed("B1") == "1"
    assert s.get_typed("B2") == "hi there"
    assert s.get_typed("B3") == ""


# === Validation + encoding ===


def test_url_valid_distinguishes_good_and_bad():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "https://x.com/"),
            ("A2", "not a url"),
            ("B1", "=URL_VALID(A1)"),
            ("B2", "=URL_VALID(A2)"),
        ]
    )
    assert s.get_typed("B1") is True
    assert s.get_typed("B2") is False


def test_url_encode_decode_round_trip():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "hello world & friends?"),
            ("B1", "=URL_ENCODE(A1)"),
            ("B2", "=URL_DECODE(B1)"),
        ]
    )
    assert s.get_typed("B1") == "hello%20world%20%26%20friends%3F"
    assert s.get_typed("B2") == "hello world & friends?"


def test_url_join_resolves_relative_against_base():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "https://docs.rs/url/latest/url/"),
            ("A2", "struct.Url.html"),
            ("B1", "=URL_JOIN(A1, A2)"),
        ]
    )
    assert s.get_typed("B1") == "https://docs.rs/url/latest/url/struct.Url.html"


# === Error round-trip ===


def test_invalid_url_surfaces_value_error():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "not a url"),
            ("B1", "=URL_HOST(A1)"),
        ]
    )
    val = s.get_typed("B1")
    assert isinstance(val, str)
    assert val.startswith("#VALUE!")


# === Duplicate registration ===


def test_register_url_twice_errors():
    s = lotus.Sheet()
    s.register_url()
    with pytest.raises(ValueError):
        s.register_url()


# === Feature gating ===


def test_register_url_method_present():
    """Sanity check on the gating mechanism itself."""
    assert hasattr(lotus.Sheet(), "register_url")
