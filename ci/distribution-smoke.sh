#!/usr/bin/env bash
# Focused release-installer contract: macOS consumes a backward-compatible
# common archive plus a separate native archive, and a failed multi-binary
# replacement restores the prior installation.
set -euo pipefail
cd "$(dirname "$0")/.."

scratch=$(mktemp -d "${TMPDIR:-/tmp}/mandatum-distribution.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

fixtures="$scratch/fixtures"
common_stage="$scratch/common"
native_stage="$scratch/native"
mock_bin="$scratch/mock-bin"
install_dir="$scratch/install"
mkdir -p "$fixtures" "$common_stage" "$native_stage" "$mock_bin" "$install_dir"

make_binary() {
  path=$1
  output=$2
  printf '#!/bin/sh\nprintf "%s\\n"\n' "$output" >"$path"
  chmod 0755 "$path"
}

make_checksum() {
  archive=$1
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$fixtures" && sha256sum "$archive" >"${archive}.sha256")
  else
    (cd "$fixtures" && shasum -a 256 "$archive" >"${archive}.sha256")
  fi
}

make_binary "$common_stage/mandatum" "mandatum 0.2.0"
make_binary "$common_stage/mandatum-approval-bridge" "new bridge"
install -m 0644 LICENSE "$common_stage/LICENSE"
tar -C "$common_stage" -czf \
  "$fixtures/mandatum-aarch64-apple-darwin.tar.gz" \
  mandatum mandatum-approval-bridge LICENSE
make_checksum "mandatum-aarch64-apple-darwin.tar.gz"

make_binary "$native_stage/mandatum-native" "new native"
install -m 0644 LICENSE "$native_stage/LICENSE"
tar -C "$native_stage" -czf \
  "$fixtures/mandatum-native-aarch64-apple-darwin.tar.gz" \
  mandatum-native LICENSE
make_checksum "mandatum-native-aarch64-apple-darwin.tar.gz"

printf '#!/bin/sh\ncase "$1" in -s) echo Darwin ;; -m) echo arm64 ;; *) exit 2 ;; esac\n' \
  >"$mock_bin/uname"
chmod 0755 "$mock_bin/uname"

printf '%s\n' \
  '#!/bin/sh' \
  'set -eu' \
  'destination=""' \
  'url=""' \
  'head_request=0' \
  'write_out=""' \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in' \
  '    --output) destination=$2; shift 2 ;;' \
  '    --write-out) write_out=$2; shift 2 ;;' \
  '    --head) head_request=1; shift ;;' \
  '    https://*) url=$1; shift ;;' \
  '    *) shift ;;' \
  '  esac' \
  'done' \
  'test -n "$url"' \
  'if [ "$head_request" -eq 1 ]; then' \
  '  if [ "${MANDATUM_TEST_NATIVE_MISSING:-0}" = 1 ] &&' \
  '     [ "${url##*/}" = "mandatum-native-aarch64-apple-darwin.tar.gz" ]; then' \
  '    printf 404' \
  '  else' \
  '    printf 200' \
  '  fi' \
  '  exit 0' \
  'fi' \
  'test -n "$destination"' \
  'cp "$MANDATUM_TEST_FIXTURES/${url##*/}" "$destination"' \
  >"$mock_bin/curl"
chmod 0755 "$mock_bin/curl"

printf '%s\n' \
  '#!/bin/sh' \
  'case "$1" in' \
  '  --verify) exit 0 ;;' \
  '  --display)' \
  '    echo "Authority=Developer ID Application: Mandatum Test (3S2Y6XKV4P)" >&2' \
  '    echo "TeamIdentifier=3S2Y6XKV4P" >&2' \
  '    exit 0' \
  '    ;;' \
  'esac' \
  'exit 2' \
  >"$mock_bin/codesign"
chmod 0755 "$mock_bin/codesign"

printf '%s\n' \
  '#!/bin/sh' \
  'set -eu' \
  'source_argument=${2:-}' \
  'last_argument=""' \
  'for argument in "$@"; do last_argument=$argument; done' \
  'if [ "${MANDATUM_TEST_FAIL_NATIVE_MOVE:-0}" = 1 ] &&' \
  '   [ "$last_argument" = "$MANDATUM_INSTALL_DIR/mandatum-native" ] &&' \
  '   [ ! -e "$MANDATUM_TEST_MV_STATE" ]; then' \
  '  : >"$MANDATUM_TEST_MV_STATE"' \
  '  exit 71' \
  'fi' \
  'if [ "${MANDATUM_TEST_FAIL_BRIDGE_RESTORE:-0}" = 1 ] &&' \
  '   [ -e "$MANDATUM_TEST_MV_STATE" ] &&' \
  '   [ "$last_argument" = "$MANDATUM_INSTALL_DIR/mandatum-approval-bridge" ]; then' \
  '  case "$source_argument" in' \
  '    */.mandatum-backup.*/mandatum-approval-bridge) exit 72 ;;' \
  '  esac' \
  'fi' \
  'exec /bin/mv "$@"' \
  >"$mock_bin/mv"
