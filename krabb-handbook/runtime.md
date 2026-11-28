# Runtime

This guide explains how the background runtime admits events, runs turns, delivers
replies, and recovers after a crash. See [Concepts](../krabb-docs/concepts.md) for
sessions, tasks, and agent instructions, and [Safety](../krabb-docs/safety.md) for
defaults and confinement.

## Table Of Contents

- [Events](#events)
- [Inference And Background Work](#inference-and-background-work)
- [Delivery](#delivery)
- [Leases](#leases)
- [Recovery](#recovery)
- [Limits](#limits)

## Events

Channel plugins convert provider payloads into stable events before the host
sees them. An event carries an ID, channel, chat, sender, privacy scope, text,
attachments, and optional topic or thread metadata. The host uses the event ID
for deduplication and advances the channel cursor only after durable admission.

## Inference And Background Work

Providers advertising execution deadlines receive an allowance estimated from
bounded deployment/model observations and conservative fallback priors. A healthy
slow local model can exceed one minute without changing a request's semantic
class. Context preparation and generation share the admitted deadline. Legacy
providers retain one-minute fast and five-minute thoughtful allowances.

The `[inference]` configuration bounds active request and queue allowances.
Prediction is advisory admission policy. Saved host tasks retain cumulative inference
allowance accounts across retries and restart; `inference.task_seconds` optionally
sets a ceiling. Conversations take a provider's next free slot ahead of background
and scheduled work; a waiting background request runs after at most three
consecutive conversation turns. A busy single-slot provider still answers one
request at a time. Task token limits are supported; currency cost limits are not.

For background tasks, negotiated inference uses the adaptive allowance; legacy
inference and ordinary tool calls retain their ten-minute background limit. Durable external operations can outlive a step. A task can span days;
there is no overall lifetime deadline. Waiting tasks do not call the intelligence
provider until their next step is due.

Minimal background execution uses ordinary agent turns without another executor.
Extended execution uses optional runner operations with durable handles. Both
modes use the same saved task state and automatic delivery. Runner handles
specialized execution; the scheduler supplies timed and recurring triggers.
A missing runner preserves saved state and generic controls; unsupported extended
work needs an available capability before it can start.

## Delivery

Replies are persisted with the transcript and placed in a durable outbox.
Channel acknowledgement is separate from reply generation. The host marks a
send before dispatch and quarantines any non-success response as uncertain;
operators decide whether a retry is safe. Deduplication prevents a provider
retry from creating a second session message.

## Leases

An active turn holds a durable lease. Safe leases can be restored after a
daemon restart; leases that may have reached a mutating tool are recorded as
interrupted instead of replayed. Cancellation clears the lease and does not
retry the canceled message. The cancel command reports success only after the
active turn has stopped and acknowledged the request.

## Recovery

Accepted events, leases, transcripts, and outbox entries are durable. A crash
can retry only a lease proven safe; a mutating phase is never replayed. A
canceled turn is discarded. Ambiguous deliveries are not retried automatically;
an explicit delivery retry does not rerun model or tool work. Unix plugins run in
private process sessions; host cancellation and restart cleanup terminate shell
commands in those sessions even when they use separate process groups.
Linux host shells inherit a syscall filter that rejects `setsid` and `setpgid`,
so descendants cannot detach from cleanup. Other Unix platforms use the tracked
container sandbox for shell containment.
Windows tool commands start suspended and enter a kill-on-close Job Object before
execution. Completion, failure, cancellation, and plugin termination stop their
descendants, including background commands whose parent has already exited.

## Limits

Protocol frames, model context, session history, queues, archive extraction,
search work, plugin metadata, provider bodies, and persisted values are bounded.
When a limit is reached, Krabb reports an error or dead-letters the item
instead of silently expanding resource use.
