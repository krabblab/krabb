#!/usr/bin/env bash
# Steps of the Install and Update drills on Linux and macOS. The serve steps also run on
# Windows, under Git Bash. Each step reads and extends the job environment through
# GITHUB_ENV and GITHUB_PATH, so the workflow lists them as named steps.
set -euo pipefail

REGISTRY="ghcr.io/$(printf '%s' "${GITHUB_REPOSITORY:-krabblab/krabb}" | tr '[:upper:]' '[:lower:]')/plugins"

usage() {
    printf '%s\n' 'Usage: krabb-scripts/drills/drill.sh STEP'
    printf '%s\n' 'Run one Install or Update drill step; see the case list for the steps.'
}

fail() {
    echo "::error::$*"
    [ -z "${KRABB_HOME:-}" ] || tail -n 40 "$KRABB_HOME"/logs/*.log 2>/dev/null || true
    exit 1
}

# GNU tools mark a line whose file name has a backslash, as Windows paths do, with one.
digest() {
    local sum
    sum=$(sha256sum "$1" 2>/dev/null || shasum -a 256 "$1")
    sum=${sum#\\}
    printf '%s\n' "${sum%% *}"
}

python() {
    if [ "$RUNNER_OS" = Windows ]; then command python "$@"; else python3 "$@"; fi
}

daemon_state() {
    krabb --json status < /dev/null | jq -r .daemon
}

until_daemon() {
    for _ in $(seq 1 60); do
        [ "$(daemon_state)" = "$1" ] && return 0
        sleep 1
    done

    return 1
}

wait_daemon() {
    until_daemon "$1" || fail "The daemon did not become $1."
}

has_session() {
    krabb --json session list < /dev/null | jq -e --arg id "$1" '.items[] | select(.id == $id)' \
        > /dev/null
}

# systemd reads user units from the runner's own home and needs a user session, so Linux
# keeps Krabb's home isolated but uses the runner's systemd user manager.
runner_systemd() {
    [ "$RUNNER_OS" = Linux ] || return 0
    XDG_RUNTIME_DIR="/run/user/$(id -u)"
    XDG_CONFIG_HOME="$KRABB_RUNNER_HOME/.config"
    export XDG_RUNTIME_DIR XDG_CONFIG_HOME
}

enable_systemd() {
    [ "$RUNNER_OS" = Linux ] || return 0
    sudo loginctl enable-linger "$(id -un)"
    runner_systemd

    for _ in $(seq 1 30); do
        state=$(systemctl --user is-system-running 2>/dev/null || true)
        [ "$state" = running ] || [ "$state" = degraded ] && break
        sleep 1
    done
}

# Checks that DIR holds the core archive at VERSION and its SHA256SUMS line.
check_release() {
    local dir=$1 version=$2
    local archive="krabb-core-v$version-$KRABB_TARGET.$KRABB_ARCHIVE"

    test -f "$dir/$archive" || fail "The release in $dir has no $archive."
    test -f "$dir/SHA256SUMS" || fail "The release in $dir has no SHA256SUMS."

    awk -v archive="$archive" -v sum="$(digest "$dir/$archive")" \
        '$1 == sum && ($2 == archive || $2 == "*" archive) { found = 1 } END { exit !found }' \
        "$dir/SHA256SUMS" || fail "SHA256SUMS in $dir does not match $archive."
}

# Plugins come from an index of the plugin archives in DIR when it has any, with published
# versions filling in the rest, and from the published index otherwise.
serve_plugins() {
    local dir=$1 index plugins file built=false

    for file in "$dir"/krabb-*-v*-"$KRABB_TARGET.$KRABB_ARCHIVE"; do
        if [ -f "$file" ] && [[ "${file##*/}" != krabb-core-v* ]]; then built=true; fi
    done

    if [ "$built" = true ]; then
        index=$(python krabb-scripts/plugins.py local "$dir" "$KRABB_TARGET" \
            "$RUNNER_TEMP/plugins" "$REGISTRY")
        plugins=$(python krabb-scripts/plugins.py ids "$index")
        echo "KRABB_PLUGIN_INDEX=$index" >> "$GITHUB_ENV"
        echo "Plugins come from the local index $index."
    else
        plugins=$(python krabb-scripts/plugins.py ids "$REGISTRY")
        echo "Plugins come from the published index."
    fi

    echo "KRABB_PLUGINS=$plugins" >> "$GITHUB_ENV"
}

install_serve() {
    local version
    version=$(bash krabb-scripts/version.sh)
    check_release "$RUNNER_TEMP/release" "$version"
    serve_plugins "$RUNNER_TEMP/release"

    {
        echo "KRABB_EXPECTED=$version"
        echo "KRABB_VERSION=$version"
    } >> "$GITHUB_ENV"
}

