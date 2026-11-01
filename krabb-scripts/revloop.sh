#!/usr/bin/env bash
set -euo pipefail

exec bash "$(cd "$(dirname "${BASH_SOURCE[0]}")/../krabb-loops" && pwd -P)/revloop.sh" "$@"
