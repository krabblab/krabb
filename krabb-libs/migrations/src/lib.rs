//! Ordered, journaled schema migrations for a Krabb home.
//!
//! The home schema is versioned separately from the application and protocol.
//! Step `n` of a [`Registry`] migrates schema `n` to `n + 1`; a home without a
//! marker is schema 0. Before a step runs, every path it declares is copied to
//! a verified recovery point and a journal is committed. An interrupted or
//! failed migration is restored from that recovery point before anything else
//! opens the home.

mod journal;
pub mod recovery;

use std::{
    fmt, fs, io,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};

pub use journal::JOURNAL;

/// File that records the schema of a Krabb home.
pub const MARKER: &str = "schema.json";
/// Directory that holds the recovery point of a running migration.
pub const BACKUP: &str = ".migration";

const LOCK: &str = ".migration.lock";
const MARKER_LIMIT: u64 = 4096;

/// One ordered transformation, owned by the component whose state it changes.
pub struct Step {
    pub name: &'static str,
    /// Home-relative files and directories the step may change.
    pub paths: &'static [&'static str],
    pub apply: fn(&Path) -> io::Result<()>,
}

/// Ordered steps; the index of each step is the schema it migrates from.
pub struct Registry {
    steps: &'static [Step],
}

impl Registry {
    pub const fn new(steps: &'static [Step]) -> Self {
        Self { steps }
    }

    pub fn latest(&self) -> u32 {
        u32::try_from(self.steps.len()).unwrap_or(u32::MAX)
    }
}

/// Pending work for a home, computed without changing it.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub from: u32,
    pub to: u32,
    pub steps: Vec<&'static str>,
    pub interrupted: bool,
}

#[derive(Debug)]
pub enum Error {
    Newer { found: u32, supported: u32 },
    Busy,
    Invalid(String),
    Step { name: &'static str, source: io::Error },
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Newer { found, supported } => write!(
                formatter,
                "This Krabb home uses schema {found}, but this Krabb supports up to schema \
                 {supported}; install a newer Krabb or restore a compatible backup."
            ),

            Self::Busy => formatter.write_str("Another Krabb process is migrating this home."),
            Self::Invalid(message) => formatter.write_str(message),

            Self::Step { name, source } => {
                write!(formatter, "Migration \"{name}\" failed and was rolled back: {source}")
            }

            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Step { source, .. } | Self::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<Error> for io::Error {
    fn from(error: Error) -> Self {
        let kind = match &error {
            Error::Newer { .. } | Error::Invalid(_) => io::ErrorKind::InvalidData,
            Error::Busy => io::ErrorKind::WouldBlock,
            Error::Step { source, .. } | Error::Io(source) => source.kind(),
        };

        io::Error::new(kind, error.to_string())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    schema: u32,
}

/// Reads the home schema; a home without a marker is schema 0.
pub fn version(root: &Path) -> Result<u32, Error> {
    let Some(bytes) = krabb_file::load(root.join(MARKER), MARKER_LIMIT)? else {
        return Ok(0);
    };

    serde_json::from_slice::<Marker>(&bytes)
        .map(|marker| marker.schema)
        .map_err(|_| Error::Invalid(format!("The home schema marker {MARKER} is not valid.")))
}

/// Describes pending work and rejects unsupported schemas before any change.
pub fn plan(root: &Path, registry: &Registry) -> Result<Plan, Error> {
    let pending = journal::read(root)?;

    let interrupted = pending.is_some();

    let from = match pending {
        Some(pending) => pending.from,
        None => version(root)?,
    };

    let to = registry.latest();

    if from > to {
        return Err(Error::Newer { found: from, supported: to });
    }

    let steps = registry.steps[from as usize..].iter().map(|step| step.name).collect();

    Ok(Plan { from, to, steps, interrupted })
}

/// Restores an interrupted migration, then applies every pending step.
///
/// Callers must hold exclusive ownership of the home's state.
pub fn migrate(root: &Path, registry: &Registry) -> Result<Plan, Error> {
    let _lock = lock(root)?;
    let interrupted = restore_unlocked(root)?;
    let mut plan = plan(root, registry)?;

    plan.interrupted = interrupted;

    if plan.from == plan.to {
        return Ok(plan);
    }

    let steps = &registry.steps[plan.from as usize..];
    let mut paths = vec![MARKER.to_owned()];

    for step in steps {
        for path in step.paths {
            relative(path)?;

            if !paths.iter().any(|known| known == path) {
                paths.push((*path).to_owned());
            }
        }
    }

    let entries = recovery::save(root, &root.join(BACKUP), &paths)?;

    journal::write(root, &journal::Journal { from: plan.from, to: plan.to, paths: entries })?;

    for step in steps {
        if let Err(source) = (step.apply)(root) {
            restore_unlocked(root)?;

            return Err(Error::Step { name: step.name, source });
        }
    }

    let marker = serde_json::to_vec(&Marker { schema: plan.to }).map_err(io::Error::other)?;

    if let Err(error) = krabb_file::save(root.join(MARKER), marker) {
        restore_unlocked(root)?;

        return Err(error.into());
    }

    journal::remove(root)?;
    recovery::remove(&root.join(BACKUP))?;

    Ok(plan)
}

/// Restores the recovery point of an interrupted migration; returns whether one existed.
pub fn restore(root: &Path) -> Result<bool, Error> {
    let _lock = lock(root)?;

    restore_unlocked(root)
}

fn restore_unlocked(root: &Path) -> Result<bool, Error> {
    let backup = root.join(BACKUP);

    let Some(journal) = journal::read(root)? else {
        recovery::remove(&backup)?;
        return Ok(false);
    };

    recovery::restore(root, &backup, &journal.paths)?;
    journal::remove(root)?;
    recovery::remove(&backup)?;

    Ok(true)
}

fn lock(root: &Path) -> Result<fs::File, Error> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(LOCK))?;

    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err(Error::Busy),
        Err(fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

fn relative(path: &str) -> Result<(), Error> {
    let valid = !path.is_empty()
        && Path::new(path).components().all(|component| matches!(component, Component::Normal(_)))
        && ![JOURNAL, BACKUP, LOCK].contains(&path.split(['/', '\\']).next().unwrap_or(path));

    if valid {
        Ok(())
    } else {
        Err(Error::Invalid(format!("Migration path {path} must stay inside the home.")))
    }
}

#[cfg(test)]
mod tests;
