use crate::*;

/// A running process never executes the mutable installed path, including on Windows.
pub(crate) struct Image {
    root: PathBuf,
    lease: Option<DaemonLock>,
    pub binary: PathBuf,
}

impl Image {
    pub fn new(home: &Path, source: &Path) -> Result<Self, PluginError> {
        let parent = home.join("run/plugin-images");
        std::fs::create_dir_all(&parent)?;

        if !parent.canonicalize()?.starts_with(home.canonicalize()?) {
            return Err("Plugin image directory escapes the Krabb home.".into());
        }

        let _creation = lock_at_timeout(&parent, ".images.lock", Duration::from_secs(5))?;
        let root = parent.join(format!("{}-{}", std::process::id(), now()));
        std::fs::create_dir(&root)?;
        let lease = lock_at(&root, ".lease")?;
        let image = Self {
            binary: root.join(source.file_name().ok_or("Invalid plugin path.")?),
            root,
            lease: Some(lease),
        };

        copy_binary(source, &image.binary)?;
        Ok(image)
    }

    pub fn clean(home: &Path) -> std::io::Result<()> {
        Self::clean_with(home, |path| std::fs::remove_dir_all(path))
    }

    pub(super) fn clean_with(
        home: &Path,
        remove: impl Fn(&Path) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        let parent = home.join("run/plugin-images");

        if !parent.exists() {
            return Ok(());
        }

        if !parent.canonicalize()?.starts_with(home.canonicalize()?) {
            return Err(std::io::Error::other("Plugin image directory escapes the Krabb home."));
        }

        let _creation = lock_at_timeout(&parent, ".images.lock", Duration::from_secs(5))?;

        for entry in std::fs::read_dir(&parent)? {
            let entry = entry?;

            if !entry.file_type()?.is_dir() {
                continue;
            }

            let path = entry.path();

            match lock_at(&path, ".lease") {
                Ok(lease) => {
                    drop(lease);

                    if let Err(error) = remove(&path) {
                        warn!(
                            error = %diagnostic(error.to_string()),
                            "A stale plugin image is in use; cleanup will retry at next start."
                        );
                    }
                }

                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}

                // Windows can deny a lease for a moment after its process exits.
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    warn!(
                        error = %diagnostic(error.to_string()),
                        "A stale plugin image is busy; cleanup will retry at next start."
                    );
                }

                Err(error) => return Err(error),
            }
        }

        Ok(())
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        self.lease.take();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
