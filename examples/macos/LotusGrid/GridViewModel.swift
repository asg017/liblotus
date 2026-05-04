import SwiftUI

/// Source of truth for the grid. Raw inputs are stored in Swift (canonical, serializable);
/// computed values are produced on demand by the Rust engine.
@MainActor
final class GridViewModel: ObservableObject {
    nonisolated static let cols = GridBounds.cols
    nonisolated static let rows = GridBounds.rows

    @Published private(set) var raw: [String: String] = [:]
    @Published private(set) var version: Int = 0

    private let engine = SheetEngine()
    private let store = Persistence()

    // [sheet.undo.scope]
    // Snapshots of `raw` taken *before* a committed mutation. Cap depth to
    // keep memory bounded; oldest entries are dropped silently.
    private var undoStack: [[String: String]] = []
    private var redoStack: [[String: String]] = []
    private let undoDepth = 50

    init() {
        raw = store.load()
        for (cell, value) in raw { engine.set(cell, value) }
    }

    func setCell(_ id: String, _ value: String) {
        let trimmed = value.trimmingCharacters(in: .whitespaces)
        if trimmed.isEmpty {
            raw.removeValue(forKey: id)
        } else {
            raw[id] = trimmed
        }
        engine.set(id, trimmed)
        version &+= 1
        store.save(raw)
    }

    /// Snapshot the current `raw` and push it onto the undo stack. Any caller
    /// about to run an undoable mutation (cell commit, delete, paste) should
    /// call this once before the mutation — `sheet.undo.scope` defines the set.
    func snapshot() {
        undoStack.append(raw)
        if undoStack.count > undoDepth { undoStack.removeFirst() }
        redoStack.removeAll()
    }

    // [sheet.undo.cmd-z]
    func undo() {
        guard let prev = undoStack.popLast() else { return }
        redoStack.append(raw)
        restore(prev)
    }

    // [sheet.undo.redo]
    func redo() {
        guard let next = redoStack.popLast() else { return }
        undoStack.append(raw)
        restore(next)
    }

    var canUndo: Bool { !undoStack.isEmpty }
    var canRedo: Bool { !redoStack.isEmpty }

    private func restore(_ snapshot: [String: String]) {
        // Clear any cells that existed before but don't in the snapshot.
        for key in raw.keys where snapshot[key] == nil {
            engine.set(key, "")
        }
        for (cell, value) in snapshot {
            engine.set(cell, value)
        }
        raw = snapshot
        version &+= 1
        store.save(raw)
    }

    func display(_ id: String) -> String { engine.display(id) }
    func rawValue(_ id: String) -> String { raw[id] ?? "" }

    nonisolated static func colLetter(_ index: Int) -> String {
        String(UnicodeScalar(0x41 + index)!)
    }
}
