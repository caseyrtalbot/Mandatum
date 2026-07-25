#!/usr/bin/env bash
# Assembles and ad-hoc signs Mandatum.app from built binaries.
#
# Usage:
#   packaging/package-app.sh \
#     --native <mandatum-native binary> \
#     --bridge <mandatum-approval-bridge binary> \
#     --version <x.y.z> \
#     --output <directory>
#
# The bundle keeps the approval bridge beside the app executable in
# Contents/MacOS (the app resolves it as a sibling) and carries the
# command-line launcher at Contents/Resources/mandatum for install.sh.
set -euo pipefail

packaging_dir=$(cd "$(dirname "$0")" && pwd)
repo_root=$(dirname "$packaging_dir")

native="" bridge="" version="" output=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --native) native=$2; shift 2 ;;
        --bridge) bridge=$2; shift 2 ;;
        --version) version=$2; shift 2 ;;
        --output) output=$2; shift 2 ;;
        *) echo "package-app: unknown option: $1" >&2; exit 1 ;;
    esac
done

[[ -f "$native" ]] || { echo "package-app: missing native binary: $native" >&2; exit 1; }
[[ -f "$bridge" ]] || { echo "package-app: missing bridge binary: $bridge" >&2; exit 1; }
[[ -n "$output" ]] || { echo "package-app: --output is required" >&2; exit 1; }
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    || { echo "package-app: version must be x.y.z: $version" >&2; exit 1; }

app="$output/Mandatum.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

sed "s/__MANDATUM_VERSION__/$version/g" "$packaging_dir/Info.plist" \
    >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null

install -m 0755 "$native" "$app/Contents/MacOS/Mandatum"
install -m 0755 "$bridge" "$app/Contents/MacOS/mandatum-approval-bridge"
install -m 0755 "$packaging_dir/mandatum-launcher.sh" \
    "$app/Contents/Resources/mandatum"
install -m 0644 "$packaging_dir/Mandatum.icns" \
    "$app/Contents/Resources/Mandatum.icns"
install -m 0644 "$repo_root/LICENSE" "$app/Contents/Resources/LICENSE"

# Ad-hoc signatures: free, offline-verifiable, and required on Apple
# Silicon. Sign the auxiliary executable first, then the bundle.
codesign --force --sign - "$app/Contents/MacOS/mandatum-approval-bridge"
codesign --force --sign - "$app"
codesign --verify --strict "$app/Contents/MacOS/mandatum-approval-bridge"
codesign --verify --strict "$app"

echo "packaged $app (version $version)"
