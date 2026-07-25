#!/bin/sh
# Mandatum installer.
#
#   curl --proto '=https' --tlsv1.2 -LsSf \
#     https://raw.githubusercontent.com/caseyrtalbot/Mandatum/main/install.sh | sh
#
# Downloads the latest Mandatum.app release, verifies its checksum,
# installs it to /Applications (or ~/Applications), and installs the
# `mandatum` command. `mandatum update` runs this same script.
#
# Overrides:
#   MANDATUM_APP_DIR      directory that receives Mandatum.app
#   MANDATUM_INSTALL_DIR  directory that receives the mandatum command
#                         (default ~/.local/bin)
set -eu

REPOSITORY="caseyrtalbot/Mandatum"
RELEASE_BASE_URL="https://github.com/${REPOSITORY}/releases/latest/download"
ASSET_NAME="Mandatum.app.zip"

temporary_dir=""
stage_dir=""
backup_dir=""

fail() {
    printf 'mandatum installer: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [ -n "$backup_dir" ] && [ -d "$backup_dir/Mandatum.app" ] \
        && [ ! -d "${app_dir}/Mandatum.app" ]; then
        if ! mv "$backup_dir/Mandatum.app" "${app_dir}/Mandatum.app"; then
            printf 'mandatum installer: previous app retained at %s\n' \
                "$backup_dir/Mandatum.app" >&2
            backup_dir=""
        fi
    fi
    if [ -n "$backup_dir" ]; then rm -rf "$backup_dir"; fi
    if [ -n "$stage_dir" ]; then rm -rf "$stage_dir"; fi
    if [ -n "$temporary_dir" ]; then rm -rf "$temporary_dir"; fi
}

download() {
    curl --fail --location --silent --show-error \
        --proto '=https' --tlsv1.2 --retry 3 \
        --output "$2" "$1"
}

is_numeric_triplet() {
    printf '%s\n' "$1" | awk -F. '
        NF == 3 &&
        $1 ~ /^[0-9]+$/ && $2 ~ /^[0-9]+$/ && $3 ~ /^[0-9]+$/ { valid = 1 }
        END { exit !valid }
    '
}

version_is_older() {
    awk -v candidate="$1" -v current="$2" 'BEGIN {
        split(candidate, a, ".")
        split(current, b, ".")
        for (i = 1; i <= 3; i += 1) {
            if ((a[i] + 0) < (b[i] + 0)) exit 0
            if ((a[i] + 0) > (b[i] + 0)) exit 1
        }
        exit 1
    }'
}

app_version() {
    plutil -extract CFBundleShortVersionString raw -o - \
        "$1/Contents/Info.plist" 2>/dev/null
}

install_launcher() {
    launcher_source="${app_dir}/Mandatum.app/Contents/Resources/mandatum"
    [ -f "$launcher_source" ] \
        || fail "the installed app is missing its command-line launcher"
    if [ -e "$install_dir" ] || [ -L "$install_dir" ]; then
        [ -d "$install_dir" ] \
            || fail "install path is not a directory: $install_dir"
    else
        mkdir -p "$install_dir" \
            || fail "could not create install directory: $install_dir"
    fi
    install -m 0755 "$launcher_source" "${install_dir}/mandatum" \
        || fail "could not install the mandatum command"
}

