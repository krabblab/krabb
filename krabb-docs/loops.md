# Engineering Loops

Each loop alternates a fresh independent reviewer with a worker that investigates,
fixes, and tests findings. Only independent reviews and host verification establish success.

| Loop | Purpose |
| --- | --- |
| `revloop` | Fix material correctness defects and regressions |
| `secloop` | Fix reachable security weaknesses |
| `testloop` | Protect important behavior and fix confirmed flaky tests |

```bash
make revloop REVLOOP_ARGS="--global,--agent,codex"
make secloop SECLOOP_ARGS="--global,--agent,claude"
make testloop TESTLOOP_ARGS="--agent,codex"
```

Make arguments are comma-separated. Direct scripts accept ordinary shell arguments:
`bash krabb-loops/testloop.sh --global --agent codex`. All loops support `--global`,
`--clean`, `--verbose`, `--fast`, `--no-fast`, `--agent`, and `--help`.
The legacy `krabb-scripts/revloop.sh` delegates to the shared implementation.

## Scope And Completion

Default scope is uncommitted changes, otherwise the branch diff against local `main`
or `origin/main`, otherwise the latest commit. `--global` reviews the whole repository.
Workers preserve unrelated changes and establish a defect before changing production code.
All loops follow the shared [Rust conventions](../krabb-loops/prompts/rust.md) and
[test organization](architecture.md#test-organization).

Loops run until three consecutive independent reviews have no blocking findings or
unresolved investigations, with comprehensive verification passing after the last worker.
Findings, unanswered questions, and product/test failures trigger further worker cycles.
Read-only reviewers delegate reproductions requiring writes to workers and build gates
to the host; those restrictions alone are not findings. Each review receives the latest
evidence. Non-blocking findings remain visible.

Security reports require a reachable attack path, evidence, impact, and regression checks.
Test reports require a concrete contract or reproducible intermittent failure. Workers
investigate synchronization, shared state, ordering, clocks, and external dependencies,
then repeat affected tests under the relevant conditions. Blind retries, sleeps, ignored
tests, weakened assertions, and coverage-only changes do not establish success.

Execution failures, missing prerequisites, interruptions, and explicit cycle budgets return
non-success. They never count as clean convergence. Loops do not commit, push, or publish.

## Requirements And Backends

Use Linux or compatible WSL with `/proc`, Bash 4+, Python 3, Git, GNU coreutils, Cargo,
Make, and an authenticated backend. Only Codex and Claude Code are supported;
`auto` selects Codex first, then Claude. Incompatible CLIs fail explicitly.

- **Codex:** fresh ephemeral sessions with native read-only/workspace-write containment.
  A local sandbox preflight checks repository reads. MCP servers, hooks, apps, plugins,
  browser tools, subagents, and interactive escalation are disabled.
- **Claude Code:** fresh restricted sessions without persistence or MCP tools. Reviewers
  receive Read/Glob/Grep; workers also receive Edit/Write. Shell and subagent tools are
  disabled. Reviewers require Landlock ABI 3+; verification runs in the host.

Final gates require the repository toolchain, including cargo-llvm-cov, cargo-deny,
cargo-audit, Agentskill, and Rust 1.89. They run `make verify`, documentation and shell
checks, all-target/MSRV builds, release smoke tests, and dependency audits. Agent calls
use your configured provider; automated product tests must not call paid provider APIs.

## Configuration

CLI flags override environment settings. Otherwise, `KRABB_REVLOOP_*`, `KRABB_SECLOOP_*`,
or `KRABB_TESTLOOP_*` overrides `KRABB_LOOP_*`, then defaults apply.

| Suffix | Default |
| --- | --- |
| `AGENT` | `auto` |
| `MODEL` | Codex: `gpt-6-astra`; Claude: `claude-fable-5-1` |
| `REASONING` | Codex: `high`; Claude: backend default |
| `MAX_CYCLES` | `0` (unlimited); positive values set an explicit budget |
| `CLEAN_PASSES` | `3` |
| `OUTPUT` | `clean`; `verbose` prints transcripts |
| `FAST` | `false`; enabled only for Codex |
| `TIMEOUT` | `60m` per agent invocation |
| `VERIFICATION_TIMEOUT` | `30m` per verification phase |

Durations accept positive integers with optional `s/m/h/d`. `KRABB_REVLOOP_CODEX_TIMEOUT`
is a legacy alias below loop-specific `TIMEOUT` and above the shared setting.
Unsupported explicit options fail. Model defaults are pinned; override `MODEL` to change them.

## Reports And Recovery

Private run artifacts live under `.krabb/loops/<rev|sec|test>/<run-id>/`.
Start with `latest-orchestrator.json`, `latest-worker.md`, and `latest-verification.log`.
Validated reports use `.json`; raw responses use `.response`, event streams `.jsonl`,
prompts/summaries `.md`, and transcripts `.log`. Plain-text configuration, scope, outcome,
and `latest-run.txt` pointers retain `.txt`. Historical artifacts keep their original names.
Invalid reports are preserved but cannot replace the latest validated report.

| Exit | Meaning |
| --- | --- |
| `0` | Verified clean completion |
| `2` | Invalid configuration or unsupported option |
| `70` | Agent unavailable, incompatible, failed, or timed out |
| `71` | Verification unavailable |
| `72` | Invalid reviewer report |
| `73` | Explicit cycle budget exhausted |
| `74` | Repository/state initialization failed |
| `75` | Verification timeout |
| `130` / `143` | Interrupted / terminated |

Verification timeouts retry once before bounded recovery; logs retain every attempt.
Missing final-gate tools stop immediately. Fix environment, network, or resource failures
before starting a new run; do not change production code or lower gates to conceal them.
Cleanup stops descendant processes. Review and redact diagnostics before sharing them.

## Validation Evidence

Run `make loops` for deterministic orchestration tests without provider calls, and
`make verify` for repository checks. To test installed Codex sandbox reads and denied
writes without a model call, run `KRABB_TEST_CODEX="$(command -v codex)" make loops`.

The [loop tests](../krabb-loops/tests/) cover scope, backend controls, report validation,
flaky-test repair, investigation handoff, clean-streak resets, verification recovery,
artifact formats, and process cleanup. They use fake agents and do not establish a live
model's diagnostic quality. Native Codex sandbox checks have passed; authenticated Claude
execution has not been verified in this environment.
