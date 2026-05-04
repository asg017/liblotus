"""Interactive Textual TUI showcasing Python-side custom cell types.

Run from the lotus-pyo3 crate directory:

    uv run --no-project --with maturin --with textual bash -c \
        "maturin develop --release && python demo/app.py"

Three pre-registered handlers:

- `file` — any path ending in `.pdf` / `.txt` / `.md` becomes a file value.
   Custom function `PAGES(file)` mocks a page count from the path length.
- `color` — hex strings like `#ff8833` render with a swatch.
- `date` — `yyyy-mm-dd` strings ingest as dates.
   Custom function `DAYS_UNTIL(date)` returns a day count (may be negative).
"""

from __future__ import annotations

import datetime
import re
from pathlib import Path

import lotus
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.coordinate import Coordinate
from textual.reactive import reactive
from textual.widgets import DataTable, Footer, Header, Input, Static

# ── handlers ──────────────────────────────────────────────────────────

FILE_RE = re.compile(r".+\.(pdf|txt|md)$", re.IGNORECASE)
HEX_RE = re.compile(r"^#([0-9a-fA-F]{6})$")
DATE_RE = re.compile(r"^(\d{4})-(\d{2})-(\d{2})$")


class FileHandler:
    type_tag = "file"

    def parse_literal(self, raw):
        return {"data": raw} if FILE_RE.match(raw) else None

    def display(self, v):
        path = Path(v["data"])
        icons = {".pdf": "📄", ".txt": "📝", ".md": "📘"}
        return f"{icons.get(path.suffix.lower(), '📁')} {path.name}"


class ColorHandler:
    type_tag = "color"

    def parse_literal(self, raw):
        m = HEX_RE.match(raw.strip())
        return {"data": "#" + m.group(1).lower()} if m else None

    def display(self, v):
        return v["data"]

    def compare(self, op, lhs, rhs):
        both = (
            isinstance(lhs, dict)
            and isinstance(rhs, dict)
            and lhs.get("type_tag") == "color"
            and rhs.get("type_tag") == "color"
        )
        if not both:
            return None
        if op == "=":
            return lhs["data"] == rhs["data"]
        if op == "<>":
            return lhs["data"] != rhs["data"]
        return None


class DateHandler:
    type_tag = "date"

    def parse_literal(self, raw):
        return {"data": raw} if DATE_RE.match(raw.strip()) else None

    def display(self, v):
        return f"📅 {v['data']}"

    def compare(self, op, lhs, rhs):
        both = (
            isinstance(lhs, dict)
            and isinstance(rhs, dict)
            and lhs.get("type_tag") == "date"
            and rhs.get("type_tag") == "date"
        )
        if not both:
            return None
        a, b = lhs["data"], rhs["data"]  # ISO strings compare lexically
        return {"=": a == b, "<>": a != b, "<": a < b, "<=": a <= b, ">": a > b, ">=": a >= b}.get(op)


def pages_fn(args):
    """Mock PDF page count: hash the path string to something stable."""
    v = args[0]
    data = v["data"] if isinstance(v, dict) else str(v)
    return (len(data) * 13) % 200 + 1


def days_until_fn(args):
    v = args[0]
    data = v["data"] if isinstance(v, dict) else str(v)
    m = DATE_RE.match(data)
    if not m:
        return {"error": "#DATE!"}
    target = datetime.date(int(m.group(1)), int(m.group(2)), int(m.group(3)))
    today = datetime.date.today()
    return (target - today).days


# ── starter cells ─────────────────────────────────────────────────────

STARTER = [
    ("A1", "report.pdf"),
    ("A2", "=PAGES(A1)"),
    ("A3", "readme.md"),
    ("A4", "=PAGES(A3)"),
    ("B1", "#ff8833"),
    ("B2", "#ff8833"),
    ("B3", "=B1=B2"),
    ("B4", "#00aa55"),
    ("B5", "=B1=B4"),
    ("C1", "2026-12-25"),
    ("C2", "=DAYS_UNTIL(C1)"),
    ("C3", '=CONCAT("days to xmas: ", C2)'),
    ("D1", "42"),
    ("D2", "=D1 * 2"),
    ("E1", "hello"),
    ("E2", '=UPPER(E1) & " world"'),
]

ROWS = 10
COLS = 6


def col_letter(idx: int) -> str:
    return chr(65 + idx)


def cell_id(r: int, c: int) -> str:
    return f"{col_letter(c)}{r + 1}"


# ── Textual app ───────────────────────────────────────────────────────


class FormulaBar(Input):
    BINDINGS = [Binding("escape", "app.focus_grid", "cancel")]


