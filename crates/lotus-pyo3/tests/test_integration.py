"""Integration tests for the lotus Python module."""

import lotus


def test_literal_values():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "42"), ("A2", "hello")])
    assert sheet.get("A1") == "42"
    assert sheet.get("A2") == "hello"
    assert sheet.get("Z1") is None


def test_formula_evaluation():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "10"), ("A2", "20"), ("A3", "=A1+A2")])
    assert sheet.get("A3") == "30"


def test_sum_range():
    sheet = lotus.Sheet()
    sheet.set_cells(
        [
            ("A1", "1"),
            ("A2", "2"),
            ("A3", "3"),
            ("A4", "=SUM(A1:A3)"),
        ]
    )
    assert sheet.get("A4") == "6"


def test_transitive_dependency_and_update():
    sheet = lotus.Sheet()
    sheet.set_cells(
        [
            ("A1", "1"),
            ("A2", "=A1+1"),
            ("A3", "=A2+1"),
        ]
    )
    assert sheet.get("A3") == "3"

    # Update A1 → should cascade
    sheet.set_cells([("A1", "10")])
    assert sheet.get("A2") == "11"
    assert sheet.get("A3") == "12"


def test_get_all():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "1"), ("B1", "=A1*2")])
    result = sheet.get_all()
    assert result["A1"] == "1"
    assert result["B1"] == "2"


def test_circular_dependency():
    sheet = lotus.Sheet()
    try:
        sheet.set_cells([("A1", "=B1"), ("B1", "=A1")])
        assert False, "Should have raised ValueError"
    except ValueError as e:
        assert "CIRCULAR" in str(e)


def test_delete_cell():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "42")])
    assert sheet.get("A1") == "42"
    sheet.set_cells([("A1", "")])
    assert sheet.get("A1") is None


def test_evaluate_standalone():
    assert lotus.evaluate("=2+3") == "5"
    assert lotus.evaluate("=2^10") == "1024"
    assert lotus.evaluate("=SUM(1,2,3)") == "6"
    assert lotus.evaluate("hello") == "hello"
    assert lotus.evaluate("") is None


def test_evaluate_div_zero():
    try:
        lotus.evaluate("=1/0")
        assert False, "Should have raised ValueError"
    except ValueError as e:
        assert "DIV/0" in str(e)


def test_string_functions():
    sheet = lotus.Sheet()
    sheet.set_cells(
        [
            ("A1", "hello"),
            ("A2", "=UPPER(A1)"),
            ("A3", "=LEN(A1)"),
            ("A4", '=CONCAT(A1, " world")'),
        ]
    )
    assert sheet.get("A2") == "HELLO"
    assert sheet.get("A3") == "5"
    assert sheet.get("A4") == "hello world"


def test_column_range():
    sheet = lotus.Sheet()
    sheet.set_cells(
        [
            ("A1", "1"),
            ("A2", "2"),
            ("A3", "3"),
            ("B1", "=SUM(A:A)"),
        ]
    )
    assert sheet.get("B1") == "6"


def test_if_function():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "1"), ("A2", '=IF(A1, "yes", "no")')])
    assert sheet.get("A2") == "yes"

    sheet.set_cells([("A1", "0")])
    assert sheet.get("A2") == "no"


def test_rectangular_range():
    sheet = lotus.Sheet()
    sheet.set_cells(
        [
            ("A1", "1"),
            ("B1", "2"),
            ("A2", "3"),
            ("B2", "4"),
            ("C1", "=SUM(A1:B2)"),
        ]
    )
    assert sheet.get("C1") == "10"


def test_parse_range_bounded():
    r = lotus.parse_range("A1:F10")
    assert r["start"] == {"row": 0, "col": 0}
    assert r["end_col"] == 5
    assert r["end_row"] == 9
    assert r["unbounded"] is False
    assert r["normalized"] == "A1:F10"


def test_parse_range_shapes():
    cases = {
        "A1:F10": (0, 0, 5, 9, False),
        "A1:A1": (0, 0, 0, 0, False),
        "A:F": (0, 0, 5, None, True),
        "A1:F": (0, 0, 5, None, True),
        "B5:D": (4, 1, 3, None, True),
    }
    for input_str, (sr, sc, ec, er, unbounded) in cases.items():
        r = lotus.parse_range(input_str)
        assert r["start"] == {"row": sr, "col": sc}, input_str
        assert r["end_col"] == ec, input_str
        assert r["end_row"] == er, input_str
        assert r["unbounded"] is unbounded, input_str


def test_parse_range_case_insensitive():
    lower = lotus.parse_range("a1:f")
    upper = lotus.parse_range("A1:F")
    assert lower == upper
    assert lower["normalized"] == "A1:F"


def test_parse_range_dollar_signs_stripped():
    r = lotus.parse_range("$A$1:$F$10")
    assert r["normalized"] == "A1:F10"
    assert r["start"] == {"row": 0, "col": 0}


def test_parse_range_reversed_normalized():
    r = lotus.parse_range("F10:A1")
    assert r["start"] == {"row": 0, "col": 0}
    assert r["end_col"] == 5
    assert r["end_row"] == 9


def test_parse_range_invalid_raises():
    invalid_inputs = [
        "",
        "A1",
        "A1:",
        ":F10",
        "A1:F10:Z20",
        "1:10",
        "A1:F10 extra junk",
    ]
    for bad in invalid_inputs:
        try:
            lotus.parse_range(bad)
        except ValueError:
            pass
        else:
            raise AssertionError(f"expected ValueError for {bad!r}")


def test_is_unbounded_range():
    assert lotus.is_unbounded_range("A:F") is True
    assert lotus.is_unbounded_range("A1:F") is True
    assert lotus.is_unbounded_range("B5:D") is True
    assert lotus.is_unbounded_range("A1:F10") is False
    assert lotus.is_unbounded_range("garbage") is False
    assert lotus.is_unbounded_range("") is False


def test_parse_cell_id():
    assert lotus.parse_cell_id("A1") == {"row": 0, "col": 0}
    assert lotus.parse_cell_id("AB42") == {"row": 41, "col": 27}
    assert lotus.parse_cell_id("$A$1") == {"row": 0, "col": 0}


