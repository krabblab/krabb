use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

/// Create a relative directory beneath an existing root without following symlinks.
pub fn directory(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let mut path = fs::canonicalize(root)?;

    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Directory must be relative without traversal.",
            ));
        };

        path.push(name);

        match fs::create_dir(&path) {
            Ok(()) => {}

            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }

        let metadata = fs::symlink_metadata(&path)?;

        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Directory cannot be a symbolic link or file.",
            ));
        }
    }

    Ok(path)
}

/// Select a directory under the configured root and create private state beneath it.
pub fn confined(root: &Path, selected: &Path, relative: &Path) -> io::Result<PathBuf> {
    let root = fs::canonicalize(root)?;
    let selected = fs::canonicalize(selected)?;

    if !selected.is_dir() || !selected.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Selected storage leaves the configured root.",
        ));
    }

    directory(&selected, relative)
}

/// Resolve private host state for a checkout without storing state in its content directory.
pub fn workspace_state(home: &Path, selected: &Path) -> io::Result<PathBuf> {
    let home = fs::canonicalize(home)?;
    let selected = fs::canonicalize(selected)?;
    let parent = directory(&home, Path::new("worktrees"))?;

    if selected.parent() != Some(parent.as_path()) || !selected.join(".git").is_file() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Invalid private workspace."));
    }

    let id = selected.file_name().ok_or_else(|| io::Error::other("Missing workspace identity."))?;

    directory(&home, &Path::new("data/workspaces/conversations").join(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_state_is_outside_content_and_rejects_other_workspaces() {
        let home = std::env::temp_dir().join(format!("krabb-private-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);

        fs::create_dir_all(home.join("worktrees/chat")).unwrap();
        fs::create_dir_all(home.join("workspace")).unwrap();

        let chat = home.join("worktrees/chat");

        assert!(workspace_state(&home, &chat).is_err());

        fs::write(chat.join(".git"), "fixture").unwrap();

        assert_eq!(
            workspace_state(&home, &chat).unwrap(),
            home.join("data/workspaces/conversations/chat")
        );

        assert!(!chat.join(".krabb").exists());
        assert!(workspace_state(&home, &home.join("workspace")).is_err());

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn creates_nested_directories_and_rejects_unsafe_components() {
        let root = std::env::temp_dir().join(format!("krabb-directory-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        fs::create_dir(&root).unwrap();

        let path = directory(&root, Path::new(".krabb/media")).unwrap();

        assert!(path.is_dir());
        assert_eq!(directory(&root, Path::new(".krabb/media")).unwrap(), path);
        assert!(directory(&root, Path::new("../outside")).is_err());

        fs::write(root.join("file"), "x").unwrap();

        assert!(directory(&root, Path::new("file/child")).is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&root, root.join("link")).unwrap();

            assert!(directory(&root, Path::new("link/child")).is_err());
        }

        fs::remove_dir_all(root).unwrap();
    }
}
