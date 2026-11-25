#!/bin/sh
set -eu

usage() {
    printf '%s\n' \
        'Usage: uninstall.sh [--purge] [--yes]' \
        'Remove Krabb software: the service, the krabb and krabb-daemon binaries, and plugin code.' \
        'Settings, credentials, and conversations in KRABB_HOME stay unless you pass --purge.' \
        '--purge  Also remove KRABB_HOME, saved keyring entries, and stores Krabb created.' \
        '--yes    Skip the confirmation that --purge asks for.' \
        'Environment: KRABB_PREFIX, KRABB_HOME, KRABB_DB, KRABB_MEMORY.'
}

die() {
    printf '[ERROR] %s\n' "$*" >&2
    exit 2
}

info() {
    printf '[INFO] %s\n' "$*"
}

purge=false
yes=false

for argument in "$@"; do
    case "$argument" in
        --purge) purge=true ;;
        --yes | -y) yes=true ;;

        --help | -h)
            usage
            exit 0
            ;;

        *)
            usage >&2
            exit 2
            ;;
    esac
done

[ -n "${HOME:-}" ] || die 'HOME is not set; configure KRABB_PREFIX and KRABB_HOME.'

prefix=${KRABB_PREFIX:-"$HOME/.local"}
home=${KRABB_HOME:-"${XDG_CONFIG_HOME:-"$HOME/.config"}/krabb"}
krabb="$prefix/bin/krabb"
label=it.airscript.krabb

case "$(uname -s)" in
    Linux) service="${XDG_CONFIG_HOME:-"$HOME/.config"}/systemd/user/krabb.service" ;;
    Darwin) service="$HOME/Library/LaunchAgents/$label.plist" ;;
    *) die 'Unsupported operating system; use uninstall.ps1 on Windows.' ;;
esac

if [ "$purge" = true ] && [ "$yes" = false ]; then
    # Piped from curl, standard input is the script, so the answer comes from the terminal.
    ( : < /dev/tty ) 2>/dev/null ||
        die 'Confirm --purge with --yes when no terminal is available.'

    printf 'Delete %s with its settings, credentials, and conversations? [y/N] ' "$home" > /dev/tty
    read -r answer < /dev/tty || answer=''

    case "$answer" in
        y | Y | yes | YES) ;;
        *) die 'Nothing was removed.' ;;
    esac
fi

removed=''

remove() {
    [ -e "$1" ] || [ -L "$1" ] || return 0
    rm -rf "$1"
    removed="$removed
  $1"
}

# Krabb removes its own service when it can run; the definition is removed directly otherwise.
if [ -x "$krabb" ] && [ -f "$service" ] && "$krabb" service uninstall --yes > /dev/null 2>&1; then
    removed="$removed
  $service"
elif [ -f "$service" ]; then
    # Service managers address the unit by name, so stop it only when it is this definition.
    case "$(uname -s)" in
        Linux)
            loaded=$(systemctl --user show -p FragmentPath --value krabb.service 2>/dev/null) ||
                loaded=''

            [ "$loaded" != "$service" ] ||
                systemctl --user disable --now krabb.service > /dev/null 2>&1 || true
            ;;

        Darwin)
            launchctl bootout "gui/$(id -u)" "$service" > /dev/null 2>&1 || true
            ;;
    esac

    remove "$service"
    [ "$(uname -s)" != Linux ] || systemctl --user daemon-reload > /dev/null 2>&1 || true
fi

remove "$(dirname "$service")/$(basename "$service").credentials.json"

# A daemon started by hand keeps running from a deleted binary, so stop the ones started
# from this installation only.
if command -v pkill > /dev/null 2>&1; then
    pkill -u "$(id -u)" -f "^$prefix/bin/krabb-daemon( |$)" > /dev/null 2>&1 || true
fi

remove "$prefix/bin/krabb"
remove "$prefix/bin/krabb-daemon"
remove "$home/plugins"
remove "$home/cache"
for staged in "$home"/updates/v*; do
    remove "$staged"
done

if [ "$purge" = true ]; then
    remove "$home"

    # Stores at custom paths are files Krabb created; folders such as KRABB_ROOT are not.
    [ -z "${KRABB_DB:-}" ] || remove "$KRABB_DB"
    [ -z "${KRABB_MEMORY:-}" ] || { remove "$KRABB_MEMORY"; remove "$KRABB_MEMORY.lock"; }

    case "$(uname -s)" in
        Darwin)
            while security delete-generic-password -s "$label" > /dev/null 2>&1; do
                removed="$removed
  the $label keychain entry"
            done
            ;;

        Linux)
            if command -v secret-tool > /dev/null 2>&1 &&
                secret-tool search --all service "$label" 2>/dev/null | grep -q .; then
                secret-tool clear service "$label" 2>/dev/null || true
                removed="$removed
  the $label keyring entries"
            fi
            ;;
    esac
fi

if [ -n "$removed" ]; then
    info "Removed:$removed"
else
    info 'Krabb was not found, so nothing was removed.'
fi

if [ "$purge" = false ] && [ -d "$home" ]; then
    info "Kept $home with your settings, credentials, and conversations."
    info 'Remove them too with `uninstall.sh --purge`.'
fi

for variable in KRABB_ROOT KRABB_MEDIA; do
    eval "value=\${$variable:-}"
    [ -z "$value" ] || [ ! -e "$value" ] || info "Kept $value from $variable; delete it yourself if Krabb was its only user."
done

info 'Remove KRABB_* secrets set in your shell profile, and revoke keys where they were issued.'
