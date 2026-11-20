//! `krabb update`: one transaction for the host, the official plugins it can no longer
//! load, and the service.
//!
//! The running host stages and verifies the update, then takes a recovery point
//! with the service stopped. After the binaries are swapped, the new host finishes
//! activation with `update --finish`; any failure restores the recovery point. The
//! hidden `--inspect` and `--finish` flags are a contract between releases.

mod finish;
mod snapshot;
mod stage;

use std::time::Duration;

use clap::Args;

use super::*;

const READY: Duration = Duration::from_secs(60);
const STOP: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Args)]
pub(super) struct Options {
    #[arg(value_name = "VERSION", help = "Release to install; defaults to the latest stable one.")]
    version: Option<String>,
    #[arg(long, conflicts_with = "version", help = "Restore the retained recovery point.")]
    rollback: bool,
    #[arg(short, long, help = "Apply without a confirmation prompt.")]
    yes: bool,
    #[arg(long, hide = true, value_name = "DIRECTORY", conflicts_with_all = ["version", "rollback"])]
    finish: Option<PathBuf>,
    #[arg(long, hide = true, requires = "finish")]
    start: bool,
    #[arg(long, hide = true, conflicts_with_all = ["version", "rollback", "finish"])]
    inspect: bool,
}

pub(super) async fn run(options: Options, root: &Path, json: bool) -> Result<(), PluginError> {
    if options.inspect {
        return inspect(root);
    }

    if let Some(dir) = options.finish {
        return finish::run(root, &dir, options.start, json).await;
    }

    ensure_real_directory(root)?;

    let _lock = lock_at(root, ".update.lock").map_err(|error| -> PluginError {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            "Another Krabb update is running.".into()
        } else {
            error.into()
        }
    })?;

    if options.rollback {
        return snapshot::rollback(root, &service_path(), options.yes, json).await;
    }

    update(root, options.version, options.yes, json).await
}

/// Retained recovery point for `krabb status`.
pub(super) fn recovery(root: &Path) -> Option<serde_json::Value> {
    snapshot::summary(root)
}

fn inspect(root: &Path) -> Result<(), PluginError> {
    let plan = schema::plan(root)?;

    println!(
        "{}",
        serde_json::json!({
            "version": VERSION,
            "schema": {"from": plan.from, "to": plan.to, "steps": plan.steps},
            "protocol": Protocol::CURRENT,
        })
    );

    Ok(())
}

