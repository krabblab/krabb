#!/bin/sh
set -eu

usage() {
    printf '%s\n' \
        'Usage: uninstall.sh [--purge] [--yes]' \
        'Remove Krabb software: its service and the krabb and krabb-daemon binaries.' \
        'KRABB_HOME stays with plugins, settings, credentials, and conversations.' \
        '--purge  Also remove KRABB_HOME, its keyring entry, and the stores Krabb created.' \
        '--yes    Skip the confirmation that --purge asks for.' \
        'Environment: KRABB_PREFIX, and KRABB_HOME when no service is installed.'
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
daemon="$prefix/bin/krabb-daemon"
label=it.airscript.krabb
system=$(uname -s)

case "$system" in
    Linux) service="${XDG_CONFIG_HOME:-"$HOME/.config"}/systemd/user/krabb.service" ;;
    Darwin) service="$HOME/Library/LaunchAgents/$label.plist" ;;
    *) die 'Unsupported operating system; use uninstall.ps1 on Windows.' ;;
esac

removed=''
kept=''

note() {
    eval "$1=\"\$$1
  \$2\""
}

# The program a service definition runs, as `krabb service install` writes it.
program() {
    case "$system" in
        Linux)
            sed -n 's/^ExecStart="\(.*\)"$/\1/p' "$service" | head -n 1 |
                sed -e 's/\\"/"/g' -e 's/\\\\/\\/g'
            ;;

        Darwin)
            tr -d '\n' < "$service" |
                sed -n 's:.*<key>ProgramArguments</key><array><string>\([^<]*\)</string>.*:\1:p' |
                sed -e 's/&lt;/</g' -e 's/&gt;/>/g' -e 's/&quot;/"/g' -e "s/&apos;/'/g" \
                    -e 's/&amp;/\&/g'
            ;;
    esac
}

# A variable the installed service sets, decoded from the definition.
setting() {
    case "$system" in
        Linux)
            sed -n "s/^Environment=\"$1=\\(.*\\)\"\$/\\1/p" "$service" | head -n 1 |
                sed -e 's/%%/%/g' -e 's/\\\$/$/g' -e 's/\\"/"/g' -e 's/\\\\/\\/g'
            ;;

        Darwin)
            tr -d '\n' < "$service" |
                sed -n "s:.*<key>$1</key><string>\\([^<]*\\)</string>.*:\\1:p" |
                sed -e 's/&lt;/</g' -e 's/&gt;/>/g' -e 's/&quot;/"/g' -e "s/&apos;/'/g" \
                    -e 's/&amp;/\&/g'
            ;;
    esac
}

# Only a service that runs this installation's daemon belongs to this installation; another
# Krabb, such as a build installed elsewhere, keeps its service.
owned=false

if [ -f "$service" ]; then
    runs=$(program)

    if [ -n "$runs" ] && { [ "$runs" = "$daemon" ] || [ "$runs" -ef "$daemon" ]; }; then
        owned=true
    else
        note kept "$service, the service of the Krabb at ${runs:-an unknown location}"
    fi
fi

# The installed service names the home and stores this installation really uses.
path() {
    value=''
    [ "$owned" = false ] || value=$(setting "$1")
    [ -n "$value" ] || eval "value=\${$1:-}"
    printf '%s' "$value"
}

home=$(path KRABB_HOME)
home=${home:-"${XDG_CONFIG_HOME:-"$HOME/.config"}/krabb"}

# Deleting a folder that is not a Krabb home, such as HOME itself, would destroy user files.
krabb_home=false

for marker in plugins.lock config.toml schema.json; do
    [ ! -f "$home/$marker" ] || krabb_home=true
done

if [ "$purge" = true ] && [ "$yes" = false ]; then
    # Piped from curl, standard input is the script, so the answer comes from the terminal.
    ( : < /dev/tty ) 2>/dev/null || die 'Confirm --purge with --yes when no terminal is available.'

    printf '%s\n' "Purging deletes $home with its plugins, settings, credentials, and" \
        'conversations, its keyring entry, and the stores Krabb created elsewhere.' > /dev/tty
    printf 'Purge Krabb? [y/N] ' > /dev/tty
    read -r answer < /dev/tty || answer=''

    case "$answer" in
        y | Y | yes | YES) ;;
        *) die 'Nothing was removed.' ;;
    esac
fi

remove() {
    [ -e "$1" ] || [ -L "$1" ] || return 0
    rm -rf "$1"
    note removed "$1"
}

