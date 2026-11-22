# Workflows

Use these workflows to set up, operate, and update a Krabb installation.

## Table Of Contents

- [First Run](#first-run)
- [Local Development](#local-development)
- [Background Service](#background-service)
- [Optional Worktrees](#optional-worktrees)
- [Session Operations](#session-operations)
- [Updates](#updates)
- [Backup And Restore](#backup-and-restore)
- [Uninstall](#uninstall)

## First Run

Install the CLI and daemon with the command in the [README](../README.md#get-started).
The installer verifies the release checksum and places `krabb` and `krabb-daemon` in
`~/.local/bin`, or `%LOCALAPPDATA%\Krabb\bin` on Windows. Set `KRABB_VERSION` to pin a
release and `KRABB_PREFIX` to choose another location.

```sh
krabb init
krabb status
```

`krabb init` installs and sets up a provider and a channel, then the background service.
To do the same by hand, run `krabb plugin install <intelligence> <channel>`, `krabb
plugin setup <id>`, and `krabb-daemon`. `krabb plugin install` downloads the newest compatible version of each official
plugin from the plugin index and verifies it. `krabb plugin setup` explains each field and checks values before
saving them. Choose a hosted provider such as `openai`, `anthropic`, `gemini`, or
`openrouter` for remote inference, or `ollama` for a local model; local inference
needs its own memory and CPU, described in [Resources](resources.md). See
[Providers](providers.md) for credentials and default models.

`krabb status` lists each plugin as `needs setup`, `configured`, `failed to start`,
or `ready`, with the command to run next. Keep the daemon in the foreground until
replies and acknowledgements work, then move it to the
[background service](#background-service). Configuration, conversations,
credentials, and plugins live in `KRABB_HOME`; [Storage](storage.md) lists every
location.

## Local Development

```sh
cargo build -p krabb-plugin-telegram --locked
cargo install --path krabb --locked
cargo install --path krabb-daemon --locked
krabb plugin install telegram --link
krabb doctor
```

Before opening a pull request, check Rust formatting and repository block
spacing with `make fmt`. Apply both formatters with:

```sh
cargo fmt --all
make spacing
make fmt
```

`make spacing` separates control flow, multiline statements, and setup/assertion phases
in Rust and Python; it also checks JavaScript block spacing. It preserves embedded strings
and fixtures and is idempotent. Keep the inserted blank lines when editing.

Make targets, Git hooks, and engineering loops build with two parallel jobs to bound
memory; raise it on larger machines with `make verify JOBS=4` or `CARGO_BUILD_JOBS`.
Dev builds keep only line tables for debugging, which keeps backtrace file and line
numbers while cutting link memory and test binary size.

`make coverage` tests the runner, then measures each package with fresh workspace
coverage data. Dependency builds remain cached; cleanup, test, and threshold failures
fail the gate.

Build only the plugins you plan to use. After changing a linked plugin, rebuild
its package and run `krabb plugin install <id> --link`; the host health-checks the
staged executable and loads it into a running daemon without a restart.
`krabb plugin update` previews available changes, then prompts to apply them
in a terminal. Use `--yes` to skip that prompt. It switches active plugins over
between their requests without restarting the daemon or pausing other
conversations, and plugins that were inactive remain inactive.

Continuous integration also checks the `x86_64-pc-windows-gnu` target from
Linux. The job installs MinGW for native dependencies before running the
locked workspace check; release archives continue to use the supported MSVC
targets.

Pull requests test only what they can affect. `krabb-scripts/changes.py` maps the
changed files to their workspace packages and every package that depends on them, and
the crate tests, coverage, and plugin integrations run for those alone. Changes to only
documentation or assets skip the Rust jobs. Shared files such as `Cargo.lock`,
workflows, and scripts select every package, and pushes to `main` and manual runs
always test everything before a release.

Repository scripts keep their responsibilities separate and expose `--help`.
Use `krabb-scripts/metrics.sh` for Rust, script, and test counts; use
`krabb-scripts/check.sh ARCHIVE` to validate a release archive without
extracting it.

On a Linux host with Docker or Podman available, run
`krabb-scripts/sandbox.sh target/debug/krabb-plugin-tools` to verify the
networkless, read-only-root tools sandbox against a local image. The script
uses only an image already present on the host and does not pull one.

Use `make ci` for new or changed GitHub Actions workflows and for debugging
workflow behavior that depends on action ordering, inputs, runners, or
artifacts. It runs `krabb-ci/ci.sh`, which performs the local preflight and
then exercises the important Actions flow through `act`. For ordinary source
changes, use `make verify`, `make test`, and `make build` without the container
workflow.

## Background Service

```sh
krabb service install
krabb service start
krabb service status
```

Use `krabb service install --force` to replace an existing definition, and
`krabb service stop` before changing binaries or environment values. Use
`krabb service uninstall -y` to unregister the definition; pending worktree
cleanup is reported for the next daemon start.

## Optional Worktrees

Ordinary chats use the plain workspace with ownership filtering and need no Git
setup. Outgoing visibility grants are explicit; see [Visibility And Worktrees](sharing.md).

To enforce worktree isolation for every conversation on an integration:

```sh
krabb worktree setup discord --force
# Or, for a client:
krabb worktree setup tui --client --force
```

Review the displayed root and confirm. Setup creates a managed empty Git baseline under `data/workspaces` if
needed, preserves the plain workspace, and restarts the daemon.
New worktrees start from committed baseline files. Worktrees cover DMs as well as
groups and disable tools that cannot enforce their filesystem boundary.

Use `krabb worktree disable discord --force` to return to ordinary conversation
storage. Old worktree files remain available for manual transfer or later reuse.

## Session Operations

```sh
krabb session new telegram-123 --model gpt-4o-mini
krabb session list
krabb session show telegram-123
krabb session model telegram-123 gpt-4o-mini
krabb session cancel telegram-123
```

Session listings are bounded summaries. Use `session show` for one transcript,
and delete only idle sessions after reviewing their worktree state.

## Updates

Run `krabb update` to move the CLI, daemon, and service to the latest release,
together with any official plugin the new host can no longer load; `krabb update
--rollback` returns to the retained recovery point. Run `krabb plugin update` for
new plugin versions. See [Updates](commands.md#updates) for the preview, recovery
point, and limits.

Use `update = "check"` to observe source changes, `"prompt"` to require an
explicit update command, or `"auto"` for staged startup updates. Keep explicit
Git revisions pinned in production and review permission changes before
accepting an update.

To upgrade without `krabb update`, stop the service, rerun the installer with
`KRABB_VERSION` set to the release, update plugins with `krabb plugin update -y`, and
start the service. The new host migrates the home on first start and restores it if
a migration fails. This path keeps no recovery point, so back up first.

## Backup And Restore

Stop the service with `krabb service stop`, then copy the whole `KRABB_HOME`
directory. It holds configuration, conversations, tasks, memories, plugins, and
credentials; treat the copy as secret. Secrets saved in the OS keyring with
`KRABB_KEYRING=1` are not in the directory; run `krabb plugin setup` again after
restoring on another machine.

To restore, stop the service, replace `KRABB_HOME` with the copy, and run
`krabb doctor` and `krabb status` before starting the service again. A copy from an
older release is migrated on first start; a copy from a newer release is refused
without changes.

## Uninstall

```sh
krabb service uninstall -y
krabb plugin uninstall <id>... -y
```

Uninstalling a plugin removes its saved secrets and sharing grants. Then delete the
`krabb` and `krabb-daemon` binaries from the install location. Delete `KRABB_HOME`
only if you no longer need its conversations and settings.
