//! Activation steps that run inside the newly installed host.

use super::*;

/// Installs the staged plugins, migrates the home, and proves readiness.
pub(super) async fn run(
    root: &Path,
    dir: &Path,
    start: bool,
    json: bool,
) -> Result<(), PluginError> {
    let plan = stage::plan(dir)?;

    if plan.version != VERSION {
        return Err(format!(
            "The staged update is for {}, but this Krabb is {VERSION}.",
            plan.version
        )
        .into());
    }

    for artifact in &plan.plugins {
        install(root, dir, artifact)?;
    }

    for (id, entry) in load_lock_at(root)?.plugins {
        if !Protocol::CURRENT.compatible(entry.protocol) {
            return Err(format!(
                "Plugin {id} uses a protocol Krabb {VERSION} does not support, so \
                 reinstall a compatible build after rolling back."
            )
            .into());
        }
    }

    drop(offline_lock(root)?);

    if start {
        carry(root, &plan.detached)?;
        service_action_with(&service_path(), ServiceAction::Start, json)?;
        ready(root).await?;

        let missing = loaded(root, &plan.attached, READY).await;

        if !missing.is_empty() {
            eprintln!(
                "Plugins not loaded after the update: {}.\nRun `krabb status` for their errors, \
                 reinstall them with `krabb plugin install`, or return with `krabb update \
                 --rollback`.",
                missing.join(", ")
            );
        }
    }

    Ok(())
}

/// Asks the next daemon start to keep these plugins detached.
pub(super) fn carry(root: &Path, detached: &[String]) -> Result<(), PluginError> {
    if detached.is_empty() {
        return Ok(());
    }

    krabb_file::save(root.join(DETACHED), serde_json::to_vec(detached)?)?;

    Ok(())
}

/// Waits until every plugin loaded before the update is loaded again, and returns any that are not.
pub(super) async fn loaded(root: &Path, attached: &[String], limit: Duration) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + limit;

    loop {
        let active = control_at("plugin.active", serde_json::json!({}), root)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();

        let missing = attached
            .iter()
            .filter(|id| {
                !active["items"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item == *id))
            })
            .cloned()
            .collect::<Vec<_>>();

        if missing.is_empty() || tokio::time::Instant::now() >= deadline {
            return missing;
        }

        tokio::time::sleep(POLL).await;
    }
}

fn install(root: &Path, dir: &Path, artifact: &stage::Artifact) -> Result<(), PluginError> {
    let archive = dir.join(&artifact.file);

    link_recorded(
        Source {
            id: artifact.id.clone(),
            source: Some(format!("{}#sha256={}", archive.display(), artifact.sha256)),
            revision: None,
            version: None,
            yes: true,
        },
        false,
        root,
        Some(&index::source(&artifact.id, artifact.pin.as_deref())),
    )
    .map_err(|error| format!("Plugin {} failed: {}", artifact.id, sentence(error.to_string())))?;

    Ok(())
}

async fn ready(root: &Path) -> Result<(), PluginError> {
    let deadline = tokio::time::Instant::now() + READY;

    loop {
        if let Ok(Some(_)) = control_at("plugin.active", serde_json::json!({}), root).await {
            return Ok(());
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "The updated daemon did not become ready within {} seconds.",
                READY.as_secs()
            )
            .into());
        }

        tokio::time::sleep(POLL).await;
    }
}