class LotusApp(App):
    CSS = """
    Header { content-align: center middle; }
    #wrap { height: 1fr; }
    #left { width: 3fr; }
    #right { width: 1fr; padding: 1 2; background: $panel; }
    #formula { margin: 1 2 0 2; }
    #grid { margin: 0 2 1 2; height: 1fr; }
    DataTable { height: 1fr; }
    .swatch { color: $accent; }
    .error { color: $error; }
    .custom { color: $success; }
    #handlers { color: $text-muted; }
    """
    BINDINGS = [
        Binding("enter", "edit_cell", "edit"),
        Binding("delete", "clear_cell", "clear"),
        Binding("q", "quit", "quit"),
    ]

    selected = reactive(Coordinate(0, 0))

    def __init__(self) -> None:
        super().__init__()
        self.sheet = lotus.Sheet()
        self.sheet.register_type(FileHandler())
        self.sheet.register_type(ColorHandler())
        self.sheet.register_type(DateHandler())
        self.sheet.register_function("PAGES", pages_fn)
        self.sheet.register_function("DAYS_UNTIL", days_until_fn)
        self.sheet.set_cells(STARTER)
        self.raw_shadow: dict[str, str] = dict(STARTER)

    def compose(self) -> ComposeResult:
        yield Header(show_clock=False)
        yield FormulaBar(placeholder="select a cell, press Enter to edit…", id="formula")
        with Horizontal(id="wrap"):
            with Vertical(id="left"):
                yield DataTable(id="grid", cursor_type="cell", zebra_stripes=True)
            with Vertical(id="right"):
                yield Static(
                    "[b]handlers[/b]\n"
                    "  [dim]file[/dim]  — .pdf / .txt / .md\n"
                    "         PAGES(ref) → int\n"
                    "  [dim]color[/dim] — #rrggbb, compares by RGB\n"
                    "  [dim]date[/dim]  — yyyy-mm-dd\n"
                    "         DAYS_UNTIL(ref)\n\n"
                    "[b]try[/b]\n"
                    "  notes.txt\n"
                    "  #bada55\n"
                    "  2030-01-01\n"
                    "  =DAYS_UNTIL(C1)\n"
                    "  =CONCAT(\"📘 \", A3)\n\n"
                    "[dim]↑↓←→  navigate\n"
                    "Enter  edit\n"
                    "Del    clear\n"
                    "q      quit[/dim]",
                    id="handlers",
                )
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one("#grid", DataTable)
        table.add_column("", width=4, key="__rownum__")
        for c in range(COLS):
            table.add_column(col_letter(c), width=22)
        for r in range(ROWS):
            table.add_row(str(r + 1), *(self._render_cell(r, c) for c in range(COLS)))
        table.focus()
        self._sync_formula()

    def on_data_table_cell_highlighted(self, event: DataTable.CellHighlighted) -> None:
        self.selected = event.coordinate
        self._sync_formula()

    def action_focus_grid(self) -> None:
        self.query_one("#grid", DataTable).focus()

    def action_edit_cell(self) -> None:
        inp = self.query_one("#formula", FormulaBar)
        inp.focus()

    def action_clear_cell(self) -> None:
        r, c = self.selected.row, self.selected.column - 1
        if c < 0:
            return
        cid = cell_id(r, c)
        self.sheet.set_cells([(cid, "")])
        self.raw_shadow.pop(cid, None)
        self._redraw()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        r, c = self.selected.row, self.selected.column - 1
        if c < 0:
            self.query_one("#grid", DataTable).focus()
            return
        cid = cell_id(r, c)
        value = event.value
        try:
            self.sheet.set_cells([(cid, value)])
        except ValueError as e:
            self.notify(f"{cid}: {e}", severity="error")
            return
        if value:
            self.raw_shadow[cid] = value
        else:
            self.raw_shadow.pop(cid, None)
        self._redraw()
        # advance to next row
        next_row = min(ROWS - 1, r + 1)
        table = self.query_one("#grid", DataTable)
        table.cursor_coordinate = Coordinate(next_row, c + 1)
        table.focus()

    # ── rendering ───────────────────────────────────────────────────

    def _render_cell(self, r: int, c: int) -> str:
        cid = cell_id(r, c)
        v = self.sheet.get_typed(cid)
        if v is None:
            return ""
        if isinstance(v, bool):
            return "TRUE" if v else "FALSE"
        if isinstance(v, (int, float)):
            return f"{v}"
        if isinstance(v, str):
            if v.startswith("#") and v.endswith("!"):
                return f"[red]{v}[/red]"
            return v
        if isinstance(v, dict) and "type_tag" in v:
            tag, data = v["type_tag"], v["data"]
            if tag == "color":
                return f"[on {data}]  [/on {data}] [bold]{data}[/bold]"
            if tag == "file":
                path = Path(data)
                icons = {".pdf": "📄", ".txt": "📝", ".md": "📘"}
                return f"{icons.get(path.suffix.lower(), '📁')} {path.name}"
            if tag == "date":
                return f"📅 {data}"
            return f"[cyan]{data}[/cyan]"
        return str(v)

    def _redraw(self) -> None:
        table = self.query_one("#grid", DataTable)
        for r in range(ROWS):
            for c in range(COLS):
                table.update_cell_at(Coordinate(r, c + 1), self._render_cell(r, c))

    def _sync_formula(self) -> None:
        inp = self.query_one("#formula", FormulaBar)
        r, c = self.selected.row, self.selected.column - 1
        if c < 0:
            inp.value = ""
            inp.placeholder = "row-header selected — arrow right to a cell"
            return
        cid = cell_id(r, c)
        inp.value = self.raw_shadow.get(cid, "")
        inp.placeholder = f"{cid} — Enter to edit"


if __name__ == "__main__":
    LotusApp().run()
