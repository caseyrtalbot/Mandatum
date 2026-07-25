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

mandatum_metadata_path=$(mktemp "${TMPDIR:-/tmp}/mandatum-conformance.XXXXXX.json")
trap 'rm -f "$mandatum_metadata_path"' EXIT
cargo metadata --format-version 1 --locked --all-features >"$mandatum_metadata_path"
export MANDATUM_CONFORMANCE_METADATA="$mandatum_metadata_path"

python3 - <<'PY'
import json, os, re, sys

with open(os.environ["MANDATUM_CONFORMANCE_METADATA"]) as metadata_file:
    meta = json.load(metadata_file)
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

# ---- production native frontend dependency boundary ----------------------
# GPU/window dependencies belong only to the two production native frontend
# packages. Every other workspace member is checked fail-closed against this
# known-stack tripwire over its transitive normal dependency closure.
NATIVE_FRONTEND_PACKAGES = {"mandatum-native", "mandatum-native-renderer"}
GPU_WINDOW_DEPS = {
    "winit", "wgpu", "glyphon", "cosmic-text", "raw-window-handle",
    "vello", "skia-safe", "metal", "ash", "glow", "glium",
    "vulkano", "vulkano-shaders",
}

workspace_names = {packages[pkg_id]["name"] for pkg_id in meta["workspace_members"]}
missing_native_packages = NATIVE_FRONTEND_PACKAGES - workspace_names
if missing_native_packages:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] production native package allowlist "
        f"references missing workspace members: {sorted(missing_native_packages)}"
    )

closure_by_name = {
    packages[pkg_id]["name"]: transitive_normal_deps(pkg_id)
    for pkg_id in meta["workspace_members"]
}

def native_boundary_failures(closures):
    found = []
    for name, closure in sorted(closures.items()):
        if name in NATIVE_FRONTEND_PACKAGES:
            continue
        hit = closure & GPU_WINDOW_DEPS
        if hit:
            found.append(
                f"[NATIVE-DEPENDENCY-BOUNDARY] {name} transitively depends on "
                f"native-only GPU/window crates: {sorted(hit)}. Only "
                f"{sorted(NATIVE_FRONTEND_PACKAGES)} may reach this stack."
            )
    return found

failures.extend(native_boundary_failures(closure_by_name))

# Executable negative tests: model a forbidden wgpu edge in every non-native
# production crate and prove the boundary checker rejects each one.
for name in sorted(workspace_names - NATIVE_FRONTEND_PACKAGES):
    modeled = {pkg: set(closure) for pkg, closure in closure_by_name.items()}
    modeled[name].add("wgpu")
    expected = f"[NATIVE-DEPENDENCY-BOUNDARY] {name} transitively depends"
    if not any(expected in failure for failure in native_boundary_failures(modeled)):
        failures.append(
            "[NATIVE-DEPENDENCY-BOUNDARY] negative self-test failed to reject "
            f"a modeled {name} -> wgpu edge"
        )

# Keep release/install surfaces on an explicit shipping contract: releases
# build only allowlisted binaries, package them into a single Mandatum.app
# archive, and the installer keeps the user flow free of developer
# machinery (no cargo, no certificates, no bridge knowledge).
ALLOWED_RELEASE_TARGETS = {
    ("mandatum-agent-runtime", "mandatum-approval-bridge"),
    ("mandatum-native", "mandatum-native"),
}
APP_BUNDLE_EXECUTABLES = {"Mandatum", "mandatum-approval-bridge"}
release_path = ".github/workflows/release.yml"
install_path = "install.sh"
package_path = "packaging/package-app.sh"
release_text = open(release_path).read()
install_text = open(install_path).read()
package_text = open(package_path).read()
shipping_surfaces = (
    (release_path, release_text),
    (install_path, install_text),
    (package_path, package_text),
)
for forbidden_ref in ("spikes/frontend-wgpu", "frontend-wgpu", "mandatum-frontend-wgpu-spike"):
    for path, source in shipping_surfaces:
        if forbidden_ref in source:
            failures.append(
                f"[NATIVE-DEPENDENCY-BOUNDARY] shipping surface {path} "
                f"references lab-only token {forbidden_ref!r}"
            )

release_targets = set()
for line in release_text.splitlines():
    if "cargo build" not in line:
        continue
    package = re.search(r"(?:^|\s)-p\s+([A-Za-z0-9_-]+)", line)
    binary = re.search(r"(?:^|\s)--bin\s+([A-Za-z0-9_-]+)", line)
    if "--manifest-path" in line or "--workspace" in line or not package or not binary:
        failures.append(
            "[NATIVE-DEPENDENCY-BOUNDARY] release build is not an allowlisted "
            "package/bin pair: "
            f"{line.strip()}"
        )
        continue
    release_targets.add((package.group(1), binary.group(1)))