def test_parse_cell_id_invalid_raises():
    for bad in ["", "A1:B2", "A", "1", "A1X"]:
        try:
            lotus.parse_cell_id(bad)
        except ValueError:
            pass
        else:
            raise AssertionError(f"expected ValueError for {bad!r}")


def test_index_to_col():
    assert lotus.index_to_col(0) == "A"
    assert lotus.index_to_col(25) == "Z"
    assert lotus.index_to_col(26) == "AA"
    assert lotus.index_to_col(701) == "ZZ"
    assert lotus.index_to_col(702) == "AAA"


def test_col_to_index():
    assert lotus.col_to_index("A") == 0
    assert lotus.col_to_index("Z") == 25
    assert lotus.col_to_index("AA") == 26
    assert lotus.col_to_index("AAA") == 702
    # Lowercase matches parse_cell_id's case-insensitive behavior.
    assert lotus.col_to_index("a") == 0
    assert lotus.col_to_index("aa") == 26


def test_col_to_index_invalid_raises():
    for bad in ["", "A1", " A", "!"]:
        try:
            lotus.col_to_index(bad)
        except ValueError:
            pass
        else:
            raise AssertionError(f"expected ValueError for {bad!r}")


def test_cell_id():
    assert lotus.cell_id(0, 0) == "A1"
    assert lotus.cell_id(11, 1) == "B12"
    assert lotus.cell_id(0, 26) == "AA1"
    assert lotus.cell_id(99, 14) == "O100"


def test_cell_id_round_trips_with_parse_cell_id():
    for row, col in [(0, 0), (0, 26), (11, 1), (99, 14), (41, 27)]:
        assert lotus.parse_cell_id(lotus.cell_id(row, col)) == {
            "row": row,
            "col": col,
        }


def test_extract_refs_includes_whole_column_and_row():
    refs = lotus.extract_refs("=SUM(B:B)")
    assert len(refs) == 1
    assert refs[0]["text"] == "B:B"
    assert refs[0]["kind"] == "whole_column"
    assert refs[0]["cells"] == []

    refs = lotus.extract_refs("=SUM(1:5)")
    assert len(refs) == 1
    assert refs[0]["text"] == "1:5"
    assert refs[0]["kind"] == "whole_row"
    assert refs[0]["cells"] == []


def test_extract_refs_kind_on_cell_and_range():
    refs = lotus.extract_refs("=A1+SUM(B1:C2)")
    assert [r["kind"] for r in refs] == ["cell", "range"]


def test_rewrite_refs_for_deletion_matches_table():
    cases = [
        ("=A1", [], [], "=A1"),
        ("=B1", [1], [], "=#REF!"),
        ("=A1+B1", [1], [], "=A1+#REF!"),
        ("=A1+B2+C3", [1, 2], [], "=A1+#REF!+#REF!"),
        ("=SUM(B:B)", [1], [], "=SUM(#REF!)"),
        ("=SUM(A:B)", [1], [], "=SUM(A:#REF!)"),
        ("=SUM(A:B)", [0, 1], [], "=SUM(#REF!)"),
        ("=SUM(A1:B2)", [1], [], "=SUM(A1:#REF!)"),
        ("=SUM(A1:B2)", [0, 1], [], "=SUM(#REF!)"),
        ("=SUM(1:1)", [], [0], "=SUM(#REF!)"),
        ("=A1:A5", [], [4], "=A1:#REF!"),
        ("=A1:A5", [], [0, 1, 2, 3, 4], "=#REF!"),
        ('=IF(A1, "B1", "B:B")', [1], [], '=IF(A1, "B1", "B:B")'),
        ("hello", [1], [], "hello"),
    ]
    for formula, cols, rows, expected in cases:
        got = lotus.rewrite_refs_for_deletion(
            formula, deleted_cols=cols, deleted_rows=rows
        )
        assert got == expected, f"{formula!r} cols={cols} rows={rows} → {got!r}, want {expected!r}"


def test_rewrite_refs_for_deletion_is_idempotent():
    cases = [
        ("=B1", [1], []),
        ("=SUM(A:B)", [1], []),
        ("=SUM(A1:B2)", [1], []),
        ("=A1:A5", [], [4]),
    ]
    for formula, cols, rows in cases:
        once = lotus.rewrite_refs_for_deletion(
            formula, deleted_cols=cols, deleted_rows=rows
        )
        twice = lotus.rewrite_refs_for_deletion(
            once, deleted_cols=cols, deleted_rows=rows
        )
        assert once == twice, f"not idempotent: {formula!r}"


def test_rewrite_refs_for_deletion_defaults():
    # Both kwargs default to empty — formula passes through.
    assert lotus.rewrite_refs_for_deletion("=A1+B2") == "=A1+B2"
    # Only one kwarg given.
    assert lotus.rewrite_refs_for_deletion("=B1", deleted_cols=[1]) == "=#REF!"


def _adjust(formula, cols=None, rows=None):
    return lotus.adjust_refs_for_deletion(
        formula, deleted_cols=cols or [], deleted_rows=rows or []
    )


def test_adjust_refs_for_deletion_shifts_surviving_cells():
    # D (idx 3) shifts left to C after col B is deleted.
    assert _adjust("=D1", cols=[1]) == "=C1"
    # A (idx 0) sits before the deletion, so it's untouched.
    assert _adjust("=A1", cols=[1]) == "=A1"
    # Deleted cell becomes #REF!.
    assert _adjust("=B1", cols=[1]) == "=#REF!"


def test_adjust_refs_for_deletion_trims_a1_ranges():
    # Inner column deleted → surviving C shifts to B.
    assert _adjust("=A1:C1", cols=[1]) == "=A1:B1"
    # First or last col deleted → range shrinks to surviving extent.
    assert _adjust("=A1:D1", cols=[3]) == "=A1:C1"
    assert _adjust("=A1:D1", cols=[0]) == "=A1:C1"
    # Every col in the range deleted → #REF!.
    assert _adjust("=B1:C1", cols=[1, 2]) == "=#REF!"


def test_adjust_refs_for_deletion_trims_whole_col_and_row_ranges():
    assert _adjust("=SUM(A:C)", cols=[1]) == "=SUM(A:B)"
    assert _adjust("=SUM(B:B)", cols=[1]) == "=SUM(#REF!)"
    assert _adjust("=SUM(1:3)", rows=[1]) == "=SUM(1:2)"
    assert _adjust("=SUM(2:4)", rows=[0]) == "=SUM(1:3)"


