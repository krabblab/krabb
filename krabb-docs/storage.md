# Storage

Krabb separates installation settings, durable service data, runtime files,
diagnostics, and conversation workspaces. `KRABB_HOME` defaults to
`$XDG_CONFIG_HOME/krabb` or `$HOME/.config/krabb`; an explicit override wins.

```text
KRABB_HOME/
  config.toml
  schema.json                     # home schema version, separate from the app version
  plugins.lock
  credentials.json
  credentials.key                 # fallback when no OS keyring is available
  plugins/<plugin>/               # installed code and manifests
  data/
    inference.json                # bounded advisory timing cache, no prompts
    sessions/index.json           # atomic catalog, cursors, deliveries, run receipts
    sessions/records/             # immutable conversation snapshots
    sessions/state/deliveries/    # immutable delivery records
    tasks/<task-id>/records/      # private task context and public drafts
    schedules/schedules.json     # scheduler-owned task state
    schedules/runs/<job>/<run>/result.json
    memories/                    # default memory index and records
    plugins/<plugin>/            # cursors, inboxes, SQLite, local TUI state
    media/                       # downloaded media, including pending input
    system/                      # host-loaded KRAB.md and KLAW.md
    workspaces/
      files.json                 # plain workspace file provenance
      repository/                # managed Git baseline and metadata
      conversations/<session>/   # private history, memory, structured records
  run/                           # daemon.lock, ipc.token, ipc.port
  cache/                         # disposable files only
  logs/                          # daily diagnostics, debug/, schedules/
  workspace/                     # ordinary user/agent content and attachments/
  worktrees/<session>/           # private content, attachments/, Git .git pointer
```

`data/inference.json` stores up to 64 opaque profile identities with 32 timing
observations each, capped at 1 MiB. Atomic private writes preserve an earlier valid
cache on failure. Missing or invalid timing history falls back to conservative
priors; deleting it does not reset task state or grant resource authority.
Cache format 2 adds attempt outcomes; format 1 success observations migrate on
load, with format 2 written on the next observation. Incomplete attempts retain
unknown usage and capped elapsed time. Writes are coalesced; a crash may lose
recent timing observations. This cache does not provide durable spending receipts.

Workspace directories contain content; persistent agent state belongs under data.
Worktree-private state is host-managed and inaccessible to conversation file tools.
The central session catalog retains routing and private-record references. No unused
agent directories or shared/global content directories are created.

Since catalog version 8, each saved task has a constant-size execution account:
its captured ceiling, confirmed use, unresolved exposure, sequence, and pending
reservation. Up to 32 scoped administrative grant receipts are retained per task.
Pending reservations protect their task from retention even after a terminal state.
Reservation persistence precedes generation dispatch. Settlement is
saved before accepting a completed inference result. Restart retains unresolved
reservations without billing offline wall time as observed work. Old catalogs
migrate with earlier usage marked unknown; missing accounts in current catalogs
fail closed, including version 8 records read by a newer daemon. Older daemons
reject newer catalog versions rather than silently discarding balances.
Keep the catalog and private task journals together when backing up task state.
The disposable timing cache cannot replace or reset these records.

Sessions and deliveries stay in the same atomic catalog so a crash cannot publish
a reply without its transcript or cursor. Unchanged immutable records are reused
only after verification. Full schedule results are limited to 256 KiB; each result
has a frozen destination plan and a separate receipt per target. A result is saved
before its deliveries are published together in the catalog. Delivery failure does
not cause another model execution.

Catalog version 15 stores foreground and background tasks, checkpoints, captured endpoint identities
and permissions, recovery phases, and pending notifications. It also keeps each saved task's pending
approval: the exact proposed tool call, its bounded description, and the recorded decision. It also
retains operation admission intent, opaque handles, receipts, and monitoring backoff. Minimal and
extended background execution share this catalog and its private journals; runner does not introduce
another task database. Typed retry reasons and earliest retry times survive controls and restart.
Upgrading a legacy pending retry without a known reason blocks for review, preserving its input,
checkpoint, and prior use. Versions 1 through 14 remain readable; older writers reject version 15
rather than discard task state. Foreground task admission saves the task and its private context
together. A foreground journal keeps only its own turn messages and prompts plus a reference to the
last conversation message it covers; it reads the conversation up to that point instead of copying
it. Loading an older catalog converts saved copies whose messages are still in their conversation.
The catalog references immutable task journals under `data/tasks/`. When backing up a stopped
runtime, include that directory and `data/sessions/` together, along with any referenced workspace
and attachment data. A checkpoint and its tool receipt commit together. Client messages and channel
outbox entries commit with their notification receipt, so delivery recovery does not re-execute the
goal. At most 64 job records are retained; capacity reclaims terminal records whose notifications
are published and protects active, paused, and blocked work. Commands carry expected revisions and
durable receipts; bounded receipt retention still rejects stale commands after old receipts expire.
Manual pauses remain paused after restart. Legacy paused blockers migrate to the explicit blocked
state. Worker generations fence checkpoint, tool, draft, and compaction writes after a stop or a
later execution. Control receipts appear in the conversation transcript and are omitted from the
model's working context. Operation handles are inspected after restart without starting the external
work again. A lost start receipt blocks the task for review. Cancelled tasks with outstanding
operations and tasks with uncertain effects remain protected from eviction.

