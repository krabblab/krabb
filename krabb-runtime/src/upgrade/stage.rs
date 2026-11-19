//! Download, verify, and unpack a release set before the installation changes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::*;

pub(super) const PLAN: &str = "plan.json";

const PLAN_LIMIT: u64 = 1024 * 1024;

/// Staged release set handed to the new binary; later releases must keep reading it.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Plan {
    pub version: String,
    pub plugins: Vec<Artifact>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Artifact {
    pub id: String,
    /// Release archive URL with its `#sha256=` digest, recorded in the plugin lock.
    pub source: String,
    /// Verified archive file name inside the staging directory.
    pub file: String,
}

/// What the downloaded host reports about itself and this home.
#[derive(Debug, Deserialize)]
pub(super) struct Inspection {
    pub version: String,
    pub schema: Schema,
    /// Plugin protocol the downloaded host speaks; absent from releases that predate it.
    #[serde(default)]
    pub protocol: Option<Protocol>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Schema {
    pub from: u32,
    pub to: u32,
    pub steps: Vec<String>,
}

/// A prepared release set; its directory is removed unless it is kept.
pub(super) struct Staged {
    pub dir: PathBuf,
    pub krabb: PathBuf,
    pub daemon: PathBuf,
    pub plan: Plan,
    pub manifests: BTreeMap<String, Manifest>,
    keep: bool,
}

impl Staged {
    pub fn keep(&mut self) {
        self.keep = true;
    }