def test_adjust_refs_for_deletion_preserves_strings_and_non_formulas():
    assert _adjust('=IF(A1, "B1", "B:B")', cols=[1]) == '=IF(A1, "B1", "B:B")'
    assert _adjust("hello", cols=[1]) == "hello"


def test_adjust_refs_for_deletion_two_sequential_deletions():
    # Applying the same deletion twice means two deletions, not a no-op.
    once = _adjust("=C1", cols=[1])
    assert once == "=B1"
    assert _adjust(once, cols=[1]) == "=#REF!"


def _insert(formula, cols=None, rows=None):
    return lotus.adjust_refs_for_insertion(
        formula, inserted_cols=cols or [], inserted_rows=rows or []
    )


def test_adjust_refs_for_insertion_shifts_cells_at_or_past_insertion():
    # A1 (col 0) + insert col 0 → B1.
    assert _insert("=A1", cols=[0]) == "=B1"
    # A1 + insert col 1 → unchanged.
    assert _insert("=A1", cols=[1]) == "=A1"
    # B2 + insert col 1 → C2.
    assert _insert("=B2", cols=[1]) == "=C2"
    # Duplicate indices → shift by count.
    assert _insert("=B2", cols=[1, 1]) == "=D2"


def test_adjust_refs_for_insertion_preserves_absolute_markers():
    assert _insert("=$B$2", cols=[1]) == "=$C$2"


def test_adjust_refs_for_insertion_grows_ranges_straddling_insertion():
    assert _insert("=A1:C3", cols=[1]) == "=A1:D3"
    assert _insert("=A1:C3", cols=[5]) == "=A1:C3"
    assert _insert("=A1:C3", cols=[0]) == "=B1:D3"


def test_adjust_refs_for_insertion_whole_col_and_row_ranges():
    assert _insert("=SUM(A:A)", cols=[0]) == "=SUM(B:B)"
    assert _insert("=SUM(A:C)", cols=[1]) == "=SUM(A:D)"
    assert _insert("=SUM(1:1)", rows=[0]) == "=SUM(2:2)"
    assert _insert("=SUM(1:3)", rows=[1]) == "=SUM(1:4)"


def test_adjust_refs_for_insertion_passes_through_non_formula():
    assert _insert("hello", cols=[0]) == "hello"


def test_adjust_refs_for_insertion_empty_kwargs_are_noop():
    assert _insert("=A1") == "=A1"
    assert lotus.adjust_refs_for_insertion("=A1") == "=A1"


def test_adjust_refs_for_insertion_two_sequential_insertions():
    once = _insert("=A1", cols=[0])
    assert once == "=B1"
    assert _insert(once, cols=[0]) == "=C1"


# ── named ranges ────────────────────────────────────────────────────


def test_named_constant_in_formula():
    sheet = lotus.Sheet()
    sheet.set_name("TaxRate", "0.05")
    sheet.set_cells([("A1", "100"), ("B1", "=A1*TaxRate")])
    assert sheet.get("B1") == "5"


def test_named_range_in_sum():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "1"), ("A2", "2"), ("A3", "3")])
    sheet.set_name("Revenue", "=A1:A3")
    sheet.set_cells([("B1", "=SUM(Revenue)")])
    assert sheet.get("B1") == "6"


def test_changing_definition_propagates():
    sheet = lotus.Sheet()
    sheet.set_name("Rate", "0.05")
    sheet.set_cells([("A1", "100"), ("B1", "=A1*Rate")])
    assert sheet.get("B1") == "5"
    sheet.set_name("Rate", "0.10")
    assert sheet.get("B1") == "10"


def test_remove_name_yields_name_error():
    sheet = lotus.Sheet()
    sheet.set_name("Rate", "0.05")
    sheet.set_cells([("A1", "100"), ("B1", "=A1*Rate")])
    sheet.remove_name("Rate")
    assert sheet.get("B1") == "#NAME?"


def test_get_name_and_names_dict():
    sheet = lotus.Sheet()
    sheet.set_name("Foo", "=A1")
    sheet.set_name("Bar", "42")
    # Names are uppercased internally.
    assert sheet.get_name("foo") == "=A1"
    assert sheet.get_name("FOO") == "=A1"
    assert sheet.get_name("missing") is None
    assert sheet.names() == {"FOO": "=A1", "BAR": "42"}


def test_set_name_validation_errors():
    sheet = lotus.Sheet()
    for bad in ["A1", "BC42", "SUM", "if", "1Foo", "Foo Bar", ""]:
        try:
            sheet.set_name(bad, "0")
        except ValueError:
            pass
        else:
            raise AssertionError(f"expected ValueError for {bad!r}")


def test_unparseable_definition_raises():
    sheet = lotus.Sheet()
    try:
        sheet.set_name("Foo", "=1 + +")
    except ValueError:
        pass
    else:
        raise AssertionError("expected ValueError for malformed definition")


def test_extract_refs_includes_name_kind():
    refs = lotus.extract_refs("=TaxRate+A1")
    kinds = [r["kind"] for r in refs]
    assert kinds == ["name", "cell"]
    # Lexer uppercases identifiers — surface that to callers.
    assert refs[0]["text"] == "TAXRATE"
    assert refs[0]["cells"] == []


# ── completion / signature help ─────────────────────────────────────


def test_list_functions_includes_every_builtin():
    names = {f["name"] for f in lotus.list_functions()}
    for expected in ["SUM", "AVERAGE", "MIN", "MAX", "COUNT", "ABS", "ROUND",
                     "IF", "CONCAT", "LEN", "UPPER", "LOWER"]:
        assert expected in names, f"missing {expected}"


def test_list_functions_metadata_shape():
    funcs = {f["name"]: f for f in lotus.list_functions()}
    sum_def = funcs["SUM"]
    assert sum_def["category"] == "Math"
    assert sum_def["aliases"] == []
    assert sum_def["params"] == []
    assert sum_def["variadic"]["name"] == "value"
    assert "Returns the sum" in sum_def["description"]
    assert any(ex["formula"] == "=SUM(1, 2, 3)" for ex in sum_def["examples"])

    avg = funcs["AVERAGE"]
    assert avg["aliases"] == ["MEAN", "AVG"]

    rnd = funcs["ROUND"]
    assert len(rnd["params"]) == 2
    assert rnd["params"][0]["optional"] is False
    assert rnd["params"][1]["optional"] is True
    assert rnd["variadic"] is None


