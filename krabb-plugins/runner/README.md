# Runner

Runner provides optional extended execution for background requests. Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- runner
```

The request classes are **fast**, **thoughtful**, and **background**. Minimal
background execution continues ordinary agent work in bounded, checkpointed host
turns and works without runner. Extended execution uses runner's advertised
operations. Both modes share the host task registry, journals, control receipts,
budgets, restart recovery, and automatic completion delivery. Neither mode needs
the scheduler; that plugin supplies timed and recurring triggers.

The initial extended operation, `delegate`, starts an external A2A agent task and
returns a durable handle. Runner reuses the A2A transport library, including its
origin checks and bounded response parsing; it does not load another plugin or
maintain a second task database. Arbitrary local process execution is unavailable.

Set `KRABB_RUNNER_AGENT_CARD` to an agent-card URL or supply `card` in the operation arguments.
Without a valid configured card, the declaration requires `card`, so the host rejects incomplete
requests before recording any dispatch. `KRABB_RUNNER_RPC_ORIGIN` explicitly trusts a separate RPC
origin for the configured card. Optional `KRABB_RUNNER_TOKEN` is declared as a secret in the
manifest and can be stored through `krabb plugin setup runner`. Discovery never receives this token.
Unrelated card overrides cannot reuse it or the configured origin mapping. Redirects and non-HTTPS
endpoints outside loopback are rejected.

The plugin negotiates the `runner` capability and `operation.list` declarations.
The host advertises `operation_runner_delegate` only after this negotiation.
Malformed declarations withhold extended tools while ordinary work stays available.
Replacing or restarting runner invalidates previously discovered offers; new work
must discover capabilities again. Existing saved handles remain available for
reconciliation, including handles admitted through older adapters.

`delegate` supports inspection, result collection, and cancellation requests.
It does not offer backend pause, events/wait, input submission, idempotent
start, or recovery by admission ID. Losing a start receipt blocks replay because
the remote effect may already exist. The host retains the receipt and handle,
releases inference while waiting, and inspects with backoff. A plugin outage never
authorizes another start or a synchronous fallback.

Use natural language in the originating conversation to inspect, pause, resume,
cancel, or steer the saved task. These host controls still work when runner is
missing. A pause records intent and retains the external result when it settles;
a cancellation records intent and requests interruption only when supported.
Neither receipt claims that a remote operation stopped before confirmation.
Visibility and context sharing do not grant control from another conversation.

Run the native protocol and process recovery fixtures without paid APIs:

```sh
cargo test --locked -p krabb-plugin-runner
cargo test --locked -p krabb-runtime --test operations
```
