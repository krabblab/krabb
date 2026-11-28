# Commands

Use this reference to find CLI, daemon, plugin, and terminal commands.

```text
krabb help
krabb --version
krabb init [home] [provider] [hatch] [channel] [service] [--provider <id>] [--channel <id>|none]
krabb init --force [-y|--yes] [--json]
krabb hatch [<what to change>...] [--name <text>] [--call <text>] [--personality <text>] [--voice <text>] [--writing <text>] [--work <text>] [--language <text>] [--notes <text>] [-y|--yes] [--json]
krabb hatch --undo [-y|--yes] [--json]
krabb doctor [--fix] [--json]
krabb config get [<key>] [--json]
krabb config set <key=value> [--force] [--json]
krabb worktree setup <channel-or-client> [--client] [-y|--yes] [--force] [--json]
krabb worktree disable <channel-or-client> [--client] [--force] [--json]
krabb status [--json]
krabb version [--json]
krabb update [<version>] [-y|--yes] [--json]
krabb update --rollback [-y|--yes] [--json]
krabb completion <bash|fish|powershell|zsh>
krabb plugin list [<page>] [--json]
krabb plugin setup <id> [--no-activate] [--json]
krabb plugin check <id> [--json]
krabb <id> setup [--no-activate] [--json]
krabb plugin install <id>[@<version>|@latest]... [--source <path-or-url>] [--revision <rev>] [--link] [--unstable] [-y|--yes] [--json]
krabb plugin versions <id> [--json]
krabb plugin update [-y|--yes] [--json]
krabb plugin uninstall <id>... [-y|--yes] [--force] [--json]
krabb plugin detach <id> [--json]
krabb plugin attach <id> [--json]
krabb memory <status|search|list|show|remember|edit|forget|audit|converge|learning|help> [arguments...]
krabb session new <id> [--model <name>] [--json]
krabb session list [<page>] [--json]
krabb session show <id> [--json]
krabb session fork <source> <target> [--json]
krabb session model <id> <model> [--json]
krabb session cancel <id> [--json]
krabb session delete <id> [-y|--yes] [--json]
krabb delivery list [<page>] [--json]
krabb delivery retry <id> [-y|--yes] [--json]
krabb delivery drop <id> [-y|--yes] [--json]
krabb delivery drop --all [--channel <id>] [-y|--yes] [--json]
krabb export [PATH] [--path <PATH>] [--force] [--json]
krabb validate [--path <PATH>] [--json]
krabb import [--path <PATH>] [-y|--yes] [--force] [--json]
krabb service [install [--force]|uninstall [-y|--yes]|status|start|stop] [--json]
krabb tui [--session <id>] [--model <name>] [--plugin <id>]
krabb tui --once <prompt> [--session <id>] [--model <name>] [--plugin <id>]
krabb <plugin-command> [arguments...]
```

## Table Of Contents

