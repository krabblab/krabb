# Storage

Krabb separates installation settings, durable service data, runtime files,
diagnostics, and conversation workspaces. `KRABB_HOME` defaults to
`$XDG_CONFIG_HOME/krabb` or `$HOME/.config/krabb`; an explicit override wins.

```text
KRABB_HOME/
  config.toml
  plugins.lock
  credentials.json
  credentials.key                 # fallback when no OS keyring is available
  plugins/<plugin>/               # installed code and manifests
  data/
    sessions/index.json           # atomic catalog, cursors, deliveries, run receipts
    sessions/records/             # immutable conversation snapshots
    sessions/state/deliveries/    # immutable delivery records
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

Workspace directories contain content; persistent agent state belongs under data.
Worktree-private state is host-managed and inaccessible to conversation file tools.
The central session catalog retains routing and private-record references. No unused
agent directories or shared/global content directories are created.

Sessions and deliveries stay in the same atomic catalog so a crash cannot publish
a reply without its transcript or cursor. Unchanged immutable records are reused
only after verification. Full schedule results are limited to 256 KiB; each result
has a frozen destination plan and a separate receipt per target. A result is saved
before its deliveries are published together in the catalog. Delivery failure does
not cause another model execution.

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
Krabb refuses to open state until migration rollback completes. Keep backups until
you have checked chats, pending deliveries, memory, and scheduled jobs.
