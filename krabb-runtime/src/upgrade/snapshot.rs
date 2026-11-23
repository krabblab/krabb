//! The retained recovery point: previous binaries plus home configuration and state.
//!
//! Credentials are never copied, so restoring cannot reactivate removed secrets. Plugins
//! uninstalled after the point are recorded beside it, so restoring keeps their grants revoked.

use serde::{Deserialize, Serialize};

use krabb_migrations::recovery::{self, Entry};

use super::*;

const DIRECTORY: &str = "updates/recovery";
const POINT: &str = "point.json";
const POINT_LIMIT: u64 = 1024 * 1024;
const REVOKED: &str = "revoked.json";
const REVOKED_LIMIT: u64 = 64 * 1024;
const REVOKED_PLUGINS: usize = 256;
const INVALID: &str = "The record of plugins uninstalled since the update is invalid.";
const PLUGINS_LOCK: &str = ".plugins.lock";
const PLUGINS_WAIT: Duration = Duration::from_secs(30);
const EXCLUDED: &[&str] = &["media", "workspaces"];

/// Recovery point record; a binary restored from it must be able to read it.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Point {
    pub from: String,
    pub to: String,
    pub created: u64,
    pub accepted: bool,
    pub service: bool,
    pub binaries: Vec<Binary>,
    pub home: Vec<Entry>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Binary {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
}

pub(super) fn store(root: &Path) -> PathBuf {
    root.join(DIRECTORY)
}

pub(super) fn load(root: &Path) -> Result<Option<Point>, PluginError> {
    let Some(bytes) = krabb_file::load(store(root).join(POINT), POINT_LIMIT)? else {
        return Ok(None);
    };

    Ok(Some(serde_json::from_slice(&bytes).map_err(|_| "The update recovery point is invalid.")?))
}

/// Copies binaries and home state, verifies them, and commits the record last.
pub(super) fn create(
    root: &Path,
    from: &str,
    to: &str,
    binaries: &[PathBuf],
    service: bool,
) -> Result<Point, PluginError> {
    let directory = store(root);

    // Uninstall records itself only once a point exists, so it must not run while one is taken.
    let _plugins = crate::lock_at_timeout(root, PLUGINS_LOCK, PLUGINS_WAIT)?;

    let result = (|| -> Result<Point, PluginError> {
        recovery::remove(&directory)?;

        let home = recovery::save(root, &directory.join("home"), &paths(root)?)?;
        let mut saved = Vec::with_capacity(binaries.len());

        for (index, path) in binaries.iter().enumerate() {
            let (parent, name) = split(path)?;
            let entries =
                recovery::save(parent, &directory.join("bin").join(index.to_string()), &[name])?;

            saved.push(Binary { path: path.clone(), entries });
        }

        let point = Point {
            from: from.to_owned(),
            to: to.to_owned(),
            created: unix_seconds(),
            accepted: false,
            service,
            binaries: saved,
            home,
        };

        write(root, &point)?;

        Ok(point)
    })();

    if result.is_err() {
        let _ = recovery::remove(&directory);
    }

    result
}

pub(super) fn accept(root: &Path, point: &mut Point) -> Result<(), PluginError> {
    point.accepted = true;

    write(root, point)
}

/// Puts the previous binaries and home state back, then the service as it was.
pub(super) async fn restore(
    root: &Path,
    point: &Point,
    service: &Path,
    json: bool,
) -> Result<(), PluginError> {
    let directory = store(root);
    let uninstalled = revoked(root)?;
    let _plugins = crate::lock_at_timeout(root, PLUGINS_LOCK, PLUGINS_WAIT)?;

    if service.is_file() && running()? {
        service_action_with(service, ServiceAction::Stop, json)?;
    }

    released(root).await?;

    for (index, binary) in point.binaries.iter().enumerate() {
        let (_, name) = split(&binary.path)?;
        let saved = directory.join("bin").join(index.to_string()).join(&name);

        if binary.entries.iter().any(|entry| entry.present) {
            stage::swap(&binary.path, &saved)?;
        }
    }

    recovery::restore(root, &directory.join("home"), &point.home)?;

    for id in &uninstalled {
        crate::revoke::restored(root, id)?;
    }

    recovery::remove(&root.join(krabb_migrations::JOURNAL))?;
    recovery::remove(&root.join(krabb_migrations::BACKUP))?;

    if point.service && service.is_file() {
        service_action_with(service, ServiceAction::Start, json)?;
    }

    let _ = recovery::remove(&directory);

    Ok(())
}

