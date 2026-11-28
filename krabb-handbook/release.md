# Release

Releases follow the versions in the code. Krabb's version is the workspace
`version` in `Cargo.toml`, and each plugin's version is the `version` in its
`krabb-plugin.toml` and `Cargo.toml`. A version with an `-rc.N` suffix is a
pre-release. Changing a version is the only way to release, and a person always
approves it: run `make bump`, review the plan, confirm it, and merge the pull
request. When CI passes for the head of `main`, the Release workflow publishes
each version it declares that is not published yet and skips the rest with a
notice. Each release covers every earlier commit, so CI that finishes after
`main` has moved on releases nothing; the newer commit's CI releases it. When CI
fails for the head, rerun it, and the release follows once it passes. When the
head has no CI run, such as after a `[skip ci]` commit, run the Main workflow on
`main` to release it.

For the current `0.1.0` release, add changelog entries only under `Added`.
Include only important, shipped, user-facing additions, each in one very short
sentence. Carefully assess the value to users; routine or minor additions need
no entry. Omit internal work, tests, documentation, and planning. Later notes
go under `## [Unreleased]`, which a stable bump turns into the new version's
section. See the [change rules](../CONTRIBUTING.md#change-rules).

## Table Of Contents

- [Version Bumps](#version-bumps)
- [Release Workflow](#release-workflow)
- [Dry Runs](#dry-runs)
- [Krabb Releases](#krabb-releases)
- [Plugin Releases](#plugin-releases)
- [Release Checklist](#release-checklist)

## Version Bumps

```sh
make bump                              # every changed component, next patch
make bump BUMP_ARGS=--unstable         # every changed component, next -rc.N
make bump BUMP_ARGS=krabb,tools,--version,0.2.0
```

`krabb-scripts/bump.py` selects every component whose sources, or whose locked
dependencies, changed since its current version was set, or the components you
name: `krabb` or plugin IDs. It shows the planned versions and asks for
confirmation before writing; outside a terminal it needs `--yes`, which only a
maintainer's approval justifies. A stable bump promotes a pre-release to its
version, or raises the patch version, and `--version X.Y.Z` picks another;
`--unstable` moves to the next `-rc.N` of that version. A bump updates every
file that holds the version and `Cargo.lock`. A stable Krabb bump also moves the
`Unreleased` notes into the new version's `CHANGELOG.md` section, as does a
stable plugin bump for a plugin with its own `CHANGELOG.md`.

## Release Workflow

```text
Plan ─┬─ Build ────┬─ Install ─┬─ Plugins ─ Publish
      └─ Previous ─┴─ Update ──┘
```

- **Plan** confirms that the run covers the head of `main` and that CI passed for it,
  writes the release notes, and decides what to build: Krabb when its version has no
  GitHub Release yet, and every plugin whose manifest version is unpublished. When
  neither is new, the run ends here with a notice. When only plugins are new, Krabb is
  built for the drills but not published.
- **Build** packages Krabb and the plugins to release in one job per target, and writes
  that target's `SHA256SUMS` line. **Previous** packages the same source as
  version `0.0.0-previous` for the Update drill. The six targets are listed once, in
  `krabb-scripts/targets.json`.
- **Install** and **Update** run the drills in `krabb-scripts/drills` on exactly these
  archives. When the run built plugins, the drills install them from a local plugin
  index that `krabb-scripts/plugins.py local` writes from the archives, with published
  versions filling in the other plugins, through `KRABB_PLUGIN_INDEX`. Otherwise they
  use the published index.
- **Plugins** pushes the new plugin versions and rebuilds the index, and
  **Publish** then publishes the Krabb GitHub Release. Both run only when both drills
  passed, so nothing is published from a run whose archives fail on any target. Plan
  also finds an index that misses published versions after a failed run, and Plugins
  then rebuilds it without building anything.

## Dry Runs

Run the Release workflow from the Actions tab with its publish option cleared to test a
branch before merging it. A dry run runs from any branch, plans the same decisions,
builds, and drills, but skips both publish jobs and their permissions, and never joins
the release queue. When every version is already published, name plugins under rebuild
to build and drill them anyway:

```sh
gh workflow run release.yml --ref <branch> -f publish=false -f rebuild="tools memory"
```

## Krabb Releases

When the workspace version has no published GitHub Release, the Release workflow
builds the host archive (`krabb` and `krabb-daemon`) for Linux, macOS, and
Windows on x86_64 and ARM64, emits `SHA256SUMS`, and publishes the release as
`vX.Y.Z`. The release stays a draft until every file is attached. A stable
release takes its notes from the matching `CHANGELOG.md` section under a
`krabb@vX.Y.Z | YYYY-MM-DD` headline, dated by the heading or else the release
day, and uses a short standard description when the section is missing or empty.
A pre-release gets a short standard description and is published as a GitHub
pre-release. Releases contain no plugins. Installers take the newest stable
release unless `KRABB_VERSION` names another, and verify checksums before
activation. Krabb does not publish crates.

## Plugin Releases

Plugins have no tags or GitHub Releases. The Release workflow builds every
plugin whose manifest version has no tag in `ghcr.io/<owner>/<repo>/plugins`
yet, for the six targets, and pushes each version as the tag `<id>-<version>`
with its declarations as annotations. Publication refuses a plugin whose
manifest and `Cargo.toml` versions differ. It then rebuilds the
[plugin index](plugins.md#official-plugin-index) from the registry's version
tags, pushes it as the `index` tag when it changed, and confirms that anonymous
clients can download it. A failed run leaves nothing half recorded: the next run
publishes what is missing and rebuilds the index again. To release only plugins,
run the Release workflow manually from `main` and clear its Krabb option; clear
the plugin option to release only Krabb. A manual run that publishes releases the head
of `main` and refuses to release until CI has passed for it. Release runs take turns,
and a newer run replaces a waiting one; that loses nothing, because each run
releases the whole head of `main`. A manual run with an option cleared can
therefore replace a waiting full release, so run a full release afterwards if
one was waiting.

The first publication creates the `plugins` package as private. Make it public
once in the package settings and run the Release workflow again.

Before releasing, check plugin compatibility as
[DEPRECATIONS.md](../DEPRECATIONS.md#plugin-compatibility) describes. `make bump`
warns when `krabb-core` changed since the last release but the plugin protocol
did not.

Published versions are never changed or removed. To withdraw a broken version,
run the Withdraw workflow with the plugin ID, the version, and a reason. Krabb
then skips the version when choosing one, installs it only when it is named
explicitly, and shows the reason.

## Release Checklist

1. Run `make bump` and review the planned versions and changelog notes.
2. Run `make gate` and confirm Linux coverage remains at or above 80% for
   every package. Review the native core coverage checks on macOS and Windows
   x86_64, which require 60% per package and 40% for the runtime host.
3. Inspect archive contents and checksums with `krabb-scripts/check.sh`.
4. Review installer output on each published target before merging.
5. Release builds the archives, then runs the Install workflow on them before
   publishing: it installs them with `install.sh` or `install.ps1` on every target,
   runs `krabb init`, `krabb doctor`, and `krabb status`, installs every official
   plugin without setting it up, including the plugin versions the run is about to
   publish, and installs and removes the service. If any target fails, nothing is
   published. Run the same workflow from the Actions tab: it builds
   the branch or tag chosen under "Use workflow from", latest `main` by default, and
   also runs every Monday against `main`. To test another commit, push a branch or tag
   at it first.
6. Release also runs the Update workflow on the same archives before publishing. On
   every target it installs the same source built as version `0.0.0-previous`, sets up
   official plugins and the service, and serves releases from local `file://`
   directories. An update whose daemon cannot start must restore the previous binaries,
   state, and service; the real update must keep the service ready and a detached
   plugin detached; and the recovery point's `krabb update --rollback` must restore the
   previous build while the new daemon cannot start. It also runs from the Actions tab
   and every Monday against `main`.
7. Before a stable release, remove every bridge the [ledger](../DEPRECATIONS.md) lists for
   it; the deprecation test fails until they are gone. Add the release's row to the home
   schema floor table, and announce any bridge or floor change planned for the next one.

For a local installer smoke test, set `KRABB_RELEASE_BASE` to a `file://`
directory containing the archives and `SHA256SUMS`, for example
`file:///tmp/krabb-release`. The default remains the versioned HTTPS release
directory, and installers reject other release-base schemes.

The Package workflow checks every archive and installer it builds, and writes the
target's `SHA256SUMS` line beside a core archive. On both Windows targets it also
installs the service in an isolated home, checks its status, and removes it. Run it
from the Actions tab with `core` to check packaging without releasing. Its optional
version input builds Krabb at another workspace version; Release and the Update
workflow use it for the previous build.

Release scripts are intentionally explicit. They do not publish crates,
containers, or services, and they must not receive credentials through command
arguments.

Core archives contain the `krabb` and `krabb-daemon` executables
plus license files, but no plugin binaries. Every plugin version has its own
archive per target with its manifest and executable under `bin/`. Plugins are
registered one at a time in `plugins.lock` under
`KRABB_HOME/plugins/<id>` (or the platform configuration directory). A
running daemon starts a newly installed plugin over authenticated local IPC;
otherwise, it discovers the plugin the next time it starts.
Service activation remains an explicit `krabb service start` action after
installation.
