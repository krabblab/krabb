//! `krabb update`: one transaction for the host, the official plugins it can no longer
//! load, and the service.
//!
//! The running host stages and verifies the update, then takes a recovery point
//! with the service stopped. After the binaries are swapped, the new host finishes
//! activation with `update --finish`; any failure restores the recovery point. The
//! hidden `--inspect` and `--finish` flags are a contract between releases.

mod finish;
mod interrupted;
mod snapshot;
mod space;
mod stage;

pub(crate) use interrupted::{Caller, Outcome, before_command, outcome, recover};
pub(super) use snapshot::uninstalled;

use std::time::Duration;

use clap::Args;

use super::*;

const READY: Duration = Duration::from_secs(60);
const STOP: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(250);
const DETACHED: &str = "updates/detached.json";
const DETACHED_LIMIT: u64 = 64 * 1024;

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
        // An activation whose update died still runs; restoring underneath it mixes versions.
        let Some(_finish) = interrupted::held(root, interrupted::FINISH_LOCK)? else {
            return Err(interrupted::BUSY.into());
        };

        snapshot::rollback(root, &service_path(), options.yes, json).await?;

        // A cancelled rollback keeps its recovery point, and with it what status reports.
        if snapshot::load(root)?.is_none() {
            interrupted::clear_intent(root)?;
            interrupted::clear_outcome(root)?;
        }

        return Ok(());
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
            "definition": definition::stale(&service_path()).is_some(),
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
    // An interrupted update is settled first, so this one starts from a working installation.
    interrupted::before_update(root, &service_path(), json).await?;

    let version = match requested {
        Some(version) if releases::valid_version(&version) => version,

        Some(version) => {
            return Err(format!(
                "Version {version} is not a valid release.\nUpdate to the latest release with \
                 `krabb update`."
            )
            .into());
        }

        None => releases::latest().map_err(|error| unchanged(error, None))?,
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
        return Err(format!(
            "A Krabb daemon is running outside the service.\nStop it, then try again with `{}`.",
            retry(Some(&version))
        )
        .into());
    }

    let binaries = [std::env::current_exe()?, daemon_executable()?];

    for binary in &binaries {
        stage::writable(binary)?;
        stage::sweep(binary);
    }

    let lock = load_lock_at(root)?;

    let spinner = terminal::progress(format!("Downloading Krabb {version}..."), json);
    let mut staged = stage::prepare(root, &version, releases::base().as_deref())
        .map_err(|error| unchanged(error, Some(&version)))?;

    let inspection =
        stage::inspect(&staged.krabb, root).map_err(|error| unchanged(error, Some(&version)))?;

    if inspection.version != version {
        return Err(format!(
            "The downloaded release reports version {}, not {version}.\nThe installation was \
             not changed.\nReport it at {ISSUES}.",
            inspection.version
        )
        .into());
    }

    stage::plugins(&mut staged, &lock, &inspection, &|| index::load(root))
        .map_err(|error| unchanged(error, Some(&version)))?;

    drop(spinner);

    stage::compatible(&staged, &inspection)?;
    stage::readable(root, &mut staged, &lock)?;
    space::check(root, &binaries, &staged).map_err(|error| unchanged(error, Some(&version)))?;

    let preview = preview(
        &version,
        &lock,
        &staged,
        &inspection.schema,
        &service,
        running,
        inspection.definition,
    );

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

    interrupted::clear_outcome(root)?;

    // Saved before the service stops, so a cut-off update can start it again.
    interrupted::intend(
        root,
        &interrupted::Intent { from: VERSION.into(), to: version.clone(), service: running },
    )?;

    if running {
        let stopped = match record(root, &mut staged).await {
            Ok(()) => {
                service_action_with(&service, ServiceAction::Stop, ServiceReport::nested(json))
            }

            Err(error) => Err(error),
        };

        // A clean failure is not an interruption, so the service comes back and the intent goes.
        // If the service cannot start, the intent stays so the next command starts it.
        if let Err(error) = stopped {
            let restarted = match self::running() {
                Ok(true) => Ok(()),

                _ => {
                    service_action_with(&service, ServiceAction::Start, ServiceReport::nested(json))
                }
            };

            if restarted.is_ok() {
                interrupted::clear_intent(root)?;
            }

            return Err(error);
        }
    }

    let installed = service.is_file().then_some(service.as_path());

    let point = match released(root).await {
        Ok(()) => snapshot::create(root, VERSION, &version, &binaries, installed, running).map_err(
            |error| -> PluginError {
                let failure = format!("Could not create the recovery point: {error}");

                ending(&failure, &format!("Krabb {VERSION} is still installed."), Some(&version))
                    .into()
            },
        ),

        Err(error) => Err(error),
    };

    // Nothing was changed yet, so bring the service back before reporting the failure.
    let mut point = match point {
        Ok(point) => point,

        Err(error) => {
            let restarted = if running {
                service_action_with(&service, ServiceAction::Start, ServiceReport::nested(json))
            } else {
                Ok(())
            };

            // A service that did not start keeps the intent, so the next command starts it.
            if restarted.is_ok() {
                interrupted::clear_intent(root)?;
            }

            return Err(error);
        }
    };

    interrupted::clear_intent(root)?;

    staged.keep();

    if !json {
        eprintln!("Activating Krabb {version}...");
    }

    let result = activate(root, &staged, &binaries, running, json).await;

    if let Err(error) = result {
        let failure = sentence(error.to_string());

        staged.discard();

        return Err(match snapshot::restore(root, &point, &service, json).await {
            Ok(()) => ending(
                &format!(
                    "The update to {version} failed, so Krabb {VERSION} was restored.\n{failure}"
                ),
                &format!(
                    "The log at {} has more detail.",
                    logging::application_log_path().display()
                ),
                Some(&version),
            ),

            Err(restore) => format!(
                "The update to {version} failed: {failure}\nRecovery also failed: {}\nRetry the \
                 rollback with `{}`.",
                sentence(restore.to_string()),
                rollback_command(root)
            ),
        }
        .into());
    }

    let report = finish::report(&staged.dir);

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
            Refresh::None | Refresh::Done => serde_json::Value::Null,

            Refresh::Pending => {
                serde_json::json!({"status": "pending", "command": "krabb plugin update"})
            }

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
                "unloaded": report.as_ref().map(|report| &report.unloaded),
                "service_definition": report.as_ref().map(definition_outcome),
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

