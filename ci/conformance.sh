#!/usr/bin/env bash
# [L1-GATE] [L2-GATE] Dependency-boundary conformance.
#
# L2: mandatum-core is a runtime-free leaf. Its direct dependency set must be
#     exactly {serde, serde_json}. This gate fails if the set grows or shrinks.
# L1: engine/frontend separation. Frontend, parser, process, and async-runtime
#     crates must never appear in the transitive dependency closure of the
#     engine-side crates listed below.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo metadata --format-version 1 --locked >/tmp/mandatum-metadata.json

python3 - <<'PY'
import json, sys

meta = json.load(open("/tmp/mandatum-metadata.json"))
packages = {p["id"]: p for p in meta["packages"]}
by_name = {p["name"]: p for p in meta["packages"]}
resolve = {n["id"]: n for n in meta["resolve"]["nodes"]}

failures = []

# ---- L2: core's direct dependency set is frozen -------------------------
ALLOWED_CORE_DEPS = {"serde", "serde_json"}
core = by_name["mandatum-core"]
core_deps = {d["name"] for d in core["dependencies"]}
if core_deps != ALLOWED_CORE_DEPS:
    failures.append(
        f"[L2] mandatum-core dependency set changed: {sorted(core_deps)} "
        f"(allowed: {sorted(ALLOWED_CORE_DEPS)}). core is a runtime-free leaf; "
        "if a feature needs more here, the boundary is wrong, not the law."
    )

# ---- L1: frontend/runtime crates never reach engine-side crates ---------
FORBIDDEN = {
    "ratatui", "crossterm", "vte", "portable-pty", "tokio", "async-std",
    "winit", "wgpu", "smol", "mio",
}
# Engine-side crates that must stay frontend/runtime-free. Grows as the
# workspace grows; scene crates belong here the day they exist.
ENGINE_SIDE = [
    "mandatum-core",
    "mandatum-commands",
    "mandatum-scene",
    "mandatum-agent-runtime",
]

def transitive_normal_deps(pkg_id):
    seen, stack = set(), [pkg_id]
    while stack:
        node = resolve.get(stack.pop())
        if node is None:
            continue
        for dep in node["deps"]:
            kinds = {k["kind"] for k in dep["dep_kinds"]}
            if None in kinds or "normal" in {k or "normal" for k in kinds}:
                if dep["pkg"] not in seen:
                    seen.add(dep["pkg"])
                    stack.append(dep["pkg"])
    return {packages[i]["name"] for i in seen}

for name in ENGINE_SIDE:
    pkg = by_name.get(name)
    if pkg is None:
        continue  # crate not created yet
    closure = transitive_normal_deps(pkg["id"])
    hit = closure & FORBIDDEN
    if hit:
        failures.append(
            f"[L1] {name} transitively depends on forbidden crates: {sorted(hit)}"
        )

# ---- [L1-GATE] direct-dependency bans across the render seam ------------
# Frontend adapters consume the scene contract only. The ratatui renderer
# must never reach the terminal engine directly; the app converts engine
# grids into scene surfaces.
DIRECT_DEP_BANS = {
    "mandatum-renderer": {"mandatum-terminal-vt"},
}
for name, banned in DIRECT_DEP_BANS.items():
    pkg = by_name.get(name)
    if pkg is None:
        continue
    direct = {d["name"] for d in pkg["dependencies"]}
    hit = direct & banned
    if hit:
        failures.append(
            f"[L1] {name} directly depends on banned crates: {sorted(hit)}. "
            "Frontends render scenes; the scene builder in the app owns the "
            "engine-to-scene conversion."
        )

if failures:
    print("CONFORMANCE FAILURES:")
    for f in failures:
        print("  -", f)
    sys.exit(1)

print("conformance: L1/L2 dependency laws hold")
PY

# ---- [L1-GATE] module-level input seam inside the app crate --------------
# crossterm is a frontend concern. Cargo can only express dependency bans at
# crate granularity, and the app crate legitimately hosts the terminal
# frontend, so this seam is enforced as a source scan: inside crates/app
# only the frontend modules (app_shell.rs, frontend.rs) may use crossterm
# (imports or paths; prose in comments is fine). app_state and all dispatch
# logic consume mandatum_scene::input values only.
seam_violations=$(grep -rlE 'use crossterm|crossterm::' crates/app/src crates/app/tests \
  | grep -Ev '^crates/app/src/(app_shell|frontend)\.rs$' || true)
if [ -n "$seam_violations" ]; then
  echo "CONFORMANCE FAILURES:"
  echo "  - [L1] crossterm named outside the frontend modules:"
  echo "$seam_violations" | sed 's/^/      /'
  exit 1
fi
echo "conformance: app-crate input seam holds (crossterm only in frontend modules)"
