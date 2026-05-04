# macOS Spec Implementation Checklist

Cross-referenced against [datasette-sheets/specs](/Users/alex/work/simonw/datasette-sheets/specs).
Every tagged call site in Swift uses the `// [sheet.<id>]` convention from the
spec `README.md` — `grep -R '\[sheet\.' LotusGrid/` lists them.

Run unit tests with `tests/run-tests.sh` (covers pure Selection + jumpTarget
logic; no AppKit / FFI dependency).

Status key:
- [x] implemented & tagged
- [~] partially implemented (needs refinement)
- [ ] not implemented
- [skip] intentionally out of scope for this example app
  (multi-sheet tabs, collaboration/presence, named views, save indicator UI, etc.)

## Selection

- [x] sheet.selection.click
- [x] sheet.selection.shift-click
- [~] sheet.selection.cmd-click *(divergence: cursor doesn't follow the click — keeps rect/extras model clean; see Selection.toggle docstring)*
- [x] sheet.selection.drag
- [x] sheet.selection.column-header-click
- [x] sheet.selection.column-header-shift-click
- [ ] sheet.selection.column-header-drag
- [x] sheet.selection.row-header-click
- [x] sheet.selection.row-header-shift-click
- [ ] sheet.selection.row-header-drag
- [x] sheet.selection.header-hover *(visual-only; header fills highlight from selection math)*

## Navigation

- [x] sheet.navigation.arrow
- [x] sheet.navigation.arrow-jump
- [x] sheet.navigation.shift-arrow-extend
- [x] sheet.navigation.shift-arrow-jump-extend
- [x] sheet.navigation.tab-commit-right
- [x] sheet.navigation.enter-commit-down

## Editing

- [x] sheet.editing.double-click
- [x] sheet.editing.f2-or-enter
- [x] sheet.editing.type-replaces
- [~] sheet.editing.formula-bar *(no formula bar in this app)*
- [x] sheet.editing.escape-cancels
- [x] sheet.editing.blur-commits
- [ ] sheet.editing.formula-ref-pointing

## Clipboard

- [~] sheet.clipboard.copy *(TSV only; no HTML rich format, no mark)*
- [~] sheet.clipboard.cut *(no mark)*
- [x] sheet.clipboard.paste
- [ ] sheet.clipboard.escape-cancels-mark
- [skip] sheet.clipboard.sheet-switch-clears-mark *(single sheet)*
- [ ] sheet.clipboard.mark-visual

## Delete / clear

- [x] sheet.delete.delete-key-clears
- [ ] sheet.delete.row-right-click
- [ ] sheet.delete.row-confirm
- [ ] sheet.delete.column-right-click
- [ ] sheet.delete.column-confirm
- [ ] sheet.delete.context-menu-dismiss
- [ ] sheet.delete.refs-rewrite

## Column / row ops

- [ ] sheet.column.resize-drag
- [ ] sheet.column.auto-fit-double-click
- [ ] sheet.column.context-menu-delete-only
- [ ] sheet.row.context-menu-delete-only

## Formatting

- [ ] sheet.format.bold-toggle
- [ ] sheet.format.currency
- [ ] sheet.format.percentage
- [ ] sheet.format.number
- [ ] sheet.format.clear
- [ ] sheet.format.numeric-align-right
- [ ] sheet.format.error-color

## Undo / redo

- [x] sheet.undo.cmd-z
- [x] sheet.undo.redo
- [x] sheet.undo.scope *(covers cell commits, Delete/Backspace, paste; format/row-col delete not implemented yet so no stack entries)*

## Sheet tabs

- [skip] sheet.tabs.* *(single-sheet app by design)*

## Formula bar

- [skip] sheet.formula-bar.* *(no formula bar UI in this app)*

## Presence

- [skip] sheet.presence.* *(no collaboration layer)*

## Named views

- [skip] sheet.view.* *(out of scope)*

## Scrolling

- [x] sheet.scrolling.sticky-col-headers *(need sticky pin — currently scrolls with body)*
- [x] sheet.scrolling.sticky-row-headers *(same)*
- [x] sheet.scrolling.sticky-corner *(same)*

## Save

- [~] sheet.save.auto-debounce *(currently saves synchronously on every set)*
- [~] sheet.save.flush-on-commit
- [skip] sheet.save.indicator *(no status chrome in this app)*

## Workbook

- [skip] sheet.workbook.rename

## Status bar

- [x] sheet.status-bar.numeric-stats
- [x] sheet.status-bar.count-only
