# lotus-pyo3 — Python Bindings

PyO3/maturin wrapper around `lotus-core`. Provides the `lotus` Python module used by the Datasette backend for server-side formula evaluation.

## Python API

```python
import lotus

# Full sheet with dependency tracking
sheet = lotus.Sheet()
sheet.set_cells([("A1", "10"), ("A2", "=A1*2")])
sheet.get("A2")       # "20"
sheet.get_all()       # {"A1": "10", "A2": "20"}

# One-shot evaluation (no cell context)
lotus.evaluate("=SUM(1,2,3)")  # "6"

# Reference extraction with source positions
lotus.extract_refs("=A1+B2")
# [{"start": 1, "end": 3, "text": "A1", "cells": ["A1"]}, ...]
```

## Build

```bash
maturin develop           # Dev install into active venv
maturin build --release   # Build wheel
```

The Justfile wraps this as `just engine`.

## Error Handling

`set_cells()` raises `ValueError` on circular dependencies (message contains "CIRCULAR").
`evaluate()` raises `ValueError` on errors like `#DIV/0!`.

## Tests

`tests/test_integration.py` — 13 Python tests covering literals, formulas, ranges, dependencies, circular detection, string functions, standalone evaluation.

Run via: `uv run pytest rust/crates/lotus-pyo3/tests/`
