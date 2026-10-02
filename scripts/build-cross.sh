#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked --target x86_64-pc-windows-gnu
mkdir -p dist/QuickPanel-Windows-x64
cp target/x86_64-pc-windows-gnu/release/quick-panel.exe dist/QuickPanel-Windows-x64/QuickPanel.exe
cp README.md LICENSE dist/QuickPanel-Windows-x64/
cd dist/QuickPanel-Windows-x64
zip -q -r ../QuickPanel-Windows-x64.zip QuickPanel.exe README.md LICENSE
