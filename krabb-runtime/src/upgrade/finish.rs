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
                "Plugin {id} uses a protocol Krabb {VERSION} does not support; \
                 reinstall a compatible build after rolling back."
            )
            .into());
        }
    }

    drop(offline_lock(root)?);

    if start {
        service_action_with(&service_path(), ServiceAction::Start, json)?;
        ready(root).await?;
    }

    Ok(())
}

fn install(root: &Path, dir: &Path, artifact: &stage::Artifact) -> Result<(), PluginError> {
    let (_, digest) =
        releases::parts(&artifact.source).ok_or("A staged plugin source has no checksum.")?;

    let archive = dir.join(&artifact.file);

    link_at(
        Source {
            id: artifact.id.clone(),
            source: Some(format!("{}#sha256={digest}", archive.display())),
            revision: None,
            version: None,
            yes: true,
        },
        false,
        root,
    )
    .map_err(|error| format!("Plugin {} failed: {}", artifact.id, sentence(error.to_string())))?;

    let _plugins = lock_at(root, ".plugins.lock")?;
    let mut lock = load_lock_at(root)?;
    let entry = lock.plugins.get_mut(&artifact.id).ok_or("A staged plugin was not recorded.")?;

    entry.source = artifact.source.clone();
    save_lock_at(root, &lock)?;

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
