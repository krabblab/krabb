//! The official plugin index and verified downloads from its registry.
//!
//! The index lists every published plugin version with its protocol, declarations, and one
//! SHA-256 digest per target. It is the `index` tag of the OCI repository that holds every
//! archive, and CI rebuilds it from that repository's version tags after each publication.
//! Archives are blobs addressed by their digest, so each download is verified by
//! construction and published versions are immutable.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::*;

type Error = Box<dyn std::error::Error + Send + Sync>;

const INDEX: &str = "ghcr.io/krabblab/krabb/plugins:index";
const INDEX_MEDIA: &str = "application/vnd.krabb.plugin-index.v1+json";
const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
const INDEX_LIMIT: u64 = 4 * 1024 * 1024;
const MANIFEST_LIMIT: u64 = 64 * 1024;
const TOKEN_LIMIT: u64 = 64 * 1024;
const SCHEMA: u32 = 1;
const SCHEME: &str = "index:";
const CACHE: &str = "cache/plugin-index";
const FRESH: Duration = Duration::from_secs(60);

/// The last outcome per process, so commands touching many plugins load the index once.
static RECENT: Mutex<Option<Recent>> = Mutex::new(None);

/// When an index location was last loaded, and with what outcome.
type Recent = (Instant, String, Result<Arc<Index>, String>);

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Index {
    pub(crate) schema: u32,
    /// OCI repository holding every plugin archive, such as `ghcr.io/owner/plugins`.
    pub(crate) registry: String,
    pub(crate) plugins: BTreeMap<String, Vec<Release>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Release {
    pub(crate) version: String,
    pub(crate) protocol: Protocol,
    pub(crate) released: String,
    #[serde(default)]
    pub(crate) permissions: Vec<String>,
    #[serde(default)]
    pub(crate) secrets: Vec<String>,
    /// Why the version was withdrawn; withdrawn versions install only when named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) yanked: Option<String>,
    pub(crate) artifacts: BTreeMap<String, Artifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Artifact {
    pub(crate) sha256: String,
    pub(crate) size: u64,
}

/// One release and its archive for a target.
pub(crate) struct Choice<'a> {
    pub(crate) release: &'a Release,
    pub(crate) artifact: &'a Artifact,
    target: &'static str,
    extension: &'static str,
}

/// The lock source of an official plugin; it follows new versions unless one is pinned.
pub(crate) fn source(id: &str, version: Option<&str>) -> String {
    match version {
        Some(version) => format!("{SCHEME}{id}@{version}"),
        None => format!("{SCHEME}{id}"),
    }
}

/// Splits an official source into its plugin ID and pinned version.
pub(crate) fn parse(source: &str) -> Option<(&str, Option<&str>)> {
    let rest = source.strip_prefix(SCHEME)?;

    Some(rest.split_once('@').map_or((rest, None), |(id, version)| (id, Some(version))))
}

/// The plugin ID and pinned version of an official lock entry; linked builds are not.
pub(crate) fn official(entry: &Entry) -> Option<(&str, Option<&str>)> {
    parse(&entry.source).filter(|_| !entry.linked)
}

/// Downloads and unpacks the official plugin version a source selects.
pub(crate) fn resolve(home: &Path, id: &str, version: Option<&str>) -> Result<SourceRoot, Error> {
    unpacked(&*load(home)?, id, version)
}

fn unpacked(index: &Index, id: &str, version: Option<&str>) -> Result<SourceRoot, Error> {
    let choice = select(index, id, version, Protocol::CURRENT)?;
    let download = temp("krabb-download")?;

    let result = pull(index, id, &choice, &download).and_then(|archive| {
        crate::resolve(&format!("{}#sha256={}", archive.display(), choice.artifact.sha256), None)
    });

    let _ = std::fs::remove_dir_all(&download);

    result
}

/// The version an official plugin source selects now; other sources select none.
pub(crate) fn selected(home: &Path, entry: &Entry) -> Result<Option<String>, Error> {
    let Some((id, version)) = official(entry) else {
        return Ok(None);
    };

    let index = load(home)?;

    Ok(Some(select(&index, id, version, Protocol::CURRENT)?.release.version.clone()))
}

/// Whether an official plugin already runs its selected version or a newer one, so an
/// update check needs no download and never downgrades. Index failures return false and
/// surface during the full check.
pub(crate) fn current(home: &Path, entry: &Entry) -> bool {
    selected(home, entry).ok().flatten().is_some_and(|version| !newer(&version, entry))
}

/// Whether `version` would update `entry` rather than keep or downgrade it.
pub(crate) fn newer(version: &str, entry: &Entry) -> bool {
    releases::compare(version, &entry.version) == std::cmp::Ordering::Greater
}

/// Reads the index, preferring a fresh copy and falling back to the cached one offline.
pub(crate) fn load(home: &Path) -> Result<Arc<Index>, Error> {
    let location = std::env::var("KRABB_PLUGIN_INDEX")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| INDEX.into());

    load_from(home, &location)
}

