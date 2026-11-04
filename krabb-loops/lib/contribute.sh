#!/usr/bin/env bash
set -euo pipefail

LOOP_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
LOOP=contrib
print_error() { printf '[ERROR] %s\n' "$*" >&2; }
print_info() { printf '[INFO] %s\n' "$*"; }
die() { local status=$1; shift; print_error "$*"; exit "$status"; }
source "$LOOP_DIR/lib/config.sh"
source "$LOOP_DIR/lib/agent.sh"
configure
AGENT=$(agent_detect "$AGENT")
agent_validate
agent_run "$1" "$2" "$3" "$4" "$MODEL" "$REASONING" \
    "$AGENT_TIMEOUT" "$5" "$6"
