#!/usr/bin/env sh
set -eu

usage() {
    printf '%s\n' 'Usage: install.sh [PLUGIN[@VERSION]]'
    printf '%s\n' 'Install Krabb core from a release, or one official plugin from the plugin index.'
    printf '%s\n' \
        'Environment: KRABB_VERSION and KRABB_RELEASE_BASE (core), KRABB_PREFIX, KRABB_BIN.'
}

die() {
    printf '[ERROR] %s\n' "$*" >&2
    exit 2
}

info() {
    printf '[INFO] %s\n' "$*"
}

if [ "$#" -eq 1 ] && { [ "$1" = '--help' ] || [ "$1" = '-h' ]; }; then
    usage
    exit 0
fi
[ "$#" -le 1 ] || {
    usage >&2
    exit 2
}

plugin=${1:-core}
id=${plugin%%@*}
pin=${plugin#"$id"}

invalid="Plugin ID '$plugin' is not valid."

case "$id" in
    '' | [![:lower:][:digit:]]* | *[![:lower:][:digit:]-]*) die "$invalid" ;;
esac

case "$pin" in
    '') ;;
    @ | @*[![:alnum:].-]*) die "$invalid" ;;
esac

if [ -n "${KRABB_PREFIX:-}" ]; then
    prefix=$KRABB_PREFIX
else
    [ -n "${HOME:-}" ] || die 'HOME is not set; configure KRABB_PREFIX.'
    prefix="$HOME/.local"
fi

case "$plugin" in
    core@*) die 'Set KRABB_VERSION to choose a Krabb version.' ;;
esac

if [ "$plugin" != core ]; then
    [ -z "${KRABB_VERSION:-}" ] || \
        die "KRABB_VERSION applies to Krabb itself; install a plugin version with $id@VERSION."
    [ -z "${KRABB_RELEASE_BASE:-}" ] || \
        die 'KRABB_RELEASE_BASE applies to Krabb itself; plugins come from the plugin index.'
    krabb=${KRABB_BIN:-"$prefix/bin/krabb"}
    if [ ! -x "$krabb" ]; then
        krabb=$(command -v krabb || true)
    fi
    [ -n "$krabb" ] && [ -x "$krabb" ] || \
        die 'Install the Krabb core before installing an official plugin.'
    "$krabb" plugin install "$plugin" --yes
    info "Installed plugin $plugin."
    exit 0
fi

for command in awk curl find install mkdir mktemp mv tar tr; do
    command -v "$command" >/dev/null 2>&1 || die "The $command command is required."
done

os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)
case "$arch" in
    x86_64|amd64) target_arch=x86_64 ;;
    aarch64|arm64) target_arch=aarch64 ;;
    *) die "Unsupported architecture: '$arch'." ;;
esac
case "$os" in
    linux) target_os=unknown-linux-gnu; archive_format=tar.gz ;;
    darwin) target_os=apple-darwin; archive_format=tar.gz ;;
    *) die 'Use install.ps1 on Windows.' ;;
esac

tmp=''
cleanup() {
    [ -z "$tmp" ] || rm -rf "$tmp"
}
trap cleanup EXIT

valid_version() {
    printf '%s\n' "$1" | awk \
        '$0 ~ /^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$/ { valid = 1 } END { exit !valid }' || \
        die "Version '$1' is not valid. Use a version such as 0.1.0 or 0.1.0-rc.1."
}