pub(crate) fn load_from(home: &Path, location: &str) -> Result<Arc<Index>, Error> {
    if let Some(path) = location.strip_prefix("file://") {
        let bytes = read_bounded(Path::new(path), INDEX_LIMIT).map_err(|error| {
            format!("The plugin index is unreadable: {}", sentence(error.to_string()))
        })?;

        return parse_index(&bytes, true).map(Arc::new);
    }

    if !location.starts_with("https://") && reference(location).is_none() {
        return Err("KRABB_PLUGIN_INDEX must be an OCI reference such as \
                    ghcr.io/owner/plugins:index, an https:// URL, or a file:// path."
            .into());
    }

    if let Ok(recent) = RECENT.lock()
        && let Some((at, cached, outcome)) = recent.as_ref()
        && cached == location
        && at.elapsed() < FRESH
    {
        return outcome.clone().map_err(Error::from);
    }

    let outcome = refresh(home, location).map_err(|error| error.to_string());

    if let Ok(mut recent) = RECENT.lock() {
        *recent = Some((Instant::now(), location.to_owned(), outcome.clone()));
    }

    outcome.map_err(Error::from)
}

fn refresh(home: &Path, location: &str) -> Result<Arc<Index>, Error> {
    let cache = cache(home, location);

    let error: Error = match fetch(location) {
        Ok(bytes) => match parse_index(&bytes, false) {
            Ok(index) => {
                if let Err(error) = krabb_file::save(&cache, &bytes) {
                    debug!(error = %error, "The plugin index could not be cached.");
                }

                return Ok(Arc::new(index));
            }

            Err(error) => error,
        },

        Err(error) => format!(
            "The plugin index is unavailable: {} Check the network connection, or install \
             with --source.",
            sentence(error.to_string())
        )
        .into(),
    };

    match krabb_file::load(&cache, INDEX_LIMIT).ok().flatten() {
        Some(bytes) => stale(&bytes, error),
        None => Err(error),
    }
}

/// Downloads the index document from an OCI reference or an https:// URL.
fn fetch(location: &str) -> Result<Vec<u8>, Error> {
    let Some((host, repository, tag)) = reference(location) else {
        return download_bounded(location, INDEX_LIMIT);
    };

    let session = temp("krabb-registry")?;
    let headers = session.join("headers");
    let accept = session.join("accept");
    let manifest = session.join("manifest");
    let document = session.join("index");

    let result = token(host, repository).and_then(|authorization| {
        krabb_file::save(&headers, format!("{authorization}\n"))?;
        krabb_file::save(&accept, format!("{authorization}\nAccept: {OCI_MANIFEST}\n"))?;

        download_with(
            &format!("https://{host}/v2/{repository}/manifests/{tag}"),
            &manifest,
            Some(&accept),
            MANIFEST_LIMIT,
        )?;

        let digest = layer(&read_bounded(&manifest, MANIFEST_LIMIT)?)?;

        download_with(
            &format!("https://{host}/v2/{repository}/blobs/sha256:{digest}"),
            &document,
            Some(&headers),
            INDEX_LIMIT,
        )?;

        let bytes = read_bounded(&document, INDEX_LIMIT)?;

        if crate::digest::sha256(&bytes) != digest {
            return Err("The plugin index does not match its registry digest.".into());
        }

        Ok(bytes)
    });

    let _ = std::fs::remove_dir_all(&session);

    result
}

