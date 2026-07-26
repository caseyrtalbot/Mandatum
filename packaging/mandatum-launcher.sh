#!/bin/sh
# mandatum — command-line launcher for the Mandatum app.
# Installed to ~/.local/bin/mandatum by install.sh; the app bundle carries
# this script at Contents/Resources/mandatum so updates refresh it too.
set -eu

INSTALLER_URL="https://raw.githubusercontent.com/caseyrtalbot/Mandatum/main/install.sh"

fail() {
    printf 'mandatum: %s\n' "$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
mandatum — GPU-native development workstation

Usage:
  mandatum              open Mandatum in the current directory
  mandatum update       update Mandatum to the latest release
  mandatum --version    print the installed version
  mandatum --help       show this help

Other options (for example --font-family and --font-size) are passed
through to the app. Press F1 inside Mandatum for the full command list.
EOF
}

resolve_app() {
    for candidate in ${MANDATUM_APP:+"$MANDATUM_APP"} \
        "/Applications/Mandatum.app" \
        "${HOME}/Applications/Mandatum.app"
    do
        if [ -x "${candidate}/Contents/MacOS/Mandatum" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    fail "Mandatum.app was not found. Reinstall with: mandatum update"
}

installed_version() {
    plutil -extract CFBundleShortVersionString raw -o - \
        "$1/Contents/Info.plist" 2>/dev/null \
        || fail "could not read the installed version"
}

run_update() {
    self_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd) \
        || fail "could not resolve the launcher directory"
    MANDATUM_INSTALL_DIR="${MANDATUM_INSTALL_DIR:-$self_dir}"
    export MANDATUM_INSTALL_DIR
    if app=$(resolve_app 2>/dev/null); then
        # Pin the destination to the app actually resolved, never an
        # ambient override: the update must replace this installation.
        MANDATUM_APP_DIR=$(dirname "$app")
        export MANDATUM_APP_DIR
        # Prefer the installer shipped inside that app: it is versioned
        # and checksummed with the release, so updating executes no code
        # fetched from a mutable branch.
        if [ -f "${app}/Contents/Resources/install.sh" ]; then
            exec /bin/sh "${app}/Contents/Resources/install.sh"
        fi
    fi
    # Bootstrap fallback (no installed app carries the installer yet):
    # download to a file and require transport success — never pipe a
    # possibly-empty stream into the shell.
    command -v curl >/dev/null 2>&1 || fail "curl is required to update"
    installer=$(mktemp "${TMPDIR:-/tmp}/mandatum-installer.XXXXXX") \
        || fail "could not create a temporary file for the installer"
    trap 'rm -f "$installer"' EXIT
    curl --proto '=https' --tlsv1.2 -fsSL --output "$installer" \
        "$INSTALLER_URL" || fail "could not download the installer"
    [ -s "$installer" ] || fail "downloaded installer is empty"
    /bin/sh "$installer"
}

case "${1:-}" in
    update)
        run_update
        ;;
    --version | -V | version)
        app=$(resolve_app)
        printf 'mandatum %s\n' "$(installed_version "$app")"
        ;;
    --help | -h | help)
        usage
        ;;
    *)
        app=$(resolve_app)
        log_dir="${HOME}/Library/Logs"
        mkdir -p "$log_dir"
        nohup "${app}/Contents/MacOS/Mandatum" "$@" \
            >>"${log_dir}/Mandatum.log" 2>&1 &
        printf 'Opening Mandatum in %s\n' "$PWD"
        ;;
esac
