#!/usr/bin/env bash
# Distribution contract smoke: install.sh installs Mandatum.app and the
# mandatum launcher from a checksummed release zip; reinstalls are
# idempotent; downgrades are refused; a failed swap restores the
# previous app; and `mandatum update` reaches the same installer.
set -euo pipefail
cd "$(dirname "$0")/.."
repo_root=$(pwd)

scratch=$(mktemp -d "${TMPDIR:-/tmp}/mandatum-distribution.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

fixtures="$scratch/fixtures"
mock_bin="$scratch/mock-bin"
app_dir="$scratch/apps"
bin_dir="$scratch/bin"
mkdir -p "$fixtures" "$mock_bin" "$app_dir" "$bin_dir"

# Fixture app bundles are produced by the real packaging script from
# tiny compiled binaries, so the smoke exercises the shipped layout.
make_fixture() {
  version=$1
  workdir="$scratch/build-$version"
  mkdir -p "$workdir"
  printf '#include <stdio.h>\nint main(void){puts("fixture %s");return 0;}\n' \
    "$version" >"$workdir/main.c"
  cc -o "$workdir/binary" "$workdir/main.c"
  ./packaging/package-app.sh \
    --native "$workdir/binary" \
    --bridge "$workdir/binary" \
    --version "$version" \
    --output "$workdir" >/dev/null
  ditto -c -k --keepParent "$workdir/Mandatum.app" \
    "$fixtures/Mandatum-$version.app.zip"
}

serve_version() {
  cp "$fixtures/Mandatum-$1.app.zip" "$fixtures/Mandatum.app.zip"
  (cd "$fixtures" && shasum -a 256 Mandatum.app.zip >Mandatum.app.zip.sha256)
}

make_fixture 0.1.0
make_fixture 0.1.1

# Mock curl serves release assets and the hosted installer from fixtures,
# supporting both --output and stdout modes.
cat >"$mock_bin/curl" <<'MOCK'
#!/bin/sh
set -eu
destination=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) destination=$2; shift 2 ;;
    https://*) url=$1; shift ;;
    *) shift ;;
  esac
done
test -n "$url"
case "$url" in
  */install.sh) source_file="$MANDATUM_TEST_REPO/install.sh" ;;
  *) source_file="$MANDATUM_TEST_FIXTURES/${url##*/}" ;;
esac
test -f "$source_file"
if [ -n "$destination" ]; then
  cp "$source_file" "$destination"
else
  cat "$source_file"
fi
MOCK
chmod 0755 "$mock_bin/curl"

# Injectable mv failure for the final app swap.
cat >"$mock_bin/mv" <<'MOCK'
#!/bin/sh
last_argument=""
for argument in "$@"; do last_argument=$argument; done
if [ "${MANDATUM_TEST_FAIL_APP_SWAP:-0}" = 1 ]; then
  case "$1" in
    */.mandatum-stage.*/Mandatum.app)
      case "$last_argument" in
        */Mandatum.app) exit 71 ;;
      esac
      ;;
  esac
fi
exec /bin/mv "$@"
MOCK
chmod 0755 "$mock_bin/mv"

run_installer() {
  env \
    PATH="$mock_bin:$PATH" \
    MANDATUM_APP_DIR="$app_dir" \
    MANDATUM_INSTALL_DIR="$bin_dir" \
    MANDATUM_TEST_FIXTURES="$fixtures" \
    MANDATUM_TEST_REPO="$repo_root" \
    "$@" \
    sh install.sh
}

installed_version() {
  plutil -extract CFBundleShortVersionString raw -o - \
    "$app_dir/Mandatum.app/Contents/Info.plist"
}

# 1. Fresh install places the app, the launcher, and reports the version.
serve_version 0.1.0
run_installer >"$scratch/fresh.stdout"
test -x "$app_dir/Mandatum.app/Contents/MacOS/Mandatum"
test -x "$bin_dir/mandatum"
test "$(installed_version)" = "0.1.0"
test "$("$app_dir/Mandatum.app/Contents/MacOS/Mandatum")" = "fixture 0.1.0"
test "$(env MANDATUM_APP="$app_dir/Mandatum.app" "$bin_dir/mandatum" --version)" = \
  "mandatum 0.1.0"

# 2. Reinstalling the same version is a no-op.
run_installer >"$scratch/current.stdout"
grep -q "already the latest release" "$scratch/current.stdout"
test "$(installed_version)" = "0.1.0"

# 3. `mandatum update` reaches the hosted installer and applies the
#    newer release.
serve_version 0.1.1
env \
  PATH="$mock_bin:$PATH" \
  MANDATUM_APP="$app_dir/Mandatum.app" \
  MANDATUM_INSTALL_DIR="$bin_dir" \
  MANDATUM_TEST_FIXTURES="$fixtures" \
  MANDATUM_TEST_REPO="$repo_root" \
  "$bin_dir/mandatum" update >"$scratch/update.stdout"
test "$(installed_version)" = "0.1.1"
test "$(env MANDATUM_APP="$app_dir/Mandatum.app" "$bin_dir/mandatum" --version)" = \
  "mandatum 0.1.1"

# 4. Downgrades are refused.
serve_version 0.1.0
if run_installer >"$scratch/downgrade.stdout" 2>"$scratch/downgrade.stderr"; then
  echo "distribution smoke: downgrade unexpectedly succeeded" >&2
  exit 1
fi
grep -q "older than installed" "$scratch/downgrade.stderr"
test "$(installed_version)" = "0.1.1"

# 5. A failed app swap restores the previous installation.
make_fixture 0.1.2
serve_version 0.1.2
if run_installer MANDATUM_TEST_FAIL_APP_SWAP=1 \
  >"$scratch/swapfail.stdout" 2>"$scratch/swapfail.stderr"; then
  echo "distribution smoke: injected swap failure unexpectedly succeeded" >&2
  exit 1
fi
test "$(installed_version)" = "0.1.1"
test "$("$app_dir/Mandatum.app/Contents/MacOS/Mandatum")" = "fixture 0.1.1"
if find "$app_dir" -maxdepth 1 -name '.mandatum-*' | grep -q .; then
  echo "distribution smoke: swap failure left staging debris" >&2
  exit 1
fi

echo "distribution smoke: app install, update, downgrade refusal, and swap rollback hold"