def test_complete_prefix_filters_functions():
    sheet = lotus.Sheet()
    result = sheet.complete("=SU", 3)
    labels = [i["label"] for i in result["items"]]
    assert "SUM" in labels
    assert "AVERAGE" not in labels
    assert result["replace_start"] == 1
    assert result["replace_end"] == 3


def test_complete_includes_named_ranges():
    sheet = lotus.Sheet()
    sheet.set_name("Revenue", "=A1:A10")
    sheet.set_name("Rate", "0.05")
    result = sheet.complete("=R", 2)
    by_label = {i["label"]: i for i in result["items"]}
    assert "REVENUE" in by_label
    assert by_label["REVENUE"]["kind"] == "name"
    assert by_label["REVENUE"]["insert"] == "REVENUE"
    assert "ROUND" in by_label
    assert by_label["ROUND"]["kind"] == "function"
    assert by_label["ROUND"]["insert"] == "ROUND("


def test_complete_function_item_includes_documentation():
    sheet = lotus.Sheet()
    result = sheet.complete("=SU", 3)
    sum_item = next(i for i in result["items"] if i["label"] == "SUM")
    assert sum_item["detail"] == "SUM(value, ...)"
    assert "Returns the sum" in sum_item["documentation"]
    assert "Examples:" in sum_item["documentation"]


def test_complete_no_completions_for_non_formula():
    sheet = lotus.Sheet()
    assert sheet.complete("hello", 5)["items"] == []


def test_complete_inside_unterminated_string_returns_empty():
    sheet = lotus.Sheet()
    assert sheet.complete('="hel', 5)["items"] == []


def test_signature_help_first_param():
    sig = lotus.signature_help("=SUM(", 5)
    assert sig is not None
    assert sig["function"]["name"] == "SUM"
    assert sig["active_param"] == 0


def test_signature_help_second_param_after_comma():
    sig = lotus.signature_help("=ROUND(3.14, ", 13)
    assert sig["function"]["name"] == "ROUND"
    assert sig["active_param"] == 1
    assert sig["function"]["params"][1]["name"] == "decimals"


def test_signature_help_nested_call_picks_innermost():
    sig = lotus.signature_help("=SUM(1, MAX(", 12)
    assert sig["function"]["name"] == "MAX"
    assert sig["active_param"] == 0


def test_signature_help_alias_resolves_to_primary():
    sig = lotus.signature_help("=AVG(1, ", 8)
    assert sig["function"]["name"] == "AVERAGE"


def test_signature_help_outside_call_returns_none():
    assert lotus.signature_help("=SUM(1, 2)", 10) is None
    assert lotus.signature_help("hello", 3) is None
    assert lotus.signature_help("=FOOBAR(", 8) is None


# ── instance-level list_functions / signature_help (sees runtime registrations)


def test_sheet_list_functions_includes_every_builtin():
    sheet = lotus.Sheet()
    names = {f["name"] for f in sheet.list_functions()}
    for expected in ["SUM", "AVERAGE", "MIN", "MAX", "COUNT", "ABS", "ROUND",
                     "IF", "CONCAT", "LEN", "UPPER", "LOWER"]:
        assert expected in names, f"missing {expected}"


def test_sheet_list_functions_includes_host_registered_function():
    sheet = lotus.Sheet()
    baseline = len(sheet.list_functions())
    sheet.register_function("MY_FN", lambda args: 42)
    after = sheet.list_functions()
    assert len(after) == baseline + 1
    by_name = {f["name"]: f for f in after}
    assert "MY_FN" in by_name
    # Custom functions report a minimal shape: name + open-ended variadic.
    info = by_name["MY_FN"]
    assert info["aliases"] == []
    assert info["params"] == []
    assert info["variadic"] is not None
    assert info["category"] == "Custom"


def test_sheet_signature_help_resolves_host_registered_function():
    sheet = lotus.Sheet()
    sheet.register_function("MY_FN", lambda args: 0)
    sig = sheet.signature_help("=MY_FN(1, ", 10)
    assert sig is not None
    assert sig["function"]["name"] == "MY_FN"
    assert sig["active_param"] == 1


def test_sheet_signature_help_falls_back_to_builtin():
    sheet = lotus.Sheet()
    sig = sheet.signature_help("=SUM(", 5)
    assert sig["function"]["name"] == "SUM"


def test_sheet_signature_help_unknown_returns_none():
    sheet = lotus.Sheet()
    assert sheet.signature_help("=NOPE(", 6) is None


# ── string concatenation with & ─────────────────────────────────────


def test_concat_operator():
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "world"),
        ("B1", '="hello " & A1'),
        ("B2", '="count: " & 42'),
    ])
    assert sheet.get("B1") == "hello world"
    assert sheet.get("B2") == "count: 42"


def test_concat_standalone():
    assert lotus.evaluate('="a" & "b" & "c"') == "abc"
    # Lower precedence than +/-: "a" & (1+2) == "a3"
    assert lotus.evaluate('="a" & 1 + 2') == "a3"


# ── array formulas / spill ─────────────────────────────────────────


def test_sequence_spills_across_cells():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "=SEQUENCE(5)")])
    # Get reads the computed value at each cell — spill-owned cells included.
    assert sheet.get("A1") == "1"
    assert sheet.get("A2") == "2"
    assert sheet.get("A5") == "5"
    assert sheet.spill_at("A1") == {"rows": 5, "cols": 1}
    assert sheet.owner_of("A3") == "A1"
    assert sheet.owner_of("A6") is None


def test_2d_sequence_spill():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "=SEQUENCE(2, 3)")])
    assert sheet.get("A1") == "1"
    assert sheet.get("C2") == "6"
    info = sheet.spill_at("A1")
    assert info == {"rows": 2, "cols": 3}


def test_spill_blocked_emits_error():
    sheet = lotus.Sheet()
    sheet.set_cells([("A3", "99")])
    sheet.set_cells([("A1", "=SEQUENCE(5)")])
    assert sheet.get("A1") == "#SPILL!"
    assert sheet.get("A3") == "99"
    assert sheet.spill_at("A1") is None


