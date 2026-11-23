//! Revokes an uninstalled plugin's saved secrets and sharing grants.

use super::*;

#[derive(Debug, Default, PartialEq, Serialize)]
pub(super) struct Revoked {
    pub(super) secrets: Vec<String>,
    pub(super) keyring: bool,
    pub(super) grants: usize,
}

/// Removes grants first, then secrets; every step is idempotent so a failed uninstall can retry.
pub(super) fn revoke(root: &Path, id: &str, lock: &Lock) -> Result<Revoked, PluginError> {
    let mut revoked = Revoked::default();

    revoked.grants = grants(root, id).map_err(|error| failure(id, &revoked, error))?;

    let names = secrets(lock, id);

    revoked.secrets = credentials::forget(&setup::path(root), &credentials::key_path(root), &names)
        .map_err(|error| failure(id, &revoked, error))?;

    revoked.keyring =
        credentials::forget_entry(id).map_err(|error| failure(id, &revoked, error))?;

    Ok(revoked)
}

fn failure(id: &str, revoked: &Revoked, error: PluginError) -> PluginError {
    let done = if revoked.grants > 0 {
        format!("\nRemoved {} sharing grant(s) already.", revoked.grants)
    } else {
        String::new()
    };

    format!(
        "Could not revoke the saved access of {id}: {}{done}\nThe plugin is still installed, so \
         retry the uninstall.",
        sentence(error.to_string())
    )
    .into()
}

/// Names the plugin declares that no other installed plugin also declares.
fn secrets(lock: &Lock, id: &str) -> Vec<String> {
    let declared = |entry: &Entry| {
        entry
            .variables
            .iter()
            .cloned()
            .chain(entry.settings.iter().map(|setting| setting.name.clone()))
            .collect::<BTreeSet<_>>()
    };

    let Some(entry) = lock.plugins.get(id) else {
        return Vec::new();
    };

    let shared = lock
        .plugins
        .iter()
        .filter(|(other, _)| other.as_str() != id)
        .flat_map(|(_, entry)| declared(entry))
        .collect::<BTreeSet<_>>();

    declared(entry).difference(&shared).cloned().collect()
}

fn grants(root: &Path, id: &str) -> Result<usize, PluginError> {
    let Some(text) = load_config_text(root)? else {
        return Ok(0);
    };

    let mut document = text.parse::<toml_edit::DocumentMut>()?;
    let removed = unshare(&mut document, id);

    if removed == 0 {
        return Ok(0);
    }

    let rendered = document.to_string();
    let config: Config = toml::from_str(&rendered)?;

    config.validate().map_err(|error| -> PluginError { error.into() })?;
    secure(root.join("config.toml"), rendered.as_bytes())?;
    info!(plugin = id, grants = removed, "Plugin sharing grants revoked.");

    Ok(removed)
}

/// Removes a plugin's grants from a configuration restored from a recovery point.
///
/// It skips schema validation: the restored file may predate the running binary, and removing
/// grants cannot make it invalid, so validating could only block the rollback.
pub(super) fn restored(root: &Path, id: &str) -> Result<usize, PluginError> {
    let Some(text) = load_config_text(root)? else {
        return Ok(0);
    };

    let mut document = text.parse::<toml_edit::DocumentMut>()?;
    let removed = unshare(&mut document, id);

    if removed > 0 {
        secure(root.join("config.toml"), document.to_string().as_bytes())?;
        info!(plugin = id, grants = removed, "Restored plugin sharing grants revoked again.");
    }

    Ok(removed)
}

fn unshare(document: &mut toml_edit::DocumentMut, id: &str) -> usize {
    let principals = [format!("channels.{id}"), format!("clients.{id}")];
    let mut removed = 0;

    for kind in ["channels", "clients"] {
        let Some(table) = document.get_mut(kind).and_then(toml_edit::Item::as_table_like_mut)
        else {
            continue;
        };

        if let Some(own) = table.get_mut(id).and_then(toml_edit::Item::as_table_like_mut)
            && let Some(rules) = own.get_mut("visibility")
            && count(rules) > 0
        {
            removed += count(rules);
            *rules = toml_edit::value(toml_edit::Array::new());
        }

        for (_, entry) in table.iter_mut() {
            if let Some(local) = entry.as_table_like_mut()
                && let Some(rules) = local.get_mut("visibility")
            {
                removed += prune(rules, &principals, true);
            }
        }
    }

    if let Some(rules) = document.get_mut("visibility") {
        removed += prune(rules, &principals, false);
    }

    removed
}

fn count(rules: &toml_edit::Item) -> usize {
    match rules {
        toml_edit::Item::ArrayOfTables(tables) => tables.len(),
        toml_edit::Item::Value(toml_edit::Value::Array(array)) => array.len(),
        _ => 0,
    }
}

/// Drops `principals` from each rule's resources, then rules left without resources.
///
/// A local list emptied this way stays an explicit `[]`, so it never starts inheriting.
fn prune(rules: &mut toml_edit::Item, principals: &[String], local: bool) -> usize {
    let strip = |resources: Option<&mut toml_edit::Array>| -> (usize, bool) {
        let Some(array) = resources else {
            return (0, false);
        };

        let before = array.len();

        array.retain(|value| {
            value.as_str().is_none_or(|name| !principals.iter().any(|p| p == name))
        });

        (before - array.len(), array.is_empty())
    };

    let mut removed = 0;

    match rules {
        toml_edit::Item::ArrayOfTables(tables) => {
            let mut empty = Vec::new();

            for (index, table) in tables.iter_mut().enumerate() {
                let (count, now_empty) =
                    strip(table.get_mut("resources").and_then(toml_edit::Item::as_array_mut));

                removed += count;

                if now_empty {
                    empty.push(index);
                }
            }

            for index in empty.into_iter().rev() {
                tables.remove(index);
            }

            if local && tables.is_empty() {
                *rules = toml_edit::value(toml_edit::Array::new());
            }
        }

        toml_edit::Item::Value(toml_edit::Value::Array(array)) => {
            let mut kept = toml_edit::Array::new();

            for mut value in array.iter().cloned() {
                if let Some(table) = value.as_inline_table_mut() {
                    let resources =
                        table.get_mut("resources").and_then(toml_edit::Value::as_array_mut);
                    let (count, now_empty) = strip(resources);

                    removed += count;

                    if now_empty {
                        continue;
                    }
                }

                kept.push_formatted(value);
            }

            *array = kept;
        }

        _ => {}
    }

    removed
}

#[cfg(test)]
mod tests;
