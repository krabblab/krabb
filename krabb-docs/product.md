# Product

Krabb is a self-hostable, open-source agent operating system for building,
running, and orchestrating native and external AI agents. This document
describes the product’s purpose, capabilities, and boundaries; the
[engineering principles](../krabb-handbook/principles.md) cover architecture,
delivery, and verification. [PLAN.md](../PLAN.md) contains only active work, while
[ROADMAP.md](../ROADMAP.md) contains future expansions.

## Table Of Contents

- [Summary](#summary)
- [Product Behavior](#product-behavior)
- [Product Boundary](#product-boundary)

## Summary

Krabb is a self-hosted Rust agent runtime with a deliberately
capability-free core. Model providers, channels, persistence, tools,
memory, scheduling, speech recognition, MCP, and the TUI are independently
installed process plugins.

The supported experience includes Telegram, Discord, Codex, Anthropic and
OpenAI APIs, Ollama Local/Cloud, ACP, A2A, AG-UI, a full terminal UI, local
coding tools, persistent memory, scheduled tasks, MCP integrations, and local
Whisper transcription. Experimental browser automation and computer control are
available as optional plugins with separate system dependencies. Computer
control uses isolated Xvfb on Linux and explicitly enabled interactive desktop
access on macOS and Windows.

Brand positioning:

- Primary claim: A self-hostable agent OS for native and external AI agents.
- Primary description: Krabb is a self-hostable, open-source agent operating
  system for building, running, and orchestrating native and external AI agents.
- GitHub description: A self-hostable, open-source agent operating system for
  building, running, and orchestrating native and external AI agents.

## Product Behavior

Channels run concurrently, with independent sessions and optional provider/model
selection per channel. Additional channels and intelligence providers activate without
replacing existing ones. Local concurrency fixtures verify provider overlap, separate
cursors, plugin activation/removal, and single claims of queued replies.
Queued messages use exact persisted channel routes, including after session
renaming. Legacy queues without channel or chat metadata remain intact until
their route is established. Each approval resumes its own turn independently
of pending approvals on other channels.

The AOS is a shared platform: conversation history and delivery state are separate,
while global memories, instructions, and workspace resources are intentionally shared.
Agent-owned memories, context, instructions, and workspaces belong to the planned
multi-agent runtime. Channel separation does not create an independent agent.

Intelligence adapters resolve implicit model selections to verified small/efficient
models, including separate local and cloud Ollama defaults. Explicit selections are
preserved. Defaults never trigger automatic model downloads or fallback upgrades.

Global and per-channel/client intelligence defaults are configurable in TOML.
CLI arguments take precedence over environment variables, which take precedence
over scoped and global TOML defaults. One-shot CLI requests can omit provider
and model flags. Channel conversations follow live configuration changes unless
their model is explicitly pinned; their history is preserved.
- Channels negotiate Markdown input and preserve supported formatting through
  their adapters for progress, final answers, and scheduled delivery. Plugins without
  formatting support receive readable plain text with visible source URLs.
  Terminal formatting is unchanged. Research guidance prioritizes exact supplied
  URLs and factual tool-result reporting. Codex sign-in explicitly uses live search.
- Persistent daemon with authenticated local IPC, opt-in OS keyring
  credentials, protected-file fallback, and systemd-user, launchd-agent, and
  Windows Service support.
- CLI commands include init, doctor, status, version, plugin
  install/link/list/update/uninstall, session, delivery, service, export,
  validate, and import. Conditional native commands such as `session`,
  `delivery`, `schedule`, `memory`, `tui`, and `data` appear when their backing
  capability is available. The `ask` command is always listed and reports an
  actionable error if no model is ready. Other plugin-specific commands include
  `code` and `codex`. Human output is default; JSON and noninteractive
  confirmation are explicit.
- TUI supports chat, sessions, model/workspace switching, approvals, schedules,
  memory, plugin health, and streamed activity. Session transcripts retain
  displayed TUI command exchanges alongside model turns.
  Typed shell commands support Linux and Windows with descendant containment. Commands and
  manual compaction retain exclusive session ownership until execution and cleanup finish.
  Automatic compaction shares the model turn's deadline and retires failed requests before
  generation resumes. Stream failures finalize turns after cleanup; approval history writes
  remain available while uninstall waits for active turns.
- Sessions support new, resume, fork, cancel, and model switching. One turn
  runs per session; later messages queue; stop cancels. Bounds cover steps,
  time, and tokens. Crashes interrupt turns and mutating tools are never
  replayed.
- Plugin attach/detach controls runtime membership without uninstalling; start/stop and
  add/remove alias attach/detach and install/uninstall. Force cleanup is explicit.
  Uninstall revokes the plugin's secrets and sharing grants and reports any cleanup step
  it could not finish.
  Delivery dropping supports all pages and exact
  channel filtering, with one confirmation and a durable batch update. Terminal dropped
  history does not block uninstall.
- `KRABB_HOME/data/system/KRAB.md` defines Krabb's identity and communication
  style; `KLAW.md` defines behavioral guidance and workflows. `krabb hatch` and
  `/hatch` have the model rewrite both from the owner's own words, with undo. Both
  are loaded into every model turn after Krabb's built-in charter and alongside the
  bounded session transcript. Krabb does not load `AGENTS.md` itself; delegated
  coding agents may use it.
- Telegram uses long polling by default; Discord uses Gateway. Support DMs,
  allowlisted Telegram topics, Discord channels/threads, mention-only groups,
  owner/admin/member roles, and signed expiring approvals.
- Show separate progress for tool work and slow turns, keep typing active, and send
  final answers as fresh channel-safe chunks with completion notifications.
  Terminal clients stream answers with separate tool status and inline approvals.
- Accept text, images, safe text/code files, and voice. Gate media by model
  support and limits. Delete raw voice after transcription and expire other
  cached attachments after 24 hours unless pinned. Bound retained images and caches;
  reclaim only unreferenced durable images and report omissions when storage is full.
  See [media persistence](../krabb-handbook/architecture.md#turn-flow) for retention guarantees.
- Browser observations can reach vision-capable models as temporary image
  inputs. The Playwright plugin requires separately installed Node.js,
  Playwright, and Chromium; network interception does not replace OS egress
  controls.
- The computer plugin starts an authenticated Xvfb session on Linux. On macOS
  and Windows it can control the signed-in desktop only when the operator sets
  interactive mode and explicitly enables host control. Native desktop access
  is not isolated; process, filesystem, and network isolation remain the
  operator's responsibility.
- Whisper downloads local models separately and is CPU-first.
- SQLite gives the agent durable structured databases and typed tables for
  user data, with scoped record CRUD and bounded queries. Its separate store
  methods also provide key/value values, leases, and deduplication; daemon
  message delivery remains in runtime state.
- The optional memory plugin provides scoped, auditable, searchable, editable,
  and deletable records. Guided learning is the default and saves only on an
  explicit user request; autonomous learning retains stable, useful facts
  without secrets or sensitive inferences. Its private `memory/` home is
  created only after the plugin is installed and first used.
- Scheduling supports approved one-time and recurring agent tasks, including
  cron expressions with IANA timezones. Model proposals follow approval policy;
  remote controls are conversation-scoped and local administration covers all tasks.
  Pause automatic retries after possible mutations, including failed response saves.
  Missed tasks default to skip-and-notify, with configurable once recovery.
- Tools provide root-confined files, search, patches, Git status/diff/
  worktrees, and optional shell. Shell is disabled by default and each
  command requires approval. Display complete arguments and reject requests exceeding
  the [approval limits](configuration.md).
  Docker/Podman isolation is optional on Linux and Windows
  and required on other Unix hosts for shell containment. Containers are local-only,
  networkless, read-only-root, and limited to a writable active workspace.
  Direct Git and Git-backed patches share host shell descendant containment on Linux
  and Windows; other platforms require Git execution through the sandboxed shell.
  All conversations default to isolated scopes in ordinary folders. Worktrees are opt-in
  through guided Git setup, including owner DMs and TUI sessions.
- Codex signs in with a personal ChatGPT account through its
  installed runtime, owner-only by default. Anthropic uses API keys with no
  Claude OAuth or
  Claude Code delegation. Ollama supports local unauthenticated use and Cloud
  with KRABB_OLLAMA_API_KEY.
- Personal Codex sign-in uses the installed Codex CLI app-server, with runtime
  protocol validation; shared deployments can use provider API keys without
  Codex account coupling. Inherited MCP servers are individually disabled before
  model threads, leaving Krabb dynamic tools under host approval.
- A2A discovery cannot redirect task content or bearer credentials to an unrelated
  RPC origin. Separate discovery and RPC origins require explicit configuration.
- Collect no telemetry and redact credentials, tokens, authorization headers,
  and sensitive paths from logs. Suppress WebSocket transport logs at every verbosity
  to protect connection credentials and message payloads.

## Product Boundary

Browser automation and computer control stay experimental plugins. Video,
text-to-speech, Telegram webhooks, email/calendar/smart-home integrations, hosted
multi-tenancy, Claude Code delegation, and Claude subscription OAuth are outside
the current product boundary. They require a proven use case and a design that preserves Krabb’s
small core before entering the roadmap.
