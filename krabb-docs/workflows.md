# Workflows

Use these workflows to set up, operate, and update a Krabb installation.

## Table Of Contents

- [First Run](#first-run)
- [Manual Download](#manual-download)
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
release and `KRABB_PREFIX` to choose another location. Platforms without a complete
row in [Compatibility](../COMPATIBILITY.md) are experimental.

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
location. If a reply does not arrive, start with
[Troubleshooting](troubleshooting.md#check-the-installation).

## Manual Download

Each release on [GitHub Releases](https://github.com/krabblab/krabb/releases)
publishes one archive per platform, named `krabb-core-v<version>-<target>`, and a
`SHA256SUMS` file:

| Platform | Target | Archive |
|---|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | `.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `.tar.gz` |
| macOS Apple silicon | `aarch64-apple-darwin` | `.tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `.zip` |
| Windows ARM64 | `aarch64-pc-windows-msvc` | `.zip` |

On Linux or macOS, download and verify the archive, then put `krabb` and
`krabb-daemon` on your `PATH`. Set `version` and `target` for your platform:

```sh
version=0.1.0
target=x86_64-unknown-linux-gnu
archive="krabb-core-v$version-$target.tar.gz"
base="https://github.com/krabblab/krabb/releases/download/v$version"

curl -fsSLO "$base/$archive"
curl -fsSLO "$base/SHA256SUMS"
grep -F "$archive" SHA256SUMS | sha256sum -c -    # on macOS: shasum -a 256 -c -

rm -rf krabb && mkdir -p krabb ~/.local/bin
tar -xzf "$archive" -C krabb
find krabb -type f \( -name krabb -o -name krabb-daemon \) -exec install -m 0755 {} ~/.local/bin/ \;
```

If `~/.local/bin` is not on your `PATH`, add `export PATH="$HOME/.local/bin:$PATH"`
to your shell profile.

On Windows, in PowerShell:

```powershell
$version = "0.1.0"
$target = "x86_64-pc-windows-msvc"
$archive = "krabb-core-v$version-$target.zip"
$base = "https://github.com/krabblab/krabb/releases/download/v$version"

Invoke-WebRequest -UseBasicParsing "$base/$archive" -OutFile $archive
Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS" -OutFile SHA256SUMS

$expected = ((Select-String -Path SHA256SUMS -SimpleMatch $archive).Line -split "\s+")[0]
$actual = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower()
if ($actual -ne $expected) { throw "Checksum verification failed for $archive." }

Expand-Archive -Force $archive -DestinationPath krabb
$bin = Join-Path $env:LOCALAPPDATA "Krabb\bin"
New-Item -ItemType Directory -Force $bin | Out-Null
Get-ChildItem krabb -Recurse -Include krabb.exe, krabb-daemon.exe | Copy-Item -Destination $bin

$path = [Environment]::GetEnvironmentVariable("Path", "User")

if (($path -split ";") -notcontains $bin) {
    $entries = @($path, $bin) | Where-Object { $_ }
    [Environment]::SetEnvironmentVariable("Path", ($entries -join ";"), "User")
}
```

Open a new terminal so the `PATH` change applies, then run `krabb init`. The
binaries end up where the install scripts put them, so `krabb update` works the
same way afterwards.

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

Uninstalling a plugin removes its saved secrets and its sharing grants. Then delete the `krabb` and `krabb-daemon` binaries from the
install location. Delete `KRABB_HOME` only if you no longer need its
conversations and settings.

Uninstall cannot reach access that lives outside Krabb:

- **Environment variables.** A secret you set yourself, such as
  `KRABB_TELEGRAM_TOKEN` in a shell profile, stays set. Remove it there.
  `krabb service install` also copies such variables into an encrypted
  `.credentials.json` file next to the service definition. Uninstalling a plugin
  removes its values from that copy, and `krabb service uninstall` deletes it.
- **Keys at the provider.** Deleting a saved key does not disable it. Revoke or
  rotate it where it was issued, such as the provider's API key settings or
  Telegram's BotFather.
- **Backups.** A copy of `KRABB_HOME` made earlier still holds the credentials
  saved at that time. Delete such copies, or keep them as secret as the keys
  themselves. Update recovery points under `updates/recovery` do not copy
  `credentials.json` or `credentials.key`.
- **Shared names.** A secret that another installed plugin also declares stays
  saved until that plugin is uninstalled too.
- **OS keyring.** With `KRABB_KEYRING=1`, the key that encrypts saved credentials
  stays in the OS keyring even after you delete `KRABB_HOME`. Remove its Krabb
  entry with your keyring manager.
- **Data outside the home.** Stores at custom paths such as `KRABB_DB`,
  `KRABB_MEMORY`, or `KRABB_ROOT`, and logins kept by other tools, such as Codex's
  own sign-in, stay until you delete them.
