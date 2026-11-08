use super::*;
use crossterm::event::{KeyEvent, KeyEventState};
use std::fs;

fn fixture(id: &str, variables: &[&str], commands: Vec<krabb_core::types::CommandSpec>) -> PathBuf {
    let root = crate::tests::test_root("setup");
    let plugin = root.join("plugins").join(id);

    fs::create_dir_all(plugin.join("bin")).unwrap();
    fs::write(plugin.join("bin").join(crate::plugin_name(id)), "must never be executed").unwrap();

    let manifest = crate::Manifest {
        id: id.into(),
        version: "0.1.0".into(),
        protocol: krabb_core::types::Protocol::CURRENT,
        capabilities: vec!["channel".into()],
        permissions: vec![],
        variables: variables.iter().map(|name| (*name).into()).collect(),
        settings: Vec::new(),
        commands: commands.clone(),
    };

    fs::write(plugin.join("krabb-plugin.toml"), toml::to_string(&manifest).unwrap()).unwrap();

    let entry = crate::Entry {
        source: "fixture".into(),
        revision: String::new(),
        pinned: false,
        default: false,
        hash: String::new(),
        version: manifest.version,
        protocol: manifest.protocol,
        capabilities: manifest.capabilities,
        permissions: vec![],
        variables: manifest.variables,
        settings: manifest.settings,
        linked: false,
        commands,
    };

    crate::save_lock_at(&root, &crate::Lock { plugins: BTreeMap::from([(id.into(), entry)]) })
        .unwrap();

    root
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

#[test]
fn secret_input_edits_cancels_and_bounds_without_echo() {
    let mut events = [
        Event::Resize(80, 24),
        Event::Key(KeyEvent {
            code: KeyCode::Char('z'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        }),
        key(KeyCode::Left),
        key(KeyCode::Char('a')),
        key(KeyCode::Char('x')),
        key(KeyCode::Backspace),
        key(KeyCode::Char('b')),
        Event::Paste("cd".into()),
        key(KeyCode::Enter),
    ]
    .into_iter();

    assert_eq!(input(|| Ok(events.next().unwrap())).unwrap(), "abcd");

    for text in ["a\nb".into(), "x".repeat(LIMIT + 1)] {
        assert!(input(|| Ok(Event::Paste(text.clone()))).is_err());
    }

    for event in [
        key(KeyCode::Esc),
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
    ] {
        assert!(input(|| Ok(event.clone())).unwrap_err().to_string().contains("cancelled"));
    }

    assert!(input(|| Ok(key(KeyCode::Char('a')))).unwrap_err().to_string().contains("limit"));

    assert!(input(|| Err(io::Error::other("closed"))).is_err());
}

#[test]
fn validates_all_values_before_saving_and_never_quotes_invalid_values() {
    let names = ["KRABB_SETUP_A", "KRABB_SETUP_B"]
        .map(|name| Field::from(Setting { name: name.into(), default: None }));

    let values = collect(&names, |_| Ok("synthetic".into())).unwrap();

    assert_eq!(values.len(), 2);

    for invalid in ["".into(), " ".into(), "synthetic\nvalue".into(), "x".repeat(LIMIT + 1)] {
        let error = collect(&names, |_| Ok(invalid.clone())).unwrap_err().to_string();

        assert!(error.starts_with("Setup value must be"));
        assert!(!error.contains("synthetic"));
    }

    let mut prompts = 0;

    assert!(
        collect(&names, |_| {
            prompts += 1;

            if prompts == 2 { Err("cancelled".into()) } else { Ok("synthetic".into()) }
        })
        .is_err()
    );
}

#[test]
fn empty_secrets_clear_saved_keys_without_leaking_to_plugins() {
    let root = fixture("custom", &["KRABB_SETUP_A", "KRABB_SETUP_B"], vec![]);
    let fields = fields(&root, "custom").unwrap();
    let destination = path(&root);
    let manifest = crate::read_manifest_checked(&root.join("plugins/custom")).unwrap();

    for secret in ["", "synthetic", ""] {
        let values = collect(&fields, |_| Ok(secret.into())).unwrap();

        assert_eq!(values.len(), 2);

        save(&destination, values).unwrap();

        assert!(activation(&root, "custom", &destination.canonicalize().unwrap()).is_ok());

        let environment = crate::env_for(&manifest, &crate::Config::default(), &root).unwrap();

        for name in ["KRABB_SETUP_A", "KRABB_SETUP_B"] {
            assert_eq!(read(&destination).unwrap()[name], secret);

            assert_eq!(
                environment.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str()),
                (!secret.is_empty()).then_some(secret)
            );
        }
    }

    for invalid in [" ".into(), "synthetic\nvalue".into(), "x".repeat(LIMIT + 1)] {
        let error = collect(&fields, |_| Ok(invalid.clone())).unwrap_err().to_string();

        assert!(error.starts_with("Setup value must be"));
        assert!(!error.contains("synthetic"));
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stores_private_credentials_preserves_other_keys_and_rejects_corruption() {
    let root = crate::tests::test_root("setup-store");
    let path = root.join("credentials.json");

    assert!(read(&path).unwrap().is_empty());

    save(&path, BTreeMap::from([("OTHER".into(), "retained".into())])).unwrap();
    save(&path, BTreeMap::from([("KRABB_SETUP_A".into(), "synthetic".into())])).unwrap();

    assert_eq!(read(&path).unwrap().len(), 2);

    krabb_file::check(&path).unwrap();

    for content in ["[]", "{\"key\":123}", "{broken synthetic data}"] {
        fs::write(&path, content).unwrap();

        let error = save(&path, BTreeMap::new()).unwrap_err().to_string();

        assert_eq!(error, "Credential file must contain a JSON object of string values.");
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
    }

    fs::write(&path, "x".repeat(FILE_LIMIT as usize + 1)).unwrap();

    assert!(read(&path).is_err());

    fs::write(&path, "{}").unwrap();

    assert!(
        save(&path, BTreeMap::from([("key".into(), "x".repeat(FILE_LIMIT as usize))])).is_err()
    );

    assert_eq!(fs::read_to_string(&path).unwrap(), "{}");

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_and_shared_files_without_modifying_them() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = crate::tests::test_root("setup-link");

    fs::create_dir_all(&root).unwrap();

    let original = root.join("original");
    let path = root.join("credentials.json");

    fs::write(&original, "{}").unwrap();
    symlink(&original, &path).unwrap();

    assert!(save(&path, BTreeMap::new()).is_err());
    assert_eq!(fs::read_to_string(&original).unwrap(), "{}");

    fs::remove_file(&path).unwrap();
    fs::write(&path, "{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(read(&path).is_err());
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o644);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_setup_merges_under_a_shared_file_lock() {
    let root = crate::tests::test_root("setup-concurrent");
    let path = root.join("credentials.json");

    let handles = (0..4)
        .map(|index| {
            let path = path.clone();
            std::thread::spawn(move || {
                save(&path, BTreeMap::from([(index.to_string(), "synthetic".into())])).unwrap()
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(read(&path).unwrap().len(), 4);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn setup_uses_reviewed_manifest_secrets_and_default_credentials() {
    let root = fixture("telegram", &["KRABB_SETUP_A"], vec![]);
    let manifest = crate::read_manifest_checked(&root.join("plugins/telegram")).unwrap();
    let lock = crate::load_lock_at(&root).unwrap();

    for metadata in [
        serde_json::to_value(&manifest).unwrap(),
        serde_json::to_value(&lock.plugins["telegram"]).unwrap(),
    ] {
        assert_eq!(metadata["secrets"], json!(["KRABB_SETUP_A"]));
        assert!(metadata.get("variables").is_none());
    }

    assert_eq!(fields(&root, "telegram").unwrap(), [Field::secret("KRABB_SETUP_A".into())]);

    assert!(fields(&root, "../escape").is_err());
    assert!(fields(&root, "missing").is_err());

    let destination = path(&root);

    assert_eq!(destination, root.join("credentials.json"));

    save(&destination, BTreeMap::from([("KRABB_SETUP_A".into(), "synthetic".into())])).unwrap();

    assert_eq!(
        crate::credentials::resolve(&["KRABB_SETUP_A".into()], &root)
            .unwrap()
            .remove("KRABB_SETUP_A")
            .as_deref(),
        Some("synthetic")
    );

    assert!(crate::credentials::resolve(&["OTHER".into()], &root).unwrap().is_empty());

    assert!(
        crate::credentials::resolve(&["KRABB_SETUP_MISSING".into()], &root).unwrap().is_empty()
    );

    let manifest = crate::read_manifest_checked(&root.join("plugins/telegram")).unwrap();

    assert!(
        crate::env_for(&manifest, &crate::Config::default(), &root)
            .unwrap()
            .contains(&("KRABB_SETUP_A".into(), "synthetic".into()))
    );

    assert!(activation(&root, "telegram", &destination.canonicalize().unwrap()).is_ok());

    assert!(activation(&root, "telegram", Path::new("/different")).is_err());

    let mut lock = crate::load_lock_at(&root).unwrap();

    lock.plugins.get_mut("telegram").unwrap().variables.clear();
    crate::save_lock_at(&root, &lock).unwrap();

    assert!(fields(&root, "telegram").unwrap_err().to_string().contains("changed"));

    fs::remove_dir_all(root).unwrap();

    let root = fixture("ollama", &[], vec![]);

    assert!(fields(&root, "ollama").unwrap_err().to_string().contains("no secrets"));
    assert!(!crate::available_commands(&root).unwrap().contains_key("ollama"));

    fs::remove_dir_all(root).unwrap();

    let root = fixture("config", &["KRABB_SETUP_A"], vec![]);

    assert!(!crate::available_commands(&root).unwrap().contains_key("config"));

    assert_eq!(fields(&root, "config").unwrap(), [Field::secret("KRABB_SETUP_A".into())]);

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn standardized_aliases_and_codex_login_registration_coexist_without_launching() {
    for id in ["telegram", "codex", "anthropic", "custom"] {
        let commands = if id == "codex" {
            vec![krabb_core::types::CommandSpec {
                name: id.into(),
                description: "Account sign-in".into(),
                interactive: true,
            }]
        } else {
            vec![]
        };

        let root = fixture(id, &["KRABB_SETUP_A"], commands);
        let available = crate::available_commands(&root).unwrap();

        assert_eq!(available[id].len(), 1);

        if id == "codex" {
            assert_eq!(available[id][0].0, "codex");
        }

        let help = crate::help_text_with_plugins(&root).unwrap();
        let (_, sections) = help.split_once("Conditional Commands:").unwrap();
        let (conditional, plugins) = sections.split_once("Plugin Commands:").unwrap();
        let (plugins, _) = plugins.split_once("Options:").unwrap();

        assert!(!conditional.lines().any(|line| line.split_whitespace().next() == Some(id)));

        assert_eq!(
            plugins.lines().filter(|line| line.split_whitespace().next() == Some(id)).count(),
            1
        );

        assert!(conditional.contains("delivery"));
        assert!(!plugins.contains("delivery"));

        for arguments in [
            vec![id.into(), "setup".into(), "--help".into()],
            vec![id.into(), "--json".into(), "setup".into(), "--help".into()],
            vec![id.into(), "--debug".into(), "setup".into(), "--help".into()],
            vec![id.into(), "--help".into()],
        ] {
            crate::external_command_at(arguments, false, root.clone()).await.unwrap();
        }

        assert!(
            command(&["setup".into(), "--token".into(), "synthetic".into()], id, &root, false)
                .await
                .is_err()
        );

        assert!(command(&["unknown".into()], id, &root, false).await.is_err());
        assert!(!root.join("credentials.json").exists());

        fs::remove_dir_all(root).unwrap();
    }

    let parsed = crate::Cli::try_parse_from([
        "krabb",
        "plugin",
        "setup",
        "telegram",
        "--no-activate",
        "--json",
    ])
    .unwrap();

    assert!(matches!(
        parsed.command,
        crate::Command::Plugin {
            command: crate::PluginCommand::Setup(Options { no_activate: true, .. })
        }
    ));
}

#[tokio::test]
async fn no_daemon_and_offline_activation_report_saved_but_inactive() {
    let root = crate::tests::test_root("setup-offline");
    let destination = root.join("credentials.json");

    for offline in [true, false] {
        let result = finish(&root, "telegram", &destination, offline).await;

        assert_eq!(result["saved"], true);
        assert_eq!(result["active"], false);
    }
}

#[tokio::test]
async fn activation_sends_only_metadata_and_redacts_daemon_failures() {
    use krabb_core::{
        jsonl,
        types::{IpcRequest, IpcResponse},
    };

    use tokio::{io::BufReader, net::TcpListener};

    for success in [true, false] {
        let root = crate::tests::test_root("setup-ipc");

        fs::create_dir_all(&root).unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();

        fs::write(root.join("ipc.port"), listener.local_addr().unwrap().port().to_string())
            .unwrap();
        fs::write(root.join("ipc.token"), "local-fixture").unwrap();

        let worker = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (input, mut output) = stream.into_split();
            let request: IpcRequest =
                jsonl::read(&mut BufReader::new(input), 64 * 1024).await.unwrap().unwrap();
            assert_eq!(request.method, "plugin.setup");
            assert_eq!(request.params.as_object().unwrap().len(), 2);
            assert_eq!(request.params["id"], "telegram");
            let response = if success {
                IpcResponse::ok(request.id, json!({"loaded": true}))
            } else {
                IpcResponse::fail(request.id, -32000, "synthetic-sensitive-provider-error")
            };

            jsonl::write(&mut output, &response).await.unwrap();
        });

        let result = finish(&root, "telegram", &root.join("credentials.json"), false).await;

        worker.await.unwrap();

        assert_eq!(result["active"], success);
        assert!(!result.to_string().contains("synthetic-sensitive"));

        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn setup_detection_preserves_plugin_specific_subcommands() {
    assert!(requested(&["--json".into(), "--verbose".into(), "setup".into()]));
    assert!(!requested(&["login".into(), "setup".into()]));
    assert!(!requested(&[]));
}

#[test]
fn service_install_reuses_default_credentials() {
    let root = crate::tests::test_root("setup-service");
    let destination = root.join("credentials.json");

    save(&destination, BTreeMap::from([("KRABB_SETUP_A".into(), "synthetic".into())])).unwrap();

    let values =
        crate::service_environment_from(&root.join("service"), root.clone(), &BTreeMap::new())
            .unwrap();

    assert!(values.contains(&("KRABB_CREDENTIALS".into(), destination.display().to_string())));
    assert!(!crate::service_credentials_path(&root.join("service")).exists());

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn nonterminal_setup_fails_before_writing() {
    if io::stdin().is_terminal() && io::stderr().is_terminal() {
        return;
    }

    let root = fixture("telegram", &["KRABB_SETUP_A"], vec![]);

    let error =
        run(Options { id: "telegram".into(), no_activate: true }, &root, false).await.unwrap_err();

    assert!(error.to_string().contains("interactive terminal"));
    assert!(!root.join("credentials.json").exists());

    fs::remove_dir_all(root).unwrap();
}

const FILE_LIMIT: u64 = 1024 * 1024;

fn read(path: &Path) -> Result<BTreeMap<String, String>, Error> {
    crate::credentials::read(path, &path.parent().unwrap().join("credentials.key"))
}

fn save(path: &Path, values: BTreeMap<String, String>) -> Result<(), Error> {
    crate::credentials::save(
        path,
        &path.parent().unwrap().join("credentials.key"),
        values,
        Some(false),
    )
    .map(|_| ())
}

fn configure(root: &Path, manifest: &crate::Manifest) {
    let mut lock = crate::load_lock_at(root).unwrap();

    let entry = lock.plugins.get_mut(&manifest.id).unwrap();

    entry.variables = manifest.variables.clone();
    entry.settings = manifest.settings.clone();

    fs::write(
        root.join("plugins").join(&manifest.id).join("krabb-plugin.toml"),
        toml::to_string(manifest).unwrap(),
    )
    .unwrap();

    crate::save_lock_at(root, &lock).unwrap();
}

#[test]
fn ollama_setup_switches_hosts_without_changing_the_service() {
    let manifest: crate::Manifest = toml::from_str(include_str!(
        "../../../krabb-plugins/intelligence/ollama/krabb-plugin.toml"
    ))
    .unwrap();

    let root = fixture("ollama", &[], vec![]);

    manifest.validate().unwrap();
    configure(&root, &manifest);

    let fields = fields(&root, "ollama").unwrap();
    let destination = path(&root);
    let environment = crate::env_for(&manifest, &crate::Config::default(), &root).unwrap();

    assert!(environment.contains(&("KRABB_OLLAMA_HOST".into(), "https://ollama.com".into())));

    save(&destination, BTreeMap::from([("KRABB_TELEGRAM_TOKEN".into(), "retained".into())]))
        .unwrap();

    for (entered, expected, secret) in [
        ("", "https://ollama.com", "synthetic-ollama-secret"),
        ("http://127.0.0.1:11434", "http://127.0.0.1:11434", ""),
        ("http://localhost:11435", "http://localhost:11435", ""),
        ("https://ollama.com", "https://ollama.com", "synthetic-ollama-secret"),
    ] {
        let mut prompted = Vec::new();

        let values = collect(&fields, |field| {
            prompted.push(field.name.clone());

            Ok(if field.name == "KRABB_OLLAMA_HOST" { entered.into() } else { secret.into() })
        })
        .unwrap();

        assert_eq!(prompted, ["KRABB_OLLAMA_API_KEY", "KRABB_OLLAMA_HOST"]);
        assert_eq!(values["KRABB_OLLAMA_HOST"], expected);

        save(&destination, values).unwrap();

        assert!(activation(&root, "ollama", &destination.canonicalize().unwrap()).is_ok());
        assert!(!fs::read_to_string(&destination).unwrap().contains("synthetic-ollama-secret"));
        assert_eq!(read(&destination).unwrap()["KRABB_TELEGRAM_TOKEN"], "retained");

        let reloaded = crate::read_manifest_checked(&root.join("plugins/ollama")).unwrap();
        let environment = crate::env_for(&reloaded, &crate::Config::default(), &root).unwrap();

        let hosts = environment
            .iter()
            .filter(|(name, _)| name == "KRABB_OLLAMA_HOST")
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>();

        assert_eq!(hosts, [expected]);

        assert_eq!(
            environment
                .iter()
                .find(|(name, _)| name == "KRABB_OLLAMA_API_KEY")
                .map(|(_, value)| value.as_str()),
            (!secret.is_empty()).then_some(secret)
        );

        assert!(!environment.iter().any(|(name, _)| name == "KRABB_TELEGRAM_TOKEN"));
        assert!(!root.join("service").exists());
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn whatsapp_setup_persists_all_fields_for_activation_and_restart() {
    let manifest: crate::Manifest =
        toml::from_str(include_str!("../../../krabb-plugins/channels/whatsapp/krabb-plugin.toml"))
            .unwrap();

    let root = fixture("whatsapp", &[], vec![]);

    manifest.validate().unwrap();
    configure(&root, &manifest);

    let fields = fields(&root, "whatsapp").unwrap();
    let mut prompted = Vec::new();

    let values = collect(&fields, |field| {
        prompted.push(field.name.clone());

        Ok(match field.name.as_str() {
            "KRABB_WHATSAPP_PHONE" => "123456789".into(),
            "KRABB_WHATSAPP_GRAPH_URL" => "https://graph.example.invalid/v1.0".into(),
            "KRABB_WHATSAPP_LISTEN" => String::new(),
            _ => "synthetic-secret".into(),
        })
    })
    .unwrap();

    assert_eq!(
        prompted,
        [
            "KRABB_WHATSAPP_TOKEN",
            "KRABB_WHATSAPP_APP_SECRET",
            "KRABB_WHATSAPP_VERIFY",
            "KRABB_WHATSAPP_PHONE",
            "KRABB_WHATSAPP_GRAPH_URL",
            "KRABB_WHATSAPP_LISTEN",
        ]
    );

    for name in ["KRABB_WHATSAPP_TOKEN", "KRABB_WHATSAPP_APP_SECRET", "KRABB_WHATSAPP_VERIFY"] {
        assert_eq!(values[name], "synthetic-secret");
    }

    assert_eq!(values["KRABB_WHATSAPP_LISTEN"], "127.0.0.1:8787");
    assert_eq!(values["KRABB_WHATSAPP_PHONE"], "123456789");

    let destination = path(&root);
    let key = root.join("credentials.key");

    save(&destination, BTreeMap::from([("KRABB_TELEGRAM_TOKEN".into(), "retained".into())]))
        .unwrap();
    save(&destination, values.clone()).unwrap();

    let encrypted = fs::read_to_string(&destination).unwrap();

    assert!(!encrypted.contains("synthetic-secret"));
    assert!(!encrypted.contains("123456789"));
    assert_eq!(read(&destination).unwrap()["KRABB_TELEGRAM_TOKEN"], "retained");
    assert!(activation(&root, "whatsapp", &destination.canonicalize().unwrap()).is_ok());
    assert!(crate::ready_at("whatsapp", Some(&destination), &key));
    assert!(crate::ready_at("telegram", Some(&destination), &key));

    let environment = crate::env_for(&manifest, &crate::Config::default(), &root).unwrap();

    for (name, value) in &values {
        assert!(environment.contains(&(name.clone(), value.clone())), "{name}");
        assert_eq!(environment.iter().filter(|(key, _)| key == name).count(), 1);
    }

    assert!(!environment.iter().any(|(name, _)| name == "KRABB_TELEGRAM_TOKEN"));

    save(&destination, BTreeMap::from([("KRABB_WHATSAPP_PHONE".into(), "987654321".into())]))
        .unwrap();

    let reloaded = crate::read_manifest_checked(&root.join("plugins/whatsapp")).unwrap();

    let environment = crate::env_for(&reloaded, &crate::Config::default(), &root).unwrap();

    assert!(environment.contains(&("KRABB_WHATSAPP_PHONE".into(), "987654321".into())));

    for missing in [
        "KRABB_WHATSAPP_TOKEN",
        "KRABB_WHATSAPP_APP_SECRET",
        "KRABB_WHATSAPP_VERIFY",
        "KRABB_WHATSAPP_PHONE",
        "KRABB_WHATSAPP_GRAPH_URL",
    ] {
        let mut incomplete = values.clone();

        incomplete.remove(missing);
        fs::remove_file(&destination).unwrap();
        save(&destination, incomplete).unwrap();

        assert!(!crate::ready_at("whatsapp", Some(&destination), &key), "{missing}");
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn slack_setup_persists_conversations_for_activation_and_restart() {
    let manifest: crate::Manifest =
        toml::from_str(include_str!("../../../krabb-plugins/channels/slack/krabb-plugin.toml"))
            .unwrap();

    let root = fixture("slack", &[], vec![]);

    manifest.validate().unwrap();
    configure(&root, &manifest);

    let fields = fields(&root, "slack").unwrap();
    let mut prompted = Vec::new();

    let values = collect(&fields, |field| {
        prompted.push(field.name.clone());

        Ok(match field.name.as_str() {
            "KRABB_SLACK_CHANNELS" => "D0123456789,C9876543210".into(),
            _ => "synthetic-slack-secret".into(),
        })
    })
    .unwrap();

    assert_eq!(
        prompted,
        ["KRABB_SLACK_BOT_TOKEN", "KRABB_SLACK_APP_TOKEN", "KRABB_SLACK_CHANNELS"]
    );

    let destination = path(&root);
    let key = root.join("credentials.key");

    save(&destination, BTreeMap::from([("KRABB_TELEGRAM_TOKEN".into(), "retained".into())]))
        .unwrap();
    save(&destination, values.clone()).unwrap();

    let encrypted = fs::read_to_string(&destination).unwrap();

    assert!(!encrypted.contains("synthetic-slack-secret"));
    assert!(!encrypted.contains("D0123456789"));
    assert_eq!(read(&destination).unwrap()["KRABB_TELEGRAM_TOKEN"], "retained");
    assert!(activation(&root, "slack", &destination.canonicalize().unwrap()).is_ok());
    assert!(crate::ready_at("slack", Some(&destination), &key));

    let environment = crate::env_for(&manifest, &crate::Config::default(), &root).unwrap();

    for (name, value) in &values {
        assert!(environment.contains(&(name.clone(), value.clone())), "{name}");
        assert_eq!(environment.iter().filter(|(key, _)| key == name).count(), 1);
    }

    assert!(!environment.iter().any(|(name, _)| name == "KRABB_TELEGRAM_TOKEN"));

    save(&destination, BTreeMap::from([("KRABB_SLACK_CHANNELS".into(), "D1111111111".into())]))
        .unwrap();

    let reloaded = crate::read_manifest_checked(&root.join("plugins/slack")).unwrap();

    let environment = crate::env_for(&reloaded, &crate::Config::default(), &root).unwrap();

    assert!(environment.contains(&("KRABB_SLACK_CHANNELS".into(), "D1111111111".into())));

    assert!(
        !environment.contains(&("KRABB_SLACK_CHANNELS".into(), "D0123456789,C9876543210".into()))
    );

    assert!(crate::ready_at("slack", Some(&destination), &key));

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn settings_only_plugins_offer_setup_and_reject_unreviewed_defaults() {
    let root = fixture("custom", &[], vec![]);
    let mut manifest = crate::read_manifest_checked(&root.join("plugins/custom")).unwrap();

    manifest.settings = vec![Setting {
        name: "KRABB_SETUP_ADDRESS".into(),
        default: Some("127.0.0.1:8787".into()),
    }];

    configure(&root, &manifest);

    let fields = fields(&root, "custom").unwrap();

    assert!(crate::available_commands(&root).unwrap().contains_key("custom"));

    assert_eq!(
        crate::plugin_summary(&manifest, "ok")["setup_command"],
        "krabb plugin setup custom"
    );

    crate::external_command_at(
        vec!["custom".into(), "setup".into(), "--help".into()],
        false,
        root.clone(),
    )
    .await
    .unwrap();

    let environment = crate::env_for(&manifest, &crate::Config::default(), &root).unwrap();

    assert!(environment.contains(&("KRABB_SETUP_ADDRESS".into(), "127.0.0.1:8787".into())));
    assert!(!path(&root).exists());

    let values = collect(&fields, |_| Ok("127.0.0.1:9898".into())).unwrap();

    save(&path(&root), values).unwrap();

    let environment = crate::env_for(&manifest, &crate::Config::default(), &root).unwrap();

    assert!(environment.contains(&("KRABB_SETUP_ADDRESS".into(), "127.0.0.1:9898".into())));

    manifest.settings[0].default = Some("0.0.0.0:8787".into());
    fs::write(root.join("plugins/custom/krabb-plugin.toml"), toml::to_string(&manifest).unwrap())
        .unwrap();

    assert!(super::fields(&root, "custom").unwrap_err().to_string().contains("changed"));

    assert!(activation(&root, "custom", &path(&root).canonicalize().unwrap()).is_err());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn settings_preserve_secret_and_permission_metadata() {
    let root = fixture("custom", &["KRABB_SETUP_TOKEN"], vec![]);
    let mut manifest = crate::read_manifest_checked(&root.join("plugins/custom")).unwrap();

    manifest.permissions = vec!["filesystem".into()];

    manifest.settings = vec![Setting {
        name: "KRABB_SETUP_ADDRESS".into(),
        default: Some("127.0.0.1:8787".into()),
    }];

    let mut lock = crate::load_lock_at(&root).unwrap();

    lock.plugins.get_mut("custom").unwrap().permissions = manifest.permissions.clone();
    crate::save_lock_at(&root, &lock).unwrap();
    configure(&root, &manifest);

    let reloaded = crate::read_manifest_checked(&root.join("plugins/custom")).unwrap();
    let lock = crate::load_lock_at(&root).unwrap();

    for metadata in [
        serde_json::to_value(&reloaded).unwrap(),
        serde_json::to_value(&lock.plugins["custom"]).unwrap(),
        crate::plugin_summary(&reloaded, "ok"),
    ] {
        assert_eq!(metadata["permissions"], json!(["filesystem"]));
        assert_eq!(metadata["secrets"], json!(["KRABB_SETUP_TOKEN"]));
        assert_eq!(metadata["settings"], serde_json::to_value(&manifest.settings).unwrap());
        assert!(metadata.get("variables").is_none());
    }

    assert_eq!(
        fields(&root, "custom").unwrap(),
        [Field::secret("KRABB_SETUP_TOKEN".into()), manifest.settings[0].clone().into()]
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn settings_manifest_is_backward_compatible_and_rejects_invalid_declarations() {
    let legacy = "id='custom'\nversion='0.1.0'\ncapabilities=[]\nprotocol={major=0,minor=1}";
    let mut manifest: crate::Manifest = toml::from_str(legacy).unwrap();

    assert!(manifest.settings.is_empty());
    assert!(!toml::to_string(&manifest).unwrap().contains("settings"));

    manifest.validate().unwrap();

    for name in ["PATH", "KRABB_bad", "KRABB_BAD-NAME", ""] {
        manifest.settings = vec![Setting { name: name.into(), default: None }];

        assert!(manifest.validate().is_err());
    }

    let field = Setting { name: "KRABB_SETUP_ADDRESS".into(), default: None };

    manifest.settings = vec![field.clone(), field.clone()];

    assert!(manifest.validate().is_err());

    manifest.settings.pop();
    manifest.variables = vec![field.name.clone()];

    assert!(manifest.validate().is_err());

    manifest.variables.clear();

    for invalid in ["".into(), " ".into(), "private\nvalue".into(), "x".repeat(LIMIT + 1)] {
        manifest.settings[0].default = Some(invalid);

        let error = manifest.validate().unwrap_err();

        assert!(!error.contains("private"));
    }

    manifest.settings[0].default = Some("127.0.0.1:8787".into());
    manifest.validate().unwrap();

    let parsed: crate::Manifest = toml::from_str(&toml::to_string(&manifest).unwrap()).unwrap();

    assert_eq!(parsed.settings, manifest.settings);

    assert!(
        toml::from_str::<crate::Manifest>(&format!(
            "{legacy}\n[[settings]]\nname='KRABB_SETUP_ADDRESS'\nunknown=true"
        ))
        .is_err()
    );
}

#[test]
fn environment_overrides_saved_settings_and_blocks_misleading_activation() {
    const NAME: &str = "KRABB_WHATSAPP_LISTEN";

    if std::env::var_os("KRABB_SETUP_OVERRIDE_TEST").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "setup::tests::environment_overrides_saved_settings_and_blocks_misleading_activation",
            ])
            .env("KRABB_SETUP_OVERRIDE_TEST", "1")
            .env(NAME, "127.0.0.1:9898")
            .status()
            .unwrap();

        assert!(status.success());

        return;
    }

    let root = fixture("custom", &[], vec![]);
    let mut manifest = crate::read_manifest_checked(&root.join("plugins/custom")).unwrap();

    manifest.settings = vec![Setting { name: NAME.into(), default: Some("127.0.0.1:8787".into()) }];
    configure(&root, &manifest);
    save(&path(&root), BTreeMap::from([(NAME.into(), "127.0.0.1:8989".into())])).unwrap();

    let environment = crate::env_for(&manifest, &crate::Config::default(), &root).unwrap();

    assert!(environment.contains(&(NAME.into(), "127.0.0.1:9898".into())));
    assert_eq!(environment.iter().filter(|(name, _)| name == NAME).count(), 1);

    assert!(
        activation(&root, "custom", &path(&root).canonicalize().unwrap())
            .unwrap_err()
            .to_string()
            .contains("overriding setup environment")
    );

    assert_eq!(read(&path(&root)).unwrap()[NAME], "127.0.0.1:8989");

    fs::remove_dir_all(root).unwrap();
}
