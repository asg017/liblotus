import SwiftUI
import AppKit

struct ContentView: View {
    @ObservedObject var vm: GridViewModel
    @State private var selection = Selection(CellCoord(col: 0, row: 1))
    @State private var editing: String? = nil
    @State private var editBuffer: String = ""
    @State private var dragAnchor: CellCoord? = nil
    @FocusState private var focus: Focus?

    enum Focus: Hashable {
        case grid
        case cell(String)
    }

    private let cellWidth: CGFloat = 90
    private let cellHeight: CGFloat = 22

    var body: some View {
        VStack(spacing: 0) {
            gridView
            statusBar
        }
    }

    private var gridView: some View {
        ScrollViewReader { proxy in
            ScrollView([.horizontal, .vertical]) {
                Grid(horizontalSpacing: 0, verticalSpacing: 0) {
                    GridRow {
                        corner()
                        ForEach(0..<GridViewModel.cols, id: \.self) { c in
                            columnHeader(c)
                        }
                    }
                    ForEach(1...GridViewModel.rows, id: \.self) { r in
                        GridRow {
                            rowHeader(r)
                            ForEach(0..<GridViewModel.cols, id: \.self) { c in
                                cell(coord: CellCoord(col: c, row: r))
                            }
                        }
                    }
                }
                .coordinateSpace(name: "grid")
            }
            .onChange(of: selection.cursor) { _, new in
                proxy.scrollTo(new.id, anchor: .center)
            }
            // [sheet.editing.blur-commits]
            .onChange(of: focus) { _, new in
                if let id = editing, new != .cell(id) { commitInPlace() }
            }
        }
        .padding(8)
        .frame(
            minWidth: cellWidth * CGFloat(GridViewModel.cols + 1) + 32,
            minHeight: cellHeight * CGFloat(GridViewModel.rows + 1) + 32
        )
        .focusable()
        .focused($focus, equals: .grid)
        .focusEffectDisabled()
        .onAppear { focus = .grid }
        .onKeyPress(phases: [.down, .repeat]) { press in handleKey(press) }
    }

    // MARK: - Status bar

    @ViewBuilder
    private var statusBar: some View {
        let stats = selectionStats()
        // [sheet.status-bar.count-only] (single-cell hides entirely)
        if stats.count <= 1 {
            EmptyView()
        } else if stats.numericValues.isEmpty {
            // [sheet.status-bar.count-only]
            HStack {
                Spacer()
                Text("\(stats.count) cells selected")
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 4)
            .background(Color(NSColor.controlBackgroundColor))
        } else {
            // [sheet.status-bar.numeric-stats]
            let values = stats.numericValues
            let sum = values.reduce(0, +)
            let avg = sum / Double(values.count)
            let mn = values.min() ?? 0
            let mx = values.max() ?? 0
            HStack(spacing: 14) {
                Spacer()
                statLabel("Sum",   formatStat(sum))
                statLabel("Avg",   formatStat(avg))
                statLabel("Min",   formatStat(mn))
                statLabel("Max",   formatStat(mx))
                statLabel("Count", "\(values.count)")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 4)
            .background(Color(NSColor.controlBackgroundColor))
        }
    }

    private func statLabel(_ name: String, _ value: String) -> some View {
        HStack(spacing: 4) {
            Text(name).foregroundStyle(.secondary)
            Text(value).foregroundStyle(.primary)
        }
        .font(.system(size: 11, design: .monospaced))
    }

    private func formatStat(_ value: Double) -> String {
        let f = NumberFormatter()
        f.numberStyle = .decimal
        f.maximumFractionDigits = 4
        f.minimumFractionDigits = 0
        return f.string(from: NSNumber(value: value)) ?? "\(value)"
    }

    private func selectionStats() -> (count: Int, numericValues: [Double]) {
        let bb = selection.boundingBox
        var count = 0
        var nums: [Double] = []
        for r in bb.minRow...bb.maxRow {
            for c in bb.minCol...bb.maxCol {
                guard selection.contains(col: c, row: r) else { continue }
                count += 1
                let display = vm.display("\(GridViewModel.colLetter(c))\(r)")
                if let d = Double(display) { nums.append(d) }
            }
        }
        return (count, nums)
    }

    // MARK: - Cells