/// The digest of the index document an index manifest points to.
fn layer(manifest: &[u8]) -> Result<String, Error> {
    let value: Value = serde_json::from_slice(manifest)
        .map_err(|_| "The plugin registry returned an invalid index manifest.")?;

    let layer = &value["layers"][0];

    layer["digest"]
        .as_str()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|digest| hex_digest(digest) && layer["mediaType"] == INDEX_MEDIA)
        .map(str::to_owned)
        .ok_or_else(|| "The plugin registry has no index document.".into())
}

/// Splits `host/repository:tag` into its parts when each part is valid.
fn reference(location: &str) -> Option<(&str, &str, &str)> {
    let (repository, tag) = location.rsplit_once(':')?;
    let (host, path) = repository.split_once('/')?;

    let tag_valid = !tag.is_empty()
        && tag.len() <= 128
        && !tag.starts_with(['.', '-'])
        && tag.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));

    (tag_valid && registry(repository, false)).then_some((host, path, tag))
}

fn hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Uses the offline copy after a failed refresh, keeping the refresh error if it is unusable.
fn stale(bytes: &[u8], error: Error) -> Result<Arc<Index>, Error> {
    let index = parse_index(bytes, false).map_err(|_| error.to_string())?;

    warn!(error = %diagnostic(sentence(error.to_string())), "Using the cached plugin index.");

    Ok(Arc::new(index))
}

/// The offline copy of one index location, so another index is never used in its place.
fn cache(home: &Path, location: &str) -> PathBuf {
    if location == INDEX {
        return home.join(format!("{CACHE}.json"));
    }

    home.join(format!("{CACHE}-{}.json", &crate::digest::sha256(location)[..16]))
}

fn parse_index(bytes: &[u8], local: bool) -> Result<Index, Error> {
    let mut index: Index = serde_json::from_slice(bytes)
        .map_err(|error| format!("The plugin index is invalid: {error}."))?;

    // Archives for targets or sizes this host cannot use stay out of selection, so later
    // publications never break older hosts.
    for release in index.plugins.values_mut().flatten() {
        release.artifacts.retain(|target, artifact| {
            releases::published(target) && artifact.size <= ARCHIVE_LIMIT
        });
    }

    validate(&index, local).map_err(|error| format!("The plugin index is invalid: {error}."))?;

    Ok(index)
}

fn validate(index: &Index, local: bool) -> Result<(), String> {
    if index.schema != SCHEMA {
        return Err(format!("it uses schema {}, so update Krabb to read it", index.schema));
    }

    if !registry(&index.registry, local) {
        return Err(format!("registry {} is not an allowed location", index.registry));
    }

    for (id, releases) in &index.plugins {
        if !valid(id) {
            return Err(format!("plugin ID {id} is not valid"));
        }

        let mut versions = BTreeSet::new();

        for release in releases {
            if !releases::valid_version(&release.version) || !versions.insert(&release.version) {
                return Err(format!(
                    "{id} lists version {} more than once or badly",
                    release.version
                ));
            }

            for (target, artifact) in &release.artifacts {
                if !hex_digest(&artifact.sha256) {
                    return Err(format!(
                        "{id} {} has an invalid {target} archive",
                        release.version
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Accepts an OCI repository, or a local directory when the index itself is local.
fn registry(value: &str, local: bool) -> bool {
    if let Some(path) = value.strip_prefix("file://") {
        return local && !path.is_empty();
    }

    let Some((host, path)) = value.split_once('/') else {
        return false;
    };

    let host_valid = !host.is_empty()
        && host.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-:".contains(&byte)
        });

    host_valid
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
                })
        })
}

/// Picks `version`, or the newest stable, non-withdrawn version that speaks `protocol`.
pub(crate) fn select<'a>(
    index: &'a Index,
    id: &str,
    version: Option<&str>,
    protocol: Protocol,
) -> Result<Choice<'a>, Error> {
    let unstable = prerelease(crate::VERSION);

    select_for(index, id, version, unstable, protocol, std::env::consts::OS, std::env::consts::ARCH)
}

