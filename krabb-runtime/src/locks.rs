//! Cross-process locks that guard the daemon, offline commands, and metadata.

use crate::*;

pub(crate) fn owner_lock(root: &Path) -> std::io::Result<DaemonLock> {
    ensure_real_directory(root)?;

    let legacy = if std::fs::symlink_metadata(root.join("daemon.lock")).is_ok() {
        Some(Box::new(lock_at(root, "daemon.lock")?))
    } else {
        None
    };

    let directory = krabb_file::directory(root, Path::new("run"))?;
    let mut lock = lock_at(&directory, "daemon.lock")?;

    lock._legacy = legacy;

    Ok(lock)
}

pub(crate) fn daemon_lock(root: &Path) -> std::io::Result<DaemonLock> {
    let lock = owner_lock(root)?;

    schema::migrate(root)?;

    Ok(lock)
}

pub(crate) fn offline_lock(root: &Path) -> std::io::Result<DaemonLock> {
    daemon_lock(root).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "The daemon owns the state; use the IPC client instead of opening state directly.",
            )
        } else {
            error
        }
    })
}

pub(crate) fn metadata_lock(root: &Path) -> std::io::Result<DaemonLock> {
    lock_at_timeout(root, ".metadata.lock", std::time::Duration::from_secs(5))
}

pub(crate) fn lock_at(root: &Path, name: &str) -> std::io::Result<DaemonLock> {
    lock_at_timeout(root, name, std::time::Duration::ZERO)
}

pub(crate) fn lock_at_timeout(
    root: &Path,
    name: &str,
    timeout: std::time::Duration,
) -> std::io::Result<DaemonLock> {
    std::fs::create_dir_all(root)?;

    let path = root.join(name);

    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Lock path cannot be a symbolic link.",
            ));
        }

        Ok(_) => {}

        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;

    let deadline = std::time::Instant::now() + timeout;

    loop {
        match file.try_lock() {
            Ok(()) => break,

            Err(std::fs::TryLockError::WouldBlock) => {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());

                if remaining.is_zero() {
                    return Err(std::io::Error::new(
                        if timeout.is_zero() {
                            std::io::ErrorKind::AlreadyExists
                        } else {
                            std::io::ErrorKind::TimedOut
                        },
                        format!("Lock {name} is already held."),
                    ));
                }

                std::thread::sleep(remaining.min(std::time::Duration::from_millis(10)));
            }

            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }

    file.set_len(0)?;
    use std::io::Write as _;
    writeln!(file, "{}", std::process::id())?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&path)?.permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(&path, permissions)?;
    }

    Ok(DaemonLock { _file: file, _legacy: None })
}
