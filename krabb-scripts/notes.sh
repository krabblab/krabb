#!/usr/bin/env bash
set -euo pipefail

usage() {
    printf '%s\n' 'Usage: krabb-scripts/notes.sh FILE'
    printf '%s\n' 'Write the release notes for the workspace version to FILE, then print the'
    printf '%s\n' 'version and whether it is a pre-release.'
}

die() {
    printf '[ERROR] %s\n' "$*" >&2
    exit 2
}

if (($# == 1)) && [[ "$1" == '--help' || "$1" == '-h' ]]; then
    usage
    exit 0
fi

if (($# != 1)); then
    usage >&2
    exit 2
fi

notes=$1
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
version=$(bash "$root/krabb-scripts/version.sh")

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || \
    die "Workspace version '$version' is not a valid release version."

prerelease=true
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] && prerelease=false

if [[ "$prerelease" == true ]]; then
    cat > "$notes" <<NOTES
Krabb $version is a pre-release for testing.

It is not offered by \`krabb update\` and is not recommended for production use.
Install it with \`KRABB_VERSION=$version\`, and install pre-release plugins
with \`krabb plugin install <id> --unstable\`. Please report any issues on GitHub.
NOTES
else
    changelog="$root/CHANGELOG.md"
    [[ -f "$changelog" ]] || changelog=/dev/null

    awk -v version="$version" -v today="$(date -u +%F)" '
        function headline(date) { print "## krabb@v" version " | " date; print "" }
        index($0, "## [" version "]") == 1 {
            found = 1
            date = substr($0, length("## [" version "]") + 1)
            sub(/^ - /, "", date)
            headline(date == "" ? today : date)
            next
        }
        found && /^## / { exit }
        found && (body || $0 != "") { body = 1; print }
        END {
            if (!body) {
                if (!found) headline(today)
                print "Krabb " version " is a stable release. Install it with the instructions in"
                print "the README, or update an existing installation with `krabb update`."
            }
        }
    ' "$changelog" > "$notes"
fi

printf '%s %s\n' "$version" "$prerelease"
