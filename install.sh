#!/bin/sh

set -eu

umask 077

REPOSITORY="caseyrtalbot/Mandatum"
RELEASE_BASE_URL="https://github.com/${REPOSITORY}/releases/latest/download"
EXPECTED_APPLE_TEAM_ID="3S2Y6XKV4P"

temporary_dir=""
install_stage=""
install_backup=""
install_committed=0

fail() {
    printf 'mandatum installer: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [ -n "$install_backup" ] && [ -d "$install_backup" ]; then
        restore_failed=0
        if [ "$install_committed" -eq 0 ]; then
            for binary in ${release_binaries:-}; do
                destination="${install_dir}/${binary}"
                if [ -f "${install_backup}/${binary}" ]; then
                    if ! mv -f "${install_backup}/${binary}" "$destination"; then
                        printf 'mandatum installer: warning: could not restore %s\n' \
                            "$destination" >&2
                        restore_failed=1
                    fi
                elif [ -f "${install_backup}/.missing-${binary}" ]; then
                    if ! rm -f "$destination"; then
                        printf 'mandatum installer: warning: could not remove partial %s\n' \
                            "$destination" >&2
                        restore_failed=1
                    fi
                fi
            done
        fi
        if [ "$restore_failed" -eq 0 ]; then
            rm -rf "$install_backup"
        else
            printf 'mandatum installer: backup retained for manual recovery: %s\n' \
                "$install_backup" >&2
        fi
    fi
    if [ -n "$install_stage" ] && [ -d "$install_stage" ]; then
        rm -rf "$install_stage"
    fi
    if [ -n "$temporary_dir" ] && [ -d "$temporary_dir" ]; then
        rm -rf "$temporary_dir"
    fi
}

trap cleanup EXIT
trap 'exit 1' HUP INT TERM

download() {
    url=$1
    destination=$2

    if command -v curl >/dev/null 2>&1; then
        curl --fail --location --silent --show-error \
            --proto '=https' --tlsv1.2 --retry 3 \
            --output "$destination" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget --https-only --quiet --output-document="$destination" "$url"
    else
        fail "curl or wget is required"
    fi
}

is_numeric_triplet() {
    printf '%s\n' "$1" | awk -F. '
        NF == 3 &&
        $1 ~ /^[0-9]+$/ && $2 ~ /^[0-9]+$/ && $3 ~ /^[0-9]+$/ {
            valid = 1
        }
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

fetch_verified_archive() {
    release_archive_name=$1
    release_expected_members=$2
    release_archive_path="${temporary_dir}/${release_archive_name}"
    release_checksum_name="${release_archive_name}.sha256"
    release_checksum_path="${temporary_dir}/${release_checksum_name}"

    printf 'Downloading %s...\n' "$release_archive_name"
    download "${RELEASE_BASE_URL}/${release_archive_name}" "$release_archive_path"
    download "${RELEASE_BASE_URL}/${release_checksum_name}" "$release_checksum_path"

    if ! release_checksum_value=$(awk '
        NF {
            count += 1
            if (count == 1) print $1
        }
        END {
            if (count != 1) exit 1
        }
    ' "$release_checksum_path"); then
        fail "$release_checksum_name must contain exactly one checksum"
    fi

    [ "${#release_checksum_value}" -eq 64 ] \
        || fail "$release_checksum_name is not SHA-256"
    case "$release_checksum_value" in
        *[!0123456789abcdefABCDEF]*)
            fail "$release_checksum_name is not SHA-256"
            ;;
    esac

    release_verification_file="${temporary_dir}/verify-${release_checksum_name}"
    printf '%s  %s\n' "$release_checksum_value" "$release_archive_name" \
        >"$release_verification_file"
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$temporary_dir" && sha256sum -c "$(basename "$release_verification_file")") \
            || fail "$release_archive_name checksum verification failed"
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$temporary_dir" && shasum -a 256 -c "$(basename "$release_verification_file")") \
            || fail "$release_archive_name checksum verification failed"
    else
        fail "sha256sum or shasum is required"
    fi

    if ! release_archive_members=$(tar -tzf "$release_archive_path" | LC_ALL=C sort); then
        fail "could not inspect $release_archive_name"
    fi
    [ "$release_archive_members" = "$release_expected_members" ] \
        || fail "$release_archive_name contains unexpected paths"
    tar -xzf "$release_archive_path" -C "$extract_dir" \
        || fail "could not extract $release_archive_name"
}

remote_http_status() {
    url=$1
    if command -v curl >/dev/null 2>&1; then
        curl --location --silent --show-error \
            --proto '=https' --tlsv1.2 --retry 3 \
            --head --output /dev/null --write-out '%{http_code}' "$url" \
            || fail "could not inspect the native macOS release"
    elif command -v wget >/dev/null 2>&1; then
        wget_response=$(wget --https-only --server-response --spider "$url" 2>&1 || true)
        wget_status=$(printf '%s\n' "$wget_response" | awk '
            /^  HTTP\// { status = $2 }
            END {
                if (status ~ /^[0-9][0-9][0-9]$/) print status
                else exit 1
            }
        ') || fail "could not inspect the native macOS release"
        printf '%s' "$wget_status"
    else
        fail "curl or wget is required"
    fi
}

verify_macos_binary() {
    binary_path=$1
    codesign --verify --strict --verbose=2 "$binary_path" \
        || fail "$(basename "$binary_path") has an invalid Developer ID signature"
    if ! signature_info=$(codesign --display --verbose=4 "$binary_path" 2>&1); then
        fail "could not inspect $(basename "$binary_path") signing identity"
    fi
    team_id=$(printf '%s\n' "$signature_info" | sed -n 's/^TeamIdentifier=//p')
    [ "$team_id" = "$EXPECTED_APPLE_TEAM_ID" ] \
        || fail "$(basename "$binary_path") is not signed by the expected Apple team"
    printf '%s\n' "$signature_info" |
        grep -q '^Authority=Developer ID Application:' \
        || fail "$(basename "$binary_path") is not signed with a Developer ID Application certificate"
}

operating_system=$(uname -s) || fail "could not detect the operating system"
machine=$(uname -m) || fail "could not detect the processor architecture"
common_release_binaries="mandatum mandatum-approval-bridge"
native_release_available=0

case "$operating_system" in
    Darwin)
        platform="apple-darwin"
        release_binaries="$common_release_binaries mandatum-native"
        ;;
    Linux)
        platform="unknown-linux-gnu"
        release_binaries=$common_release_binaries
        libc_description=""
        if command -v getconf >/dev/null 2>&1; then
            libc_description=$(getconf GNU_LIBC_VERSION 2>/dev/null || true)
        fi
        if [ -z "$libc_description" ] && command -v ldd >/dev/null 2>&1; then
            libc_description=$(ldd --version 2>&1 || true)
        fi
        case "$libc_description" in
            *musl* | *MUSL* | *Musl*)
                fail "prebuilt Linux archives require glibc; build Mandatum from source on musl"
                ;;
        esac
        ;;
    *)
        fail "unsupported operating system: $operating_system"
        ;;
