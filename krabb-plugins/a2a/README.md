# A2A

> **Experimental:** This plugin is not fully tested and is outside the supported
> v0.1 scope. See the [functional test matrix](../../FUNCTIONAL.md) for the
> components included in v0.1.

The A2A plugin is a client for the released Agent2Agent Protocol 1.0 JSON-RPC
binding. Configure `KRABB_A2A_AGENT_CARD` with an agent-card URL, then use
`krabb a2a discover`, `krabb a2a send PROMPT`, `krabb a2a status --task ID`,
or `krabb a2a cancel --task ID`. Add `--return-immediately` to request a
non-blocking task response, then poll it with `status`.

The client discovers the JSON-RPC endpoint and streaming capability from the
agent card, sends text messages, consumes task, status, and artifact updates
over SSE when advertised, retrieves task status, and requests cancellation.
It accepts an optional bearer token through `KRABB_A2A_TOKEN`, bound to the
configured agent card's origin (scheme, host, and port). RPC endpoints must
share that origin. For separate discovery and RPC hosts, explicitly set
`KRABB_A2A_RPC_ORIGIN` to the trusted RPC origin; this mapping applies only to
cards on the configured discovery origin. Unrelated `--card` overrides cannot
reuse the configured token or mapping. Discovery requests never carry the token.
Untrusted RPC origins are rejected before sending task content, even without a token.
OAuth and A2A
server mode are not implemented. It rejects non-HTTPS endpoints except
loopback fixtures and never follows redirects.

Add `--quote "Earlier message"` to a message request to include bounded reference
context alongside the prompt. The adapter sends ordinary protocol text; it does
not require the external agent to support a Krabb-specific message type.

The legacy JSON-RPC operation interface exposes `operation.list`,
`operation.start`, `operation.inspect`, `operation.cancel`, and `operation.result`.
Starting `delegate` requests an immediate response and returns an opaque handle
for later inspection. The handle preserves the discovery card, RPC endpoint,
and remote task ID. Each inspection rechecks endpoint authorization and rejects
endpoint changes. A cancellation request reports the state returned by the backend.

Pause, wait, idempotent start, and recovery by admission ID are unavailable. Keep a successful start
receipt and inspect its handle after a disconnect. A lost start receipt requires review before
another start. The runtime saves admitted handles and monitors them without inference, then resumes
the original task with the terminal receipt. Backend pause and additional-input submission are
unavailable; a pause waits for existing remote work to settle and retains its result.

New extended operations use the optional [runner plugin](../runner/README.md), which reuses this
transport library with its own configuration and secret. The legacy A2A operation interface remains
available for previously saved handles. The runtime's local process-recovery fixtures exercise
runner and this backend. Run `cargo test --locked -p krabb-runtime --test operations` to exercise
host, adapter, and backend restarts, lost start/cancel receipts, and original-conversation
completion without paid APIs.
