//! `krabb plugin use`, and the selection that installs and removals keep.

use std::path::Path;

use krabb_core::{Capability, Composition};
use serde_json::json;
use tracing::{info, warn};

use super::{Choice, Table, describe, installed, validate, validate_installed};
use crate::{Config, PluginError, control_at, load_config_text, load_lock_at, secure};

pub(crate) const RELOAD: &str = "config.plugins.reload";

/// The capability a `--for` value names, which must have a primary.
fn primary_capability(name: &str) -> Result<Capability, PluginError> {
    let capability = Capability::parse(name).ok_or_else(|| {
        format!("Unknown capability {name}.\nList capabilities with `krabb plugin list`.")
    })?;

    match capability.composition() {
        Composition::Primary => Ok(capability),

        composition => Err(format!(
            "{name} plugins {}, so there is no primary to select.",
            describe(composition)
        )
        .into()),
    }
}

/// `krabb plugin use <id> --for <capability> [--also]`.
pub(crate) async fn run(
    root: &Path,
    id: &str,
    name: &str,
    also: bool,
    json: bool,
) -> Result<(), PluginError> {
    let capability = primary_capability(name)?;

    let lock = load_lock_at(root)?;

    let entry = lock.plugins.get(id).ok_or_else(|| {
        format!("Plugin {id} is not installed.\nInstall it with `krabb plugin install {id}`.")
    })?;

    if !entry.capabilities.iter().any(|declared| declared == name) {
        return Err(format!("Plugin {id} does not provide {name}.").into());
    }

    let text = load_config_text(root)?;
    let config: Config = text.as_deref().map(toml::from_str).transpose()?.unwrap_or_default();
    let mut table = config.plugins.clone();
    let previous = installed(&capability, &table, &lock).primary.map(str::to_owned);
    let choice = table.entry(name.to_owned()).or_default();

    let message = if also {
        if !capability.additional() {
            return Err(format!(
                "{name} has one provider, so it takes no additional plugins.\nSelect {id} with \
                 `krabb plugin use {id} --for {name}`."
            )
            .into());
        }

        if previous.as_deref() == Some(id) {
            return Err(format!("Plugin {id} is already the primary {name} plugin.").into());
        }

        if choice.also.iter().any(|member| member == id) {
            return report(json, id, name, "unchanged", &format!("{id} already serves {name}."));
        }

        if choice.primary.is_none() {
            choice.primary.clone_from(&previous);
        }

        choice.also.push(id.to_owned());

        format!("{id} now adds to {name}; writes stay with the primary.")
    } else {
        if previous.as_deref() == Some(id) {
            return report(json, id, name, "unchanged", &format!("{name} already uses {id}."));
        }

        if let Some(previous) = &previous {
            switchable(&capability, previous)?;
        }

        choice.primary = Some(id.to_owned());
        choice.also.retain(|member| member != id);

        match &previous {
            Some(previous) if capability.additional() => {
                if !choice.also.contains(previous) {
                    choice.also.push(previous.clone());
                }

                format!(
                    "{name} now uses {id}.\n{previous} stays as an additional plugin, so its data \
                     stays available; new writes go to {id}.\nUndo with `krabb plugin use \
                     {previous} --for {name}`."
                )
            }

            Some(previous) => format!(
                "{name} now uses {id}.\nUndo with `krabb plugin use {previous} --for {name}`."
            ),

            None => format!("{name} now uses {id}."),
        }
    };

    validate(&table)?;
    validate_installed(&table, &lock)?;

    let applied = save(root, text.as_deref(), &table).await?;

    info!(plugin = id, capability = name, also, "Plugin selection saved.");

    let message =
        if applied { message } else { format!("{message}\nThe daemon applies it when it starts.") };

    report(json, id, name, if also { "added" } else { "selected" }, &message)
}

/// Data that a capability cannot carry to another plugin keeps the current one in place.
fn switchable(capability: &Capability, previous: &str) -> Result<(), PluginError> {
    if *capability == Capability::Store {
        return Err(format!(
            "{previous} holds Krabb data that store plugins cannot export, so the store stays \
             with {previous}.\nMove the data out, then remove it with `krabb plugin remove \
             {previous}`."
        )
        .into());
    }

    Ok(())
}

fn report(
    json: bool,
    id: &str,
    name: &str,
    status: &str,
    message: &str,
) -> Result<(), PluginError> {
    if json {
        let value = json!({"id": id, "capability": name, "status": status, "message": message});

        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{message}");
    }

    Ok(())
}