install_home() {
    local user="$RUNNER_TEMP/user"
    mkdir -p "$user"

    {
        echo "KRABB_RUNNER_HOME=$HOME"
        echo "HOME=$user"
        echo "XDG_CONFIG_HOME=$user/.config"
        echo "KRABB_HOME=$user/.krabb"
        echo "KRABB_PREFIX=$user/.local"
        echo "KRABB_RELEASE_BASE=file://$RUNNER_TEMP/release"
    } >> "$GITHUB_ENV"

    echo "$user/.local/bin" >> "$GITHUB_PATH"
}

install_plugins() {
    local failed=() installed id

    for id in $(jq -r '.[]' <<<"$KRABB_PLUGINS"); do
        krabb plugin install "$id" --yes < /dev/null || failed+=("$id")
    done

    installed=$(krabb --json plugin list | jq -r '.items[].id')

    for id in $(jq -r '.[]' <<<"$KRABB_PLUGINS"); do
        grep -qx "$id" <<<"$installed" || failed+=("$id (not listed)")
    done

    [ "${#failed[@]}" -eq 0 ] || fail "Plugins that did not install: ${failed[*]}"
}

install_health() {
    local unhealthy
    krabb --json doctor < /dev/null > /dev/null

    unhealthy=$(krabb --json plugin list | jq -r '[.items[]
        | select(.health | IN("needs setup", "configured", "ready") | not)
        | "\(.id) (\(.health))"] | join(", ")')

    [ -z "$unhealthy" ] || fail "Plugins that are not healthy: $unhealthy"
}

install_daemon() {
    local log="$RUNNER_TEMP/daemon.log" pid

    krabb-daemon >> "$log" 2>&1 &
    pid=$!
    until_daemon running || { cat "$log"; fail "The daemon did not start."; }
    krabb --json session new install-flow < /dev/null > /dev/null

    kill "$pid"
    wait "$pid" || true
    until_daemon stopped || { cat "$log"; fail "The daemon did not stop."; }

    krabb-daemon >> "$log" 2>&1 &
    pid=$!
    until_daemon running || { cat "$log"; fail "The daemon did not start."; }
    has_session install-flow || fail "The session did not survive the restart."

    kill "$pid"
    wait "$pid" || true
}

install_service() {
    local path
    enable_systemd

    path=$(krabb --json service install | jq -r .path)
    test -f "$path"
    test "$(krabb --json service status | jq -r .status)" = installed

    krabb service start
    wait_daemon running
    krabb --json session new service-flow < /dev/null > /dev/null

    krabb service restart
    wait_daemon running
    has_session service-flow || fail "The session did not survive the service restart."

    krabb service stop
    wait_daemon stopped
    krabb service uninstall --yes
    test ! -e "$path"
}

install_uninstall() {
    local path left
    runner_systemd

    krabb --json service install > /dev/null
    krabb service start
    sh uninstall.sh

    if [ "$RUNNER_OS" = Linux ] && systemctl --user is-active --quiet krabb.service; then
        fail "Uninstall left the service running."
    fi

    for path in "$KRABB_PREFIX/bin/krabb" "$KRABB_PREFIX/bin/krabb-daemon"; do
        [ ! -e "$path" ] || fail "Uninstall left $path."
    done

    test -f "$KRABB_HOME/config.toml"
    test -d "$KRABB_HOME/plugins"
    sh uninstall.sh --purge --yes

    left=$(find "$HOME" "$KRABB_RUNNER_HOME/.config/systemd" -iname '*krabb*' 2>/dev/null || true)
    [ -z "$left" ] || fail "Purge left: $left"
}

update_serve() {
    local candidate root unstable=false
    candidate=$(bash krabb-scripts/version.sh)
    test "$candidate" != "$KRABB_PREVIOUS"
    check_release "$RUNNER_TEMP/previous" "$KRABB_PREVIOUS"
    check_release "$RUNNER_TEMP/candidate" "$candidate"
    serve_plugins "$RUNNER_TEMP/candidate"

    # `krabb update` reads a file:// base as a native path, so Windows uses D:/a/... form.
    root=${RUNNER_TEMP//\\//}

    case "$candidate" in *-*) unstable=true ;; esac

    {
        echo "KRABB_CANDIDATE=$candidate"
        echo "KRABB_UNSTABLE=$unstable"
        echo "KRABB_PREVIOUS_BASE=file://$root/previous"
        echo "KRABB_CANDIDATE_BASE=file://$root/candidate"
        echo "KRABB_BROKEN_BASE=file://$root/broken"
    } >> "$GITHUB_ENV"
}

