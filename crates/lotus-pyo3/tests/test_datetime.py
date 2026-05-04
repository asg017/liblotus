"""Integration tests for the optional `datetime` feature of the lotus module.

These tests are skipped when the wheel was built without `--features datetime`.
Build with: `maturin develop --features datetime`.
"""

import lotus
import pytest


def _has_datetime():
    """True if the wheel was built with the datetime feature."""
    return hasattr(lotus.Sheet(), "register_datetime")


pytestmark = pytest.mark.skipif(
    not _has_datetime(),
    reason="lotus wheel built without `datetime` feature",
)


def _sheet():
    s = lotus.Sheet()
    s.register_datetime()
    return s


# === Custom-value dict marshalling (the prereq for datetime) ===


def test_custom_value_returns_as_dict():
    s = _sheet()
    s.set_cells([("A1", "=DATE(2025, 4, 27)")])
    val = s.get_typed("A1")
    assert isinstance(val, dict)
    assert val == {"type_tag": "jdate", "data": "2025-04-27"}


def test_get_all_typed_includes_custom():
    s = _sheet()
    s.set_cells([("A1", "=DATE(2025, 4, 27)")])
    all_vals = s.get_all_typed()
    assert all_vals["A1"] == {"type_tag": "jdate", "data": "2025-04-27"}


# === Type round-trips for the six handlers ===


def test_jdate_round_trip():
    s = _sheet()
    s.set_cells([("A1", "2025-04-27")])  # literal parse
    assert s.get_typed("A1") == {"type_tag": "jdate", "data": "2025-04-27"}


def test_jtime_round_trip():
    s = _sheet()
    s.set_cells([("A1", "=TIME(14, 30, 45)")])
    assert s.get_typed("A1") == {"type_tag": "jtime", "data": "14:30:45"}


def test_jdatetime_round_trip():
    s = _sheet()
    s.set_cells([("A1", "=DATETIME(2025, 4, 27, 14, 30, 0)")])
    assert s.get_typed("A1") == {
        "type_tag": "jdatetime",
        "data": "2025-04-27T14:30:00",
    }


def test_jspan_round_trip():
    s = _sheet()
    s.set_cells([("A1", "=DAYS(7)")])
    assert s.get_typed("A1") == {"type_tag": "jspan", "data": "P7D"}


def test_jzoned_round_trip():
    s = _sheet()
    s.set_cells(
        [("A1", '=ZONED(DATETIME(2025, 4, 27, 14, 30, 0), "America/New_York")')]
    )
    val = s.get_typed("A1")
    assert val["type_tag"] == "jzoned"
    assert "[America/New_York]" in val["data"]


def test_jtimezone_round_trip():
    s = _sheet()
    s.set_cells([("A1", '=TIMEZONE("America/New_York")')])
    assert s.get_typed("A1") == {
        "type_tag": "jtimezone",
        "data": "America/New_York",
    }


# === Operator dispatch through the Python boundary ===


def test_date_plus_days():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "=DATE(2025, 4, 27)"),
            ("A2", "=A1 + DAYS(10)"),
        ]
    )
    assert s.get_typed("A2") == {"type_tag": "jdate", "data": "2025-05-07"}


def test_date_minus_date_yields_span():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "=DATE(2025, 4, 27)"),
            ("A2", "=DATE(2025, 4, 20)"),
            ("A3", "=SPAN_TO_DAYS(A1 - A2)"),
        ]
    )
    assert s.get_typed("A3") == 7


def test_year_month_day_extract_as_python_int():
    s = _sheet()
    s.set_cells(
        [
            ("A1", "=DATE(2025, 4, 27)"),
            ("B1", "=YEAR(A1)"),
            ("B2", "=MONTH(A1)"),
            ("B3", "=DAY(A1)"),
        ]
    )
    # cell_value_to_py promotes integral floats to Python int.
    assert s.get_typed("B1") == 2025
    assert isinstance(s.get_typed("B1"), int)
    assert s.get_typed("B2") == 4
    assert s.get_typed("B3") == 27


# === Error round-trip ===


def test_invalid_date_surfaces_value_error():
    s = _sheet()
    s.set_cells([("A1", "=DATE(2025, 2, 30)")])
    val = s.get_typed("A1")
    # Errors come back as their sentinel string (per cell_value_to_py).
    assert isinstance(val, str)
    assert val.startswith("#VALUE!")


# === Duplicate registration ===


def test_register_datetime_twice_errors():
    s = lotus.Sheet()
    s.register_datetime()
    with pytest.raises(ValueError):
        s.register_datetime()


# === Feature gating ===


def test_register_datetime_method_present():
    """Sanity check on the gating mechanism itself."""
    assert hasattr(lotus.Sheet(), "register_datetime")
