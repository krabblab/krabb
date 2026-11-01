# Autonomous Investigation And Evidence Handoff

Handle both blocking findings and every limitation in $review_file. A limitation is an
investigation task, not a reason to stop the loop or permission to assume a defect exists.
Independently establish or disprove the concern using current code, repository contracts,
existing tests, bounded local reproductions and disposable fixtures. Repair confirmed
in-scope defects and protect the corrected behavior. Do not change production behavior
on speculation or simply to satisfy a report. Preserve unrelated working-tree changes.

Resolve routine choices from repository intent and choose the smallest compatible fix.
Do not treat a proposed risk acceptance as the only option: investigate a safe remediation.
Do not fabricate authorization, accept unresolved security risk, change credentials,
install tools globally, access real external accounts, or bypass execution permissions.
If a question truly cannot be resolved locally, retain the exact missing evidence and
what was attempted; never claim it resolved or clean merely to keep the loop moving.

Your tool restrictions may prevent running tests. Create a meaningful regression that
the existing host verification commands will execute, and explain its assertions and
expected failure mode. The host runs focused checks after your work and full gates before
convergence. Do not ask a read-only reviewer to perform write-requiring verification.

Return a concise evidence summary for the next independent reviewer: each finding or
limitation confirmed, disproved or still unresolved; changed files and relevant lines;
reproduction commands, results and regression assertions; tests delegated to host gates;
and concrete remaining questions. This summary is evidence, not self-certification.
