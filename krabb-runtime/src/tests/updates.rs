use crate::*;
use std::fs;

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn lock_commit_failure_restores_installed_plugin_before_recovery() {
    let fixture = Fixture(tests::test_root("update-rollback"));
    let root = &fixture.0;

    init_at(root).unwrap();

    let source = root.join("source");

    fs::create_dir_all(source.join("bin")).unwrap();

    let source_manifest = source.join("krabb-plugin.toml");
    let source_binary = source.join("bin/krabb-plugin-memory");

    let write_version = |version: &str| {
        fs::write(
            &source_manifest,
            format!(
                "id = 'memory'\nversion = '{version}'\n\
                 protocol = {{ major = 0, minor = 1 }}\ncapabilities = ['memory']\n"
            ),
        )
        .unwrap();
        tests::write_test_plugin(&source_binary, "memory", "memory", version);
    };

    write_version("0.1.0");

    link_at(
        Source {
            id: "memory".into(),
            source: Some(source.display().to_string()),
            revision: None,
            yes: true,
        },
        false,
        root,
    )
    .unwrap();

    let installed = root.join("plugins/memory");
    let binary = installed.join("bin/krabb-plugin-memory");
    let manifest = installed.join("krabb-plugin.toml");
    let lock_path = root.join("plugins.lock");
    let previous_binary = fs::read(&binary).unwrap();
    let previous_manifest = fs::read(&manifest).unwrap();
    let previous_lock = fs::read(&lock_path).unwrap();

    assert!(!load_lock_at(root).unwrap().plugins["memory"].linked);
    assert!(!fs::symlink_metadata(&binary).unwrap().file_type().is_symlink());

    write_version("0.2.0");

    let replacement_binary = fs::read(&source_binary).unwrap();
    let replacement_manifest = fs::read(&source_manifest).unwrap();
    let mut commits = 0;

    let error = update_with_commit(root, true, &BTreeMap::new(), false, |root, lock| {
        commits += 1;

        if commits != 1 {
            return save_lock_at(root, lock);
        }

        // Inject the filesystem fault only after the replacement has been activated.

        assert_eq!(fs::read(&binary).unwrap(), replacement_binary);
        assert_eq!(fs::read(&manifest).unwrap(), replacement_manifest);
        assert_eq!(fs::read(&lock_path).unwrap(), previous_lock);
        assert_eq!(lock.plugins["memory"].version, "0.2.0");

        let saved = root.join("committed.lock");
        fs::rename(&lock_path, &saved).unwrap();
        fs::create_dir(&lock_path).unwrap();
        let result = save_lock_at(root, lock);
        fs::remove_dir(&lock_path).unwrap();
        fs::rename(saved, &lock_path).unwrap();

        assert!(result.is_err(), "a directory cannot be replaced by lock metadata");
        result
    })
    .unwrap_err();

    assert!(error.to_string().contains("Could not save the plugin lock"), "{error}");
    assert!(!error.to_string().contains("Rollback failed"), "{error}");

    // Inspect bytes before any recovery or metadata-reading entrypoint can repair them.

    assert_eq!(fs::read(&binary).unwrap(), previous_binary);
    assert_eq!(fs::read(&manifest).unwrap(), previous_manifest);
    assert_eq!(fs::read(&lock_path).unwrap(), previous_lock);
    assert_eq!(verify_plugin_integrity(root, "memory", &binary).unwrap().version, "0.1.0");

    health(root, binary.clone(), "memory", &load_config_at(root).unwrap()).unwrap();

    update_at_with_previews(root, true, &BTreeMap::new(), false).unwrap();

    assert_eq!(fs::read(&binary).unwrap(), replacement_binary);
    assert_eq!(fs::read(&manifest).unwrap(), replacement_manifest);
    assert_eq!(load_lock_at(root).unwrap().plugins["memory"].version, "0.2.0");
    assert_eq!(verify_plugin_integrity(root, "memory", &binary).unwrap().version, "0.2.0");

    health(root, binary, "memory", &load_config_at(root).unwrap()).unwrap();

    assert_eq!(fs::read_dir(root.join("plugins")).unwrap().count(), 1);
}
