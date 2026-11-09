# AGENTS.reference.md

## Provenance And Decisions

- Agentskill Version: `2.1.0`.
- Evidence Schema Version: `4`.
- Repository Revision: `e0db408f112573f60614e7b02c47f1929822a2a7`.
- Configuration: inherited from the repository root; default signature enabled.
- Maintainer-Confirmed Decisions: retain plugin capability boundaries and shared host lifecycle
  machinery. Current official artifacts release together; independent component releases
  are planned.
- Unresolved Uncertainty: none specific to this scope.

Evidence: workspace Cargo.toml, [packaging workflow](../.github/workflows/package.yml), manifests,
and maintainer decisions recorded in [root reference](../AGENTS.reference.md).

## Boundary

Child plugin IDs and manifests define installation and capability discovery.
The `intelligence/`, `channels/`, and `clients/` paths are organizational groupings,
not capability boundaries. Each process uses `krabb_core::plugin::serve` or
`serve_with`, emits only JSON-RPC protocol frames on stdout, and negotiates
protocol `0.1`.

---

> Generated and maintained by [Agentskill](https://github.com/airscripts/agentskill).
> Do not touch this file. It is automatically managed by Agentskill.
