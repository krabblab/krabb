# AGENTS.reference.md

## Provenance And Decisions

- Agentskill Version: `2.1.0`.
- Evidence Schema Version: `4`.
- Repository Revision: `e0db408f112573f60614e7b02c47f1929822a2a7`.
- Configuration: default signature enabled.
- Maintainer-Confirmed Decisions: capability-free core; process plugins over JSONL; Apache-2.0; one-word naming where clear; Lefthook pre-commit gates; Gitfleet-style releases; the implementation ledger is authoritative.
- Unresolved Uncertainty: vendor OAuth terms remain provider-specific. Roadmap capabilities require
  implementation evidence; full host upgrades, migrations, and rollback must not be inferred from
  existing plugin update safeguards.

## Maintainer Principles And Evidence

The maintainer confirmed simplicity, consistent CLI/TUI output, low resource use,
shared implementations, safe upgrades, credential cleanup on uninstall, and
verified recovery as development requirements. ROADMAP.md records future scope;
it does not establish that these features already ship. Documentation-only work
uses targeted checks; Rust edits retain full verification and coverage gates.

This update used Agentskill 2.1.0 scope discovery and evidence (schema 4), with
signature auto and deep workflow scope for existing managed root, runtime, CLI,
plugin, and TUI guidance. Evidence facts `configuration.signature`,
`tool.rust.formatter`, and `test.command.1` were checked against configuration
and Makefile. Direct inspection covered Cargo.toml, krabb/src/main.rs,
krabb-core/src/agent.rs, runtime setup and updates, plugin update tests, and TUI
entrypoints. The working tree included the maintainer's uncommitted roadmap.

Root principles are inherited; scoped guidance adds local responsibilities.
Generated guidance preserves each Free Region. No runtime personality templates
or newly suggested guidance scopes are part of this update.

## Boundaries And Ownership

`krabb-core` owns normalized types, JSON-RPC framing, policy, and the bounded
turn loop. `krabb-runtime` owns host orchestration, daemon state, authenticated
IPC, and plugin lifecycle, while `krabb` and `krabb-daemon` are thin
entrypoints. `krabb-libs/file` owns shared filesystem primitives. Plugin
crate directories under `krabb-plugins` are process boundaries; `krabb-plugins/intelligence/`
and `krabb-plugins/channels/` only group plugins. Manifests declare capabilities.
`krabb-docs` is the user-facing source of truth; `krabb-scripts` contains
release, metrics, verification, and review helpers.

## Development Workflow

Lefthook runs format, locked workspace check, and library/binary tests before a
commit. `make verify` runs formatting, Clippy, locked check, tests, coverage,
build, and metrics. Coverage uses cargo-llvm-cov without source exclusions.

## Testing Topology

Unit tests live beside Rust code. Protocol tests use in-memory JSONL fixtures.
Provider tests must use deterministic fake responses and never call live paid
APIs. Release scripts must validate `VERSION`, changelog headings, archives,
and checksums.

## Nested Scopes

Child scopes inherit this document additively:

- `krabb-core/` — kernel, policy, types, and protocol.
- `krabb-runtime/` — host orchestration, daemon state, IPC, and plugin lifecycle.
- `krabb-libs/file/` — shared private and crash-safe filesystem primitives.
- `krabb-plugins/` — independently released capability processes.
- `krabb/` — thin host CLI entrypoint.
- `krabb-daemon/` — thin daemon entrypoint.

## Naming

Prefer `load`, `save`, `send`, `route`, `approve`, and `retry` over redundant
compound names. Keep public artifact prefixes only where package uniqueness
requires them.

---

> Generated and maintained by [Agentskill](https://github.com/airscripts/agentskill).
> Do not touch this file. It is automatically managed by Agentskill.
