# Task IPC

Authenticated local clients use these daemon methods to recover requests and
control saved tasks. See [Commands](../krabb-docs/commands.md#request-recovery) for
the user-facing behavior.

## Table Of Contents

- [Request Recovery](#request-recovery)
- [Deferred Task Resume](#deferred-task-resume)
- [Task Inference Grants](#task-inference-grants)

## Request Recovery

Authenticated local clients can call `task.receipt` with
`{"session":"tui-work","client":"tui","request":"saved-message-id"}`.
The response has `found`; retained tasks also include `task: {id, state}`,
`provider`, and nullable `text`. Text is exposed only for a verified reporting or
completed result. When retention has removed the task, the response is
`{"found":true,"expired":true,"task":{id,state}}` with no text, and answering or
queueing that request ID again is refused instead of running it a second time.
Lookup bypasses occupied work slots and plugin-update locks.
`session.answer` accepts the same stable ID as optional `request`; a repeated
accepted request returns its receipt or saved answer before provider discovery,
with `done` true only when the saved answer is final.
Without `request`, the host uses the conversation's saved last user message from
that client as the identity, so a client that resends after a lost reply also
receives the saved result instead of a second execution.
Repeated reserved message appends are idempotent while the saved message remains;
reusing its identity with different content is rejected.
Unknown IDs must match the latest saved user message before admission. Reusing an
accepted identity with a different provider is rejected.

Receipt lookup uses the conversation's captured client identity and retained task
records. Unknown IDs return `found: false`; conflicting legacy records require
review. Lookup remains bounded by task retention. Conversational task discovery
and controls belong to the originating conversation; existing context-sharing
visibility grants do not grant task-control rights. Older daemons without receipt
lookup retain their existing behavior.

One-shot TUI requests keep temporary conversations when saved work, pending results,
or uncertain effects still need them, and print a reopen command. The host's
`session.cleanup` endpoint takes `id` and returns `deleted: false` while the client
conversation is needed; otherwise it removes the idle conversation and private
records. Ordinary deletion also rejects protected task state. If cleanup is
unavailable or its result cannot be confirmed, the client reports that uncertainty
and provides a command to check the conversation. It does not claim deletion or retention.

## Deferred Task Resume

Authenticated local clients can also call `task.defer` with `id` and
`command: {id, revision, when: {at, timezone}}`. `at` is whole Unix seconds;
`timezone` is a valid named zone or UTC offset. Use the current task revision and
a stable command ID for retries. The response includes a durable receipt and
current state. Repeating the request returns its retained receipt without
rescheduling. `task.list` exposes the active `resume` time. These host controls
bypass occupied execution slots and plugin-update locks; they do not call a model.
Saved authority is revalidated before execution, and uncertain effects require
review instead of automatic resume.

## Task Inference Grants

Authenticated local clients can inspect `task.list` and grant a larger cumulative
inference ceiling with `task.budget`. The payload contains `id` and `grant`:

```json
{
  "id": "task-123",
  "grant": {
    "id": "grant-123",
    "revision": 4,
    "sequence": 2,
    "limit_millis": 7200000
  }
}
```

Use the task revision and sequence from the current listing and a stable grant ID
for retries. The amount is the new total ceiling. Grants preserve confirmed use
and unresolved exposure, fit the configured host ceiling, and leave pause/block
state unchanged. A changed task, active reservation, untracked historical usage,
terminal task, or conflicting request ID rejects the grant. Receipts survive restart;
duplicate requests return the retained receipt. Receipt history holds 32 grants.
If the proposed total exceeds `inference.task_seconds`, an administrator must first
raise the host ceiling and restart the daemon to load that policy. This change
alone does not raise the saved task's ceiling. Resume the saved task separately
after reviewing the grant receipt. This API uses local
daemon administrative authority and bypasses occupied work slots. Model task tools
cannot call it. Conversational grant UI and usage reconciliation remain planned.
