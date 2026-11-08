//! Prepare replacements while installed processes serve, then commit at a turn boundary.
mod image;
mod review;
pub(super) use image::Image;
pub(super) use review::{declarations, review};

use super::*;

pub(super) struct Prepared {
    pub stage: PathBuf,
    pub destination: PathBuf,
    pub backup: Option<PathBuf>,
    pub linked: bool,
}

impl Prepared {
    pub fn promote(&self) -> Result<Update, PluginError> {
        if let Some(backup) = &self.backup {
            std::fs::rename(&self.destination, backup)?;
        }

        if let Err(error) = std::fs::rename(&self.stage, &self.destination) {
            if let Some(backup) = &self.backup {
                std::fs::rename(backup, &self.destination)?;
            }

            return Err(error.into());
        }

        Ok(Update {
            destination: self.destination.clone(),
            backup: self.backup.clone(),
            linked: self.linked,
        })
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.stage);
    }
}

pub(super) async fn apply(
    root: &Path,
    json: bool,
    snapshot: &UpdatePreviewSnapshot,
) -> Result<(), PluginError> {
    let snapshot_name = snapshot
        .root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("Invalid plugin update snapshot.")?;
    let mut rows = Vec::new();

    for (id, candidate) in &snapshot.candidates {
        let _progress = terminal::progress(
            format!(
                "Updating {id}: validating replacement and waiting for active work; current plugin stays loaded..."
            ),
            json,
        );
        let result = ipc::call(
            root,
            "plugin.update",
            serde_json::json!({
                "id": id, "snapshot": snapshot_name, "hash": candidate.hash,
            }),
        )
        .await;

        match result {
            Ok(value) => rows.push(value),
            Err(error) => rows.push(serde_json::json!({
                "id": id, "version": candidate.base_version, "status": "failed",
                "error": if error.to_string() == "Method not found." {
                    "The running daemon does not support live updates; upgrade it and run `krabb service restart` once, then retry.".into()
                } else { diagnostic(error.to_string()) },
            })),
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"items": rows, "status": if rows.iter().any(|row| row["status"] == "failed") { "failed" } else { "updated" }})
            )?
        );
    } else {
        println!("{}", plugin_update_output(&rows));
    }

    if rows.iter().any(|row| row["status"] == "failed") {
        return Err(plugin_update_error(&rows).into());
    }

    Ok(())
}

pub(super) async fn replace(
    root: &Path,
    plugins: &Plugins,
    id: &str,
    snapshot: &str,
    hash: &str,
) -> Result<serde_json::Value, PluginError> {
    replace_with_commit(root, plugins, id, snapshot, hash, save_lock_at).await
}

async fn replace_with_commit(
    root: &Path,
    plugins: &Plugins,
    id: &str,
    snapshot: &str,
    hash: &str,
    mut commit: impl FnMut(&Path, &Lock) -> Result<(), PluginError>,
) -> Result<serde_json::Value, PluginError> {
    if !valid(id)
        || !snapshot.starts_with(".plugin-update-apply-")
        || !snapshot.chars().all(|value| value.is_ascii_alphanumeric() || "-_.".contains(value))
        || snapshot.contains("..")
    {
        return Err("Invalid plugin update snapshot or ID.".into());
    }

    let root = root.to_path_buf();
    let stage_root = root.join(snapshot).join(id);
    let id = id.to_owned();
    let expected_hash = hash.to_owned();
    let prepare_root = root.clone();
    let prepare_id = id.clone();
    let (guard, previous, mut lock, manifest, stage, hash, revision) =
        tokio::task::spawn_blocking(move || -> Result<_, PluginError> {
            let guard = lock_at(&prepare_root, ".plugins.lock")?;
            let lock = load_lock_at(&prepare_root)?;
            let entry = lock.plugins.get(&prepare_id).ok_or("Plugin is no longer installed.")?;
            let directory = stage_root.canonicalize()?;

            if !directory.starts_with(prepare_root.canonicalize()?) {
                return Err("Plugin update snapshot escapes the Krabb home.".into());
            }

            let candidate = load_update_candidate_at(&directory, &prepare_id, entry)?
                .ok_or("Plugin update snapshot disappeared; preview updates again.")?;
            let candidate =
                validate_update_candidate(&prepare_root, &prepare_id, entry, &candidate)?;

            if candidate.hash != expected_hash {
                return Err("Plugin update snapshot changed after confirmation.".into());
            }

            let (manifest, hash, revision, stage) =
                prepare_update(&prepare_root, &prepare_id, entry, Some(&candidate), false)?;
            Ok((guard, entry.clone(), lock, manifest, stage, hash, revision))
        })
        .await??;

    // The candidate has its own executable image. Its restart path survives promotion.
    let mut candidate = Some(
        launch(
            &root,
            stage.stage.join("bin").join(plugin_name(&id)),
            &id,
            None,
            true,
            &plugins.config,
        )
        .await?,
    );

    let result = async {
        if digest(&stage.stage.join("krabb-plugin.toml"),
            &candidate.as_ref().unwrap().image.as_ref().unwrap().binary)? != hash {
            return Err("Plugin changed after confirmation; preview updates again.".into());
        }

        // Readers already serving turns finish normally; new turns queue briefly for the swap.
        let _boundary = tokio::time::timeout(
            Duration::from_secs(310), plugins.lifecycle.write(),
        ).await.map_err(|_| "Plugin update deferred: active turns are still running; the old plugin remains available.")?;
        let _loading = plugins.loading.lock().await;
        let active = plugins.get(&id).await.is_some();
        let update = stage.promote()?;
        let mut entry = previous.clone();
        entry.version = manifest.version.clone();
        entry.revision = revision;
        entry.hash = hash;
        entry.linked = update.linked;
        entry.protocol = manifest.protocol;
        entry.capabilities = manifest.capabilities.clone();
        entry.permissions = manifest.permissions.clone();
        entry.variables = manifest.variables.clone();
        entry.settings = manifest.settings.clone();
        entry.commands = manifest.commands.clone();
        lock.plugins.insert(id.clone(), entry);

        if let Err(error) = commit(&root, &lock) {
            lock.plugins.insert(id.clone(), previous);
            let restored_lock = commit(&root, &lock);
            let restored_files = rollback_update(&update);
            restored_lock?;
            restored_files?;
            return Err(error);
        }

        let old = if active {
            plugins.insert(Live::new(candidate.take().unwrap())).await
        } else {
            None
        };

        finish_update(&update);
        Ok::<_, PluginError>(old)
    }.await;

    drop(guard);

    if let Some(candidate) = candidate {
        let _ = candidate.stop().await;
    }

    if let Some(old) = result?
        && let Err(error) = old.stop().await
    {
        warn!(plugin = %id, error = %diagnostic(error.to_string()),
            "Replaced plugin cleanup failed; the replacement remains active.");
    }

    info!(plugin = %id, version = %manifest.version, "Plugin updated without unloading its registry slot.");
    Ok(serde_json::json!({"id": id, "version": manifest.version, "status": "updated"}))
}

#[cfg(test)]
mod tests;
