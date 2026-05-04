import Foundation

// Pure-logic tests for Selection.swift. Compiled standalone so they don't
// depend on AppKit/SwiftUI or the lotus-ffi bridge — this lets us run them
// with plain `swiftc` from `run-tests.sh` without standing up an Xcode
// project. Run: `./tests/run-tests.sh`.

@main
struct SelectionTests {
    static var failures: [String] = []

    static func expectEqual<T: Equatable>(_ actual: T, _ expected: T, _ label: String,
                                           file: String = #file, line: Int = #line) {
        if actual != expected {
            failures.append("\(label): expected \(expected), got \(actual) (\(file):\(line))")
        }
    }

    static func main() {
        selectionBounds()
        cellCoordClamping()
        jumpTargetRules()
        cmdClickToggles()

        if failures.isEmpty {
            print("OK")
            exit(0)
        }
        for f in failures { print("FAIL: \(f)") }
        print("\(failures.count) failure(s)")
        exit(1)
    }

    // ---- Selection -----------------------------------------------------

    static func selectionBounds() {
        do {
            let s = Selection(CellCoord(col: 2, row: 3))
            expectEqual(s.minCol, 2, "solo min col")
            expectEqual(s.maxCol, 2, "solo max col")
            expectEqual(s.minRow, 3, "solo min row")
            expectEqual(s.maxRow, 3, "solo max row")
            expectEqual(s.contains(col: 2, row: 3), true, "solo contains self")
            expectEqual(s.contains(col: 1, row: 3), false, "solo rejects neighbor")
        }

        do {
            let s = Selection(
                anchor: CellCoord(col: 1, row: 2),
                cursor: CellCoord(col: 4, row: 5)
            )
            expectEqual(s.minCol, 1, "rect min col")
            expectEqual(s.maxCol, 4, "rect max col")
            expectEqual(s.minRow, 2, "rect min row")
            expectEqual(s.maxRow, 5, "rect max row")
            expectEqual(s.topLeft, CellCoord(col: 1, row: 2), "rect topLeft")
            expectEqual(s.contains(col: 3, row: 4), true, "rect contains mid")
            expectEqual(s.contains(col: 0, row: 4), false, "rect rejects out-left")
            expectEqual(s.contains(col: 5, row: 4), false, "rect rejects out-right")
            expectEqual(s.contains(col: 3, row: 1), false, "rect rejects out-above")
        }

        do {
            // Anchor bottom-right, cursor top-left — bounds should still be min/max.
            let s = Selection(
                anchor: CellCoord(col: 4, row: 5),
                cursor: CellCoord(col: 1, row: 2)
            )
            expectEqual(s.minCol, 1, "inverted min col")
            expectEqual(s.maxCol, 4, "inverted max col")
            expectEqual(s.topLeft, CellCoord(col: 1, row: 2), "inverted topLeft")
        }
    }

    // ---- CellCoord clamping --------------------------------------------

    static func cellCoordClamping() {
        do {
            let c = CellCoord(col: -3, row: 0).clamped()
            expectEqual(c.col, 0, "clamp col floor")
            expectEqual(c.row, 1, "clamp row floor")
        }
        do {
            let c = CellCoord(col: 99, row: 99).clamped()
            expectEqual(c.col, GridBounds.cols - 1, "clamp col ceil")
            expectEqual(c.row, GridBounds.rows, "clamp row ceil")
        }
    }

    // ---- jumpTarget (sheet.navigation.arrow-jump) ----------------------

