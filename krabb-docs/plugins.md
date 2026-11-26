# Plugins

Official plugins are small Rust binaries under `krabb-plugins/`. Each folder
contains a manifest and a one-line installation example. Community plugins may
use another language if they implement the JSON-RPC 2.0 protocol.

## Table Of Contents

- [Manifest](#manifest)
  - [Plugin State](#plugin-state)
- [Credentials And Sign-In](#credentials-and-sign-in)
- [Requests](#requests)
- [Authoring](#authoring)
- [Create A Custom Plugin](#create-a-custom-plugin)
- [Install And Update Lifecycle](#install-and-update-lifecycle)
  - [Official Plugin Index](#official-plugin-index)
- [Agent Interoperability](#agent-interoperability)
- [Runner](#runner)

## Manifest

```toml
id = "example"
version = "0.1.0"
capabilities = ["model"]
permissions = ["network"]
secrets = ["KRABB_CODEX_KEY"]
krabb = "0.1.0"

[protocol]
major = 0
minor = 2

[[commands]]
name = "example"
description = "Run the example command."
interactive = false
```

Use a stable one-word ID. Declare secret names, never secret values. Declare
every permission and external endpoint so installation and updates can show
the operator what changes. Command names must be unique across installed plugins
and cannot shadow native commands. Capability, permission, and secret lists are
validated for known, unique values before a plugin can be installed or started.
Protocol minor versions are backward-compatible: this host accepts plugins
declaring 0.0 through 0.2. The current protocol is 0.2, and all bundled plugins
declare 0.2. The optional `krabb` key names the oldest Krabb version the plugin
works with, for a plugin that relies on host behavior the protocol does not
cover, such as a tool name; older Krabb versions refuse to install it and say
which version it needs.

`plugins.lock` records the canonical source, revision marker, SHA-256 of the
manifest and executable, protocol, version, capabilities, permissions, secrets,
and state schema. An explicit Git `--revision` is pinned for later updates;
unpinned Git sources follow their configured default revision. Local and remote updates
health-check staged binaries before activation and refresh the hash only after
staged activation succeeds; lock persistence failure restores the previous
plugin bytes. Git
sources may use HTTPS, SSH, or local `file://` URLs. Archive
sources must use `.tar`, `.tar.gz`, `.tgz`, or `.zip` and include a SHA-256
fragment; remote archives require HTTPS. Krabb rejects embedded URL
credentials, unsafe archive paths, and archive symbolic links. Release archives
place plugin executables in `bin/` beside their manifest.

### Plugin State

A plugin that keeps saved state declares its schema in a `[state]` table:

```toml
[state]
schema = 3
from = [1, 2]
```

`schema` is the version of the state this release reads and writes, starting at
1. `from` lists the older schemas this release migrates to `schema` itself; list
up to 64, each lower than `schema`. Krabb never migrates plugin state, and the
protocol has no migration method. Migrate on the first request that needs the
state, not during `hello`, because Krabb starts a new release once to check it
before the replacement is final.

Before replacing a plugin, through `krabb update`, `krabb plugin update`,
automatic updates, `krabb plugin install`, or `krabb import`, Krabb compares the
schema recorded for it with the new release's:

- the same schema, or neither release declaring one, needs nothing;
- a release that declares state for the first time starts recording it, and
  update previews show `state: starts recording schema 3`;
- a schema listed in `from` is a pending migration, which update previews show as
  `state: schema 1 → 3 when it starts`;
- a lower schema, a schema missing from `from`, or a release that stops declaring
  state is refused before anything changes, naming the schemas involved. Under
  `krabb update`, this refuses the whole update.

Uninstalling a plugin keeps its saved data and its recorded schema, so
reinstalling an older release is refused the same way. Keep `[state]` in every
later release, and keep older schemas in `from` for as long as installs may still
hold them.

## Credentials And Sign-In

For simple tokens and API keys, list their environment-variable names in the
manifest's `secrets` array. Krabb supplies `krabb plugin setup <id>` and
then passes only its declared stored values into its environment. Every declared
secret is prompted for, including optional ones. Cleared and skipped secrets are
omitted from the plugin environment; the plugin remains responsible for requiring
credentials when needed.

Declare additional setup fields with `[[settings]]` entries:

```toml
[[settings]]
name = "KRABB_EXAMPLE_PHONE"

[[settings]]
name = "KRABB_EXAMPLE_LISTEN"
default = "127.0.0.1:8787"
```

Each setting names a `KRABB_` environment variable. Names must be unique across
secrets and settings. An optional `default` must be nonempty, single-line, and
at most 16 KiB. Only public defaults belong in manifests; never put credentials
there. Setup prompts for secrets first, then settings in declaration order;
secret input stays hidden and setting input is shown. All values
are saved together in the encrypted credential store. The host resolves each
field from a nonempty environment override, saved value, then manifest default.
It passes the result to the plugin on launch and reload, so services do not need
extra environment entries. Plugin-specific validation remains in the plugin.

Describe each secret or setting with an optional `[setup.NAME]` table, so setup
explains what to enter instead of showing only the variable name:

```toml
[setup.KRABB_EXAMPLE_TOKEN]
label = "Bot token"
help = "Create a bot in the example dashboard and copy its token."
url = "https://example.com/bots"
example = "123456:ABC..."
required = true

[setup.KRABB_EXAMPLE_LISTEN]
label = "Webhook listener"
kind = "address"
```

| Key | Meaning |
| --- | --- |
| `label` | Short name shown in the prompt and summary, at most 64 characters. |
| `help` | One-line explanation, at most 320 characters. |
| `url` | HTTPS page where the user gets the value. |
| `example` | A public, made-up sample value; never a real credential. |
| `required` | Refuses empty and cleared answers; defaults to `false`. |
| `kind` | `text` (default), `integer`, `boolean`, `url`, or `address` (`host:port`). |
| `accept` | Literal values accepted in addition to `kind`, such as `["auto"]`. |

Every key names a declared secret or setting, and a setting default must match its
kind. Older hosts ignore the table, and manifests without it keep the previous
prompts. Metadata is read from the installed manifest; it is not recorded in the
plugin lock.

Set `check = true` at the top level of the manifest when the plugin answers `check`.
After setup saves, the host starts the plugin with the saved values and sends `check`
with empty params. Use a read-only request, such as asking the provider who the
credential belongs to. Return `{}` when the credential works, and a short JSON-RPC
error when it is rejected. Setup does not show other result fields. Error text is
redacted and clipped before setup shows it. Setup reports the result but never fails
because of it, and skips plugins without the flag or that answer method-not-found.

Declare each external program the plugin runs, which Krabb cannot install for it, in a
`[[requires]]` entry:

```toml
[[requires]]
program = "signal-cli"
setting = "KRABB_SIGNAL_COMMAND"
help = "Install signal-cli from https://github.com/AsamK/signal-cli#installation."
```

`program` is a plain file name looked up on `PATH`. `setting` names an environment
variable whose value, a file name or an absolute path, overrides it. With no `program`,
the requirement applies only once that variable is set, and `off` lists its values that
need no program, such as `off = ["off"]`. `help` is one line, up to 300 bytes, that says
how to install or start the program. When a set-up plugin's program is missing,
`krabb status` and `krabb plugin list` report `missing program` with this help, and
`krabb doctor` lists the plugin under "Plugin programs". A running daemon checks its own
environment and `PATH`, which can differ from the shell's. A plugin may declare up to
eight requirements. Older hosts ignore the table.

Manifest and lock records without `settings` keep their existing behavior.
Settings and defaults are recorded in the plugin lock. Update previews show before/after
capabilities, permissions, secret names, settings, and commands. Confirming the update
also accepts these declaration changes; a separate reinstall is unnecessary. `--yes`
authorizes the saved preview, or creates and applies one when none exists. Credential
values never appear in the preview. Invalid declarations and incompatible protocols
remain errors.

For a custom sign-in flow, declare a command named after your plugin and implement
a subcommand such as `login`. Krabb delegates it to the plugin, as with
`krabb codex login`. The plugin must be able to start without credentials to offer
that flow. `setup` never redirects to `login`: a plugin may support both API-key
The host reserves `<id> setup` when secrets or settings are declared.
Other subcommands remain yours. Plugins without either get no generated setup
shortcut; use `krabb plugin setup <id>` when an ID conflicts with another command.
Document which route users should choose, any non-secret settings they need,
and whether activation starts network activity. See
[Plugin Setup](authentication.md#plugin-setup) for the user-facing behavior.

## Requests

Model plugins that accept images declare the `vision` capability in both their
manifest and `hello` response. The host preflights that capability before it
dispatches an image turn; the capability describes request-shape support, not
whether every selected model accepts vision. Model plugins accept `generate`
with a `ModelRequest` containing a model name,
normalized messages, a stream flag, and an optional active workspace path. The
workspace field is backward-compatible and gives providers the isolated session
root instead of making them infer it from process state. Streamed requests may
emit bounded `event` notifications before the correlated response; each event
must contain a normalized `Event` object, and malformed notifications are
rejected. Providers that support streamed tool calls route each request through
the host's policy callback. They return `ModelReply` or a JSON-RPC error. Keep
stdout reserved for JSON-RPC frames; write diagnostics to stderr.

Model plugins may advertise the additive `context` capability and implement:

- `model.info` with `{ "model": "..." }`, returning optional
  `window: { tokens, output }`, `compact: bool`, and `search: bool`.
  `tokens` is total context capacity and `output` is reserved generation space.
- `model.compact` when `compact` is true, accepting `model`, normalized `messages`,
  `max_tokens` (desired summary length), and optional `output` (total generation
  allowance), returning `{ "text": "portable summary" }`.

The host owns compaction, fallback summarization, caching, and final truncation;
providers only supply limits and integrations. `ModelRequest.generation` carries
`output`, `summary`, and `search`. Summary requests must disable tools and search.
An advertised capability with invalid metadata fails closed. Legacy plugins without
`context` keep their existing request path and do not receive automatic budgeting.

Model metadata may include optional `execution` information: `deployment`
(`unknown`, `local`, or `remote`), an opaque `identity`, and explicit `streaming`,
`cancellation`, `usage`, `recoverable`, and `deadlines` booleans. The identity must
change with the deployment/model/configuration and must not contain an endpoint
URL or credential. Ollama also exposes this object through `model.execution`.
Cancellation means confirmed cessation of provider computation; closing an HTTP
request alone does not establish that guarantee.

After deadline negotiation, `generation.limits` can contain `first_millis`,
`idle_millis`, and `total_millis`. Each must be positive, first and idle cannot
exceed total, and total cannot exceed 86,400,000 milliseconds. These are bounds
for an individual inference call, independent of a durable task's lifetime.
The host restricts admitted limits to its remaining allowance and preserves them
through context preparation and summary generation. Automatic forwarding requires
an output allowance; providers without a context window can receive explicitly
admitted generation metadata. Requested deadlines fail when the provider does
not advertise support. Legacy requests omit the new fields.

A provider that also advertises `renewal` can receive `generation.limits.ceiling_millis`
on streaming calls: the highest total the host may later admit. The provider
applies its transport timeout at that ceiling but enforces `total_millis`. During
the call the host may send the note `generation.renew` with `id` (the call ID) and a
larger `total_millis` no greater than the ceiling. The host first persists the
extension in the task account, renews only after real output since its last check,
renews at most three times by half the original total, and never renews a silent
call. Providers without `renewal` keep their fixed admitted envelope.

Ollama applies the admitted HTTP timeout instead of its default client timeout,
enforces silent-prefill and inactivity limits, and keeps its total limit fixed
unless a valid renewal raises it within the admitted ceiling.
Text, reasoning-token progress, and changed tool content count as activity;
transport heartbeats and unchanged tool snapshots do not. Reasoning text is not
retained or displayed. Execution metadata refreshes after a provider restart.
The host selects an allowance using bounded local timing observations and
conservative priors. Optional `ModelReply.timing` reports `load_millis`,
`prefill_millis`, and `decode_millis`; missing phases remain unknown. The host
rejects impossible reports from learning. Ollama converts final response timings
from nanoseconds, and excludes cached prefill from fresh-prompt rate learning.
The cache holds at most 64 profiles and 32 observations per profile, expiring after
seven days. Rate estimates require eight reports and at least 512 input or 128
output tokens, using the conservative 90th percentile. A 50% plus two-second
margin applies. Configuration identity and context-window changes isolate samples.

Local and unknown priors allow 120 seconds for cold loading, 100 milliseconds per
input token, and one second per planned output token; remote priors use five
seconds, two milliseconds, and 50 milliseconds respectively. Predictions also
include warm floors and separate inactivity limits. These priors and synthetic
held-out fixtures are not hardware performance guarantees. Requests predicted to
exceed the remaining hard allowance fail before generation. Current forecasts
use 512 planned output tokens for fast work and 1,024 for other work, plus summary
preparation where required; the provider's actual output bound is preserved.
Attempt observations distinguish completion, failure, host timeout, cancellation,
interruption, and intentional handoff. Incomplete attempts retain bounded elapsed
time with unknown token usage and phase timings; this is a censored observation,
not a zero-cost charge or a measured decode rate. Recent failures reduce confidence
and use at least the fallback priors without shortening measured rates retained
in history. Both phase rates need eight valid fresh reports after the last failure
to restore measured confidence. Cancelled and handed-off attempts do not mark the
provider as failed. Cancellation records a local request outcome and does not
confirm that remote computation stopped. Unknown adapter failure causes stay
unknown rather than being classified from message text. Measured responsiveness
and resource benchmarks are tracked in the [roadmap](../ROADMAP.md).

Optional `Event::Context` and `Content::Context` carry a bounded opaque `state`
with `provider`, `model`, and `data`. Only the matching provider/model may interpret
it. It is retained with assistant tool calls, excluded from display and summaries,
and never treated as host instructions. Do not modify signed provider payloads.
New hosts and plugins should be upgraded together for this protocol 0.1 extension;
older hosts cannot decode the new content/event variants.

Agent plugins can call `host/model_stream` to receive normalized text and tool
events in `ModelReply.events`. The host collects these events and returns them
with the completed reply. Bridges should preserve opaque context and thought
signatures with assistant tool calls. The host has a bounded live continuation
cache for clients that omit these fields; it does not survive a daemon restart.

Test plugins with malformed frames, oversized frames, cancellation, process
exit, unknown methods, and protocol mismatch before publishing an archive.

## Authoring

Start a plugin as an independent process that reads and writes bounded
JSON-RPC 2.0 messages over the host framing transport. Complete the
`hello` handshake before serving requests, keep diagnostics on stderr, and
return a JSON-RPC error for malformed or unsupported calls. Use bounded reads,
bounded writes, and a clean `shutdown` path; the host may terminate the process
tree after a timeout.

The smallest useful plugin usually needs these handlers:

```text
hello     describe the plugin ID, version, protocol, and capabilities
ping      confirm that the process is healthy
shutdown  stop accepting work and exit cleanly
command   run a manifest-registered CLI command
```

Capability methods are intentionally typed by the normalized protocol rather
than by vendor wire formats. A model plugin implements `generate`; a channel
plugin implements `poll`, `send`, and acknowledgement methods; a tool plugin
implements only the operations it declares. Media-capable channels may expose
`media` to resolve a normalized external reference into a bounded private file.
Channels may implement `typing` with the same chat/channel and thread routing
fields as `send`, without text or a delivery ID, and return `true` on success.
The host also supplies `message` (the incoming event ID as a string) and
`private` (whether the conversation is a DM). These additive fields let plugins
reference the incoming message or distinguish individual and group recipients;
older handlers may ignore them.
The host refreshes it while waiting for the first streamed reply. Unsupported
methods, failures, and a two-second timeout disable this hint for the current
turn; they never fail or replay the turn.
Discord and Signal support typing in direct and group conversations. WhatsApp
typing also marks the incoming message as read. Slack supports assistant status
in DM threads when the app has access to that API; ordinary channels and
unthreaded messages return `false` without sending a placeholder message.
Do not add a direct dependency from one plugin to another.

### Channel Formatting

The optional `hello.formats` array advertises accepted text input formats. A channel
that accepts portable CommonMark with GFM strikethrough, tasks and tables returns
`"formats": ["markdown"]`. The host then sends `"format": "markdown"` alongside the
original `text` in `send` and `edit`. The adapter owns rendering to its platform's
supported markup, entities, or rich-text blocks, including safe escaping, Unicode
offset conversion, and chunking. Unsupported structures must remain readable and
link destinations must not disappear. Literal code is preserved.

Omitting `formats`, or advertising only unknown formats, preserves the legacy
contract: the host converts Markdown to plain text and omits `format`. An absent
`format` always retains the adapter's existing text semantics. This is an additive
protocol 0.1 extension; older hosts ignore the optional handshake field. New
platforms opt in through their handshake without changing a host platform list.

Streaming sends/edits may include `"draft": true`. A rich adapter sends or edits
only its first rendered chunk for such previews. The final call omits `draft`, edits
the preview, and sends overflow chunks once. Never retry an uncertain delivery just
to fall back to another format. Outbox records retain original Markdown so retries
and scheduled deliveries negotiate against the currently loaded channel adapter.

### Tool Discovery And Results

A plugin advertising the `tool` capability can expose its model-callable tools
through `tools/list`. Return an object with a `tools` array; every entry uses
the normalized `ToolSpec` fields `name`, optional `description`, and JSON
Schema `schema`. Name each tool `radix_action`, such as `browser_open`, and see
[Tools](tools.md) for the names Krabb already uses. Keep names unique across all
active tool plugins. The runtime rejects duplicate names rather than routing a
call to an arbitrary provider.
The legacy `tools` plugin keeps its built-in schemas as a fallback for older
installed binaries that do not implement `tools/list`.

Tool methods may return the legacy `{ "text": "..." }` result or normalized
multimodal content:

```json
{
  "content": [
    { "kind": "text", "text": "Screenshot captured." },
    {
      "kind": "image",
      "uri": "data:image/png;base64,...",
      "alt": "Browser viewport"
    }
  ],
  "metadata": { "width": 1280, "height": 720 }
}
```

For non-`tools` providers, invocation parameters use a trusted envelope:
`{ "arguments": <model arguments>, "host": { "session": "...", "turn": "..." } }`.
The runtime supplies the host values after validating the model arguments. A
plugin must use them for ownership and turn-scoped observations; it must never
accept model-supplied values as a substitute.

The runtime validates inline PNG/JPEG image data and enforces its image and
protocol limits before adding it to the next model request. The selected model
must advertise vision support. Temporary tool images remain available during
the current turn and are replaced with text in persisted transcripts.

The `browser` plugin requires Node.js, Playwright 1.64.1, and a separately
installed Chromium build. Install those dependencies explicitly; browser turns
never download executable packages. Its URL allowlist and request filters are
defense in depth and do not replace an operating-system egress boundary. See
[`krabb-plugins/browser/README.md`](../krabb-plugins/browser/README.md).

The [Computer plugin guide](../krabb-plugins/computer/README.md) covers its
Linux isolated display and opt-in macOS/Windows desktop modes. A virtual display
does not confine processes, files, or network access.

### Asynchronous Operations

Plugins can expose optional operation endpoints alongside their existing
capabilities. `operation.list` returns an `operations` array of normalized specs.
Each spec provides its input schema and declares cancellation support. Hosts
discover this interface once per runner process before using it.

| Method | Input | Result |
| --- | --- | --- |
| `operation.start` | `id`, `name`, `arguments` | Normalized operation receipt |
| `operation.inspect` | `name`, `handle` | Current state and any final result |
| `operation.cancel` | `name`, `handle` | Backend state after the cancellation request |
| `operation.result` | `name`, `handle` | Terminal receipt or an error while work continues |

A receipt contains `state`, an optional opaque `handle`, an optional normalized
tool `result`, and optional `detail`. Active states require a handle. Successful
completion requires a result. Start arguments are limited to 64 KiB, handles to
4 KiB, and receipts to 256 KiB. An error response to `operation.start` means
nothing started, so the task continues. Unknown outcomes require reconciliation
before another external start. The host owns authorization and durable coordination.
Adapters own external handles and wire payloads. The A2A adapter implements these
endpoints with endpoint pinning and authenticated status retrieval.

For durable foreground and background tasks with tool permission, the runtime
offers discovered operations as `<plugin>_<name>`, such as `runner_delegate`. Isolated workspaces
and scheduled runs do not expose this interface. The configured mutation approval
policy applies before admission. The model supplies a contextual acknowledgement;
the host saves intent before dispatch and the receipt before handoff. Monitoring
uses no inference and backs off to at most five minutes between inspections.
Adapter outages retry inspection of the saved handle, not the external start.
An operation past the optional `operations.max_seconds` allowance receives a
cancellation request when supported and blocks for review once it settles. A
cancellation call that fails or is refused is sent again; one interrupted by a
restart is reconciled by inspection instead.

Pause fences further productive actions. Without backend pause support, running
external work keeps the task pausing until it settles. A backend waiting for input
also provides a pause boundary; minimal receipt inspection can continue while paused.
Cancellation requests are dispatched at most once, followed by inspection of the
actual state; a running receipt does not mean effective cancellation. A lost start
receipt or unknown outcome blocks related work for review. Terminal receipts are
collected once and included as external data when inference resumes. The task has
no lifetime deadline, although each inspection has a bounded transport allowance.
Backend input requests are surfaced as blockers; sending additional input through
an existing external handle is not yet supported.

## Create A Custom Plugin

Custom plugins are supported today. A plugin source directory needs a manifest
and a built executable with this layout:

```text
hello/
├── krabb-plugin.toml
└── bin/
    └── krabb-plugin-hello
```

On Windows, use `krabb-plugin-hello.exe`. The manifest ID, executable name,
and command install ID must match. A command-only plugin can start with no
capabilities:

```toml
id = "hello"
version = "0.1.0"
capabilities = []
permissions = []
secrets = []

[protocol]
major = 0
minor = 2

[[commands]]
name = "hello"
description = "Run the Hello plugin."
interactive = false
```

The executable must complete the `hello` handshake and implement `ping`,
`shutdown`, and `command`. Keep protocol responses on stdout, diagnostics on
stderr, and use bounded JSON-RPC 2.0 lines. Add capability-specific handlers
only for the capabilities declared in the manifest.

Build the executable, then link it into a local Krabb installation for
development, or install a copy for regular use:

```sh
krabb plugin install hello --source ./hello --link
krabb plugin list
krabb doctor
krabb hello
```

Linking validates the manifest, checks the executable, runs a health check,
records the canonical source and checksum in `plugins.lock`, and hot-loads the
plugin when the daemon is running. A stopped daemon discovers it on its next
start. For distribution, publish the source or a checksummed archive and
install it with a pinned revision:

```sh
krabb plugin install hello https://github.com/example/hello.git \
  --revision COMMIT_SHA
```

Private repositories work when Git authentication is already configured on the
operator’s machine. Review permissions, secrets, and source revisions before
installing a custom plugin. The repository’s planned `krabb-plugins/template`
starter will provide a complete Rust example; until then, use this guide and
the existing plugins as protocol references.

## Install And Update Lifecycle

When `plugin install` is given one or more IDs without `--source`, Krabb
installs each from the [official plugin index](#official-plugin-index): the
newest stable, non-withdrawn version whose protocol this host supports and that
has a build for this target. A pre-release of Krabb picks the newest such version
with pre-releases included, whatever its number. A pre-release plugin installed
this way stays after Krabb moves to a stable release, until a newer stable plugin
version replaces it. The lock records the source as `index:<id>`, which
`plugin update` follows to newer versions. `<id>@<version>` installs and pins one
published version instead (recorded as `index:<id>@<version>`); a pinned version
other than the latest is used at your own risk, and `<id>@latest` returns to
following updates. `--unstable` installs the newest version, pre-releases
included, and pins it when it is a pre-release; run it again to move to a newer
one. Krabb never compiles a plugin when no build exists for the target; pass an
explicit local path or Git URL with `--source` instead (explicit sources apply
to one plugin ID per command).

### Official Plugin Index

Plugins are versioned and published independently of Krabb releases, which
contain only the host. Every published version is a tag in one public OCI
repository, `ghcr.io/krabblab/krabb/plugins`, and the repository's `index` tag
holds a summary that release automation rebuilds from those tags, so it never
drifts from what is published. Each plugin lists every published version with
its protocol, the oldest Krabb it works with when its manifest names one, release
date, permissions, secret names, an optional withdrawal reason, and one SHA-256
digest and size per target:

```json
{
  "schema": 1,
  "registry": "ghcr.io/krabblab/krabb/plugins",
  "plugins": {
    "telegram": [
      {
        "version": "0.1.0",
        "protocol": {"major": 0, "minor": 2},
        "krabb": "0.1.0",
        "released": "2026-11-20",
        "permissions": ["network", "filesystem"],
        "secrets": ["KRABB_TELEGRAM_TOKEN"],
        "artifacts": {
          "x86_64-unknown-linux-gnu": {"sha256": "…", "size": 4194304}
        }
      }
    ]
  }
}
```

Archives are blobs in that repository, addressed by their digest. Krabb requests
an anonymous pull token, downloads the index or an archive by digest, and
verifies it, so published versions are immutable and need no GitHub account or
other tools. Krabb reads the index on each install, update, or `plugin versions`
call (reusing it for a minute) and keeps a copy of each index location under
`cache/` for use when that index is unreachable. `KRABB_PLUGIN_INDEX` reads
another index from an OCI reference such as `ghcr.io/owner/plugins:index`, an
`https://` URL, or a `file://` path; a `file://` index may also name a `file://`
registry directory laid out as `blobs/sha256/<digest>`.

A version installs automatically when its protocol has the host's major version
and a minor version no newer than the host's, and when the host is at least the
`krabb` version the plugin names, if any. On a stable Krabb, pre-release versions such as
`0.2.0-rc.1` install only when named or with `--unstable`; a pre-release of Krabb
installs them by default.

With `--link` and no `--source`, Krabb looks for each plugin in the local
`krabb-plugins/<id>` directory or in a category directory such as
`krabb-plugins/channels/<id>`. Run the command from the repository root; each
plugin must already be built in the workspace's `target/debug` directory.

By default, `krabb plugin install` copies the verified executable into the
Krabb home so it is independent of its source. Add `--link` to use the local
development workflow instead: Krabb canonicalizes the source, validates the
manifest, stages the executable, runs a health check, and links the built
executable into the Krabb home (copying it if symlinks are unavailable).
Rebuilding and running the command again picks up local changes. `krabb plugin update`
repeats the process for locked entries and keeps installed files in place until
each staged replacement passes validation.

Linked plugins depend on the checkout staying at the same path. If you move it,
build in the new checkout and run `krabb plugin install codex tui --link --yes`
from its root, substituting the IDs you linked. This refreshes both executable
links and recorded sources without resetting configuration or credentials.
Running `make` installs the CLI and daemon; it does not relocate existing plugins.

`plugin list` reports binary presence, not authentication or running health, and
shows the credential setup command where available. Its JSON `health` field
retains `ready`/`missing` for binary presence. Use `krabb doctor` for health checks.
Update checks report unavailable local sources directly; restore the source or
reinstall from its new location before updating.

After `install` commits the plugin and lock entry, the CLI asks a
running daemon to start it through authenticated local IPC. The plugin becomes
available without restarting the daemon or reinstalling the core. When no
daemon is running, it is discovered at the next daemon start. A selected model
plugin loads when installed and reports missing authentication when a request
needs it. Channel plugins load only when their required credentials are ready;
an activation error leaves the verified plugin installed, prints the reason, and returns
a failure exit status. `krabb doctor` checks local installation integrity; use
`krabb status` to check runtime availability. After upgrading the daemon binary itself,
restart the service once so it understands new host capabilities.

`plugin uninstall` unloads an active process before deleting its files. `plugin update`
prepares and verifies each replacement while the old process keeps serving, then switches
that plugin over between its requests. The request in flight finishes on the old version;
the plugin's next requests wait only for the switch, while other plugins, channels,
clients, and conversations keep running. The old process stops before the new one starts,
so a channel's inbox, socket, or webhook port never has two owners. The daemon commits
the files and lock record and publishes the new version under the same process slot, so
turns already under way continue on it. As after a restart, state a plugin keeps only in
memory does not carry over; the host keeps conversations, deliveries, and provider
continuations. A failed start or commit restores the installed files and restarts the
previous version from its running image; if it cannot restart, the plugin is unloaded
and the update says to run `krabb service restart`. Inactive plugins stay
inactive, and the daemon does not restart. If a single request runs longer than 310
seconds, the update is deferred and the old plugin keeps running.

Running processes use private executable copies, so replacing installed files does not
change the executable used by an existing turn or its recovery. Copies are removed on
shutdown; startup reclaims abandoned copies without deleting leased live images.
An older running daemon that lacks the update operation is left untouched and receives
an actionable upgrade/restart message. Offline updates hold the daemon lock.

When a plugin changes its capabilities, permissions, secrets, settings, or protocol
range, bump its version and update its README. Merging a version bump to `main`
publishes that version to the plugin index; see
[plugin releases](release.md#plugin-releases). Keep explicit
Git revisions pinned for reproducible deployments. Never place credentials in
the manifest, source URL, archive name, or diagnostic output.

The host honors dedicated environment variables first, then decrypts stored
credentials using the keyring or the configured key file. Plugins receive only
their declared secret values in their process environment; they do not need to
implement encryption or access the master key. Keep credential and key files
outside version control. See [credential storage](authentication.md#provider-credentials).

The MCP plugin exposes bounded `stdio` and Streamable HTTP bridges. Stdio
requests require an explicit `approved: true` flag because they start an
operator-selected process. HTTP accepts only HTTPS or loopback endpoints,
with strict host-boundary checks, never invokes a shell, and forwards
normalized JSON-RPC requests without allowing one plugin to call another
directly. Stdio initializes the child and sends `notifications/initialized` before
performing an operation. HTTP negotiates JSON/SSE responses and accepts empty 202
acknowledgements. HTTP callers perform initialization and pass the returned
`session_id` and negotiated `protocol_version` on subsequent calls to the same
authenticated endpoint; see the [MCP transport contract](../krabb-plugins/mcp/README.md).

Channel plugins split outgoing text at Telegram, Discord, and WhatsApp message
limits and retain normalized attachment metadata for providers. Telegram,
Discord, and WhatsApp media operations resolve opaque provider references into
bounded private files, preserving binary image and audio bytes; generated daemon replies currently remain text-only.

The host reads attached files the same way for every channel and client. Files
whose bytes are PNG, JPEG, GIF, or WebP become images; GIF and WebP images reach
the model as a PNG of their first frame, decoded within fixed size and memory
bounds. PDFs contribute their extracted text. Extraction runs one PDF at a time
in a short-lived child process of the Krabb binary, without the host's
environment. It is stopped after 30 seconds per message and, on Linux, capped at
1 GiB of memory. UTF-8 text and code files are included directly. PDF and text
content is cut off after 64 KiB, and files over 4 MiB are not read. Videos are
not downloaded. Files that cannot be read reach the model as a short note naming
the file and the reason, never as silent omissions. Clients upload attachments
with `session.attach`, which reads them before the message is saved. See
[Media](media.md) for the per-channel matrix.

The SQLite plugin uses WAL mode and idempotent schema migrations. When it is
installed and agent tools are enabled, Krabb adds a `data_query` tool to model
requests. The agent can create logical databases and tables with typed columns,
insert structured JSON records, query by exact field values, update a record by
ID, add or remove optional columns, and delete records or structures. The tool
receives a host-assigned scope for the current channel and conversation; models
cannot select or change that scope. Mutating operations follow the configured
tool approval policy. Raw SQL is not exposed.

SQLite data is available through `krabb data ...` in the local CLI and `/data
...` in the TUI. CLI data is in its own local scope; TUI commands share the
active TUI conversation scope with the agent. See the
[SQLite plugin guide](../krabb-plugins/sqlite/README.md) for commands, limits,
column types, and examples.

SQLite persistence methods; daemon channel delivery and its retry policy
remain in runtime state. The TUI manages session-scoped values through
`/data value`; `/store` remains as a compatibility alias.
Host outbox entries expose stable IDs and explicit uncertainty; plugins receive
the ID with each send so operators can correlate a provider receipt.

The [Memory plugin guide](../krabb-plugins/memory/README.md) documents record
scopes, learning modes, and commands. Memory tools still follow channel and
client policy.

The [Scheduler plugin guide](../krabb-plugins/scheduler/README.md) documents
task creation, authorization, timing, retries, recovery, and history.

The [Whisper plugin](../krabb-plugins/whisper/README.md) invokes a configured
local executable and returns normalized transcription text.

## Agent Interoperability

ACP starts a configured local agent, creates or resumes its protocol sessions,
and forwards text, progress, and tool status updates. Its filesystem callbacks
use Krabb's confined tools and approval policy. The current adapter does not
advertise terminal access, cannot answer generic ACP permission requests, and
does not implement cancellation. ACP agents run their own tools and model
workflow.

A2A is a client for discovering remote agents, sending messages, streaming
task updates, checking status, and requesting cancellation. Streamed artifact
replacements and appended parts are accumulated in the final command result,
so `--json` retains the answer after a completion status update. It does not expose
Krabb agents as A2A services or implement OAuth. AG-UI accepts runs on a
loopback HTTP endpoint and emits run, text, frontend tool-call, state snapshot,
and error events. Frontend tools are returned to the client for execution; the
backend does not invoke client-supplied tools.

AG-UI requires `Authorization: Bearer <token>` on every POST. Configure a random
token of at least 32 ASCII characters through `KRABB_AG_UI_TOKEN`. Browser
origins must be explicitly listed in `KRABB_AG_UI_ORIGINS` as comma-separated,
exact loopback origins. Browser access is disabled when that list is empty.
Incomplete HTTP requests expire after 10 seconds.

AG-UI's transport emits chunked SSE events, but the host model call returns a
complete reply before text and tool events are written. It does not provide
live token delivery, persist AG-UI thread state, mutate frontend state, or
propagate client disconnects as model cancellation. Human approval for
frontend tools remains the frontend's responsibility.

The retired coding-session adapter supplied a model-context handoff and an
authenticated local bridge for host read, write, patch, search, Git, shell, and
list tools. No current protocol recreates that combined workflow. ACP agents
use their own tools, while A2A and AG-UI have remote-collaboration and
user-interface roles. Workflows that depended on the old host-mediated coding
Secret and setting declarations must use the `KRABB_` prefix. The host rejects manifests
that declare unprefixed configuration variables.

Git tools and Git-backed patches require shell access and tool approval,
including status and diff: Git filters can execute repository-configured
programs. These tools are unavailable when shell access is disabled.

## Guarded Storage Operations

The host uses `scoped` for private memory, structured records, and speech:

```json
{"jsonrpc":"2.0","id":5,"method":"scoped","params":{"workspace":"/home/krabb/worktrees/discord-123","method":"index","params":{"scope":"provider:discord:123","visibility":["provider:discord:123"],"excluded":[]}}}
```

Plugins validate the host-assigned checkout under `KRABB_HOME/worktrees`. Memory
uses `data/workspaces/conversations/<id>/memory`; SQLite uses `data/workspaces/conversations/<id>/records`.
Whisper reads only the selected checkout's `attachments/`. Its `retained` method
checks the file owner and integration attachment directory for ordinary workspace audio. Inner methods are
allowlisted. Unsupported guarded operations fail without an unguarded fallback.

Memory retrieval and record reads accept host-issued `visibility` and `excluded`
lists of exact scopes or prefixes ending in `*`. The host overwrites model-supplied
access metadata. Record `source` selects an authorized namespace for reads only.
Writes retain their originating scope.

Ordinary file operations use the tools plugin's `visible` wrapper with `method`,
`params`, and typed `access`. Provenance is stored outside the workspace, under
`data/workspaces/files.json`. Reads, lists, and searches filter by owner; writes
cannot overwrite foreign ownership. Unknown files require an operator assignment.
Old direct calls remain available for trusted standalone callers. See
[Visibility And Worktrees](sharing.md) for configuration and capability boundaries.

Channel events may include an additive `reply` object with `id`, optional `sender`,
and optional `text`. IDs and senders are bounded to 256 bytes; quoted text is bounded
to 4096 UTF-8 bytes with a visible clipping marker. An optional `message` field holds
the native message ID when it differs from the event cursor (for example Telegram).
Older events without these fields remain valid. Adapters normalize provider fields
before emitting this contract. References do not alter sender roles or authorization.

Successful sends may return an additive `references` array of these objects, one per
sent text chunk. The host stores up to 128 snapshots in the matching session, using
its channel, chat and thread route. ID-only replies resolve from this history; missing
references are explicitly marked unavailable. Clearing session history removes the
snapshots. The host does not fetch arbitrary channels or attachment URLs for quotes.

Authenticated IPC `session.append` and `session.append_reserved` accept an optional
`reply_to` message ID or `last`. Targets are resolved from that session's own history
and saved with the user message, preserving context across client restarts. The
response returns the saved `message`, so clients show the same reference the model reads.

## Runner

The optional [runner plugin](../krabb-plugins/runner/README.md) provides extended
background operations. Its `runner` handshake capability and `operation.list`
declarations control tool availability. Ordinary minimal continuation and generic
host task controls do not require runner. The scheduler supplies independent timed
and recurring triggers. The host retains one task catalog, journals, receipts,
accounting, and delivery path for both execution modes.

The initial `delegate` operation uses the A2A transport library with runner's own
configuration and declared secret. Saved handles survive plugin replacement or
absence; unsupported operations do not fall back to synchronous retries. Existing
A2A operation handles remain reconcilable, but new extended work goes through runner.
