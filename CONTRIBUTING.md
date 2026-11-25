# Contributing

Thanks for helping make Krabb useful, safe, and easy to extend. Discuss
substantial changes through an issue before opening a pull request. Use
[SECURITY.md](SECURITY.md) for private vulnerability reports.

## Table Of Contents

- [Local Setup](#local-setup)
- [Formatting](#formatting)
- [Change Rules](#change-rules)
- [Commit Messages](#commit-messages)
- [Issues And Milestones](#issues-and-milestones)
- [Pull Requests](#pull-requests)

## Local Setup

```sh
git clone https://github.com/krabblab/krabb.git
cd krabb
rustup toolchain install 1.89.0
lefthook install
cargo build --workspace --locked
make verify
```

Lefthook runs fast checks before commits. `make verify` runs formatting,
Clippy, locked compilation, all tests, coverage, builds, and metrics. Provider
tests use local fixtures and never call paid APIs. Every implementation
handoff must finish with the configured coverage gates passing; run `make verify`
at the end of each work turn and resolve coverage failures without lowering
thresholds.

### Interpreting Coverage Results

Coverage verification requires localhost loopback access. Some provider and
channel integration tests return early when binding `127.0.0.1` fails with
`PermissionDenied`; the test process can still exit successfully while coverage
is understated. Treat an under-threshold report from a run with loopback binding
failures as inconclusive, not as a failed coverage gate. Rerun the full
configured profile with loopback available, and report a gate failure only if
that complete run still misses a threshold. Do not infer a gate failure from an
isolated package rerun or a restricted run, and do not lower thresholds to work
around skipped integration tests.

Before adding coverage tests in response to a local shortfall, check that the
environment permits a loopback bind:

```sh
python3 -c 'import socket; socket.create_server(("127.0.0.1", 0))'
```

If this fails with `PermissionError` or `Operation not permitted`, rerun the
coverage profile with local socket access before changing tests or thresholds.

## Formatting

Rust formatting has two checks: `cargo fmt` handles syntax formatting, while
`krabb-scripts/spacing.sh` keeps blank lines between adjacent Rust blocks and
multiline statements. Run the complete formatting check with:

```sh
make fmt
```

To apply both formatters before checking the result, run:

```sh
cargo fmt --all
make spacing
make fmt
```

The spacing formatter is idempotent and understands Rust raw strings, so it
does not alter their contents. Keep the resulting blank lines in the source;
do not remove them as cosmetic noise.

Use Title Case for section titles in Markdown files, including new or edited
documentation headings.

For the complete non-release CI pipeline, install Docker and
[act](https://github.com/nektos/act), then run:

```sh
make ci
```

The command first runs the Verify, Test, and Build preflight commands directly
on the current machine. Only after that passes does it use act's medium
`catthehacker/ubuntu:act-latest` image and a cached local CI image based on it
with Rust 1.89 and the native Docker architecture (`linux/arm64` or
`linux/amd64`). Docker keeps the images and action cache locally, so subsequent
runs do not download the runner again.
The command runs Verify, Agentskill, Security, the native Linux tests, the
sandbox test, and the native Linux build. Release, package, checksum, install,
update, and publish workflows are intentionally excluded; the Install and Update
workflows check installation, update, and rollback on every target before a
release (see the [release guide](krabb-docs/release.md)). Native macOS, Windows, and
cross-architecture jobs still run only on their GitHub-hosted runners.

Use `make ci` when adding or changing GitHub Actions workflows, or when the
workflow itself needs debugging: it runs the local preflight and then executes
the important Actions flow through `krabb-ci/ci.sh` and `act`. For ordinary
Rust or plugin changes, the local commands are sufficient:

```sh
make verify
make test
make build
```

This keeps action-flow debugging separate from normal implementation checks.

When changing CLI commands, update the completion command tests and verify at
least one generated script contains the new command. The completion command
supports Bash, Fish, PowerShell, and Zsh through `clap_complete`.

## Change Rules

- These changelog rules apply to human and agentic contributions.
- For the current `0.1.0` release, add entries only under `Added`, and only for
  important, shipped, user-facing additions. Use one very short sentence per
  entry. Carefully assess the value to users; routine or minor additions need
  no entry. Omit internal work, tests, documentation, and planning.
- Include only shipped changes that matter to end users. Omit refactors,
  dependency updates, tests, CI work, and planned features, including platform
  fixes that do not change user-visible behavior.
- Write each entry as one physical line in plain English. Describe what users
  can do or how their experience changes; omit implementation details, commit
  history, and jargon when a simpler phrase is clear.
- When preparing a release, review the shipped changes and user documentation;
  use commit history as a checklist, not as release-note copy. Group related
  changes and keep only the most valuable capabilities, especially for the
  first release.
- For later releases, keep upcoming notes under `Unreleased` and use Keep a
  Changelog categories only when they contain a user-visible entry. Omit empty
  headings. A stable bump turns `Unreleased` into the new version's section; a
  plugin may keep its own `CHANGELOG.md` with the same layout.
- A version bump is a release: merging it publishes the new version. Change
  versions only with `make bump`, only when a maintainer asks for the release,
  and only after a person confirms the planned versions. Never bump a version
  as part of another change. See the [release guide](krabb-docs/release.md).
- Keep the core capability-free and provider-neutral.
- Add provider or channel behavior only in its plugin.
- Keep plugin stdout as protocol-only JSONL and logs on stderr.
- Use one precise word for names when possible; do not invent vague utility
  modules or comments that merely restate code.
- Add deterministic tests and documentation for public behavior, permissions,
  and protocol changes.
- Follow the [deprecation policy](DEPRECATIONS.md) when you replace an interface or
  change saved data. Register every bridge with the release that removes it, and
  change saved data only through `krabb-migrations` steps.
- Keep filesystem access confined, shell disabled by default, and secrets out
  of logs, manifests, issues, and planning files.
- Never add telemetry or commit credentials.
- Keep [Compatibility](COMPATIBILITY.md) as simple checkbox tables for basic
  functionality per operating system and architecture. Keep the
  [functional test matrix](FUNCTIONAL.md) as one tested checkbox per user-testable
  crate or plugin, independent of platform.
  Record verified functional success only by ticking existing checkboxes. Do not add
  status phrases, dates, versions, evidence links, or narrative notes. Leave
  partial, untested, or unsupported checks unchecked.

## Commit Messages

Use lowercase Conventional Commits. When a scope applies, use the
corresponding monorepo module without the `krabb-` prefix. The CLI is the
exception and uses `cli`; plugin scopes use the plugin ID. Do not use `repo` as
a scope. For genuinely cross-cutting or repository-wide changes, omit the
scope entirely.

```text
feat(core): add turn contracts
feat(runtime): add session state
feat(telegram): add telegram channel
fix(tools): reject unsafe paths
docs: document commands
ci: add verification workflow
```

Documentation changes are intentionally unscoped: use `docs: ...`, never
`docs(docs): ...`. A module-specific scope is appropriate only when the commit
changes that module's implementation rather than merely documenting it.

Keep each commit atomic: one user-visible capability, module, fix, or support
change per commit. Commit messages must be one short, entirely lowercase line
with no body, footer, or extended description. Keep subjects imperative and
under 50 characters. Use `refactor` for structure-only changes, `test` for
test-only changes, `docs` for documentation, `ci` for automation, and `chore`
for repository maintenance.

## Issues And Milestones

Track work in [GitHub Issues](https://github.com/krabblab/krabb/issues) and follow
it on the [project board](https://github.com/orgs/krabblab/projects). Open issues
with the issue forms:

- Size an issue to one pull request. Split larger work into sub-issues.
- Give it a concise, Title Case title that names the outcome.
- Write a one-sentence user story: "As a <role>, I want <capability>, so that
  <benefit>." Add short, testable acceptance criteria and link the roadmap
  section or related issues under Context.
- Add one type label, such as `feature` or `fix`, and one area label: `core`,
  `runtime`, `plugins`, `channels`, `clients`, `providers`, `security`,
  `release`, or `documentation`.

Each [ROADMAP](ROADMAP.md) release has a milestone with the same name, such as
`v0.3 | Media`. The roadmap stays the plan; its milestone tracks the work. When a
milestone closes, update the roadmap and changelog.

## Pull Requests

If you work with a coding agent (Claude or Codex CLI), the
[engineering loops](krabb-docs/loops.md) help you meet these standards before opening a PR.
They are optional: CI and review apply the same rules either way. `make revloop` reviews
and repairs your change for correctness, `make secloop` for security, and `make testloop`
for test coverage and flaky tests.

Run `make contribloop` to verify all current changes, create a feature branch,
commit conventionally without a scope, push to `origin`, and prepare a PR using
the template and recent merged PR conventions. It empties `PLAN.md` and
`IMPLEMENTATION.md` before verification; move future work to `ROADMAP.md` and
durable implementation details to the relevant documentation first. It assigns the
authenticated GitHub user and adds the relevant existing label. The PR remains a draft while
CI runs; failed checks trigger repairs, verification, another commit and push.
Success means the current PR commit has green CI and is ready for review. It ends with a
summary of every commit it made and every recovery; review those before asking for review.
See [Contribution Loop](krabb-docs/loops.md#contribution-loop) for prerequisites,
resuming, limits, and publication authorization.

Keep one logical change per pull request, and close or reference its issue in
the template's Linked Issues section, for example `Closes #12`. Describe the
behavior, security and platform impact, test commands, documentation updates,
and release-note entry. Explain the fix or feature, not only the symptom that
motivated it. Keep review instructions in the pull request template and write
the submission under its comments.
