# Output Contract

Return only one JSON object, with exactly two keys: findings (array) and
limitations (array of strings). No Markdown fences or commentary. Empty findings
and limitations means no actionable in-scope work remains under this loop's criteria.

Every finding must contain exactly these common keys:
- severity: "Critical", "High", or "Medium"; use the lowest justified severity.
- blocking: boolean, separate from severity. Blocking findings and unresolved limitations
  trigger workers.
- file: repository-relative source path and optional :line location.
- problem: concrete defect or important unprotected behavior.
- evidence: inspected code/tests/contracts supporting the finding.
- impact: realistic failure mode, reproduction or regression consequence.
- action: smallest focused root-cause fix or meaningful test task.
- scope: explain its relationship to the selected scope.
- validation: specific assertions/checks that establish the correction.
All fields except blocking are nonempty strings. Do not add other common fields.
Add only the domain-specific fields explicitly required in the domain instructions.

Actively attempt to disprove findings. Deduplicate root causes. Do not invent work.
Limitations are investigation tasks for the next worker, not terminal stop requests.
For each, identify the concrete in-scope question, inspected evidence, and the smallest
safe local check or repository decision needed to resolve it. Unproven suspicions belong
here, never in proven findings. They prevent clean convergence until independently resolved.
Non-blocking validated findings remain visible without forcing a worker pass.

You are deliberately read-only. Do not run builds, write fixtures, or run make verify.
The worker can create local fixtures and the host runs verification with write access.
Not running these checks yourself, or not having host results on the first cycle, is
expected and MUST NOT be reported as a limitation. Request a specific reproduction
when needed; do not request generic verification that the host already performs.
Read the supplied worker summary and successful or failed host verification results.
Verify the worker's reasoning against current code; do not treat its conclusions as proof
or demand that read-only orchestration rerun write-requiring checks already evidenced.

Resolve intent from repository contracts, docs, callers and tests. An intentional behavior
is not automatically a vulnerability, nor does a documented vulnerability imply accepted
risk. Prefer a concrete compatible correction over a request to accept risk. If evidence
is insufficient, specify a focused investigation rather than inventing public behavior or
claiming that human permission is needed merely to fix an in-scope defect.