Accepted foreground and queued client requests retain the source user-message ID. `task.receipt`
reads the matching task and verified result without another model call or a journal rewrite. Full
receipts last as long as the task record remains. When retention removes a finished task, the
catalog keeps its request identity, task ID, and final state in a ledger of the most recent 256
retired requests, so lookups report an expired receipt and the same request cannot be admitted
again. Protected task work, pending notifications, uncertain effects, and unresolved inference
accounts also prevent deletion of their conversation. Temporary one-shot cleanup keeps such
conversations available for later delivery and control.

Newly accepted foreground and queued requests retain a fingerprint of their normalized sender and
content, plus the original user message in the private task journal. Trimming conversation history
does not remove that input or reclaim its referenced attachments. Background continuation receives
the original requirements alongside its checkpoint and operation receipts; a clipped goal preview is
not the complete request. When its text exceeds 16 KiB, each step receives the first 8 KiB and a
note of the remaining length, plus a read-only `input` tool that returns exact 16 KiB pages of the
saved original, so a large request is not re-sent whole. The fingerprint describes normalized input
metadata, not a separate verification of attachment file bytes.

Reusing a retained request ID with different content is rejected even after the
ordinary history is trimmed. Missing or changed pinned input blocks continuation;
an exact fingerprint-matching replay can repair an existing private journal but
does not resume the task or grant more inference allowance. A lost journal is not
recreated from a replay because its effect history may be missing too. Older
catalogs retain unknown fingerprints rather than inventing evidence of accepted
content. Conflicting legacy tasks remain saved and fenced for review; existing
external operations can still be reconciled. Receipt expiry and explicit conflict
resolution remain limitations of the bounded task catalog.

One-off deferred resume stores an absolute instant, interpreted timezone, and up
to 32 durable receipts per task in the same catalog. It uses no scheduler record.
Restart preserves the saved pause and releases due work once; offline time does
not replenish its captured inference allowance. Manual pause/resume or cancellation
clears the active timer, and uncertain effects clear it for review. Older catalogs
have no deferred timer. Keep these records with task journals when backing up.

## Retention

Completed, acknowledged schedule runs expire after 30 days when new runs are
published, with at most 1,000 tracked runs. Unfinished or unacknowledged deliveries
are protected; full storage applies backpressure rather than deleting them.
Diagnostic schedule history contains excerpts and rotates per file at 256 KiB.
At daemon startup and hourly thereafter, each diagnostics directory is limited to 64 MiB, 1,000 files,
and 30 days; disposable cache files are limited to 256 MiB, 1,000 files, and 30 days.
Cleanup preserves subdirectories, links, and transaction files.

Pending input is durable data, so `data/media` is not the disposable `cache/`.
Conversation media has its own [bounded retention](sharing.md#files-and-attachments).
Krabb does not automatically delete user workspace files, credentials, or plugins.

## Upgrades And Custom Paths

`schema.json` records the home schema, versioned independently of the application
and plugin protocol; a home without it is schema 0. `krabb-migrations` holds an
ordered registry in which step `n` migrates schema `n` to `n + 1`, so skipped
releases run every intermediate step. Before the first step, each path the
pending steps declare is copied to `.migration/` and verified byte for byte, then
the `.migration.json` journal is committed. The daemon and every command that
takes exclusive ownership of the state apply pending steps first. A failed step,
or a journal left by an interruption, restores the recovery point before state
is opened. Krabb refuses a home with a newer or unreadable schema before changing
it; `version`, `completion`, `service`, and `init --force` remain available.

Existing root channel state, `krabb.db`, and `media/` remain usable until an offline
migration moves them. New installations use the layout above. IPC clients can
connect to a legacy daemon, and the new daemon also acquires an existing legacy
`daemon.lock` to prevent concurrent writers. That legacy lock inode may remain at
the root after migration; metadata/plugin transaction locks also stay beside the
state they protect.

`KRABB_ROOT`, `KRABB_DB`, `KRABB_MEMORY`, and `KRABB_MEDIA` remain explicit path
overrides. Run migration as the daemon's OS user with the same environment. A service's
configured environment is independent of the interactive shell. If an override
must keep a legacy path, exclude that category from migration. Existing common
workspace files have no reliable conversation owner and remain untouched. Assign
visibility explicitly with `krabb workspace <path> --owner <integration-or-global>`.

An offline migration must lock both daemon locations and plugin/metadata state,
reject old/new conflicts, back up original data, update attachment references,
and publish newly hashed immutable records before catalog references. It must not
edit hash-named records in place. An interrupted migration leaves `.migration.json`;
the next exclusive owner restores the recovery point before opening state. Keep
backups until you have checked chats, pending deliveries, memory, and scheduled jobs.
