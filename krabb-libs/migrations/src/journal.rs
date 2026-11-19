use std::{io, path::Path};

use serde::{Deserialize, Serialize};

use crate::{Error, recovery::Entry, relative};

/// Journal that marks a migration in progress; its presence blocks normal state access.
pub const JOURNAL: &str = ".migration.json";

const LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Journal {
    pub from: u32,
    pub to: u32,
    pub paths: Vec<Entry>,
}

pub(crate) fn read(root: &Path) -> Result<Option<Journal>, Error> {
    let Some(bytes) = krabb_file::load(root.join(JOURNAL), LIMIT)? else {
        return Ok(None);
    };

    let journal = serde_json::from_slice::<Journal>(&bytes).map_err(|_| {
        Error::Invalid(format!(
            "The migration journal {JOURNAL} is not valid; restore the home from a backup."
        ))
    })?;

    for entry in &journal.paths {
        relative(&entry.path)?;
    }

    Ok(Some(journal))
}

pub(crate) fn write(root: &Path, journal: &Journal) -> Result<(), Error> {
    let bytes = serde_json::to_vec(journal).map_err(io::Error::other)?;

    krabb_file::save(root.join(JOURNAL), bytes)?;

    Ok(())
}

pub(crate) fn remove(root: &Path) -> Result<(), Error> {
    match std::fs::remove_file(root.join(JOURNAL)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
