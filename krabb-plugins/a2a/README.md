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
