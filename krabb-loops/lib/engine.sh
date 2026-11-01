#!/usr/bin/env bash
set -euo pipefail

LOOP_DIR=$1
LOOP=$2
shift 2

readonly EXIT_USAGE=2
readonly EXIT_AGENT=70
readonly EXIT_VERIFICATION=71
readonly EXIT_ORCHESTRATOR_OUTPUT=72
readonly EXIT_EXHAUSTED=73
readonly EXIT_REPOSITORY=74
readonly EXIT_VERIFICATION_TIMEOUT=75
readonly REVIEW_CLEAR='Review is clean, so no findings will be listed.'
readonly REVIEW_FINDINGS='Review is not clean, findings are:'

source "$LOOP_DIR/lib/config.sh"

print_error() {
    printf '[ERROR] %s\n' "$*" >&2
}

print_info() {
    printf '[INFO] %s\n' "$*"
}

print_warn() {
    printf '[WARN] %s\n' "$*" >&2
}

die() {
    local status=$1
    shift
    print_error "$*"
    exit "$status"
}

configure "$@"

if ! SCRIPT_DIR="$(cd "$LOOP_DIR" && pwd -P)"; then
    die "$EXIT_REPOSITORY" 'Unable to determine the script directory.'
fi

if REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null)"; then
    if ! REPO_ROOT="$(cd "$REPO_ROOT" && pwd -P)"; then
        die "$EXIT_REPOSITORY" 'Unable to resolve the Git repository root.'
    fi
else
    if ! REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"; then
        die "$EXIT_REPOSITORY" 'Unable to resolve the repository root.'
    fi
fi

[[ -f "$REPO_ROOT/Cargo.toml" ]] || die "$EXIT_REPOSITORY" \
    "Repository root '$REPO_ROOT' has no Cargo.toml."

[[ -f "$REPO_ROOT/Makefile" ]] || die "$EXIT_REPOSITORY" \
    "Repository root '$REPO_ROOT' has no Makefile."

for required_tool in bash cargo make python3; do
    command -v "$required_tool" >/dev/null 2>&1 || die "$EXIT_VERIFICATION" \
        "Required tool '$required_tool' is not available on PATH."
done

TIMEOUT_BIN=$(command -v timeout || command -v gtimeout) || die "$EXIT_USAGE" \
    'GNU timeout is required (install coreutils on macOS).'
for duration in "$AGENT_TIMEOUT" "$VERIFICATION_TIMEOUT"; do
    [[ "$duration" =~ ^[1-9][0-9]*[smhd]?$ ]] || die "$EXIT_USAGE" \
        'Timeouts must be positive integer durations with an optional s/m/h/d suffix.'
    "$TIMEOUT_BIN" --kill-after=1s "$duration" true || die "$EXIT_USAGE" \
        'The timeout utility must support GNU timeout options.'
done

source "$LOOP_DIR/lib/agent.sh"
AGENT=$(agent_detect "$AGENT") || exit $?
agent_validate || exit $?

source "$LOOP_DIR/lib/scope.sh"

source "$LOOP_DIR/lib/state.sh"
initialize_state

write_log() {
    local log_file=$1

    if [[ "$OUTPUT_MODE" == 'verbose' ]]; then
        tee "$log_file"
        return
    fi

    : >"$log_file"

    while IFS= read -r line || [[ -n "$line" ]]; do
        printf '%s\n' "$line" >>"$log_file" || return 1

        case "$line" in
            '[INFO]'*|'[WARN]'*|'[ERROR]'*)
                printf '%s\n' "$line" || return 1
                ;;
        esac
    done
}

source "$LOOP_DIR/lib/prompt.sh"

source "$LOOP_DIR/lib/report.sh"

source "$LOOP_DIR/lib/verify.sh"

run_agent() {
    agent_run "$1" "$REPO_ROOT" "$3" "$2" "$MODEL" "$REASONING" \
        "$AGENT_TIMEOUT" "$4" "$5"
}

copy_latest_verification() {
    local log_file=$1

    cp "$log_file" "$RUN_DIR/latest-verification.log"
    host_verification="Verification failed; inspect $RUN_DIR/latest-verification.log."
}