chmod 0755 "$mock_bin/mv"

env \
  PATH="$mock_bin:$PATH" \
  MANDATUM_INSTALL_DIR="$install_dir" \
  MANDATUM_TEST_FIXTURES="$fixtures" \
  sh install.sh >"$scratch/install.stdout"

test "$("$install_dir/mandatum")" = "mandatum 0.2.0"
test "$("$install_dir/mandatum-approval-bridge")" = "new bridge"
test "$("$install_dir/mandatum-native")" = "new native"

make_binary "$install_dir/mandatum" "old mandatum"
make_binary "$install_dir/mandatum-approval-bridge" "old bridge"
make_binary "$install_dir/mandatum-native" "old native"

if env \
  PATH="$mock_bin:$PATH" \
  MANDATUM_INSTALL_DIR="$install_dir" \
  MANDATUM_TEST_FIXTURES="$fixtures" \
  MANDATUM_TEST_FAIL_NATIVE_MOVE=1 \
  MANDATUM_TEST_MV_STATE="$scratch/mv-failed" \
  sh install.sh >"$scratch/rollback.stdout" 2>"$scratch/rollback.stderr"
then
  echo "distribution smoke: injected native install failure unexpectedly succeeded" >&2
  exit 1
fi

test "$("$install_dir/mandatum")" = "old mandatum"
test "$("$install_dir/mandatum-approval-bridge")" = "old bridge"
test "$("$install_dir/mandatum-native")" = "old native"

recovery_dir="$scratch/recovery"
mkdir "$recovery_dir"
make_binary "$recovery_dir/mandatum" "recovery old mandatum"
make_binary "$recovery_dir/mandatum-approval-bridge" "recovery old bridge"
make_binary "$recovery_dir/mandatum-native" "recovery old native"

if env \
  PATH="$mock_bin:$PATH" \
  MANDATUM_INSTALL_DIR="$recovery_dir" \
  MANDATUM_TEST_FIXTURES="$fixtures" \
  MANDATUM_TEST_FAIL_NATIVE_MOVE=1 \
  MANDATUM_TEST_FAIL_BRIDGE_RESTORE=1 \
  MANDATUM_TEST_MV_STATE="$scratch/recovery-mv-failed" \
  sh install.sh >"$scratch/recovery.stdout" 2>"$scratch/recovery.stderr"
then
  echo "distribution smoke: injected restore failure unexpectedly succeeded" >&2
  exit 1
fi

retained_backup=$(sed -n \
  's/^mandatum installer: backup retained for manual recovery: //p' \
  "$scratch/recovery.stderr")
test -n "$retained_backup"
test -d "$retained_backup"
test "$("$retained_backup/mandatum-approval-bridge")" = "recovery old bridge"

env \
  PATH="$mock_bin:$PATH" \
  MANDATUM_INSTALL_DIR="$install_dir" \
  MANDATUM_CURRENT_VERSION=0.2.0 \
  MANDATUM_TEST_FIXTURES="$fixtures" \
  sh install.sh >"$scratch/already-current.stdout"
grep -q "already the latest published release" "$scratch/already-current.stdout"
test "$("$install_dir/mandatum")" = "old mandatum"
test "$("$install_dir/mandatum-approval-bridge")" = "old bridge"
test "$("$install_dir/mandatum-native")" = "old native"

terminal_only_dir="$scratch/terminal-only"
mkdir "$terminal_only_dir"
env \
  PATH="$mock_bin:$PATH" \
  MANDATUM_INSTALL_DIR="$terminal_only_dir" \
  MANDATUM_TEST_FIXTURES="$fixtures" \
  MANDATUM_TEST_NATIVE_MISSING=1 \
  sh install.sh >"$scratch/terminal-only.stdout"
test "$("$terminal_only_dir/mandatum")" = "mandatum 0.2.0"
test "$("$terminal_only_dir/mandatum-approval-bridge")" = "new bridge"
test ! -e "$terminal_only_dir/mandatum-native"
grep -q "does not include the native macOS application yet" \
  "$scratch/terminal-only.stdout"

env \
  PATH="$mock_bin:$PATH" \
  MANDATUM_INSTALL_DIR="$terminal_only_dir" \
  MANDATUM_CURRENT_VERSION=0.2.0 \
  MANDATUM_TEST_FIXTURES="$fixtures" \
  sh install.sh >"$scratch/equal-version-migration.stdout"
test "$("$terminal_only_dir/mandatum")" = "mandatum 0.2.0"
test "$("$terminal_only_dir/mandatum-approval-bridge")" = "new bridge"
test "$("$terminal_only_dir/mandatum-native")" = "new native"

echo "distribution smoke: separate native archive installs and replacement rollback hold"
