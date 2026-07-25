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

# Keep release/install surfaces on explicit target and artifact allowlists.
# macOS ships the native product beside the terminal/recovery tool and approval
# bridge; Linux remains terminal/headless only.
ALLOWED_RELEASE_TARGETS = {
    ("mandatum-app", "mandatum"),
    ("mandatum-agent-runtime", "mandatum-approval-bridge"),
    ("mandatum-native", "mandatum-native"),
}
COMMON_RELEASE_BINARIES = {"mandatum", "mandatum-approval-bridge"}
MACOS_RELEASE_BINARIES = COMMON_RELEASE_BINARIES | {"mandatum-native"}
release_path = ".github/workflows/release.yml"
install_path = "install.sh"
release_text = open(release_path).read()
install_text = open(install_path).read()
for forbidden_ref in ("spikes/frontend-wgpu", "frontend-wgpu", "mandatum-frontend-wgpu-spike"):
    for path, source in ((release_path, release_text), (install_path, install_text)):
        if forbidden_ref in source:
            failures.append(
                f"[NATIVE-DEPENDENCY-BOUNDARY] legacy shipping surface {path} "
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

release_common_match = re.search(
    r"common_release_binaries=\(([^)]*)\)", release_text
)
release_common = (
    set(release_common_match.group(1).split()) if release_common_match else set()
)
if release_common != COMMON_RELEASE_BINARIES:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] common release binaries changed: "
        f"{sorted(release_common)} (allowed: {sorted(COMMON_RELEASE_BINARIES)})"
    )
release_macos = release_common | (
    {"mandatum-native"}
    if 'native_archive="mandatum-native-${TARGET}.tar.gz"' in release_text
    else set()
)
if release_macos != MACOS_RELEASE_BINARIES:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] macOS release binaries changed: "
        f"{sorted(release_macos)} "
        f"(allowed: {sorted(MACOS_RELEASE_BINARIES)})"
    )

installer_common_match = re.search(
    r'common_release_binaries="([^"]+)"', install_text
)
installer_macos_match = re.search(
    r'release_binaries="\$common_release_binaries ([^"]+)"', install_text
)
installer_common = (
    set(installer_common_match.group(1).split()) if installer_common_match else set()
)
installer_macos_extra = (
    set(installer_macos_match.group(1).split()) if installer_macos_match else set()
)
if installer_common != COMMON_RELEASE_BINARIES:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] common installer binaries changed: "
        f"{sorted(installer_common)} (allowed: {sorted(COMMON_RELEASE_BINARIES)})"
    )
if installer_common | installer_macos_extra != MACOS_RELEASE_BINARIES:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] macOS installer binaries changed: "
        f"{sorted(installer_common | installer_macos_extra)} "
        f"(allowed: {sorted(MACOS_RELEASE_BINARIES)})"
    )

required_release_assertions = (
    'if [[ "$RUNNER_OS" == "macOS" ]]',
    'if [[ "$TARGET" == *-apple-darwin ]]',
    'expected=$(printf \'%s\\n\' LICENSE "${common_release_binaries[@]}"',
    "native_expected=$(printf '%s\\n' LICENSE mandatum-native",
    'test "$actual" = "$expected"',
    'test "$native_actual" = "$native_expected"',
)
for assertion in required_release_assertions:
    if assertion not in release_text:
        failures.append(
            "[NATIVE-DEPENDENCY-BOUNDARY] release target/archive assertion "
            f"is missing: {assertion}"
        )
required_installer_assertions = (
    "common_expected_members=$(printf '%s\\n' LICENSE $common_release_binaries",
    "native_expected_members=$(printf '%s\\n' LICENSE mandatum-native",
    'fetch_verified_archive "$common_archive_name" "$common_expected_members"',
    'fetch_verified_archive "$native_archive_name" "$native_expected_members"',
    '[ "$release_archive_members" = "$release_expected_members" ]',
)
for assertion in required_installer_assertions:
    if assertion in install_text:
        continue
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] installer archive assertion "
        f"is missing: {assertion}"
    )
if install_text.count("for binary in $release_binaries; do") != 7:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] installer must validate members, "
        "signatures, and installation completeness, then back up, stage, and "
        "install only the selected platform binary allowlist"
    )
if '"${extract_dir}/${binary}" "${install_stage}/${binary}"' not in install_text:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] installer staging no longer follows "
        "the selected binary allowlist"
    )
if 'mv -f "${install_stage}/mandatum" "${install_dir}/mandatum"' not in install_text:
    failures.append(
        "[NATIVE-DEPENDENCY-BOUNDARY] self-update owner must be installed last"
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