if release_targets != ALLOWED_RELEASE_TARGETS:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] release targets changed: "
        f"{sorted(release_targets)} "
        f"(allowed: {sorted(ALLOWED_RELEASE_TARGETS)})"
    )

# The release publishes exactly the app archive produced by the packaging
# script, plus its checksum.
for required_release_ref in (
    "packaging/package-app.sh",
    "Mandatum.app.zip",
    "ditto -c -k",
):
    if required_release_ref not in release_text:
        failures.append(
            "[NATIVE-DEPENDENCY-BOUNDARY] release no longer ships the packaged "
            f"app archive (missing {required_release_ref!r})"
        )

# The packaging script installs exactly the allowlisted executables into
# Contents/MacOS, and the launcher rides in Contents/Resources.
bundle_executables = set(re.findall(r'Contents/MacOS/([A-Za-z][A-Za-z-]*)"', package_text))
if bundle_executables != APP_BUNDLE_EXECUTABLES:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] app bundle executables changed: "
        f"{sorted(bundle_executables)} (allowed: {sorted(APP_BUNDLE_EXECUTABLES)})"
    )
if 'Contents/Resources/mandatum"' not in package_text:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] the app bundle no longer carries the "
        "mandatum launcher at Contents/Resources/mandatum"
    )

# Installer contract: one asset, checksum verified before extraction, no
# developer machinery in the user flow, downgrades refused, and the
# command that owns `mandatum update` installed last.
if 'ASSET_NAME="Mandatum.app.zip"' not in install_text:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] installer no longer installs the "
        "Mandatum.app.zip release asset"
    )
for developer_token in ("cargo", "rustup", "Developer ID", "mandatum-approval-bridge"):
    if developer_token in install_text:
        failures.append(
            "[NATIVE-DEPENDENCY-BOUNDARY] installer leaks developer machinery "
            f"into the user flow: {developer_token!r}"
        )
try:
    checksum_index = install_text.index("shasum -a 256 -c")
    extract_index = install_text.index("ditto -x -k")
    if checksum_index > extract_index:
        failures.append(
            "[NATIVE-DEPENDENCY-BOUNDARY] installer must verify the checksum "
            "before extracting the archive"
        )
except ValueError:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] installer lost checksum verification "
        "or archive extraction"
    )
if "version_is_older" not in install_text:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] installer lost its downgrade refusal"
    )
if 'mv "${stage_dir}/Mandatum.app"' not in install_text \
        or "install_launcher" not in install_text \
        or install_text.index('mv "${stage_dir}/Mandatum.app"') \
        > install_text.rindex("install_launcher"):
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] self-update owner must be installed "
        "after the app swap"
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

native_renderer = by_name.get("mandatum-native-renderer")
if native_renderer is not None:
    renderer_internal_deps = {
        dependency["name"]
        for dependency in native_renderer["dependencies"]
        if dependency.get("kind") in (None, "normal")
        and dependency["name"].startswith("mandatum-")
    }
    allowed_renderer_internal_deps = {"mandatum-scene"}
    if renderer_internal_deps != allowed_renderer_internal_deps:
        failures.append(
            "[NATIVE-DEPENDENCY-BOUNDARY] mandatum-native-renderer internal "
            f"dependency set changed: {sorted(renderer_internal_deps)} "
            f"(allowed: {sorted(allowed_renderer_internal_deps)}). The native "
            "renderer consumes the scene contract, not app/runtime internals."
        )

def native_shell_boundary_failures(internal_deps):
    allowed = {
        "mandatum-app",
        "mandatum-native-renderer",
        "mandatum-scene",
    }
    if internal_deps == allowed:
        return []
    return [
        "[NATIVE-DEPENDENCY-BOUNDARY] mandatum-native internal dependency "
        f"set changed: {sorted(internal_deps)} (allowed: {sorted(allowed)}). "
        "The native shell reaches product state only through FrontendHost."
    ]

native_shell = by_name.get("mandatum-native")
if native_shell is not None:
    native_shell_internal_deps = {
        dependency["name"]
        for dependency in native_shell["dependencies"]
        if dependency.get("kind") in (None, "normal")
        and dependency["name"].startswith("mandatum-")
    }
    failures.extend(native_shell_boundary_failures(native_shell_internal_deps))
    modeled_native_shell_deps = native_shell_internal_deps | {"mandatum-pty"}
    if not native_shell_boundary_failures(modeled_native_shell_deps):
        failures.append(
            "[NATIVE-DEPENDENCY-BOUNDARY] negative self-test failed to reject "
            "a modeled mandatum-native -> mandatum-pty edge"
        )

if failures:
    print("CONFORMANCE FAILURES:")
    for f in failures:
        print("  -", f)
    sys.exit(1)

print(
    "conformance: L1/L2 laws and native frontend dependency boundary hold; "
    f"negative edge models rejected for "
    f"{len(workspace_names - NATIVE_FRONTEND_PACKAGES)} non-native production crates"
)
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
