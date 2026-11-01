#!/usr/bin/env bash

verification_failure_kind() {
    local log_file=$1 status=$2
    local network='network is unreachable|failed to download|could not resolve host'
    network+='|advisory database'
    if (( status == 75 || status == 124 )); then
        printf 'timeout\n'
    elif grep -Eiq 'No space left on device|Cannot allocate memory|out of memory' "$log_file"; then
        printf 'resource\n'
    elif grep -Eiq 'test result: FAILED|error: test failed' "$log_file"; then
        printf 'test\n'
    elif grep -Eiq 'tool is missing|toolchain is not available|command not found' "$log_file"; then
        printf 'dependency\n'
    elif grep -Eiq "$network" \
        "$log_file"; then
        printf 'network\n'
    elif grep -Eiq 'read-only file system|permission denied|operation not permitted' \
        "$log_file"; then
        printf 'environment\n'
    elif grep -Eiq 'failed to parse manifest|invalid configuration|unknown field' "$log_file"; then
        printf 'configuration\n'
    else
        printf 'product\n'
    fi
}

record_verification_failure() {
    local kind
    kind=$(verification_failure_kind "$1" "$2")
    printf '%s\n' "$kind" >"$1.failure"
    print_warn "Verification failure classification: $kind. Evidence: $1"
}

run_verification_command() {
    local label=$1
    shift
    local status

    if python3 "$LOOP_DIR/lib/process.py" "$VERIFICATION_TIMEOUT" "$@"; then
        status=0
    else
        status=$?
    fi

    if (( status == 124 )); then
        print_warn "$label timed out after $VERIFICATION_TIMEOUT."
        return "$EXIT_VERIFICATION_TIMEOUT"
    fi

    return "$status"
}

run_fast_verification() {
    local log_file=$1
    local status
    local pipeline_status
    local sink_status

    print_info 'Running focused verification.'

    set +e
    (
        set -e
        cd "$REPO_ROOT"

        print_info 'Running format check (1/4)...'
        run_verification_command 'Format check' make fmt || exit $?

        print_info 'Running workspace check (2/4)...'
        run_verification_command 'Workspace check' make check || exit $?

        print_info 'Running workspace tests (3/4)...'
        run_verification_command 'Workspace tests' make test || exit $?

        print_info 'Running shell syntax check (4/4)...'
        run_verification_command 'Shell syntax check' \
            bash "$LOOP_DIR/lib/syntax.sh" "$REPO_ROOT" || exit $?
    ) </dev/null 2>&1 | write_log "$log_file"
    pipeline_status=("${PIPESTATUS[@]}")
    status=${pipeline_status[0]}
    sink_status=${pipeline_status[1]}
    set -e

    if (( status == 0 && sink_status != 0 )); then
        status=$sink_status
    fi

    if (( status != 0 )); then
        record_verification_failure "$log_file" "$status"
        if (( status == EXIT_VERIFICATION_TIMEOUT )); then
            print_warn 'Focused verification stopped because a phase timed out.'
            print_warn "Verification log: $log_file"
        else
            print_error "Focused verification failed with exit status $status."
            print_error "Verification log: $log_file"
        fi
        return "$status"
    fi
}

run_fast_verification_with_retry() {
    local log_file=$1
    local status

    if run_fast_verification "$log_file"; then
        return 0
    else
        status=$?
    fi

    if (( status != EXIT_VERIFICATION_TIMEOUT )); then
        return "$status"
    fi

    cp "$log_file" "$(mktemp "$log_file.attempt-XXXXXX")"
    print_warn 'Focused verification timed out; retrying once without consuming a cycle.'

    if run_fast_verification "$log_file"; then
        return 0
    else
        status=$?
    fi

    if (( status == EXIT_VERIFICATION_TIMEOUT )); then
        print_warn 'Focused verification timed out twice.'
    fi

    return "$status"
}

