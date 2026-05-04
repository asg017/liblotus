import Foundation

/// Thin Swift wrapper over the lotus-ffi C ABI.
final class SheetEngine {
    private let handle: OpaquePointer

    init() {
        guard let p = lotus_sheet_new() else { fatalError("lotus_sheet_new failed") }
        self.handle = p
    }

    deinit { lotus_sheet_free(handle) }

    /// Set a cell and recalculate. Returns nil on success, error message on failure.
    @discardableResult
    func set(_ cell: String, _ value: String) -> String? {
        guard let err = lotus_sheet_set(handle, cell, value) else { return nil }
        defer { lotus_string_free(err) }
        return String(cString: err)
    }

    /// Computed display value for a cell.
    func display(_ cell: String) -> String {
        guard let p = lotus_sheet_get(handle, cell) else { return "" }
        defer { lotus_string_free(p) }
        return String(cString: p)
    }

    /// Raw input the user typed. Empty if unset.
    func raw(_ cell: String) -> String {
        guard let p = lotus_sheet_get_raw(handle, cell) else { return "" }
        defer { lotus_string_free(p) }
        return String(cString: p)
    }
}
