#!/usr/bin/env bash
set -euo pipefail

cd "${1:?repository root required}"
while IFS= read -r -d '' file; do
    bash -n "$file"
done < <(find krabb-loops krabb-scripts -type f -name '*.sh' -print0)
[[ ! -f install.sh ]] || bash -n install.sh
