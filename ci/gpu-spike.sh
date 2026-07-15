#!/usr/bin/env bash
# Keep the deferred GPU adapter source-compatible without promoting it into
# the product workspace or release artifacts.
set -euo pipefail
cd "$(dirname "$0")/.."

manifest="spikes/frontend-wgpu/Cargo.toml"

cargo fmt --manifest-path "$manifest" -- --check
# gpu-renderer compiles this file through include!, which cargo fmt does not
# discover as a module. Keep the shared renderer source in the format gate.
rustfmt --check --edition 2024 spikes/frontend-wgpu/src/gpu.rs
cargo test --manifest-path "$manifest" --locked --workspace --all-targets

# The renderer is a separate spike-local crate. Prove its current-platform
# normal dependency tree contains the scene contract and GPU stack but no
# PTY/parser package. Structural crate separation makes Rust import aliases
# irrelevant: forbidden modules are not in this crate's dependency graph.
renderer_tree=$(cargo tree --manifest-path "$manifest" --locked \
  --package mandatum-gpu-renderer-spike --edges normal --prefix none)
if ! printf '%s\n' "$renderer_tree" | grep -q '^mandatum-scene '; then
  echo "gpu-spike: isolated renderer lost the mandatum-scene contract"
  exit 1
fi
if ! printf '%s\n' "$renderer_tree" | grep -q '^wgpu '; then
  echo "gpu-spike: isolated renderer lost its GPU dependency"
  exit 1
fi
if printf '%s\n' "$renderer_tree" \
  | grep -Eq '^(mandatum-pty|mandatum-terminal-vt|portable-pty|vte) '; then
  echo "gpu-spike: isolated renderer dependency tree crossed into PTY/parser code"
  exit 1
fi

echo "gpu-spike: deferred adapter compiles, tests, and holds the scene boundary"
