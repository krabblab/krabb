# Engineering Loops

The review loops alternate a fresh independent reviewer with a worker that investigates,
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
`bash krabb-loops/testloop.sh --global --agent codex`. Review loops support `--global`,
`--clean`, `--verbose`, `--fast`, `--no-fast`, `--agent`, and `--help`.
The legacy `krabb-scripts/revloop.sh` delegates to the shared implementation.

## Scope And Completion

Default scope is uncommitted changes, otherwise the branch diff against local `main`
or `origin/main`, otherwise the latest commit. `--global` reviews the whole repository.
Workers preserve unrelated changes and establish a defect before changing production code.
Review loops follow the shared [Rust conventions](../krabb-loops/prompts/rust.md) and
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
non-success. They never count as clean convergence. Review loops do not commit, push, or publish.

## Contribution Loop

`make contribloop` handles the contribution after implementation: run `make verify`,
repair failures, create a branch, commit with a lowercase conventional title without a
scope, push to `origin`, and create a draft PR. It reads recent merged PRs for writing
style and fills the repository template while preserving its guidance comments. The PR
title summarizes the complete change; the authenticated GitHub user becomes its assignee,
and the change type selects an existing label. It does not invent linked issues,
milestones or reviewers; those remain available for a maintainer to add.

Invoking this command authorizes staging and committing **all uncommitted changes**,
pushing the feature branch and creating or updating its PR. Run it in a dedicated
working tree without concurrent edits. It never force-pushes or merges. New branches
use a short, single-word name inferred from the full contribution diff, such as
`multiprovider`. The read-only naming agent considers staged, unstaged and untracked
changes. Names contain only lowercase letters; invalid or occupied names trigger another
inference, using the shared recovery budget. Local and remote branch names are reserved,
including names used as branch prefixes. An existing open PR on the current branch is resumed.
To choose a name, use `make contribloop CONTRIBLOOP_ARGS="--branch,rt"`, or
`bash krabb-loops/contribloop.sh --branch rt`. An existing named branch must already be
checked out. A branch whose tree matches the fetched default branch starts from that
default branch, avoiding duplicate commits after a squash merge.

The host waits for the `main.yml` pull-request workflow for the exact pushed commit
and every reported PR check. Missing or pending checks cannot count as success. A failed
CI run is saved as evidence for a restricted worker, followed by local verification,
a repair commit, another push, and another CI wait. This repeats until green. Only then
does the host mark the PR ready for review and exit successfully. Skipped conditional
jobs are permitted only inside a successful workflow. Merge conflicts, external head
changes, missing credentials or other host-operation blockers return
nonzero and preserve the evidence; they never produce a ready result. Green describes
the checked commit at completion, not future pushes or future changes to the base branch.

Recoverable local failures also stay inside the loop. Staging new files preserves their
verification fingerprint. Content changes during verification, metadata preparation,
staging, commit hooks or the CI wait trigger fresh verification before publication.
Failed commit hooks retain logs for the repair worker; hooks that already changed files
trigger verification of those changes first. Invalid PR metadata is regenerated with
validation feedback. A worker that makes no changes prints its explanation and retries
with that feedback and the original evidence, using the same repair budget. No-change
responses cannot mark the PR ready. These recovery cycles share the optional repair budget
with local test and CI repairs. Branch changes, external commits and agent index mutations remain
blockers; recovery never bypasses verification or publishes stale content.

When Agentskill is installed, local verification first runs
`agentskill validate . --signature auto`. Contribution workers may correct factual paths,
links and scope metadata in managed guidance files when validation fails; policies,
restrictions and signatures must remain intact. CI guidance validation remains required.

The contribution loop requires the same Linux process supervision and authenticated
Codex or Claude backend as the review loops, plus authenticated `gh` and push access
to the GitHub `origin`. Agents reuse the existing restricted runner; only the host
runs verification and Git/GitHub mutations. Configure the backend with
`KRABB_CONTRIBLOOP_AGENT=codex` or `claude` and the shared `KRABB_LOOP_*` model settings.
Agent timeout defaults to 60 minutes and each local verification has 30 minutes.
`--max-repairs N` optionally limits recovery cycles across all phases (zero means unlimited);
`--ci-timeout SECONDS` limits each CI wait (default 3600). Pending CI is checked immediately,
then every five minutes. Use `--ci-interval SECONDS` to adjust that interval (default 300),
for example `make contribloop CONTRIBLOOP_ARGS="--ci-interval,180"` for three minutes.
Polling sleeps are capped at the remaining CI timeout. The contribution script has its
own options, shown by `--help`; review-loop flags such as `--global` do not apply.
Artifacts live under ignored `.krabb/loops/contrib/<timestamp>/`. Reinvoke the command
on the same feature branch to resume after resolving a blocker. A per-worktree lock
rejects a concurrent contribution loop.

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
