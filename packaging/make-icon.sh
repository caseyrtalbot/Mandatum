#!/usr/bin/env bash
# Regenerates packaging/Mandatum.icns from make-icon.swift.
# Run on macOS; requires swiftc, sips, and iconutil (Xcode Command Line Tools).
set -euo pipefail
cd "$(dirname "$0")"

work=$(mktemp -d "${TMPDIR:-/tmp}/mandatum-icon.XXXXXX")
trap 'rm -rf "$work"' EXIT

swiftc -O -o "$work/make-icon" make-icon.swift
"$work/make-icon" "$work/icon-1024.png"

iconset="$work/Mandatum.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$work/icon-1024.png" \
        --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$work/icon-1024.png" \
        --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done

iconutil -c icns "$iconset" -o Mandatum.icns
echo "wrote packaging/Mandatum.icns"