    static func jumpTargetRules() {
        let cols = 8, rows = 20

        // Rule 1: at the grid edge in direction of travel → stay put.
        do {
            let start = CellCoord(col: 0, row: 1)
            let t = jumpTarget(from: start, dx: -1, dy: 0, cols: cols, rows: rows) { _ in false }
            expectEqual(t, start, "edge-left stays put")
            let t2 = jumpTarget(from: start, dx: 0, dy: -1, cols: cols, rows: rows) { _ in false }
            expectEqual(t2, start, "edge-up stays put")
        }

        // Rule 2: current filled + neighbour filled → walk to end of run.
        do {
            let filled: Set<String> = ["A1", "A2", "A3"]
            let t = jumpTarget(
                from: CellCoord(col: 0, row: 1), dx: 0, dy: 1,
                cols: cols, rows: rows
            ) { filled.contains($0.id) }
            expectEqual(t, CellCoord(col: 0, row: 3), "jump-down to end-of-run")
        }

        // Rule 2 edge: contiguous run that reaches the grid edge.
        do {
            let t = jumpTarget(
                from: CellCoord(col: 0, row: 1), dx: 0, dy: 1,
                cols: cols, rows: rows
            ) { $0.col == 0 }
            expectEqual(t, CellCoord(col: 0, row: rows), "jump-down run to grid edge")
        }

        // Rule 3: empty → skip to first filled cell.
        do {
            let t = jumpTarget(
                from: CellCoord(col: 0, row: 1), dx: 0, dy: 1,
                cols: cols, rows: rows
            ) { $0.id == "A5" }
            expectEqual(t, CellCoord(col: 0, row: 5), "skip empties to first filled")
        }

        // Rule 3: filled with empty neighbour → skip past empties to next filled.
        do {
            let filled: Set<String> = ["A1", "A5"]
            let t = jumpTarget(
                from: CellCoord(col: 0, row: 1), dx: 0, dy: 1,
                cols: cols, rows: rows
            ) { filled.contains($0.id) }
            expectEqual(t, CellCoord(col: 0, row: 5), "gap-skip to next filled")
        }

        // Rule 3: no filled cell in direction → snap to grid edge.
        do {
            let start = CellCoord(col: 0, row: 1)
            let t = jumpTarget(from: start, dx: 0, dy: 1, cols: cols, rows: rows) { _ in false }
            expectEqual(t, CellCoord(col: 0, row: rows), "no-filled snaps to bottom")
            let t2 = jumpTarget(from: start, dx: 1, dy: 0, cols: cols, rows: rows) { _ in false }
            expectEqual(t2, CellCoord(col: cols - 1, row: 1), "no-filled snaps to right")
        }

        // Rule 2: horizontal run.
        do {
            let filled: Set<String> = ["A1", "B1", "C1"]
            let t = jumpTarget(
                from: CellCoord(col: 0, row: 1), dx: 1, dy: 0,
                cols: cols, rows: rows
            ) { filled.contains($0.id) }
            expectEqual(t, CellCoord(col: 2, row: 1), "horizontal run ends at C1")
        }
    }

    // ---- Cmd+click toggle (sheet.selection.cmd-click) -------------------

    static func cmdClickToggles() {
        // Add cell outside the base rect → ends up in extras.
        do {
            var s = Selection(CellCoord(col: 0, row: 1))
            let target = CellCoord(col: 3, row: 5)
            s.toggle(target)
            expectEqual(s.extras.contains(target), true, "cmd-click adds to extras")
            expectEqual(s.anchor, CellCoord(col: 0, row: 1), "cmd-click preserves anchor")
            expectEqual(s.cursor, CellCoord(col: 0, row: 1), "cmd-click preserves cursor")
            expectEqual(s.contains(col: 3, row: 5), true, "extras reflected in contains()")
        }

        // Toggle off — cell already in extras is removed.
        do {
            var s = Selection(CellCoord(col: 0, row: 1))
            let target = CellCoord(col: 3, row: 5)
            s.toggle(target)
            s.toggle(target)
            expectEqual(s.extras.contains(target), false, "cmd-click removes from extras")
            expectEqual(s.contains(col: 3, row: 5), false, "removed cell no longer selected")
        }

        // Cell inside the base rect — toggle is a no-op (kept documented).
        do {
            var s = Selection(
                anchor: CellCoord(col: 0, row: 1),
                cursor: CellCoord(col: 3, row: 3)
            )
            s.toggle(CellCoord(col: 1, row: 2))
            expectEqual(s.extras.isEmpty, true, "rect cells aren't added to extras")
            expectEqual(s.contains(col: 1, row: 2), true, "rect cells stay selected")
        }

        // Bounding box expands to cover extras.
        do {
            var s = Selection(CellCoord(col: 0, row: 1))
            s.toggle(CellCoord(col: 5, row: 7))
            let bb = s.boundingBox
            expectEqual(bb.minCol, 0, "bbox min col over extras")
            expectEqual(bb.maxCol, 5, "bbox max col over extras")
            expectEqual(bb.minRow, 1, "bbox min row over extras")
            expectEqual(bb.maxRow, 7, "bbox max row over extras")
        }
    }
}
