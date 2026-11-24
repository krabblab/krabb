//! Implements `krabb plugin update`: preview, prepare, commit, and roll back.

use crate::*;

pub(crate) async fn update_plugins(
    yes: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_plugins_at(&home(), yes, json).await
}

pub(crate) async fn update_plugins_at(
    root: &Path,
    yes: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !yes {
        let updates = preview_updates_at(root, json)?;

        if !updates || json || !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
            return Ok(());
        }

        if !confirm_action(false, false, "Apply these plugin updates?")? {
            discard_preview(root)?;
            print_cancelled("Update", false);

            return Ok(());
        }
    } else if !root.join(".plugin-update-preview/preview.json").is_file() {
        preview_updates_at_inner(root, json, false)?;
    }

    let spinner = terminal::progress("Preparing plugin updates...", json);
    let previews = load_update_preview_snapshot_at(root)?;
    let changed = previews.candidates.keys().cloned().collect::<Vec<_>>();

    if changed.is_empty() {
        drop(spinner);

        return update_at_with_previews(root, json, &previews.candidates, true);
    }

    let active = active_at(root).await?;

    drop(spinner);

    if active.is_none() {
        let _offline = offline_lock(root)?;

        return update_at_with_previews(root, json, &previews.candidates, true);
    }

    updates::apply(root, json, &previews).await
}

/// Removes a declined preview so a later `--yes` cannot apply it.
fn discard_preview(root: &Path) -> Result<(), PluginError> {
    let _lock = lock_at(root, ".plugins.lock")?;
    let preview = root.join(".plugin-update-preview");

    match std::fs::remove_file(preview.join("preview.json")) {
        Ok(()) => {}

        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    match std::fs::remove_dir_all(&preview) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn update_at(
    home_root: &Path,
    json: bool,
    preview_only: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if preview_only {
        let previews = load_update_previews_at(home_root)?;

        return update_at_with_previews(home_root, json, &previews, true);
    }

    update_at_with_previews(home_root, json, &BTreeMap::new(), false)
}

pub(crate) fn update_at_with_previews(
    home_root: &Path,
    json: bool,
    previews: &BTreeMap<String, UpdateCandidate>,
    preview_only: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_with_commit(home_root, json, previews, preview_only, save_lock_at)
}

pub(crate) fn update_with_commit(
    home_root: &Path,
    json: bool,
    previews: &BTreeMap<String, UpdateCandidate>,
    preview_only: bool,
    mut commit: impl FnMut(&Path, &Lock) -> Result<(), PluginError>,
) -> Result<(), PluginError> {
    let spinner = terminal::progress("Updating plugins...", json);
    let _lock = lock_at(home_root, ".plugins.lock")?;
    let mut lock = load_lock_at(home_root)?;
    let mut rows = Vec::new();
    let mut failed = false;
    let ids = lock.plugins.keys().cloned().collect::<Vec<_>>();

    for id in ids {
        let previous_lock = lock.clone();
        let previous = lock.plugins.get(&id).cloned().ok_or("Plugin lock entry disappeared.")?;
        let preview_candidate = previews.get(&id);

        if preview_only && preview_candidate.is_none() {
            continue;
        }

        if preview_candidate.is_none() && index::current(home_root, &previous) {
            rows.push(serde_json::json!({
                "id": id,
                "version": previous.version,
                "status": "unchanged"
            }));
            continue;
        }

        // Previews are hashed when saved or loaded, and staging hashes their copy again.
        let inspection = if let Some(candidate) = preview_candidate {
            check_update_candidate(home_root, &id, &previous, candidate).map(|()| candidate.clone())
        } else {
            let entry = lock.plugins.get(&id).ok_or("Plugin lock entry disappeared.")?;
            inspect_update(home_root, &id, entry)
        };

        let result = match inspection {
            Ok(candidate)
                if candidate.hash == previous.hash
                    && candidate.source_revision == previous.revision
                    && candidate.manifest.version == previous.version =>
            {
                rows.push(serde_json::json!({
                    "id": id,
                    "version": previous.version,
                    "status": "unchanged"
                }));
                continue;
            }

            Ok(_) => {
                let entry = lock.plugins.get(&id).ok_or("Plugin lock entry disappeared.")?;
                update_one(home_root, &id, entry, preview_candidate)
            }

            Err(error) => Err(error),
        };

        match result {
            Ok((manifest, hash, source_revision, update)) => {
                {
                    let entry =
                        lock.plugins.get_mut(&id).ok_or("Plugin lock entry disappeared.")?;

                    super::record(entry, &manifest, source_revision, hash, update.linked);
                }

                if let Err(error) = commit(home_root, &lock) {
                    if let Some(entry) = lock.plugins.get_mut(&id) {
                        *entry = previous.clone();
                    }

                    let restored_lock = commit(home_root, &previous_lock);
                    let restored_plugin = rollback_update(&update);
                    let detail = if let Err(restore) = restored_lock {
                        format!(" Rollback failed: {}.", sentence(restore.to_string()))
                    } else if let Err(restore) = restored_plugin {
                        format!(" Rollback failed: {}.", sentence(restore.to_string()))
                    } else {
                        String::new()
                    };

                    failed = true;
                    let rollback_failed = !detail.is_empty();

                    warn!(
                        plugin = %id,
                        error = %diagnostic(sentence(error.to_string())),
                        rollback_failed,
                        "Plugin update could not be committed."
                    );

                    rows.push(serde_json::json!({
                        "id": id,
                        "version": previous.version,
                        "status": "failed",
                        "error": format!("Could not save the plugin lock: {}{detail}", sentence(error.to_string()))
                    }));
                } else {
                    finish_update(&update);
                    info!(plugin = %id, version = %manifest.version, "Plugin updated.");
                    rows.push(super::updated(&id, &manifest.version));
                }
            }

            Err(error) => {
                failed = true;

                warn!(
                    plugin = %id,
                    error = %diagnostic(sentence(error.to_string())),
                    "Plugin update failed."
                );

                rows.push(serde_json::json!({
                    "id": id,
                    "version": previous.version,
                    "status": "failed",
                    "error": sentence(error.to_string())
                }));
            }
        }
    }

    drop(spinner);

    if !json {
        println!("\n{}", plugin_update_output(&rows));
        std::io::stdout().flush()?;
    }

    if failed {
        if !json {
            println!();
            std::io::stdout().flush()?;
        }

        return Err(plugin_update_error(&rows).into());
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "items": rows,
                "status": if rows.iter().any(|row| row["status"] == "updated") {
                    "updated"
                } else {
                    "no changes"
                },
            }))?
        );
    }

    Ok(())
}

