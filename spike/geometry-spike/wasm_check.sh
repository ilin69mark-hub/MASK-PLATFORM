#!/usr/bin/env bash
# Criterion 5 for ADR-0003: the geometry kernel must compile to wasm32 so the
# browser engine (ADR-0013) can run the same code the server runs.
#
# The probe binary is a native executable, so we compile the library
# dependency chain for wasm32 rather than linking a binary.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")"

TARGET="wasm32-unknown-unknown"

echo "=== rustc version ==="
rustc --version

echo
echo "=== installed targets ==="
rustup target list --installed

if ! rustup target list --installed | grep -qx "$TARGET"; then
  echo
  echo "--- adding $TARGET ---"
  rustup target add "$TARGET"
fi

echo
echo "=== building dependency graph for $TARGET ==="
# `cargo build` with the wasm target compiles i_overlay and all of its
# dependencies for wasm32. --lib is unavailable for a bin-only crate, so we
# build the binary target and simply verify compilation succeeds.
if cargo build --target "$TARGET" 2>&1 | tail -20; then
  echo
  echo "RESULT: geometry dependency graph COMPILES for $TARGET"
else
  echo
  echo "RESULT: FAILED to compile for $TARGET"
  exit 1
fi