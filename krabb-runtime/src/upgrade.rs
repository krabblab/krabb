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

pub(super) use snapshot::uninstalled;

use std::time::Duration;

use clap::Args;

use super::*;

const READY: Duration = Duration::from_secs(60);
const STOP: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Args)]
pub(super) struct Options {
    #[arg(value_name = "VERSION", help = "Release to install, by default the latest stable one.")]
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
            "The update to {} did not finish.\nRun `krabb update --rollback` to restore Krabb {}.",
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
                "Krabb {VERSION} is newer than {version}.\nUse `krabb update --rollback` to \
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
            "A Krabb daemon is running outside the service, so stop it before updating.".into()
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

    if !json {
        println!(
            "Updated Krabb {VERSION} to {version}.\nRun `krabb update --rollback` to return to \
            {VERSION}.\nThe recovery point is kept until the next update."
        );
    }

    let unofficial = lock.plugins.values().any(|entry| unofficial(entry).is_some());

    let refreshed = if !unofficial {
        Refresh::None
    } else if json || !interactive() {
        Refresh::Pending
    } else {
        refresh(root, &binaries[0]).await
    };

    if json {
        let refresh = match &refreshed {
            Refresh::None => serde_json::Value::Null,

            Refresh::Pending => {
                serde_json::json!({"status": "pending", "command": "krabb plugin update"})
            }

            Refresh::Done => serde_json::json!({"status": "reviewed"}),
            Refresh::Failed(error) => serde_json::json!({"status": "failed", "error": error}),
        };

        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "action": "update",
                "status": "updated",
                "from": VERSION,
                "version": version,
                "refresh": refresh,
            }))?
        );
    } else if let Refresh::Pending = refreshed {
        eprintln!(
            "Linked and custom-source plugins were not refreshed without a terminal to review \
             them.\nRun `krabb plugin update` to review and apply their changes."
        );
    } else if let Refresh::Failed(error) = refreshed {
        eprintln!(
            "Linked and custom-source plugins were not refreshed: {error}\nRun `krabb plugin \
             update` to try again; Krabb {version} stays installed."
        );
    }

    Ok(())
}

/// What happened to plugins that refresh from their own sources after the host update.
enum Refresh {
    None,
    Pending,
    Done,
    Failed(String),
}

/// Lets the new host review and apply refreshes from linked and custom sources in the terminal.
///
/// The update preview cannot show their permission changes, so the user confirms them here.
async fn refresh(root: &Path, krabb: &Path) -> Refresh {
    eprintln!("Checking linked and custom-source plugins for changes...");

    let status = tokio::process::Command::new(krabb)
        .args(["plugin", "update"])
        .env("KRABB_HOME", root)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await;

    match status {
        Ok(status) if status.success() => Refresh::Done,
        Ok(status) => Refresh::Failed(format!("`krabb plugin update` exited with {status}.")),
        Err(error) => Refresh::Failed(sentence(error.to_string())),
    }
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

        let staged_pin = staged
            .plan
            .plugins
            .iter()
            .find(|artifact| &artifact.id == id)
            .and_then(|artifact| artifact.pin.as_deref());

        let pin = match (index::parse(&entry.source).and_then(|(_, pinned)| pinned), staged_pin) {
            (_, Some(pin)) => format!(" (pinned to pre-release {pin})"),

            (Some(pinned), None) if !releases::stable(pinned) => {
                format!(" (pre-release pin to {pinned} replaced by the stable release)")
            }

            (Some(pinned), None) => {
                format!(" (pin to {pinned} removed because it cannot run on {version})")
            }

            (None, None) => String::new(),
        };

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
    let mut refreshed = Vec::new();

    for (id, entry) in lock.plugins.iter().filter(|(id, _)| !staged.manifests.contains_key(*id)) {
        if let Some(source) = unofficial(entry) {
            refreshed.push(id);
            rows.push(format!("{id}: refreshes from its {source} after the update"));
            continue;
        }

        let pinned = index::official(entry).and_then(|(_, version)| version);

        let reason = if let Some(version) = pinned {
            Some(format!("pinned to {version}"))
        } else if entry.pinned {
            Some("pinned to a revision".to_owned())
        } else if staged.unlisted.contains(id) {
            Some("no longer in the plugin index".to_owned())
        } else if staged.unchecked.contains(id) {
            Some("plugin index unavailable".to_owned())
        } else {
            None
        };

        match &reason {
            Some(_) => kept.push(id),
            None => supported.push(id),
        }

        let reason = reason.as_deref().unwrap_or("current");

        rows.push(format!("{id}: stays at {} ({reason})", entry.version));
    }

    if schema.to > schema.from {
        let count = schema.steps.len();
        let noun = if count == 1 { "migration" } else { "migrations" };

        rows.push(format!("home schema: {} → {} ({count} {noun})", schema.from, schema.to));
    }

    let interruption = if running {
        "service: restarts, and replies pause until it is ready"
    } else if service.is_file() {
        "service: stays stopped"
    } else {
        "service: not installed"
    };

    rows.push(interruption.into());

    let text = format!(
        "Update Krabb {VERSION} to {version}\n{}\n\nA recovery point is kept.\n`krabb update \
        --rollback` restores {VERSION}.",
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
        "refresh": refreshed,
        "schema": schema,
        "restart": running,
    });

    Preview { text, value }
}

/// The source an unpinned plugin refreshes from after an update, when it is not an official
/// release.
fn unofficial(entry: &Entry) -> Option<&'static str> {
    if entry.pinned {
        None
    } else if entry.linked {
        Some("linked development build")
    } else if index::official(entry).is_none() {
        Some("custom source")
    } else {
        None
    }
}

fn change(label: &str, before: &[String], after: &[String]) -> Option<String> {
    let added = after.iter().filter(|value| !before.contains(value)).cloned().collect::<Vec<_>>();
    let removed = before.iter().filter(|value| !after.contains(value)).cloned().collect::<Vec<_>>();

    let parts = [("adds", added), ("removes", removed)]
        .into_iter()
        .filter(|(_, values)| !values.is_empty())
        .map(|(verb, values)| format!("{verb} {}", values.join(", ")))
        .collect::<Vec<_>>();

    (!parts.is_empty()).then(|| format!("{label}: {}", parts.join(", ")))
}

#[cfg(test)]
mod tests;
