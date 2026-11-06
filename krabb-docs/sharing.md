# Visibility And Worktrees

Ordinary conversations use one plain workspace. Visibility controls which
integration can read another integration's content. Worktrees are optional,
default to `false`, and isolate each conversation from all visibility grants.
Ordinary chat requires no Git setup.

## Configuration

A visibility rule is an **outgoing grant**. This example lets Discord read
Telegram's files, chat contents, and saved memories:

```toml
[[channels.telegram.visibility]]
data = ["files", "context", "memory"]
resources = ["channels.discord"]
```

Telegram cannot read Discord unless Discord grants access back. Grants are not
transitive: Discord cannot pass Telegram's content on to a third integration
through another visibility rule. An agent can still quote information in its own
replies; visibility is access control, not information-flow tracking.

Rules also work at the top level:

```toml
[[visibility]]
data = ["context", "memory"]
resources = ["global"]

[channels.telegram]
visibility = []
```

Here every configured ordinary integration exports context and memory, except
Telegram. Telegram still receives other integrations' grants. A channel/client's
explicit `visibility` list replaces the entire inherited list, including resource
categories omitted locally. An omitted list inherits the top-level list. No rules
means no exports. `visibility = []` explicitly disables exports.

To export only Telegram globally, put the rule under Telegram:

```toml
[[channels.telegram.visibility]]
data = ["context", "memory"]
resources = ["global"]
```

| Resource selector | Recipients |
| --- | --- |
| `global` | All configured ordinary channels and clients |
| `channels`, `clients` | All configured members of that category |
| `channels.discord`, `clients.tui` | One integration |
| `isolated` or an empty list | Nobody |
| `agents`, `agents.foo` | Typed selectors reserved for future agent identities; no agent runtime is configured today |

`isolated` cannot be combined with other selectors. Contradictory isolated and
export rules for the same data category are rejected. Selectors name integrations,
not individual chats. A plain integration can read its own conversations and
explicitly global data without an export rule. Worktrees never import or export.

## Data Categories

| `data` value | What becomes visible |
| --- | --- |
| `files` | Owned workspace content, including retained attachments |
| `context` | User messages and assistant replies |
| `memory` | Saved facts and preferences |
| `records` | Structured SQLite databases and records (`data` is an alias) |
| `permissions` | Read-only tool, shell, and approval policy information |
| `approvals` | Exact matching approval decisions, including denials |
| `schedules` | Task listing and management, subject to current approvals |

Provider/model selection, channel owners and allowlists, credentials, installed
plugins, session administration, delivery queues, and daemon state retain their
existing configuration and authorization rules. Visibility never enables tools,
changes a recipient's permissions, or authorizes access to a channel.

Context supplies at most eight recent conversations, eight messages each, and
16 KiB of message text. Excerpts retain source identities and are quoted background
data, not instructions. They do not merge durable histories.

Memory writes and structured-data writes retain their originating conversation
identity. Retrieval can include visible sources and explicitly global records.
The data tool's `list_databases` reports accessible source identities; select
`source` for read operations. Mutating another source is rejected. Visibility
changes never merge databases or overwrite records.

Approval reuse lasts five minutes and requires the same tool, exact arguments,
and canonical workspace. It includes sources granting `approvals`; default conversations still prompt
per operation unless a matching grant explicitly includes them. Changed arguments, expiry, or daemon restart require a new decision.
Worktrees do not reuse other conversations' decisions. Local `tools`, `shell`, and
`approval` settings continue to govern execution; visibility is independent.

## Files And Attachments

The plain workspace is `KRABB_HOME/workspace`, or the explicit `KRABB_ROOT` path.
There are no generated conversation/group directories or hidden runtime folders
inside it. Ownership is stored in `data/workspaces/files.json` and checked for
file reads, listings, and searches. New files belong to their writing integration.
Writes to a foreign-owned or unassigned filename choose a distinct name and return
the actual path. Read visibility does not permit overwriting another source.

Unknown files are not exposed automatically. The operator can assign existing
content, including imported legacy files:

```sh
krabb workspace notes.txt --owner channels.telegram
krabb workspace reference.pdf --owner global
```

These commands change provenance, not file contents. A selected TUI workspace
has its own ownership catalog entries. File ownership remains stable when grants
change. Shell and third-party plugins are trusted capabilities and do not enforce
this file catalog; keep them disabled when requiring filtered filesystem access.
Use worktrees for enforced conversation confinement.

Incoming attachments are stored under `workspace/attachments/<integration>/` or
`worktrees/<conversation>/attachments/`. Plugins may briefly download into
`data/media`; admitted content is copied into the appropriate content directory.
Retained media is bounded to 64 MiB and 1,024 files per media directory. Storage
changes preserve retained attachment references. Host state stays under `data/`.

## Schedules

Granting `schedules` lets recipients list and manage source tasks when their tool
and approval policies permit it. It never transfers task ownership, workspace,
provider, or captured capabilities. Each run rechecks its origin's authorization.
Revoked access pauses execution. Worktree-origin tasks cannot execute outside
their boundary.

Recipients are explicit and independent of visibility. Use repeated `--to` options:

```sh
krabb schedule add --after 5m --provider codex --model <model> \
  --to telegram:private:123 --to discord:group:456 "Prepare the daily report"
krabb schedule update <id-or-name> --to telegram:private:123 --to tui:group:tui-default
krabb schedule update <id-or-name> --clear
```

The task executes once. Its persisted result has independent delivery receipts;
successful recipients are not resent on retry. Delivery retries do not rerun the
agent. Uncertain deliveries require operator review through `krabb delivery`.
Authenticated CLI/TUI slash commands remain operator administration. Agent schedule
tools always apply visibility and approval checks.

## Worktrees

Enable worktrees once per integration:

```sh
krabb worktree setup discord --force
krabb worktree setup tui --client --force
```

Setup confirms creation of a managed Git repository under
`data/workspaces/repository/`, with an empty initial commit. It does not initialize
Git in the plain workspace or capture its files. An operator can commit intended
baseline content in that managed repository. Krabb disables hooks, checkout filters,
and automatic fetching when creating worktrees.

One worktree is created and reused per conversation at runtime, including DMs,
groups, topics, and TUI sessions. Discord uses channel/thread IDs, not server names.
Content lives in `KRABB_HOME/worktrees/<conversation>/`; Git's `.git` pointer is
reserved. History snapshots, memory, and structured records live privately under
`data/workspaces/conversations/<conversation>/`. On first use, only existing exact-conversation
memory and structured records are imported. System instructions always come from
`data/system/KRAB.md` and `KLAW.md` through the host.

The agent receives confined file tools (`read`, `write`, `list`, `search`), private
memory, and private structured data when enabled. Shell, Git/patch tools,
third-party tools, desktop/browser control, and scheduling are unavailable because
they do not enforce this boundary. Git itself is not an OS sandbox; installed
plugins and the operator remain trusted.

```sh
krabb worktree disable discord --force
krabb worktree disable tui --client --force
```

Disabling preserves worktree content and private data for later reuse. History and
retained attachments return to ordinary storage. Resume a TUI session with worktrees
disabled before renaming it. `--force` restarts the daemon; otherwise restart it
yourself. Update the tools, memory, SQLite, and Whisper plugins together with the
host. Unsupported guarded operations fail without falling back to unfiltered storage.
