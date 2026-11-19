//! Verified copies of home paths that can be put back exactly as they were.

use std::{
    fs,
    io::{self, Read},
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::Error;

const CHUNK: usize = 64 * 1024;

/// A root-relative path and whether it existed when the recovery point was taken.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub present: bool,
}

/// Copies each path below `root` into a new private `store` and verifies every file.
pub fn save(root: &Path, store: &Path, paths: &[String]) -> Result<Vec<Entry>, Error> {
    remove(store)?;

    let mut builder = fs::DirBuilder::new();

    builder.recursive(true);

    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);

    builder.create(store)?;

    let mut entries = Vec::with_capacity(paths.len());

    for path in paths {
        let source = root.join(path);

        let present = match fs::symlink_metadata(&source) {
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };

        if present {
            copy(&source, &store.join(path))?;
        }

        entries.push(Entry { path: path.clone(), present });
    }

    Ok(entries)
}

/// Puts every recorded path below `root` back exactly as it was in `store`.
pub fn restore(root: &Path, store: &Path, entries: &[Entry]) -> Result<(), Error> {
    for entry in entries {
        if entry.present && fs::symlink_metadata(store.join(&entry.path)).is_err() {
            return Err(Error::Invalid(format!(
                "The recovery point is missing {}; restore the home from a backup.",
                entry.path
            )));
        }
    }

    for entry in entries {
        let live = root.join(&entry.path);

        remove(&live)?;

        if entry.present {
            copy(&store.join(&entry.path), &live)?;
        }
    }

    Ok(())
}

/// Removes a file, link, or directory tree; a missing path is not an error.
pub fn remove(path: &Path) -> Result<(), Error> {
    let result = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) => Err(error),
    };

    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn copy(source: &Path, destination: &Path) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(source)?;

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }

    if metadata.file_type().is_symlink() {
        return link(&fs::read_link(source)?, destination).map_err(|error| {
            Error::Invalid(format!(
                "The symbolic link {} could not be copied: {error}",
                source.display()
            ))
        });
    }

    if metadata.is_dir() {
        fs::create_dir(destination)?;

        for entry in fs::read_dir(source)? {
            let entry = entry?;

            copy(&entry.path(), &destination.join(entry.file_name()))?;
        }

        fs::set_permissions(destination, metadata.permissions())?;

        return Ok(());
    }

    let mut input = fs::File::open(source)?;
    let mut output = create(destination)?;

    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    fs::set_permissions(destination, metadata.permissions())?;

    if !same(source, destination)? {
        return Err(Error::Invalid(format!(
            "The recovery copy of {} did not match the original.",
            source.display()
        )));
    }

    Ok(())
}

/// Opens the copy for writing; Windows can only flush a file through a writable handle.
fn create(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();

    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);

    options.open(path)
}

#[cfg(unix)]
fn link(target: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(windows)]
fn link(target: &Path, destination: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, destination)
}

fn same(left: &Path, right: &Path) -> io::Result<bool> {
    let mut left = fs::File::open(left)?;
    let mut right = fs::File::open(right)?;

    if left.metadata()?.len() != right.metadata()?.len() {
        return Ok(false);
    }

    let mut first = vec![0; CHUNK];
    let mut second = vec![0; CHUNK];

    loop {
        let count = left.read(&mut first)?;

        if count == 0 {
            return Ok(true);
        }

        right.read_exact(&mut second[..count])?;

        if first[..count] != second[..count] {
            return Ok(false);
        }
    }
}
