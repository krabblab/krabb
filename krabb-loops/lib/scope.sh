#!/usr/bin/env bash

scope_git() {
    python3 "$LOOP_DIR/lib/process.py" "$VERIFICATION_TIMEOUT" \
        git -c core.fsmonitor=false -c core.hooksPath=/dev/null -c core.pager=cat \
        -C "$REPO_ROOT" "$@"
}

scope_paths=(. ':(exclude).krabb/loops' ':(exclude).revloop' ':(exclude)target')
CURRENT_STATUS=$(scope_git status --porcelain --untracked-files=all -- "${scope_paths[@]}") || \
    die 74 'Unable to inspect repository status.'
if [[ "$GLOBAL_REVIEW" == true ]]; then
    REVIEW_SCOPE='global repository-wide audit'
elif [[ -n "$CURRENT_STATUS" ]]; then
    REVIEW_SCOPE='uncommitted working-tree changes (staged, unstaged, and untracked files)'
else
    CURRENT_BRANCH=$(scope_git branch --show-current) || die 74 'Unable to inspect branch.'
    REVIEW_SCOPE='latest commit on the current revision (HEAD^..HEAD)'
    if [[ "$CURRENT_BRANCH" == main ]]; then
        REVIEW_SCOPE='latest commit on main (HEAD^..HEAD)'
    elif [[ -n "$CURRENT_BRANCH" ]]; then
        base=''
        if scope_git show-ref --verify --quiet refs/heads/main; then
            base=main
        elif scope_git show-ref --verify --quiet refs/remotes/origin/main; then
            base=origin/main
        fi
        if [[ -n "$base" ]]; then
            diff_status=0
            scope_git diff --quiet --no-ext-diff --no-textconv "$base...HEAD" \
                -- "${scope_paths[@]}" || diff_status=$?
            case "$diff_status" in
                0) REVIEW_SCOPE='latest commit because the feature branch has no changes' ;;
                1) REVIEW_SCOPE="feature branch '$CURRENT_BRANCH' against $base ($base...HEAD)" ;;
                *) die 74 'Unable to compare the feature branch with its base.' ;;
            esac
        fi
    fi
fi

snapshot_scope() {
    local destination="$RUN_DIR/scope-details.txt" base=''
    local paths=(. ':(exclude).krabb' ':(exclude).revloop' ':(exclude)target'
        ':(exclude)krabb-ci/.cache' ':(exclude)krabb-ci/.artifacts'
        ':(exclude)krabb-ci/.target' ':(exclude).env' ':(exclude).env.*')
    local git_command=(scope_git)

    case "$REVIEW_SCOPE" in
        feature*origin/main*) base=origin/main ;;
        feature*) base=main ;;
        latest*)
            if "${git_command[@]}" rev-parse --verify HEAD^ >/dev/null 2>&1; then
                base=HEAD^
            fi
            ;;
    esac

    {
        printf 'Scope: %s\n\nCurrent status (source only):\n' "$REVIEW_SCOPE"
        "${git_command[@]}" status --short --untracked-files=all -- "${paths[@]}"
        printf '\nCurrent tracked changes relative to HEAD:\n'
        if "${git_command[@]}" rev-parse --verify HEAD >/dev/null 2>&1; then
            "${git_command[@]}" diff --no-ext-diff --no-textconv HEAD -- "${paths[@]}"
        fi
        if [[ -n "$base" ]]; then
            printf '\nSelected committed changes:\n'
            "${git_command[@]}" diff --no-ext-diff --no-textconv "$base...HEAD" -- "${paths[@]}"
        fi
        printf '\nTracked source paths (read untracked files listed in status directly):\n'
        "${git_command[@]}" ls-files -- "${paths[@]}"
    } >"$destination"
}