async fn update(
    root: &Path,
    requested: Option<String>,
    yes: bool,
    json: bool,
) -> Result<(), PluginError> {
    if let Some(point) = snapshot::load(root)?
        && !point.accepted
    {
        return Err(format!(
            "The update to {} did not finish; run `krabb update --rollback` to restore Krabb {}.",
            point.to, point.from
        )
        .into());
    }

    let version = match requested {
        Some(version) if releases::valid_version(&version) => version,
        Some(version) => return Err(format!("Version {version} is not a valid release.").into()),
        None => releases::latest()?,
    };

    match releases::compare(&version, VERSION) {
        std::cmp::Ordering::Less => {
            return Err(format!(
                "Krabb {VERSION} is newer than {version}; use `krabb update --rollback` to \
                 return to the retained recovery point."
            )
            .into());
        }

        std::cmp::Ordering::Equal => {
            report_current(json)?;
            return Ok(());
        }

        std::cmp::Ordering::Greater => {}
    }

    let service = service_path();
    let running = service.is_file() && running()?;

    if !running && control_at("plugin.active", serde_json::json!({}), root).await?.is_some() {
        return Err(
            "A Krabb daemon is running outside the service; stop it before updating.".into()
        );
    }

    let binaries = [std::env::current_exe()?, daemon_executable()?];

    for binary in &binaries {
        stage::writable(binary)?;
        stage::sweep(binary);
    }

    let lock = load_lock_at(root)?;

    let spinner = terminal::progress(format!("Downloading Krabb {version}..."), json);
    let mut staged = stage::prepare(root, &version, releases::base().as_deref())?;
    let inspection = stage::inspect(&staged.krabb, root)?;

    if inspection.version != version {
        return Err(format!(
            "The downloaded release reports version {}, not {version}.",
            inspection.version
        )
        .into());
    }

    stage::plugins(&mut staged, &lock, &inspection, &|| index::load(root))?;

    drop(spinner);

    stage::compatible(&staged, &inspection)?;

    let preview = preview(&version, &lock, &staged, &inspection.schema, &service, running);

    if json && !yes {
        println!("{}", serde_json::to_string_pretty(&preview.value)?);
        return Ok(());
    }

    if !json {
        println!("{}\n", preview.text);
    }

    if !confirm_action(yes, json, "Apply this update?")? {
        print_cancelled("Update", json);
        return Ok(());
    }

    if running {
        service_action_with(&service, ServiceAction::Stop, json)?;
    }

    let point = match released(root).await {
        Ok(()) => snapshot::create(root, VERSION, &version, &binaries, running).map_err(
            |error| -> PluginError {
                format!(
                    "Could not create the recovery point: {} The installation was not changed.",
                    sentence(error.to_string())
                )
                .into()
            },
        ),

        Err(error) => Err(error),
    };

    // Nothing was changed yet, so bring the service back before reporting the failure.
    let mut point = match point {
        Ok(point) => point,

        Err(error) => {
            if running {
                service_action_with(&service, ServiceAction::Start, json)?;
            }

            return Err(error);
        }
    };

    staged.keep();

    if !json {
        eprintln!("Activating Krabb {version}...");
    }

    let result = activate(root, &staged, &binaries, running, json).await;

    if let Err(error) = result {
        let failure = sentence(error.to_string());

        staged.discard();

        return Err(match snapshot::restore(root, &point, &service, json).await {
            Ok(()) => {
                format!("The update to {version} failed: {failure} Restored Krabb {VERSION}.")
            }

            Err(restore) => format!(
                "The update to {version} failed: {failure} Recovery also failed: {} Run `{}` \
                 to retry the rollback.",
                sentence(restore.to_string()),
                rollback_command(root)
            ),
        }
        .into());
    }

    snapshot::accept(root, &mut point)?;
    staged.discard();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "action": "update",
                "status": "updated",
                "from": VERSION,
                "version": version,
            }))?
        );
    } else {
        println!(
            "Updated Krabb {VERSION} to {version}.\nRun `krabb update --rollback` to return to \
             {VERSION}; the recovery point is kept until the next update."
        );
    }

    Ok(())
}

async fn activate(
    root: &Path,
    staged: &stage::Staged,
    binaries: &[PathBuf; 2],
    start: bool,
    json: bool,
) -> Result<(), PluginError> {
    stage::swap(&binaries[0], &staged.krabb)?;
    stage::swap(&binaries[1], &staged.daemon)?;

    let mut command = tokio::process::Command::new(&binaries[0]);

    command.arg("update").arg("--finish").arg(&staged.dir).env("KRABB_HOME", root);

    if start {
        command.arg("--start");
    }

    if json {
        command.arg("--json");
    }

    let status = command.status().await?;

    if !status.success() {
        return Err("The new version could not finish activation.".into());
    }

    Ok(())
}

fn report_current(json: bool) -> Result<(), PluginError> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "action": "update",
                "status": "current",
                "version": VERSION,
            }))?
        );
    } else {
        println!("Krabb {VERSION} is up to date.");
    }

    Ok(())
}

fn running() -> Result<bool, PluginError> {
    Ok(matches!(service_manager_state()?, ServiceState::Active | ServiceState::Starting))
}

/// Waits until no process holds the daemon lock.
async fn released(root: &Path) -> Result<(), PluginError> {
    let deadline = tokio::time::Instant::now() + STOP;

    loop {
        match owner_lock(root) {
            Ok(lock) => {
                drop(lock);
                return Ok(());
            }

            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(error.into());
            }

            Err(_) if tokio::time::Instant::now() >= deadline => {
                return Err(format!(
                    "The Krabb daemon did not stop within {} seconds.",
                    STOP.as_secs()
                )
                .into());
            }

            Err(_) => tokio::time::sleep(POLL).await,
        }
    }
}