def test_spill_shrinks_on_update():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "=SEQUENCE(5)")])
    sheet.set_cells([("A1", "=SEQUENCE(3)")])
    assert sheet.get("A3") == "3"
    assert sheet.get("A4") is None
    assert sheet.get("A5") is None
    assert sheet.spill_at("A1") == {"rows": 3, "cols": 1}


def test_broadcasting_spills():
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "1"), ("A2", "2"), ("A3", "3"),
        ("B1", "=A1:A3 * 2"),
    ])
    assert sheet.get("B1") == "2"
    assert sheet.get("B2") == "4"
    assert sheet.get("B3") == "6"


def test_spill_operator():
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "=SEQUENCE(5)"),
        ("B1", "=SUM(A1#)"),
    ])
    assert sheet.get("B1") == "15"


def test_spill_operator_feeds_broadcast():
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "=SEQUENCE(3)"),
        ("B1", "=A1# * 10"),
    ])
    assert sheet.get("B1") == "10"
    assert sheet.get("B2") == "20"
    assert sheet.get("B3") == "30"


def test_array_literal_spill():
    sheet = lotus.Sheet()
    sheet.set_cells([("E1", "={10, 20; 30, 40}")])
    assert sheet.get("E1") == "10"
    assert sheet.get("F1") == "20"
    assert sheet.get("E2") == "30"
    assert sheet.get("F2") == "40"


def test_filter_spills():
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "1"), ("A2", "2"), ("A3", "3"),
        ("A4", "4"), ("A5", "5"),
        ("B1", "0"), ("B2", "0"), ("B3", "1"),
        ("B4", "1"), ("B5", "1"),
        ("D1", "=FILTER(A1:A5, B1:B5)"),
    ])
    assert sheet.get("D1") == "3"
    assert sheet.get("D2") == "4"
    assert sheet.get("D3") == "5"


def test_get_array_returns_2d_nested_list():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "=SEQUENCE(2, 3)")])
    arr = sheet.get_array("A1")
    assert arr == [["1", "2", "3"], ["4", "5", "6"]]


def test_get_array_returns_none_for_non_anchor():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "42")])
    assert sheet.get_array("A1") is None


def test_extract_refs_includes_spill_kind():
    refs = lotus.extract_refs("=SUM(A1#)")
    kinds = [r["kind"] for r in refs]
    assert "spill" in kinds


# ── comparison operators ────────────────────────────────────────────


def test_comparison_operators():
    assert lotus.evaluate("=1 = 1") == "TRUE"
    assert lotus.evaluate("=1 = 2") == "FALSE"
    assert lotus.evaluate("=1 <> 2") == "TRUE"
    assert lotus.evaluate("=3 > 2") == "TRUE"
    assert lotus.evaluate("=3 >= 3") == "TRUE"
    assert lotus.evaluate("=2 < 1") == "FALSE"
    assert lotus.evaluate("=2 <= 2") == "TRUE"


def test_comparison_feeds_filter_natural_form():
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "=SEQUENCE(5)"),
        ("C1", "=FILTER(A1#, A1# > 2)"),
    ])
    assert sheet.get("C1") == "3"
    assert sheet.get("C2") == "4"
    assert sheet.get("C3") == "5"


def test_comparison_with_if_over_array():
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "1"), ("A2", "-5"), ("A3", "3"),
        ("B1", '=IF(A1:A3 >= 0, "non-negative", "negative")'),
    ])
    assert sheet.get("B1") == "non-negative"
    assert sheet.get("B2") == "negative"
    assert sheet.get("B3") == "non-negative"


def test_countif_pattern_via_sum_of_comparison():
    # =SUM(A1:A5 > 2) counts values greater than 2.
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "1"), ("A2", "3"), ("A3", "5"), ("A4", "2"), ("A5", "4"),
        ("B1", "=SUM(A1:A5 > 2)"),
    ])
    assert sheet.get("B1") == "3"


def test_array_formulas_demo():
    """End-to-end demo from ARRAY_FORMULAS_PLAN.md §11."""
    sheet = lotus.Sheet()

    # A1: =SEQUENCE(5)              → A1..A5 = 1..5
    sheet.set_cells([("A1", "=SEQUENCE(5)")])
    assert [sheet.get(f"A{i}") for i in range(1, 6)] == ["1", "2", "3", "4", "5"]

    # B1: =A1#*2                    → B1..B5 = 2..10
    sheet.set_cells([("B1", "=A1# * 2")])
    assert [sheet.get(f"B{i}") for i in range(1, 6)] == ["2", "4", "6", "8", "10"]

    # C1: =SUM(B1#)                 → 30
    sheet.set_cells([("C1", "=SUM(B1#)")])
    assert sheet.get("C1") == "30"

    # D1: =FILTER(A1#, B1:B5)       → D1..D(kept) = values where matching B is truthy
    # Every B is truthy here (>0), so D ends up = A1..A5.
    # Demo uses FILTER on a comparison — we simulate with pre-built boolean column.
    # Use B > 4 as truthy (B1..B5 = 2,4,6,8,10 → truthy where >4, i.e. 6,8,10).
    # To avoid comparison operators (not implemented), use A values >2 via IF.
    # Simpler demo: FILTER(A1#, A1#) drops zeros — here all truthy, keeps everything.
    sheet.set_cells([("D1", "=FILTER(A1#, A1#)")])
    assert [sheet.get(f"D{i}") for i in range(1, 6)] == ["1", "2", "3", "4", "5"]

    # E1: ={10,20;30,40}            → E1=10, F1=20, E2=30, F2=40
    sheet.set_cells([("E1", "={10, 20; 30, 40}")])
    assert sheet.get("E1") == "10"
    assert sheet.get("F1") == "20"
    assert sheet.get("E2") == "30"
    assert sheet.get("F2") == "40"

    # A6: 99  # doesn't block A1's spill (outside of range).
    sheet.set_cells([("A6", "99")])
    assert sheet.get("A1") == "1"
    assert sheet.spill_at("A1") == {"rows": 5, "cols": 1}

    # Now blocker: author A4=99. A1 → #SPILL!
    sheet.set_cells([("A4", "99")])
    assert sheet.get("A1") == "#SPILL!"
    assert sheet.spill_at("A1") is None
    # A4 keeps its user-authored value.
    assert sheet.get("A4") == "99"


