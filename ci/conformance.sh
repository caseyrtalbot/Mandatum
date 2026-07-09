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
ENGINE_SIDE = ["mandatum-core", "mandatum-commands", "mandatum-scene"]

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

if failures:
    print("CONFORMANCE FAILURES:")
    for f in failures:
        print("  -", f)
    sys.exit(1)

print("conformance: L1/L2 dependency laws hold")
PY