fn rollback_command(root: &Path) -> String {
    let name = format!("krabb{}", std::env::consts::EXE_SUFFIX);

    format!("{} update --rollback", snapshot::store(root).join("bin/0").join(name).display())
}

struct Preview {
    text: String,
    value: serde_json::Value,
}

fn preview(
    version: &str,
    lock: &Lock,
    staged: &stage::Staged,
    schema: &stage::Schema,
    service: &Path,
    running: bool,
) -> Preview {
    let mut rows = vec![format!("krabb and krabb-daemon: {VERSION} → {version}")];
    let mut plugins = Vec::new();

    for (id, manifest) in &staged.manifests {
        let Some(entry) = lock.plugins.get(id) else {
            continue;
        };

        let pin = index::parse(&entry.source)
            .and_then(|(_, pinned)| pinned)
            .map(|pinned| format!(" (pin to {pinned} removed; it cannot run on {version})"))
            .unwrap_or_default();

        rows.push(format!("{id}: {} → {}{pin}", entry.version, manifest.version));

        let changes = [
            ("permissions", &entry.permissions, &manifest.permissions),
            ("capabilities", &entry.capabilities, &manifest.capabilities),
            ("secrets", &entry.variables, &manifest.variables),
        ]
        .into_iter()
        .filter_map(|(label, before, after)| change(label, before, after))
        .collect::<Vec<_>>();

        for change in &changes {
            rows.push(format!("{id} {change}"));
        }

        plugins.push(serde_json::json!({
            "id": id,
            "from": entry.version,
            "to": manifest.version,
            "changes": changes,
        }));
    }

    let mut kept = Vec::new();
    let mut supported = Vec::new();

    for (id, entry) in lock.plugins.iter().filter(|(id, _)| !staged.manifests.contains_key(*id)) {
        let reason = if entry.linked {
            Some("linked development build")
        } else if index::official(entry).is_none() {
            Some("custom source")
        } else {
            None
        };

        match reason {
            Some(_) => kept.push(id),
            None => supported.push(id),
        }

        rows.push(format!("{id}: stays at {} ({})", entry.version, reason.unwrap_or("supported")));
    }

    if schema.to > schema.from {
        let count = schema.steps.len();
        let noun = if count == 1 { "migration" } else { "migrations" };

        rows.push(format!("home schema: {} → {} ({count} {noun})", schema.from, schema.to));
    }

    let interruption = if running {
        "service: restarts; replies pause until it is ready"
    } else if service.is_file() {
        "service: stays stopped"
    } else {
        "service: not installed"
    };

    rows.push(interruption.into());

    let text = format!(
        "Update Krabb {VERSION} to {version}\n{}\n\nA recovery point is kept; \
         `krabb update --rollback` restores {VERSION}.",
        format_tree_list(&rows)
    );

    let value = serde_json::json!({
        "action": "update",
        "status": "preview",
        "from": VERSION,
        "to": version,
        "plugins": plugins,
        "kept": kept,
        "supported": supported,
        "schema": schema,
        "restart": running,
    });

    Preview { text, value }
}

fn change(label: &str, before: &[String], after: &[String]) -> Option<String> {
    let added = after.iter().filter(|value| !before.contains(value)).cloned().collect::<Vec<_>>();
    let removed = before.iter().filter(|value| !after.contains(value)).cloned().collect::<Vec<_>>();

    let parts = [("adds", added), ("removes", removed)]
        .into_iter()
        .filter(|(_, values)| !values.is_empty())
        .map(|(verb, values)| format!("{verb} {}", values.join(", ")))
        .collect::<Vec<_>>();

    (!parts.is_empty()).then(|| format!("{label}: {}", parts.join("; ")))
}

#[cfg(test)]
mod tests;
