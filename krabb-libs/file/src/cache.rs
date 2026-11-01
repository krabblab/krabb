use std::{
    collections::BTreeSet,
    fs, io,
    path::Path,
    time::{Duration, SystemTime},
};

/// Expire and evict regular cache files, preserving directories, links, and transaction dotfiles.
/// Call with room reserved for the incoming file before writing it. Errors must reject writes.
pub fn trim(root: &Path, bytes: u64, files: usize, ttl: Duration) -> io::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    let cutoff = SystemTime::now().checked_sub(ttl).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut retained = BTreeSet::new();
    let mut total = 0_u64;
    let mut protected = 0_usize;

    for entry in entries {
        let entry = entry?;

        if !entry.file_type()?.is_file() {
            continue;
        }

        let metadata = entry.metadata()?;
        let modified = metadata.modified()?;
        let transaction = entry.file_name().to_string_lossy().starts_with('.');

        if !transaction && modified < cutoff {
            fs::remove_file(entry.path())?;
            continue;
        }

        total = total.saturating_add(metadata.len());

        if transaction {
            protected = protected.saturating_add(1);
        } else {
            retained.insert((modified, entry.path(), metadata.len()));
        }

        // Keep memory bounded by the file budget even when migrating an oversized cache.
        while total > bytes || retained.len().saturating_add(protected) > files {
            let Some((_, path, size)) = retained.pop_first() else {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "Cache transactions occupy the available storage.",
                ));
            };

            fs::remove_file(path)?;
            total = total.saturating_sub(size);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_bounds_bytes_files_and_age_without_touching_pins() {
        let root = std::env::temp_dir().join(format!("krabb-cache-trim-{}", std::process::id()));

        fs::create_dir_all(root.join("pinned")).unwrap();
        fs::write(root.join("pinned/image.bin"), [0; 30]).unwrap();

        let now = SystemTime::now();

        for index in 0..10 {
            let path = root.join(format!("{index}.bin"));

            fs::write(&path, [0; 6]).unwrap();
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(now - Duration::from_secs(10 - index)))
                .unwrap();
        }

        trim(&root, 12, 10, Duration::from_secs(8)).unwrap();

        assert!(!root.join("0.bin").exists());
        assert!(root.join("8.bin").is_file());
        assert!(root.join("9.bin").is_file());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 3);

        trim(&root, 100, 1, Duration::from_secs(60)).unwrap();

        assert!(!root.join("8.bin").exists());
        assert!(root.join("9.bin").is_file());

        fs::write(root.join("empty.bin"), []).unwrap();
        trim(&root, 0, 0, Duration::from_secs(60)).unwrap();

        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(fs::read(root.join("pinned/image.bin")).unwrap().len(), 30);
        assert!(trim(&root.join("pinned/image.bin"), 1, 1, Duration::ZERO).is_err());

        fs::write(root.join(".transaction"), "journal").unwrap();
        trim(&root, 100, 1, Duration::ZERO).unwrap();

        assert!(trim(&root, 0, 0, Duration::ZERO).is_err());
        assert_eq!(fs::read_to_string(root.join(".transaction")).unwrap(), "journal");

        fs::remove_dir_all(&root).unwrap();
        trim(&root, 1, 1, Duration::ZERO).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cache_does_not_follow_links() {
        let root = std::env::temp_dir().join(format!("krabb-cache-links-{}", std::process::id()));

        fs::create_dir_all(root.join("cache")).unwrap();

        let target = root.join("target");

        fs::write(&target, "preserve").unwrap();
        std::os::unix::fs::symlink(&target, root.join("cache/link")).unwrap();
        trim(&root.join("cache"), 0, 0, Duration::ZERO).unwrap();

        assert_eq!(fs::read_to_string(target).unwrap(), "preserve");
        assert!(root.join("cache/link").symlink_metadata().unwrap().file_type().is_symlink());

        fs::remove_dir_all(root).unwrap();
    }
}