# ── $ absolute markers in formulas ──────────────────────────────────


def test_evaluate_absolute_cell_refs_matches_relative():
    # The bug this closes: previously `=$A$1*2` would throw
    # "Unexpected character: $". After the lexer change it evaluates
    # identically to `=A1*2` (but A1 isn't populated here, so both
    # forms act on "missing" → the point is neither errors).
    assert lotus.evaluate("=$A$1+1") == lotus.evaluate("=A1+1")
    assert lotus.evaluate("=$A1*2") == lotus.evaluate("=A1*2")
    assert lotus.evaluate("=A$1*2") == lotus.evaluate("=A1*2")


def test_sheet_absolute_refs_resolve_correctly():
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "10"),
        ("A2", "10"),
        ("B1", "3"),
        ("C1", "=$A$1*B1"),
        ("C2", "=$A2*B$1*4"),
    ])
    assert sheet.get("C1") == "30"
    # $A2=10, B$1=3, 10*3*4=120.
    assert sheet.get("C2") == "120"


def test_sheet_absolute_range_sum():
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "1"), ("A2", "2"), ("A3", "3"),
        ("B1", "=SUM($A$1:$A$3)"),
    ])
    assert sheet.get("B1") == "6"


def test_sheet_absolute_spill_ref():
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "=SEQUENCE(3)"),
        ("B1", "=SUM($A$1#)"),
    ])
    assert sheet.get("B1") == "6"


def test_extract_refs_preserves_dollar_in_text():
    refs = lotus.extract_refs("=$A$1*2")
    assert len(refs) == 1
    assert refs[0]["text"] == "$A$1"
    # cells field is canonical so callers can match sheet IDs.
    assert refs[0]["cells"] == ["A1"]


def test_absolute_refs_roundtrip_through_shift():
    # Motivating end-to-end case from the TODO:
    # eval should succeed and shift should move only the relative axes.
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "10"),
        ("B1", "2"),
        ("B2", "=$A1*B$1"),
    ])
    assert sheet.get("B2") == "20"
    shifted = lotus.shift_formula_refs("=$A1*B$1", 1, 1, 1000, 26)
    assert shifted == "=$A2*C$1"


# ── shift_formula_refs ──────────────────────────────────────────────


def _shift(formula, d_row, d_col, max_row=1000, max_col=26):
    return lotus.shift_formula_refs(formula, d_row, d_col, max_row, max_col)


def test_shift_relative_cell_refs():
    assert _shift("=A2*B1*4", 1, 0) == "=A3*B2*4"
    assert _shift("=A2*B1*4", 0, 1) == "=B2*C1*4"
    assert _shift("=A2*B1*4", -1, 0) == "=A1*#REF!*4"


def test_shift_preserves_absolute_markers():
    assert _shift("=$A$2*B1", 1, 1) == "=$A$2*C2"
    assert _shift("=$A2*A$1", 1, 1) == "=$A3*B$1"


def test_shift_ranges():
    assert _shift("=A1:B5", 2, 0) == "=A3:B7"
    # B5 col+25=27 > max_col=26 → full collapse.
    assert _shift("=A1:B5", 0, 25) == "=#REF!"
    assert _shift("=$A$1:B5", 1, 0) == "=$A$1:B6"


def test_shift_whole_col_and_row_ranges():
    assert _shift("=A:A", 5, 1) == "=B:B"
    assert _shift("=A:A", 0, 26) == "=#REF!"
    assert _shift("=1:1", 3, 5) == "=4:4"


def test_shift_spill_ref():
    assert _shift("=A1#", 2, 1) == "=B3#"
    assert _shift("=$A$1#", 2, 1) == "=$A$1#"


def test_shift_mixed_function_call():
    assert _shift("=SUM(A1, $B$2, C:C)", 1, 1) == "=SUM(B2, $B$2, D:D)"


def test_shift_preserves_named_range_and_non_formulas():
    assert _shift("=my_range + 1", 5, 5) == "=my_range + 1"
    assert _shift("=SEQUENCE(5)", 1, 1) == "=SEQUENCE(5)"
    assert _shift("123", 1, 0) == "123"
    assert _shift("hello", 1, 0) == "hello"
    assert _shift("", 1, 0) == ""


def test_shift_zero_delta_is_identity():
    assert _shift("=A1", 0, 0) == "=A1"


# ── pin_value (host-injected spills) ────────────────────────────────


def test_pin_value_spills_like_native_formula():
    sheet = lotus.Sheet()
    sheet.pin_value("A1", [["10"], ["20"], ["30"]])
    assert sheet.get("A1") == "10"
    assert sheet.get("A2") == "20"
    assert sheet.get("A3") == "30"
    assert sheet.spill_at("A1") == {"rows": 3, "cols": 1}
    assert sheet.is_pinned("A1") is True


def test_pin_value_scalar_no_overflow():
    sheet = lotus.Sheet()
    sheet.pin_value("A1", [["only"]])
    assert sheet.get("A1") == "only"
    assert sheet.spill_at("A1") is None


def test_pin_feeds_spill_ref_consumers():
    sheet = lotus.Sheet()
    sheet.pin_value("A1", [["1"], ["2"], ["3"], ["4"]])
    sheet.set_cells([("B1", "=SUM(A1#)")])
    assert sheet.get("B1") == "10"


def test_pin_overrides_existing_formula():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "=SEQUENCE(5)")])
    sheet.pin_value("A1", [["x"], ["y"]])
    assert sheet.get("A1") == "x"
    assert sheet.get("A2") == "y"
    assert sheet.get("A3") is None


def test_pin_blocked_yields_spill_error():
    sheet = lotus.Sheet()
    sheet.set_cells([("A2", "blocker")])
    sheet.pin_value("A1", [["1"], ["2"], ["3"]])
    assert sheet.get("A1") == "#SPILL!"
    assert sheet.get("A2") == "blocker"


