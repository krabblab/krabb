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

        let missing = missing(&plan.attached, &ready(root).await?);

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

/// Asks the next daemon start to keep these plugins detached, replacing any stale request.
pub(super) fn carry(root: &Path, detached: &[String]) -> Result<(), PluginError> {
    krabb_file::save(root.join(DETACHED), serde_json::to_vec(detached)?)?;

    Ok(())
}

/// Plugins loaded before the update that the restarted daemon does not report as loaded.
pub(super) fn missing(attached: &[String], active: &[String]) -> Vec<String> {
    attached.iter().filter(|id| !active.contains(id)).cloned().collect()
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

/// Waits for the restarted daemon and returns the plugins it loaded, which is final once it answers.
async fn ready(root: &Path) -> Result<Vec<String>, PluginError> {
    let deadline = tokio::time::Instant::now() + READY;

    loop {
        if let Ok(Some(active)) = active_at(root).await {
            return Ok(active);
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