    pub fn discard(mut self) {
        self.keep = false;
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.keep {
            let _ = krabb_migrations::recovery::remove(&self.dir);
        }
    }
}

pub(super) fn prepare(
    root: &Path,
    version: &str,
    ids: &[String],
    base: Option<&str>,
) -> Result<Staged, PluginError> {
    let dir = root.join("updates").join(format!("v{version}"));

    krabb_migrations::recovery::remove(&dir)?;
    std::fs::create_dir_all(&dir)?;

    let mut staged = Staged {
        dir: dir.clone(),
        krabb: PathBuf::new(),
        daemon: PathBuf::new(),
        plan: Plan { version: version.to_owned(), plugins: Vec::new() },
        manifests: BTreeMap::new(),
        keep: false,
    };

    let core = fetch(&dir, &releases::source_at("core", version, base)?)?;
    let unpacked = dir.join("core");

    std::fs::create_dir(&unpacked)?;
    unpack(&core, &unpacked)?;

    staged.krabb = binary(&unpacked, "krabb")?;
    staged.daemon = binary(&unpacked, "krabb-daemon")?;

    for id in ids {
        let source = releases::source_at(id, version, base)?;
        let archive = fetch(&dir, &source)?;
        let manifest = inspect_plugin(id, &archive, &source)?;
        let file = archive.file_name().map(|name| name.to_string_lossy().into_owned());

        staged.manifests.insert(id.clone(), manifest);
        staged.plan.plugins.push(Artifact {
            id: id.clone(),
            source,
            file: file.unwrap_or_default(),
        });
    }

    krabb_file::save(dir.join(PLAN), serde_json::to_vec_pretty(&staged.plan)?)?;

    Ok(staged)
}

pub(super) fn plan(dir: &Path) -> Result<Plan, PluginError> {
    let bytes = krabb_file::load(dir.join(PLAN), PLAN_LIMIT)?
        .ok_or("The staged update is missing its plan.")?;

    Ok(serde_json::from_slice(&bytes)?)
}

/// Rejects staged plugins the downloaded host cannot talk to, before anything is replaced.
pub(super) fn compatible(staged: &Staged, inspection: &Inspection) -> Result<(), PluginError> {
    let Some(protocol) = inspection.protocol else {
        return Ok(());
    };

    for (id, manifest) in &staged.manifests {
        if !protocol.compatible(manifest.protocol) {
            return Err(format!(
                "The {id} plugin in Krabb {} uses protocol {}.{}, which that release does not \
                 support. The installation was not changed.",
                inspection.version, manifest.protocol.major, manifest.protocol.minor
            )
            .into());
        }
    }

    Ok(())
}

/// Runs the downloaded host once; it must start here and accept this home.
pub(super) fn inspect(krabb: &Path, root: &Path) -> Result<Inspection, PluginError> {
    let output = command_output(
        std::process::Command::new(krabb)
            .args(["update", "--inspect", "--json"])
            .env("KRABB_HOME", root),
    )?;

    if !output.status.success() {
        return Err(format!(
            "The downloaded Krabb could not run on this system: {}",
            sentence(redact(&output.stderr))
        )
        .into());
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|_| "The downloaded Krabb returned an invalid update report.".into())
}

/// Replaces `live` with a copy of `replacement`, moving the old file aside first so a
/// running executable can be replaced on every supported OS.
pub(super) fn swap(live: &Path, replacement: &Path) -> std::io::Result<()> {
    let directory = live.parent().ok_or_else(|| std::io::Error::other("Binary has no parent."))?;
    let name = live.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let nonce = now();
    let incoming = directory.join(format!(".{name}.new-{nonce}"));
    let outgoing = directory.join(format!(".{name}.old-{nonce}"));

    copy_binary(replacement, &incoming)?;

    let present = std::fs::symlink_metadata(live).is_ok();

    if present && let Err(error) = std::fs::rename(live, &outgoing) {
        let _ = std::fs::remove_file(&incoming);

        return Err(error);
    }

    if let Err(error) = std::fs::rename(&incoming, live) {
        if present {
            let _ = std::fs::rename(&outgoing, live);
        }

        let _ = std::fs::remove_file(&incoming);

        return Err(error);
    }

    let _ = std::fs::remove_file(&outgoing);

    Ok(())
}

/// Removes executables that an earlier swap could not delete while they were running.
pub(super) fn sweep(live: &Path) {
    let (Some(directory), Some(name)) = (live.parent(), live.file_name()) else {
        return;
    };

    let prefix = format!(".{}.old-", name.to_string_lossy());

    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };

    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Confirms a binary directory accepts new files before anything is replaced.
pub(super) fn writable(binary: &Path) -> Result<(), PluginError> {
    let directory = binary.parent().ok_or("Binary has no parent directory.")?;
    let probe = directory.join(format!(".krabb-update-{}", now()));

    std::fs::write(&probe, b"").map_err(|error| {
        format!(
            "Krabb cannot replace files in {}: {} Update with the tool that installed it, or \
             reinstall Krabb in a writable directory.",
            directory.display(),
            sentence(error.to_string())
        )
    })?;

    let _ = std::fs::remove_file(probe);

    Ok(())
}

fn fetch(dir: &Path, source: &str) -> Result<PathBuf, PluginError> {
    let (url, digest) = releases::parts(source).ok_or("Release source has no checksum.")?;
    let name = url.rsplit('/').next().filter(|name| !name.is_empty()).ok_or("Bad release URL.")?;
    let destination = dir.join(name);

    if let Some(path) = url.strip_prefix("file://") {
        if std::fs::metadata(path)?.len() > ARCHIVE_LIMIT {
            return Err("Archive exceeds the size limit.".into());
        }

        std::fs::copy(path, &destination)?;
    } else {
        download(url, &destination)?;
    }

    verify_archive(&destination, Some(digest))
        .map_err(|error| format!("{name} failed verification: {}", sentence(error.to_string())))?;

    Ok(destination)
}

fn inspect_plugin(id: &str, archive: &Path, source: &str) -> Result<Manifest, PluginError> {
    let digest = releases::parts(source).map(|(_, digest)| digest).unwrap_or_default();
    let root = resolve(&format!("{}#sha256={digest}", archive.display()), None)?;
    let bytes = read_bounded(&root.path.join("krabb-plugin.toml"), MANIFEST_LIMIT)?;

    let manifest: Manifest = toml::from_slice(&bytes)
        .map_err(|error| format!("The {id} release manifest is invalid: {error}"))?;

    if manifest.id != id {
        return Err(format!("The {id} release archive contains plugin {}.", manifest.id).into());
    }

    Ok(manifest)
}

fn binary(unpacked: &Path, name: &str) -> Result<PathBuf, PluginError> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);

    for entry in std::fs::read_dir(unpacked)? {
        let candidate = entry?.path().join("bin").join(&file);

        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(format!("The release archive does not contain {file}.").into())
}
