//! Prepare replacements while installed processes serve, then switch each plugin over
//! between its requests without pausing other plugins.
pub(crate) mod command;
mod image;
mod review;
pub(crate) mod state;
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
                "Updating {id}: switching over after its current request, and everything else keeps running..."
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
                    "The running daemon does not support live updates, so upgrade it and run `krabb service restart` once, then retry.".into()
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

/// A validated replacement staged beside the installed plugin, ready to promote.
struct Replacement {
    previous: Entry,
    lock: Lock,
    manifest: Manifest,
    stage: Prepared,
    hash: String,
    revision: String,
}

impl Replacement {
    /// Moves the staged files into place and records them, restoring both if the record fails.
    fn promote(
        &mut self,
        root: &Path,
        id: &str,
        commit: &mut impl FnMut(&Path, &Lock) -> Result<(), PluginError>,
    ) -> Result<(), PluginError> {
        let update = self.stage.promote()?;
        let mut entry = self.previous.clone();

        record(&mut entry, &self.manifest, self.revision.clone(), self.hash.clone(), update.linked);
        self.lock.plugins.insert(id.to_owned(), entry);

        if let Err(error) = commit(root, &self.lock) {
            self.lock.plugins.insert(id.to_owned(), self.previous.clone());

            let restored_lock = commit(root, &self.lock);
            let restored_files = rollback_update(&update);

            restored_lock?;
            restored_files?;
            return Err(error);
        }

        finish_update(&update);
        Ok(())
    }
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

    let (guard, mut replacement) =
        tokio::task::spawn_blocking(move || -> Result<_, PluginError> {
            let guard = lock_at(&prepare_root, ".plugins.lock")?;
            let lock = load_lock_at(&prepare_root)?;
            let entry = lock.plugins.get(&prepare_id).ok_or("Plugin is no longer installed.")?;
            let directory = stage_root.canonicalize()?;

            if !directory.starts_with(prepare_root.canonicalize()?) {
                return Err("Plugin update snapshot escapes the Krabb home.".into());
            }

            let candidate = load_update_candidate_at(&directory, &prepare_id, entry)?
                .ok_or("Plugin update snapshot disappeared, so preview updates again.")?;

            // Staging hashes its own copy against this confirmed hash.
            check_update_candidate(&prepare_root, &prepare_id, entry, &candidate)?;

            if candidate.hash != expected_hash {
                return Err("Plugin update snapshot changed after confirmation.".into());
            }

            let (manifest, hash, revision, stage) =
                prepare_update(&prepare_root, &prepare_id, entry, Some(&candidate), false)?;
            let previous = entry.clone();

            Ok((guard, Replacement { previous, lock, manifest, stage, hash, revision }))
        })
        .await??;

    let binary = replacement.stage.stage.join("bin").join(plugin_name(&id));

    // The private image is copied and hashed while the current version still serves.
    let verified = verify_checked(&root, binary, &id, &plugins.config, &replacement.hash)?;

    let Some(live) = plugins.get(&id).await else {
        let _loading = plugins.loading.lock().await;

        if plugins.get(&id).await.is_some() {
            return Err(superseded(&id));
        }

        // Nothing runs this plugin, so its replacement only has to start before promotion.
        let candidate = verified.start().await?;
        let promoted = replacement.promote(&root, &id, &mut commit);
        let _ = candidate.stop().await;

        drop(guard);
        promoted?;

        info!(plugin = %id, version = %replacement.manifest.version, "Plugin updated.");
        return Ok(updated(&id, &replacement.manifest.version));
    };

    let deadline = tokio::time::Instant::now() + HANDOFF_WAIT;
    let deferred = |_| {
        PluginError::from(format!(
            "Plugin update deferred: {id} is still handling a request, and it keeps running."
        ))
    };

    // The request in flight finishes on the current version; later ones wait for the switch.
    // Unloads hold `loading` while they wait for the slot, so the slot never waits on it.
    let (mut slot, _loading) = loop {
        let slot =
            tokio::time::timeout_at(deadline, live.process.lock()).await.map_err(deferred)?;

        if let Ok(loading) = plugins.loading.try_lock() {
            break (slot, loading);
        }

        drop(slot);
        drop(tokio::time::timeout_at(deadline, plugins.loading.lock()).await.map_err(deferred)?);
    };

