use std::{
    collections::BTreeMap,
    fs::File,
    io,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize)]
struct Catalog {
    roots: BTreeMap<String, BTreeMap<PathBuf, String>>,
}

/// A locked provenance catalog. Unknown files are private until explicitly assigned an owner.
pub struct Ownership {
    lock: File,
    path: PathBuf,
    root: String,
    catalog: Catalog,
}

impl Drop for Ownership {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
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
            lock,
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
            .filter_map(|(path, owner)| path.to_str().map(|path| (path, owner.as_str())))
    }

    pub fn owner(&self, path: &Path) -> Option<&str> {
        self.catalog.roots.get(&self.root)?.get(path).map(String::as_str)
    }

    /// Drops entries under `prefix` in every catalogued root whose files no longer exist.
    ///
    /// It only reads metadata, so live files are never touched. Returns how many entries it removed.
    pub fn prune(&mut self, prefix: &Path) -> io::Result<usize> {
        let mut removed = 0;

        for (root, entries) in &mut self.catalog.roots {
            let root = Path::new(root);
            let before = entries.len();

            entries.retain(|path, _| {
                !path.starts_with(prefix) || std::fs::symlink_metadata(root.join(path)).is_ok()
            });

            removed += before - entries.len();
        }

        if removed > 0 {
            self.catalog.roots.retain(|_, entries| !entries.is_empty());
            self.save()?;
        }

        Ok(removed)
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
            .insert(PathBuf::from(path), owner.to_owned());

        self.save()
    }

    fn save(&self) -> io::Result<()> {
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
    fn dropping_ownership_releases_the_lock_even_with_an_inherited_descriptor() {
        let home =
            std::env::temp_dir().join(format!("krabb-ownership-inherited-{}", std::process::id()));

        let _ = std::fs::remove_dir_all(&home);
        let root = home.join("workspace");

        std::fs::create_dir_all(&root).unwrap();

        let mut ownership = Ownership::open(&home, &root).unwrap();

        ownership.assign(Path::new("notes.txt"), "channels.telegram").unwrap();

        let inherited = ownership.lock.try_clone().unwrap();

        assert!(Ownership::open(&home, &root).is_err());

        drop(ownership);

        let mut replacement = Ownership::open(&home, &root).unwrap();

        assert_eq!(replacement.owner(Path::new("notes.txt")), Some("channels.telegram"));

        replacement.assign(Path::new("notes.txt"), "channels.discord").unwrap();
        drop(inherited);

        assert!(Ownership::open(&home, &root).is_err());

        drop(replacement);

        assert_eq!(
            Ownership::open(&home, &root).unwrap().owner(Path::new("notes.txt")),
            Some("channels.discord")
        );

        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn ownership_matches_native_paths_across_reloads() {
        let home =
            std::env::temp_dir().join(format!("krabb-ownership-paths-{}", std::process::id()));

        let _ = std::fs::remove_dir_all(&home);
        let root = home.join("workspace");
        let relative = Path::new("attachments/channels.telegram/voice");

        std::fs::create_dir_all(root.join(relative.parent().unwrap())).unwrap();
        std::fs::write(root.join(relative), "fixture").unwrap();

        let root = std::fs::canonicalize(root).unwrap();
        let voice = std::fs::canonicalize(root.join(relative)).unwrap();
        let native = voice.strip_prefix(&root).unwrap();

        Ownership::open(&home, &root).unwrap().assign(relative, "channels.telegram").unwrap();

        let mut ownership = Ownership::open(&home, &root).unwrap();

        assert_eq!(ownership.owner(native), Some("channels.telegram"));

        ownership.assign(native, "channels.discord").unwrap();

        assert_eq!(ownership.owner(relative), Some("channels.discord"));
        assert_eq!(ownership.entries().count(), 1);

        drop(ownership);

        assert_eq!(
            Ownership::open(&home, &root).unwrap().owner(relative),
            Some("channels.discord")
        );

        #[cfg(unix)]
        {
            assert!(
                Ownership::open(&home, &root)
                    .unwrap()
                    .owner(Path::new(r"attachments\channels.telegram\voice"))
                    .is_none()
            );
        }

        std::fs::remove_dir_all(home).unwrap();
    }

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

    #[test]
    fn prune_drops_only_missing_attachment_entries() {
        let home =
            std::env::temp_dir().join(format!("krabb-ownership-prune-{}", std::process::id()));

        let _ = std::fs::remove_dir_all(&home);

        let root = home.join("workspace");
        let attachments = root.join("attachments/channels.telegram");

        std::fs::create_dir_all(&attachments).unwrap();
        std::fs::write(attachments.join("kept.bin"), "kept").unwrap();
        std::fs::write(attachments.join("gone.bin"), "gone").unwrap();

        let mut ownership = Ownership::open(&home, &root).unwrap();

        for path in [
            "attachments/channels.telegram/kept.bin",
            "attachments/channels.telegram/gone.bin",
            "notes.txt",
        ] {
            ownership.assign(Path::new(path), "channels.telegram").unwrap();
        }

        std::fs::remove_file(attachments.join("gone.bin")).unwrap();

        assert_eq!(ownership.prune(Path::new("attachments")).unwrap(), 1);
        assert_eq!(ownership.prune(Path::new("attachments")).unwrap(), 0);

        drop(ownership);

        let reopened = Ownership::open(&home, &root).unwrap();

        assert!(reopened.owner(Path::new("attachments/channels.telegram/kept.bin")).is_some());
        assert!(reopened.owner(Path::new("attachments/channels.telegram/gone.bin")).is_none());
        assert!(reopened.owner(Path::new("notes.txt")).is_some());
        assert!(attachments.join("kept.bin").exists());

        drop(reopened);

        let _ = std::fs::remove_dir_all(home);
    }
}
