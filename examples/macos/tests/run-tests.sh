#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="build/test-selection"
mkdir -p build

# Only compile files that have no AppKit / SwiftUI / FFI dependencies.
# Selection.swift is the unit under test; the test file uses Foundation only.
swiftc \
  -O \
  -parse-as-library \
  -target "$(uname -m)-apple-macos14.0" \
  -o "$OUT" \
  LotusGrid/Selection.swift \
  tests/SelectionTests.swift

"$OUT"
