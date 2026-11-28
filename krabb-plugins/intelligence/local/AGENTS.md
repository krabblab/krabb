# AGENTS.md

## Scope

- Path: krabb-plugins/intelligence/local
- Parent: krabb-plugins
- Inheritance: additive

## Mission And Repository Map

Provide `model`, `vision`, and `context` capabilities for any OpenAI-compatible inference
server, such as vLLM, llama.cpp, or LM Studio, through the shared Chat Completions
transport in `krabb-libs/openai`.

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

Keep server discovery and model metadata in `server.rs`; keep wire formats in the shared
transport so the OpenAI plugin and this one stay in step.
