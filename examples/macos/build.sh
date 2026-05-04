#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

echo "==> Building lotus-ffi (Rust staticlib)..."
(cd lotus-ffi && cargo build --release)

APP="build/LotusGrid.app"
BIN_DIR="$APP/Contents/MacOS"
RES_DIR="$APP/Contents/Resources"
rm -rf "$APP"
mkdir -p "$BIN_DIR" "$RES_DIR"
cp Info.plist "$APP/Contents/Info.plist"

echo "==> Compiling Swift..."
swiftc \
  -O \
  -parse-as-library \
  -target "$(uname -m)-apple-macos14.0" \
  -import-objc-header LotusGrid/lotus_ffi.h \
  -L lotus-ffi/target/release \
  -llotus_ffi \
  -framework AppKit \
  -framework Carbon \
  -framework SwiftUI \
  -o "$BIN_DIR/LotusGrid" \
  LotusGrid/*.swift

echo "==> Ad-hoc code signing..."
codesign --force --sign - "$APP"

echo ""
echo "Built $APP"
echo "Run it with:  open $APP"
echo "Hotkey:       ⌘⇧Space toggles the grid"
