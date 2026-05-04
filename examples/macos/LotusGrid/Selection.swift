import Foundation

enum GridBounds {
    static let cols = 8   // columns are 0-based; valid range is 0..<cols
    static let rows = 20  // rows are 1-based;    valid range is 1...rows
}

/// 0-based column letter (A..H for the default 8-column grid).
func colLetter(_ index: Int) -> String {
    String(UnicodeScalar(0x41 + index)!)
}

struct CellCoord: Hashable {
    var col: Int      // 0-based
    var row: Int      // 1-based
    var id: String { "\(colLetter(col))\(row)" }

    func clamped() -> CellCoord {
        CellCoord(
            col: max(0, min(GridBounds.cols - 1, col)),
            row: max(1, min(GridBounds.rows, row))
        )
    }
}

struct Selection: Equatable {
    var anchor: CellCoord
    var cursor: CellCoord
    /// Extra singleton cells added via Cmd+click outside the base rectangle.
    /// The visible selection is `rect(anchor, cursor) ∪ extras`. See
    /// `sheet.selection.cmd-click`.
    var extras: Set<CellCoord> = []

    init(_ c: CellCoord) { self.anchor = c; self.cursor = c }
    init(anchor: CellCoord, cursor: CellCoord) { self.anchor = anchor; self.cursor = cursor }

    var minCol: Int { min(anchor.col, cursor.col) }
    var maxCol: Int { max(anchor.col, cursor.col) }
    var minRow: Int { min(anchor.row, cursor.row) }
    var maxRow: Int { max(anchor.row, cursor.row) }
    var topLeft: CellCoord { CellCoord(col: minCol, row: minRow) }

    /// Bounding box that covers the base rectangle together with any Cmd+click
    /// extras. Used for `sheet.clipboard.copy` ("iterate the full box and
    /// treat cells that aren't in the selection as empty").
    var boundingBox: (minCol: Int, maxCol: Int, minRow: Int, maxRow: Int) {
        var lo = (col: minCol, row: minRow)
        var hi = (col: maxCol, row: maxRow)
        for e in extras {
            if e.col < lo.col { lo.col = e.col }
            if e.col > hi.col { hi.col = e.col }
            if e.row < lo.row { lo.row = e.row }
            if e.row > hi.row { hi.row = e.row }
        }
        return (lo.col, hi.col, lo.row, hi.row)
    }

    func rectContains(_ c: CellCoord) -> Bool {
        c.col >= minCol && c.col <= maxCol && c.row >= minRow && c.row <= maxRow
    }

    func contains(col: Int, row: Int) -> Bool {
        let c = CellCoord(col: col, row: row)
        return rectContains(c) || extras.contains(c)
    }

    /// Cmd+click semantics: toggle a cell in/out of the disjoint extras set.
    ///
    /// Known divergence from `sheet.selection.cmd-click`: the active cell
    /// does *not* follow the click. Moving the cursor would make
    /// `rect(anchor, cursor)` swallow the "extra" cell, conflating the rect
    /// and extras models — the simple fix is to keep the cursor put and
    /// treat extras as a pure disjoint set. Shift+Arrow / Shift+click still
    /// rebuild from the anchor as the spec requires.
    ///
    /// Cells inside the base rectangle cannot be removed here either; the
    /// rect is atomic.
    mutating func toggle(_ c: CellCoord) {
        if rectContains(c) { return }
        if extras.remove(c) != nil { return }
        extras.insert(c)
    }
}

/// Pure content-aware jump logic for `sheet.navigation.arrow-jump`.
///
/// `cols` is exclusive upper bound for columns (0-based), `rows` is inclusive
/// upper bound for rows (1-based), matching `GridViewModel`'s conventions.
/// `isFilled` returns true iff the cell's raw value is a non-empty string.
func jumpTarget(
    from start: CellCoord,
    dx: Int,
    dy: Int,
    cols: Int,
    rows: Int,
    isFilled: (CellCoord) -> Bool
) -> CellCoord {
    func inBounds(_ c: CellCoord) -> Bool {
        c.col >= 0 && c.col < cols && c.row >= 1 && c.row <= rows
    }
    let firstNext = CellCoord(col: start.col + dx, row: start.row + dy)
    // Rule 1: already at the grid edge → no-op.
    if !inBounds(firstNext) { return start }

    // Rule 2: current filled AND neighbour filled → walk to end of the run.
    if isFilled(start) && isFilled(firstNext) {
        var cur = firstNext
        while true {
            let next = CellCoord(col: cur.col + dx, row: cur.row + dy)
            if !inBounds(next) || !isFilled(next) { return cur }
            cur = next
        }
    }

    // Rule 3: skip empties to the first filled cell, or snap to the grid edge.
    var cur = firstNext
    while !isFilled(cur) {
        let next = CellCoord(col: cur.col + dx, row: cur.row + dy)
        if !inBounds(next) { return cur }
        cur = next
    }
    return cur
}
