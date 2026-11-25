# Deprecations

How Krabb retires old behavior without carrying bridge code forever, and what is
pending removal.

## Policy

### Interfaces

The policy covers every interface people, scripts, or plugins depend on:

- CLI commands, flags, and output fields
- configuration keys
- local IPC methods and fields
- plugin protocol messages and manifest keys
- model-facing tool names and arguments

**Pre-releases make no promise.** Anything that shipped only in a pre-release, such as
`0.1.0-rc.1`, can change or disappear in the next release without a bridge.

**After a stable release, a replaced interface keeps working for one minor release.**

1. **Release 0.N** introduces the replacement. The old form keeps working, warns once
   where a person can see it, and names its replacement.
2. **Release 0.N+1** removes the old form.

A security fix may remove an interface at once; its release notes say so.

### Saved Data

Changes to the data in a Krabb home are versioned migrations, never compatibility
code in the read path.

- **Steps:** each change is a `krabb-migrations` step that moves the home schema from
  `n` to `n + 1`. Steps run in order, each from a verified recovery point. One that
  fails or is interrupted is rolled back before anything opens the home. Readers then
  read only the current format.
- **The schema floor:** every release states the oldest home schema it updates from.
  A home below it is refused before anything changes, and updates through an earlier
  release first.
- **Raising the floor** retires the oldest steps. It counts as a deprecation, so the
  release before announces it in the ledger.
- **Pre-releases** never set the floor.

Stored names are part of the data format, not a bridge. For example, the session
catalog keeps saving background tasks under `jobs`. Renaming a stored name is a
migration.

**This applies from the stable 0.1.0 release on.** The data format 0.1.0 ships is the
baseline. The session catalog's own upgrades from earlier pre-release formats are not
steps. They stay only so homes created by 0.1.0 pre-releases keep loading, and they go
when a release raises the floor. Every change to saved data after 0.1.0 is a step.

### Bridges

A bridge is any code that keeps an old interface working.

- **Placement:** each bridge lives in one named place, such as an alias table or a
  migration step, never scattered through the code that serves the current interface.
- **Registration:** each bridge is registered in `krabb-runtime/src/deprecation.rs`
  with the release that removes it, and listed below.
- **Enforcement:** a test fails once Krabb's version reaches that release, or when a
  registered bridge is missing from this ledger. The bridge then goes in that release.

## Ledger

| Bridge | Deprecated in | Removed in |
| --- | --- | --- |

No bridges are pending.

## Home Schema Floor

| Release | Oldest home schema it updates from |
| --- | --- |
| 0.1.0 | 0 |
