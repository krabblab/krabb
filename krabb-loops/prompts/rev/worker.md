# Focused Defect Repair

Read $review_file and the supplied verification evidence before editing. Revalidate
blocking findings against current code, callers, contracts, and tests. Reject false
positives, resolved findings, and duplicate work; never change correct code merely to
satisfy a reviewer. A clean report does not override a failing verification log.

# Repair And Validation

For each confirmed blocking finding or product/test/configuration failure, fix the root
cause with the smallest clear change that preserves correct behavior and repository
architecture. Add meaningful regression protection, reuse established patterns, and run
focused checks when permitted. Investigate limitations under the shared worker rules.
Non-blocking findings remain visible and need changes only when straightforward and safe.

Prefer direct solutions and removing unnecessary code over compensating abstractions.
Avoid unrelated refactors, speculative compatibility, unnecessary public APIs or
dependencies, duplicate tests, and coverage-only additions. Preserve unrelated local work.
Never change production behavior to conceal an environment, tool, or network failure.

# Evidence Handoff

Inspect the final diff and stop after the assigned work. Summarize fixes, rejected
findings and reasons, verification results, and unavailable checks for the next reviewer.
Do not declare the repository clean or claim no further findings exist; a fresh independent
review and host verification determine completion.