/// Keeps the current primary when a newly installed plugin provides the same capability, and
/// returns how to select the new one.
pub(crate) async fn hold(root: &Path, id: &str) -> Result<Vec<String>, PluginError> {
    let lock = load_lock_at(root)?;

    let Some(entry) = lock.plugins.get(id) else {
        return Ok(Vec::new());
    };

    let text = load_config_text(root)?;
    let config: Config = text.as_deref().map(toml::from_str).transpose()?.unwrap_or_default();
    let mut table = config.plugins.clone();
    let mut notices = Vec::new();

    for name in &entry.capabilities {
        let Some(capability) = Capability::parse(name) else {
            continue;
        };

        if capability.composition() != Composition::Primary {
            continue;
        }

        let others = lock
            .plugins
            .iter()
            .filter(|(other, entry)| {
                *other != id && entry.capabilities.iter().any(|declared| declared == name)
            })
            .map(|(other, _)| other.as_str())
            .collect::<Vec<_>>();

        let Some(first) = others.first() else {
            continue;
        };

        let choice = table.entry(name.clone()).or_default();

        if choice.primary.as_deref() == Some(id) || choice.also.iter().any(|member| member == id) {
            continue;
        }

        let current = choice.primary.get_or_insert_with(|| (*first).to_owned()).clone();

        let also = if capability.additional() {
            format!(", or add it with `krabb plugin use {id} --for {name} --also`")
        } else {
            String::new()
        };

        notices.push(format!(
            "{name} stays with {current}. Select {id} with `krabb plugin use {id} --for \
             {name}`{also}."
        ));
    }

    if table != config.plugins {
        save(root, text.as_deref(), &table).await?;
    }

    Ok(notices)
}

/// Drops a removed plugin from every selection.
pub(crate) async fn forget(root: &Path, id: &str) -> Result<(), PluginError> {
    let text = load_config_text(root)?;
    let config: Config = text.as_deref().map(toml::from_str).transpose()?.unwrap_or_default();
    let mut table = config.plugins.clone();

    for choice in table.values_mut() {
        if choice.primary.as_deref() == Some(id) {
            choice.primary = None;
        }

        choice.also.retain(|member| member != id);
    }

    table.retain(|_, choice| *choice != Choice::default());

    if table != config.plugins {
        save(root, text.as_deref(), &table).await?;
    }

    Ok(())
}

/// Writes `[plugins]` without disturbing the rest of the file, then applies it to a running
/// daemon. A daemon that refuses it gets the previous file back.
async fn save(root: &Path, text: Option<&str>, table: &Table) -> Result<bool, PluginError> {
    let rendered = render(text.unwrap_or_default(), table)?;
    let config: Config = toml::from_str(&rendered)?;

    config.validate()?;

    let path = root.join("config.toml");

    std::fs::create_dir_all(root)?;
    secure(&path, rendered.as_bytes())?;

    match control_at(RELOAD, json!({}), root).await {
        Ok(applied) => Ok(applied.is_some()),

        Err(error) => {
            warn!(error = %crate::diagnostic(crate::sentence(error.to_string())), "Plugin selection was refused, restoring the previous configuration.");

            match text {
                Some(text) => secure(&path, text.as_bytes())?,
                None => std::fs::remove_file(&path)?,
            }

            Err(format!(
                "The daemon could not apply the selection, so the previous one stays: {}",
                crate::sentence(error.to_string())
            )
            .into())
        }
    }
}

fn render(text: &str, table: &Table) -> Result<String, PluginError> {
    let mut document = text.parse::<toml_edit::DocumentMut>()?;

    if table.is_empty() {
        document.remove("plugins");
        return Ok(document.to_string());
    }

    let mut plugins = toml_edit::Table::new();

    plugins.set_implicit(true);

    for (name, choice) in table {
        let mut section = toml_edit::Table::new();

        if let Some(primary) = &choice.primary {
            section["primary"] = toml_edit::value(primary.as_str());
        }

        if !choice.also.is_empty() {
            section["also"] = toml_edit::value(choice.also.iter().collect::<toml_edit::Array>());
        }

        plugins[name.as_str()] = toml_edit::Item::Table(section);
    }

    document["plugins"] = toml_edit::Item::Table(plugins);

    Ok(document.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(primary: Option<&str>, also: &[&str]) -> Choice {
        Choice {
            primary: primary.map(str::to_owned),
            also: also.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    #[test]
    fn rendering_keeps_the_rest_of_the_configuration() {
        let table = Table::from([("memory".into(), choice(Some("zep"), &["memory"]))]);
        let text = render("# mine\nname = \"Krabb\"\n", &table).unwrap();

        assert!(text.starts_with("# mine\nname = \"Krabb\"\n"), "{text}");
        assert!(
            text.contains("[plugins.memory]\nprimary = \"zep\"\nalso = [\"memory\"]"),
            "{text}"
        );

        let config: Config = toml::from_str(&text).unwrap();

        assert_eq!(config.plugins, table);
        assert_eq!(render(&text, &Table::new()).unwrap(), "# mine\nname = \"Krabb\"\n");
    }

    #[test]
    fn only_primary_capabilities_can_be_selected() {
        assert_eq!(primary_capability("memory").unwrap(), Capability::Memory);
        assert!(primary_capability("tool").unwrap_err().to_string().contains("all combine"));
        assert!(primary_capability("vision").unwrap_err().to_string().contains("model selection"));
        assert!(primary_capability("classifier").unwrap_err().to_string().contains("Unknown"));
    }

    #[test]
    fn a_store_with_data_is_never_switched_away() {
        let error = switchable(&Capability::Store, "sqlite").unwrap_err().to_string();

        assert!(error.contains("cannot export"), "{error}");
        assert_eq!(error.lines().count(), 2);
        assert!(switchable(&Capability::Memory, "memory").is_ok());
        assert!(switchable(&Capability::Speech, "whisper").is_ok());
    }
}