/// Explicit recovery; it needs neither the new daemon nor network access.
pub(super) async fn rollback(
    root: &Path,
    service: &Path,
    yes: bool,
    json: bool,
) -> Result<(), PluginError> {
    let Some(point) = load(root)? else {
        return Err(
            "No update recovery point is retained, so there is nothing to roll back.".into()
        );
    };

    let mut question = if point.accepted {
        format!(
            "Restore Krabb {} from the recovery point taken before updating to {}?\n\
             Conversations, tasks, settings, and plugins changed since then will be replaced, and \
             saved credentials are kept.",
            point.from, point.to
        )
    } else {
        format!("Restore Krabb {} after the unfinished update to {}?", point.from, point.to)
    };

    let returning = revoked(root)?
        .into_iter()
        .filter(|id| store(root).join("home/plugins").join(id).is_dir())
        .collect::<Vec<_>>();

    if !returning.is_empty() {
        question.push_str(&format!(
            "\nPlugins uninstalled since then come back without their sharing grants or \
             credentials: {}.",
            returning.join(", ")
        ));
    }

    if !confirm_action(yes, json, &question)? {
        print_cancelled("Rollback", json);
        return Ok(());
    }

    let spinner = terminal::progress(format!("Restoring Krabb {}...", point.from), json);

    restore(root, &point, service, json).await?;
    drop(spinner);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "action": "rollback",
                "status": "restored",
                "version": point.from,
            }))?
        );
    } else {
        println!("Restored Krabb {}.", point.from);
    }

    Ok(())
}

/// Status summary of the retained recovery point, if any.
pub(super) fn summary(root: &Path) -> Option<serde_json::Value> {
    let point = load(root).ok().flatten()?;

    Some(serde_json::json!({
        "from": point.from,
        "to": point.to,
        "created": point.created,
        "accepted": point.accepted,
        "path": store(root),
    }))
}

/// Records a plugin being uninstalled while a recovery point exists.
///
/// It runs before any grant is removed, so a failed uninstall errs toward revoking access. It
/// never blocks an uninstall: an unreadable record already makes rollback refuse, and one entry
/// past the limit does the same.
pub(crate) fn uninstalled(root: &Path, id: &str) -> Result<(), PluginError> {
    if !store(root).join(POINT).is_file() {
        return Ok(());
    }

    let Ok(mut ids) = revoked(root) else {
        return Ok(());
    };

    if ids.iter().any(|known| known == id) {
        return Ok(());
    }

    ids.push(id.to_owned());
    krabb_file::save(store(root).join(REVOKED), serde_json::to_vec_pretty(&ids)?)?;

    Ok(())
}

fn revoked(root: &Path) -> Result<Vec<String>, PluginError> {
    let Some(bytes) = krabb_file::load(store(root).join(REVOKED), REVOKED_LIMIT)? else {
        return Ok(Vec::new());
    };

    let ids: Vec<String> = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;

    if ids.iter().any(|id| !crate::valid(id)) {
        return Err(INVALID.into());
    }

    if ids.len() > REVOKED_PLUGINS {
        return Err(
            "Too many plugins were uninstalled since the update to roll back safely.".into()
        );
    }

    Ok(ids)
}

fn write(root: &Path, point: &Point) -> Result<(), PluginError> {
    krabb_file::save(store(root).join(POINT), serde_json::to_vec_pretty(point)?)?;

    Ok(())
}

fn paths(root: &Path) -> Result<Vec<String>, PluginError> {
    let mut paths = ["config.toml", "plugins.lock", krabb_migrations::MARKER, "plugins"]
        .map(str::to_owned)
        .to_vec();

    match std::fs::read_dir(root.join("data")) {
        Ok(entries) => {
            for entry in entries {
                let name = entry?.file_name().to_string_lossy().into_owned();

                if !EXCLUDED.contains(&name.as_str()) {
                    paths.push(format!("data/{name}"));
                }
            }
        }

        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    paths.sort();

    Ok(paths)
}

fn split(path: &Path) -> Result<(&Path, String), PluginError> {
    let parent = path.parent().ok_or("Binary has no parent directory.")?;
    let name = path.file_name().ok_or("Binary has no file name.")?;

    Ok((parent, name.to_string_lossy().into_owned()))
}
