# Development

Use these checks while changing Krabb. See
[Local Development](../krabb-docs/workflows.md#local-development) to build and link
Krabb and its plugins from source.

Before opening a pull request, check Rust formatting and repository block
spacing with `make fmt`. Apply both formatters with:

```sh
cargo fmt --all
make spacing
make fmt
```

`make spacing` separates control flow, multiline statements, and setup/assertion phases
in Rust and Python; it also checks JavaScript block spacing. It preserves embedded strings
and fixtures and is idempotent. Keep the inserted blank lines when editing.

Make targets, Git hooks, and engineering loops build with two parallel jobs to bound
memory; raise it on larger machines with `make verify JOBS=4` or `CARGO_BUILD_JOBS`.
Dev builds keep only line tables for debugging, which keeps backtrace file and line
numbers while cutting link memory and test binary size.

`make coverage` tests the runner, then measures each package with fresh workspace
coverage data. Dependency builds remain cached; cleanup, test, and threshold failures
fail the gate.

Continuous integration also checks the `x86_64-pc-windows-gnu` target from
Linux. The job installs MinGW for native dependencies before running the
locked workspace check; release archives continue to use the supported MSVC
targets.

Pull requests test only what they can affect. `krabb-scripts/changes.py` maps the
changed files to their workspace packages and every package that depends on them, and
the crate tests, coverage, and plugin integrations run for those alone. Changes to only
documentation or assets skip the Rust jobs. Shared files such as `Cargo.lock`,
workflows, and scripts select every package, and pushes to `main` and manual runs
always test everything before a release.

Repository scripts keep their responsibilities separate and expose `--help`.
Use `krabb-scripts/metrics.sh` for Rust, script, and test counts; use
`krabb-scripts/check.sh ARCHIVE` to validate a release archive without
extracting it.
`make loops` also runs `krabb-scripts/links.py`, which reports every relative Markdown
link whose file or heading anchor is missing.

On a Linux host with Docker or Podman available, run
`krabb-scripts/sandbox.sh target/debug/krabb-plugin-tools` to verify the
networkless, read-only-root tools sandbox against a local image. The script
uses only an image already present on the host and does not pull one.

Use `make ci` for new or changed GitHub Actions workflows and for debugging
workflow behavior that depends on action ordering, inputs, runners, or
artifacts. It runs `krabb-ci/ci.sh`, which performs the local preflight and
then exercises the important Actions flow through `act`. For ordinary source
changes, use `make verify`, `make test`, and `make build` without the container
workflow.
