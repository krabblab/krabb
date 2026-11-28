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
release, with or without the leading `v`, and `KRABB_PREFIX` to choose another location.
These variables must reach the installer: put them after the pipe, as in `curl … |
KRABB_VERSION=0.1.0 sh`, or export them first. On Windows the installer adds its folder
to your user `PATH`; elsewhere it prints the line to add when the folder is missing from
`PATH`. Platforms without a complete row in [Compatibility](../COMPATIBILITY.md) are
experimental.

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

`krabb status` lists each plugin as `needs setup`, `configured`, `missing program`,
`failed to start`, or `ready`, with the step to take next. Keep the daemon in the foreground until
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

Build only the plugins you plan to use. After changing a linked plugin, rebuild
its package and run `krabb plugin install <id> --link`; the host health-checks the
staged executable and loads it into a running daemon without a restart.
`krabb plugin update` previews available changes, then prompts to apply them
in a terminal. Use `--yes` to skip that prompt. It switches active plugins over
between their requests without restarting the daemon or pausing other
conversations, and plugins that were inactive remain inactive.

See [Development](../krabb-handbook/development.md) for formatting, coverage, and CI.

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

Remove Krabb with the uninstall script for your system. By default it removes only the
program: its background service, the `krabb` and `krabb-daemon` binaries, downloaded
updates, and on Windows the `PATH` entry. `KRABB_HOME` stays with your plugins,
settings, credentials, conversations, and recovery points, so installing Krabb again
picks up where you left off.

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/uninstall.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/krabblab/krabb/main/uninstall.ps1 | iex
```

To leave nothing of Krabb on the machine, purge it. Purging also deletes `KRABB_HOME`,
the key Krabb saved in the OS keyring for its credentials, and the files Krabb created
at custom `KRABB_DB`, `KRABB_MEMORY`, `KRABB_CREDENTIALS`, and `KRABB_CREDENTIALS_KEY`
paths. It asks for confirmation first; pass `--yes`, or `-Yes` in PowerShell, to skip it
in scripts. Folders Krabb did not create, such as a project used as `KRABB_ROOT`, a
`KRABB_MEDIA` folder, or a Codex profile, are listed but never deleted.

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/uninstall.sh | sh -s -- --purge
```

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/krabblab/krabb/main/uninstall.ps1))) -Purge
```

Both scripts print what they removed and what they kept, and work even when the `krabb`
binary no longer runs. They take the home and store paths from the installed service, so
they clean up the installation that is really running. A service that runs another
Krabb, such as a build installed elsewhere, is left alone, and so is a `KRABB_HOME` that
does not look like a Krabb home. To remove a single plugin instead, run
`krabb plugin uninstall <id> -y`, which also removes its saved secrets and its sharing
grants.

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
- **OS keyring.** Deleting `KRABB_HOME` by hand leaves the key that encrypts saved
  credentials in the OS keyring; purging removes it. Secrets you stored in the keyring
  yourself under `it.airscript.krabb` stay; remove them with your keyring manager.
- **Data outside the home.** Folders such as `KRABB_ROOT` and logins kept by other
  tools, such as Codex's own sign-in, stay until you delete them.