update_broken() {
    local work broken="$RUNNER_TEMP/broken" release version name variable
    work=$(mktemp -d)
    mkdir -p "$broken"

    for release in previous candidate; do
        version=$KRABB_PREVIOUS
        [ "$release" = candidate ] && version=$KRABB_CANDIDATE
        name="krabb-core-v$version-$KRABB_TARGET"
        mkdir -p "$work/$release"
        tar -xzf "$RUNNER_TEMP/$release/$name.tar.gz" -C "$work/$release"
        variable=KRABB_PREVIOUS_DAEMON
        [ "$release" = candidate ] && variable=KRABB_CANDIDATE_DAEMON
        echo "$variable=$(digest "$work/$release/$name/bin/krabb-daemon")" >> "$GITHUB_ENV"
    done

    name="krabb-core-v$KRABB_CANDIDATE-$KRABB_TARGET"
    cp /usr/bin/false "$work/candidate/$name/bin/krabb-daemon"
    COPYFILE_DISABLE=1 tar -czf "$broken/$name.tar.gz" -C "$work/candidate" "$name"
    printf '%s  %s\n' "$(digest "$broken/$name.tar.gz")" "$name.tar.gz" > "$broken/SHA256SUMS"
}

update_home() {
    local user="$RUNNER_TEMP/user"
    mkdir -p "$user"

    {
        echo "KRABB_RUNNER_HOME=$HOME"
        echo "HOME=$user"
        echo "KRABB_HOME=$user/.krabb"
        echo "KRABB_PREFIX=$user/.local"
    } >> "$GITHUB_ENV"

    echo "$user/.local/bin" >> "$GITHUB_PATH"

    if [ "$RUNNER_OS" = Linux ]; then
        KRABB_RUNNER_HOME=$HOME enable_systemd
        echo "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR" >> "$GITHUB_ENV"
        echo "XDG_CONFIG_HOME=$XDG_CONFIG_HOME" >> "$GITHUB_ENV"
    else
        echo "XDG_CONFIG_HOME=$user/.config" >> "$GITHUB_ENV"
    fi
}

update_previous() {
    KRABB_RELEASE_BASE="$KRABB_PREVIOUS_BASE" KRABB_VERSION="$KRABB_PREVIOUS" sh install.sh
    test "$(krabb version)" = "krabb $KRABB_PREVIOUS"
}

update_setup() {
    local flags=(--yes) id
    krabb --json init < /dev/null > /dev/null
    [ "$KRABB_UNSTABLE" = true ] && flags+=(--unstable)

    for id in $(jq -r '.[]' <<<"$KRABB_PLUGINS"); do
        krabb plugin install "$id" "${flags[@]}" < /dev/null
    done

    krabb --json service install > /dev/null
    krabb service start
    wait_daemon running
    krabb --json session new update-drill < /dev/null > /dev/null
    krabb --json plugin list < /dev/null | jq -c '[.items[].id] | sort' > "$RUNNER_TEMP/plugins.ids"
    cat "$KRABB_HOME/config.toml" "$KRABB_HOME/plugins.lock" > "$RUNNER_TEMP/state"
}

update_restore() {
    local err="$RUNNER_TEMP/failed.err" status plugins

    if KRABB_RELEASE_BASE="$KRABB_BROKEN_BASE" \
        krabb --json update "$KRABB_CANDIDATE" --yes < /dev/null > /dev/null 2> "$err"; then
        cat "$err"
        fail "The update succeeded although the new daemon cannot start."
    fi

    cat "$err"
    grep -q "so Krabb $KRABB_PREVIOUS was restored" "$err" || fail "The update did not restore."
    test "$(krabb version)" = "krabb $KRABB_PREVIOUS" || fail "The CLI was not restored."

    [ "$(digest "$KRABB_PREFIX/bin/krabb-daemon")" = "$KRABB_PREVIOUS_DAEMON" ] ||
        fail "The daemon binary was not restored."

    until_daemon running || fail "The service did not return."
    status=$(krabb --json status < /dev/null)
    jq -e '.recovery == null' <<<"$status" > /dev/null || fail "The recovery point remains."

    cat "$KRABB_HOME/config.toml" "$KRABB_HOME/plugins.lock" | cmp -s - "$RUNNER_TEMP/state" ||
        fail "The configuration or plugin lock was not restored."

    plugins=$(krabb --json plugin list < /dev/null | jq -c '[.items[].id] | sort')
    [ "$plugins" = "$(cat "$RUNNER_TEMP/plugins.ids")" ] || fail "The plugins changed: $plugins."
    has_session update-drill || fail "The session did not survive the restore."
}

