//! Activation steps that run inside the newly installed host.

use super::*;

/// Installs the staged plugins, migrates the home, and proves readiness.
pub(super) async fn run(
    root: &Path,
    dir: &Path,
    start: bool,
    json: bool,
) -> Result<(), PluginError> {
    // Held until activation ends, so recovery never undoes it even when the update that
    // started it has died.
    let _finishing = lock_at(root, interrupted::FINISH_LOCK)?;

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
                "Plugin {id} uses a protocol Krabb {VERSION} does not support.\nUninstall it \
                 with `krabb plugin uninstall {id}` before updating."
            )
            .into());
        }
    }

    let service = service_path();

    let saved = snapshot::load(root)?
        .and_then(|point| point.definition)
        .is_some_and(|saved| saved.path == service);

    let mut report = Report::default();

    if saved {
        match definition::refresh(&service) {
            Ok(refreshed) => {
                report.refreshed = refreshed;

                if refreshed && !json {
                    eprintln!("Refreshed the service definition to the current format.");
                }
            }

            Err(error) => {
                let error = sentence(error.to_string());

                eprintln!("The service definition could not be refreshed: {error}");
                report.unrefreshed = Some(error);
            }
        }
    }

    drop(offline_lock(root)?);

    if start {
        carry(root, &plan.detached)?;
        service_action_with(&service, ServiceAction::Start, ServiceReport::nested(json))?;

        let missing = missing(&plan.attached, &ready(root).await?);

        if !missing.is_empty() {
            eprintln!(
                "Plugins not loaded after the update: {}.\nRun `krabb status` for their errors, \
                 reinstall them with `krabb plugin install`, or return with `krabb update \
                 --rollback`.",
                missing.join(", ")
            );
        }

        report.unloaded = missing;
    }

    // Recovery reads the report as proof that activation finished, so it is written last and
    // failing to save it fails activation. Later releases must keep both rules.
    krabb_file::save(dir.join(REPORT), serde_json::to_vec(&report)?)?;

    Ok(())
}

/// What activation noticed without failing, which the host that started the update reports.
/// It is a file in the staged update, so a host from an earlier release simply ignores it.
#[derive(Debug, Default, Deserialize, PartialEq, Serialize)]
pub(super) struct Report {
    /// Plugins loaded before the update that the restarted daemon did not load.
    #[serde(default)]
    pub unloaded: Vec<String>,
    /// Whether the service definition was rewritten to the current format.
    #[serde(default)]
    pub refreshed: bool,
    /// Why an outdated service definition could not be rewritten.
    #[serde(default)]
    pub unrefreshed: Option<String>,
}

const REPORT: &str = "finish.json";

const REPORT_LIMIT: u64 = 64 * 1024;

/// Whether the new host finished activating. Existence alone decides, so a report a later
/// release writes in another format still counts.
pub(super) fn reported(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(REPORT)).is_ok_and(|metadata| metadata.is_file())
}

/// The report the new host left in DIR, or none from a release that writes no report.
pub(super) fn report(dir: &Path) -> Option<Report> {
    krabb_file::load(dir.join(REPORT), REPORT_LIMIT)
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
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