/// Records which plugins the running daemon has loaded or detached, for activation to restore.
async fn record(root: &Path, staged: &mut stage::Staged) -> Result<(), PluginError> {
    let Ok(Some(active)) = control_at("plugin.active", serde_json::json!({}), root).await else {
        return Ok(());
    };

    let detached = serde_json::json!({"items": active["detached"].clone()});

    staged.plan.attached = active_ids(active).unwrap_or_default();
    staged.plan.detached = active_ids(detached).unwrap_or_default();

    stage::save(staged)
}

/// Reads and removes the plugins activation asked the next daemon start to keep detached.
pub(crate) fn carried(root: &Path) -> BTreeSet<String> {
    let path = root.join(DETACHED);
    let bytes = krabb_file::load(&path, DETACHED_LIMIT).ok().flatten();

    let _ = std::fs::remove_file(&path);

    bytes
        .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|id| valid(id))
        .collect()
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

    if !json {
        let status = command.status().await?;

        if !status.success() {
            return Err("The new version could not finish activation.".into());
        }

        return Ok(());
    }

    // The new host reports its own failure as a JSON document; fold it into this command's
    // single error instead of printing two.
    let output = command
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let (warnings, error) = reported(&stderr);

    eprint!("{warnings}");

    if output.status.success() {
        return Ok(());
    }

    Err(error.unwrap_or_else(|| "The new version could not finish activation.".into()).into())
}

/// The service definition's outcome for the JSON result, or null when nothing changed.
fn definition_outcome(report: &finish::Report) -> serde_json::Value {
    match (&report.unrefreshed, report.refreshed) {
        (Some(error), _) => serde_json::json!({"status": "failed", "error": error}),
        (None, true) => serde_json::json!({"status": "refreshed"}),
        (None, false) => serde_json::Value::Null,
    }
}

/// Splits a `--json` host's standard error into its warnings and the message of the error
/// document it ends with, if any.
fn reported(stderr: &str) -> (&str, Option<String>) {
    let start =
        if stderr.starts_with('{') { Some(0) } else { stderr.rfind("\n{").map(|at| at + 1) };

    let Some(start) = start else {
        return (stderr, None);
    };

    let error = serde_json::from_str::<serde_json::Value>(&stderr[start..])
        .ok()
        .and_then(|value| value["error"].as_str().map(str::to_owned));

    match error {
        Some(error) => (&stderr[..start], Some(error)),
        None => (stderr, None),
    }
}

/// Where to report a release that is broken for everyone.
pub(super) const ISSUES: &str = "https://github.com/krabblab/krabb/issues";

/// A failure before anything changed.
fn unchanged(error: PluginError, version: Option<&str>) -> PluginError {
    ending(&error.to_string(), "The installation was not changed.", version).into()
}

/// A failure, what state it left behind, and one next action: its own when it names one,
/// otherwise the update command that tries again.
fn ending(failure: &str, state: &str, version: Option<&str>) -> String {
    let failure = sentence(failure);
    let failure = failure.lines().filter(|line| *line != state).collect::<Vec<_>>().join("\n");

    let Some((cause, action)) = failure.rsplit_once('\n').filter(|(_, last)| crate::acts(last))
    else {
        return format!("{failure}\n{state}\nTry again with `{}`.", retry(version));
    };

    format!("{cause}\n{state}\n{action}")
}

/// The command that repeats an update, keeping the version the user asked for.
fn retry(version: Option<&str>) -> String {
    version.map_or_else(|| "krabb update".into(), |version| format!("krabb update {version}"))
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
                    "The Krabb daemon did not stop within {} seconds.\nStop it with `krabb \
                     service stop`, then try again.",
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
    definition: bool,
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

        let migration = staged.migrations.get(id);

        if let Some(migration) = migration {
            rows.push(format!("{id} {}", migration.describe()));
        }

        plugins.push(serde_json::json!({
            "id": id,
            "from": entry.version,
            "to": manifest.version,
            "changes": changes,
            "state": migration.map(|migration| serde_json::json!({
                "from": migration.from,
                "to": migration.to,
                "text": migration.describe(),
            })),
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

    if definition {
        rows.push("service definition: refreshes to the current format".into());
    }

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
        "definition": definition,
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
