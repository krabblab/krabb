use crate::*;
use std::{fs, io::ErrorKind};

const ORIGINAL: &str = "\
# Preserve this local policy and its formatting verbatim.\n\
approval = 'prompt'\n\
[clients.tui]\n\
shell = false\n\
[channels.telegram]\n\
allow = ['original-chat']\n";

const IMPORTED: &str = "\
approval = 'auto'\n\
[clients.tui]\n\
shell = true\n\
[channels.telegram]\n\
allow = ['imported-chat']\n";

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy)]
enum Failure {
    MissingBinary,
    Version,
    Capabilities,
}

fn assert_rollback_and_retry(existing: bool, failure: Failure) {
    let fixture = Fixture(tests::test_root("krabfile-rollback"));
    let root = fixture.0.join("home");
    let destination = root.join("config.toml");
    let source = fixture.0.join("source");

    fs::create_dir_all(source.join("bin")).unwrap();

    fs::write(
        source.join("krabb-plugin.toml"),
        "id = 'memory'\nversion = '0.1.0'\n\
         protocol = { major = 0, minor = 1 }\ncapabilities = ['memory']\n",
    )
    .unwrap();

    let binary = source.join("bin").join(plugin_name("memory"));

    if !matches!(failure, Failure::MissingBinary) {
        tests::write_test_plugin(&binary, "memory", "memory", "0.1.0");
    }

    if existing {
        init_at(&root).unwrap();
        fs::write(&destination, ORIGINAL).unwrap();
    }

    let mut file = Krabfile {
        version: KRABFILE_VERSION.into(),
        config: toml::from_str(IMPORTED).unwrap(),
        plugins: vec![KrabbPlugin {
            id: "memory".into(),
            source: source.to_str().unwrap().into(),
            revision: "local".into(),
            version: if matches!(failure, Failure::Version) { "0.2.0" } else { "0.1.0" }.into(),
            capabilities: vec![
                if matches!(failure, Failure::Capabilities) { "tool" } else { "memory" }.into(),
            ],
        }],
    };

    let path = fixture.0.join("Krabfile");

    fs::write(&path, toml::to_string_pretty(&file).unwrap()).unwrap();

    let import = || {
        import_krabfile_at_mode(
            KrabfileImport { path: Some(path.clone()), yes: true, force: true },
            &root,
            true,
        )
    };

    let error = import().unwrap_err().to_string();

    match failure {
        Failure::MissingBinary => assert_eq!(
            error,
            format!("Binary {} was not found; build the plugin first.", plugin_name("memory"))
        ),

        Failure::Version | Failure::Capabilities => {
            assert_eq!(error, "Plugin memory metadata did not match the Krabfile.");
        }
    }

    // Check the file before loading metadata can perform recovery.
    if existing {
        assert_eq!(fs::read(&destination).unwrap(), ORIGINAL.as_bytes());

        let restored = load_config_at(&root).unwrap();

        assert_eq!(restored.approval_mode(), ApprovalMode::Prompt);
        assert!(!restored.tui_shell());
        assert_eq!(restored.channels["telegram"].allow, ["original-chat"]);
    } else {
        assert_eq!(fs::symlink_metadata(&destination).unwrap_err().kind(), ErrorKind::NotFound);
    }

    match failure {
        Failure::MissingBinary => {
            assert!(!load_lock_at(&root).unwrap().plugins.contains_key("memory"));

            tests::write_test_plugin(&binary, "memory", "memory", "0.1.0");
        }

        Failure::Version | Failure::Capabilities => {
            let lock = load_lock_at(&root).unwrap();

            assert_eq!(lock.plugins["memory"].version, "0.1.0");
            assert_eq!(lock.plugins["memory"].capabilities, ["memory"]);

            file.plugins[0].version = "0.1.0".into();
            file.plugins[0].capabilities = vec!["memory".into()];
            fs::write(&path, toml::to_string_pretty(&file).unwrap()).unwrap();
        }
    }

    import().unwrap();

    let applied = load_config_at(&root).unwrap();

    assert_eq!(applied.approval_mode(), ApprovalMode::Auto);
    assert!(applied.tui_shell());
    assert_eq!(applied.channels["telegram"].allow, ["imported-chat"]);

    let lock = load_lock_at(&root).unwrap();

    assert_eq!(lock.plugins["memory"].version, "0.1.0");
    assert_eq!(lock.plugins["memory"].capabilities, ["memory"]);
    assert!(root.join("plugins/memory/bin").join(plugin_name("memory")).is_file());
}

#[test]
fn installation_failure_restores_existing_configuration_and_allows_retry() {
    assert_rollback_and_retry(true, Failure::MissingBinary);
}

#[test]
fn installation_failure_removes_new_configuration_and_allows_retry() {
    assert_rollback_and_retry(false, Failure::MissingBinary);
}

#[test]
fn metadata_mismatch_restores_existing_configuration_and_allows_retry() {
    for failure in [Failure::Version, Failure::Capabilities] {
        assert_rollback_and_retry(true, failure);
    }
}

#[test]
fn metadata_mismatch_removes_new_configuration_and_allows_retry() {
    for failure in [Failure::Version, Failure::Capabilities] {
        assert_rollback_and_retry(false, failure);
    }
}
