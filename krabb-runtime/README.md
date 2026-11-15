# Krabb Runtime

`krabb-runtime` is the internal host library shared by the `krabb` CLI and
the `krabb-daemon` executable. It owns configuration, authenticated IPC,
plugin lifecycle, persistent state, channel bridging, and daemon orchestration.

The production crate does not depend on optional plugins. Plugin processes remain
independently installable and communicate through the core JSON-RPC protocol.

Development tests link the runner protocol entrypoint (using its A2A backend) to exercise the real
host and adapter across process restarts against a local persistent backend:

```sh
cargo test --locked -p krabb-runtime --test operations
```

The fixture installs linked binaries under Cargo's test directory, uses no paid
APIs, and kills owned host processes at backend acceptance barriers. It checks
saved handles, lost start/cancel receipts, effect counts, original-conversation
controls, retained accounting and partial files, and exact final delivery. These
are process-recovery tests; they do not simulate a day of wall-clock downtime or
measure production latency from the large instrumented fixture executables.
