//! Resolves, links, and copies plugin sources into the plugin home.

use crate::*;

pub(crate) fn link(
    source: Source,
    linked: bool,
) -> Result<Manifest, Box<dyn std::error::Error + Send + Sync>> {
    link_at(source, linked, &home())
}

pub(crate) fn default_plugin_source(
    plugins_root: &Path,
    id: &str,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let direct = plugins_root.join(id);

    if direct.is_dir() {
        return Ok(direct);
    }

    for group in ["intelligence", "channels", "clients", "messaging"] {
        let grouped = plugins_root.join(group).join(id);

        if grouped.is_dir() {
            return Ok(grouped);
        }
    }

    Err(format!(
        "Plugin {id} was not found under {}, so pass a source path or Git URL.",
        plugins_root.display()
    )
    .into())
}

pub(crate) fn ensure_linkable_sources(
    linked: bool,
    sources: &[Source],
    working_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !linked || sources.iter().any(|source| source.source.is_some()) {
        return Ok(());
    }

    let plugins_root = working_dir.join("krabb-plugins");

    if sources.iter().any(|source| default_plugin_source(&plugins_root, &source.id).is_ok()) {
        return Ok(());
    }

    Err(concat!(
        "No linkable plugins were found in the current working directory. ",
        "Run this command from the repository root or specify a local plugin with --source PATH."
    )
    .into())
}

pub(crate) fn link_at(
    source: Source,
    linked: bool,
    home_root: &Path,
) -> Result<Manifest, Box<dyn std::error::Error + Send + Sync>> {
    link_recorded(source, linked, home_root, None)
}

/// Installs from `source` and locks `recorded` as its origin when it was fetched elsewhere,
/// such as a staged archive of an official plugin.
pub(crate) fn link_recorded(
    source: Source,
    linked: bool,
    home_root: &Path,
    recorded: Option<&str>,
) -> Result<Manifest, Box<dyn std::error::Error + Send + Sync>> {
    let _lock = lock_at(home_root, ".plugins.lock")?;

    if !valid(&source.id) {
        return Err(format!("Invalid plugin ID: {}.", source.id).into());
    }

    let default_source = source.source.is_none() && linked;

    let origin = match source.source.clone() {
        Some(source) => source,

        None if linked => {
            default_plugin_source(Path::new("krabb-plugins"), &source.id)?.display().to_string()
        }

        None => index::source(&source.id, source.version.as_deref()),
    };

    let root = resolve_at(home_root, &origin, source.revision.as_deref())?;

    if linked && root.temp.is_some() {
        return Err(
            "--link requires a stable local plugin source, so install remote plugins without --link."
                .into(),
        );
    }

    let root_path = root.path.clone();

    let manifest = read_manifest(&root_path)
        .ok_or_else(|| format!("Manifest not found in {}.", root_path.display()))?;

    if manifest.id != source.id {
        return Err(format!("Manifest ID {} does not match {}.", manifest.id, source.id).into());
    }

    if !Protocol::CURRENT.compatible(manifest.protocol) {
        return Err(format!("Plugin {} requires unsupported protocol.", manifest.id).into());
    }

    if let Some(error) = manifest.needs_newer_krabb() {
        return Err(error.into());
    }

    manifest.validate().map_err(|error| format!("Invalid plugin manifest: {error}"))?;
    validate_commands(home_root, &source.id, &manifest.commands)?;

    updates::state::check(home_root, load_lock_at(home_root)?.plugins.get(&source.id), &manifest)?;

    let dest = home_root.join("plugins").join(&source.id);

    if root_path == std::fs::canonicalize(&dest).unwrap_or_default() {
        return Err("A plugin source cannot be its managed destination.".into());
    }

    if dest.exists() && !source.yes {
        return Err(
            format!("Plugin {} already exists, so use --yes to replace it.", source.id).into()
        );
    }

    let name = plugin_name(&source.id);

    let Some(binary) = plugin_binary(&root_path, &source.id, default_source) else {
        return Err(format!("Binary {name} was not found, so build the plugin first.").into());
    };

    let plugins = home_root.join("plugins");

    std::fs::create_dir_all(&plugins)?;

    let nonce = format!("{}-{}", std::process::id(), now());
    let stage = plugins.join(format!(".{}.stage-{nonce}", source.id));
    let backup = plugins.join(format!(".{}.backup-{nonce}", source.id));

    let result = (|| {
        std::fs::create_dir_all(stage.join("bin"))?;
        std::fs::copy(root_path.join("krabb-plugin.toml"), stage.join("krabb-plugin.toml"))?;
        let staged = stage.join("bin").join(&name);

        if linked && root.temp.is_none() {
            link_binary(&binary, &staged)?;
        } else {
            copy_binary(&binary, &staged)?;
        }

        let config = load_config_at(home_root)?;

        config
            .validate()
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
        health(home_root, staged.clone(), &source.id, &config)?;

        let mut lock = load_lock_at(home_root)?;
        let previous_lock = lock.clone();
        lock.plugins.insert(
            source.id.clone(),
            Entry {
                source: recorded.map_or_else(
                    || match index::parse(&origin) {
                        Some(_) => origin.clone(),
                        None => canonical_source(&origin),
                    },
                    str::to_owned,
                ),
                revision: revision(&root_path),
                pinned: source.revision.as_deref().is_some_and(|value| value != "local"),
                hash: digest(&stage.join("krabb-plugin.toml"), &staged)?,
                version: manifest.version.clone(),
                protocol: manifest.protocol,
                capabilities: manifest.capabilities.clone(),
                permissions: manifest.permissions.clone(),
                variables: manifest.variables.clone(),
                settings: manifest.settings.clone(),
                commands: manifest.commands.clone(),
                linked,
                default: default_source,
                state: manifest.state.clone(),
            },
        );

        if dest.exists() {
            std::fs::rename(&dest, &backup)?;
        }

        if let Err(error) = std::fs::rename(&stage, &dest) {
            if backup.exists() {
                let _ = std::fs::rename(&backup, &dest);
            }

            return Err(error.into());
        }

        if let Err(error) = save_lock_at(home_root, &lock) {
            let restored_lock = save_lock_at(home_root, &previous_lock);
            let restored_plugin = (|| {
                if dest.exists() {
                    std::fs::remove_dir_all(&dest)?;
                }

                if backup.exists() {
                    std::fs::rename(&backup, &dest)?;
                }

                Ok::<(), std::io::Error>(())
            })();

            if let Err(restore) = restored_lock {
                return Err(format!(
                    "Could not save the plugin lock: {}. Rollback failed: {}.",
                    sentence(error.to_string()),
                    sentence(restore.to_string())
                )
                .into());
            }

            if let Err(restore) = restored_plugin {
                return Err(format!(
                    "Could not save the plugin lock: {}. Rollback failed: {}.",
                    sentence(error.to_string()),
                    sentence(restore.to_string())
                )
                .into());
            }

            return Err(error);
        }

        if backup.exists() {
            let _ = std::fs::remove_dir_all(&backup);
        }

        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&stage);
    }

    result?;

    if let Err(error) = updates::state::release(home_root, &source.id) {
        tracing::warn!(plugin = %source.id, %error, "Kept plugin state record was not cleared.");
    }

    Ok(manifest)
}