pub(crate) fn preview_updates_at(
    home_root: &Path,
    json: bool,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    preview_updates_at_inner(home_root, json, true)
}

pub(crate) fn preview_updates_at_inner(
    home_root: &Path,
    json: bool,
    display: bool,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let spinner = terminal::progress("Checking for plugin updates...", json);
    let _lock = lock_at(home_root, ".plugins.lock")?;
    let lock = load_lock_at(home_root)?;
    let mut updates = Vec::new();
    let preview_root = home_root.join(".plugin-update-preview");

    if preview_root.exists() {
        std::fs::remove_dir_all(&preview_root)?;
    }

    std::fs::create_dir_all(&preview_root)?;
    std::fs::write(preview_root.join("preview.json"), b"{}")?;

    let mut errors = Vec::new();

    for (id, entry) in &lock.plugins {
        if index::current(home_root, entry) {
            continue;
        }

        let candidate = match inspect_update(home_root, id, entry) {
            Ok(candidate) => candidate,

            Err(error) => {
                warn!(plugin = %id, error = %diagnostic(sentence(error.to_string())), "Plugin update check failed.");
                errors.push(serde_json::json!({
                    "id": id,
                    "error": sentence(error.to_string())
                }));
                continue;
            }
        };

        let changed = candidate.hash != entry.hash
            || candidate.source_revision != entry.revision
            || candidate.manifest.version != entry.version;

        if changed {
            if let Err(error) = save_update_preview_at(home_root, id, entry, &candidate) {
                warn!(plugin = %id, error = %diagnostic(sentence(error.to_string())), "Plugin update preview failed.");

                errors.push(serde_json::json!({
                    "id": id,
                    "error": sentence(error.to_string())
                }));

                continue;
            }

            updates.push(serde_json::json!({
                "id": id,
                "current_version": entry.version,
                "available_version": candidate.manifest.version,
                "current_revision": entry.revision,
                "available_revision": candidate.source_revision,
                "contents_changed": candidate.hash != entry.hash,
                "declarations": updates::declarations(entry, &candidate.manifest),
            }));
        }
    }

    drop(spinner);

    if display && json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": if updates.is_empty() && !errors.is_empty() {
                    "inspection_failed"
                } else if updates.is_empty() {
                    "up_to_date"
                } else {
                    "updates_available"
                },

                "items": updates,
                "errors": errors,
                "apply_command": "krabb plugin update --yes",
            }))?
        );
    } else if display {
        let mut sections = Vec::new();

        if lock.plugins.is_empty() {
            sections.push("No installed plugins to update.".to_owned());
        } else if updates.is_empty() && errors.is_empty() {
            let count = lock.plugins.len();
            let noun = if count == 1 { "plugin is" } else { "plugins are" };

            sections.push(format!("All {count} installed {noun} up to date."));
        }

        if !updates.is_empty() {
            let mut section = vec!["Updates available:".to_owned()];

            for (index, update) in updates.iter().enumerate() {
                let id = update["id"].as_str().unwrap_or("unknown");
                let current = update["current_version"].as_str().unwrap_or("unknown");
                let available = update["available_version"].as_str().unwrap_or("unknown");

                let detail = if current == available {
                    " (plugin contents or source revision changed)"
                } else {
                    ""
                };

                section.push(format_tree_entry(
                    &format!("{id}@v{current} → v{available}{detail}"),
                    &updates::review(&update["declarations"]),
                    index + 1 == updates.len(),
                ));
            }

            section.push(String::new());
            section.push("Confirm at the prompt or run `krabb plugin update --yes`.".into());
            sections.push(section.join("\n"));
        }

        if !errors.is_empty() {
            let count = errors.len();
            let heading = if count == 1 { "Update check issue:" } else { "Update check issues:" };

            let mut section = vec![heading.to_owned()];

            for (index, error) in errors.iter().enumerate() {
                let id = error["id"].as_str().unwrap_or("unknown");

                if index > 0 {
                    section.push(String::new());
                }

                section.push(format_tree_entry(
                    &format!("{id} update check failed"),
                    &[diagnostic(error["error"].as_str().unwrap_or("Unknown update error."))],
                    index + 1 == errors.len(),
                ));
            }

            section.push(String::new());
            section.push(plugin_log_hint("update check issue"));
            sections.push(section.join("\n"));
        }

        println!("{}", sections.join("\n\n"));
    }

    Ok(!updates.is_empty())
}