# Stable releases are what releases/latest serves; before the first one, name the newest
# pre-release so the user can pin it.
prerelease() {
    list=$(curl --fail --silent --location --max-time 30 \
        "${KRABB_RELEASE_LIST:-https://api.github.com/repos/krabblab/krabb/releases?per_page=1}") ||
        return 0

    # A stable newest release means the download failed for another reason.
    printf '%s\n' "$list" | grep -q '"prerelease": *true' || return 0

    newest=$(printf '%s\n' "$list" |
        sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' | head -n 1)

    # The tag goes into a command the user copies, so it must be a plain version.
    printf '%s\n' "$newest" | awk '$0 ~ /^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$/ { ok = 1 }
        END { exit !ok }' || return 0

    # The variable must reach sh, not curl, so it goes after the pipe or is exported first.
    die "No stable Krabb release is published yet.
The newest pre-release is $newest; install it with this command:
  curl -fsSL $script | KRABB_VERSION=$newest sh
KRABB_VERSION goes after the pipe so sh receives it, or export it first."
}

target="$target_arch-$target_os"
name=krabb
releases=https://github.com/krabblab/krabb/releases
script=https://raw.githubusercontent.com/krabblab/krabb/main/install.sh

if [ -n "${KRABB_VERSION:-}" ]; then
    version=${KRABB_VERSION#v}
    valid_version "$version"
    base=${KRABB_RELEASE_BASE:-"$releases/download/v$version"}
    missing="Cannot reach Krabb $version; check the version and the connection."
else
    version=''
    base=${KRABB_RELEASE_BASE:-"$releases/latest/download"}
    missing='Cannot reach the latest Krabb release; check the connection or set KRABB_VERSION.'
fi
[ -z "${KRABB_RELEASE_BASE:-}" ] || missing="Cannot download SHA256SUMS from $base."
base=${base%/}
case "$base" in
    https://*|file://*) ;;
    *) die 'KRABB_RELEASE_BASE must use https:// or file://.' ;;
esac
tmp=$(mktemp -d)

curl --fail --silent --show-error --location --retry 3 \
    --connect-timeout 10 --max-time 120 \
    "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || {
    [ -n "$version" ] || { [ -n "${KRABB_RELEASE_BASE:-}" ] && [ -z "${KRABB_RELEASE_LIST:-}" ]; } ||
        prerelease
    die "$missing"
}

if [ -z "$version" ]; then
    suffix="-$target.$archive_format"
    latest=$(awk -v suffix="$suffix" '
        { file = $2; sub(/^\*/, "", file) }
        index(file, "krabb-core-v") == 1 && substr(file, length(file) - length(suffix) + 1) == suffix {
            print file
            exit
        }
    ' "$tmp/SHA256SUMS")
    version=${latest#krabb-core-v}
    version=${version%"$suffix"}
    [ -n "$version" ] || die "The latest Krabb release has no build for $target."
    valid_version "$version"
    [ -n "${KRABB_RELEASE_BASE:-}" ] || base="$releases/download/v$version"
fi

archive_name="krabb-core-v$version-$target.$archive_format"

info "Downloading $archive_name."
curl --fail --silent --show-error --location --retry 3 \
    --connect-timeout 10 --max-time 120 \
    "$base/$archive_name" -o "$tmp/$archive_name" || die "Cannot download $archive_name."

expected=$(awk -v archive="$archive_name" '$2 == archive || $2 == "*" archive { print $1; exit }' \
    "$tmp/SHA256SUMS")
[ -n "$expected" ] || die "Checksum is missing for '$archive_name'."

if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$archive_name" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp/$archive_name" | awk '{ print $1 }')
else
    die 'The sha256sum or shasum command is required.'
fi
[ "$actual" = "$expected" ] || die "Checksum verification failed for '$archive_name'."

tar -xzf "$tmp/$archive_name" -C "$tmp"
binary=$(find "$tmp" -type f -name "$name" -print -quit)
[ -n "$binary" ] || die "Archive does not contain '$name'."
daemon=$(find "$tmp" -type f -name "krabb-daemon" -print -quit)
[ -n "$daemon" ] || die "Archive does not contain 'krabb-daemon'."
mkdir -p "$prefix/bin"
install -m 0755 "$binary" "$prefix/bin/.$name.tmp.$$"
mv -f "$prefix/bin/.$name.tmp.$$" "$prefix/bin/$name"
install -m 0755 "$daemon" "$prefix/bin/.krabb-daemon.tmp.$$"
mv -f "$prefix/bin/.krabb-daemon.tmp.$$" "$prefix/bin/krabb-daemon"
info "Installed $name and krabb-daemon in $prefix/bin."

case ":$PATH:" in
    *":$prefix/bin:"*) ;;

    *)
        line="export PATH=\"$prefix/bin:\$PATH\""

        # Terminal.app opens bash as a login shell, which reads ~/.bash_profile, not ~/.bashrc.
        case "${SHELL:-}" in
            */zsh) profile='~/.zshrc' ;;
            */bash) [ "$target_os" = apple-darwin ] && profile='~/.bash_profile' || profile='~/.bashrc' ;;
            */fish) profile='~/.config/fish/config.fish' line="fish_add_path $prefix/bin" ;;
            *) profile='~/.profile' ;;
        esac

        info "Add $prefix/bin to your PATH by adding this line to $profile, then open a new terminal:"
        printf '%s\n' "$line"
        ;;
esac

info 'Set up Krabb with `krabb init`.'
