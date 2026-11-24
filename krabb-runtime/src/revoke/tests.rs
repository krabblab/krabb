use super::*;

fn entry(variables: &[&str], settings: &[&str]) -> Entry {
    Entry {
        source: String::new(),
        revision: String::new(),
        pinned: false,
        default: false,
        hash: String::new(),
        version: "0.1.0".into(),
        protocol: Protocol::CURRENT,
        capabilities: Vec::new(),
        permissions: Vec::new(),
        variables: variables.iter().map(|name| (*name).into()).collect(),
        settings: settings
            .iter()
            .map(|name| setup::Setting { name: (*name).into(), default: None })
            .collect(),
        linked: false,
        commands: Vec::new(),
        state: None,
    }
}

fn lock() -> Lock {
    let mut lock = Lock::default();

    lock.plugins.insert(
        "telegram".into(),
        entry(&["KRABB_TELEGRAM_TOKEN", "KRABB_SHARED"], &["KRABB_TELEGRAM_MODE"]),
    );

    lock.plugins.insert("discord".into(), entry(&["KRABB_DISCORD_TOKEN", "KRABB_SHARED"], &[]));

    lock
}

const CONFIG: &str = r#"
[[visibility]]
data = ["context"]
resources = ["channels.telegram"]

[[visibility]]
data = ["memory"]
resources = ["global", "channels.telegram"]

[channels.telegram]
worktree = false

[[channels.telegram.visibility]]
data = ["files"]
resources = ["channels.discord"]

[channels.discord]
visibility = [{ data = ["context"], resources = ["channels.telegram"] }]

[clients.desktop]
visibility = [
    { data = ["context"], resources = ["channels.telegram", "clients.tui"] },
    { data = ["memory"], resources = ["clients.tui"] },
]
"#;

#[test]
fn uninstall_revokes_only_the_plugin_grants_and_exclusive_secrets() {
    let root = crate::tests::test_root("revoke");

    std::fs::create_dir_all(&root).unwrap();
    secure(root.join("config.toml"), CONFIG).unwrap();

    let values = [
        ("KRABB_TELEGRAM_TOKEN", "synthetic"),
        ("KRABB_TELEGRAM_MODE", "polling"),
        ("KRABB_SHARED", "keep"),
        ("KRABB_DISCORD_TOKEN", "keep"),
    ];

    credentials::save(
        &setup::path(&root),
        &credentials::key_path(&root),
        values.iter().map(|(name, value)| ((*name).into(), (*value).into())).collect(),
        Some(false),
    )
    .unwrap();

    let revoked = revoke(&root, "telegram", &lock()).unwrap();

    assert_eq!(
        revoked,
        Revoked {
            secrets: vec!["KRABB_TELEGRAM_MODE".into(), "KRABB_TELEGRAM_TOKEN".into()],
            keyring: false,
            grants: 5,
        }
    );

    let config: Config =
        toml::from_str(&std::fs::read_to_string(root.join("config.toml")).unwrap()).unwrap();

    assert_eq!(config.visibility.len(), 1);
    assert_eq!(config.visibility[0].resources, ["global".to_owned().try_into().unwrap()]);
    assert_eq!(config.channels["telegram"].visibility.as_deref(), Some(&[][..]));
    assert_eq!(config.channels["discord"].visibility.as_deref(), Some(&[][..]));
    assert_eq!(config.clients["desktop"].visibility.as_ref().unwrap().len(), 2);
    assert!(!config.channels["telegram"].worktree);

    let left = credentials::read(&setup::path(&root), &credentials::key_path(&root)).unwrap();

    assert_eq!(left.keys().collect::<Vec<_>>(), ["KRABB_DISCORD_TOKEN", "KRABB_SHARED"]);
    assert_eq!(revoke(&root, "telegram", &lock()).unwrap(), Revoked::default());
    assert_eq!(revoke(&root, "missing", &lock()).unwrap(), Revoked::default());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn failed_revocation_keeps_the_plugin_installed_for_a_retry() {
    let root = crate::tests::test_root("revoke-retry");

    std::fs::create_dir_all(&root).unwrap();
    secure(root.join("config.toml"), CONFIG).unwrap();
    secure(setup::path(&root), "not json").unwrap();

    let error = revoke(&root, "telegram", &lock()).unwrap_err().to_string();

    assert!(error.contains("Removed 5 sharing grant(s) already."), "{error}");
    assert!(error.contains("retry the uninstall"), "{error}");
    assert_eq!(revoke(&root, "absent", &Lock::default()).unwrap(), Revoked::default());

    let _ = std::fs::remove_dir_all(root);
}
