//! Brings back a working installation after `krabb update` was cut off by a closed
//! terminal, a crash, or power loss.
//!
//! An update records its intent before it stops the service, then a recovery point before it
//! changes anything. The next CLI command or daemon start that finds either one, while no update
//! holds the update lock, finishes the update when the new host already activated, and restores
//! the previous installation otherwise.

use super::*;

const INTENT: &str = "updates/intent.json";
const OUTCOME: &str = "updates/recovered.json";
const LIMIT: u64 = 64 * 1024;

/// What an update was about to do, saved before it stops the service.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Intent {
    pub from: String,
    pub to: String,
    /// Whether the service was running, so recovery starts it again.
    pub service: bool,
}

/// What recovery did, kept for `krabb status` until the next update or rollback.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub from: String,
    pub to: String,
    /// The version installed after recovery: `to` when the update had finished.
    pub kept: String,
    pub at: u64,
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
        } else {
            format!(
                "The update to Krabb {} was interrupted, so Krabb {} was restored.",
                self.to, self.from
            )
        }
    }
}

pub(super) fn intend(root: &Path, intent: &Intent) -> Result<(), PluginError> {
    krabb_file::save(root.join(INTENT), serde_json::to_vec(intent)?)?;

    Ok(())
}

/// Forgets the intent once a recovery point records the same facts, or nothing changed.
pub(super) fn settle(root: &Path) -> Result<(), PluginError> {
    krabb_migrations::recovery::remove(&root.join(INTENT))?;

    Ok(())
}

/// Forgets a past recovery, because a new update or rollback replaces what it describes.
pub(super) fn forget(root: &Path) -> Result<(), PluginError> {
    krabb_migrations::recovery::remove(&root.join(OUTCOME))?;

    Ok(())
}

/// The last recovery, for `krabb status`.
pub(crate) fn outcome(root: &Path) -> Option<Outcome> {
    let bytes = krabb_file::load(root.join(OUTCOME), LIMIT).ok().flatten()?;

    serde_json::from_slice(&bytes).ok()
}

fn intent(root: &Path) -> Result<Option<Intent>, PluginError> {
    let Some(bytes) = krabb_file::load(root.join(INTENT), LIMIT)? else {
        return Ok(None);
    };

    Ok(Some(serde_json::from_slice(&bytes).map_err(|_| "The update intent record is invalid.")?))
}

/// Where recovery runs. The daemon is the service, so it never stops or starts it: it
/// restores the files and exits, and the service manager starts the restored daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Caller {
    Command,
    Daemon,
}

/// Recovers when no update is running, and says what it did.
pub(crate) async fn recover(root: &Path, caller: Caller) -> Result<Option<Outcome>, PluginError> {
    // Tests run on machines with a real service, which recovery must never stop or start.
    let service = if cfg!(test) { root.join("none.service") } else { service_path() };

    recover_at(root, &service, caller).await
}

pub(super) async fn recover_at(
    root: &Path,
    service: &Path,
    caller: Caller,
) -> Result<Option<Outcome>, PluginError> {
    let lock = match lock_at(root, ".update.lock") {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
        Err(error) => return Err(error.into()),
    };

    let outcome = recover_locked(root, service, VERSION, caller).await;

    drop(lock);

    outcome
}

/// Recovery once the caller holds the update lock; VERSION is the running host's version.
pub(super) async fn recover_locked(
    root: &Path,
    service: &Path,
    version: &str,
    caller: Caller,
) -> Result<Option<Outcome>, PluginError> {
    let manages = caller == Caller::Command;

    let outcome = match snapshot::load(root)? {
        Some(mut point) if !point.accepted => {
            let staged = root.join("updates").join(format!("v{}", point.to));
            let finished = version == point.to && finish::report(&staged).is_some();

            if finished {
                snapshot::accept(root, &mut point)?;

                if manages && point.service {
                    start(service)?;
                }

                Some(outcome_of(&point.from, &point.to, &point.to))
            } else {
                snapshot::restore_with(root, &point, service, true, manages).await?;

                Some(outcome_of(&point.from, &point.to, &point.from))
            }
        }

        _ => match intent(root)? {
            // The update stopped before its recovery point, so only the service changed.
            Some(intent) => {
                if manages && intent.service {
                    start(service)?;
                }

                Some(outcome_of(&intent.from, &intent.to, &intent.from))
            }

            None => None,
        },
    };

    if let Some(outcome) = &outcome {
        settle(root)?;
        krabb_file::save(root.join(OUTCOME), serde_json::to_vec(outcome)?)?;
    }

    Ok(outcome)
}

fn outcome_of(from: &str, to: &str, kept: &str) -> Outcome {
    Outcome { from: from.into(), to: to.into(), kept: kept.into(), at: now() }
}

/// Starts the installed service unless it already runs; a missing definition needs nothing.
fn start(service: &Path) -> Result<(), PluginError> {
    if service.is_file() && !running()? {
        service_action_with(service, ServiceAction::Start, ServiceReport::nested(true))?;
    }

    Ok(())
}
