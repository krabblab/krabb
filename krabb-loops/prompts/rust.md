# Idiomatic Rust

Apply these conventions in every review, investigation and implementation, including tests.
Follow the crate's established idiomatic patterns and applicable AGENTS.md instructions.
Prefer clear ownership and borrowing, enums for meaningful states, typed errors and `?`,
standard iterators and pattern matching, narrow visibility, and RAII for resource cleanup.
Avoid unnecessary clones, allocations, unwraps in fallible production paths, unsafe code,
blanket lint suppression, speculative abstractions and widening APIs only to enable tests.
Keep async work bounded and cancellation-safe. Preserve formatting and logical-phase spacing.

Place private implementation tests under the owning module's `#[cfg(test)] mod tests`.
Keep small suites inline. Split larger suites into named child modules under `src/tests/`
(or the owning module's tests directory) when that improves navigation; use ordinary module
resolution. A `_tests.rs` suffix is not required by Rust and does not register tests.
Prefer a single coherent tests module over parallel crate-root test modules.

Use a crate's top-level `tests/` for integration tests exercising public APIs or binaries.
Keep integration-only support under `tests/support/mod.rs`, not as an accidental extra
integration-test target. Do not put private tests there or make production APIs public just
for testing. Share substantial cross-crate fixtures through a dev-dependency with explicit
interfaces; avoid path-importing another crate's source or parent-dependent test injection.
Test-only dependencies belong in `[dev-dependencies]`. Keep tests discovered by Cargo.

Tests must protect behavior with deterministic local fixtures and useful assertions.
Preserve regression coverage when moving tests, including subprocess test-filter names,
platform gates and cleanup. Do not rewrite unrelated idiomatic code or create style-only
findings merely because a different valid layout is possible. Run rustfmt, Clippy and the
relevant tests; host verification and coverage remain required before convergence.