    @ViewBuilder
    private func cell(coord: CellCoord) -> some View {
        let id = coord.id
        let isSelected = selection.contains(col: coord.col, row: coord.row)
        let isCursor = selection.cursor == coord

        if editing == id {
            TextField("", text: $editBuffer)
                .textFieldStyle(.plain)
                .font(.system(size: 12, design: .monospaced))
                .frame(width: cellWidth, height: cellHeight)
                .padding(.horizontal, 4)
                .background(Color.accentColor.opacity(0.18))
                .overlay(Rectangle().stroke(Color.accentColor, lineWidth: 2))
                .focused($focus, equals: .cell(id))
                .id(id)
                // [sheet.navigation.enter-commit-down]
                .onSubmit { commit(moveDx: 0, moveDy: 1) }
                .onKeyPress(keys: [.return], phases: [.down]) { press in
                    guard press.modifiers.contains(.shift) else { return .ignored }
                    commit(moveDx: 0, moveDy: -1)
                    return .handled
                }
                // [sheet.editing.escape-cancels]
                .onExitCommand { cancelEdit() }
                // [sheet.navigation.tab-commit-right]
                .onKeyPress(keys: [.tab], phases: [.down]) { press in
                    commit(moveDx: press.modifiers.contains(.shift) ? -1 : 1, moveDy: 0)
                    return .handled
                }
        } else {
            Text(vm.display(id))
                .font(.system(size: 12, design: .monospaced))
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(width: cellWidth, height: cellHeight, alignment: .leading)
                .padding(.horizontal, 4)
                .background(cellBackground(selected: isSelected, cursor: isCursor))
                .overlay(
                    Rectangle().stroke(
                        isCursor ? Color.accentColor : Color.gray.opacity(0.25),
                        lineWidth: isCursor ? 2 : 0.5
                    )
                )
                .contentShape(Rectangle())
                .id(id)
                // [sheet.editing.double-click]
                .onTapGesture(count: 2) {
                    beginEdit(coord, initial: vm.rawValue(id))
                }
                .onTapGesture(count: 1) {
                    let mods = NSEvent.modifierFlags
                    if mods.contains(.command) {
                        // [sheet.selection.cmd-click]
                        selection.toggle(coord)
                    } else if mods.contains(.shift) {
                        // [sheet.selection.shift-click]
                        selection.extras.removeAll()
                        selection.cursor = coord
                    } else {
                        // [sheet.selection.click]
                        selection = Selection(coord)
                    }
                    focus = .grid
                }
                // [sheet.selection.drag]
                .gesture(
                    DragGesture(minimumDistance: 4, coordinateSpace: .named("grid"))
                        .onChanged { value in
                            if dragAnchor == nil {
                                dragAnchor = coord
                                selection = Selection(coord)
                                focus = .grid
                            }
                            guard let a = dragAnchor else { return }
                            let target = cellAt(value.location)
                            selection = Selection(anchor: a, cursor: target)
                        }
                        .onEnded { _ in dragAnchor = nil }
                )
        }
    }

    private func cellBackground(selected: Bool, cursor: Bool) -> Color {
        guard selected else { return Color(NSColor.textBackgroundColor) }
        return cursor
            ? Color.accentColor.opacity(0.10)
            : Color.accentColor.opacity(0.22)
    }

    private func header(_ label: String, highlighted: Bool) -> some View {
        Text(label)
            .font(.system(size: 11, design: .monospaced).weight(.semibold))
            .foregroundStyle(highlighted ? .primary : .secondary)
            .frame(width: cellWidth, height: cellHeight)
            .background(
                highlighted
                    ? Color.accentColor.opacity(0.18)
                    : Color(NSColor.controlBackgroundColor)
            )
            .overlay(Rectangle().stroke(Color.gray.opacity(0.25), lineWidth: 0.5))
    }

    @ViewBuilder
    private func columnHeader(_ c: Int) -> some View {
        let highlighted = c >= selection.minCol && c <= selection.maxCol
        header(GridViewModel.colLetter(c), highlighted: highlighted)
            .contentShape(Rectangle())
            .onTapGesture {
                let mods = NSEvent.modifierFlags
                if mods.contains(.shift) {
                    // [sheet.selection.column-header-shift-click]
                    selection.extras.removeAll()
                    selection.cursor = CellCoord(col: c, row: 1)
                } else {
                    // [sheet.selection.column-header-click]
                    selection = Selection(
                        anchor: CellCoord(col: c, row: GridViewModel.rows),
                        cursor: CellCoord(col: c, row: 1)
                    )
                }
                focus = .grid
            }
    }