run_worker_pass() {
    final_verification_pending=1
    local cycle=$1
    local review_file=$2
    local verification_file=${3:-}
    local artifact_name=${4:-worker}
    local cycle_tag
    local prompt_file
    local worker_file
    local worker_log

    printf -v cycle_tag '%02d' "$cycle"
    worker_sequence=$((worker_sequence + 1))
    artifact_name="${artifact_name}-${worker_sequence}"

    prompt_file="$RUN_DIR/cycle-${cycle_tag}-${artifact_name}-prompt.md"
    worker_file="$RUN_DIR/cycle-${cycle_tag}-${artifact_name}.md"
    worker_log="$RUN_DIR/cycle-${cycle_tag}-${artifact_name}.log"

    write_worker_prompt \
        "$prompt_file" \
        "$review_file" \
        "$verification_file"

    if ! run_agent \
        'worker' \
        'workspace-write' \
        "$prompt_file" \
        "$worker_file" \
        "$worker_log"
    then
        print_error "Worker output is preserved at $worker_file."
        return "$EXIT_AGENT"
    fi

    cp "$worker_file" "$LATEST_WORKER"
    cp "$worker_log" "$LATEST_WORKER_LOG"
}

report_exhausted() {
    local review_file=$1

    print_error "Did not converge within the configured maximum of $MAX_CYCLES cycles."
    print_error "Latest orchestrator output: $review_file"
    print_error 'Current working tree changes were preserved.'
    exit "$EXIT_EXHAUSTED"
}

pending_verification=''
host_verification='Not run yet; the host will run verification after worker work or a clear review.'
final_verification_pending=1
clean_passes=0
cycle=1
worker_sequence=0

print_info "Repository: $REPO_ROOT."
print_info "State: $RUN_DIR."
print_info "Agent: $AGENT."
print_info "Model: $MODEL."
print_info "Reasoning: $REASONING."
print_info "Fast mode: $FAST_MODE_LABEL."
print_info "Output mode: $OUTPUT_MODE."
print_info "Review scope: $REVIEW_SCOPE."
cycle_limit=$MAX_CYCLES
(( MAX_CYCLES > 0 )) || cycle_limit=unlimited
print_info "Maximum cycles: $cycle_limit."
print_info "Required consecutive clean reviews: $CLEAN_PASSES."
print_info "Verification timeout per phase: $VERIFICATION_TIMEOUT."