pub(crate) fn load_update_previews_at(
    home_root: &Path,
) -> Result<BTreeMap<String, UpdateCandidate>, Box<dyn std::error::Error + Send + Sync>> {
    let _lock = lock_at(home_root, ".plugins.lock")?;
    let preview_root = home_root.join(".plugin-update-preview");

    if !preview_root.join("preview.json").is_file() {
        return Err(
            "No saved plugin update preview was found, so run `krabb plugin update` first.".into(),
        );
    }

    let lock = load_lock_at(home_root)?;
    let mut candidates = BTreeMap::new();

    for (id, entry) in &lock.plugins {
        if let Some(candidate) = load_update_preview_at(home_root, id, entry)? {
            candidates
                .insert(id.clone(), validate_update_candidate(home_root, id, entry, &candidate)?);
        }
    }

    Ok(candidates)
}

pub(crate) fn load_update_preview_snapshot_at(
    home_root: &Path,
) -> Result<UpdatePreviewSnapshot, Box<dyn std::error::Error + Send + Sync>> {
    let _lock = lock_at(home_root, ".plugins.lock")?;

    let preview_root = home_root.join(".plugin-update-preview");

    if !preview_root.join("preview.json").is_file() {
        return Err(
            "No saved plugin update preview was found, so run `krabb plugin update` first.".into(),
        );
    }

    let lock = load_lock_at(home_root)?;
    let mut candidates = BTreeMap::new();

    let snapshot_root = (0..)
        .find_map(|attempt| {
            let path = home_root.join(format!(
                ".plugin-update-apply-{}-{}-{attempt}",
                std::process::id(),
                now()
            ));

            match std::fs::create_dir(&path) {
                Ok(()) => Some(Ok(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .ok_or("Could not allocate a plugin update preview snapshot.")??;

    let mut snapshot = UpdatePreviewSnapshot { candidates: BTreeMap::new(), root: snapshot_root };

    for (id, entry) in &lock.plugins {
        let Some(candidate) = load_update_preview_at(home_root, id, entry)? else {
            continue;
        };

        check_update_candidate(home_root, id, entry, &candidate)?;

        let destination = snapshot.root.join(id);

        std::fs::create_dir_all(destination.join("bin"))?;

        std::fs::copy(
            candidate.source_path.join("krabb-plugin.toml"),
            destination.join("krabb-plugin.toml"),
        )?;

        let binary = destination.join("bin").join(plugin_name(id));

        copy_binary(&candidate.binary, &binary)?;

        let candidate = UpdateCandidate {
            manifest: candidate.manifest,
            hash: candidate.hash,
            source_revision: candidate.source_revision,
            base_hash: candidate.base_hash,
            base_revision: candidate.base_revision,
            base_version: candidate.base_version,
            source_path: destination,
            binary,
        };

        std::fs::write(
            candidate.source_path.join("preview.json"),
            serde_json::to_vec(&UpdatePreviewRecord {
                base_hash: candidate.base_hash.clone(),
                base_revision: candidate.base_revision.clone(),
                base_version: candidate.base_version.clone(),
                hash: candidate.hash.clone(),
                source_revision: candidate.source_revision.clone(),
            })?,
        )?;

        candidates.insert(id.clone(), validate_update_candidate(home_root, id, entry, &candidate)?);
    }

    snapshot.candidates = candidates;
    Ok(snapshot)
}

#[derive(Clone)]
pub(crate) struct UpdateCandidate {
    pub(crate) manifest: Manifest,
    pub(crate) hash: String,
    pub(crate) source_revision: String,
    pub(crate) base_hash: String,
    pub(crate) base_revision: String,
    pub(crate) base_version: String,
    pub(crate) source_path: PathBuf,
    pub(crate) binary: PathBuf,
}

pub(crate) struct UpdatePreviewSnapshot {
    pub(crate) candidates: BTreeMap<String, UpdateCandidate>,
    pub(crate) root: PathBuf,
}

impl Drop for UpdatePreviewSnapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct UpdatePreviewRecord {
    pub(crate) base_hash: String,
    pub(crate) base_revision: String,
    pub(crate) base_version: String,
    pub(crate) hash: String,
    pub(crate) source_revision: String,
}

pub(crate) fn inspect_update(
    home_root: &Path,
    id: &str,
    entry: &Entry,
) -> Result<UpdateCandidate, Box<dyn std::error::Error + Send + Sync>> {
    if !valid(id) {
        return Err(format!("Invalid plugin ID: {id}.").into());
    }

    let pin = entry.pinned.then_some(entry.revision.as_str());
    let source = resolve_at(home_root, &entry.source, pin)?;

    let manifest = read_manifest(&source.path)
        .ok_or_else(|| format!("Manifest not found in {}.", source.path.display()))?;

    manifest.validate().map_err(|error| format!("Invalid plugin manifest: {error}"))?;

    if manifest.id != id {
        return Err(format!("Manifest ID {} does not match {id}.", manifest.id).into());
    }

    if !Protocol::CURRENT.compatible(manifest.protocol) {
        return Err(format!("Plugin {id} requires unsupported protocol.").into());
    }

    validate_commands(home_root, id, &manifest.commands)?;
    super::state::check(home_root, Some(entry), &manifest)?;

    let name = plugin_name(id);

    let Some(binary) = plugin_binary(&source.path, id, entry.default) else {
        return Err(format!("Binary {name} was not found, so build the plugin first.").into());
    };

    Ok(UpdateCandidate {
        manifest,
        hash: digest(&source.path.join("krabb-plugin.toml"), &binary)?,
        source_revision: revision(&source.path),
        base_hash: entry.hash.clone(),
        base_revision: entry.revision.clone(),
        base_version: entry.version.clone(),
        source_path: source.path.clone(),
        binary,
    })
}

pub(crate) fn save_update_preview_at(
    home_root: &Path,
    id: &str,
    entry: &Entry,
    candidate: &UpdateCandidate,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = home_root.join(".plugin-update-preview");

    let stage = root.join(format!(".{id}.stage-{}", now()));
    let destination = root.join(id);

    std::fs::create_dir_all(stage.join("bin"))?;

    std::fs::copy(
        candidate.source_path.join("krabb-plugin.toml"),
        stage.join("krabb-plugin.toml"),
    )?;

    copy_binary(&candidate.binary, &stage.join("bin").join(plugin_name(id)))?;

    let record = UpdatePreviewRecord {
        base_hash: entry.hash.clone(),
        base_revision: entry.revision.clone(),
        base_version: entry.version.clone(),
        hash: candidate.hash.clone(),
        source_revision: candidate.source_revision.clone(),
    };

    std::fs::write(stage.join("preview.json"), serde_json::to_vec(&record)?)?;

    if destination.exists() {
        std::fs::remove_dir_all(&destination)?;
    }

    std::fs::rename(stage, destination)?;
    Ok(())
}

pub(crate) fn load_update_preview_at(
    home_root: &Path,
    id: &str,
    entry: &Entry,
) -> Result<Option<UpdateCandidate>, Box<dyn std::error::Error + Send + Sync>> {
    load_update_candidate_at(&home_root.join(".plugin-update-preview").join(id), id, entry)
}

pub(crate) fn load_update_candidate_at(
    root: &Path,
    id: &str,
    entry: &Entry,
) -> Result<Option<UpdateCandidate>, PluginError> {
    let record_path = root.join("preview.json");

    if !record_path.is_file() {
        return Ok(None);
    }

    let record: UpdatePreviewRecord = serde_json::from_slice(&std::fs::read(record_path)?)?;

    if record.base_hash != entry.hash
        || record.base_revision != entry.revision
        || record.base_version != entry.version
    {
        return Err(format!(
            "The saved update preview for {id} is stale, so preview updates again."
        )
        .into());
    }

    let binary = root.join("bin").join(plugin_name(id));

    let manifest = read_manifest(root)
        .ok_or_else(|| format!("Saved update preview for {id} has no manifest."))?;

    Ok(Some(UpdateCandidate {
        manifest,
        hash: record.hash,
        source_revision: record.source_revision,
        base_hash: record.base_hash,
        base_revision: record.base_revision,
        base_version: record.base_version,
        source_path: root.to_path_buf(),
        binary,
    }))
}

/// Checks a saved candidate's contents against its confirmed hash, then its declarations.
pub(crate) fn validate_update_candidate(
    home_root: &Path,
    id: &str,
    entry: &Entry,
    candidate: &UpdateCandidate,
) -> Result<UpdateCandidate, Box<dyn std::error::Error + Send + Sync>> {
    check_update_candidate(home_root, id, entry, candidate)?;

    if digest(&candidate.source_path.join("krabb-plugin.toml"), &candidate.binary)?
        != candidate.hash
    {
        return Err(
            format!("Saved update preview for {id} changed, so preview updates again.").into()
        );
    }

    Ok(candidate.clone())
}

/// Checks a saved candidate's declarations; its copy is hashed against the confirmed hash.
pub(crate) fn check_update_candidate(
    home_root: &Path,
    id: &str,
    entry: &Entry,
    candidate: &UpdateCandidate,
) -> Result<(), PluginError> {
    let manifest = &candidate.manifest;

    manifest.validate().map_err(|error| format!("Invalid plugin manifest: {error}"))?;

    if candidate.base_hash != entry.hash
        || candidate.base_revision != entry.revision
        || candidate.base_version != entry.version
    {
        return Err(format!(
            "The saved update preview for {id} is stale, so preview updates again."
        )
        .into());
    }

    if manifest.id != id || !Protocol::CURRENT.compatible(manifest.protocol) {
        return Err(format!("Saved update preview for {id} is invalid.").into());
    }

    validate_commands(home_root, id, &manifest.commands)?;

    if !candidate.binary.is_file() {
        return Err(
            format!("Saved update preview for {id} changed, so preview updates again.").into()
        );
    }

    Ok(())
}

pub(crate) fn update_one(
    home_root: &Path,
    id: &str,
    entry: &Entry,
    preview: Option<&UpdateCandidate>,
) -> Result<(Manifest, String, String, Update), PluginError> {
    let (manifest, hash, revision, stage) = prepare_update(home_root, id, entry, preview, true)?;

    let update = stage.promote()?;

    Ok((manifest, hash, revision, update))
}

pub(crate) fn prepare_update(
    home_root: &Path,
    id: &str,
    entry: &Entry,
    preview: Option<&UpdateCandidate>,
    check_health: bool,
) -> Result<(Manifest, String, String, updates::Prepared), Box<dyn std::error::Error + Send + Sync>>
{
    if !valid(id) {
        return Err(format!("Invalid plugin ID: {id}.").into());
    }

    let resolved = if preview.is_none() || entry.linked {
        let pin = entry.pinned.then_some(entry.revision.as_str());
        Some(resolve_at(home_root, &entry.source, pin)?)
    } else {
        None
    };

    let source_path = preview
        .map(|candidate| candidate.source_path.as_path())
        .or_else(|| resolved.as_ref().map(|source| source.path.as_path()))
        .ok_or("Plugin update source disappeared.")?;

    let manifest = preview
        .map(|candidate| candidate.manifest.clone())
        .or_else(|| read_manifest(source_path))
        .ok_or_else(|| format!("Manifest not found in {}.", source_path.display()))?;

    manifest.validate().map_err(|error| format!("Invalid plugin manifest: {error}"))?;

    if manifest.id != id {
        return Err(format!("Manifest ID {} does not match {id}.", manifest.id).into());
    }

    if !Protocol::CURRENT.compatible(manifest.protocol) {
        return Err(format!("Plugin {id} requires unsupported protocol.").into());
    }

    validate_commands(home_root, id, &manifest.commands)?;
    super::state::check(home_root, Some(entry), &manifest)?;

    let name = plugin_name(id);

    let linked_binary = if entry.linked {
        let source = resolved.as_ref().ok_or("Linked plugin source disappeared.")?;

        if source.temp.is_some() {
            return Err(format!("Linked plugin source is not stable: {id}.").into());
        }

        let binary = plugin_binary(&source.path, id, entry.default).ok_or_else(|| {
            format!("Binary {} was not found, so build the plugin first.", plugin_name(id))
        })?;

        let matches_preview = match preview {
            Some(candidate) => {
                digest(&source.path.join("krabb-plugin.toml"), &binary)? == candidate.hash
            }

            None => true,
        };

        matches_preview.then_some(binary)
    } else {
        None
    };

    let linked = linked_binary.is_some();

    let binary = linked_binary
        .or_else(|| preview.map(|candidate| candidate.binary.clone()))
        .or_else(|| plugin_binary(source_path, id, entry.default));

    let Some(binary) = binary else {
        return Err(format!("Binary {name} was not found, so build the plugin first.").into());
    };

    let source_revision =
        preview.map_or_else(|| revision(source_path), |item| item.source_revision.clone());

    let preview_hash = preview.map(|item| item.hash.as_str());

    let plugins = home_root.join("plugins");

    std::fs::create_dir_all(&plugins)?;

    let nonce = format!("{}-{}", std::process::id(), now());
    let stage = plugins.join(format!(".{id}.stage-{nonce}"));
    let backup = plugins.join(format!(".{id}.backup-{nonce}"));
    let destination = plugins.join(id);

    if std::fs::canonicalize(&destination).is_ok_and(|path| path == source_path) {
        return Err("A plugin source cannot be its managed destination.".into());
    }

    let had_destination = destination.exists();

    let result =
        (|| -> Result<(String, updates::Prepared), Box<dyn std::error::Error + Send + Sync>> {
            std::fs::create_dir_all(stage.join("bin"))?;
            std::fs::copy(source_path.join("krabb-plugin.toml"), stage.join("krabb-plugin.toml"))?;

            let staged = stage.join("bin").join(&name);

            if linked {
                link_binary(&binary, &staged)?;
            } else {
                copy_binary(&binary, &staged)?;
            }

            let hash = digest(&stage.join("krabb-plugin.toml"), &staged)?;

            if preview_hash.is_some_and(|expected| expected != hash) {
                return Err("The saved plugin update candidate changed while staging.".into());
            }

            let config = load_config_at(home_root)?;

            config
                .validate()
                .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;

            if check_health {
                health(home_root, staged.clone(), id, &config)?;
            }

            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((
                hash,
                updates::Prepared {
                    stage: stage.clone(),
                    destination: destination.clone(),
                    backup: had_destination.then_some(backup.clone()),
                    linked,
                },
            ))
        })();

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&stage);
    }

    let (hash, update) = result?;

    Ok((manifest, hash, source_revision, update))
}

pub(crate) fn finish_update(update: &Update) {
    if let Some(backup) = &update.backup {
        let _ = std::fs::remove_dir_all(backup);
    }
}

pub(crate) fn rollback_update(update: &Update) -> std::io::Result<()> {
    if update.destination.exists() {
        std::fs::remove_dir_all(&update.destination)?;
    }

    if let Some(backup) = &update.backup {
        std::fs::rename(backup, &update.destination)?;
    }

    Ok(())
}
