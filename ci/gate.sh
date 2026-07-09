#!/usr/bin/env bash
# Mandatum CI gate — single source of truth for local and remote CI.
# Every check here is a merge gate: red means the change does not land.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }

step "format"
cargo fmt --all --check

step "clippy (-D warnings)"
cargo clippy --workspace --all-targets -- -D warnings

step "build"
cargo build --workspace --all-targets

step "test"
cargo test --workspace

step "conformance (Constitution L1/L2 dependency laws)"
./ci/conformance.sh

step "doc-trace (every law has docs + an executable gate)"
./ci/doc-trace.sh

printf '\n\033[1;32mGATE GREEN\033[0m\n'