pub(crate) fn copy_binary(source: &Path, destination: &Path) -> std::io::Result<u64> {
    let bytes = std::fs::copy(source, destination)?;

    #[cfg(unix)]
    {
        let mut permissions = std::fs::metadata(destination)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(destination, permissions)?;
    }

    Ok(bytes)
}

pub(crate) fn digest(
    manifest: &Path,
    binary: &Path,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut hash = Sha256::new();

    hash.update(std::fs::read(manifest)?);

    let mut file = std::fs::File::open(binary)?;

    if file.metadata()?.len() > ARCHIVE_EXPANDED {
        return Err("Plugin binary exceeds the integrity-check size limit.".into());
    }

    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let count = std::io::Read::read(&mut file, &mut buffer)?;

        if count == 0 {
            break;
        }

        hash.update(&buffer[..count]);
    }

    Ok(hex_digest(hash.finalize()))
}

pub(crate) fn revision(path: impl AsRef<Path>) -> String {
    command_output(git_command().args(["-C"]).arg(path.as_ref()).args(["rev-parse", "HEAD"]))
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "local".into(), |value| value.trim().into())
}

pub(crate) fn relocated_plugin_source(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return None;
    }

    let parent = path.parent()?;
    let name = path.file_name()?;

    let (plugins, relative) = if parent.ends_with("krabb-plugins/messaging") {
        (parent.parent()?, Path::new("channels").join(name))
    } else if parent.file_name()? == "krabb-plugins" && name == krabb_core::client::TUI {
        (parent, PathBuf::from("clients/tui"))
    } else {
        return None;
    };

    let relocated = plugins.join(relative);

    relocated.is_dir().then_some(relocated)
}

/// Resolves any plugin source, including official ones selected through the plugin index.
pub(crate) fn resolve_at(
    home_root: &Path,
    source: &str,
    wanted: Option<&str>,
) -> Result<SourceRoot, Box<dyn std::error::Error + Send + Sync>> {
    match index::parse(source) {
        Some((id, version)) => index::resolve(home_root, id, version),
        None => resolve(source, wanted),
    }
}