main() {
    [ "$(uname -s)" = "Darwin" ] \
        || fail "Mandatum is a macOS app; this installer requires macOS"
    command -v curl >/dev/null 2>&1 || fail "curl is required"
    command -v ditto >/dev/null 2>&1 || fail "ditto is required"
    command -v plutil >/dev/null 2>&1 || fail "plutil is required"

    if [ -n "${MANDATUM_APP_DIR:-}" ]; then
        app_dir=$MANDATUM_APP_DIR
        case "$app_dir" in
            /*) ;;
            *) fail "MANDATUM_APP_DIR must be an absolute path: $app_dir" ;;
        esac
        mkdir -p "$app_dir" || fail "could not create $app_dir"
    elif [ -d /Applications ] && [ -w /Applications ]; then
        app_dir="/Applications"
    else
        [ -n "${HOME:-}" ] || fail "HOME is not set; set MANDATUM_APP_DIR"
        app_dir="${HOME}/Applications"
        mkdir -p "$app_dir" || fail "could not create $app_dir"
    fi

    if [ -n "${MANDATUM_INSTALL_DIR:-}" ]; then
        install_dir=$MANDATUM_INSTALL_DIR
        case "$install_dir" in
            /*) ;;
            *) fail "MANDATUM_INSTALL_DIR must be an absolute path: $install_dir" ;;
        esac
    else
        [ -n "${HOME:-}" ] || fail "HOME is not set; set MANDATUM_INSTALL_DIR"
        install_dir="${HOME}/.local/bin"
    fi

    temporary_root=${TMPDIR:-/tmp}
    temporary_dir=$(mktemp -d "${temporary_root%/}/mandatum-install.XXXXXX") \
        || fail "could not create a temporary directory"

    printf 'Downloading %s...\n' "$ASSET_NAME"
    download "${RELEASE_BASE_URL}/${ASSET_NAME}" \
        "${temporary_dir}/${ASSET_NAME}"
    download "${RELEASE_BASE_URL}/${ASSET_NAME}.sha256" \
        "${temporary_dir}/${ASSET_NAME}.sha256"

    checksum_value=$(awk 'NF { print $1; exit }' \
        "${temporary_dir}/${ASSET_NAME}.sha256")
    printf '%s\n' "$checksum_value" | grep -Eq '^[0-9a-fA-F]{64}$' \
        || fail "${ASSET_NAME}.sha256 does not contain a SHA-256 checksum"
    printf '%s  %s\n' "$checksum_value" "$ASSET_NAME" \
        >"${temporary_dir}/verify.sha256"
    if command -v shasum >/dev/null 2>&1; then
        (cd "$temporary_dir" && shasum -a 256 -c verify.sha256 >/dev/null) \
            || fail "checksum verification failed for $ASSET_NAME"
    elif command -v sha256sum >/dev/null 2>&1; then
        (cd "$temporary_dir" && sha256sum -c verify.sha256 >/dev/null) \
            || fail "checksum verification failed for $ASSET_NAME"
    else
        fail "shasum or sha256sum is required"
    fi

    extract_dir="${temporary_dir}/extract"
    mkdir "$extract_dir"
    ditto -x -k "${temporary_dir}/${ASSET_NAME}" "$extract_dir" \
        || fail "could not extract $ASSET_NAME"
    [ -d "${extract_dir}/Mandatum.app" ] \
        || fail "$ASSET_NAME does not contain Mandatum.app"
    [ -x "${extract_dir}/Mandatum.app/Contents/MacOS/Mandatum" ] \
        || fail "downloaded app is missing its executable"

    new_version=$(app_version "${extract_dir}/Mandatum.app") \
        || fail "could not read the downloaded app version"
    is_numeric_triplet "$new_version" \
        || fail "downloaded app version is not x.y.z: $new_version"

    installed_app="${app_dir}/Mandatum.app"
    if [ -d "$installed_app" ]; then
        installed_version=$(app_version "$installed_app" || true)
        if [ "$installed_version" = "$new_version" ]; then
            install_launcher
            printf 'Mandatum %s is already the latest release.\n' "$new_version"
            return 0
        fi
        if is_numeric_triplet "${installed_version:-}" \
            && version_is_older "$new_version" "$installed_version"; then
            fail "latest release $new_version is older than installed $installed_version; nothing was changed"
        fi
    fi

    # Stage inside the destination directory so the final move is atomic
    # on one volume, and keep the previous app until the swap succeeds.
    stage_dir=$(mktemp -d "${app_dir}/.mandatum-stage.XXXXXX") \
        || fail "could not stage in $app_dir"
    ditto "${extract_dir}/Mandatum.app" "${stage_dir}/Mandatum.app" \
        || fail "could not stage Mandatum.app"
    xattr -dr com.apple.quarantine "${stage_dir}/Mandatum.app" 2>/dev/null || true

    if [ -d "$installed_app" ]; then
        backup_dir=$(mktemp -d "${app_dir}/.mandatum-backup.XXXXXX") \
            || fail "could not back up the installed app"
        mv "$installed_app" "${backup_dir}/Mandatum.app" \
            || fail "could not move the installed app aside"
    fi
    mv "${stage_dir}/Mandatum.app" "$installed_app" \
        || fail "could not install Mandatum.app"
    rm -rf "$backup_dir" "$stage_dir"
    backup_dir=""
    stage_dir=""

    # The command that owns `mandatum update` is installed last, so a
    # failed app swap leaves the previous updater in place.
    install_launcher

    printf 'Installed Mandatum %s to %s\n' "$new_version" "$installed_app"
    printf 'Installed the mandatum command to %s\n' "${install_dir}/mandatum"
    case ":${PATH:-}:" in
        *:"$install_dir":*) ;;
        *) printf 'Add %s to PATH to use the mandatum command.\n' "$install_dir" ;;
    esac
    printf 'Type mandatum in a project directory to open it.\n'
}

trap cleanup EXIT
trap 'exit 1' HUP INT TERM
main "$@"
