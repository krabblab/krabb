# AGENTS.reference.md

## Provenance And Decisions

- Agentskill Version: `2.1.0`.
- Evidence Schema Version: `4`.
- Repository Revision: `e0db408f112573f60614e7b02c47f1929822a2a7`.
- Configuration: inherited from the repository root; default signature enabled.
- Maintainer-Confirmed Decisions: adopt krabb as a nested scope; the host owns lifecycle and policy while optional behavior remains in process plugins.
- Unresolved Uncertainty: coordinated host updates remain planned; plugin update behavior alone
  does not establish complete host rollback.

Evidence: `src/main.rs`, `../krabb-runtime/src/lib.rs`, and the maintainer-confirmed
UX principles in [root reference](../AGENTS.reference.md).

## Boundary

`src/main.rs` is the thin CLI entrypoint. It invokes `krabb_runtime::cli()` and
propagates its exit status; command parsing lives in the shared runtime. Host
lifecycle, authenticated local control, durable sessions, platform services, and
remote updates belong to `krabb-runtime`;
provider requests do not belong in this scope.

---

> Generated and maintained by [Agentskill](https://github.com/airscripts/agentskill).
> Do not touch this file. It is automatically managed by Agentskill.
