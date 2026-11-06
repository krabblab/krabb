use std::{
    collections::BTreeMap,
    fs::File,
    io,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize)]
struct Catalog {
    roots: BTreeMap<String, BTreeMap<String, String>>,
}

/// A locked provenance catalog. Unknown files are private until explicitly assigned an owner.
pub struct Ownership {
    _lock: File,
    path: PathBuf,
    root: String,
    catalog: Catalog,
}

impl Ownership {
    pub fn open(home: &Path, root: &Path) -> io::Result<Self> {
        let directory = crate::directory(home, Path::new("data/workspaces"))?;
        let lock_path = directory.join("files.lock");

        if std::fs::symlink_metadata(&lock_path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Invalid ownership lock."));
        }

        let lock =
            File::options().create(true).truncate(false).read(true).write(true).open(lock_path)?;

        lock.try_lock().map_err(|_| {
            io::Error::new(io::ErrorKind::WouldBlock, "File ownership is busy; retry.")
        })?;

        let path = directory.join("files.json");

        let catalog = crate::load(&path, 16 * 1024 * 1024)?
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()
            .map_err(io::Error::other)?
            .unwrap_or_default();

        Ok(Self {
            _lock: lock,
            path,
            root: std::fs::canonicalize(root)?.to_string_lossy().into_owned(),
            catalog,
        })
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.catalog
            .roots
            .get(&self.root)
            .into_iter()
            .flat_map(|entries| entries.iter())
            .map(|(path, owner)| (path.as_str(), owner.as_str()))
    }

    pub fn owner(&self, path: &Path) -> Option<&str> {
        self.catalog.roots.get(&self.root)?.get(path.to_str()?).map(String::as_str)
    }

    pub fn assign(&mut self, path: &Path, owner: &str) -> io::Result<()> {
        if path.as_os_str().is_empty()
            || path.components().any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Ownership requires a relative file path.",
            ));
        }

        let path = path.to_str().ok_or_else(|| io::Error::other("File path must be UTF-8."))?;

        self.catalog
            .roots
            .entry(self.root.clone())
            .or_default()
            .insert(path.to_owned(), owner.to_owned());

        let bytes = serde_json::to_vec(&self.catalog).map_err(io::Error::other)?;

        if bytes.len() > 16 * 1024 * 1024 {
            return Err(io::Error::other("File ownership catalog is full."));
        }

        crate::save(&self.path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_is_durable_root_specific_locked_and_traversal_safe() {
        let home = std::env::temp_dir().join(format!("krabb-ownership-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);

        std::fs::create_dir_all(home.join("workspace")).unwrap();

        let root = home.join("workspace");
        let mut ownership = Ownership::open(&home, &root).unwrap();

        assert!(Ownership::open(&home, &root).is_err());
        assert!(ownership.assign(Path::new("../escape"), "global").is_err());

        ownership.assign(Path::new("notes.txt"), "channels.telegram").unwrap();

        assert_eq!(ownership.owner(Path::new("notes.txt")), Some("channels.telegram"));
        assert_eq!(ownership.entries().count(), 1);

        drop(ownership);

        assert_eq!(
            Ownership::open(&home, &root).unwrap().owner(Path::new("notes.txt")),
            Some("channels.telegram")
        );

        assert!(Ownership::open(&home, &home).unwrap().owner(Path::new("notes.txt")).is_none());

        std::fs::remove_dir_all(home).unwrap();
    }
}
