#!/usr/bin/env bash
set -euo pipefail

LOOP_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
exec python3 "$LOOP_DIR/lib/process.py" loop \
    bash "$LOOP_DIR/lib/engine.sh" "$LOOP_DIR" sec "$@"