/// Whether a Krabb at VERSION follows plugin pre-releases without `--unstable`, because a
/// pre-release host is published together with the plugin pre-releases it was tested with.
pub(crate) fn prerelease(version: &str) -> bool {
    !releases::stable(version)
}

/// The newest version that speaks `protocol`, including pre-releases when `unstable`.
pub(crate) fn newest<'a>(
    index: &'a Index,
    id: &str,
    unstable: bool,
    protocol: Protocol,
) -> Result<Choice<'a>, Error> {
    select_for(index, id, None, unstable, protocol, std::env::consts::OS, std::env::consts::ARCH)
}

/// The pin for the newest version of `id`, pre-releases included; none when it is stable.
pub(crate) fn unstable(home: &Path, id: &str) -> Result<Option<String>, Error> {
    let index = load(home)?;
    let version = &newest(&index, id, true, Protocol::CURRENT)?.release.version;

    Ok((!releases::stable(version)).then(|| version.clone()))
}

fn select_for<'a>(
    index: &'a Index,
    id: &str,
    version: Option<&str>,
    unstable: bool,
    protocol: Protocol,
    os: &str,
    arch: &str,
) -> Result<Choice<'a>, Error> {
    let (target, extension) = releases::target(os, arch).ok_or_else(|| {
        format!(
            "Official plugins are not published for {os} ({arch}).\nInstall a local build with \
             `krabb plugin install {id} --source <path>`."
        )
    })?;

    let releases = index.plugins.get(id).ok_or_else(|| {
        format!(
            "Plugin {id} is not in the official plugin index.\nCheck the ID, or install a local \
             build with `krabb plugin install {id} --source <path>`."
        )
    })?;

    let release = match version {
        Some(version) => {
            let release =
                releases.iter().find(|release| release.version == version).ok_or_else(|| {
                    format!(
                        "Plugin {id} has no version {version}.\nSee every release with \
                         `krabb plugin versions {id}`."
                    )
                })?;

            if !protocol.compatible(release.protocol) {
                return Err(format!(
                    "Plugin {id} {version} needs plugin protocol {}.{}, which this version of \
                     Krabb does not support.\nSee every release with `krabb plugin versions {id}`.",
                    release.protocol.major, release.protocol.minor
                )
                .into());
            }

            release
        }

        None => {
            let supported = releases
                .iter()
                .filter(|release| {
                    release.yanked.is_none()
                        && (unstable || releases::stable(&release.version))
                        && protocol.compatible(release.protocol)
                })
                .collect::<Vec<_>>();

            if supported.is_empty() {
                return Err(Unavailable::new(id, releases, unstable, protocol, target).into());
            }

            supported
                .into_iter()
                .filter(|release| release.artifacts.contains_key(target))
                .max_by(|left, right| releases::compare(&left.version, &right.version))
                .ok_or_else(|| {
                    format!(
                        "No release of plugin {id} has a build for {target}.\nInstall a local \
                         build with `krabb plugin install {id} --source <path>`."
                    )
                })?
        }
    };

    let artifact = release.artifacts.get(target).ok_or_else(|| {
        format!(
            "Plugin {id} {} has no build for {target}.\nInstall a local build with `krabb plugin \
             install {id} --source <path>`.",
            release.version
        )
    })?;

    Ok(Choice { release, artifact, target, extension })
}

/// No release of a plugin can run on this host. Its message states only the fact, so an
/// update that embeds it stays accurate, and `next` gives the command for an install.
#[derive(Debug)]
pub(crate) struct Unavailable {
    id: String,
    prerelease: bool,
}

impl Unavailable {
    fn new(
        id: &str,
        releases: &[Release],
        unstable: bool,
        protocol: Protocol,
        target: &str,
    ) -> Self {
        let prerelease = !unstable
            && releases.iter().any(|release| {
                release.yanked.is_none()
                    && !releases::stable(&release.version)
                    && protocol.compatible(release.protocol)
                    && release.artifacts.contains_key(target)
            });

        Self { id: id.to_owned(), prerelease }
    }