# A file Krabb saves atomically, with its lock and the sidecars of an interrupted save.
store() {
    case "$1" in
        '') return 0 ;;
        /*) ;;

        *)
            note kept "$1, because it is not an absolute path"
            return 0
            ;;
    esac

    directory=$(dirname "$1")
    name=$(basename "$1")
    remove "$1"

    for sidecar in "$1-wal" "$1-shm" "$1-journal" "$directory/.$name.lock" \
        "$directory/.$name".tmp-* "$directory/.$name".backup-* \
        "$directory/.$name".txn-* "$directory/.$name".live-*; do
        remove "$sidecar"
    done

    # The memory plugin locks its file with the extension replaced, as memory.lock.
    [ "${name%.*}" = "$name" ] || remove "$directory/${name%.*}.lock"
}

# Read before the home goes: the exact keyring entry this home's credentials use.
credentials=$(path KRABB_CREDENTIALS)
credentials=${credentials:-"$home/credentials.json"}
entry=''

if [ -f "$credentials" ] && grep -q '"backend": *"keyring"' "$credentials"; then
    entry=$(sed -n 's/.*"key_id": *"\([0-9a-fA-F]\{64\}\)".*/credentials:\1/p' "$credentials" |
        head -n 1)
fi

if [ "$owned" = true ]; then
    krabb="$prefix/bin/krabb"

    if [ -x "$krabb" ] && "$krabb" service uninstall --yes > /dev/null 2>&1 && [ ! -f "$service" ]
    then
        note removed "$service"
    else
        # Service managers address the unit by name, so stop it only when it is this definition.
        case "$system" in
            Linux)
                loaded=$(systemctl --user show -p FragmentPath --value krabb.service 2>/dev/null) ||
                    loaded=''

                [ "$loaded" != "$service" ] ||
                    systemctl --user disable --now krabb.service > /dev/null 2>&1 || true
                ;;

            Darwin) launchctl bootout "gui/$(id -u)" "$service" > /dev/null 2>&1 || true ;;
        esac

        remove "$service"
        [ "$system" != Linux ] || systemctl --user daemon-reload > /dev/null 2>&1 || true
    fi

    store "$service.credentials.json"
    store "$service.credentials.json.new"
fi

# A daemon started by hand keeps running from a deleted binary, so stop the ones started
# from this installation only.
if command -v pkill > /dev/null 2>&1; then
    pkill -u "$(id -u)" -f "^$daemon( |\$)" > /dev/null 2>&1 || true
fi

remove "$prefix/bin/krabb"
remove "$daemon"

if [ "$krabb_home" = true ]; then
    remove "$home/cache"

    for staged in "$home"/updates/v*; do
        remove "$staged"
    done
elif [ -e "$home" ]; then
    note kept "$home, because it does not look like a Krabb home"
fi

if [ "$purge" = true ]; then
    # Stores at custom paths are files Krabb created; folders such as KRABB_ROOT are not.
    for variable in KRABB_DB KRABB_MEMORY KRABB_CREDENTIALS KRABB_CREDENTIALS_KEY KRABB_TIMER; do
        store "$(path "$variable")"
    done

    [ "$krabb_home" = false ] || remove "$home"

    if [ -n "$entry" ]; then
        case "$system" in
            Darwin)
                if security delete-generic-password -s "$label" -a "$entry" > /dev/null 2>&1
                then
                    note removed "the $entry keychain entry"
                fi
                ;;

            Linux)
                if command -v secret-tool > /dev/null 2>&1 &&
                    secret-tool clear service "$label" username "$entry" 2>/dev/null; then
                    note removed "the $entry keyring entry"
                else
                    note kept "the $entry keyring entry; remove it with your keyring manager"
                fi
                ;;
        esac
    fi
fi

for variable in KRABB_ROOT KRABB_MEDIA KRABB_CODEX_HOME; do
    value=$(path "$variable")
    [ -z "$value" ] || [ ! -e "$value" ] || note kept "$value from $variable"
done

if [ -n "$removed" ]; then
    info "Removed:$removed"
else
    info 'Krabb was not found, so nothing was removed.'
fi

[ -z "$kept" ] || info "Kept:$kept"

if [ "$purge" = false ] && [ "$krabb_home" = true ]; then
    info "Kept $home with your plugins, settings, credentials, and conversations."
    info 'Remove them too with `uninstall.sh --purge`.'
fi

info 'Remove KRABB_* secrets set in your shell profile, and revoke keys where they were issued.'