ready() {
    krabb --json status < /dev/null |
        jq -c '[.plugin_states[] | select(.state == "ready") | .id]'
}

update_candidate() {
    local before after detached result status
    before=$(ready)

    detached=$(krabb --json plugin list < /dev/null | jq -r --argjson ready "$before" '
        [.items[] | select(.id as $id | $ready | index($id))
          | select(.capabilities | all(. == "client") | not) | .id][0] // empty')

    [ -n "$detached" ] || fail "No plugin is loaded, so none can stay detached."
    krabb --json plugin detach "$detached" < /dev/null > /dev/null
    echo "Detached $detached; ready before the update: $before."

    result=$(KRABB_RELEASE_BASE="$KRABB_CANDIDATE_BASE" \
        krabb --json update "$KRABB_CANDIDATE" --yes < /dev/null) || fail "The update failed."

    echo "$result"

    jq -e --arg version "$KRABB_CANDIDATE" \
        '.status == "updated" and .version == $version and ((.unloaded // []) | length == 0)' \
        <<<"$result" > /dev/null || fail "The update result is not a clean update."

    test "$(krabb version)" = "krabb $KRABB_CANDIDATE" || fail "The CLI was not replaced."

    [ "$(digest "$KRABB_PREFIX/bin/krabb-daemon")" = "$KRABB_CANDIDATE_DAEMON" ] ||
        fail "The daemon binary was not replaced."

    status=$(krabb --json status < /dev/null)
    jq -e '.daemon == "running"' <<<"$status" > /dev/null || fail "The service is not running."

    jq -e --arg from "$KRABB_PREVIOUS" '.recovery.accepted == true and .recovery.from == $from' \
        <<<"$status" > /dev/null || fail "The recovery point was not kept."

    after=$(ready)
    echo "Ready after the update: $after."

    jq -e --arg id "$detached" --argjson after "$after" \
        'map(select(. != $id)) - $after | length == 0' <<<"$before" > /dev/null ||
        fail "Plugins loaded before the update are not loaded after it."

    jq -e --arg id "$detached" 'index($id) == null' <<<"$after" > /dev/null ||
        fail "The detached plugin $detached was attached by the update."

    has_session update-drill || fail "The session did not survive the update."

    krabb --json plugin attach "$detached" < /dev/null > /dev/null
    ready | jq -e --arg id "$detached" 'index($id) != null' > /dev/null ||
        fail "The detached plugin $detached did not attach again."
}

update_rollback() {
    local daemon="$KRABB_PREFIX/bin/krabb-daemon" rollback result

    krabb service stop
    wait_daemon stopped
    cp /usr/bin/false "$daemon.broken"
    mv -f "$daemon.broken" "$daemon"
    krabb service start || true
    sleep 15

    [ "$(daemon_state)" = stopped ] || fail "The daemon started although it cannot."

    rollback="$KRABB_HOME/updates/recovery/bin/0/krabb"
    test -x "$rollback" || fail "The recovery point has no krabb binary."
    result=$("$rollback" --json update --rollback --yes < /dev/null) || fail "Rollback failed."
    echo "$result"

    jq -e --arg version "$KRABB_PREVIOUS" '.status == "restored" and .version == $version' \
        <<<"$result" > /dev/null || fail "The rollback result is not a restore."

    test "$(krabb version)" = "krabb $KRABB_PREVIOUS" || fail "The CLI was not restored."
    [ "$(digest "$daemon")" = "$KRABB_PREVIOUS_DAEMON" ] || fail "The daemon was not restored."
    wait_daemon running
    has_session update-drill || fail "The session did not survive the rollback."

    krabb --json status < /dev/null | jq -e '.recovery == null' > /dev/null ||
        fail "The recovery point remains after the rollback."
}

main() {
    case "${1:-}" in
        install-serve) install_serve ;;
        install-home) install_home ;;
        install) sh install.sh ;;
        version) test "$(krabb version)" = "krabb $KRABB_EXPECTED" ;;
        init) krabb --json init < /dev/null ;;
        doctor) krabb --json doctor < /dev/null ;;
        status) krabb --json status < /dev/null ;;
        plugins) install_plugins ;;
        health) install_health ;;
        daemon) install_daemon ;;
        service) install_service ;;
        uninstall) install_uninstall ;;
        update-serve) update_serve ;;
        broken) update_broken ;;
        update-home) update_home ;;
        previous) update_previous ;;
        setup) update_setup ;;
        restore) update_restore ;;
        update) update_candidate ;;
        rollback) update_rollback ;;
        remove) { command -v krabb > /dev/null && krabb service uninstall --yes; } || true ;;

        *)
            usage >&2
            exit 2
            ;;
    esac
}

main "$@"
