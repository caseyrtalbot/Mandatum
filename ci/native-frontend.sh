#!/usr/bin/env bash
# Production native frontend gate. This is invoked by ci/gate.sh so the
# workspace retains one local/remote CI authority.
set -euo pipefail
cd "$(dirname "$0")/.."

native_package="mandatum-native"
renderer_package="mandatum-native-renderer"
lab_manifest="spikes/frontend-wgpu/Cargo.toml"

cargo fmt \
  --package "$native_package" \
  --package "$renderer_package" \
  -- --check
cargo clippy --locked \
  --package "$native_package" \
  --package "$renderer_package" \
  --all-targets -- -D warnings
cargo build --locked \
  --package "$native_package" \
  --package "$renderer_package" \
  --all-targets
cargo test --locked \
  --package "$native_package" \
  --package "$renderer_package" \
  --all-targets
cargo test --locked \
  --package "$renderer_package" \
  --features fault-injection \
  --all-targets

# The separate lab harness retains measurement, stress, fault, and real-host
# regression tools. Production shell behavior is covered in the package above;
# these tests do not substitute for the product package or live smoke.
cargo fmt --manifest-path "$lab_manifest" -- --check
cargo clippy --manifest-path "$lab_manifest" --locked \
  --all-targets -- -D warnings
cargo test --manifest-path "$lab_manifest" --locked --all-targets

# The renderer consumes the scene contract and GPU/window stack without
# reaching into the app, PTY, parser, or terminal renderer.
renderer_tree=$(cargo tree --locked \
  --package "$renderer_package" --edges normal --prefix none)
if ! printf '%s\n' "$renderer_tree" | grep -q '^mandatum-scene '; then
  echo "native-frontend: renderer lost the mandatum-scene contract"
  exit 1
fi
for required_dep in glyphon wgpu winit; do
  if ! printf '%s\n' "$renderer_tree" | grep -q "^${required_dep} "; then
    echo "native-frontend: renderer lost its ${required_dep} dependency"
    exit 1
  fi
done
if printf '%s\n' "$renderer_tree" \
  | grep -Eq '^(mandatum-app|mandatum-pty|mandatum-renderer|mandatum-terminal-vt|portable-pty|ratatui|vte) '; then
  echo "native-frontend: renderer dependency tree crossed the scene-only boundary"
  exit 1
fi

# Fault-injection remains lab tooling. The production native command's default
# feature closure must never enable it, even indirectly through the renderer.
native_feature_tree=$(cargo tree --locked \
  --package "$native_package" --edges normal --prefix none \
  --format '{p} features=[{f}]')
if printf '%s\n' "$native_feature_tree" \
  | grep -Eiq 'features=\[[^]]*fault[-_]injection'; then
  echo "native-frontend: production default feature closure enables fault injection"
  exit 1
fi

echo "native-frontend: product and lab packages pass; production boundaries hold"
