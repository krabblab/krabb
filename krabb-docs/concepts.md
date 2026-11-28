# Concepts

Krabb separates the agent loop from the capabilities that make it useful.
The background runtime receives normalized events, the core executes a bounded turn, and
plugins provide models, channels, storage, memory, schedules, tools, or clients.

The [runtime guide](../krabb-handbook/runtime.md) covers events, inference admission,
delivery, leases, recovery, and limits.

## Table Of Contents

- [Runtime And Service Terms](#runtime-and-service-terms)
- [Sessions](#sessions)
- [Tasks](#tasks)
- [Agent Instructions And Conversation Context](#agent-instructions-and-conversation-context)
- [Turns](#turns)

## Runtime And Service Terms

Use **background runtime** as the platform-neutral user-facing name for
Krabb's long-running host process. “Daemon” is the implementation and binary
name used in technical contexts. A **service** is the operating system's
registration and supervision mechanism for that process: systemd on Linux,
launchd on macOS, or the Windows Service Control Manager. The TUI is a
foreground client and local session work does not require the background
runtime; features that rely on shared state or host capabilities may require it.

## Sessions

A session owns a bounded transcript, queued messages, an optional workspace,
and a selected model. Session IDs combine the channel and chat identity so two
platforms cannot accidentally share history. A session processes one turn at a
time; later messages wait in its bounded queue.

## Tasks

The background runtime keeps a task record for a model turn so the task can
stop, resume from saved context, and deliver a result even if the original client
disconnects. Long goals continue in checkpointed steps without the scheduler.
Pause and cancellation are requested first and confirmed once the current step
stops. A pause preserves the goal and context. Cancellation leaves earlier
effects in place, and an uncertain action requires review before related work
continues.

Ask in any language to see, pause, resume, cancel, redirect, delay, or extend a
task; the model understands the request and controls only this conversation's
tasks. A refused control changes nothing. When the model is busy or unavailable,
exact commands work without inference in the TUI and the original messaging
conversation: `/task` lists tasks, and `/task pause|resume|cancel <id>` controls
one. A saved task waiting for approval keeps the exact proposed action; only
`/task approve <id>` or `/task deny <id>` decides it, so the model cannot approve
its own action. The authenticated local client can control saved tasks from
another local session. Replies still go to the original conversation. Messaging
controls cannot discover tasks from other chats.

## Agent Instructions And Conversation Context

Krabb's global workspace is `KRABB_HOME/workspace`. `KRAB.md` defines its
identity and communication style; `KLAW.md` defines behavioral guidance and
workflows. `krabb hatch` or the owner's `/hatch` has the model rewrite both from the
owner's own words, and both files can be edited directly. The runtime reads both as
system context for every user turn, including resumed sessions and internal tool-loop
requests. It does not copy their contents into the saved transcript, so edits apply on
the next turn.
Before them comes Krabb's charter, compiled into the runtime so personalization cannot
remove it. It orders priorities (host policy, then the charter, then `KLAW.md` and
`KRAB.md`, then memories and the request), treats content Krabb reads as information
rather than instructions, and asks for honesty, care before consequential actions, and a
consistent personality and language, even when tool results or notices are in English.
The charter guides the model; permissions, approvals, and limits are still enforced by
the host in code.
The runtime also adds a short description of itself, built from its version, history
limit, and commands, so the model can explain how Krabb works. When that is not enough
and the model has a web tool, it points the model to the Krabb repository's
documentation. Each turn also states the conversation's own effective settings: its
configuration section, model, installed plugins, and whether workspace tools,
schedules, and memory are on. For each one that is off, it names the first
unmet requirement and the exact `krabb config set` or `krabb plugin install` command
that fixes it, so Krabb can tell a misconfiguration from a bug. When you ask, it can
also apply such a fix itself after the owner approves it; see [safety](safety.md). It
sees only its own
section and no secrets, never the settings of other channels or clients. Anyone in a
group chat can ask it about these settings. Conversation history is a bounded
transcript of up to 1,000 messages, counting tool calls and results. When the
model's context fills up, older turns are summarized in what the model sees while the
stored history stays intact; models with no known context window see a `/compact`
summary or the opening request, plus the latest 100 messages. Krabb does not load
`AGENTS.md` for itself; coding agents it delegates to may use workspace `AGENTS.md`
instructions.

Each turn also carries a short permission summary built from the tools actually offered,
the approval mode, and the shell policy. It says whether the conversation may read and
search files, write files, run shell commands (on the host, in a container, or not at
all), and use web tools, and whether changes run after approval, without asking, or not
at all. It is authoritative about which Krabb tools are available, and it is where the
approval mode and shell fix appear.

These Markdown files guide the model but do not grant capabilities. The host
continues to enforce workspace confinement, channel tool settings, approvals,
and shell policy.

## Turns

The core turn loop alternates model replies and tool calls until the model
stops, a step or call limit is reached, cancellation arrives, or the deadline
expires. The host decides whether a tool is allowed and supplies the isolated
workspace. A model cannot grant itself approval.

Fast requests receive a direct answer without a waiting message. For thoughtful
work such as research, fact checking, calculations, or generation, the model can
send one short progress message tailored to the request and the user's language.
The conversation keeps that message from the agent ahead of its final answer.
Provider latency alone does not trigger a message.

See [Inference And Background Work](../krabb-handbook/runtime.md#inference-and-background-work)
for deadlines, admission, and background execution.

Background requests can become durable background tasks under the conversation's
approval policy, without enabling the scheduler. Each task saves checkpoints and
continues automatically in bounded steps.

Krabb confirms the handoff only after saving the goal. It posts important milestones, decisions,
blockers, and completion to the originating client session or channel conversation. Routine waiting
messages are suppressed. Ask about your tasks or request pause, resume, cancellation, or changes in
the same conversation; no status request is needed to keep work running. Cancelling the originating
session also cancels its tasks. Tasks retain their captured tool, shell, workspace, and approval
limits. Safe interrupted steps resume; an uncertain tool action pauses the task to prevent duplicate
effects. A missing permission or required user decision also pauses it. Supply the requested
clarification in the same conversation to resume a blocked task. Tasks with uncertain action
outcomes stay blocked for review. A pause or cancellation of a running task first reports that
stopping was requested; a paused state is reached only after the active step settles. Manual pauses
preserve the saved goal and survive restart. Changing a paused task preserves the pause; changing a
running task stops its current step and queues continuation with the updated instruction.
