//! Exports, validates, and imports Krabfiles.

use crate::*;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Krabfile {
    pub(crate) version: String,
    pub(crate) config: Config,
    pub(crate) plugins: Vec<KrabbPlugin>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KrabbPlugin {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) revision: String,
    pub(crate) version: String,
    pub(crate) capabilities: Vec<String>,
}

impl Krabfile {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.version != KRABFILE_VERSION {
            return Err(format!(
                "unsupported version {} (expected {})",
                self.version, KRABFILE_VERSION
            ));
        }

        self.config.validate().map_err(|error| format!("configuration: {error}"))?;

        let mut ids = BTreeSet::new();

        for plugin in &self.plugins {
            if !valid(&plugin.id) {
                return Err(format!(
                    "plugin {} has an invalid ID, which must use lowercase letters, digits, or \
                    hyphens",
                    plugin.id
                ));
            }

            if plugin.source.trim().is_empty() {
                return Err(format!("plugin {} has an empty source", plugin.id));
            }

            if !ids.insert(&plugin.id) {
                return Err(format!("plugin {} is listed more than once", plugin.id));
            }
        }

        Ok(())
    }
}

pub(crate) const KRABFILE_VERSION: &str = "0.1";

pub(crate) fn export_krabfile(
    args: KrabfileExport,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    export_krabfile_at_mode(args, &home(), json)
}

#[cfg(test)]
pub(crate) fn export_krabfile_at(
    args: KrabfileExport,
    root: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    export_krabfile_at_mode(args, root, false)
}

pub(crate) fn export_krabfile_at_mode(
    args: KrabfileExport,
    root: &Path,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = krabfile_output_path(args.destination, args.path)?;

    if path.exists() && !args.force {
        return Err(format!(
            "Krabfile already exists at {}; use --force to overwrite it.",
            path.display()
        )
        .into());
    }

    let config = load_config_at(root)?;

    let lock = load_lock_at(root)?;

    let mut plugins = lock
        .plugins
        .into_iter()
        .map(|(id, entry)| KrabbPlugin {
            id,
            source: entry.source,
            revision: entry.revision,
            version: entry.version,
            capabilities: entry.capabilities,
        })
        .collect::<Vec<_>>();

    plugins.sort_by(|left, right| left.id.cmp(&right.id));

    let file = Krabfile { version: KRABFILE_VERSION.into(), config, plugins };
    let text = toml::to_string_pretty(&file)?;

    secure(&path, text.as_bytes())?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "action": "export",
                "path": path,
                "plugins": file.plugins.len(),
                "status": "exported"
            })
        );
    } else {
        println!("Exported {}.", path.display());
    }

    info!(plugins = file.plugins.len(), "Krabfile exported.");

    Ok(())
}

pub(crate) fn import_krabfile(
    args: KrabfileImport,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    import_krabfile_at_mode(args, &home(), json)
}

pub(crate) fn validate_krabfile(
    args: KrabfileValidate,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = args.path.unwrap_or_else(default_krabfile_path);
    let file = read_krabfile(&path)?;

    validate_krabfile_spec(&file, &path)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "path": path,
                "plugins": file.plugins.len(),
                "status": "valid",
                "version": file.version,
            }))?
        );
    } else {
        println!("Krabfile at {} is valid.", path.display());
    }

    info!(plugins = file.plugins.len(), "Krabfile validated.");

    Ok(())
}

#[cfg(test)]
pub(crate) fn import_krabfile_at(
    args: KrabfileImport,
    root: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    import_krabfile_at_mode(args, root, false)
}

pub(crate) fn import_krabfile_at_mode(
    args: KrabfileImport,
    root: &Path,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = args.path.unwrap_or_else(default_krabfile_path);

    let file = read_krabfile(&path)?;

    validate_krabfile_spec(&file, &path)?;

    let destination = root.join("config.toml");

    let previous_config = load_config_text(root)?;

    if previous_config.is_some() && !args.force {
        return Err("Configuration already exists; use --force to replace it.".into());
    }

    if !confirm_action(args.yes, json, &format!("Import configuration from {}?", path.display()))? {
        print_cancelled("Import", json);
        return Ok(());
    }

    init_at(root)?;
    secure(&destination, toml::to_string_pretty(&file.config)?.as_bytes())?;

    let spinner = if !json && !file.plugins.is_empty() {
        terminal::Spinner::start("Importing configuration and plugins...")
    } else {
        None
    };

    let result = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for plugin in &file.plugins {
            let official = index::parse(&plugin.source);

            let revision =
                (!plugin.revision.is_empty() && plugin.revision != "local" && official.is_none())
                    .then(|| plugin.revision.clone());

            let fetched = match official {
                Some((id, None)) => index::source(id, Some(&plugin.version)),
                _ => plugin.source.clone(),
            };

            link_recorded(
                Source {
                    id: plugin.id.clone(),
                    source: Some(fetched),
                    revision,
                    version: None,
                    yes: args.force,
                },
                false,
                root,
                official.map(|_| plugin.source.as_str()),
            )?;

            let lock = load_lock_at(root)?;
            let entry = lock
                .plugins
                .get(&plugin.id)
                .ok_or_else(|| format!("Plugin {} was not recorded after import.", plugin.id))?;

            if entry.version != plugin.version || entry.capabilities != plugin.capabilities {
                return Err(
                    format!("Plugin {} metadata did not match the Krabfile.", plugin.id).into()
                );
            }
        }

        Ok(())
    })();

    drop(spinner);

    if let Err(error) = result {
        match previous_config {
            Some(text) => secure(&destination, text.as_bytes())?,

            None => {
                let _lock = metadata_lock(root)?;
                let _ = std::fs::remove_file(&destination);
            }
        }

        return Err(error);
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "action": "import",
                "path": path,
                "plugins": file.plugins.len(),
                "status": "imported"
            })
        );
    } else {
        let count = file.plugins.len();
        let noun = if count == 1 { "plugin" } else { "plugins" };

        println!(
            "Imported configuration from {} with {count} {noun}.\nLocal credentials were preserved.",
            path.display(),
        );
    }

    info!(plugins = file.plugins.len(), "Krabfile imported.");

    Ok(())
}

pub(crate) fn read_krabfile(
    path: &Path,
) -> Result<Krabfile, Box<dyn std::error::Error + Send + Sync>> {
    let source = std::fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!(
                "Krabfile not found at {}. Use --path <PATH> to choose a Krabfile.",
                path.display()
            )
        } else {
            format!("Could not read Krabfile at {}: {error}.", path.display())
        }
    })?;

    toml::from_str(&source).map_err(|error: toml::de::Error| {
        let location = error.span().map(|span| toml_position(&source, span.start));
        let location = location
            .map_or_else(String::new, |(line, column)| format!(" at line {line}, column {column}"));

        format!("Krabfile at {} is invalid{}: {}.", path.display(), location, error.message())
            .into()
    })
}

pub(crate) fn validate_krabfile_spec(
    file: &Krabfile,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    file.validate()
        .map_err(|error| format!("Krabfile at {} is invalid: {error}", path.display()).into())
}

pub(crate) fn default_krabfile_path() -> PathBuf {
    PathBuf::from("./Krabfile")
}

pub(crate) fn krabfile_output_path(
    destination: Option<PathBuf>,
    path: Option<PathBuf>,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let path = match (destination, path) {
        (Some(_), Some(_)) => {
            return Err("Choose either a positional export path or --path, not both.".into());
        }

        (Some(path), None) | (None, Some(path)) => path,
        (None, None) => default_krabfile_path(),
    };

    Ok(if path.is_dir() { path.join("Krabfile") } else { path })
}
