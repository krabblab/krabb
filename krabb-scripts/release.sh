#!/usr/bin/env bash
set -euo pipefail

usage() {
    printf '%s\n' 'Usage: krabb-scripts/release.sh TAG'
    printf '%s\n' '       krabb-scripts/release.sh --notes TAG FILE'
    printf '%s\n' 'Validate a release tag and build the optimized workspace at its version, or'
    printf '%s\n' 'write its release notes to FILE and print its version and whether it is a'
    printf '%s\n' 'pre-release.'
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

notes=''

if (($# == 3)) && [[ "$1" == '--notes' ]]; then
    notes=$3
    shift
elif (($# != 1)); then
    usage >&2
    exit 2
fi

tag=$1
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || \
    die "Tag '$tag' is not a valid release tag."

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
repo_root=$(git -C "$script_dir" rev-parse --show-toplevel 2>/dev/null || {
    cd "$script_dir/.."
    pwd -P
})
repo_root=$(cd "$repo_root" && pwd -P)
[[ -f "$repo_root/Cargo.toml" ]] || die "Repository root '$repo_root' has no Cargo.toml."
[[ -f "$repo_root/CHANGELOG.md" ]] || die "CHANGELOG.md is missing from '$repo_root'."

version=${tag#v}
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]
base=${BASH_REMATCH[0]}
prerelease=true
[[ "$version" == "$base" ]] && prerelease=false
workspace=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo_root/Cargo.toml" | head -n 1)
[[ "$base" == "$workspace" ]] || \
    die "Tag '$tag' does not match the workspace version '$workspace'."

if [[ -n "$notes" ]]; then
    if [[ "$prerelease" == true ]]; then
        cat > "$notes" <<EOF
Krabb $version is a pre-release for testing.

It is not offered by \`krabb update\` and is not recommended for production use.
Install it with \`KRABB_VERSION=$version\`, and install pre-release plugins
with \`krabb plugin install <id> --unstable\`. Please report any issues on GitHub.
EOF
    else
        awk -v version="$version" -v tag="$tag" -v today="$(date -u +%F)" '
            index($0, "## [" version "]") == 1 {
                found = 1
                date = substr($0, length("## [" version "]") + 1)
                sub(/^ - /, "", date)
                print "## krabb@" tag " | " (date == "" ? today : date)
                print ""
                next
            }
            found && /^## / { exit }
            found && (body || $0 != "") { body = 1; print }
            END { if (!body) exit 1 }
        ' "$repo_root/CHANGELOG.md" > "$notes" || \
            die "CHANGELOG.md has no notes for version '$version'."
    fi

    printf '%s %s\n' "$version" "$prerelease"
    exit 0
fi

[[ "$prerelease" == true ]] || grep -Fq "## [$version]" "$repo_root/CHANGELOG.md" || \
    die "CHANGELOG.md has no section for version '$version'."
command -v cargo >/dev/null 2>&1 || die 'The cargo command is required.'

info "Building release $tag."
(cd "$repo_root" && KRABB_BUILD_VERSION=$version cargo build --workspace --release --locked)
info "Release $tag is ready."
