# Release

The release tag sets the Krabb version, such as `vX.Y.Z`, or `vX.Y.Z-rc.N` for a
pre-release. Keep the workspace manifests and `CITATION.cff` at the next stable
version; every tag's `X.Y.Z` must match it. Use Keep a Changelog headings.

For the current `0.1.0` release, add changelog entries only under `Added`.
Include only important, shipped, user-facing additions, each in one very short
sentence. Carefully assess the value to users; routine or minor additions need
no entry. Omit internal work, tests, documentation, and planning. See the
[change rules](../CONTRIBUTING.md#change-rules).

```sh
krabb-scripts/release.sh vX.Y.Z
```

Pushing a version tag starts release automation. A stable tag needs a matching
changelog section, such as `## [X.Y.Z] - YYYY-MM-DD`; its notes open with a
`krabb@vX.Y.Z | YYYY-MM-DD` headline, dated by the heading or else the release
day. A pre-release gets a short standard description instead and is published
as a GitHub pre-release. It then builds the host archive (`krabb` and
`krabb-daemon`) for Linux, macOS, and Windows on x86_64 and ARM64, emits
`SHA256SUMS`, and publishes the GitHub Release. Releases contain no plugins;
plugins are published separately to the plugin index. The binaries report the
tag's version. Installers take the newest stable release unless `KRABB_VERSION`
names another, and verify checksums before activation. Krabb does not publish crates.

## Table Of Contents

- [Release Checklist](#release-checklist)
- [Plugin Releases](#plugin-releases)

## Release Checklist

1. For a stable release, update the workspace manifests, `CITATION.cff`, and the
   matching `CHANGELOG.md` heading. A pre-release needs no changes.
2. Run `make verify` and confirm Linux coverage remains at or above 80% for
   every package. Review the native core coverage checks on macOS and Windows
   x86_64, which require 60% per package and 40% for the runtime host.
3. Run `krabb-scripts/release.sh vX.Y.Z` to build the release binaries.
4. Inspect archive contents and checksums with `krabb-scripts/check.sh`.
5. Review installer output on each supported platform before publishing.

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

## Plugin Releases

Plugins have no tags or GitHub Releases. To release one, set the same new
`version` in its `krabb-plugin.toml` and its `Cargo.toml`, then merge the change
to `main`; publication refuses a plugin whose two versions differ. A version with
a suffix, such as `0.2.0-rc.1`, is a pre-release that users install with
`krabb plugin install <id> --unstable`. The Plugins workflow then
finds every manifest version missing from the
[plugin index](plugins.md#official-plugin-index), builds those plugins for the
six targets, pushes each version to `ghcr.io/<owner>/<repo>/plugins` as the tag
`<id>-<version>` with the repository token, confirms that anonymous clients can
download every archive, and appends the version to `index.json` on the `plugins`
branch. Unchanged plugins are not rebuilt, and a Krabb release never republishes
plugins. Run the workflow manually to retry a failed publication.

The first publication creates the `plugins` package as private. Make it public
once in the package settings and run the workflow again; publication stops
before recording any version that anonymous clients cannot download.

Published versions are never changed or removed. To withdraw a broken version,
add `"yanked": "<reason>"` to its entry on the `plugins` branch: Krabb then skips
it when choosing a version, and installs it only when it is named explicitly,
with the reason shown.
