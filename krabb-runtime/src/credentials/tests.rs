use super::*;

use std::{
    cell::{Cell, RefCell},
    fs,
};

#[derive(Default)]
struct Keys {
    values: RefCell<BTreeMap<String, Vec<u8>>>,
    unavailable: Cell<bool>,
    fail_write: Cell<bool>,
    fail_read: Cell<bool>,
}

impl Keyring for Keys {
    fn available(&self) -> bool {
        !self.unavailable.get()
    }

    fn get(&self, id: &str) -> Result<Vec<u8>, Error> {
        if self.unavailable.get() || self.fail_read.get() {
            return Err("fixture keyring locked".into());
        }

        self.values.borrow().get(id).cloned().ok_or("fixture key missing".into())
    }

    fn set(&self, id: &str, key: &[u8]) -> Result<(), Error> {
        if self.fail_write.get() {
            return Err("fixture write failed".into());
        }

        self.values.borrow_mut().insert(id.into(), key.into());
        Ok(())
    }
}

fn values() -> Secrets {
    BTreeMap::from([("KRABB_SETUP_TEST".into(), "synthetic-do-not-use".into())])
}

#[test]
fn keyring_protects_the_key_and_locked_stores_never_fall_back() {
    let root = crate::tests::test_root("keyring");
    let path = root.join("credentials.json");
    let key = root.join("credentials.key");
    let keys = Keys::default();

    assert_eq!(save_with(&path, &key, values(), None, &keys).unwrap(), "keyring");
    assert!(!key.exists());
    assert_eq!(keys.values.borrow().len(), 1);
    assert_eq!(read_with(&path, &key, &keys).unwrap().0, values());
    assert!(!fs::read_to_string(&path).unwrap().contains("synthetic-do-not-use"));

    keys.unavailable.set(true);

    assert!(read_with(&path, &key, &keys).is_err());

    let original = fs::read(&path).unwrap();

    assert!(save_with(&path, &key, values(), None, &keys).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!key.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn encrypted_fallback_is_random_private_persistent_and_customizable() {
    let root = crate::tests::test_root("encrypted");
    let path = root.join("custom/secrets.json");
    let key = root.join("separate/key");
    let keys = Keys::default();

    keys.unavailable.set(true);

    assert_eq!(save_with(&path, &key, values(), None, &keys).unwrap(), "file");

    let initial = fs::read_to_string(&path).unwrap();
    let master = fs::read(&key).unwrap();

    assert_eq!(master.len(), 32);
    assert!(!initial.contains("synthetic-do-not-use"));
    assert!(!initial.contains(&STANDARD.encode(&master)));

    krabb_file::check(&key).unwrap();
    krabb_file::check(&path).unwrap();

    assert_eq!(read(&path, &key).unwrap(), values());

    keys.unavailable.set(false);

    assert_eq!(save_with(&path, &key, Secrets::new(), None, &keys).unwrap(), "file");
    assert!(keys.values.borrow().is_empty());
    assert_eq!(fs::read(&key).unwrap(), master);
    assert_ne!(initial, fs::read_to_string(&path).unwrap());
    assert_eq!(read(&path, &key).unwrap(), values());

    let other_key = root.join("second/key");

    save_with(&root.join("second/secrets"), &other_key, values(), Some(false), &keys).unwrap();

    assert_ne!(fs::read(&other_key).unwrap(), master);
    assert!(read(&path, &other_key).is_err());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lost_key_is_not_regenerated_and_tampering_is_rejected() {
    let root = crate::tests::test_root("tamper");
    let path = root.join("credentials.json");
    let key = root.join("credentials.key");

    save(&path, &key, values(), Some(false)).unwrap();

    let original = fs::read(&path).unwrap();
    let master = fs::read(&key).unwrap();

    fs::remove_file(&key).unwrap();

    assert!(read(&path, &key).is_err());
    assert!(save(&path, &key, values(), Some(false)).is_err());
    assert!(!key.exists());
    assert_eq!(fs::read(&path).unwrap(), original);

    crate::save_file(&key, &master).unwrap();

    for (field, replacement) in [
        ("nonce", "invalid"),
        ("nonce", "AAAAAAAAAAAAAAAA"),
        ("ciphertext", "invalid"),
        ("ciphertext", "AAAA"),
        ("key_id", "bad"),
        ("format", "krabb.credentials.v99"),
        ("backend", "keyring"),
    ] {
        let mut envelope: serde_json::Value = serde_json::from_slice(&original).unwrap();
        envelope[field] = replacement.into();
        crate::save_file(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();

        assert!(read(&path, &key).is_err(), "field {field}");
    }

    crate::save_file(&path, &original).unwrap();

    crate::save_file(&key, [0; 32]).unwrap();

    assert!(read(&path, &key).is_err());

    crate::save_file(&key, [0; 31]).unwrap();

    assert!(read(&path, &key).is_err());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn migrates_plaintext_and_switches_key_backends_without_losing_other_tokens() {
    let root = crate::tests::test_root("migrate-credentials");
    let path = root.join("credentials.json");
    let key = root.join("credentials.key");

    crate::save_file(&path, br#"{"OTHER":"preserved"}"#).unwrap();

    let keys = Keys::default();

    assert_eq!(save_with(&path, &key, values(), Some(true), &keys).unwrap(), "keyring");
    assert_eq!(read_with(&path, &key, &keys).unwrap().0["OTHER"], "preserved");
    assert!(!fs::read_to_string(&path).unwrap().contains("preserved"));
    assert_eq!(save_with(&path, &key, Secrets::new(), Some(false), &keys).unwrap(), "file");
    assert_eq!(read(&path, &key).unwrap()["OTHER"], "preserved");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn keyring_failures_and_invalid_files_preserve_existing_credentials() {
    let root = crate::tests::test_root("keyring-errors");
    let path = root.join("credentials.json");
    let key = root.join("credentials.key");
    let keys = Keys::default();

    keys.unavailable.set(true);

    assert!(save_with(&path, &key, values(), Some(true), &keys).is_err());
    assert!(!path.exists());

    keys.unavailable.set(false);
    keys.fail_write.set(true);

    assert!(save_with(&path, &key, values(), None, &keys).is_err());
    assert!(!path.exists());

    keys.fail_write.set(false);
    keys.fail_read.set(true);

    assert!(save_with(&path, &key, values(), None, &keys).is_err());
    assert!(!path.exists());

    for data in ["[]", "null", "{broken}", "{\"key\":123}", "{\"format\":42}"] {
        crate::save_file(&path, data).unwrap();

        assert!(save_with(&path, &key, values(), None, &keys).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), data);
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn shared_installation_keys_serialize_across_different_credential_files() {
    let root = crate::tests::test_root("shared-key");
    let key = root.join("keys/master");

    let jobs = (0..4)
        .map(|index| {
            let path = root.join(format!("store-{index}"));
            let key = key.clone();
            std::thread::spawn(move || {
                save(&path, &key, values(), Some(false)).unwrap();

                assert_eq!(read(&path, &key).unwrap(), values());
            })
        })
        .collect::<Vec<_>>();

    for job in jobs {
        job.join().unwrap();
    }

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn rejects_shared_or_linked_keys_and_colliding_paths() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = crate::tests::test_root("key-policy");
    let path = root.join("credentials.json");
    let key = root.join("credentials.key");

    assert!(save(&path, &path, values(), Some(false)).is_err());
    assert!(!path.exists());

    fs::create_dir_all(root.join("nested")).unwrap();

    assert!(save(&path, &root.join("nested/../credentials.json"), values(), Some(false)).is_err());

    crate::save_file(&key, [0; 32]).unwrap();

    let alias = root.join("key-link");

    symlink(&key, &alias).unwrap();

    assert!(save(&path, &alias, values(), Some(false)).is_err());

    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(save(&path, &key, values(), Some(false)).is_err());
    assert_eq!(fs::metadata(&key).unwrap().permissions().mode() & 0o777, 0o644);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_ciphertexts_and_oversized_maps_are_bounded() {
    let key = [42; 32];
    let mut envelope = encrypt(&values(), &key, Backend::File).unwrap();

    envelope.nonce = STANDARD.encode([0; 11]);

    assert!(decrypt(&envelope, &key).is_err());

    envelope.nonce = STANDARD.encode([0; 12]);
    envelope.ciphertext = STANDARD.encode(vec![0; LIMIT + 17]);

    assert!(decrypt(&envelope, &key).is_err());

    assert!(
        encrypt(&BTreeMap::from([("x".into(), "x".repeat(LIMIT))]), &key, Backend::File).is_err()
    );

    assert!(cipher(&[0; 31]).is_err());
}

#[test]
fn environment_overrides_storage_without_opening_it() {
    let names = ["KRABB_SETUP_TEST".into()];

    let value =
        resolve_with(&names, |_| Some("override".into()), || Err("locked store".into())).unwrap();

    assert_eq!(value["KRABB_SETUP_TEST"], "override");
    assert_eq!(resolve_with(&names, |_| Some("  ".into()), || Ok(values())).unwrap(), values());
    assert!(resolve_with(&names, |_| None, || Err("locked store".into())).is_err());

    assert!(
        resolve_with(&["OTHER".into()], |_| Some("override".into()), || panic!("must not load"))
            .unwrap()
            .is_empty()
    );

    assert!(resolve_with(&["KRABB_MISSING".into()], |_| None, || Ok(values())).unwrap().is_empty());
    assert_eq!(preference_from(None).unwrap(), None);
    assert_eq!(preference_from(Some("auto")).unwrap(), None);
    assert_eq!(preference_from(Some("0")).unwrap(), Some(false));
    assert_eq!(preference_from(Some("1")).unwrap(), Some(true));
    assert!(preference_from(Some("invalid")).is_err());
}

#[test]
fn plugin_secrets_use_one_snapshot_and_preserve_environment_overrides() {
    let reads = Cell::new(0);
    let names = ["KRABB_A".into(), "KRABB_B".into(), "KRABB_C".into()];

    let resolved = resolve_with(
        &names,
        |name| (name == "KRABB_C").then(|| "environment".into()),
        || {
            reads.set(reads.get() + 1);
            Ok(BTreeMap::from([
                ("KRABB_A".into(), "generation-1-a".into()),
                ("KRABB_B".into(), "generation-1-b".into()),
                ("KRABB_C".into(), "stored".into()),
                ("KRABB_UNDECLARED".into(), "private".into()),
            ]))
        },
    )
    .unwrap();

    assert_eq!(reads.get(), 1);

    assert_eq!(
        resolved,
        BTreeMap::from([
            ("KRABB_A".into(), "generation-1-a".into()),
            ("KRABB_B".into(), "generation-1-b".into()),
            ("KRABB_C".into(), "environment".into()),
        ])
    );

    assert!(
        resolve_with(&[], |_| panic!("no secrets"), || panic!("no store access"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn encrypted_credentials_feed_readiness_and_only_declared_plugin_secrets() {
    let root = crate::tests::test_root("encrypted-runtime");
    let path = root.join("credentials.json");
    let key = root.join("credentials.key");

    let providers = [
        ("telegram", "KRABB_TELEGRAM_TOKEN"),
        ("discord", "KRABB_DISCORD_TOKEN"),
        ("codex", "KRABB_CODEX_KEY"),
        ("anthropic", "KRABB_ANTHROPIC_KEY"),
        ("openai", "KRABB_OPENAI_KEY"),
        ("openrouter", "KRABB_OPENROUTER_KEY"),
        ("gemini", "KRABB_GEMINI_KEY"),
        ("signal", "KRABB_SIGNAL_ACCOUNT"),
        ("slack", "KRABB_SLACK_BOT_TOKEN"),
    ];

    let stored = providers.iter().map(|(_, name)| ((*name).into(), "synthetic".into())).collect();

    save(&path, &key, stored, Some(false)).unwrap();

    for (plugin, name) in providers {
        assert!(crate::ready_at(plugin, Some(&path), &key), "{plugin}");

        let manifest = crate::Manifest {
            id: plugin.into(),
            version: "0.1.0".into(),
            protocol: krabb_core::types::Protocol::CURRENT,
            capabilities: vec![],
            permissions: vec![],
            variables: vec![name.into()],
            settings: Vec::new(),
            commands: vec![],
        };

        let environment = crate::env_for(&manifest, &crate::Config::default(), &root).unwrap();

        assert!(environment.contains(&(name.into(), "synthetic".into())));

        assert_eq!(
            environment
                .iter()
                .filter(|(name, _)| providers.iter().any(|(_, secret)| secret == name))
                .count(),
            1
        );
    }

    let manifest = crate::Manifest {
        id: "fixture".into(),
        version: "0.1.0".into(),
        protocol: krabb_core::types::Protocol::CURRENT,
        capabilities: vec![],
        permissions: vec![],
        variables: vec!["KRABB_SETUP_TEST".into()],
        settings: Vec::new(),
        commands: vec![],
    };

    fs::remove_file(&key).unwrap();

    assert!(crate::env_for(&manifest, &crate::Config::default(), &root).is_err());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn service_capture_preserves_custom_credential_and_key_paths() {
    let root = crate::tests::test_root("service-custom-key");
    let source = root.join("custom/secrets");
    let key = root.join("separate/master");

    save(&source, &key, values(), Some(false)).unwrap();

    let environment = BTreeMap::from([
        ("KRABB_CREDENTIALS".into(), source.display().to_string()),
        ("KRABB_CREDENTIALS_KEY".into(), key.display().to_string()),
        ("KRABB_CODEX_KEY".into(), "synthetic-service-override".into()),
    ]);

    let service = root.join("service");
    let captured = crate::service_environment_from(&service, root.clone(), &environment).unwrap();

    assert!(captured.contains(&("KRABB_CREDENTIALS_KEY".into(), key.display().to_string())));

    let path = crate::service_credentials_path(&service);
    let stored = read(&path, &key).unwrap();

    assert_eq!(stored["KRABB_CODEX_KEY"], "synthetic-service-override");
    assert_eq!(stored["KRABB_SETUP_TEST"], "synthetic-do-not-use");
    assert!(!fs::read_to_string(path).unwrap().contains("synthetic"));
    assert!(!root.join("credentials.key").exists());
    assert!(!captured.iter().any(|(_, value)| value.contains("synthetic")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn service_capture_honors_required_keyring_instead_of_silently_downgrading() {
    let root = crate::tests::test_root("service-required-keyring");
    let service = root.join("service");
    let destination = crate::service_credentials_path(&service);

    let mut environment = BTreeMap::from([
        ("KRABB_TELEGRAM_TOKEN".into(), "synthetic-service-token".into()),
        ("KRABB_KEYRING".into(), "1".into()),
    ]);

    let error = crate::service_environment_from(&service, root.clone(), &environment).unwrap_err();

    assert!(error.to_string().contains("Keyring was requested"));
    assert!(!destination.exists());
    assert!(!root.join("credentials.key").exists());

    environment.insert("KRABB_KEYRING".into(), "0".into());
    crate::service_environment_from(&service, root.clone(), &environment).unwrap();

    assert_eq!(
        read(&destination, &root.join("credentials.key")).unwrap()["KRABB_TELEGRAM_TOKEN"],
        "synthetic-service-token"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn openai_readiness_accepts_its_legacy_keyring_without_accessing_native_storage() {
    assert!(crate::ready_with("openai", None, Path::new("unused"), |name| {
        assert_eq!(name, "openai");
        true
    }));
}
