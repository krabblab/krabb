#!/usr/bin/env bash
# Plans one run of the Release workflow and writes its decisions to GITHUB_OUTPUT.
#
# A release covers the head of main once CI passed for it; a dry run covers any commit and
# publishes nothing. It builds when Krabb's version has no GitHub Release yet or a plugin
# version is unpublished, and otherwise ends the run here. The release notes are written to
# release-notes.md.
#
# Reads KRABB_EVENT, KRABB_REF, KRABB_COMMIT, KRABB_CI_COMMIT, KRABB_PUBLISH, KRABB_KRABB,
# KRABB_PLUGINS, and KRABB_REBUILD, plus GH_REPO and GH_TOKEN.
set -euo pipefail

registry="ghcr.io/${GH_REPO,,}/plugins"
dry=false
[[ "$KRABB_PUBLISH" == true ]] || dry=true

refuse() {
    echo "::error title=Release refused::$*"
    exit 1
}

decide() {
    {
        echo "build=$1"
        echo "publish=$([[ "$dry" == true ]] && echo false || echo true)"
        echo "version=${version:-}"
        echo "prerelease=${prerelease:-false}"
        echo "release=${release:-false}"
        echo "plugins=${plugins:-}"
        echo "index=${index:-false}"
    } >> "$GITHUB_OUTPUT"
}

skip() {
    echo "::notice title=Release skipped::$*"
    decide false
    exit 0
}

# Releases only the current head of main, once CI has passed for it. Each release covers
# every earlier commit, so CI for a commit that is no longer the head releases nothing.
gate() {
    local head short=${KRABB_CI_COMMIT:0:7} passed

    if [[ "$KRABB_EVENT" == workflow_dispatch && "$KRABB_REF" != refs/heads/main ]]; then
        refuse "Run the Release workflow from main, or clear its publish option for a dry run."
    fi

    head=$(gh api "repos/$GH_REPO/commits/main" --jq .sha)

    if [[ "$KRABB_EVENT" == workflow_dispatch && "$KRABB_COMMIT" != "$head" ]]; then
        refuse "Main moved past $short while this run was queued; run it again."
    fi

    if [[ "$KRABB_CI_COMMIT" != "$head" || "$KRABB_COMMIT" != "$head" ]]; then
        skip "Main has moved past $short, so this run releases nothing;" \
            "CI for the newer commit releases it."
    fi

    if [[ "$KRABB_EVENT" == workflow_dispatch ]]; then
        passed=$(gh run list --workflow main.yml --branch main --commit "$head" \
            --status success --limit 100 --json event \
            --jq '[.[] | select(.event != "pull_request")] | length')

        [[ "$passed" != 0 ]] ||
            refuse "The latest CI run for $short, the head of main, has not passed."
    fi
}

released() {
    local answer

    if answer=$(gh api "repos/$GH_REPO/releases/tags/v$version" --silent 2>&1); then
        return 0
    elif [[ "$answer" != *"HTTP 404"* ]]; then
        echo "$answer" >&2
        exit 1
    fi

    return 1
}

if [[ "$dry" == true ]]; then
    echo "::notice title=Dry run::This dry run publishes nothing."
else
    [[ -z "$KRABB_REBUILD" ]] || refuse "Rebuilding published plugins is for dry runs only."
    gate
fi

notes=$(bash krabb-scripts/notes.sh release-notes.md)
read -r version prerelease <<< "$notes"
release=false

if [[ "$KRABB_KRABB" == true ]]; then
    if released; then
        echo "::notice title=Krabb release skipped::Krabb $version is already released, so" \
            "this run publishes no new Krabb release."
    else
        release=true
    fi
fi

plugins=""
index=false

if [[ "$KRABB_PLUGINS" == true ]]; then
    plugins=$(python3 krabb-scripts/plugins.py plan "$registry")
    index=$(python3 krabb-scripts/plugins.py stale "$registry")

    for name in $KRABB_REBUILD; do
        [[ "$name" =~ ^[a-z0-9][a-z0-9-]*$ && "$name" != core ]] ||
            refuse "Plugin ID '$name' is not valid."
        [[ " $plugins " == *" $name "* ]] || plugins="${plugins:+$plugins }$name"
    done

    if [[ -n "$plugins" ]]; then
        echo "Plugins to release: $plugins."
    else
        echo "::notice title=Plugin release skipped::Every plugin version is already" \
            "published, so this run releases no plugin."
    fi
fi

if [[ "$release" == true || -n "$plugins" ]]; then
    decide true
elif [[ "$index" == true ]]; then
    echo "::notice title=Plugin index stale::The plugin index misses published versions," \
        "so this run rebuilds it and builds nothing."
    decide false
else
    skip "Krabb $version is released and every plugin version is published, so this run" \
        "builds and publishes nothing."
fi