    // A reload or unload during the wait wins over this update.
    if !plugins.get(&id).await.is_some_and(|current| Arc::ptr_eq(&current.process, &live.process)) {
        return Err(superseded(&id));
    }

    // Old and new never run together: a channel's inbox, socket or port has a single owner.
    if let Some(current) = slot.as_mut()
        && let Err(error) = current.halt().await
    {
        let error = format!(
            "Plugin update stopped: {id} could not shut down cleanly ({}).",
            sentence(error.to_string()).trim_end_matches('.')
        );

        return Err(restore(plugins, &mut slot, &live, &id, error.into()).await);
    }

    let mut candidate = match verified.start().await {
        Ok(candidate) => candidate,

        Err(error) => {
            return Err(restore(plugins, &mut slot, &live, &id, error).await);
        }
    };

    // Saved provider continuations belong to the conversations, so turns under way keep them.
    if let Some(current) = slot.as_mut() {
        candidate.context = std::mem::take(&mut current.context);
    }

    let hello = candidate.hello.clone();
    let approvals = candidate.approvals;
    let previous = slot.replace(candidate);

    if let Err(error) = replacement.promote(&root, &id, &mut commit) {
        if let Some(mut candidate) = std::mem::replace(&mut *slot, previous) {
            if let Some(current) = slot.as_mut() {
                current.context = std::mem::take(&mut candidate.context);
            }

            let _ = candidate.stop().await;
        }

        return Err(restore(plugins, &mut slot, &live, &id, error).await);
    }

    live.restart_generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let successor = live.successor(hello, approvals);

    drop(slot);
    plugins.insert(successor).await;
    drop(guard);

    if let Some(previous) = previous
        && let Err(error) = previous.stop().await
    {
        warn!(plugin = %id, error = %diagnostic(error.to_string()), "Plugin cleanup failed.");
    }

    info!(plugin = %id, version = %replacement.manifest.version, "Plugin switched over live.");
    Ok(updated(&id, &replacement.manifest.version))
}

/// The longest a replacement waits for the request in flight, matching the turn limit.
const HANDOFF_WAIT: Duration = Duration::from_secs(310);

fn superseded(id: &str) -> PluginError {
    format!("Plugin {id} was reloaded or removed during the update, so preview again.").into()
}

/// Records a promoted manifest's reviewed declarations in its lock entry.
fn record(entry: &mut Entry, manifest: &Manifest, revision: String, hash: String, linked: bool) {
    entry.version = manifest.version.clone();
    entry.revision = revision;
    entry.hash = hash;
    entry.linked = linked;
    entry.protocol = manifest.protocol;
    entry.capabilities = manifest.capabilities.clone();
    entry.permissions = manifest.permissions.clone();
    entry.variables = manifest.variables.clone();
    entry.settings = manifest.settings.clone();
    entry.commands = manifest.commands.clone();
    entry.state = manifest.state.clone();
}

fn updated(id: &str, version: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "version": version, "status": "updated"})
}

/// Restarts the halted version after a failed replacement, from the image it was running;
/// if it cannot restart, it is unloaded rather than left registered without a process.
async fn restore(
    plugins: &Plugins,
    slot: &mut Option<Process>,
    live: &Live,
    id: &str,
    error: PluginError,
) -> PluginError {
    let Some(process) = slot.as_mut() else {
        return error;
    };

    match process.restart().await {
        Ok(()) => {
            live.restart_generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            info!(plugin = %id, "Previous plugin version restored.");

            error
        }

        Err(restart) => {
            warn!(
                plugin = %id,
                error = %diagnostic(restart.to_string()),
                "Previous plugin version could not restart, and it is unloaded."
            );

            if let Some(process) = slot.take() {
                let _ = process.stop().await;
            }

            plugins.remove(id).await;

            format!(
                "{} The previous version could not restart either, so {id} is stopped.\nRun \
                 `krabb service restart` to load it again.",
                sentence(error.to_string())
            )
            .into()
        }
    }
}

#[cfg(test)]
mod tests;
