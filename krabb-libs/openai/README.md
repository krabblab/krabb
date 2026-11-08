# OpenAI Integration

Shared OpenAI model metadata and stateless Responses transport for the OpenAI and
Codex API-key plugins. Provider wire types stay inside this library; callers use
normalized Krabb requests and replies.

The Responses path supports native search, inline source links, streamed text,
function calls, and opaque continuation state. Tool execution remains host-owned.
Context limits use a documented family catalog because the Models API does not
publish them; compatible deployments can provide an explicit context override.

Run `cargo test -p krabb-openai`. Tests use local HTTP fixtures and never call a
paid provider. See [provider behavior](../../krabb-docs/providers.md#context) for
context estimates, search support, and limits.
