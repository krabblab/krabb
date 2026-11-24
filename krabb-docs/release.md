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
the plugin option to release only Krabb. A manual run releases the head of
`main` and refuses to release until CI has passed for it. Release runs take turns,
and a newer run replaces a waiting one; that loses nothing, because each run
releases the whole head of `main`. A manual run with an option cleared can
therefore replace a waiting full release, so run a full release afterwards if
one was waiting.

The first publication creates the `plugins` package as private. Make it public
once in the package settings and run the Release workflow again.

Published versions are never changed or removed. To withdraw a broken version,
run the Withdraw workflow with the plugin ID, the version, and a reason. Krabb
then skips the version when choosing one, installs it only when it is named
explicitly, and shows the reason.

## Release Checklist

1. Run `make bump` and review the planned versions and changelog notes.
2. Run `make verify` and confirm Linux coverage remains at or above 80% for
   every package. Review the native core coverage checks on macOS and Windows
   x86_64, which require 60% per package and 40% for the runtime host.
3. Inspect archive contents and checksums with `krabb-scripts/check.sh`.
4. Review installer output on each published target before merging.
5. After publishing, run the Install workflow. It installs the release with the
   README commands on every target, runs `krabb init`, `krabb doctor`, and
   `krabb status`, installs every official plugin without setting it up, and
   installs and removes the service. It also runs every Monday against the
   newest release.

For a local installer smoke test, set `KRABB_RELEASE_BASE` to a `file://`
directory containing the archives and `SHA256SUMS`, for example
`file:///tmp/krabb-release`. The default remains the versioned HTTPS release
directory, and installers reject other release-base schemes.

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
