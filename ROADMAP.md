# Roadmap

Krabb is an agent operating system for native and external agents. Its default
experience is one Orchestrator that owns outcomes and coordinates work for the
requester. Users can run additional Orchestrators and connect them as peers.
Channels and clients provide access to authorized agents; they do not define an
agent's identity, intelligence provider, or ownership of state.

This is a delivery plan, not a claim that the described features already ship.
Versions express dependency and product priority, not promised dates. Ship useful
increments within each milestone. The first public release, v0.1, must be useful
without a source checkout: install official binaries, configure a single Krabbling,
and interact through a channel or client. Later releases deepen recovery, managed
upgrades, orchestration, media, federation, and client support. v0.15 is the
planned final release series before v1.0; minor versions are not limited to 0.9.

## Table Of Contents

- [Product Contract](#product-contract)
- [Release Policy](#release-policy)
- [Engineering Gates](#engineering-gates)
- [v0.1 | Core](#v01--core)
- [v0.2 | Durability](#v02--durability)
- [v0.3 | Agents](#v03--agents)
- [v0.4 | Distribution](#v04--distribution)
- [v0.5 | Experience](#v05--experience)
- [v0.6 | Orchestration](#v06--orchestration)
- [v0.7 | Media](#v07--media)
- [v0.8 | Federation](#v08--federation)
- [v0.9 | Clients](#v09--clients)
- [v0.10 | Mobile](#v010--mobile)
- [v0.11 | Ecosystem](#v011--ecosystem)
- [v0.12 | Deployment](#v012--deployment)
- [v0.13 | Efficiency](#v013--efficiency)
- [v0.14 | Realtime](#v014--realtime)
- [v0.15 | Stabilization](#v015--stabilization)
- [v1.0 Acceptance](#v10-acceptance)

## Product Contract

- Make simplicity a requirement across every release: use clear language,
  predictable commands, sensible defaults, and the fewest necessary steps.
  Reveal advanced options when needed; do not require users to understand internal
  architecture for everyday tasks. Keep permissions, consequences, and recovery
  visible even when the workflow is automatic.
- Use one agent abstraction with the roles Orchestrator, Executor, and
  Subexecutor. The delegation hierarchy is Orchestrator -> Executor ->
  Subexecutor. Subexecutors return bounded results to their Executor; further
  subdivision returns to that Executor instead of creating another role level.
- An Orchestrator owns objectives, plans, coordination, result verification,
  progress reporting, and escalation. It can work directly without spawning
  agents. Executors own delegated work; Subexecutors handle smaller tasks.
- Orchestrators connect as peers, never as subordinate Orchestrators. Requests
  between peers do not transfer ownership of their teams. Keep delegation
  relationships separate from the graph of connections and visibility grants.
- Give every agent its own stable identity, instructions, provider/model
  selection, context, memory, workspace, configuration, and granted capabilities.
  Support independent native agents and explicitly connected external agents.
- Default to one Orchestrator and lazy creation of optional workers. Do not
  require a team, container engine, cloud provider, or remote control service.
  Keep direct interaction with any authorized agent possible through interfaces.
- Avoid an arbitrary architectural agent-count ceiling. Enforce explicit limits
  on active work, queues, storage, provider usage, and delegated budgets. Idle
  agent definitions must not require resident models or plugin processes.
- Give humans one accountable contact, coherent progress, and consolidated
  results. Escalate decisions that require their authority or missing information;
  do not make humans relay messages or repeatedly reconstruct team context.
- Keep the core provider-neutral and capability-free. Shared contracts belong
  in core, orchestration and durable host state in runtime, and external I/O,
  media engines, intelligence, channels, and UI behavior in their integrations.

## Release Policy

- All v0.x releases are unstable and may change commands, configuration, data
  layout, APIs, and plugin contracts. State breaking changes and migration effects
  clearly; instability is not permission to lose data or silently weaken policy.
- Make every supported upgrade a managed operation: the user executes the upgrade,
  or previously authorized automatic updates do it. Do not require manual file
  moves, configuration rewrites, service reinstallation, or database repair as
  the normal upgrade procedure.
- Version persistent schemas separately from application and protocol versions.
  Maintain ordered migrations from the first supported installation format,
  including skipped releases. Validate the complete migration path before changes;
  identify unsupported paths before touching the installation.
- Ship `krabb-migrations` with v0.1 and invoke it through the installer or updater,
  including upgrades from recognized development homes. Establish recovery
  machinery before adopting the new agent layout.
  Preserve identity, sessions, artifacts, credentials, grants, and plugin state.
- Publish compatible host, plugin, and client version ranges. Negotiate protocol
  support and reject incompatible operations explicitly. A breaking v0.x change
  may require coordinated component updates; never silently speak a different API.
- Retain a validated recovery point until an upgrade is accepted. Distinguish
  rollback during upgrade from downgrading after new user writes: the latter needs
  a tested reverse migration or an explicit restore with its data-loss consequences.
- Define supported upgrade origins, operating-system versions, architectures,
  release channels, and maintenance periods. Keep pre-release channels opt-in.
  Preserve existing grants during migration; new authority needs separate consent.

## Engineering Gates

These requirements apply to every milestone; later hardening releases do not
postpone baseline security, durability, efficiency, or portability.

- Keep small, cohesive modules, typed boundaries and errors, bounded asynchronous
  work, cancellation, and explicit ownership. Record consequential design choices
  and test their contracts. Extract shared behavior without speculative frameworks.
- Choose the simplest design that meets the actual requirements. Reuse established
  contracts and shared behavior; add abstractions, dependencies, configuration, or
  resident processes only when their benefit justifies their complexity and cost.
- Run formatting, spacing, Clippy, tests, rustdoc, guidance validation, and security
  checks. Keep the full local coverage gates at or above their configured 80%
  baseline; coverage complements behavioral verification and never replaces it.
- Test protocol framing, malformed plugins, capability negotiation, isolation,
  approvals, concurrency, crashes, retries, updates, and rollback with deterministic
  fixtures. Automated tests must not call paid provider APIs.
- Add fault injection and property-based or fuzz testing at parsers, protocols,
  migrations, and state transitions. Reproduce failures before repairing them;
  do not mask flakes or reduce gates to make a release pass.
- Build and exercise Linux, macOS, and Windows on x86_64 and aarch64. Distinguish
  build support, emulated tests, native tests, and verified functional support.
  Publish feature-specific limitations instead of claiming universal parity.
- Maintain resource baselines from v0.2: idle and peak memory, CPU, startup and
  response overhead, queue latency, storage growth, and recovery time. Separate
  Krabb overhead from local inference and optional media engines. Set regression
  budgets on fixed workloads and hardware before accepting optimizations.
- Keep secrets out of logs, artifacts, and exports by default. Keep shell opt-in,
  tool access confined, and authorization enforced by the host. Process plugins
  are fault boundaries, not a claim of OS sandboxing for untrusted native code.
- Maintain a threat model and local, redacted operational evidence without
  telemetry. Protect actor identity across interface boundaries and treat model,
  document, tool, and remote-agent output as untrusted input, never new authority.
- Gate external changes with operation IDs and durable receipts where possible.
  After uncertain mutation or lost acknowledgement, reconcile or report uncertainty;
  do not replay mutating work or promise universal exactly-once external effects.
- Keep implementation documentation and the release ledger accurate. Record real
  functional successes in the existing compatibility and component checkboxes;
  passing builds or unit tests alone do not qualify.

## v0.1 | Core

Establish a functional single-agent Krabbling with the basic installed-system
flow working on Linux, macOS, and Windows. This baseline precedes the explicit
agent identities, roles, and delegation introduced by later milestones.

- Deliver the CLI, daemon, and official plugins as versioned GitHub Release
  artifacts for supported targets. Validate the existing packaging and checksum
  verification end to end; users must not need Git, Cargo, or a repository clone.
  Provide copyable Unix and PowerShell installation commands and manual downloads.
- Make `krabb plugin install <id>` resolve compatible official release artifacts
  from any working directory. Record the installed version, origin, digest, and
  approved grants; use that origin for updates. Keep source installs and `--link`
  explicit development options rather than prerequisites for ordinary users.
- Resolve default installs against published releases, not an unreleased version
  on the main branch. Support explicit versions, check host/plugin compatibility
  before replacement, and explain unavailable artifacts or unsupported targets
  without silently compiling from source.
- Introduce `krabb-migrations` as the shared, versioned migration machinery shipped
  with the host, not an optional plugin or a separate user installation. Keep
  upgrade orchestration in runtime and component-specific state transformations
  with their owners behind explicit migration contracts.
- Provide `krabb update` as the single entrypoint for updating the CLI, daemon,
  and installed plugins to a compatible release set. Preview versions, migrations,
  permission changes, and expected interruption before approval. Download, verify,
  and stage all required components before disrupting the working installation;
  abort the transaction if any required component cannot be prepared.
- Make `krabb update` handle breaking configuration, home-layout,
  and persisted-state changes automatically. Provide an ordered migration registry,
  schema versions, exclusive upgrade ownership, and a durable journal. Support
  skipped releases through validated paths; reject unknown or newer schemas before
  mutation rather than guessing their format.
- Preflight compatibility, disk space, permissions, and required component
  migrations; safely quiesce writes and create a validated recovery point before
  changes. Use resumable or safely repeatable steps, update affected service
  definitions, and verify startup before committing. Recover compatible code and
  state together after interruption or failure, without replaying external actions.
  Users should not need to edit internal files or reinstall services to upgrade.
- Activate the prepared release set in a controlled sequence, restarting affected
  plugins and the service where required, then check readiness before declaring
  success. Preserve previously stopped services and detached plugins. Handle
  executable replacement on every supported OS, including Windows, and retain
  recovery tooling that works when the new daemon cannot start.
- Automatically restore the last working compatible binaries, configuration, and
  state if activation or health checks fail. Provide `krabb update --rollback` for
  explicit recovery and show the retained recovery point in update status. Prevent
  concurrent upgrades and preserve recovery material until successful validation.
  After new user writes, require a compatible reverse migration or explain and
  confirm any data loss before restoring an older snapshot; never silently discard
  conversations or resurrect removed credentials and grants.
- Provide one complete first-run path in the README: install, initialize, select
  and configure a provider/model, connect a channel or open the TUI, and exchange
  the first message. Explain local versus remote inference, required credentials,
  optional dependencies, and which plugins remain experimental.
- Make routine operation usable from the first release: discoverable help,
  actionable health and configuration errors, visible installation/update progress,
  service start/stop/restart, and documented manual upgrades and uninstallation.
  Preserve configuration and credentials during upgrades. Keep failed replacements
  recoverable. Advanced distribution and scheduled updates follow in v0.4.
- Remove a plugin's saved credentials, tokens, and authorization grants on
  uninstall, including its entries in Krabb-managed secret stores and keyrings.
  Stop its access before cleanup; report incomplete cleanup and make it retryable.
  Preserve unrelated plugins' credentials and retain non-secret user data unless
  deletion is requested. Distinguish uninstall from temporary detach or update.
  Explain that external environment variables, provider-side key revocation, and
  existing backups need separate handling; restoration must not silently reactivate
  removed credentials or grants.
- Make setup resumable and safe to repeat without overwriting a working
  configuration. Validate selections before saving, mask credentials, allow empty
  keys where supported, and provide an explicit connection test. Show the effective
  provider/model and configuration scope; explain when a change takes effect and
  whether a restart is needed.
- Give every setup field a readable label, a short explanation of its purpose,
  required/optional status, and an appropriate default or example. Explain where
  to obtain credentials and distinguish local hosts from cloud endpoints. Present
  environment-variable names as secondary technical details, not the main prompt.
  Declare this metadata with each plugin and render it through the shared setup
  flow so providers, channels, and clients use consistent interaction patterns.
- Show whether a saved value exists without revealing secrets. Offer explicit
  keep/change/clear choices, with Enter preserving existing values during repeat
  setup. Explain blank-value behavior for first-time setup; use choices and typed
  validation where practical, retain valid answers after errors, and summarize
  changes and their activation outcome without exposing credentials.
- Make `status` and `doctor` distinguish installed, configured, running, and ready.
  Report unavailable dependencies and failed activation honestly, with a specific
  next action and redacted diagnostics. Never report full health solely because
  configuration parses or plugin binaries exist.
- Give long operations visible progress, bounded waits, and safe cancellation.
  Preserve user input and conversation history across recoverable failures, show
  queued/running/failed turn state, and emit one actionable failure notification
  instead of repeated apologies or automatic retries of uncertain mutations.
- Preview disruptive changes and their recovery path once, then carry out the
  approved operation without repetitive prompts. Provide predictable exit codes,
  noninteractive behavior, and accessible plain-text output. Expose data locations,
  backup/restore instructions, and privacy-safe troubleshooting from the first-run
  guide so recovery does not require reading source code.
- Audit and align all user-facing CLI and TUI output, using the existing plugin
  output as the reference for spacing, indentation, hierarchy, and concise results.
  Apply consistent terminology, labels, ordering, pagination, prompts, empty states,
  warnings, errors, and next actions across setup, configuration, sessions, services,
  updates, diagnostics, and plugin operations.
- Share presentation conventions and reusable formatting where appropriate while
  respecting CLI output and interactive TUI layouts. Keep progress from colliding
  with prompts or final results, handle narrow terminals and long values, preserve
  copyable commands and paths, and support readable output without color. Keep
  redirected output free of terminal controls and machine-readable output free of
  decorative text; keep internal diagnostics out of normal user-facing results.
- Share slash-command definitions, dispatch, authorization, help, and results
  across supported channels and clients through adapters. Include `/help`,
  `/status`, `/model`, and `/session` for inspection, selection, new conversations,
  and cancellation. Expose `/workspace`, `/approvals`, `/schedule`, `/memory`,
  `/data`, and `/plugin` where the installed capabilities and caller grants allow.
- Handle slash commands deterministically without an intelligence turn. Bind
  operations to the authenticated sender and intended conversation; channel users
  must not inherit local CLI/TUI operator authority. Require confirmation for
  destructive actions and reject unauthorized requests without changing state.
- Register native command menus where supported and provide text-command handling
  otherwise. Keep command names and semantics consistent, support platform-specific
  addressing and argument formats, and show actionable usage for invalid commands.
  Verify command routing, cancellation during a turn, permission boundaries, and
  help visibility across channel/client adapters.
- Complete the [compatibility checklist](COMPATIBILITY.md): initialization and
  health checks, daemon start/stop/restart with persistence, a real intelligence
  turn, a channel round trip, a client interaction, and plugin installation.
- Keep the useful minimum to the daemon, one configured intelligence provider,
  and one channel or client. Verify both remote and local inference paths without
  requiring every provider or optional component in a basic installation.
- Scope component acceptance to the [v0.1 functional matrix](FUNCTIONAL.md).
  Exclude draft A2A, ACP, AG-UI, Browser, Computer, MCP, and Whisper integrations
  from this release's completion criteria. Existing draft code does not establish
  released support; graduate automation, interoperability, and speech through
  their later milestones.
- Keep the six OS/architecture combinations in the basic compatibility matrix:
  x86_64 and aarch64 on each of Linux, macOS, and Windows. They are six targets,
  not six architectures. Require the basic functional flow on each target claimed
  as supported; a successful build alone is insufficient.
- Run detailed component functionality once per component, independently of the
  platform matrix. Add focused target-specific checks for native dependencies,
  service management, process containment, paths, permissions, and packaging rather
  than repeating every provider/plugin scenario on all six targets.
- Keep unsupported or unverified targets explicitly experimental until their basic
  checks pass. Never infer an ARM64 result from x86_64, or mark unchecked results
  complete to meet a release date. Preserve the recorded successful checks.

Acceptance: on a clean machine without Git or Rust, a new user can install the
published host and selected plugins, configure a Krabbling, exchange a message,
use slash commands, operate the service, and follow the documented upgrade path.
The single-agent flow works across all three operating systems and every
architecture advertised as supported, with the basic and component matrices
reflecting actual functional results. Test interrupted downloads, invalid checksums,
incompatible artifacts, and failed activation without losing the working setup.
Exercise migrations from every supported origin, including skipped releases and
interruption at each phase. Verify `krabb update` handles the host, installed plugins,
and service together; failed readiness restores a usable installation, and explicit
rollback works without the new daemon. Test setup with a user unfamiliar with
environment-variable names, including repeat setup and credential clearing.
Verify plugin uninstall removes managed credentials
and grants, preserves unrelated state, and reports cleanup failures accurately.
Review representative CLI/TUI success, empty, partial-failure, and error screens
for consistent presentation, including narrow terminals, no-color mode, redirected
output, and JSON output where supported.
Delegation and the draft integrations listed above are outside the v0.1 boundary.

## v0.2 | Durability

Deepen single-agent recovery and the v0.1 migration machinery, and establish
operational and resource baselines before structural changes.

- Resolve known high and medium reliability findings; strengthen queue ownership,
  cancellation, persistence, plugin recovery, and delivery reconciliation. Keep
  repeated failures bounded and avoid duplicate failure replies or restart loops.
- Extend `krabb-migrations` fault coverage and recovery diagnostics for increasingly
  complex state changes. Exercise disk exhaustion, partial writes, process
  termination, and recovery failures across host and plugin migration boundaries.
- Add consistent backup, verification, restore, and retention workflows for host
  and plugin state. Separate configuration export from a complete backup; define
  quiescing or snapshot contracts and explicit handling of encrypted credentials
  and keyring material. Prove restoration on a fresh installation.
- Extend baseline readiness and truthful loaded-plugin status with bounded
  diagnostic bundles, task/delivery correlation IDs, and actionable errors. Report
  failed activation and unavailable providers without claiming full health.
- Reconcile TUI offline session changes on daemon reconnect without overwriting
  concurrent history. Stabilize scheduler restart recovery with deterministic
  readiness synchronization and suitable instrumented-runner deadlines.
- Establish reproducible resource measurements and baseline interruption/soak
  workloads. Keep native provider search capability-aware and permissioned;
  unsupported search must not trigger an implicit substitute service.

Acceptance: recover an interrupted migration and restore a complete installation;
crash/retry tests show no silent state loss or automatic replay of uncertain actions.

## v0.3 | Agents

Make one Orchestrator the simplest instance of the same architecture that will
support many agents. Establish identity and authority before adding team automation.

- Adopt one versioned home layout containing host infrastructure and
  `agents/<id>/` for every local agent, including the default Orchestrator. Separate
  host configuration, connections, caches, and immutable packages from agent-owned
  state. Replace implicit global-home access with explicit agent runtime contexts.
- Migrate existing homes through `krabb-migrations`, preserving all state and source
  paths needed by plugins. Avoid permanent single-agent versus multi-agent layouts
  or a special root agent. Keep one supervised daemon as the straightforward default.
- Define stable agent IDs, roles, lifecycle, ownership, and native versus external
  registrations. Assign agent ownership to sessions, tasks, memory, schedules,
  artifacts, credentials, plugin instances, and audit events.
- Provide an optional Skills plugin for discovering, inspecting, and loading
  reusable skill packages. Each package contains versioned instructions with
  optional reference files and scripts; a package does not require its own plugin
  process. Support a documented `SKILL.md` format, explicit per-agent assignment,
  local discovery, enabling, and removal.
- Keep skill discovery and content access in the plugin, normalized contracts in
  core, and authorization, instruction precedence, and context budgets in runtime.
  Load detailed content only when needed; agents without the Skills plugin remain
  usable and incur no skill indexing or background processing costs.
- Treat skill installation and execution separately: reading instructions must
  not execute bundled scripts or grant authority. Run any requested script through
  existing tool policy and approvals. Confine referenced paths, record provenance,
  and preserve the selected skill version for active tasks across updates.
- Separate authenticated actors from channels and clients. Define directional
  grants for discovery, status, task submission, messaging, artifact access,
  configuration, lifecycle, and delegation. Visibility never grants control or
  implicit access to conversations, memory, instructions, files, or secrets.
- Scope grants to selected agents, teams, tasks, or role levels, including explicit
  rules for future descendants. Default to no transitive access through connections.
  Define revocation effects on pending and running operations; revocation cannot
  retract information already delivered to another actor.
- Introduce a versioned host API for agents, conversations, tasks, artifacts,
  approvals, configuration, and events. Keep protocol transports in host adapters
  or plugins. Give authenticated requests a target agent and traceable origin.
- Preserve convenient CLI defaults for one Orchestrator; add explicit agent
  selection and management. Define configuration precedence across host, agent,
  interface, session, and request. A route selects an agent; a model switch does
  not create a different agent or silently reset its ownership and history.
- Separate channel connection instances from destinations. Route one bot/account
  to authorized agents without duplicate polling. Reuse immutable plugin binaries
  while isolating configuration, secrets, mutable state, and lifecycle per instance;
  share processes only with an explicit and tested isolation contract.

Acceptance: two agents can use different providers and isolated state through one
interface; cross-agent access fails without grants, and migrated single-agent use
remains simple with no mandatory extra resident workers.

## v0.4 | Distribution

Extend the v0.1 coordinated update and rollback workflow with stronger release
provenance, distribution options, and opt-in scheduled upgrades.

- Establish signed release metadata, immutable versions, verified digests, trusted
  signing identities, key rotation, and component compatibility metadata. Include
  dependency inventories and build provenance; plan reproducible build verification.
- Extend official artifact resolution and lockfiles to coordinated component
  upgrades. Preserve explicit source/link development and private-install workflows.
  Do not overwrite managed development installations.
- Extend the v0.1 host-owned upgrade transaction: resolve compatible components,
  download and verify, preflight, drain or checkpoint work safely, snapshot,
  migrate, promote, restart only where needed, health-check, and commit or recover.
- Extend the existing platform service and recovery integration to additional
  distribution mechanisms. Restore compatible code and data together; do not
  advertise zero interruption where a host restart is required.
- Show update availability, scope, progress, restart needs, and failures in CLI,
  status, and the host API. Preserve configuration, credentials, and durable work.
  Treat expanded permissions or unavailable migration paths as explicit blockers.

### Scheduled Updates

- Keep automatic updates disabled until the operator opts in. Configure release
  channel, version pinning, included components, local time, IANA timezone,
  maintenance window, and a maximum wait for active work.
- Check trusted metadata periodically with backoff and jitter. Cache checks and
  expose availability without requiring a daemon restart or busy polling.
- Specify daylight-saving, asleep/offline, and missed-window behavior; default to
  the next permitted window. Allow pause, snooze, update-now, and disabling the
  schedule. Avoid replaying multiple missed upgrade attempts after reconnect.
- Execute the same verified migration transaction as manual upgrades. Coordinate
  host and plugin updates under one lock; keep in-flight external mutations safe,
  defer when draining is unsafe, and stop retry loops after failed activation.
- Require separate approval for newly requested authority; update opt-in does not
  authorize broader filesystem, credential, or network access. Report deferred
  updates and recovery outcomes locally and through configured interfaces.
- Define ownership when a package manager, container deployment, or app store
  controls installation. Use that supported update mechanism rather than rewriting
  externally managed files; clients and the daemon may have different schedules.

Acceptance: clean-host install and skipped-version upgrade work on supported
platforms; scheduled updates respect their window, preserve work, and recover from
bad downloads, incompatible plugins, migration interruption, and failed startup.

## v0.5 | Experience

Refine the first-release experience for a broader audience, with a useful default
Orchestrator and consistent everyday operation and recovery guidance.

- Refine onboarding and recovery using feedback from v0.1 installations. Improve
  guidance for provider/model selection, channel linking, and TUI use; distinguish
  missing configuration, unsupported capabilities, quota errors, and unavailable
  services across the expanded feature set.
- Improve `doctor`, `status`, help, shell completion, progress, and noninteractive
  JSON output. Extend the shared v0.1 command registry and channel/client adapters
  to new features without separate authorization or slash-command implementations.
- Present objectives, conversations, pending decisions, tasks, and artifacts
  coherently. Keep direct agent access available without making the user coordinate
  specialists. Provide inspectable activity without exposing private reasoning.
- Verify Unicode messages and filenames, multilingual input, IANA timezones,
  keyboard navigation, screen-reader output, and locale-independent stored values.
  Prepare localization and right-to-left layouts; document actual language coverage.
- Document architecture, configuration precedence, provider limits, data ownership,
  backup/restore, upgrade policy, permissions, and troubleshooting. Add deterministic
  onboarding and command-output tests and clean-machine acceptance walkthroughs.

Acceptance: a new user can install, configure, converse, inspect health, back up,
and upgrade a Krabbling without reading source code or editing internal state.

## v0.6 | Orchestration

Let one Orchestrator coordinate Executors and Subexecutors while retaining
responsibility for the outcome and minimizing human coordination effort.

- Graduate the draft Browser, Computer, and MCP integrations with explicit
  permissions, bounded execution, cancellation, recovery, and functional checks.
  Document platform dependencies and actual isolation boundaries before including
  them in the release's supported component matrix.
- Develop both MCP roles in the existing plugin: a client for consuming external
  servers' tools, resources, and prompts, and a server for exposing explicitly
  authorized Krabb operations to external clients. Normalize both directions at
  the plugin boundary and enforce identity, agent scope, and grants in the host.
  Neither direction grants implicit access to the other or unrestricted proxying.
- Prefer MCP client connections for application integrations when a compatible
  server exposes the required operations. Provide guided connection setup,
  authentication and credential renewal, capability discovery, health reporting,
  and revocation. A connection exposes only what its server implements and the
  account permits; add a dedicated app plugin only for a concrete unmet need.
  Integrate supported events with the shared task system rather than assuming
  every MCP server supplies notifications or durable background jobs.
- Provide sandboxed script and calculation execution through the Tools plugin
  and optional execution backends. Keep sandbox lifecycle and resource enforcement
  host-owned, with explicit filesystem, network, credential, CPU, memory, output,
  and time limits. Support cancellation and descendant cleanup, return artifacts
  through shared contracts, and report actual platform isolation guarantees.
  Reject unavailable required isolation instead of silently running unrestricted.
- Add durable objectives and tasks distinct from sessions, with acceptance criteria,
  owners, dependencies, deadlines, progress, artifacts, and completion evidence.
  Define queued, running, waiting, completed, failed, cancelled, and uncertain states.
- Persist pause, clarification, approval, and resume state without holding an
  inference call open. Let authorized users steer running work at safe boundaries,
  with explicit handling of queued input, changed objectives, and completed effects.
  Revalidate grants and stale approvals on resume; survive client or daemon restarts
  without repeating completed mutations. Expose the same controls across interfaces.
- Define structured task-result contracts with schemas, status, evidence, artifact
  references, and explicit partial or failed outcomes. Validate results before a
  dependent task consumes them; schema validity does not prove factual correctness.
  Use provider-native structured output where available and bounded validation or
  repair otherwise, without replaying side effects or silently accepting bad data.
- Support Orchestrator -> Executor -> Subexecutor delegation with independent
  provider/model and capability choices. Enforce creation, delegation, and lifecycle
  permissions in the host; a model's request cannot grant itself authority.
- Make temporary versus persistent agent lifetime explicit. Bound worker creation,
  wake/suspend behavior, retention, and cleanup. Keep immutable artifacts and task
  receipts available according to policy after a temporary worker is retired.
- Allocate bounded task budgets for time, steps, tokens, estimated cost, concurrency,
  storage, and fan-out. Charge descendants against shared budgets so subdivision
  cannot multiply authority or resources. Reconcile actual provider usage where
  available; clearly mark estimates and prevent work beyond reserved limits.
- Implement durable mailboxes, task correlation, deduplication, bounded queues,
  fair scheduling, backpressure, worker leases, and stale-worker fencing. Define
  retries, cancellation, dependency failure, and recovery without replaying mutations.
- Share only task-relevant context and granted artifacts. Track memory and summary
  provenance, allow correction and deletion, preserve decision history, and test
  recall after compaction and provider/model changes. Do not share memory implicitly.
- Discover tools through one permission-filtered capability registry for native
  plugins and MCP connections. Expose compact descriptions first and load relevant
  schemas on demand within the model's context budget. Invalidate stale metadata,
  recheck grants at execution, preserve stable tool identities, and allow explicit
  selection so search does not make an authorized tool permanently undiscoverable.
- Support opt-in model/provider failover using explicitly configured alternatives.
  Match required capabilities, context limits, data-sharing permissions, and cost
  budgets before switching; report the effective provider and account for every
  attempt. Bound retries, preserve task state, and handle partial output explicitly.
  Never fail over to bypass a policy refusal, replay uncertain tools, or transfer
  provider-specific continuation state to an incompatible provider.
- Allow explicit skill invocation and bounded task-based selection from an agent's
  enabled skills. Explain which skills were selected, check required capabilities,
  and report unmet prerequisites without silently substituting tools or services.
  Delegation carries only authorized skill references and task-relevant context.
- Add permissioned knowledge sources with scoped retrieval, source citations,
  freshness and deletion handling, and bounded indexing. Separate retrieved
  evidence from instructions; let users inspect and correct persistent memories.
  Build on the existing memory and storage contracts rather than a parallel store.
- Route schedules and authenticated external events into the same durable task
  system. Reuse deduplication, admission limits, cancellation, and audit records;
  provide pause controls and prevent events or agent replies from creating loops.
- Evaluate skills and delegated task outcomes with repeatable fixtures and explicit
  success criteria before broadening autonomy. Support proposing reusable skills
  from successful work, but require review before enabling persistent changes to
  instructions, skills, or permissions. Keep runtime correctness checks independent
  of model quality and paid live-provider evaluations optional.
- Have Orchestrators consolidate progress, verify results against acceptance
  criteria, resolve conflicts, and escalate only required decisions. Support
  explicit delegation to an authorized decision-maker instead of requiring a human
  for every approval; never auto-approve by interpreting a model's agreement.
- Expose task graphs, agent health, resource consumption, and intervention controls
  through the shared API. Cancelling delegated work must not stop an independent
  agent's unrelated tasks. Test overlapping objectives and shared-agent workloads.

Acceptance: an Orchestrator completes a multi-step objective with Executors and
Subexecutors using different providers, survives a worker/daemon restart, respects
budgets and permissions, and returns one verifiable result with bounded escalation.
Demonstrate durable pause/steer/resume, validated task results, selective tool loading
with a small context window, and configured provider failover without duplicate
effects. Verify sandbox limits and both MCP directions with scoped identities;
an app connection must not expose unrelated Krabb state or authority.

## v0.7 | Media

Exchange text, files, images, audio, and recorded video across supported interfaces.
Ship file exchange and images first, then voice and video as independently usable
increments. Keep every processing engine optional and suitable for local or remote use.

- Declare input and output support separately for each selected model and endpoint,
  including formats and limits. Re-evaluate support when model selection changes;
  understanding an image or audio clip does not imply generating that medium.
- Reuse normalized media/artifact contracts, authenticated transfer, bounded
  processing, cancellation, retention, cleanup, and delivery receipts. Preserve
  source attachments and identify derived content, subject to retention policy.
- Prefer native inputs when supported. Reuse platform transcripts where available
  and appropriate; use alternative transcription, parsing, or conversion only when
  configured. Report unsupported media without silent provider switching or uploads.
- Extend channel/client adapters for uploads, downloads, captions, previews or
  playback, and platform limits. Distinguish native media delivery from a generic
  file attachment, with a clear alternative when the interface cannot render it.

### Files And Images

- Read native document inputs or use explicit parsers for PDF, office documents,
  spreadsheets, and text. Bound decompression and parsing; report encrypted,
  malformed, unsupported, or partly interpreted documents with their limitations.
- Generate and edit downloadable shell scripts, source files, JSON, CSV, Markdown,
  and other text artifacts. Use tools/libraries for PDF, spreadsheets, and binary
  outputs. Creating or sending executable content must not execute it.
- Support native vision and optional image generation/editing. Keep file names,
  MIME types, versions, ownership, and provenance; deliver actual artifacts rather
  than only code blocks or inaccessible host paths.

### Voice And Video

- Accept voice notes and audio through native audio models or optional local/remote
  STT. Retain Whisper as an STT implementation and provide guided setup without
  requiring hand-written wrappers for supported engines.
- Add local/remote TTS and native audio output. Offer text, voice, both, or
  follow-input replies, with voice and language selection independent of the
  conversational model. Deliver voice notes or audio files as supported.
- Support native video understanding and explicit alternatives using frames,
  timestamps, and transcripts. Explain that extracted material loses information
  and must not be presented as full native video comprehension.
- Add optional image/video generation and editing backends with durable job state,
  progress, cancellation, result retrieval, and expiry. Handle uncertain remote
  completion without automatically repeating paid generation.

Acceptance: receive, interpret, create, and deliver representative files and media
on supported routes; test mixed turns, model switches, limits, interrupted jobs,
and missing capabilities with deterministic fixtures and separate functional checks.

## v0.8 | Federation

Connect Orchestrators as peers and let native or external agents request work
through explicit authority, without assuming control of remote runtimes.

- Provide authenticated inbound and outbound agent interfaces, including appropriate
  A2A/ACP adapters. Publish capabilities and supported operations; map external task,
  session, artifact, and error semantics without claiming unsupported guarantees.
- Register remote identities and endpoints with explicit trust, credential scopes,
  pairing, rotation, revocation, and connection health. Separate discovery from
  authentication and authorization; advertised capabilities are not permissions.
- Grant visibility of an Orchestrator, selected Executors, and Subexecutors by role,
  team, task, or explicit selection. Grant direct messaging, task submission,
  inspection, artifact access, and administration separately. Apply inherited or
  future-descendant rules only when explicitly configured; connections are directional.
- Preserve peer independence: A connecting to B does not gain B's access to C or
  ownership of B's team. Allow B to accept an outcome and coordinate its own work,
  returning only authorized progress and results.
- Carry authenticated origin, task IDs, trace/correlation IDs, deadlines, and scoped
  grants across hops. Prevent delegation loops and permission amplification with
  explicit hop/work budgets, deduplication, and revocation checks.
- Support disconnect/reconnect, durable status queries, artifact transfer, bounded
  retries, and partial failure. Cancellation of remote work is a request until
  acknowledged; distinguish unavailable, rejected, expired, and uncertain outcomes.
- Keep remote execution and charging under the remote authority. Apply local
  admission and spending limits where enforceable and expose limits of external
  accounting. Do not copy remote secrets or assume remote agents share local storage.

Acceptance: two independent Orchestrators and an external agent exchange durable
work, expose only granted descendants/data, and recover from disconnection without
leaking authority, duplicating effects, or losing task identity.

## v0.9 | Clients

Deliver first-party web and desktop applications alongside the TUI, all using
the same daemon-owned agent, task, approval, and artifact state.

- Harden the v0.3 client API for authenticated remote use: encrypted transport,
  device/session credentials, pairing, revocation, bounded subscriptions, resumable
  event cursors, and explicit protocol compatibility. Avoid public unauthenticated
  listeners; distinguish remote access from local IPC and bind privileges narrowly.
- Provide a browser client for remote hosts and desktop applications for supported
  Windows, macOS, and Linux targets. Support connecting to one or multiple homes;
  make daemon provisioning optional and explicit rather than hidden in each UI.
- Share product semantics for conversations, agent selection, objectives, team
  visibility, task progress, approvals, media, files, budgets, health, and settings.
  Prefer reusable contracts and UI components where useful without forcing a
  framework that compromises platform accessibility or resource budgets.
- Keep execution and canonical state in the daemon. Reconcile reconnects and
  multi-device edits with version checks; preserve drafts and avoid duplicate
  submissions. Clearly distinguish offline drafts from accepted tasks.
- Support keyboard and assistive technology, responsive layouts, localization,
  notifications, safe attachment rendering, and approved file transfer. Protect web
  origins, browser sessions, and desktop bridges; never expose host credentials to UI.
- Add category-specific `krabb clients`, `krabb intelligence`, and `krabb channels`
  workflows on shared lifecycle infrastructure, retaining existing command aliases
  through migration. Separate client app installation, server integration, and pairing.

Acceptance: the same authorized work can be started in TUI and observed or resumed
in web/desktop clients without a second runtime, duplicate actions, or state divergence.

## v0.10 | Mobile

Provide Android and iOS clients that make the Orchestrator accessible away from
the workstation without requiring the phone to host the AOS daemon.

- Reuse the client API and device grants for secure pairing, revocation, multiple
  installations, conversations, task oversight, approvals, and scoped administration.
- Support camera/microphone capture, media playback, attachments, share sheets,
  file pickers, deep links, accessibility, localization, and intermittent networks.
  Show exactly which installation and agent will receive a shared item.
- Handle background suspension, battery constraints, draft recovery, resumable
  transfer, and duplicate-safe submission. Do not rely on an always-running phone
  process or promise offline model execution without a separately enabled engine.
- Make push notification infrastructure optional and explicit. Use minimal payloads
  without task contents or secrets; retrieve protected details after authentication.
  Ensure reconnect can recover state even without push delivery.
- Respect OS permission, signing, distribution, and app-update mechanisms. Keep
  mobile release cadence compatible with daemon upgrades and supported older clients.

Acceptance: a phone can submit an objective, disconnect, receive or recover progress,
review a decision, and retrieve an artifact without losing or duplicating work.

## v0.11 | Ecosystem

Make clients, intelligence providers, channels, and optional capabilities easy
to extend and release independently using established contracts and packaging.

- Publish shared manifests, SDK/contracts, conformance fixtures, capability schemas,
  and a minimal Hello Krabb starter. Support non-Rust implementations over the
  protocol and test malformed or incompatible implementations at the host boundary.
- Extend v0.4 artifact metadata into an official/community catalog with pinned
  sources, immutable versions, digests, compatibility ranges, maintainers, permission
  declarations, provenance, and clear trust/review status. Support private catalogs
  and direct installs without requiring inclusion in a public registry.
- Give components independent versions and release triggers. A plugin release must
  not publish a kernel release. Coordinate upgrades only where compatibility or
  migration dependencies require it; preserve agent-scoped state and grants.
- Generate catalog views and copyable installation commands from the same metadata.
  Document authoring, testing, publication, review, removal, and compromised-package
  response. Signature validity does not itself establish code trust.
- Extend catalog and provenance support to skill packages, with version pinning,
  dependency checks, reviewable updates, and removal. Reuse artifact verification
  and grant handling; installing a skill must not silently install or authorize
  executable dependencies.
- Complete composable repository organization: `krabb-clients/`,
  `krabb-intelligence/`, and `krabb-channels/`, leaving optional capabilities in
  `krabb-plugins/`. Add concrete apps as implemented rather than empty scaffolding;
  migrate build, discovery, documentation, and release paths together.
- Reuse installation, supervision, integrity, rollback, and permission machinery
  across categories. Support multi-capability components; categories must not be
  inferred solely from directory names or cause duplicate runtime implementations.

Acceptance: an independently released third-party component passes conformance,
installs from a verified artifact, declares its authority, and updates or rolls back
without breaking unrelated agents or requiring a kernel release.

## v0.12 | Deployment

Scale deployment choices from a small personal host to isolated agent instances
and multiple machines, without making distributed infrastructure mandatory.

- Publish reproducible OCI images and documented Docker/Podman operation, mounted
  secrets, persistent volumes, health checks, graceful shutdown, and rootless
  defaults. Keep optional plugins out of the base image unless explicitly selected.
- Offer shared-daemon, dedicated-daemon, and container-per-agent execution. Define
  process, filesystem, network, credential, CPU, memory, and storage boundaries;
  distinguish workspace isolation from a sandbox for hostile code.
- Add explicit placement, admission, worker registration, draining, and recovery
  across hosts. Bound distributed queues; use leases/fencing and durable ownership
  to prevent split-brain task execution. Define behavior during partitions rather
  than assuming every host or provider is always reachable.
- Support consistent backup/export and verified restore or relocation of an agent,
  including identity, configuration, grants, history, and artifacts. Rebind platform
  credentials explicitly where portability is unavailable; prevent two active owners.
- Document disaster recovery, rolling compatibility windows, maintenance, capacity,
  network topology, and external storage choices. Keep cluster services optional and
  avoid claiming high availability until failover and consistency are verified.

Acceptance: a declared deployment topology survives node loss and maintenance
within its documented guarantees; restoring an agent preserves identity and state
without enabling simultaneous conflicting owners or replaying uncertain actions.

## v0.13 | Efficiency

Use accumulated measurements to establish supported resource envelopes and
remove confirmed bottlenecks across the complete AOS.

- Publish reproducible idle, active, burst, long-history, media, team, and recovery
  workloads. Measure RSS/PSS where available, peak use, CPU/wakeups, latency,
  throughput, I/O, storage growth, network use, and mobile battery effects.
- Separate the minimal remote-provider Orchestrator from local inference, optional
  plugins, media workers, and clients. Establish minimum supported configurations
  from evidence instead of promising operation on arbitrary hardware.
- Profile thread/runtime sizing, allocations, buffering, serialization, polling,
  duplicate process overhead, caching, and model residency. Retain optimizations
  only when measurements improve without weakening isolation or correctness.
- Add lazy activation, idle suspension, bounded caches, retention, and safe garbage
  collection. Reclaim only unreferenced artifacts/state; preserve task recovery and
  shared-package ownership across agent creation, deletion, and upgrade.
- Validate fair admission, per-agent budgets, pressure handling, and bounded resource
  use under overload. Report queuing and rejection clearly; do not silently drop work
  or allow one agent to exhaust the host. Extend architecture support only with
  reproducible builds and functional evidence, including actual native hardware.

Acceptance: published resource and latency budgets hold across representative
platforms and sustained workloads, with explicit limits and regression gates.

## v0.14 | Realtime

Extend recorded media to live voice and optional live video where both the
provider and interface support it, while preserving the lightweight base setup.

- Add optional streaming audio input/output, turn detection, playback, interruption,
  and echo handling. Preserve transcripts and task continuity according to policy;
  stop speech independently from cancelling unrelated agent work.
- Add explicitly enabled camera/screen streams and supported live video inputs.
  Negotiate codecs, sample rates, frame rates, bandwidth, and provider capabilities;
  unsupported combinations fail clearly rather than silently recording elsewhere.
- Bound session duration, concurrency, buffering, latency, and cost. Handle network
  changes, reconnects, provider failure, device loss, and client suspension without
  duplicated speech, orphan sessions, or accidentally continuing capture.
- Use visible capture indicators and explicit microphone/camera permissions.
  Define recording retention and deletion; default to retaining no raw live recording
  beyond the processing needs unless the operator enables recording.

Acceptance: a supported live session can be interrupted, disconnected, resumed,
and stopped with bounded resources, clear capture state, and no unintended actions.

## v0.15 | Stabilization

Freeze the proposed v1.0 contracts and validate the supported AOS as a complete
product. This is the final planned v0.x series, including release candidates.

- Publish the v1.0 scope, supported platform/architecture matrix, resource envelopes,
  component compatibility ranges, upgrade origins, and maintenance policy. Separate
  stable capabilities from explicitly experimental extensions.
- Freeze agent/task/artifact semantics, authorization rules, client/plugin APIs,
  configuration, persisted schemas, CLI/JSON behavior, and documented migration
  guarantees. Remove or migrate obsolete interfaces before the stability promise.
- Exercise every supported historical upgrade path, including skipped versions and
  scheduled updates. Run restore drills, interrupted migrations, failed promotion,
  key rotation, mixed compatible clients/plugins, and recovery on clean hosts.
- Run sustained and adversarial workloads across routing, teams, federation, media,
  UI reconnects, provider outages, disk pressure, revoked access, and process/node
  loss. Require agreed duration and resource/latency bounds with no silent data loss.
- Complete security and architecture reviews, dependency/license audits, provenance
  checks, threat-model coverage, and remediation of blocking findings. Verify the
  advertised isolation boundaries instead of inferring safety from process separation.
- Evaluate task completion, result verification, context retention, escalation
  frequency, duplicated work, and recovery using deterministic fixtures and opt-in
  live evaluations. Track model-dependent quality separately from runtime correctness.
- Validate onboarding, accessibility, localization, documentation, support procedures,
  and clean install/uninstall with representative users. Keep release-candidate
  changes focused on defects; reopen affected gates after any contract change.

Acceptance: release candidates satisfy the v1.0 criteria with reproducible evidence;
unmet gates delay v1.0 rather than being hidden by a version bump.

## v1.0 Acceptance

v1.0 is a maintained stability contract, not a claim of perfection or support for
every model, device, or external service. Declare the supported boundary and make
all behavior within it predictable, inspectable, and recoverable.

- One default Orchestrator remains useful and lightweight without optional workers.
  Multiple Orchestrators, Executors, and Subexecutors retain isolated state,
  independent intelligence choices, bounded work, and explicit permissions.
- Connected native and external agents exchange durable tasks and authorized
  artifacts; visibility, control, delegation, and ownership remain distinct.
- Supported clients and channels share canonical daemon state and provide a
  coherent text, file, and media experience, with accurate capability limitations.
- Manual and opted-in scheduled upgrades perform supported migrations without
  routine user repair. Tested recovery preserves compatible code and data; backups
  can restore service independently of a working upgraded daemon.
- Interruptions do not silently lose accepted work, amplify permissions, or replay
  uncertain mutations. Irrecoverable or ambiguous external outcomes are visible.
- Resource, latency, availability, portability, and security claims are backed by
  published tests and explicit operating limits. Optional engines remain optional.
- Public APIs, protocols, configuration, and commands have documented compatibility
  and deprecation policies; data migration and recovery promises remain enforceable.
