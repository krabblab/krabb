#!/usr/bin/env bash

# The engine passes role, directory, prompt, mode, model, reasoning, timeout, output, log.
agent_detect() {
    local requested=$1 candidate

    case "$requested" in
        auto)
            for candidate in codex claude; do
                if command -v "$candidate" >/dev/null 2>&1; then
                    printf '%s\n' "$candidate"
                    return
                fi
            done
            print_error 'No supported agent found. Install Codex or Claude Code.'
            return 70
            ;;
        codex|claude)
            command -v "$requested" >/dev/null 2>&1 || {
                print_error "Agent '$requested' is not installed on PATH."
                return 70
            }
            printf '%s\n' "$requested"
            ;;
        *)
            print_error "Unsupported agent '$requested'. Supported: auto, codex, claude."
            return 2
            ;;
    esac
}

agent_supports() {
    case "$AGENT:$1" in
        codex:read-only|codex:workspace-write|codex:model|codex:reasoning|codex:fast|\
        claude:read-only|claude:workspace-write|claude:model|claude:reasoning)
            return 0
            ;;
        *) return 1 ;;
    esac
}

agent_validate() {
    local help flag
    local required=()

    if [[ "$AGENT" == codex ]]; then
        MODEL=${MODEL:-gpt-6-astra}
        REASONING=${REASONING:-high}
        if [[ -z "${KRABB_CODEX_HOME:-}" && -z "${HOME:-}" ]]; then
            print_error 'Set HOME or KRABB_CODEX_HOME for Codex authentication.'
            return 70
        fi
        KRABB_CODEX_HOME=${KRABB_CODEX_HOME:-$HOME/.codex}
    elif [[ "$AGENT" == claude ]]; then
        MODEL=${MODEL:-claude-fable-5-1}
    fi

    if [[ "$FAST_MODE" == true ]] && ! agent_supports fast; then
        print_error "Agent '$AGENT' does not support the requested fast mode."
        return 2
    fi

    case "$AGENT" in
        codex)
            case "$REASONING" in
                low|medium|high|xhigh) ;;
                *) print_error 'Codex reasoning must be low, medium, high, or xhigh.'; return 2 ;;
            esac
            help=$(python3 "$LOOP_DIR/lib/process.py" 15s codex exec --help 2>&1) || {
                print_error 'Codex help probe failed or timed out.'
                return 70
            }
            required=(--sandbox --ephemeral --output-last-message --ignore-rules)
            ;;
        claude)
            case "$REASONING" in
                ''|low|medium|high|xhigh|max) ;;
                *) print_error 'Unsupported Claude reasoning effort.'; return 2 ;;
            esac
            help=$(python3 "$LOOP_DIR/lib/process.py" 15s claude --help 2>&1) || {
                print_error 'Claude help probe failed or timed out.'
                return 70
            }
            required=(--restricted --safe-mode --tools --strict-mcp-config
                --no-session-persistence --permission-mode --output-format --settings)
            [[ -z "$REASONING" ]] || required+=(--effort)
            ;;
    esac

    for flag in "${required[@]}"; do
        if [[ "$help" != *"$flag"* ]]; then
            print_error "Agent '$AGENT' lacks required control $flag; update its CLI."
            return 70
        fi
    done
}

agent_run() {
    local role=$1 directory=$2 prompt=$3 mode=$4 model=$5 reasoning=$6
    local duration=$7 output=$8 log=$9
    local status=0 tier=default raw="${output%.*}.jsonl" tools_list='Read,Glob,Grep'
    local command=()
    local settings='{"disableAllHooks":true,"permissions":{"deny":['
    settings+='"Edit(.git)","Edit(.git/**)","Edit(AGENTS.md)","Edit(.krabb/loops/**)"]}}'

    case "$role:$mode" in
        orchestrator:read-only|worker:workspace-write) ;;
        *) print_error 'Invalid agent role/permission combination.'; return 70 ;;
    esac

    case "$AGENT" in
        codex)
            [[ "$FAST_MODE" != true ]] || tier=fast
            command=(env "CODEX_HOME=$KRABB_CODEX_HOME"
                python3 "$LOOP_DIR/lib/codex.py" codex exec
                --cd "$directory" --sandbox "$mode" --ignore-rules
                --config 'approval_policy="never"'
                --config "model_reasoning_effort=\"$reasoning\""
                --config "service_tier=\"$tier\"" --config "features.fast_mode=$FAST_MODE"
                --config 'sandbox_workspace_write.network_access=false'
                --config 'sandbox_workspace_write.writable_roots=[]'
                --disable hooks --disable apps --disable plugins --disable multi_agent
                --disable browser_use --disable browser_use_external
                --ephemeral --color never --output-last-message "$output")
            [[ -z "$model" ]] || command+=(--model "$model")
            command+=(-)
            ;;
        claude)
            [[ "$mode" == read-only ]] || tools_list+=',Edit,Write'
            # Restrict actual available tools, not just automatic approval rules.
            # No shell, MCP, hooks, subagents or session continuation can escape this set.
            command=(claude --print --restricted --safe-mode --no-session-persistence
                --strict-mcp-config --mcp-config '{"mcpServers":{}}'
                --tools "$tools_list" --disallowedTools 'mcp__*'
                --permission-mode dontAsk --allowedTools "$tools_list"
                --settings "$settings"
                --output-format stream-json --verbose)
            [[ -z "$model" ]] || command+=(--model "$model")
            [[ -z "$reasoning" ]] || command+=(--effort "$reasoning")
            ;;
    esac

    if [[ "$AGENT" == claude && "$mode" == read-only ]]; then
        : >"$output"
        command=(python3 "$LOOP_DIR/lib/readonly.py" "$directory" "$output" "${command[@]}")
    fi

    print_info "Starting $role invocation ($AGENT, $mode)."

    (
        cd "$directory" || exit 74
        if [[ "$AGENT" == claude ]]; then
            python3 "$LOOP_DIR/lib/process.py" "$duration" "${command[@]}" \
                <"$prompt" >"$raw" 2>"$log"
        else
            python3 "$LOOP_DIR/lib/process.py" "$duration" "${command[@]}" \
                <"$prompt" >"$log" 2>&1
        fi
    ) || status=$?

    if [[ "$AGENT" == claude && -f "$raw" ]]; then
        cat "$raw" >>"$log"
    fi

    if [[ "$AGENT" == claude && "$status" == 0 ]]; then
        python3 "$LOOP_DIR/lib/response.py" "$raw" "$output" 2>>"$log" || status=70
    fi

    [[ "$OUTPUT_MODE" != verbose ]] || cat "$log"

    if (( status != 0 )); then
        print_error "$role failed (status $status; timeout statuses: 124/137). Log: $log"
        return 70
    fi

    if [[ ! -s "$output" ]]; then
        print_error "$role produced no final response. Log: $log"
        return 70
    fi
}
