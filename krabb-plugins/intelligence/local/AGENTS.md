# AGENTS.md

## Scope

- Path: krabb-plugins/intelligence/local
- Parent: krabb-plugins
- Inheritance: additive

## Mission And Repository Map

Provide `model`, `vision`, and `context` capabilities for any OpenAI-compatible inference
server, such as vLLM, llama.cpp, or LM Studio, through the Chat Completions transport the
shared `krabb-openai` crate provides.

## Non-Negotiables

- Keep server and key configuration in the declared `KRABB_LOCAL_*` variables.
- Allow plain HTTP only for loopback, private, and local-network hosts.
- Send `KRABB_LOCAL_KEY` only to an address the user set, never to a found server.
- Report a server that refuses tool calls; never emulate tool calling.
- Never require a live inference server in automated tests.

## Quick Start

```bash
cargo test --locked -p krabb-plugin-local
cargo build --locked -p krabb-plugin-local
```

## Implementation Conventions

Keep server discovery, validation, and model metadata in `src/server.rs`; keep wire
formats in the shared transport so this plugin and the OpenAI plugin stay in step.

## Testing And Validation

Use local HTTP fixtures for discovery, keys, streaming, tool refusals, missing models,
and unreachable or stopped servers.

## Free Region

Keep server-specific behavior behind the OpenAI-compatible API and document new
configuration variables in its README.

## Further Context

See the parent plugin guidance and [README.md](README.md).

---

> Generated and maintained by [Agentskill](https://github.com/airscripts/agentskill).
> Do not touch this file. It is automatically managed by Agentskill.