while (( MAX_CYCLES == 0 || cycle <= MAX_CYCLES )); do
    printf '\n'
    print_info "Cycle $cycle/$cycle_limit: Starting fresh orchestration."

    printf -v cycle_tag '%02d' "$cycle"

    review_prompt="$RUN_DIR/cycle-${cycle_tag}-orchestrator-prompt.md"
    review_response="$RUN_DIR/cycle-${cycle_tag}-orchestrator.response"
    review_file="$RUN_DIR/cycle-${cycle_tag}-orchestrator.json"
    review_log="$RUN_DIR/cycle-${cycle_tag}-orchestrator.log"

    snapshot_scope || die 74 "Unable to capture repository scope. See $RUN_DIR/scope-details.txt."

    write_orchestrator_prompt \
        "$review_prompt" \
        "$pending_verification"

    if ! run_agent \
        'orchestrator' \
        'read-only' \
        "$review_prompt" \
        "$review_response" \
        "$review_log"
    then
        if [[ -f "$review_log" ]]; then
            cp "$review_log" "$LATEST_ORCHESTRATOR_LOG"
        fi
        print_error \
            "Orchestrator response is preserved at $review_response and $review_log."
        exit "$EXIT_AGENT"
    fi

    cp "$review_log" "$LATEST_ORCHESTRATOR_LOG"

    report_status=0
    validate_review_output "$review_response" "$review_file" || report_status=$?
    if (( report_status != 0 )); then
        print_error "Orchestrator response is preserved at $review_response."
        exit "$report_status"
    fi

    cp "$review_file" "$LATEST_ORCHESTRATOR"

    if has_pending_work "$review_file"; then
        clean_passes=0
        print_info "Unresolved work: $REPORT_BLOCKERS blocker(s), $REPORT_LIMITATIONS investigation(s)."
    fi

    if ! has_pending_work "$review_file"; then
        if (( REPORT_COUNT == 0 )); then
            print_info "$REVIEW_CLEAR"
        else
            print_info \
                "No blocking findings remain; non-blocking findings are recorded in $review_file."
        fi

        if (( clean_passes > 0 )) && \
            (( final_verification_pending == 0 ));
        then
            clean_passes=$((clean_passes + 1))
            print_info \
                "Review remained without blocking findings ($clean_passes/$CLEAN_PASSES)."

            if (( clean_passes >= CLEAN_PASSES )); then
                print_info 'Review reached its clean stability threshold.'
                exit 0
            fi

            if (( cycle == MAX_CYCLES )); then
                report_exhausted "$review_file"
            fi

            cycle=$((cycle + 1))
            continue
        fi

        print_info 'Running final verification.'

        final_log="$RUN_DIR/cycle-${cycle_tag}-final-verification.log"

        if run_final_verification_with_retry "$final_log"; then
            cp "$final_log" "$RUN_DIR/latest-verification.log"
            pending_verification=''
            host_verification="Full verification passed; see $RUN_DIR/latest-verification.log."
            final_verification_pending=0
            clean_passes=1

            if (( REPORT_COUNT == 0 )); then
                print_info "Review is clean (1/$CLEAN_PASSES); starting a stability recheck."
            else
                print_info \
                    "No blockers (1/$CLEAN_PASSES); starting a stability recheck."
            fi

            if (( clean_passes >= CLEAN_PASSES )); then
                print_info 'Review reached its clean stability threshold.'
                exit 0
            fi

            if (( cycle == MAX_CYCLES )); then
                report_exhausted "$review_file"
            fi

            cycle=$((cycle + 1))
            continue
        else
            final_status=$?
        fi

        if (( final_status == EXIT_VERIFICATION_TIMEOUT )); then
            print_warn \
                'Final verification timed out twice; running recovery within this cycle.'
            print_warn "Verification log: $final_log"
        elif (( final_status == EXIT_VERIFICATION )); then
            copy_latest_verification "$final_log"
            die 71 "Final verification unavailable. Correct the environment; see $final_log."
        fi

        copy_latest_verification "$final_log"
        pending_verification="$final_log"
        final_verification_pending=1
        clean_passes=0

        print_info 'Final verification failure is being sent to a worker.'

        worker_artifact='worker'
        if (( final_status == EXIT_VERIFICATION_TIMEOUT )); then
            worker_artifact='timeout-recovery-worker'
        fi

        if ! run_worker_pass \
            "$cycle" \
            "$review_file" \
            "$pending_verification" \
            "$worker_artifact"
        then
            exit "$EXIT_AGENT"
        fi
    else
        print_info "Review has unresolved work; evidence is recorded in $review_file."
        print_info 'Running worker.'

        if ! run_worker_pass \
            "$cycle" \
            "$review_file" \
            "$pending_verification"
        then
            exit "$EXIT_AGENT"
        fi
    fi

    verification_log="$RUN_DIR/cycle-${cycle_tag}-focused-verification.log"

    focused_status=0
    if run_fast_verification_with_retry "$verification_log"; then
        focused_status=0
    else
        focused_status=$?
    fi

    if (( focused_status == EXIT_VERIFICATION_TIMEOUT )); then
        copy_latest_verification "$verification_log"
        pending_verification="$verification_log"
        print_warn \
            'Focused verification timed out twice; running recovery within this cycle.'
        print_warn "Verification log: $verification_log"

        if ! run_worker_pass \
            "$cycle" \
            "$review_file" \
            "$pending_verification" \
            'timeout-recovery-worker'
        then
            exit "$EXIT_AGENT"
        fi

        cp "$verification_log" "$(mktemp "$verification_log.recovery-XXXXXX")"
        print_info 'Re-running focused verification after the timeout recovery worker.'
        if run_fast_verification_with_retry "$verification_log"; then
            focused_status=0
        else
            focused_status=$?
        fi
    fi

    if (( focused_status == 0 )); then
        cp "$verification_log" "$RUN_DIR/latest-verification.log"
        if (( final_verification_pending == 0 )); then
            pending_verification=''
        fi

        host_verification="Focused verification passed; see $RUN_DIR/latest-verification.log."
        print_info 'Focused verification passed.'
    else
        copy_latest_verification "$verification_log"
        pending_verification="$verification_log"

        print_info \
            'Continuing so a fresh orchestrator and worker can assess the verification failure.'
    fi

    if (( cycle == MAX_CYCLES )); then
        report_exhausted "$review_file"
    fi

    cycle=$((cycle + 1))
done

report_exhausted "$LATEST_ORCHESTRATOR"
