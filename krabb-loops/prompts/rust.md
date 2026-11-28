# Idiomatic Rust

The project's standards live in the repository, not in this prompt: root and applicable
nested AGENTS.md for conventions, and krabb-handbook/architecture.md#test-organization for
where tests live. Apply them in every review, investigation and implementation, including
tests. When they are silent, follow the crate's established patterns.

Beyond those standards, hold changed code to idiomatic Rust: clear ownership and borrowing,
enums for meaningful states, standard iterators and pattern matching, narrow visibility,
and RAII for cleanup. Flag unnecessary clones and allocations, unwraps in fallible
production paths, blanket lint suppression, speculative abstractions, and APIs widened only
to enable tests.

Tests must protect behavior with deterministic local fixtures and useful assertions.
Preserve regression coverage when moving tests, including subprocess test-filter names,
platform gates and cleanup. Do not rewrite unrelated idiomatic code or raise style-only
findings merely because a different valid layout is possible. Run rustfmt, Clippy and the
relevant tests; host verification and coverage remain required before convergence.