    @ViewBuilder
    private func rowHeader(_ r: Int) -> some View {
        let highlighted = r >= selection.minRow && r <= selection.maxRow
        header("\(r)", highlighted: highlighted)
            .contentShape(Rectangle())
            .onTapGesture {
                let mods = NSEvent.modifierFlags
                if mods.contains(.shift) {
                    // [sheet.selection.row-header-shift-click]
                    selection.extras.removeAll()
                    selection.cursor = CellCoord(col: 0, row: r)
                } else {
                    // [sheet.selection.row-header-click]
                    selection = Selection(
                        anchor: CellCoord(col: GridViewModel.cols - 1, row: r),
                        cursor: CellCoord(col: 0, row: r)
                    )
                }
                focus = .grid
            }
    }

    private func corner() -> some View {
        Color(NSColor.controlBackgroundColor)
            .frame(width: cellWidth, height: cellHeight)
            .overlay(Rectangle().stroke(Color.gray.opacity(0.25), lineWidth: 0.5))
    }

    // MARK: - Key handling

    private func handleKey(_ press: KeyPress) -> KeyPress.Result {
        if editing != nil { return .ignored }
        let mods = press.modifiers

        // [sheet.navigation.arrow] / [sheet.navigation.shift-arrow-extend]
        // [sheet.navigation.arrow-jump] / [sheet.navigation.shift-arrow-jump-extend]
        if let (dx, dy) = arrowDelta(press.key) {
            if mods.contains(.command) {
                let target = jumpTarget(
                    from: selection.cursor, dx: dx, dy: dy,
                    cols: GridViewModel.cols, rows: GridViewModel.rows,
                    isFilled: { !vm.rawValue($0.id).isEmpty }
                )
                if mods.contains(.shift) {
                    selection.extras.removeAll()
                    selection.cursor = target
                } else {
                    selection = Selection(target)
                }
                return .handled
            }
            move(dx: dx, dy: dy, extend: mods.contains(.shift))
            return .handled
        }
        switch press.key {
        case .tab:
            move(dx: mods.contains(.shift) ? -1 : 1, dy: 0, extend: false)
            return .handled
        case .return:
            // [sheet.editing.f2-or-enter]
            beginEdit(selection.cursor, initial: vm.rawValue(selection.cursor.id))
            return .handled
        case .delete, .deleteForward:
            // [sheet.delete.delete-key-clears]
            clearSelection()
            return .handled
        case .escape:
            selection = Selection(selection.cursor)
            return .handled
        default: break
        }

        if mods.contains(.command) {
            switch press.characters.lowercased() {
            // [sheet.clipboard.copy]
            case "c": copy();      return .handled
            // [sheet.clipboard.paste]
            case "v": paste();     return .handled
            // [sheet.clipboard.cut]
            case "x": cut();       return .handled
            case "a": selectAll(); return .handled
            case "z":
                if mods.contains(.shift) {
                    // [sheet.undo.redo]
                    vm.redo()
                } else {
                    // [sheet.undo.cmd-z]
                    vm.undo()
                }
                return .handled
            case "y":
                // [sheet.undo.redo]
                vm.redo()
                return .handled
            default:  return .ignored
            }
        }

        // [sheet.editing.type-replaces]
        // Any printable character starts edit mode with that character as the initial buffer.
        if !mods.contains(.control), !mods.contains(.option),
           let first = press.characters.first, first.isASCII,
           let ascii = first.asciiValue, ascii >= 0x20, ascii < 0x7F {
            beginEdit(selection.cursor, initial: press.characters)
            return .handled
        }
        return .ignored
    }

    /// Map a point in the "grid" coordinate space back to a cell. The grid
    /// is laid out with a header row / column of the same cellWidth×cellHeight,
    /// so data cells occupy x ∈ [(c+1)*cellWidth, (c+2)*cellWidth) and
    /// y ∈ [r*cellHeight, (r+1)*cellHeight) for 0-based col / 1-based row.
    private func cellAt(_ p: CGPoint) -> CellCoord {
        let col = Int((p.x / cellWidth).rounded(.down)) - 1
        let row = Int((p.y / cellHeight).rounded(.down))
        return CellCoord(col: col, row: max(row, 1)).clamped()
    }

