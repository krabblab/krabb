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
    let names = vec!["KRABB_SETUP_A".into(), "KRABB_SETUP_B".into()];
    let values = collect(&names, |_| Ok("synthetic".into())).unwrap();

    assert_eq!(values.len(), 2);

    for invalid in ["".into(), " ".into(), "synthetic\nvalue".into(), "x".repeat(LIMIT + 1)] {
        let error = collect(&names, |_| Ok(invalid.clone())).unwrap_err().to_string();

        assert!(error.starts_with("Secret must be"));
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

    assert_eq!(names(&root, "telegram").unwrap(), ["KRABB_SETUP_A"]);

    assert!(names(&root, "../escape").is_err());
    assert!(names(&root, "missing").is_err());

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

    assert!(names(&root, "telegram").unwrap_err().to_string().contains("changed"));

    fs::remove_dir_all(root).unwrap();

    let root = fixture("ollama", &[], vec![]);

    assert!(names(&root, "ollama").unwrap_err().to_string().contains("no secrets"));
    assert!(!crate::available_commands(&root).unwrap().contains_key("ollama"));

    fs::remove_dir_all(root).unwrap();

    let root = fixture("config", &["KRABB_SETUP_A"], vec![]);

    assert!(!crate::available_commands(&root).unwrap().contains_key("config"));
    assert_eq!(names(&root, "config").unwrap(), ["KRABB_SETUP_A"]);

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
