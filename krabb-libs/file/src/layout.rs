use std::path::{Path, PathBuf};

/// Keep existing state usable until an offline migration moves it to the current layout.
pub fn location(root: &Path, relative: &str, legacy: &str) -> PathBuf {
    let current = root.join(relative);
    let previous = root.join(legacy);

    if std::fs::symlink_metadata(&current).is_err() && std::fs::symlink_metadata(&previous).is_ok()
    {
        previous
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_current_state_and_preserves_existing_legacy_state() {
        let root = std::env::temp_dir().join(format!("krabb-layout-{}", std::process::id()));

        std::fs::create_dir_all(&root).unwrap();

        let current = root.join("new");

        assert_eq!(location(&root, "new", "old"), current);

        std::fs::write(root.join("old"), "legacy").unwrap();

        assert_eq!(location(&root, "new", "old"), root.join("old"));

        std::fs::write(&current, "current").unwrap();

        assert_eq!(location(&root, "new", "old"), current);

        std::fs::remove_dir_all(root).unwrap();
    }
}