pub(crate) fn resolve(
    source: &str,
    wanted: Option<&str>,
) -> Result<SourceRoot, Box<dyn std::error::Error + Send + Sync>> {
    let (source, expected) =
        source.split_once("#sha256=").map_or((source, None), |(value, hash)| (value, Some(hash)));

    if let Some(expected) = expected
        && (expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err("Archive SHA-256 checksums must contain 64 hexadecimal characters.".into());
    }

    let source_path = source.strip_prefix("file://").map(PathBuf::from).or_else(|| {
        let path = Path::new(source);
        (path.is_absolute() || source.starts_with("./") || source.starts_with("../"))
            .then(|| path.to_path_buf())
    });

    if let Some(path) = source_path
        .or_else(|| Path::new(source).exists().then(|| PathBuf::from(source)))
        .or_else(|| relocated_plugin_source(Path::new(source)))
    {
        let path = relocated_plugin_source(&path).unwrap_or(path);

        let path = std::fs::canonicalize(&path).map_err(|error| {
            format!(
                "Local plugin source {} is unavailable: {} Restore the source or reinstall the plugin with --yes from its new location.",
                path.display(), sentence(error.to_string())
            )
        })?;

        if path.is_file() {
            if !archive(&path) {
                return Err(
                    "Plugin source files must be .tar, .tar.gz, .tgz, or .zip archives.".into()
                );
            }

            let temp = archive_temp()?;

            if let Err(error) = verify_archive(&path, expected) {
                let _ = std::fs::remove_dir_all(&temp);

                return Err(error);
            }

            if let Err(error) = unpack(&path, &temp) {
                let _ = std::fs::remove_dir_all(&temp);

                return Err(error);
            }

            let root = match archive_root(&temp) {
                Ok(root) => root,

                Err(error) => {
                    let _ = std::fs::remove_dir_all(&temp);
                    return Err(error);
                }
            };

            return Ok(SourceRoot { path: root, temp: Some(temp) });
        }

        if let Some(wanted) = wanted {
            let actual = revision(&path);

            if wanted != "local" && (actual == "local" || actual != wanted) {
                return Err(format!("Source revision does not match {wanted}.").into());
            }
        }

        return Ok(SourceRoot { path, temp: None });
    }

    let url = source.strip_prefix("git+").unwrap_or(source);

    if archive_url(url) {
        if !url.starts_with("https://") {
            return Err("Remote archive sources must use HTTPS.".into());
        }

        let Some(expected) = expected else {
            return Err("Remote archive sources require a #sha256= checksum.".into());
        };

        let temp = archive_temp()?;

        let archive_name = url
            .split('?')
            .next()
            .and_then(|value| value.rsplit('/').next())
            .filter(|value| !value.is_empty())
            .unwrap_or("source.tar");

        let archive_path = temp.join(archive_name);

        let result = (|| {
            download(url, &archive_path)?;
            verify_archive(&archive_path, Some(expected))?;
            unpack(&archive_path, &temp)?;
            let root = archive_root(&temp)?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(root)
        })();

        return match result {
            Ok(root) => Ok(SourceRoot { path: root, temp: Some(temp) }),

            Err(error) => {
                let _ = std::fs::remove_dir_all(&temp);
                Err(error)
            }
        };
    }

    if embedded(url) {
        return Err("Plugin source URLs must not contain embedded credentials.".into());
    }

    if url.starts_with("http://") {
        return Err("Remote Git sources must use HTTPS or SSH.".into());
    }

    let git = url.starts_with("https://")
        || url.starts_with("ssh://")
        || url.starts_with("git@")
        || url.starts_with("file://")
        || url.ends_with(".git");

    if !git {
        return Err("Remote plugin sources must use a Git URL.".into());
    }

    let staging = temp("krabb-source")?;

    let result = (|| {
        let repo = staging.join("repo");
        let output = command_output_limited(
            git_command().args(["clone", "--depth", "1", "--no-tags"]).arg(url).arg(&repo),
            Some(&staging),
            ARCHIVE_EXPANDED,
        )?;

        if !output.status.success() {
            return Err(format!(
                "Git source could not be cloned: {}",
                sentence(redact(&output.stderr))
            )
            .into());
        }

        if let Some(wanted) = wanted.filter(|value| !value.is_empty()) {
            let checkout = command_output_limited(
                git_command().args(["-C"]).arg(&repo).args(["checkout", "--detach", wanted]),
                Some(&staging),
                ARCHIVE_EXPANDED,
            )?;

            if !checkout.status.success() {
                let fetch = command_output_limited(
                    git_command()
                        .args(["-C"])
                        .arg(&repo)
                        .args(["fetch", "--depth", "1", "origin", wanted]),
                    Some(&staging),
                    ARCHIVE_EXPANDED,
                )?;

                if !fetch.status.success() {
                    return Err(format!(
                        "Git source revision could not be fetched: {}",
                        sentence(redact(&fetch.stderr))
                    )
                    .into());
                }

                let checkout = command_output_limited(
                    git_command().args(["-C"]).arg(&repo).args(["checkout", "--detach", wanted]),
                    Some(&staging),
                    ARCHIVE_EXPANDED,
                )?;

                if !checkout.status.success() {
                    return Err(
                        format!("Git source revision could not be checked out: {wanted}.").into()
                    );
                }
            }
        }

        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(SourceRoot {
            path: repo,
            temp: Some(staging.clone()),
        })
    })();

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }

    result
}
