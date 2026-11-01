#!/usr/bin/env bash

setting() {
    local suffix=$1 fallback=$2 specific="KRABB_${LOOP^^}LOOP_$1" shared="KRABB_LOOP_$1"
    printf '%s' "${!specific:-${!shared:-$fallback}}"
}

usage() {
    printf '%s\n' \
        "Usage: krabb-loops/${LOOP}loop.sh [--global] [--clean|--verbose]" \
        '       [--fast|--no-fast] [--agent auto|codex|claude] [--help]' \
        'Scope: uncommitted changes, feature branch, latest commit; --global audits all source.' \
        'Shared settings: KRABB_LOOP_AGENT, MODEL, REASONING, FAST, OUTPUT,' \
        'MAX_CYCLES, CLEAN_PASSES, TIMEOUT, VERIFICATION_TIMEOUT (prefix each with KRABB_LOOP_).' \
        "Loop overrides: KRABB_${LOOP^^}LOOP_*; defaults: auto," \
        'unlimited cycles (MAX_CYCLES=0), 3 clean passes,' \
        '60m agent timeout, 30m verification timeout, clean output, fast disabled.' \
        'Codex defaults: gpt-6-astra, high reasoning; KRABB_CODEX_HOME is supported.' \
        'Claude defaults: claude-fable-5-1, backend effort; fast=true is unsupported.'
}

configure() {
    LOOP=${LOOP:-rev}
    AGENT=$(setting AGENT auto)
    MAX_CYCLES=$(setting MAX_CYCLES 0)
    CLEAN_PASSES=$(setting CLEAN_PASSES 3)
    AGENT_TIMEOUT=$(setting TIMEOUT 60m)
    if [[ "$LOOP" == rev && -n "${KRABB_REVLOOP_CODEX_TIMEOUT:-}" &&
        -z "${KRABB_REVLOOP_TIMEOUT:-}" ]]; then
        AGENT_TIMEOUT=$KRABB_REVLOOP_CODEX_TIMEOUT
    fi
    VERIFICATION_TIMEOUT=$(setting VERIFICATION_TIMEOUT 30m)
    OUTPUT_MODE=$(setting OUTPUT clean)
    MODEL=$(setting MODEL '')
    REASONING=$(setting REASONING '')
    FAST_MODE=$(setting FAST false)
    GLOBAL_REVIEW=false

    while (($#)); do
        case "$1" in
            --global) GLOBAL_REVIEW=true ;;
            --clean) OUTPUT_MODE=clean ;;
            --verbose) OUTPUT_MODE=verbose ;;
            --fast) FAST_MODE=true ;;
            --no-fast) FAST_MODE=false ;;
            --agent)
                (($# >= 2)) || die 2 '--agent requires a backend name.'
                AGENT=$2
                shift
                ;;
            --help|-h) usage; exit 0 ;;
            *) die 2 "Unknown option '$1'. Use --help." ;;
        esac
        shift
    done

    case "$OUTPUT_MODE" in clean|verbose) ;; *) die 2 'OUTPUT must be clean or verbose.' ;; esac
    case "$FAST_MODE" in true|false) ;; *) die 2 'FAST must be true or false.' ;; esac
    FAST_MODE_LABEL=$FAST_MODE
    [[ "$MAX_CYCLES" =~ ^(0|[1-9][0-9]{0,3})$ ]] || \
        die 2 'MAX_CYCLES must be 0 (unlimited) or 1 through 9999.'
    [[ "$CLEAN_PASSES" =~ ^[1-9][0-9]{0,3}$ ]] || die 2 'CLEAN_PASSES must be 1 through 9999.'
    [[ "$MODEL" != *$'\n'* ]] || die 2 'MODEL must be a single line.'
}