    private func arrowDelta(_ key: KeyEquivalent) -> (Int, Int)? {
        switch key {
        case .leftArrow:  return (-1,  0)
        case .rightArrow: return ( 1,  0)
        case .upArrow:    return ( 0, -1)
        case .downArrow:  return ( 0,  1)
        default:          return nil
        }
    }

    private func move(dx: Int, dy: Int, extend: Bool) {
        let next = CellCoord(
            col: selection.cursor.col + dx,
            row: selection.cursor.row + dy
        ).clamped()
        if extend {
            selection.extras.removeAll()
            selection.cursor = next
        } else {
            selection = Selection(next)
        }
    }

    private func selectAll() {
        selection = Selection(
            anchor: CellCoord(col: 0, row: 1),
            cursor: CellCoord(col: GridViewModel.cols - 1, row: GridViewModel.rows)
        )
    }

    // MARK: - Edit

    private func beginEdit(_ coord: CellCoord, initial: String) {
        if let existing = editing, existing != coord.id { commitInPlace() }
        selection = Selection(coord)
        editBuffer = initial
        editing = coord.id
        DispatchQueue.main.async { focus = .cell(coord.id) }
    }

    private func commitInPlace() {
        guard let id = editing else { return }
        // [sheet.undo.scope] — snapshot before the commit.
        vm.snapshot()
        vm.setCell(id, editBuffer)
        editing = nil
        editBuffer = ""
    }

    private func commit(moveDx: Int, moveDy: Int) {
        guard editing != nil else { return }
        commitInPlace()
        move(dx: moveDx, dy: moveDy, extend: false)
        focus = .grid
    }

    private func cancelEdit() {
        editing = nil
        editBuffer = ""
        focus = .grid
    }

    // MARK: - Clipboard / bulk ops

    private func copy() {
        let bb = selection.boundingBox
        var rows: [String] = []
        for r in bb.minRow...bb.maxRow {
            var cols: [String] = []
            for c in bb.minCol...bb.maxCol {
                let cell = selection.contains(col: c, row: r)
                    ? vm.rawValue("\(GridViewModel.colLetter(c))\(r)")
                    : ""
                cols.append(cell)
            }
            rows.append(cols.joined(separator: "\t"))
        }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(rows.joined(separator: "\n"), forType: .string)
    }

    private func paste() {
        guard let str = NSPasteboard.general.string(forType: .string) else { return }
        let cleaned = str
            .replacingOccurrences(of: "\r\n", with: "\n")
            .trimmingCharacters(in: CharacterSet(charactersIn: "\n"))
        guard !cleaned.isEmpty else { return }

        let rows = cleaned.split(separator: "\n", omittingEmptySubsequences: false)
        let origin = selection.topLeft
        var widest = 0
        // [sheet.undo.scope] — one snapshot covers the whole paste.
        vm.snapshot()
        for (dy, row) in rows.enumerated() {
            let cols = row.split(separator: "\t", omittingEmptySubsequences: false)
            widest = max(widest, cols.count)
            for (dx, value) in cols.enumerated() {
                let col = origin.col + dx
                let r = origin.row + dy
                guard col < GridViewModel.cols, r <= GridViewModel.rows else { continue }
                vm.setCell("\(GridViewModel.colLetter(col))\(r)", String(value))
            }
        }
        let lastCol = min(origin.col + max(widest - 1, 0), GridViewModel.cols - 1)
        let lastRow = min(origin.row + rows.count - 1, GridViewModel.rows)
        selection = Selection(
            anchor: origin,
            cursor: CellCoord(col: lastCol, row: lastRow)
        )
    }

    private func clearSelection() {
        let bb = selection.boundingBox
        // [sheet.undo.scope] — one snapshot covers the whole delete keypress.
        vm.snapshot()
        for r in bb.minRow...bb.maxRow {
            for c in bb.minCol...bb.maxCol {
                guard selection.contains(col: c, row: r) else { continue }
                vm.setCell("\(GridViewModel.colLetter(c))\(r)", "")
            }
        }
    }

    private func cut() { copy(); clearSelection() }
}