esac

case "$machine" in
    x86_64 | amd64)
        architecture="x86_64"
        ;;
    arm64 | aarch64)
        architecture="aarch64"
        ;;
    *)
        fail "unsupported processor architecture: $machine"
        ;;
esac

target="${architecture}-${platform}"
common_archive_name="mandatum-${target}.tar.gz"

if [ -n "${MANDATUM_INSTALL_DIR:-}" ]; then
    install_dir=$MANDATUM_INSTALL_DIR
else
    [ -n "${HOME:-}" ] || fail "HOME is not set; set MANDATUM_INSTALL_DIR"
    install_dir="${HOME}/.local/bin"
fi

case "$install_dir" in
    /*) ;;
    *) fail "install directory must be an absolute path: $install_dir" ;;
esac
case "$install_dir" in
    *:*) fail "install directory must not contain a colon" ;;
esac

temporary_root=${TMPDIR:-/tmp}
case "$temporary_root" in
    /*) ;;
    *) fail "TMPDIR must be an absolute path: $temporary_root" ;;
esac
[ -d "$temporary_root" ] || fail "temporary directory does not exist: $temporary_root"

temporary_dir=$(mktemp -d "${temporary_root%/}/mandatum-install.XXXXXX") \
    || fail "could not create a private temporary directory"
extract_dir="${temporary_dir}/extract"
mkdir "$extract_dir"

common_expected_members=$(printf '%s\n' LICENSE $common_release_binaries | LC_ALL=C sort)
fetch_verified_archive "$common_archive_name" "$common_expected_members"
if [ "$operating_system" = "Darwin" ]; then
    native_archive_name="mandatum-native-${target}.tar.gz"
    case "$(remote_http_status "${RELEASE_BASE_URL}/${native_archive_name}")" in
        200)
            native_expected_members=$(printf '%s\n' LICENSE mandatum-native | LC_ALL=C sort)
            fetch_verified_archive "$native_archive_name" "$native_expected_members"
            native_release_available=1
            ;;
        404)
            release_binaries=$common_release_binaries
            printf '%s\n' \
                "The latest release does not include the native macOS application yet." \
                "Installing the terminal frontend and approval bridge only."
            ;;
        *)
            fail "native macOS release returned an unexpected HTTP status"
            ;;
    esac
fi

for binary in $release_binaries; do
    binary_path="${extract_dir}/${binary}"
    [ -f "$binary_path" ] || fail "release archive is missing $binary"
    [ ! -L "$binary_path" ] || fail "release archive contains an unsafe $binary symlink"
    [ -x "$binary_path" ] || fail "release archive contains a non-executable $binary"
done
[ -f "${extract_dir}/LICENSE" ] && [ ! -L "${extract_dir}/LICENSE" ] \
    || fail "release archive contains an invalid LICENSE"

if [ "$native_release_available" -eq 1 ]; then
    command -v codesign >/dev/null 2>&1 \
        || fail "codesign is required to verify the macOS release"
    for binary in $release_binaries; do
        verify_macos_binary "${extract_dir}/${binary}"
    done
fi

if [ -n "${MANDATUM_CURRENT_VERSION:-}" ]; then
    is_numeric_triplet "$MANDATUM_CURRENT_VERSION" \
        || fail "running version is not a numeric x.y.z release: $MANDATUM_CURRENT_VERSION"

    if ! release_version_output=$("${extract_dir}/mandatum" --version 2>/dev/null); then
        fail "latest published release predates self-update; current installation was not changed"
    fi
    case "$release_version_output" in
        "mandatum "*) release_version=${release_version_output#mandatum } ;;
        *) fail "latest published release reported an invalid version; current installation was not changed" ;;
    esac
    is_numeric_triplet "$release_version" \
        || fail "latest published release is not a numeric x.y.z version: $release_version"
    if [ "$release_version" = "$MANDATUM_CURRENT_VERSION" ]; then
        installed_set_complete=1
        for binary in $release_binaries; do
            destination="${install_dir}/${binary}"
            if [ ! -f "$destination" ] || [ -L "$destination" ]; then
                installed_set_complete=0
            fi
        done
        if [ "$installed_set_complete" -eq 1 ]; then
            printf 'Mandatum %s is already the latest published release.\n' "$release_version"
            exit 0
        fi
    fi
    if version_is_older "$release_version" "$MANDATUM_CURRENT_VERSION"; then
        fail "latest published release $release_version is older than running version $MANDATUM_CURRENT_VERSION; current installation was not changed"
    fi
fi

if [ -e "$install_dir" ] || [ -L "$install_dir" ]; then
    [ -d "$install_dir" ] || fail "install path is not a directory: $install_dir"
else
    mkdir -p "$install_dir" || fail "could not create install directory: $install_dir"
fi

for binary in $release_binaries; do
    destination="${install_dir}/${binary}"
    if [ -e "$destination" ] || [ -L "$destination" ]; then
        [ -f "$destination" ] && [ ! -L "$destination" ] \
            || fail "refusing to replace non-regular path: $destination"
    fi
done

install_stage=$(mktemp -d "${install_dir%/}/.mandatum-install.XXXXXX") \
    || fail "could not create a private staging directory in $install_dir"
install_backup=$(mktemp -d "${install_dir%/}/.mandatum-backup.XXXXXX") \
    || fail "could not create a private backup directory in $install_dir"

for binary in $release_binaries; do
    destination="${install_dir}/${binary}"
    if [ -f "$destination" ]; then
        install -m 0755 "$destination" "${install_backup}/${binary}" \
            || fail "could not back up $destination"
    else
        : >"${install_backup}/.missing-${binary}" \
            || fail "could not record the new install path $destination"
    fi
done

for binary in $release_binaries; do
    install -m 0755 "${extract_dir}/${binary}" "${install_stage}/${binary}" \
        || fail "could not stage $binary"
done

# Install companions first and the command that owns self-update last. If a
# companion move fails, the existing `mandatum update` entry point remains
# available to retry.
for binary in $release_binaries; do
    [ "$binary" = "mandatum" ] && continue
    mv -f "${install_stage}/${binary}" "${install_dir}/${binary}" \
        || fail "could not install $binary"
done
mv -f "${install_stage}/mandatum" "${install_dir}/mandatum" \
    || fail "could not install mandatum"
rmdir "$install_stage"
install_stage=""
install_committed=1
rm -rf "$install_backup"
install_backup=""

if [ "$native_release_available" -eq 1 ]; then
    printf 'Installed mandatum-native, mandatum, and mandatum-approval-bridge to %s\n' \
        "$install_dir"
    printf 'Run mandatum-native for the GPU-native app, or mandatum for the terminal tool.\n'
else
    printf 'Installed mandatum and mandatum-approval-bridge to %s\n' "$install_dir"
fi
case ":${PATH:-}:" in
    *:"$install_dir":*) ;;
    *) printf 'Add %s to PATH, then run: mandatum\n' "$install_dir" ;;
esac
