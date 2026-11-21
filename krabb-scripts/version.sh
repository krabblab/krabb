#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)

version=$(awk '
    { sub(/\r$/, "") }
    /^\[/ { section = $0; next }
    section == "[workspace.package]" && /^version = "/ {
        gsub(/^version = "|"$/, "")
        print
        exit
    }
' "$root/Cargo.toml")

[[ -n "$version" ]] || {
    printf '%s\n' '[ERROR] Cargo.toml has no workspace version.' >&2
    exit 2
}

printf '%s\n' "$version"