- [Command Tree And Help](#command-tree-and-help)
- [Shell Completion](#shell-completion)
- [Output And Logging](#output-and-logging)
- [Confirmation And File Operations](#confirmation-and-file-operations)
- [Exit Codes And Scripts](#exit-codes-and-scripts)
- [Status And Health](#status-and-health)
- [Terminal User Interface](#terminal-user-interface)
- [Conversation Commands](#conversation-commands)
- [Updates](#updates)
- [Plugin Management](#plugin-management)
- [Worktrees](#worktrees)
- [Memory](#memory)
- [Sessions And Deliveries](#sessions-and-deliveries)
- [Scheduling](#scheduling)
- [Initialization And Service Management](#initialization-and-service-management)
- [Providers And Agent Interoperability](#providers-and-agent-interoperability)

An omitted provider or model inherits environment and TOML defaults using
[CLI arguments > environment > config.toml](configuration.md#provider-and-model-precedence).
With no configured model, the selected provider's small default is used. `--model default` and
`krabb session model <id> default` select that behavior explicitly; legacy `unset`
sessions are also accepted. See [default models](providers.md#default-models).

## Command Tree And Help

`krabb help` and `krabb --help` group commands into three types:

- **Native commands** are part of the Krabb CLI and do not depend on a plugin,
  such as `init`, `doctor`, and `plugin`.
  `delivery` needs a channel, `schedule` needs the scheduler, and
  `memory`, `tui`, and `data` need their respective plugins. These commands
  expose a Krabb-managed capability and appear in help when usable; an
  underlying plugin may implement the capability.
- **Plugin commands** include plugin-declared commands such as `codex` and setup
  shortcuts such as `telegram` and `anthropic`. They appear under Plugin Commands
  when installed. Krabb handles setup; custom commands run in the plugin.
- `krabb service` prints service help; use `krabb service status` for its
  current state. `krabb --version` and `krabb version` are equivalent.

`ask` is a native command that is always listed and reports an actionable error
if no model is ready. SQLite's `data` command is a first-party conditional
command: runtime exposes it when SQLite is installed and routes its data
operations to the plugin. The agent-facing data tool uses the same conditional
Installed plugins that declare secrets or settings expose the host-owned `setup` subcommand:
`krabb telegram setup`, `krabb anthropic setup`, and `krabb codex setup` all use
the same secure prompt and credential-file workflow. Existing plugin commands,
such as `krabb codex login`, keep their provider-specific behavior. The generic
`krabb plugin setup <id>` command also works when the plugin ID cannot be exposed
as a shortcut. Setup requires a terminal and accepts no token argument or piped
secret. It asks one numbered question per secret or setting, says what each is for
and where to get it, and hides secrets as you type. Press Enter to keep a saved
value, or type `-` to clear an optional one. Settings remain required unless they
declare a default. See
[Plugin Setup](authentication.md#plugin-setup) for storage and activation details.

`krabb whatsapp setup` also collects the phone number ID, Graph API base URL,
and listener address. Press Enter to accept the listener's `127.0.0.1:8787`
default. Saved settings are applied when the plugin starts or reloads.

`krabb ollama setup` also collects the host, defaulting to `https://ollama.com`.
For a local server, leave the API key empty and enter `http://127.0.0.1:11434` as the
host. Saved host changes reload the plugin
without reinstalling the service; remove any existing host environment override
first. See the [Ollama guide](../krabb-plugins/intelligence/ollama/README.md).

Command discovery waits up to five seconds for concurrent metadata access. If the
metadata remains locked or cannot be read, the command exits with an error;
it does not report success without executing the requested plugin command.
An unknown command exits with an error that points to `krabb --help`, which also lists
commands added by installed plugins.

For the boundary between the core library, runtime, and plugins, see
[Architecture And Interfaces](../krabb-handbook/principles.md#architecture-and-interfaces).

## Shell Completion

`krabb completion <shell>` writes a completion script to standard output.
Supported shells are Bash, Fish, PowerShell, and Zsh. Redirect the
script to the shell’s normal completion directory or source it according to
the shell’s installation conventions. The generated command name follows the
executable used to invoke it, so `krabb completion <shell>` generates a
completion script for `krabb`.

For example:

```sh
krabb completion bash > ~/.local/share/bash-completion/completions/krabb
krabb completion zsh > ~/.zfunc/_krabb
krabb completion fish > ~/.config/fish/completions/krabb.fish
```

In PowerShell, add the generated script to the current profile:

```powershell
krabb completion powershell >> $PROFILE
```

## Output And Logging

- `--json` writes structured command results to stdout. Diagnostics go to
  stderr; failures return a JSON error on stderr. Plugin commands send stream
  notifications, including device login instructions, to stderr and emit only
  the final structured result on stdout. ACP includes its answer in the result's
  `text` field.
- Multiline CLI error details are kept flush left for easier reading and copying.
- `--verbose` shows progress and nested error causes. `--debug` also prints
  internal diagnostics.
- `--help` (`-h`, `-H`) shows command help; `--version` (`-v`, `-V`) prints
  the version.
- Application logs are stored under `KRABB_HOME/logs/`. They hold Krabb's own messages
  down to `debug` and other libraries' from `info`. Set `KRABB_LOG` or `RUST_LOG` to
  filter them, such as `KRABB_LOG=trace` for everything; set `KRABB_LOG_FORMAT=json` for
  JSON logs.
  WebSocket transport internals are always excluded from files and stderr because
  they can contain credentials and message payloads, including at debug level.

See [Configuration](configuration.md) for log paths and settings.

## Confirmation And File Operations

Destructive or duplicate-prone commands prompt in an interactive terminal.
Use `--yes` or `-y` to skip the prompt. With `--json` or non-terminal input,
pass `--yes` for operations that need confirmation. `--force` allows replacement
of existing configuration, Krabfiles, or service definitions.
Import reports a missing Krabfile with its expected path and
validates TOML before changing local state; malformed Krabfiles include the
line and column of the parse error. Use `--path <PATH>` to import a different
file.

`krabb export` writes `./Krabfile` by default. Pass a positional directory,
such as `krabb export .`, or `--path <PATH>` to choose an output location;
an existing directory receives a `Krabfile` file, while a new path is treated
as the exact output filename. Exporting with no local config or plugin lock
uses the default config and an empty plugin list. If the output Krabfile is
already present, export stops without changing it; pass `--force` to overwrite
that file.

## Exit Codes And Scripts

| Code | Meaning |
|---|---|
| `0` | The command succeeded, or you declined a confirmation prompt; the output then says the action was cancelled. |
| `1` | The command failed. This includes a prompt that could not be shown because input is not a terminal or `--json` was passed. |
| `2` | The arguments were invalid, such as an unknown option or a missing value. |

Scripts should pass `--yes` to commands that ask before changing state, because
without a terminal they fail with exit code `1` instead of waiting for an answer.
Some of these commands appear only when the plugin that provides them is
installed:

- `krabb init --force`, when it resets an existing home
- `krabb hatch --undo`
- `krabb import`
- `krabb update` and `krabb update --rollback`
- `krabb plugin install`, when it replaces installed plugins
- `krabb plugin uninstall`
- `krabb session delete`
- `krabb delivery retry` and `krabb delivery drop`
- `krabb schedule approve`
- `krabb service uninstall`

`krabb plugin update` without `--yes` only previews changes with `--json` or when
input or error output is not a terminal, and `krabb update --json` without `--yes` prints the preview without
applying it. Errors go to standard error; with `--json`, results go to standard
output.

Output is plain whenever it is not a terminal: no colors, no help banner, and
progress printed once as a line, or left out, instead of redrawn. `NO_COLOR` and
`TERM=dumb` also turn colors off in a terminal, and `TERM=dumb` stops progress
from redrawing. Text from plugins and models shows control characters as `�`, so
it cannot move the cursor or change the terminal. Each command that supports
`--json` prints exactly one JSON document, even when it also stops or starts the
service, as `krabb service uninstall` and `krabb update` do. A failure, including
invalid arguments, prints one `{"error": ...}` document on standard error. CLI
results and errors never wrap commands or paths, so they stay whole and copyable
in narrow terminals.

## Status And Health
intelligence, channels, version, and the installed plugin count. When the daemon
is running, the capability lists reflect its loaded plugins, including all active
channels regardless of the CLI's environment. When it is stopped, the lists use
locally available credentials and settings; they do not prove live connectivity.

Channels are optional for local CLI and TUI use. If any channel is installed,
health requires at least one ready channel; an explicit `KRABB_CHANNEL` requires
that channel. Health also checks configuration, the selected intelligence plugin,
and command conflicts. Failed checks appear in `health_details`, and under
"Needs attention" in the text output.

Result screens share one layout, as `krabb status` shows:

```text
Krabb 0.1.0 is healthy
├─ Runtime: stopped
├─ Intelligence: configured (codex)
├─ Channels: not configured
└─ Plugins: 1 installed
   └─ codex: configured

Start codex with `krabb service start`.
```

A headline sentence comes first, then a tree of parts and their details, then,
when something is left to do, one sentence naming the command to run next.
`krabb doctor`, `krabb service status`, and `krabb init` follow it. Empty plugin,
session, schedule, and delivery lists say so in one line and, when there is
something to add, end with how to add it; the TUI words them the same way and
uses the same terms, Runtime and Intelligence.

Each installed plugin is listed with its state; the first command that moves a
plugin forward is the next action, and `--json` lists each plugin's `next`:

| State | Meaning | Next action |
| --- | --- | --- |
| `needs setup` | Required secrets or settings are missing. | `krabb plugin setup <id>` |
| `configured` | Setup is complete; the daemon is stopped. | `krabb service start`, or `krabb service install` |
| `missing program` | A program the plugin runs, such as `signal-cli`, is not on the daemon's `PATH`. | The plugin's install help |
| `failed to start` | The daemon is running but did not load the plugin; the reason is shown. | `krabb plugin setup <id>` when a credential was rejected, otherwise `krabb service restart` |
| `ready` | The daemon loaded the plugin. Clients are ready once set up. | None |

Use `--json` for automation. Capability objects are named `intelligence` and
`channels`, each with a `status` and `plugins` array. `plugin_states` lists each
plugin's `id`, `state`, `reason` (why it failed to start, or `null`), and `next` step,
which is `null` when ready. The former `messaging` key
has been renamed to `channels`; update scripts that read it.

## Terminal User Interface

`krabb tui` requires the daemon. Use `krabb service start` or run
`krabb-daemon` in the foreground. A service that runs but still starts its plugins
gets up to 30 seconds to answer. `krabb tui --once "prompt"` sends one prompt
without opening the full-screen client; pass `--session` to continue an
existing session. Nobody can approve a tool action during `--once`, so it denies
each one at once and the reply says to approve actions in the interactive `krabb tui`.

Interactive assistant replies render emphasis, inline code, headings, and code
panels using terminal styles. Links retain visible URLs. Formatting also applies
to streamed replies and saved history; `--once` keeps its plain output behavior.

The TUI supports `/help`, `/status`, `/model`, `/session`, `/workspace`,
`/approvals`, `/schedule`, `/memory`, `/data`, and `/plugin`. Use `/help` for
the commands available in the current session. `/data value` manages simple
session-scoped values; `/data database`, `/data table`, and `/data column`
manage structured records. `/store` remains as a compatibility alias. See the
[TUI guide](../krabb-plugins/clients/tui/README.md) for controls, sessions, and client
commands.

To manage a saved task, ask in your own words; the model controls tasks in this
conversation. Exact commands also work while the model is busy or unavailable:

| Command | Effect |
| --- | --- |
| `/task` | List unfinished tasks in this conversation. |
| `/task pause\|resume\|cancel <id>` | Control one task. |
| `/task approve\|deny <id>` | Decide a saved action; only the user can. |
| `/task steer <id> <instruction>` | Add a clarification to the task's goal. |
| `/task defer <id> <time> <timezone>` | Resume the task at an exact time. |
| `/task extend <id> <minutes>` | Raise the task's inference allowance. |

Only the saved action runs after approval, and cancellation discards it. Task
results still arrive in the conversation where work began.

You can enter another question while a response is running. The daemon saves it
with a task ID and answers it after the active foreground turn settles. Esc
interrupts the current turn and preserves separately queued requests.

Typed `!command` shell execution requires `clients.tui.shell` and is supported on Linux
and Windows. Other platforms reject typed shell commands because descendant containment
is unavailable for this execution path. Esc waits for process cleanup before completing
cancellation, and the next command receives a fresh cancellation token.

## Conversation Commands

The same commands work in every channel and client, with or without a command menu.
Type them as messages, pick them from Telegram's or Discord's command menu, or use
`/krabb <command>` in Slack, which reserves short names such as `/status`. Signal and
WhatsApp have no menu; typed commands work the same way.

| Command | Effect |
| --- | --- |
| `/help [command]` | List the commands available here, or explain one. |
| `/status` | Show this conversation's model, activity, and context use. |
| `/model [list \| <model>]` | Show, list, or switch this conversation's model. |
| `/session [new \| cancel \| clear]` | Start fresh, stop the current reply, or delete history. |
| `/again` | Answer the latest question again when its reply used no tools. |
| `/compact` | Summarize older messages to free the model's context. |
| `/hatch [<what to change> \| undo]` | Change Krabb's name, personality, and writing style. |
| `/task ...` | List or control background tasks. |
| `/workspace` | Show whether this conversation has its own workspace. |
| `/approvals` | List actions waiting for approval here. |
| `/memory <query>` | Search memories available here. |
| `/data [<database>]` | List saved databases, or one database's tables. |
| `/plugin` | List the active plugins. |
| `/schedule` | List scheduled work (local clients only). |

Krabb answers these itself, without an intelligence turn, so they also work while a
reply is running or the provider is unavailable. `/compact`, `/again`, and `/hatch` are
the exceptions: they need the model, so send them again once the current reply finishes.
Channel users never gain local operator authority. Commands act only on the sender's own
conversation: `/memory` and `/data` need the conversation's tool access, and `/schedule` is
limited to local clients. `/hatch` changes Krabb everywhere, so only the CLI, the TUI, and the
channel's configured `owner` can use it. `/model <model>` affects only this conversation.
`/session new` archives the earlier messages. `/session clear` deletes history only
after the same sender repeats it with the four-digit code it returns, within two
minutes.

`/compact` replaces all but the last two exchanges with a summary in what the model
sees; the stored history is unchanged. Messages from other chats on the channel are
received while it summarizes and answered right after. `/again` removes the latest reply
and its question, then answers the question as if it had just been sent, including after
a failed reply. It works in chat channels, only for the person who asked, and refuses
when that reply ran any tool, so it never repeats an action, when the question had
attachments, or when a task or schedule notice arrived after the reply.

You can also ask in your own words and any language, such as "start a new
conversation" or "which model are you using?". The model then uses the same commands
through the `conversation_command` tool. It cannot cancel its own reply, compact, answer again, change its own
personality, confirm `/session clear`, or approve or deny a task's saved action; it relays
the exact command to send instead. A new conversation it starts begins after its current reply.

Text that only looks like a command, such as `/usr/bin/env`, stays an ordinary
message. In Telegram groups, `/status@your_bot` is answered only by the bot it names.
Native commands and bot-addressed commands count as a mention for `channels.<id>.mention`.

## Updates

```sh
krabb update [<version>] [-y|--yes]
krabb update --rollback [-y|--yes]
```

`krabb update` moves the CLI and daemon to one release, the latest stable one
unless a version is given. A pre-release of Krabb takes the newest release or
pre-release from the GitHub release list instead. Releases contain only the host; plugins keep their
own versions. Krabb downloads and verifies the release and runs the new host
once before changing anything. Every official plugin moves to the newest version
the new host supports, in the same transaction, unless it still loads and is
pinned to a version or revision, or is no longer in the plugin index. An
official plugin with no compatible version, or a linked or custom-source plugin
the new host cannot load, stops the update with the installation unchanged.
After the update, in a terminal, Krabb runs `krabb plugin update` so you can
review and apply changes to linked and custom-source plugins; without a terminal
or with `--json`, it reports them as pending. Either way, the new version stays
installed. A truncated or mismatched download also stops the update, and so does
too little free disk space for the recovery point and new files plus 64 MiB,
with the missing amount named, and so does a plugin that cannot read its saved
state. It then previews versions, plugin permission, capability, secret, and state
changes, home migrations, and the service interruption.
With `--json`, the command prints that preview and applies it only with `--yes`.
Its result lists plugins that did not load after the update under `unloaded` and
reports a refreshed or failed service definition under `service_definition`, which
is null when the definition did not change. Both are null when the target release
predates this report.

After approval, Krabb stops a running service and saves a recovery point with
the previous binaries, service definition, configuration, plugins, and state
under `updates/recovery`. Saved credentials, `data/media`, and `data/workspaces`
are not copied. The new host installs the staged plugins, migrates the home,
rewrites an installed service definition that an older release wrote in an older
format, keeping its executable and environment, and restarts a previously
running service, which must answer within 60 seconds. A definition you edited
by hand is never rewritten. Plugins you detached stay detached, and every plugin
that was loaded must load again within 60 seconds; one that does not is reported
with its next step, and the new version stays installed. A stopped service stays
stopped. Any other failure
restores the recovery point and the service state.

If the update is cut off, for example by a closed terminal, a crash, or power loss, the
next `krabb` command or daemon start settles it; `krabb version`, `krabb completion`,
and `krabb update --rollback` do not. While an update or its activation still runs,
other commands ask you to try again. When the new version had finished activating, it
is kept; otherwise the recovery point is restored. Either way, a service that was
running before the update runs again. Krabb says what it did, and for a week
`krabb status` shows it as `Interrupted update`, with `krabb update` to try again when
it was undone; `--json` reports it as `recovered`, with `from`, `to`, the `kept`
version, whether it was `restored`, and the time `at` in Unix seconds. A record that
names another release, such as one you reinstalled by hand, is left for
`krabb update --rollback`, which asks first.

`krabb status` shows a retained recovery point. `krabb update --rollback`
restores it without the network or the new daemon; if the new `krabb` cannot
run, start `updates/recovery/bin/0/krabb update --rollback` from the Krabb home
instead. After an accepted update, rollback replaces conversations, tasks,
settings, and plugins changed since then and asks first; credentials are kept.
Plugins uninstalled since the update come back without their sharing grants or
credentials, and the rollback prompt lists them. Downgrades to other versions
are not supported. A daemon running outside the service must be stopped before
an update.

## Plugin Management

```sh
krabb plugin list [page]
krabb plugin install <id>[@<version>|@latest]... [--source <path-or-url>] [--revision <rev>] [--link] [--unstable]
krabb plugin versions <id>
krabb plugin update
krabb plugin uninstall <id>...
krabb plugin detach <id>
krabb plugin attach <id>
```

Install accepts official IDs or one explicit source. An official ID installs the
newest version in the [plugin index](../krabb-handbook/plugins.md#official-plugin-index) that this
host's protocol supports, from any directory, and follows new versions on
`plugin update`. `<id>@<version>` installs and pins any published version at your
own risk, including withdrawn ones; versions the host's protocol cannot load are
refused. `<id>@latest` removes the pin. `--unstable` installs the newest version,
pre-releases included, and pins it when it is a pre-release. A pre-release of
Krabb installs and follows each plugin's newest version, pre-releases included,
without a pin, with or without `--unstable`. `plugin versions <id>` lists every
published version with its release date and marks the latest, installed,
withdrawn, pre-release, and unsupported ones. Sources may be local paths, Git
revisions, or HTTPS archives with a `#sha256=` digest. `--link` is for local
development builds. Installation validates each manifest and stages files before
activation; a running daemon loads new plugins over local IPC.
Interactive install, uninstall, update, and import operations show an in-place spinner.
Progress stays on one terminal line, with long messages clipped to fit its width.
Plugin install, uninstall, and update also print plain status lines to stderr
when it is redirected. Update progress covers checking sources, preparing the
update, and switching each plugin over after its current request; other plugins and
conversations keep running. Installation identifies the plugin being installed and
activated;
uninstallation identifies the plugin being removed. Spinners clear before
prompts and results. `--json` suppresses these progress messages.
Plugin results use one-line tree entries; failed installs and updates point to
the current application log file. Update previews also show the failure reason.
`plugin list` shows each plugin's state from `krabb status`, with the reason a
plugin failed to start and its next step, plus credential setup commands. A plugin
whose binary is gone is `missing`; with no daemon to ask, the TUI shows `installed`.

`plugin update` previews changes, then replaces changed plugins after
confirmation, including reviewed changes to capabilities, permissions, secret names,
settings, commands, and the plugin's state schema. A release that cannot read the
installed state is listed as a failed check and not offered. Reinstallation is not
needed for these changes. `--yes` accepts
the saved preview; stale or modified candidates are rejected. Running plugins keep
serving during preparation and switch over between their requests. Installation
activation errors print their cause and return a failure exit status.
Uninstall accepts multiple plugin IDs and confirms the batch once;
the last model or channel plugin can require deleting dependent session or
delivery state; `--force` with confirmation performs that purge. See
[Plugins](../krabb-handbook/plugins.md) for manifests, permissions, and update behavior.

Uninstall waits for active work, including an in-flight channel poll, to settle.
Idle channel retry backoff does not keep uninstall waiting across repeated restart
failures. The spinner remains visible while a single plugin or a batch is removed.

Uninstall also revokes what the plugin was given: it deletes the saved secrets
and settings that no other installed plugin declares, and removes sharing grants
to or from the plugin in `config.toml`. A local `visibility` list emptied this way
stays `[]`, so it never starts inheriting top-level grants. If revocation fails,
the plugin stays installed and the error lists what was already removed; rerun
the uninstall to finish. Access outside Krabb, such as environment variables, keys
at the provider, and older home backups, needs its own cleanup; see
[Uninstall](workflows.md#uninstall).

Use `plugin install` (alias `add`) to install and activate a plugin, and
`plugin uninstall` (alias `remove`) to remove it. Installing does not require a
separate attach step.

Uninstalling also removes the plugin's saved secrets and sharing grants. If the copy
of its secrets kept for the service cannot be updated, the plugin is still uninstalled
and the result lists that step with a next action. With `--json`, `revoked.cleanup` is
`complete` or `partial`, and `revoked.failures` lists each failed `step` with its
`error` and `next` action.

For maintenance, `plugin detach <id>` (alias `stop`) waits for active work to finish
and disconnects the plugin from the runtime, keeping its files, credentials and data.
Use `plugin attach <id>` (alias `start`) to bring it back. Updates preserve detachment;
restarting the daemon or explicitly activating the plugin loads it again. These commands
control daemon-managed plugins, not separately launched CLI clients.
last channel plugin. Removing the last intelligence plugin with `--force` can delete
sessions. The command confirms this cleanup; `-y` skips the prompt. Dropped delivery
history does not block ordinary uninstall.

## Worktrees

`krabb worktree setup discord` enables one isolated Git workspace per Discord
conversation, including DMs. Use `krabb worktree setup tui --client` for TUI
sessions. Setup displays the managed repository under `data/workspaces` and asks
before enabling worktrees and initializing its empty Git baseline.

Use `--yes` to approve setup without a prompt, including in JSON mode. Add
`--force` to restart a running daemon. `krabb worktree disable discord --force`
returns to ordinary storage without deleting old worktrees; add `--client` for a
client. Worktrees exclude all visibility grants. See
[Visibility And Worktrees](sharing.md) for scope configuration and tool restrictions.

## Memory

The memory plugin provides `krabb memory status|search|list|show|remember|edit|forget|audit|converge|learning`.
Use `krabb memory help` for syntax. Learning defaults to `guided`; autonomous
learning is opt-in. See the
[Memory plugin guide](../krabb-plugins/memory/README.md) for scopes and record
behavior.

## Structured Data

When SQLite is installed, `krabb data` exposes commands to create and inspect
logical databases and tables, and insert, query, update, and delete structured
records. The TUI offers the same operations through `/data`; those commands
share the active TUI session's data with the agent. CLI data uses a separate
local scope by default; use `krabb data --scope tui:default ...` to inspect
that TUI conversation's data. Running `krabb data` without a subcommand or
passing `krabb data help` shows the full usage, commands, options, and notes.
CLI results are human-readable by default; add `--json` for JSON output. Tables
can be created empty, then defined with `krabb data column add DB TABLE NAME TYPE`
and `krabb data column remove DB TABLE COLUMN`; JSON schemas remain available
for bulk creation. Use `text` or `string` for text columns. List results use
tree branches like other Krabb inventories.
See the
[SQLite guide](../krabb-plugins/sqlite/README.md) for examples, limits, and
column types.

## Sessions And Deliveries

```sh
krabb session new <id> [--model <name>]
krabb session list [page]
krabb session show <id>
krabb session fork <source> <target>
krabb session model <id> <model>
krabb session cancel <id>
krabb session delete <id> [--yes]
krabb delivery list [page]
krabb delivery retry <id> [--yes]
krabb delivery drop <id> [--yes]
krabb delivery drop --all [--channel <id>] [--yes]
```

Lists are paginated; `--json` returns structured state. `session show` displays
one transcript. `session model <id> <model>` pins a model; use `inherit` as the model
to make a channel conversation follow its configured model again. Changing global
or channel provider/model settings through `config set` applies to new channel
turns without restarting the daemon. `default` selects the provider's default
model and remains an explicit pin. Cancellation waits for the turn to stop. An uncertain delivery
may already have reached its destination; retry only when a duplicate is
acceptable. See [Concepts](concepts.md) for session and delivery state.

`delivery drop --all` clears the queue across all pages after one confirmation.
Add `--channel telegram` to clear only Telegram deliveries. Dropping retains bounded
history and cannot retract messages already sent. Detach the channel first to keep new
messages from entering the queue. JSON mode requires `--yes` for cleanup.

## Scheduling

The scheduler plugin adds `krabb schedule add|list|history|update|run|retry|pause|resume|remove`.
Explicit CLI additions authorize immediate activation. Model-generated scheduling
proposals follow the turn's approval policy before activation; `off` blocks them.
Agent schedule tools use the conversation's `schedules` scope: own tasks by default,
explicit sources for shared, and configured integrations for global. Local CLI/TUI
slash-command administration can control all schedules. Sharing never grants outbound
send permission; recipients must be explicitly authorized.
Create a task with one timing option and an explicit provider and model:

```sh
krabb schedule add --after 5m --provider codex --model <model> \
  "<instruction>"
krabb schedule add --at "2026-10-20 09:00" --timezone Europe/Rome \
  --provider codex --model <model> "<instruction>"
krabb schedule add --every 1d --provider codex --model <model> \
  "<instruction>"
krabb schedule add --cron '0 9 * * *' --timezone Europe/Rome \
  --provider codex --model <model> "<instruction>"
```

`--after` sets a delay; `--at` sets a start time. Use `--every` for an interval
and combine it with `--after` or `--at` to choose the first run. Use `--cron`
for a calendar schedule with a timezone. Other options include
`--missed skip|coalesce|replay`, `--retries 1..5`, and
`--on-failure pause|continue`. Shell use requires explicit enablement. Message
delivery requires an exact destination at task creation. Uncertain side effects
pause for review, and interrupted mutating turns are not replayed.
Pausing a running schedule lets its current run finish before preventing later
occurrences.

Add up to 16 explicit result recipients with repeated
`--to channel:private|group:chat[:thread]` options. For example:

```sh
krabb schedule add --after 5m --provider codex --model <model> \
  --to telegram:private:123 --to discord:group:456 "Prepare a report"
krabb schedule update <id-or-name> --to tui:group:tui-default
krabb schedule update <id-or-name> --clear
```

The TUI accepts the same `--to` and `/schedule update` syntax. Explicit operator
commands authorize their specified destinations; natural-language requests use
the configured approval policy. Updates replace the complete list and are blocked
until any running task and its pending report finish. `--clear` removes outbound
recipients while retaining origin status reports. Existing `--message-*` options
remain available for legacy tasks using the message tool.

One run produces one stored result and independent durable deliveries. Use
`krabb delivery list`, then retry an uncertain delivery only after checking whether
it arrived. Retrying delivery does not repeat the run or resend to targets that succeeded.

Use `krabb schedule help` for full syntax. The
[Scheduler guide](../krabb-plugins/scheduler/README.md) covers task
authorization, retries, recovery, and delivery behavior.

## Initialization And Service Management

First run:

```sh
krabb init
krabb status
```

In a terminal, `krabb init` guides you through five steps, always in this order: `home`
creates the home directory, starter configuration, and workspace instructions;
`provider` installs, sets up, and selects an intelligence provider; `hatch` gives your
Krabb a personality; `channel` installs the terminal client or a chat channel and sets
it up, and choosing the terminal also installs the `tools` plugin when it is missing, so
your Krabb can read and save workspace files after you approve each change; and
`service` installs and starts the background service. If installing `tools` fails, such
as offline, the guide notes `krabb plugin install tools` and goes on. Choices use a list
moved with the arrow keys, and Esc stops the guide, so `krabb init` continues later.
Name steps to run only those, such as `krabb init home provider`. A finished step is
skipped when you run every step, and offers a change when you name it. Without a
terminal, `krabb init` creates only the home, and `--provider` and `--channel` answer
their steps, as in `krabb init --provider ollama --channel none service`.

When the terminal is a place to talk but a home made before 0.1.0-rc.5 saved
`approval = "off"`, which refuses every change the terminal's tools try, the `channel`
step asks whether your Krabb should ask before it makes changes from the terminal and
saves the answer as `clients.tui.approval`. Without a terminal, it prints the command
that turns approvals on. A `clients.tui.approval` you set stays as it is.

The `provider` step then asks for a model. A provider that lists its models, such as
Codex, offers its first 20 as choices, plus one to type another name; otherwise type a
model name. Enter keeps the model you chose before for the same provider, or the
provider's default. The summary names the provider and model, says that `config.toml`
in the home holds them for every conversation without its own choice, names any
`KRABB_MODEL_PLUGIN`, `KRABB_MODEL`, or `clients.cli` override, and says that no
restart is needed and whether the running daemon already uses them. Without a
terminal, the step keeps the earlier model or the default. Naming the step when a
provider is set also offers to change only its model. Codex skips its optional context
length, which stays `auto` until `krabb plugin setup codex` changes it. Before Codex is
chosen or kept, the step checks the Codex CLI: when it is missing, the guide stops with
where to install it, and when it is not signed in, the guide runs the ChatGPT sign-in in
a terminal. If sign-in fails, is cancelled, or there is no terminal, the guide stops and
says to run `krabb codex login`, then `krabb init` again. A guide stopped this way exits
with an error, so scripts see that setup is incomplete, while Esc stops it without one.
The `hatch` step runs only when the provider can answer, and otherwise names the fix and
moves on. The `service` step shows only its own outcome, not the service commands'
messages. It starts an installed service that is stopped, and replacing a service
restarts it so the daemon uses the new definition. When Codex is the provider and the
service was installed before Codex, the step adds the Codex CLI this shell finds to the
service and restarts it. A service that runs another home, such as when `KRABB_HOME`
names a different one, stays as it is, and the step prints how to replace it with
`krabb service install --force`.

The guide ends with the one next action. It says your Krabb is ready only when the
provider has its key or sign-in and a service or daemon runs. Otherwise it names the fix,
such as `krabb plugin setup openai`, or `krabb init service` when nothing runs. In a
terminal, when the service runs, the guide waits up to 30 seconds for it to answer and
checks that the provider loaded. If either fails, it says so with the one next step
instead.

`krabb doctor` is read-only; `krabb doctor --fix` creates missing safe defaults. `krabb init --force` erases all data under `KRABB_HOME` and
requires confirmation.

`krabb hatch` personalizes your Krabb. Run it without arguments and it asks eight
questions in three sections: identity (name, what to call you, personality),
voice (tone, writing style, language), and work (way of working, anything else). Each
question shows its character limit and an example; press Enter to keep the current
setting. Answer in your own words and in any language; the model ignores answers that
carry no meaning. Your Krabb's model then rewrites `data/system/KRAB.md` (who it is) and
`KLAW.md` (how it works) from your answers. The model is asked to keep everything you
did not ask to change, including your own edits; check its summary before you confirm,
and read the saved files with the command hatching prints.

Run it again to change something: the questions then keep whatever you skip. You can
also describe the change directly, as in `krabb hatch be more concise and call me Sam`,
or answer with flags such as `--name Pinch --voice "warm and brief"`; `--yes` saves
without asking, and is required with `--json` or without a terminal. `krabb hatch
--undo` restores the files as they were before the latest change, and running it again
switches back. Hatching refuses to save when the files changed while the model was writing,
and refuses hand-edited files over 16 KiB. Changes apply from the next reply,
including in a running daemon.

Hatching needs an intelligence provider. Without one, `krabb hatch` explains how to set
one up; meanwhile you can edit `KRAB.md` and `KLAW.md` by hand. The model's draft must
keep both files' headings and stay within the instruction budget, or nothing is saved.
Personalities shape how your Krabb speaks; they cannot change its built-in charter or
host policy.

In a chat, the channel's configured `owner` can send `/hatch` followed by the change,
such as `/hatch be playful but brief, and reply in Italian`, and so can the TUI. Krabb
applies it and replies with a summary, and `/hatch undo` goes back. Only a
message the owner types can change the personality. The model cannot run `/hatch` itself,
so content it reads cannot rewrite who it is.

Manage the background service with
`krabb service install|uninstall|status|start|stop|restart`. Installing does
not start it. `--force` replaces an existing service configuration; uninstall
confirms before removing it. Generated services preserve explicitly configured
browser and computer settings, including Playwright paths and native desktop
opt-in. Windows installations register `krabb-daemon --service` with SCM, which
receives lifecycle status and sends stop/shutdown controls through normal daemon
cleanup. Running `krabb-daemon` without this flag stays in the foreground.
Secrets set in the installing environment go to a copy next to the service definition,
and `krabb plugin setup` saves its answers in that copy too, so the service sees rotated
secrets after a restart. Service installation persists `KRABB_CODEX_HOME`, defaulting to the installing user's
`HOME/.codex` or `USERPROFILE/.codex`, so the daemon can use the existing Codex sign-in.
Windows services run as the dedicated `NT SERVICE\Krabb` account. Installation grants
that account access to the executable and existing private state, including the Codex
profile. Configured `KRABB_ROOT` and `KRABB_MEDIA` directories receive inheritable
modify access without replacing their existing permissions. Configured Playwright package
trees, Chromium directories, and explicit `KRABB_NODE` executable paths receive read/execute
access. The resolved `KRABB_CODEX_BINARY` executable also receives read/execute access,
without granting access to its parent directory or the entire user profile.
Reinstall with `krabb service install --force` to migrate an older LocalSystem
service; this stops the service, so run `krabb service start` afterward. Failed reinstalls
retain the unprivileged identity when restoring the previous configuration.
Private files remain accessible to the data-owning account after service writes.
See [Workflows](workflows.md) for platform setup.

## Providers And Agent Interoperability

Use `krabb plugin list` and `krabb doctor` to inspect installed capabilities.
For one-shot model requests, run (`--provider` and `--model` are optional):

```sh
krabb ask [--provider <id>] [--model <name>] [--quote <text>] <prompt...>
```

Place options before the prompt. To start a prompt with a hyphen, use `--`,
for example `krabb ask -- --explain this option`.

`krabb acp --workspace <path> <prompt>` routes filesystem requests through the
tools plugin. The selected directory must stay under the configured workspace;
writes require the configured approval policy. Approval prompts go to stderr;
JSON mode and noninteractive execution deny operations requiring a prompt.
`--session <id>` resumes the requested ACP session, including agents returning an
empty load result. Advertised authentication methods do not block an already signed-in
agent; an authentication-required session error is reported by the adapter.
`krabb ag-ui` serves until
interrupted, with bounded model work for each HTTP run.

Provider-specific setup and supported agent protocols are in
[Providers](providers.md). Each integration's README describes its limits.

## Workspace Ownership

`krabb workspace <path> --owner <resource>` assigns an existing file in the plain
workspace to an integration or to `global`. Paths are relative to `KRABB_ROOT`
(or `KRABB_HOME/workspace`). Files are not moved or modified.

```sh
krabb workspace notes.txt --owner channels.telegram
krabb workspace reference.pdf --owner global
```

See [Visibility And Worktrees](sharing.md) for grants and file access rules.

Reply to the last assistant answer in TUI with `/reply last Explain the second point`,
or select an exact message with `/reply <message-id> <message>`. Message IDs are
available in `krabb session show <id> --json`. Selection stays within the active
session; missing targets produce an actionable error. A quoted `!` remains model
input and does not invoke a shell. For a one-shot request, use
`krabb ask --quote "Earlier message" "Explain this"`. Quotes are bounded snapshots.

The ACP and A2A message commands also accept `--quote <text>`; A2A supports it on
`send`. For example: `krabb a2a send --quote "Earlier answer" "Explain this"`.

## Request Recovery

The TUI sends the saved user-message ID with a foreground answer request. If its
reply is lost, it looks up that accepted request without generating again. A new
client instance checks the latest saved TUI request for active work on reopening.
Completed results come from the retained task; running or blocked work returns
its actual state. A missing receipt does not authorize a fresh execution.

See [Task IPC](../krabb-handbook/ipc.md#request-recovery) for the authenticated local API.

## Deferred Task Resume

Ask the model to resume a task later, or send an exact command such as
`/task defer task-3 2030-11-12T09:00 Europe/Rome` or
`/task defer task-3 2030-11-12T09:00:00+01:00`. The model resolves relative times
and asks when the timezone is unknown. The host saves the instant and
timezone, pauses the task, and resumes eligible work when due without the scheduler
plugin or another message. If the daemon is offline then, it resumes after startup.
The original goal, budget, checkpoint, and delivery destination remain attached to
the same task. An operation still stopping must settle before productive work resumes.

Ask for status to see the saved resume time. Manual pause, immediate resume, or
cancellation clears it; steering preserves it. Unknown timezones, ambiguous or
nonexistent daylight-saving times, and past times do not change the task.

See [Task IPC](../krabb-handbook/ipc.md#deferred-task-resume) for the authenticated local API.

## Task Inference Grants

In the conversation where a task began, ask the model to extend it or send
`/task extend task-3 30`. Each grant raises its captured inference ceiling by up
to 24 hours, within the host
`inference.task_ceiling_seconds` ceiling. The task's state does not change; resume it
separately if it is paused or blocked.

See [Task IPC](../krabb-handbook/ipc.md#task-inference-grants) for the authenticated local API.
