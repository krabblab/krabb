#!/usr/bin/env bash

initialize_state() {
    local directory pointer
    umask 077
    STATE_ROOT="$REPO_ROOT/.krabb/loops/$LOOP"

    for directory in "$REPO_ROOT/.krabb" "$REPO_ROOT/.krabb/loops" "$STATE_ROOT"; do
        [[ ! -L "$directory" ]] || die 74 "Refusing symlinked state directory: $directory"
        mkdir -p "$directory" || die 74 'Unable to create private loop state.'
    done

    chmod 700 "$REPO_ROOT/.krabb/loops" "$STATE_ROOT"
    RUN_DIR=$(mktemp -d "$STATE_ROOT/$(date -u '+%Y%m%dT%H%M%SZ')-XXXXXX") || \
        die 74 'Unable to create a unique run directory.'
    RUN_ID=${RUN_DIR##*/}
    pointer=$(mktemp "$STATE_ROOT/.latest-XXXXXX")
    printf '%s\n' "$RUN_DIR" >"$pointer"
    mv -T "$pointer" "$STATE_ROOT/latest-run.txt"
    LATEST_ORCHESTRATOR="$RUN_DIR/latest-orchestrator.json"
    LATEST_ORCHESTRATOR_LOG="$RUN_DIR/latest-orchestrator.log"
    LATEST_WORKER="$RUN_DIR/latest-worker.md"
    LATEST_WORKER_LOG="$RUN_DIR/latest-worker.log"

    printf '%s\n' "loop=$LOOP" "agent=$AGENT" "model=$MODEL" "reasoning=$REASONING" \
        "fast=$FAST_MODE" "output=$OUTPUT_MODE" "max_cycles=$MAX_CYCLES" \
        "clean_passes=$CLEAN_PASSES" "agent_timeout=$AGENT_TIMEOUT" \
        "verification_timeout=$VERIFICATION_TIMEOUT" >"$RUN_DIR/config.txt"

    printf '%s\n' "$REVIEW_SCOPE" >"$RUN_DIR/scope.txt"
    trap 'finish_run $?' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
}

finish_run() {
    local status=$1
    printf 'exit=%s\ncycle=%s\nclean_passes=%s\n' "$status" "${cycle:-0}" \
        "${clean_passes:-0}" >"$RUN_DIR/outcome.txt"
}
