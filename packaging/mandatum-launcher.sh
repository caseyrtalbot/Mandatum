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
    command -v curl >/dev/null 2>&1 || fail "curl is required to update"
    self_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd) \
        || fail "could not resolve the launcher directory"
    MANDATUM_INSTALL_DIR="${MANDATUM_INSTALL_DIR:-$self_dir}"
    export MANDATUM_INSTALL_DIR
    if app=$(resolve_app 2>/dev/null); then
        MANDATUM_APP_DIR="${MANDATUM_APP_DIR:-$(dirname "$app")}"
        export MANDATUM_APP_DIR
    fi
    curl --proto '=https' --tlsv1.2 -fsSL "$INSTALLER_URL" | /bin/sh
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
