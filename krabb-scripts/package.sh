#!/usr/bin/env bash
set -euo pipefail

usage() {
    printf '%s\n' 'Usage: krabb-scripts/package.sh TARGET [NAME...]'
    printf '%s\n' 'Build and archive core or plugins for a target triple; NAME defaults to core.'
    printf '%s\n' 'Core uses VERSION; each plugin uses the version in its manifest.'
}

die() {
    printf '[ERROR] %s\n' "$*" >&2
    exit 2
}

info() {
    printf '[INFO] %s\n' "$*"
}

if (($# == 1)) && [[ "$1" == '--help' || "$1" == '-h' ]]; then
    usage
    exit 0
fi

if (($# < 1)); then
    usage >&2
    exit 2
fi

target=$1
shift
names=("$@")
((${#names[@]})) || names=(core)
[[ "$target" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || \
    die "Target '$target' is not a valid target triple."

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
repo_root=$(git -C "$script_dir" rev-parse --show-toplevel 2>/dev/null || {
    cd "$script_dir/.."
    pwd -P
})
repo_root=$(cd "$repo_root" && pwd -P)
[[ -f "$repo_root/Cargo.toml" ]] || die "Repository root '$repo_root' has no Cargo.toml."

for command in cargo mktemp tar; do
    command -v "$command" >/dev/null 2>&1 || die "The $command command is required."
done
if [[ "$target" == *-pc-windows-* ]]; then
    command -v zip >/dev/null 2>&1 || die 'The zip command is required for Windows archives.'
fi

valid_version() {
    [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]
}

manifest_for() {
    local candidate

    while IFS= read -r -d '' candidate; do
        if grep -Eq "^id = \"$1\"$" "$candidate"; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done < <(find "$repo_root/krabb-plugins" -name krabb-plugin.toml -not -path '*/target/*' -print0)

    return 1
}

version_for() {
    if [[ "$1" == core ]]; then
        tr -d '[:space:]' < "$repo_root/VERSION"
    else
        sed -n 's/^version = "\(.*\)"$/\1/p' "$(manifest_for "$1")" | head -n 1
    fi
}

release="$repo_root/target/$target/release"
stage=$(mktemp -d "$repo_root/.package.XXXXXX")
trap 'rm -rf "$stage"' EXIT
packages=()

for name in "${names[@]}"; do
    [[ "$name" =~ ^[a-z0-9][a-z0-9-]*$ ]] || die "Name '$name' is not a valid package name."

    if [[ "$name" == core ]]; then
        packages+=(-p krabb -p krabb-daemon)
    else
        manifest_for "$name" >/dev/null || die "Plugin manifest is missing: $name."
        packages+=(-p "krabb-plugin-$name")
    fi

    version=$(version_for "$name")
    valid_version "$version" || die "Version '$version' of '$name' is not a valid release version."
done

info "Building ${names[*]} for $target."
(cd "$repo_root" && cargo build --release --locked --target "$target" "${packages[@]}")

for name in "${names[@]}"; do
    artifact=krabb
    [[ "$name" == core ]] || artifact="krabb-plugin-$name"

    package_name="krabb-$name-v$(version_for "$name")-$target"
    package_dir="$stage/$package_name"
    binary="$release/$artifact"
    [[ -f "$binary" ]] || binary="$release/$artifact.exe"
    [[ -f "$binary" ]] || die "Built binary '$artifact' was not found for '$target'."

    mkdir -p "$package_dir/bin"
    destination="$package_dir/bin/$artifact"
    [[ "$binary" == *.exe ]] && destination+='.exe'
    cp "$binary" "$destination"
    if [[ "$name" == core ]]; then
        daemon="$release/krabb-daemon"
        [[ -f "$daemon" ]] || daemon="$release/krabb-daemon.exe"
        [[ -f "$daemon" ]] || die "Built binary 'krabb-daemon' was not found for '$target'."
        cp "$daemon" "$package_dir/bin/"
    else
        cp "$(manifest_for "$name")" "$package_dir/"
    fi
    cp "$repo_root/LICENSE" "$repo_root/NOTICE" "$package_dir/"

    if [[ "$target" == *-pc-windows-* ]]; then
        archive="$repo_root/$package_name.zip"
        rm -f "$archive"
        (cd "$stage" && zip -qr "$archive" "$package_name")
    else
        archive="$repo_root/$package_name.tar.gz"
        rm -f "$archive"
        tar -C "$stage" -czf "$archive" "$package_name"
    fi

    info "Created archive: ${archive#"$repo_root/"}."
done