    /// The command that installs the plugin anyway, or that lists its releases.
    pub(crate) fn next(&self) -> String {
        if self.prerelease {
            format!(
                "Install the newest pre-release with `krabb plugin install {} --unstable`.",
                self.id
            )
        } else {
            format!("See every release with `krabb plugin versions {}`.", self.id)
        }
    }
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.prerelease {
            write!(
                formatter,
                "No stable release of plugin {} works with this version of Krabb.",
                self.id
            )
        } else {
            write!(formatter, "No release of plugin {} works with this version of Krabb.", self.id)
        }
    }
}

impl std::error::Error for Unavailable {}

/// Downloads a chosen archive into `dir` and verifies it against its index digest.
pub(crate) fn pull(index: &Index, id: &str, choice: &Choice, dir: &Path) -> Result<PathBuf, Error> {
    let version = &choice.release.version;
    let digest = &choice.artifact.sha256;
    let destination = dir.join(releases::archive(id, version, choice.target, choice.extension));

    if let Some(root) = index.registry.strip_prefix("file://") {
        let blob = Path::new(root).join("blobs/sha256").join(digest);

        let copied = std::fs::metadata(&blob).and_then(|metadata| {
            if metadata.len() > ARCHIVE_LIMIT {
                return Err(std::io::Error::other("Archive exceeds the size limit."));
            }

            std::fs::copy(&blob, &destination)
        });

        copied.map_err(|error| {
            format!(
                "Plugin {id} {version} is unavailable at {}: {}",
                blob.display(),
                sentence(error.to_string())
            )
        })?;
    } else {
        let (host, repository) =
            index.registry.split_once('/').ok_or("The plugin registry is invalid.")?;
        let session = temp("krabb-registry")?;
        let headers = session.join("headers");

        let result = token(host, repository).and_then(|authorization| {
            krabb_file::save(&headers, format!("{authorization}\n"))?;

            download_with(
                &format!("https://{host}/v2/{repository}/blobs/sha256:{digest}"),
                &destination,
                Some(&headers),
                ARCHIVE_LIMIT,
            )
        });

        let _ = std::fs::remove_dir_all(&session);

        result.map_err(|error| {
            format!(
                "Plugin {id} {version} could not be downloaded: {}",
                sentence(error.to_string())
            )
        })?;
    }

    verify_archive(&destination, Some(digest)).map_err(|error| {
        format!("Plugin {id} {version} failed verification: {}", sentence(error.to_string()))
    })?;

    Ok(destination)
}

/// Requests an anonymous pull token as an `Authorization` request header.
fn token(host: &str, repository: &str) -> Result<String, Error> {
    let response = download_bounded(
        &format!("https://{host}/token?scope=repository:{repository}:pull&service={host}"),
        TOKEN_LIMIT,
    )?;

    let value: Value = serde_json::from_slice(&response)
        .map_err(|_| "The plugin registry returned an invalid token response.")?;

    let token = value["token"]
        .as_str()
        .or_else(|| value["access_token"].as_str())
        .filter(|token| !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_graphic()))
        .ok_or("The plugin registry returned no access token.")?;

    Ok(format!("Authorization: Bearer {token}"))
}

/// Explains a pinned install and names the newest supported version when it differs.
pub(crate) fn notice(home: &Path, id: &str, version: &str) -> Option<String> {
    pin(&*load(home).ok()?, id, version)
}

fn pin(index: &Index, id: &str, version: &str) -> Option<String> {
    let release = index.plugins.get(id)?.iter().find(|release| release.version == version)?;
    let newest_version = |unstable| {
        newest(index, id, unstable, Protocol::CURRENT).ok().map(|choice| &choice.release.version)
    };

    let latest = newest_version(false);
    let newer = |candidate: &str| releases::compare(candidate, version).is_gt();

    let follow = format!("Run `krabb plugin install {id}@latest --yes` to");
    let mut lines = Vec::new();

    if let Some(reason) = &release.yanked {
        lines.push(format!("{id} {version} was withdrawn: {}", sentence(reason.as_str())));
    }

    lines.push(if releases::stable(version) {
        match latest {
            Some(latest) if latest != version => format!(
                "{id} is pinned to {version}, and the latest supported version is {latest}.\nThe \
                pinned version is used at your own risk.\n{follow} follow updates again."
            ),

            _ => format!("{id} is pinned to {version}.\n{follow} follow updates again."),
        }
    } else if let Some(stable) = latest.filter(|stable| newer(stable)) {
        format!(
            "{id} is pinned to pre-release {version}, and {stable} is newer.\n{follow} follow \
            stable releases."
        )
    } else if newest_version(true).is_some_and(|candidate| newer(candidate)) {
        format!(
            "{id} is pinned to pre-release {version}.\nRun `krabb plugin install {id} --unstable \
            --yes` to move to a newer pre-release."
        )
    } else {
        format!("{id} is pinned to pre-release {version}.")
    });

    Some(lines.join(" "))
}

