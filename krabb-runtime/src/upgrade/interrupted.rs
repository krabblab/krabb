//! Brings back a working installation after `krabb update` was cut off by a closed
//! terminal, a crash, or power loss.
//!
//! An update records its intent before it stops the service, then a recovery point before it
//! changes anything. The next command or daemon start that finds either one, while no update or
//! activation holds its lock, keeps the update when the new host finished activating and
//! restores the previous installation otherwise.

use super::*;

const INTENT: &str = "updates/intent.json";
const OUTCOME: &str = "updates/recovered.json";
const LIMIT: u64 = 64 * 1024;
/// Held by the new host while it activates, even after the update that started it has died.
pub(super) const FINISH_LOCK: &str = ".finish.lock";
/// How long status mentions a recovery before it is old news.
const SHOWN: u64 = 7 * 24 * 60 * 60;

pub(super) const BUSY: &str =
    "An update or its recovery is still running.\nTry again once `krabb status` answers.";

/// What an update was about to do, saved before it stops the service.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Intent {
    pub from: String,
    pub to: String,
    /// Whether the service was running, so recovery starts it again.
    pub service: bool,
}

/// What recovery did, kept for `krabb status` for a week.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub from: String,
    pub to: String,
    /// The version installed after recovery: `to` when the update had finished.
    pub kept: String,
    /// Unix seconds.
    pub at: u64,
    /// Whether recovery put the previous binaries and home back.
    #[serde(default)]
    pub restored: bool,
}

impl Outcome {
    pub fn finished(&self) -> bool {
        self.kept == self.to
    }

    /// One sentence for the terminal, said once when recovery runs.
    pub fn notice(&self) -> String {
        if self.finished() {
            format!(
                "The update to Krabb {} had finished before it was interrupted, so it was kept.",
                self.to
            )
        } else if self.restored {
            format!(
                "The update to Krabb {} was interrupted, so Krabb {} was restored.",
                self.to, self.from
            )
        } else {
            format!(
                "The update to Krabb {} was interrupted before it changed anything, so Krabb {} \
                 stays installed.",
                self.to, self.kept
            )
        }
    }

    fn valid(&self) -> bool {
        [&self.from, &self.to, &self.kept].iter().all(|version| releases::valid_version(version))
    }
}

pub(super) fn intend(root: &Path, intent: &Intent) -> Result<(), PluginError> {
    krabb_file::save(root.join(INTENT), serde_json::to_vec(intent)?)?;

    Ok(())
}

/// Removes the intent once a recovery point records the same facts, or nothing changed.
pub(super) fn clear_intent(root: &Path) -> Result<(), PluginError> {
    krabb_migrations::recovery::remove(&root.join(INTENT))?;

    Ok(())
}

/// Removes a past recovery, because a new update or rollback replaces what it describes.
pub(super) fn clear_outcome(root: &Path) -> Result<(), PluginError> {
    krabb_migrations::recovery::remove(&root.join(OUTCOME))?;

    Ok(())
}

/// The last recovery within the past week, for `krabb status`.
pub(crate) fn outcome(root: &Path) -> Option<Outcome> {
    let bytes = krabb_file::load(root.join(OUTCOME), LIMIT).ok().flatten()?;
    let outcome = serde_json::from_slice::<Outcome>(&bytes).ok().filter(Outcome::valid)?;

    (unix_seconds().saturating_sub(outcome.at) < SHOWN).then_some(outcome)
}

/// The saved intent. An unreadable one only meant the service might need starting, so it is
/// dropped rather than blocking every start.
fn intent(root: &Path) -> Result<Option<Intent>, PluginError> {
    let Some(bytes) = krabb_file::load(root.join(INTENT), LIMIT)? else {
        return Ok(None);
    };

    let intent = serde_json::from_slice::<Intent>(&bytes).ok().filter(|intent| {
        releases::valid_version(&intent.from) && releases::valid_version(&intent.to)
    });

    if intent.is_none() {
        warn!("The update intent record is unreadable, so it was removed.");
        clear_intent(root)?;
    }

    Ok(intent)
}

