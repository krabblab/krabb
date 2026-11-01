#!/usr/bin/env bash

render_prompt() {
    local template=$1 destination=$2 line name token

    while IFS= read -r line || [[ -n "$line" ]]; do
        for name in REPO_ROOT STATE_ROOT REVIEW_SCOPE REVIEW_CLEAR REVIEW_FINDINGS \
            review_scope scope_guard verification_note review_file RUN_DIR \
            worker_note host_verification; do
            token="\$$name"
            line=${line//"$token"/"${!name-}"}
        done

        printf '%s\n' "$line"
    done <"$template" >"$destination"
}

write_orchestrator_prompt() {
    local prompt_file=$1
    local verification_file=${2:-}
    local verification_note
    local review_scope
    local scope_guard

    if [[ -n "$verification_file" ]]; then
        verification_note="$verification_file"
    else
        verification_note='None; no verification failure is currently pending.'
    fi

    case "$REVIEW_SCOPE" in
        global*)
            review_scope='Global mode is enabled. Audit the actual repository-wide
product code, scripts, documentation, manifests, release files, and CI. Include
unchanged code and the current working tree when assessing reachable material
defects.'
            scope_guard='In global mode, report relevant pre-existing defects in
untouched areas as well as defects introduced by current changes. Keep the audit
bounded to repository content and exclude generated artifacts listed above.'
            ;;
        uncommitted*)
            review_scope='Review the uncommitted working-tree change set as the
primary scope. Include staged and unstaged changes, untracked source files, and
behavior directly affected by them.'
            scope_guard='Do not turn this review into an unrestricted audit of the
entire repository. Do not report unrelated pre-existing defects in untouched
areas unless the current changes introduce them, expose them, materially worsen
them, depend on them, or make them directly relevant to the changed behavior.'
            ;;
        feature*)
            review_scope="""No uncommitted files are present. Review the complete
feature branch represented by $REVIEW_SCOPE, including its commits relative to
main and any behavior directly affected by those changes."""
            scope_guard='Do not turn this review into an unrestricted audit of the
entire repository. Do not report unrelated pre-existing defects outside the
feature branch unless the branch changes introduce them, expose them, materially
worsen them, depend on them, or make them directly relevant.'
            ;;
        *)
            review_scope="""No uncommitted files are present. Review the latest
commit represented by $REVIEW_SCOPE and behavior directly affected by it. If no
parent commit is available, inspect the current repository baseline as needed to
understand that commit."""
            scope_guard='Do not turn this review into an unrestricted audit of the
entire repository. Do not report unrelated pre-existing defects in untouched
areas unless the latest commit introduces them, exposes them, materially worsens
them, depends on them, or makes them directly relevant.'
            ;;
    esac

    render_prompt "$LOOP_DIR/prompts/$LOOP/orchestrator.md" "$prompt_file"
    append_context "$prompt_file"
    cat "$LOOP_DIR/prompts/report.md" >>"$prompt_file"
}

write_worker_prompt() {
    local prompt_file=$1
    local review_file=$2
    local verification_file=${3:-}
    local verification_note

    if [[ -n "$verification_file" ]]; then
        verification_note="$verification_file"
    else
        verification_note='None; no verification failure is currently pending.'
    fi

    local review_scope='' scope_guard='Keep edits tied to validated in-scope work.'
    render_prompt "$LOOP_DIR/prompts/$LOOP/worker.md" "$prompt_file"
    append_context "$prompt_file"
    render_prompt "$LOOP_DIR/prompts/worker.md" "$prompt_file.worker"
    cat "$prompt_file.worker" >>"$prompt_file"
    rm "$prompt_file.worker"
}

append_context() {
    local destination=$1
    local worker_note='None; no worker has run yet.'

    if [[ -s "$LATEST_WORKER" ]]; then
        worker_note="$LATEST_WORKER"
    fi

    render_prompt "$LOOP_DIR/prompts/context.md" "$destination.context"
    cat "$destination.context" >>"$destination"
    rm "$destination.context"
    cat "$LOOP_DIR/prompts/rust.md" >>"$destination"
}
