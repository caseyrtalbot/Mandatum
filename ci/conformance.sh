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

cargo metadata --format-version 1 --locked --all-features >/tmp/mandatum-metadata.json

python3 - <<'PY'
import json, re, sys

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

# ---- fail-closed production GPU admission --------------------------------
# A GPU frontend remains an isolated spike until a production-admission
# decision is backed by either a typed pixel-native scene surface plus adapter
# tests, or a sub-20 ms key-to-present product target plus symmetric end-to-end
# evidence. Selecting a product-trigger branch alone is not admission.
# Windowing alone is not GPU admission, so winit is deliberately absent. This
# known-stack list is a tripwire, not a claim to enumerate every GPU library.
GPU_FRONTEND_DEPS = {
    "wgpu", "glyphon", "vello", "skia-safe", "metal", "ash", "glow", "glium",
    "vulkano", "vulkano-shaders",
}
for pkg_id in meta["workspace_members"]:
    pkg = packages[pkg_id]
    closure = transitive_normal_deps(pkg_id)
    hit = closure & GPU_FRONTEND_DEPS
    if hit:
        failures.append(
            f"[GPU-ADMISSION-GATE] {pkg['name']} transitively depends on GPU "
            f"frontend crates: {sorted(hit)}. Keep listed GPU dependencies spike-only "
            "until a production-admission decision proves a typed pixel-native "
            "scene surface with executable adapter tests, or a sub-20 ms "
            "key-to-present product target with symmetric end-to-end evidence."
        )

# An excluded manifest is intentionally absent from workspace metadata. Keep
# the release/install surfaces on an explicit product-artifact allowlist so an
# excluded GPU spike cannot bypass admission by being built and packaged
# directly.
ALLOWED_RELEASE_TARGETS = {
    ("mandatum-app", "mandatum"),
    ("mandatum-agent-runtime", "mandatum-approval-bridge"),
}
ALLOWED_RELEASE_MEMBERS = {"LICENSE", "mandatum", "mandatum-approval-bridge"}
ALLOWED_RELEASE_BINARIES = {"mandatum", "mandatum-approval-bridge"}
release_path = ".github/workflows/release.yml"
install_path = "install.sh"
release_text = open(release_path).read()
install_text = open(install_path).read()
for forbidden_ref in ("spikes/frontend-wgpu", "frontend-wgpu", "mandatum-frontend-wgpu-spike"):
    for path, source in ((release_path, release_text), (install_path, install_text)):
        if forbidden_ref in source:
            failures.append(
                f"[GPU-ADMISSION-GATE] shipping surface {path} references excluded "
                f"GPU spike token {forbidden_ref!r}"
            )

release_targets = set()
for line in release_text.splitlines():
    if "cargo build" not in line:
        continue
    package = re.search(r"(?:^|\s)-p\s+([A-Za-z0-9_-]+)", line)
    binary = re.search(r"(?:^|\s)--bin\s+([A-Za-z0-9_-]+)", line)
    if "--manifest-path" in line or "--workspace" in line or not package or not binary:
        failures.append(
            f"[GPU-ADMISSION-GATE] release build is not an allowlisted package/bin pair: "
            f"{line.strip()}"
        )
        continue
    release_targets.add((package.group(1), binary.group(1)))

if release_targets != ALLOWED_RELEASE_TARGETS:
    failures.append(
        f"[GPU-ADMISSION-GATE] release targets changed: {sorted(release_targets)} "
        f"(allowed: {sorted(ALLOWED_RELEASE_TARGETS)})"
    )

def printf_member_set(source, variable):
    match = re.search(rf"{variable}=\$\(printf '%s\\n' ([^|]+)\|", source)
    return set(match.group(1).split()) if match else set()

release_members = printf_member_set(release_text, "expected")
installer_members = printf_member_set(install_text, "expected_members")
if release_members != ALLOWED_RELEASE_MEMBERS:
    failures.append(
        f"[GPU-ADMISSION-GATE] release archive members changed: "
        f"{sorted(release_members)} (allowed: {sorted(ALLOWED_RELEASE_MEMBERS)})"
    )
if installer_members != ALLOWED_RELEASE_MEMBERS:
    failures.append(
        f"[GPU-ADMISSION-GATE] installer archive members changed: "
        f"{sorted(installer_members)} (allowed: {sorted(ALLOWED_RELEASE_MEMBERS)})"
    )
if 'test "$actual" = "$expected"' not in release_text:
    failures.append("[GPU-ADMISSION-GATE] release archive allowlist assertion is missing")
if '[ "$archive_members" = "$expected_members" ]' not in install_text:
    failures.append("[GPU-ADMISSION-GATE] installer archive allowlist assertion is missing")

release_stage_sources = set(
    re.findall(r"install -m 0755 target/release/([A-Za-z0-9_-]+)", release_text)
)
installer_stage_sources = set(
    re.findall(r'install -m 0755 "\$\{extract_dir\}/([A-Za-z0-9_-]+)"', install_text)
)
installer_binary_loops = [
    set(match.split())
    for match in re.findall(r"for binary in ([^;]+); do", install_text)
]
if release_stage_sources != ALLOWED_RELEASE_BINARIES:
    failures.append(
        f"[GPU-ADMISSION-GATE] release staging binaries changed: "
        f"{sorted(release_stage_sources)} (allowed: {sorted(ALLOWED_RELEASE_BINARIES)})"
    )
if installer_stage_sources != ALLOWED_RELEASE_BINARIES:
    failures.append(
        f"[GPU-ADMISSION-GATE] installer staging binaries changed: "
        f"{sorted(installer_stage_sources)} (allowed: {sorted(ALLOWED_RELEASE_BINARIES)})"
    )
if len(installer_binary_loops) != 2 or any(
    binaries != ALLOWED_RELEASE_BINARIES for binaries in installer_binary_loops
):
    failures.append(
        f"[GPU-ADMISSION-GATE] installer binary loops changed: "
        f"{[sorted(binaries) for binaries in installer_binary_loops]} "
        f"(allowed twice: {sorted(ALLOWED_RELEASE_BINARIES)})"
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

print("conformance: L1/L2 dependency laws and GPU admission policy hold")
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
