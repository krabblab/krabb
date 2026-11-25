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
            ..Revoked::default()
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
    assert_eq!(serde_json::to_value(Revoked::default()).unwrap()["cleanup"], "complete");

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

#[test]
fn uninstall_also_forgets_the_service_credential_copy() {
    let root = crate::tests::test_root("revoke-service-copy");
    let service = root.join("units/krabb.service");
    let copy = service_credentials_path(&service);

    let unit = |home: &Path| {
        let environment = [
            ("KRABB_HOME".to_owned(), home.display().to_string()),
            ("KRABB_CREDENTIALS".to_owned(), copy.display().to_string()),
            ("KRABB_CREDENTIALS_KEY".to_owned(), credentials::key_path(home).display().to_string()),
        ];

        service_text(Path::new("/usr/local/bin/krabb-daemon"), &environment)
    };

    std::fs::create_dir_all(root.join("other")).unwrap();
    std::fs::create_dir_all(service.parent().unwrap()).unwrap();
    std::fs::write(&service, unit(&root.join("other"))).unwrap();

    credentials::save(
        &copy,
        &credentials::key_path(&root),
        [("KRABB_TELEGRAM_TOKEN", "synthetic"), ("KRABB_DISCORD_TOKEN", "keep")]
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect(),
        Some(false),
    )
    .unwrap();

    let names = ["KRABB_TELEGRAM_TOKEN".to_owned(), "KRABB_TELEGRAM_MODE".to_owned()];

    assert!(service_copy(&root, &service, &names).unwrap().is_empty());

    std::fs::write(&service, unit(&root)).unwrap();

    assert_eq!(service_copy(&root, &service, &names).unwrap(), ["KRABB_TELEGRAM_TOKEN"]);

    let left = credentials::read(&copy, &credentials::key_path(&root)).unwrap();

    assert_eq!(left.keys().collect::<Vec<_>>(), ["KRABB_DISCORD_TOKEN"]);
    assert!(service_copy(&root, &service, &names).unwrap().is_empty());
    assert!(service_copy(&root, &root.join("missing.service"), &names).unwrap().is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_unwritable_service_credential_copy_is_reported_as_partial_cleanup() {
    let root = crate::tests::test_root("revoke-service-blocked");
    let service = root.join("units/krabb.service");
    let copy = service_credentials_path(&service);

    let environment = [
        ("KRABB_HOME".to_owned(), root.display().to_string()),
        ("KRABB_CREDENTIALS".to_owned(), copy.display().to_string()),
        ("KRABB_CREDENTIALS_KEY".to_owned(), credentials::key_path(&root).display().to_string()),
    ];

    std::fs::create_dir_all(service.parent().unwrap()).unwrap();
    std::fs::write(&service, service_text(Path::new("/usr/local/bin/krabb-daemon"), &environment))
        .unwrap();

    credentials::save(
        &copy,
        &credentials::key_path(&root),
        [("KRABB_TELEGRAM_TOKEN".into(), "synthetic".into())].into_iter().collect(),
        Some(false),
    )
    .unwrap();

    // A directory where the copy's lock file belongs blocks the write even for root.
    let name = copy.file_name().unwrap().to_str().unwrap();

    let blocked = copy.with_file_name(format!(".{name}.lock"));

    let _ = std::fs::remove_file(&blocked);

    std::fs::create_dir_all(&blocked).unwrap();

    let revoked = revoke_with(&root, "telegram", &lock(), &service).unwrap();

    assert_eq!(revoked.cleanup, Cleanup::Partial);
    assert_eq!(revoked.failures.len(), 1);
    assert_eq!(revoked.failures[0].step, "service_credentials");
    assert!(revoked.failures[0].error.contains(&copy.display().to_string()));
    assert_eq!(revoked.failures[0].next, "Run `krabb service uninstall` to remove the copy.");

    let json = serde_json::to_value(&revoked).unwrap();

    assert_eq!(json["cleanup"], "partial");
    assert_eq!(json["failures"][0]["step"], "service_credentials");

    let _ = std::fs::remove_dir_all(root);
}
