# Implementation

- Schedules use typed source/conversation access, filtered pagination, current-origin
  policy checks, and origin context/memory/data. Destination updates require approval
  and idle reports. One persisted result fans out through atomic per-target receipts;
  pending report recovery never reruns the model or resends acknowledged deliveries.
- Home ownership: plugin state lives under data/plugins, runtime files under run,
  full schedule results under data/schedules/runs, and workspace ownership metadata
  under data/workspaces. Legacy locations remain readable until offline migration.
  Unchanged snapshot records are verified and reused; completed results and diagnostic
  storage have bounded retention. Pending deliveries and user files are preserved.

- Channel activity: the host supplies incoming message IDs and DM/group scope
  to optional typing requests. Discord uses native channel typing, WhatsApp
  uses a read receipt with typing, Signal uses `sendTyping`, and Slack uses
  assistant status in supported DM threads. Local fixtures cover routing,
  malformed input, API rejection, and optional handling.
- Visibility: typed outgoing rules with data categories and resource selectors apply
  globally or per integration. Explicit local lists replace inherited lists. Grants
  are directional and nontransitive; worktrees bypass them. Context supplies bounded
  chat excerpts; memory and record reads retain provenance. Tools remain independently
  authorized. File reads/listings/searches enforce catalog ownership; writes preserve
  foreign files and return a distinct collision path.
- Workspaces: ordinary conversations use one plain workspace. Guided setup creates
  a managed Git baseline under data/workspaces and a checkout per conversation under
  home/worktrees, including DMs and TUI sessions. Private snapshots, memories, and
  records stay under data/workspaces/conversations/<conversation>. Host-loaded KRAB/KLAW live under
  data/system. No hidden runtime state is generated in content directories.
  Git receives ordinary Windows drive/UNC paths.
- Conversation storage: admitted attachments are retained in selected storage,
  with bounded transient downloads. History media follows storage changes.
  Retained voice cleanup resolves media aliases; ownership matches native path
  separators across catalog reloads.
  Memory, SQLite, and speech support a confined optional `scoped` protocol;
  unsupported operations fail without shared-storage fallback.
- Enforcement: worktree turns expose confined filesystem tools and local
  memory/data. Host shell, Git/patch, third-party tools, and scheduling are blocked.
  Runtime metadata and sibling conversation directories are protected from
  filesystem tools. Installed plugins and operators remain trusted.
- Approvals: visibility grants may reuse exact matching decisions for five
  minutes, bound to tool, arguments, canonical workspace, configured source,
  and worktree exclusion. Channel callbacks and local/TUI decisions share the
  same bounded cache after successful delivery. Local tool authorization remains independent. Tokens
  remain authenticated and sender/room authorization still applies.