def test_unpin_reverts_to_formula():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "=SEQUENCE(3)")])
    sheet.pin_value("A1", [["frozen"]])
    assert sheet.get("A1") == "frozen"
    sheet.unpin_value("A1")
    assert sheet.get("A1") == "1"
    assert sheet.get("A2") == "2"
    assert sheet.get("A3") == "3"


def test_pinned_cells_sorted():
    sheet = lotus.Sheet()
    sheet.pin_value("C5", [["a", "b"]])
    sheet.pin_value("A1", [["1"]])
    assert sheet.pinned_cells() == ["A1", "C5"]


def test_pin_value_invalid_shape_raises():
    sheet = lotus.Sheet()
    for bad in [[], [[]], [["a", "b"], ["c"]]]:
        try:
            sheet.pin_value("A1", bad)
        except ValueError:
            pass
        else:
            raise AssertionError(f"expected ValueError for {bad!r}")


def test_pin_loading_then_resolved_simulation():
    """Simulates the =SQL(...) async flow: pin #LOADING! then replace with data."""
    sheet = lotus.Sheet()
    sheet.set_cells([("D1", "=A1")])
    sheet.pin_value("A1", [["#LOADING!"]])
    assert sheet.get("D1") == "#LOADING!"
    sheet.pin_value("A1", [["id", "name"], ["1", "alex"], ["2", "bob"]])
    assert sheet.get("A1") == "id"
    assert sheet.get("D1") == "id"  # still points at A1's top-left
    assert sheet.get("B2") == "alex"


# ── typed accessors ────────────────────────────────────────────────


def test_get_typed_matches_table():
    sheet = lotus.Sheet()
    sheet.set_cells([
        ("A1", "2"),
        ("A2", "3.14"),
        ("A3", "007"),
        ("A4", "hello"),
        ("A5", "=1+4"),
        ("A6", "1e308"),
        ("A7", "NaN"),
        ("A8", "Infinity"),
    ])

    val = sheet.get_typed("A1")
    assert val == 2 and type(val) is int

    val = sheet.get_typed("A2")
    assert val == 3.14 and type(val) is float

    # "007" is parsed by the engine as a number on input (leading zeros
    # are lost there — not recoverable at the binding layer). The point
    # pinned here: the typed accessor doesn't invent a string to match
    # the original — it returns what the engine stored.
    val = sheet.get_typed("A3")
    assert val == 7 and type(val) is int

    val = sheet.get_typed("A4")
    assert val == "hello" and type(val) is str

    # Formula result.
    val = sheet.get_typed("A5")
    assert val == 5 and type(val) is int

    val = sheet.get_typed("A6")
    assert val == 1e308 and type(val) is float

    # NaN/Infinity never become CellValue::Number — they stay strings.
    assert sheet.get_typed("A7") == "NaN"
    assert sheet.get_typed("A8") == "Infinity"

    # Empty cell.
    assert sheet.get_typed("Z99") is None


def test_get_typed_error_values_are_strings():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "=1/0")])
    val = sheet.get_typed("A1")
    assert val == "#DIV/0!" and type(val) is str


def test_get_typed_large_integer_exceeds_2_to_53():
    # 2^60 is integral but past the JS-safe-integer boundary, so we
    # fall back to float. Python int is exact but we'd lose precision
    # going through JS — the rule keeps both bindings in sync.
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "=2^60")])
    val = sheet.get_typed("A1")
    assert val == 2.0**60 and type(val) is float


def test_get_all_typed_omits_empty_and_preserves_types():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "1"), ("A2", "=A1*2"), ("B1", "hi"), ("B2", "")])
    all_typed = sheet.get_all_typed()
    assert all_typed == {"A1": 1, "A2": 2, "B1": "hi"}
    assert type(all_typed["A1"]) is int
    assert type(all_typed["A2"]) is int
    assert type(all_typed["B1"]) is str


def test_get_array_typed_mixes_ints_and_floats():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "=SEQUENCE(2, 3)")])
    arr = sheet.get_array_typed("A1")
    assert arr == [[1, 2, 3], [4, 5, 6]]
    assert all(type(v) is int for row in arr for v in row)


def test_get_array_typed_returns_none_for_non_anchor():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "42")])
    assert sheet.get_array_typed("A1") is None


# ── custom type handlers + custom functions via Python trampoline ───


class _UpperHandler:
    """Dummy handler: claims `!u:...` literals as upper-cased customs."""

    type_tag = "upper"

    def parse_literal(self, raw):
        if raw.startswith("!u:"):
            return {"data": raw[3:].upper()}
        return None

    def parse_with(self, raw, options):
        # Probe API: `options == "force"` claims any input. Empty options
        # delegates to parse_literal, matching the engine's default.
        if options == "force":
            return {"data": raw.upper()}
        return self.parse_literal(raw)

    def display(self, v):
        return f"<{v['data']}>"

    def binary_op(self, op, lhs, rhs):
        if op != "+":
            return None
        if not (isinstance(lhs, dict) and isinstance(rhs, dict)):
            return None
        return {"type_tag": "upper", "data": lhs["data"] + rhs["data"]}

    def compare(self, op, lhs, rhs):
        if not (isinstance(lhs, dict) and isinstance(rhs, dict)):
            return None
        if op == "=":
            return lhs["data"] == rhs["data"]
        if op == "<>":
            return lhs["data"] != rhs["data"]
        return None  # ordering ops not supported for this dummy type


def test_custom_handler_parse_literal_and_display():
    sheet = lotus.Sheet()
    sheet.register_type(_UpperHandler())
    sheet.set_cells([("A1", "!u:hello"), ("A2", "regular"), ("A3", '="pre-" & A1')])
    assert sheet.get_typed("A1") == {"type_tag": "upper", "data": "HELLO"}
    assert sheet.get_typed("A2") == "regular"
    # CONCAT via `&` uses registry.display() → wraps in <…>.
    assert sheet.get_typed("A3") == "pre-<HELLO>"


def test_custom_handler_binary_op():
    sheet = lotus.Sheet()
    sheet.register_type(_UpperHandler())
    sheet.set_cells([("A1", "!u:foo"), ("A2", "!u:bar"), ("A3", "=A1+A2")])
    assert sheet.get_typed("A3") == {"type_tag": "upper", "data": "FOOBAR"}