/// Implements `krabb plugin versions`: every published version, newest first.
pub(crate) fn versions(home: &Path, id: &str, json: bool) -> Result<(), Error> {
    if !valid(id) {
        return Err(format!("Invalid plugin ID: {id}.").into());
    }

    let index = load(home)?;
    let rows = history(&index, id, installed(home, id).as_deref(), prerelease(crate::VERSION))?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"id": id, "versions": rows}))?
        );

        return Ok(());
    }

    let lines = rows
        .iter()
        .map(|row| match notes(row) {
            notes if notes.is_empty() => format!("{} ({})", row.version, row.released),
            notes => format!("{} ({}) {}", row.version, row.released, notes.join(", ")),
        })
        .collect::<Vec<_>>();

    println!(
        "Versions of {id}\n{}\n\nInstall one with `krabb plugin install {id}@VERSION`.\nVersions \
         other than the latest are used at your own risk.",
        format_tree_list(&lines)
    );

    Ok(())
}

/// The installed version of an official plugin, if any.
fn installed(home: &Path, id: &str) -> Option<String> {
    load_lock_at(home)
        .ok()?
        .plugins
        .remove(id)
        .filter(|entry| official(entry).is_some())
        .map(|entry| entry.version)
}

/// One published version as `plugin versions` reports it.
#[derive(Serialize)]
struct Row<'a> {
    version: &'a str,
    released: &'a str,
    protocol: Protocol,
    permissions: &'a [String],
    secrets: &'a [String],
    yanked: Option<&'a str>,
    compatible: bool,
    available: bool,
    latest: bool,
    installed: bool,
}

/// The labels `plugin versions` prints after a version.
fn notes(row: &Row) -> Vec<String> {
    [
        row.latest.then(|| "latest".to_owned()),
        row.installed.then(|| "installed".to_owned()),
        (!releases::stable(row.version)).then(|| "pre-release".to_owned()),
        row.yanked.map(|reason| format!("withdrawn: {reason}")),
        (!row.compatible)
            .then(|| format!("needs protocol {}.{}", row.protocol.major, row.protocol.minor)),
        (!row.available).then(|| "no build for this system".to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn history<'a>(
    index: &'a Index,
    id: &str,
    installed: Option<&str>,
    unstable: bool,
) -> Result<Vec<Row<'a>>, Error> {
    let releases = index.plugins.get(id).ok_or_else(|| {
        format!("Plugin {id} is not in the official plugin index, so check the ID.")
    })?;

    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let latest = select_for(index, id, None, unstable, Protocol::CURRENT, os, arch)
        .ok()
        .map(|choice| choice.release);
    let target = releases::target(os, arch).map(|(t, _)| t);
    let mut sorted = releases.iter().collect::<Vec<_>>();

    sorted.sort_by(|left, right| releases::compare(&right.version, &left.version));

    Ok(sorted
        .into_iter()
        .map(|release| {
            let compatible = Protocol::CURRENT.compatible(release.protocol);
            let available = target.is_some_and(|target| release.artifacts.contains_key(target));
            let latest = latest.is_some_and(|latest| latest.version == release.version);
            let installed = installed == Some(release.version.as_str());

            Row {
                version: &release.version,
                released: &release.released,
                protocol: release.protocol,
                permissions: &release.permissions,
                secrets: &release.secrets,
                yanked: release.yanked.as_deref(),
                compatible,
                available,
                latest,
                installed,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests;
