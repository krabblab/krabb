#!/usr/bin/env bash
set -euo pipefail

usage() {
    printf '%s\n' 'Usage: krabb-scripts/precommit.sh [BASE]'
    printf '%s\n' 'Run the fast local gate on what changed since BASE (default: origin/main):'
    printf '%s\n' 'formatting, spacing of changed files, then Clippy and unit tests of the'
    printf '%s\n' 'affected packages. CI and `make gate` run the full checks.'
}

if [[ "${1:-}" == '--help' || "${1:-}" == '-h' ]]; then
    usage
    exit 0
fi

if (($# > 1)); then
    usage >&2
    exit 2
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
base=${1:-origin/main}
jobs=${CARGO_BUILD_JOBS:-2}

cd "$repo_root"

cargo fmt --all --check
bash krabb-scripts/spacing.sh --check --changed "$base"

selection=$(python3 -B krabb-scripts/changes.py --local "$base" | sed -n 's/^cargo=//p')

if [[ -z "$selection" ]]; then
    printf '%s\n' '[INFO] No Rust code changed; skipping Clippy and tests.'
    exit 0
fi

printf '[INFO] Checking %s\n' "$selection"

# The selection is a list of Cargo flags, so it is split on purpose.
# shellcheck disable=SC2086
CARGO_BUILD_JOBS=$jobs cargo clippy $selection --all-targets --locked -- -D warnings

# shellcheck disable=SC2086
CARGO_BUILD_JOBS=$jobs cargo test $selection --lib --bins --locked
