#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${QP_NODE:?Set QP_NODE to an absolute Node 24.19.0 executable}"
: "${QP_PNPM:?Set QP_PNPM to the pinned pnpm.cjs path}"
: "${QP_NODE_ASSETS:?Set QP_NODE_ASSETS to verified Windows node.exe and LICENSE directory}"
[[ "$("$QP_NODE" --version)" == "v24.19.0" ]] || { echo "Node 24.19.0 required"; exit 1; }
export PATH="$(dirname "$QP_NODE"):$PATH"
"$QP_NODE" "$QP_PNPM" --dir agent-runtime install --ignore-scripts --frozen-lockfile
"$QP_NODE" agent-runtime/scripts/build.mjs
export CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cargo build --release --locked --target x86_64-pc-windows-gnu
python3 scripts/package-portable.py --node-assets "$QP_NODE_ASSETS"