run_final_verification() {
    local log_file=$1
    local status
    local environment_failure_pattern
    local coverage_environment_pattern
    local pipeline_status
    local sink_status

    environment_failure_pattern='failed to acquire advisory database'
    environment_failure_pattern+='|couldn.t fetch advisory database'
    environment_failure_pattern+='|network is unreachable'
    environment_failure_pattern+='|request could not be completed in the allotted timeframe'
    environment_failure_pattern+='|couldn.t check if the package is yanked'
    environment_failure_pattern+='|read-only path'
    coverage_environment_pattern='failed to remove file.*permission denied'

    print_info 'Running final CI-equivalent verification.'

    set +e
    (
        set -e
        cd "$REPO_ROOT"

        for required_tool in \
            cargo-llvm-cov \
            agentskill \
            cargo-deny \
            cargo-audit
        do
            if ! command -v "$required_tool" >/dev/null 2>&1; then
                print_error "Required final verification tool is missing: $required_tool."
                exit "$EXIT_VERIFICATION"
            fi
        done

        toolchain_status=0
        run_verification_command 'Rust 1.89 availability' cargo +1.89 --version || \
            toolchain_status=$?
        if (( toolchain_status == EXIT_VERIFICATION_TIMEOUT )); then
            exit "$toolchain_status"
        elif (( toolchain_status != 0 )); then
            print_error 'Required Rust 1.89 toolchain is not available.'
            exit "$EXIT_VERIFICATION"
        fi

        print_info 'Running complete verification (1/10)...'
        run_verification_command 'Complete verification' make verify || exit $?

        print_info 'Running Rustdoc check (2/10)...'
        run_verification_command \
            'Rustdoc check' \
            env RUSTDOCFLAGS='-D warnings' \
            cargo doc \
            --workspace \
            --no-deps \
            --locked || exit $?

        print_info 'Running shell syntax check (3/10)...'
        run_verification_command \
            'Shell syntax check' \
            bash "$LOOP_DIR/lib/syntax.sh" "$REPO_ROOT" || exit $?

        print_info 'Running all-target workspace check (4/10)...'
        run_verification_command \
            'All-target workspace check' \
            env CARGO_BUILD_JOBS=4 \
            cargo check \
            --workspace \
            --all-targets \
            --locked || exit $?

        print_info 'Running MSRV workspace check (5/10)...'
        run_verification_command \
            'MSRV workspace check' \
            env CARGO_BUILD_JOBS=4 \
            cargo +1.89 check \
            --workspace \
            --all-targets \
            --locked || exit $?

        print_info 'Running release build (6/10)...'
        run_verification_command \
            'Release build' \
            cargo build \
            --workspace \
            --release \
            --locked || exit $?

        print_info 'Running release smoke test (7/10)...'
        run_verification_command \
            'Release smoke test' \
            cargo run \
            -p krabb \
            --bin krabb \
            --release \
            -- \
            version || exit $?

        print_info 'Running Agentskill document check (8/10)...'
        run_verification_command \
            'Agentskill document check' \
            agentskill validate . --signature auto || exit $?

        print_info 'Running dependency policy check (9/10)...'
        run_verification_command 'Dependency policy check' cargo deny check || exit $?

        print_info 'Running RustSec audit (10/10)...'
        run_verification_command 'RustSec audit' cargo audit || exit $?
    ) </dev/null 2>&1 | write_log "$log_file"
    pipeline_status=("${PIPESTATUS[@]}")
    status=${pipeline_status[0]}
    sink_status=${pipeline_status[1]}
    set -e

    if (( status == 0 && sink_status != 0 )); then
        status=$sink_status
    fi

    if (( status != 0 )); then
        record_verification_failure "$log_file" "$status"
        if (( status == EXIT_VERIFICATION_TIMEOUT )); then
            print_warn 'Final verification stopped because a phase timed out.'
            print_warn "Verification log: $log_file"
            return "$status"
        fi

        if grep -Eiq "$coverage_environment_pattern" "$log_file"; then
            print_warn \
                'Coverage unavailable: existing build artifacts could not be removed.'
            print_warn "Verification log: $log_file"
            return "$EXIT_VERIFICATION"
        fi

        if grep -Eiq "$environment_failure_pattern" "$log_file" && \
            grep -Eiq \
                'Running dependency policy check|Running RustSec audit|advisory database' \
                "$log_file"
        then
            print_warn \
                'Dependency audit unavailable: the advisory database could not be accessed.'
            print_warn "Verification log: $log_file"
            return "$EXIT_VERIFICATION"
        fi

        if (( status == EXIT_VERIFICATION )); then
            print_error \
                'Final verification could not run in the current environment.'
            print_error "Verification log: $log_file"
            return "$EXIT_VERIFICATION"
        fi

        print_error "Final verification failed with exit status $status."
        print_error "Verification log: $log_file"
        return "$status"
    fi
}

run_final_verification_with_retry() {
    local log_file=$1
    local status

    if run_final_verification "$log_file"; then
        return 0
    else
        status=$?
    fi

    if (( status != EXIT_VERIFICATION_TIMEOUT )); then
        return "$status"
    fi

    cp "$log_file" "$(mktemp "$log_file.attempt-XXXXXX")"
    print_warn 'Final verification timed out; retrying once without consuming a cycle.'

    if run_final_verification "$log_file"; then
        return 0
    else
        status=$?
    fi

    if (( status == EXIT_VERIFICATION_TIMEOUT )); then
        print_warn 'Final verification timed out twice.'
    fi

    return "$status"
}