def test_custom_handler_compare():
    sheet = lotus.Sheet()
    sheet.register_type(_UpperHandler())
    sheet.set_cells(
        [
            ("A1", "!u:same"),
            ("A2", "!u:SAME"),  # parse_literal upper-cases both → equal
            ("A3", "!u:diff"),
            ("B1", "=A1=A2"),
            ("B2", "=A1=A3"),
            ("B3", "=A1<>A3"),
        ]
    )
    assert sheet.get_typed("B1") == 1
    assert sheet.get_typed("B2") == 0
    assert sheet.get_typed("B3") == 1


def test_custom_function_pdf_pages():
    """Simulates the M4 'files' use case: a PDF-handler + PAGES function."""
    sheet = lotus.Sheet()

    class PdfHandler:
        type_tag = "pdf"

        def parse_literal(self, raw):
            if raw.endswith(".pdf"):
                return {"data": raw}
            return None

    sheet.register_type(PdfHandler())

    # Mock: "PAGES" just returns the string length as a stand-in for
    # a real PDF page count — we only care that the dispatch path works.
    def pages(args):
        v = args[0]
        data = v["data"] if isinstance(v, dict) else str(v)
        return len(data)

    sheet.register_function("PAGES", pages)

    sheet.set_cells([("A1", "report.pdf"), ("A2", "=PAGES(A1)")])
    assert sheet.get_typed("A1") == {"type_tag": "pdf", "data": "report.pdf"}
    assert sheet.get_typed("A2") == len("report.pdf")


def test_custom_function_raises_becomes_cell_error():
    sheet = lotus.Sheet()

    def boom(args):
        raise ValueError("#BOOM!")

    sheet.register_function("BOOM", boom)
    sheet.set_cells([("A1", "=BOOM()")])
    out = sheet.get("A1")
    assert out is not None and "#BOOM" in out


def test_register_collisions_rejected():
    sheet = lotus.Sheet()
    sheet.register_type(_UpperHandler())

    # Duplicate tag:
    try:
        sheet.register_type(_UpperHandler())
        raise AssertionError("expected duplicate-tag to raise")
    except ValueError:
        pass

    # Builtin name collision:
    try:
        sheet.register_function("SUM", lambda args: 0)
        raise AssertionError("expected builtin collision to raise")
    except ValueError:
        pass


def test_try_parse_probes_handler_with_options():
    sheet = lotus.Sheet()
    sheet.register_type(_UpperHandler())

    # Empty options ⇒ same gate as parse_literal (needs `!u:` prefix).
    assert sheet.try_parse("upper", "hello", "") is None
    assert sheet.try_parse("upper", "!u:hi", "") == {
        "type_tag": "upper",
        "data": "HI",
    }

    # `force` option claims any input.
    assert sheet.try_parse("upper", "hello", "force") == {
        "type_tag": "upper",
        "data": "HELLO",
    }


def test_try_parse_unknown_type_tag_returns_none():
    sheet = lotus.Sheet()
    assert sheet.try_parse("nope", "anything", "") is None


def test_set_cells_typed_kind_string_forces_literal_text():
    # "42" auto-classifies as a number; the typed string kind keeps it.
    sheet = lotus.Sheet()
    sheet.set_cells_typed([("A1", {"kind": "string", "value": "42"})])
    assert sheet.get_typed("A1") == "42"
    sheet.set_cells([("B1", '=A1 & " end"')])
    assert sheet.get_typed("B1") == "42 end"


def test_set_cells_typed_kind_raw_matches_set_cells():
    sheet = lotus.Sheet()
    sheet.set_cells_typed(
        [
            ("A1", {"kind": "raw", "value": "10"}),
            ("B1", {"kind": "raw", "value": "=A1*2"}),
        ]
    )
    assert sheet.get_typed("A1") == 10
    assert sheet.get_typed("B1") == 20


def test_set_cells_typed_kind_empty_deletes():
    sheet = lotus.Sheet()
    sheet.set_cells([("A1", "42")])
    sheet.set_cells_typed([("A1", {"kind": "empty"})])
    assert sheet.get("A1") is None


def test_set_cells_typed_kind_custom_round_trips():
    sheet = lotus.Sheet()
    sheet.register_type(_UpperHandler())
    sheet.set_cells_typed(
        [
            (
                "A1",
                {"kind": "custom", "type_tag": "upper", "data": "HELLO"},
            )
        ]
    )
    assert sheet.get_typed("A1") == {"type_tag": "upper", "data": "HELLO"}


def test_set_cells_typed_raw_write_clears_prior_typed_override():
    sheet = lotus.Sheet()
    sheet.set_cells_typed([("A1", {"kind": "string", "value": "42"})])
    assert sheet.get_typed("A1") == "42"

    # Raw write drops the override → engine re-parses "42" as int.
    sheet.set_cells([("A1", "42")])
    assert sheet.get_typed("A1") == 42


def test_set_cells_typed_rejects_unknown_kind():
    sheet = lotus.Sheet()
    try:
        sheet.set_cells_typed([("A1", {"kind": "zonked"})])
        raise AssertionError("expected ValueError for unknown kind")
    except ValueError as e:
        assert "zonked" in str(e)


def test_try_parse_then_set_cells_typed_jdate_via_strptime():
    # End-to-end flow this feature was designed for: probe a handler with
    # a strftime pattern, then write the parsed value into a cell.
    sheet = lotus.Sheet()
    try:
        sheet.register_datetime()
    except AttributeError:
        # `register_datetime` is gated on the `datetime` cargo feature.
        # If the wheel was built without it, skip the test.
        return

    parsed = sheet.try_parse("jdate", "4/2/2026", "%d/%m/%Y")
    assert parsed == {"type_tag": "jdate", "data": "2026-02-04"}

    sheet.set_cells_typed([("A1", {"kind": "custom", **parsed})])
    assert sheet.get_typed("A1") == {"type_tag": "jdate", "data": "2026-02-04"}


if __name__ == "__main__":
    import sys

    tests = [v for k, v in globals().items() if k.startswith("test_") and callable(v)]
    failed = 0
    for test in tests:
        try:
            test()
            print(f"  PASS  {test.__name__}")
        except Exception as e:
            print(f"  FAIL  {test.__name__}: {e}")
            failed += 1
    print(f"\n{len(tests)} tests, {failed} failed")
    sys.exit(1 if failed else 0)
