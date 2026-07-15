#!/bin/sh

set -eu

umask 077

REPOSITORY="caseyrtalbot/Mandatum"
RELEASE_BASE_URL="https://github.com/${REPOSITORY}/releases/latest/download"

temporary_dir=""
install_stage=""

fail() {
    printf 'mandatum installer: %s\n' "$*" >&2
    exit 1
}

cleanup() {
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

operating_system=$(uname -s) || fail "could not detect the operating system"
machine=$(uname -m) || fail "could not detect the processor architecture"

case "$operating_system" in
    Darwin)
        platform="apple-darwin"
        ;;
    Linux)
        platform="unknown-linux-gnu"
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
archive_name="mandatum-${target}.tar.gz"
checksum_name="${archive_name}.sha256"

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
archive_path="${temporary_dir}/${archive_name}"
checksum_path="${temporary_dir}/${checksum_name}"

printf 'Downloading %s...\n' "$archive_name"
download "${RELEASE_BASE_URL}/${archive_name}" "$archive_path"
download "${RELEASE_BASE_URL}/${checksum_name}" "$checksum_path"

if ! checksum_value=$(awk '
    NF {
        count += 1
        if (count == 1) print $1
    }
    END {
        if (count != 1) exit 1
    }
' "$checksum_path"); then
    fail "checksum file must contain exactly one checksum"
fi

[ "${#checksum_value}" -eq 64 ] || fail "release checksum is not SHA-256"
case "$checksum_value" in
    *[!0123456789abcdefABCDEF]*) fail "release checksum is not SHA-256" ;;
esac

verification_file="${temporary_dir}/verify.sha256"
printf '%s  %s\n' "$checksum_value" "$archive_name" >"$verification_file"
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$temporary_dir" && sha256sum -c "$(basename "$verification_file")") \
        || fail "archive checksum verification failed"
elif command -v shasum >/dev/null 2>&1; then
    (cd "$temporary_dir" && shasum -a 256 -c "$(basename "$verification_file")") \
        || fail "archive checksum verification failed"
else
    fail "sha256sum or shasum is required"
fi

expected_members=$(printf '%s\n' LICENSE mandatum mandatum-approval-bridge | LC_ALL=C sort)
if ! archive_members=$(tar -tzf "$archive_path" | LC_ALL=C sort); then
    fail "could not inspect the release archive"
fi
[ "$archive_members" = "$expected_members" ] \
    || fail "release archive contains unexpected paths"

extract_dir="${temporary_dir}/extract"
mkdir "$extract_dir"
tar -xzf "$archive_path" -C "$extract_dir" \
    || fail "could not extract the release archive"

for binary in mandatum mandatum-approval-bridge; do
    binary_path="${extract_dir}/${binary}"
    [ -f "$binary_path" ] || fail "release archive is missing $binary"
    [ ! -L "$binary_path" ] || fail "release archive contains an unsafe $binary symlink"
    [ -x "$binary_path" ] || fail "release archive contains a non-executable $binary"
done
[ -f "${extract_dir}/LICENSE" ] && [ ! -L "${extract_dir}/LICENSE" ] \
    || fail "release archive contains an invalid LICENSE"

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
    if version_is_older "$release_version" "$MANDATUM_CURRENT_VERSION"; then
        fail "latest published release $release_version is older than running version $MANDATUM_CURRENT_VERSION; current installation was not changed"
    fi
fi

if [ -e "$install_dir" ] || [ -L "$install_dir" ]; then
    [ -d "$install_dir" ] || fail "install path is not a directory: $install_dir"
else
    mkdir -p "$install_dir" || fail "could not create install directory: $install_dir"
fi

for binary in mandatum mandatum-approval-bridge; do
    destination="${install_dir}/${binary}"
    if [ -e "$destination" ] || [ -L "$destination" ]; then
        [ -f "$destination" ] && [ ! -L "$destination" ] \
            || fail "refusing to replace non-regular path: $destination"
    fi
done

install_stage=$(mktemp -d "${install_dir%/}/.mandatum-install.XXXXXX") \
    || fail "could not create a private staging directory in $install_dir"
install -m 0755 "${extract_dir}/mandatum" "${install_stage}/mandatum" \
    || fail "could not stage mandatum"
install -m 0755 "${extract_dir}/mandatum-approval-bridge" \
    "${install_stage}/mandatum-approval-bridge" \
    || fail "could not stage mandatum-approval-bridge"

mv -f "${install_stage}/mandatum-approval-bridge" \
    "${install_dir}/mandatum-approval-bridge" \
    || fail "could not install mandatum-approval-bridge"
mv -f "${install_stage}/mandatum" "${install_dir}/mandatum" \
    || fail "could not install mandatum"
rmdir "$install_stage"
install_stage=""

printf 'Installed mandatum and mandatum-approval-bridge to %s\n' "$install_dir"
case ":${PATH:-}:" in
    *:"$install_dir":*) ;;
    *) printf 'Add %s to PATH, then run: mandatum\n' "$install_dir" ;;
esac
