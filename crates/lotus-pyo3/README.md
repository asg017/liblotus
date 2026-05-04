# lotus-pyo3

PyO3 bindings that expose the `lotus-core` Rust spreadsheet engine to Python as a native extension module called `lotus`.

## Prerequisites

You need a Rust toolchain (install via [rustup](https://rustup.rs/)) and [maturin](https://github.com/PyO3/maturin), the build tool for PyO3-based Python extensions.

Install maturin:

```
pip install maturin
```

## Building

### Development build

From this directory (`rust/crates/lotus-pyo3`), run:

```
maturin develop
```

This compiles the Rust code in debug mode and installs the resulting `lotus` module directly into your current Python environment (virtualenv recommended). Changes are available immediately after re-running the command.

### Release build

```
maturin build --release
```

This produces a wheel file in `target/wheels/` that you can install with `pip install target/wheels/lotus-*.whl` or distribute to others.

## Usage

```python
import lotus

# Create a spreadsheet
sheet = lotus.Sheet()

# Set cells: pass a list of (cell_id, raw_value) tuples
sheet.set_cells([
    ("A1", "10"),
    ("A2", "20"),
    ("A3", "=A1+A2"),
])

# Read a computed value
print(sheet.get("A3"))   # "30"
print(sheet.get("Z99"))  # None (empty cell)

# Get all computed values as a dict
print(sheet.get_all())   # {"A1": "10", "A2": "20", "A3": "30"}

# One-shot formula evaluation (no sheet context)
print(lotus.evaluate("=1+2"))          # "3"
print(lotus.evaluate("=SUM(1,2,3)"))   # "6"
```

## API Reference

### `lotus.Sheet`

A spreadsheet with formula support, dependency tracking, and automatic recalculation.

#### `Sheet()`

Create a new empty spreadsheet.

```python
sheet = lotus.Sheet()
```

#### `Sheet.set_cells(changes: list[tuple[str, str]])`

Set one or more cells and recalculate the sheet. Each tuple is `(cell_id, raw_value)`. Cell IDs are strings like `"A1"`, `"B12"`, `"AA3"`. Values can be:

- A number: `"42"`, `"3.14"`
- A string: `"hello"`
- A formula: `"=A1+B1"`, `"=SUM(A1:A10)"`, `"=IF(A1, \"yes\", \"no\")"`
- An empty string `""` to delete the cell

Raises `ValueError` on circular dependencies.

```python
sheet.set_cells([("A1", "100"), ("A2", "=A1*2")])

# Delete a cell
sheet.set_cells([("A1", "")])

# Circular reference raises ValueError
try:
    sheet.set_cells([("A1", "=B1"), ("B1", "=A1")])
except ValueError as e:
    print(e)  # "#CIRCULAR! Cycle detected involving: A1, B1"
```

#### `Sheet.get(cell_id: str) -> str | None`

Return the computed value of a cell as a string, or `None` if the cell is empty.

```python
sheet.set_cells([("A1", "42"), ("A2", "=A1+8")])
sheet.get("A1")   # "42"
sheet.get("A2")   # "50"
sheet.get("Z1")   # None
```

#### `Sheet.get_all() -> dict[str, str]`

Return all non-empty computed values as a dictionary mapping cell IDs to their string representations.

```python
sheet.set_cells([("A1", "1"), ("A2", "2"), ("A3", "=A1+A2")])
sheet.get_all()   # {"A1": "1", "A2": "2", "A3": "3"}
```

### `lotus.evaluate(formula: str) -> str | None`

Evaluate a formula or literal value without any sheet context. Cell references will resolve to empty. Returns `None` for empty results. Raises `ValueError` on errors (e.g., division by zero).

```python
lotus.evaluate("=2^10")           # "1024"
lotus.evaluate("=SUM(1,2,3)")     # "6"
lotus.evaluate("hello")           # "hello"
lotus.evaluate("")                # None
lotus.evaluate("=1/0")            # raises ValueError: #DIV/0!
```

### Supported formulas

The engine supports standard spreadsheet formulas including:

- Arithmetic: `+`, `-`, `*`, `/`, `^`, `%`
- Cell references: `A1`, `B2`, `AA10`
- Ranges: `A1:B3`, `A:A` (whole column), `1:1` (whole row)
- Functions: `SUM`, `AVERAGE`, `MIN`, `MAX`, `COUNT`, `IF`, `CONCAT`, and more

## Integration with datasette-sheets

To use this engine from the datasette-sheets Python project, build the extension in a shared virtualenv:

```bash
cd rust/crates/lotus-pyo3
maturin develop --release
```

Then import from Python code in the parent project:

```python
from lotus import Sheet, evaluate

# Use Sheet for full spreadsheet functionality with dependency tracking
sheet = Sheet()
sheet.set_cells([("A1", "10"), ("B1", "=A1*2")])

# Use evaluate() for quick one-off formula computation
result = evaluate("=SUM(1,2,3)")
```

## pyproject.toml

This crate does not include a `pyproject.toml`. If you want to make the package pip-installable (e.g., `pip install .` or publish to PyPI), add a `pyproject.toml` in this directory:

```toml
[build-system]
requires = ["maturin>=1.0,<2.0"]
build-backend = "maturin"

[project]
name = "lotus"
version = "0.1.0"
requires-python = ">=3.8"
```

With that in place, `pip install .` and `maturin develop` will both work.