/// Whether an update left something to settle. It is checked before any lock, so ordinary
/// commands never contend with `krabb update`.
fn pending(root: &Path) -> bool {
    root.join(INTENT).is_file()
        || snapshot::load(root).ok().flatten().is_some_and(|point| !point.accepted)
}

/// Where recovery runs. The daemon is the service, so it never stops or starts it: it
/// restores the files and exits, and the service manager starts the restored daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Caller {
    Command,
    Daemon,
}

/// Recovers when no update or activation runs, and says what it did. While one runs, a command
/// waits for it and the daemon it started carries on.
pub(crate) async fn recover(
    root: &Path,
    service: &Path,
    caller: Caller,
) -> Result<Option<Outcome>, PluginError> {
    if !pending(root) {
        return Ok(None);
    }

    ensure_real_directory(root)?;

    let busy = || match caller {
        Caller::Command => Err(PluginError::from(BUSY)),
        Caller::Daemon => Ok(None),
    };

    let Some(update) = held(root, ".update.lock")? else {
        return busy();
    };

    let Some(finish) = held(root, FINISH_LOCK)? else {
        return busy();
    };

    let outcome = recover_locked(root, service, VERSION, caller).await;

    drop((finish, update));

    outcome
}

/// The lock NAME, or none while another process holds it.
pub(super) fn held(root: &Path, name: &str) -> Result<Option<crate::DaemonLock>, PluginError> {
    match lock_at(root, name) {
        Ok(lock) => Ok(Some(lock)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Recovery once the caller holds the update and finish locks; `version` is the running host's.
///
/// Only the new host writes its activation report, last, so the report alone proves the update
/// finished, whichever binary runs recovery.
pub(super) async fn recover_locked(
    root: &Path,
    service: &Path,
    version: &str,
    caller: Caller,
) -> Result<Option<Outcome>, PluginError> {
    let manages = caller == Caller::Command;

    let outcome = match snapshot::load(root)? {
        Some(mut point) if !point.accepted => {
            if !automatic(&point, version) {
                return Err(format!(
                    "The interrupted update to {} cannot be settled automatically.\nRestore the \
                     previous version with `krabb update --rollback`.",
                    point.to
                )
                .into());
            }

            let staged = root.join("updates").join(format!("v{}", point.to));

            let outcome = if finish::reported(&staged) {
                snapshot::accept(root, &mut point)?;

                outcome_of(&point.from, &point.to, &point.to, false)
            } else {
                snapshot::restore_with(root, &point, service, true, manages, false).await?;

                outcome_of(&point.from, &point.to, &point.from, true)
            };

            // The staged release is only needed while the update runs, so failing to remove it
            // never undoes recovery; the next update replaces it.
            if let Err(error) = krabb_migrations::recovery::remove(&staged) {
                warn!(error = %diagnostic(sentence(error.to_string())), "The staged release stays.");
            }

            (outcome, manages && point.service)
        }

        _ => match intent(root)? {
            // The update stopped before its recovery point, so only the service changed.
            Some(intent) => {
                let starts = manages && intent.service;

                (outcome_of(&intent.from, &intent.to, version, false), starts)
            }

            None => return Ok(None),
        },
    };

    let (outcome, starts) = outcome;

    // Everything recovery did is recorded first, so a failed start never hides it, and the
    // outcome before the intent goes, so a failed save leaves recovery to run again.
    krabb_file::save(root.join(OUTCOME), serde_json::to_vec(&outcome)?)?;
    clear_intent(root)?;

    if starts {
        start(service)?;
    }

    Ok(Some(outcome))
}

/// Whether recovery may act on POINT without asking. A manual reinstall of another release, or
/// a record whose versions or paths an update could not have written, is left to
/// `krabb update --rollback`, which asks first.
fn automatic(point: &snapshot::Point, version: &str) -> bool {
    let versions = releases::valid_version(&point.from) && releases::valid_version(&point.to);
    let installed = version == point.from || version == point.to;

    let suffix = std::env::consts::EXE_SUFFIX;
    let names = [format!("krabb{suffix}"), format!("krabb-daemon{suffix}")];

    // Windows and macOS file systems ignore case, so a name may come back as it was typed.
    let same = |name: &str, expected: &str| {
        if cfg!(any(windows, target_os = "macos")) {
            name.eq_ignore_ascii_case(expected)
        } else {
            name == expected
        }
    };

    let binaries = point.binaries.iter().all(|binary| {
        binary.path.is_absolute()
            && binary
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| names.iter().any(|expected| same(name, expected)))
    });

    let home = point.home.iter().all(|entry| {
        let path = Path::new(&entry.path);

        !entry.path.is_empty()
            && path.components().all(|part| matches!(part, std::path::Component::Normal(_)))
    });

    versions && installed && binaries && home
}

fn outcome_of(from: &str, to: &str, kept: &str, restored: bool) -> Outcome {
    Outcome { from: from.into(), to: to.into(), kept: kept.into(), at: unix_seconds(), restored }
}

/// Starts the installed service unless it already runs; a missing definition needs nothing.
fn start(service: &Path) -> Result<(), PluginError> {
    if service.is_file() && !running()? {
        service_action_with(service, ServiceAction::Start, ServiceReport::nested(true))?;
    }

    Ok(())
}

/// Recovers before a command runs, so it never acts on a half-updated installation.
///
/// It runs only when an update left something to settle, says what it did once, and stops the
/// command when this process is the binary recovery replaced.
pub(crate) async fn before_command(
    root: &Path,
    service: &Path,
    json: bool,
) -> Result<(), PluginError> {
    // Only an update creates this folder, so a fresh home is never created here.
    if !root.join("updates").is_dir() {
        return Ok(());
    }

    let outcome = match recover(root, service, Caller::Command).await {
        Ok(outcome) => outcome,
        Err(error) if error.to_string() == BUSY => return Err(error),

        Err(error) => {
            warn!(error = %diagnostic(sentence(error.to_string())), "Update recovery failed.");

            if !json {
                eprintln!("{}", recovery_failure(&error));
            }

            // Recovery may have replaced this binary before a later step failed, such as
            // starting the service; the command must not run on the restored home.
            if let Some(outcome) = outcome(root).filter(|outcome| outcome.kept != VERSION) {
                return Err(format!("Run the command again to use Krabb {}.", outcome.kept).into());
            }

            return Ok(());
        }
    };

    let Some(outcome) = outcome else {
        return Ok(());
    };

    info!(kept = %outcome.kept, to = %outcome.to, "Recovered an interrupted update.");

    if !json {
        eprintln!("{}", outcome.notice());
    }

    if outcome.kept != VERSION {
        return Err(format!("Run the command again to use Krabb {}.", outcome.kept).into());
    }

    Ok(())
}

/// A recovery failure as a warning with one next action, the error's own when it has one.
fn recovery_failure(error: &PluginError) -> String {
    let error = sentence(error.to_string());

    if error.lines().last().is_some_and(|line| line.contains('`')) {
        return format!("The interrupted update could not be settled: {error}");
    }

    format!(
        "The interrupted update could not be settled: {error}\nRestore the previous version \
         with `krabb update --rollback`."
    )
}

/// Settles an interrupted update before a new one starts; the caller holds the update lock.
pub(super) async fn before_update(
    root: &Path,
    service: &Path,
    json: bool,
) -> Result<(), PluginError> {
    if !pending(root) {
        return Ok(());
    }

    let Some(finish) = held(root, FINISH_LOCK)? else {
        return Err(BUSY.into());
    };

    let outcome = recover_locked(root, service, VERSION, Caller::Command).await?;

    drop(finish);

    let Some(outcome) = outcome else {
        return Ok(());
    };

    if !json {
        eprintln!("{}", outcome.notice());
    }

    if outcome.kept != VERSION {
        return Err(
            format!("Run `krabb update` again to update from Krabb {}.", outcome.kept).into()
        );
    }

    Ok(())
}
