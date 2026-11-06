use super::*;
use crate::{Cli, Command, DeliveryCli, DeliveryCommand, PluginCommand};
use clap::Parser;
use std::os::unix::fs::PermissionsExt;

fn enqueue(store: &mut Store, id: &str, channel: &str) {
    store.ensure("session", "default").unwrap();
    store
        .reply(
            "session",
            Message {
                id: id.into(),
                session: "session".into(),
                role: Role::Assistant,
                sender: None,
                content: vec![Content::Text { text: "reply".into() }],
            },
            id,
            channel,
            "chat",
            None,
            "reply",
        )
        .unwrap();
}

fn install(home: &Path, id: &str, capability: &str) {
    let folder = home.join("plugins").join(id);
    let binary = folder.join("bin").join(crate::plugin_name(id));
    let manifest = folder.join("krabb-plugin.toml");

    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();

    std::fs::write(&manifest, format!(
        "id = '{id}'\nversion = '0.1.0'\nprotocol = {{ major = 0, minor = 1 }}\ncapabilities = ['{capability}']\n"
    )).unwrap();

    std::fs::write(&binary, format!(r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    r = json.loads(line)
    result = {{'id': '{id}', 'version': '0.1.0', 'protocol': {{'major': 0, 'minor': 1}}, 'capabilities': ['{capability}']}} if r['method'] == 'hello' else {{}}
    print(json.dumps({{'jsonrpc': '2.0', 'id': r['id'], 'result': result}}), flush=True)
    if r['method'] == 'shutdown': break
"#)).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();

    let mut lock = crate::load_lock_at(home).unwrap();

    lock.plugins.insert(
        id.into(),
        crate::Entry {
            source: "fixture".into(),
            revision: "local".into(),
            pinned: false,
            default: false,
            hash: crate::digest(&manifest, &binary).unwrap(),
            version: "0.1.0".into(),
            protocol: krabb_core::types::Protocol::CURRENT,
            capabilities: vec![capability.into()],
            permissions: vec![],
            variables: vec![],
            settings: Vec::new(),
            linked: false,
            commands: vec![],
        },
    );

    crate::save_lock_at(home, &lock).unwrap();
}

#[test]
fn parses_explicit_cleanup_and_plugin_controls() {
    for action in ["attach", "detach", "start", "stop", "remove", "add"] {
        let cli = Cli::try_parse_from(["krabb", "plugin", action, "telegram"]).unwrap();

        assert!(matches!(cli.command, Command::Plugin { .. }));
    }

    for action in ["install", "add"] {
        assert!(matches!(
            Cli::try_parse_from(["krabb", "plugin", action, "telegram", "--link", "-y"])
                .unwrap().command,
            Command::Plugin { command: PluginCommand::Install(crate::PluginInstall {
                ids, link: true, yes: true, ..
            }) } if ids == ["telegram"]
        ));
    }

    assert!(matches!(
        Cli::try_parse_from(["krabb", "plugin", "remove", "telegram", "--force"]).unwrap().command,
        Command::Plugin {
            command: PluginCommand::Remove(crate::PluginUninstall { force: true, .. })
        }
    ));

    let cli =
        DeliveryCli::try_parse_from(["delivery", "drop", "--all", "--channel", "telegram", "-y"])
            .unwrap();

    assert!(matches!(
        cli.command,
        DeliveryCommand::Drop(crate::delivery::Drop { all: true, channel: Some(_), yes: true, .. })
    ));

    for args in [vec!["drop"], vec!["drop", "id", "--all"], vec!["drop", "--channel", "telegram"]] {
        assert!(DeliveryCli::try_parse_from(std::iter::once("delivery").chain(args)).is_err());
    }
}

#[tokio::test]
async fn bulk_drop_is_confirmed_scoped_durable_and_unblocks_offline_uninstall() {
    let state = super::tests::state_at("bulk-offline");
    let home = &state.home;

    install(home, "telegram", "channel");

    let mut store = crate::sessions_at(home).unwrap();

    for i in 0..13 {
        enqueue(&mut store, &format!("telegram-{i}"), "telegram");
    }

    enqueue(&mut store, "discord-1", "discord");
    store.uncertain("telegram-0", "timeout").unwrap();

    for params in [
        json!({"all": true}),
        json!({"all": true, "id": "telegram-0", "yes": true}),
        json!({"all": "yes", "yes": true}),
        json!({"all": true, "channel": 2, "yes": true}),
        json!({"all": true, "channel": "../bad", "yes": true}),
    ] {
        assert!(crate::delivery::apply(&mut store, &params).is_err());
        assert_eq!(store.outbox.len(), 14);
    }

    assert!(
        crate::delivery::drop(
            crate::delivery::Drop { all: true, ..Default::default() },
            home,
            true
        )
        .await
        .is_err()
    );

    assert!(crate::uninstall_plugin_at(home, "telegram", false).await.is_err());

    crate::delivery::drop(
        crate::delivery::Drop {
            all: true,
            channel: Some("telegram".into()),
            yes: true,
            ..Default::default()
        },
        home,
        true,
    )
    .await
    .unwrap();

    let store = crate::sessions_at(home).unwrap();

    assert_eq!(store.outbox.len(), 1);
    assert_eq!(store.outbox[0].channel, "discord");
    assert_eq!(store.dead.len(), 13);

    crate::delivery::drop(
        crate::delivery::Drop { all: true, yes: true, ..Default::default() },
        home,
        false,
    )
    .await
    .unwrap();

    crate::delivery::drop(
        crate::delivery::Drop { all: true, yes: true, ..Default::default() },
        home,
        true,
    )
    .await
    .unwrap();

    let removed = crate::uninstall_plugin_at(home, "telegram", false).await.unwrap();

    assert_eq!(removed["status"], "uninstalled");

    let store = crate::sessions_at(home).unwrap();

    assert!(store.outbox.is_empty());
    assert_eq!(store.dead.len(), 14);
    assert_eq!(store.sessions.len(), 1);

    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn live_controls_wait_for_work_and_preserve_installation_and_deliveries() {
    let state = Arc::new(super::tests::state_at("live-controls"));

    install(&state.home, "tools", "tool");
    enqueue(&mut state.sessions.lock().unwrap(), "one", "telegram");
    enqueue(&mut state.sessions.lock().unwrap(), "two", "discord");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    std::fs::write(state.home.join("ipc.port"), listener.local_addr().unwrap().port().to_string())
        .unwrap();
    std::fs::write(state.home.join("ipc.token"), "secret").unwrap();

    let server = tokio::spawn(serve(listener, Arc::clone(&state)));

    crate::lifecycle::run(&state.home, "tools", true, true).await.unwrap();

    let original = state.plugins.get("tools").await.unwrap();

    crate::lifecycle::run(&state.home, "tools", true, false).await.unwrap();

    assert!(Arc::ptr_eq(&original, &state.plugins.get("tools").await.unwrap()));

    for (method, params) in [
        ("plugin.detach", json!({"id": "tools"})),
        ("delivery.drop", json!({"all": true, "channel": "telegram", "yes": true})),
    ] {
        let work = state.plugins.lifecycle.read().await;
        let home = state.home.clone();
        let request = tokio::spawn(async move { call(home, method, params).await });

        timeout(Duration::from_secs(2), async {
            while state.plugins.lifecycle.try_read().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        assert!(!request.is_finished());
        assert_eq!(state.sessions.lock().unwrap().outbox.len(), 2);
        drop(work);
        timeout(Duration::from_secs(2), request).await.unwrap().unwrap().unwrap();
    }

    assert!(state.plugins.get("tools").await.is_none());

    assert!(crate::binary_at("tools", &state.home).is_some());
    assert_eq!(state.sessions.lock().unwrap().outbox.len(), 1);

    crate::lifecycle::run(&state.home, "tools", true, true).await.unwrap();
    crate::lifecycle::run(&state.home, "tools", false, false).await.unwrap();

    assert!(state.plugins.get("tools").await.is_none());

    crate::delivery::drop(
        crate::delivery::Drop { all: true, yes: true, ..Default::default() },
        &state.home,
        true,
    )
    .await
    .unwrap();

    assert_eq!(call(&state.home, "plugin.guard", json!({})).await.unwrap()["deliveries"], false);
    assert_eq!(state.sessions.lock().unwrap().dead.len(), 2);

    state.stop.signal();
    server.await.unwrap().unwrap();
    std::fs::remove_file(state.home.join("ipc.port")).unwrap();

    crate::lifecycle::run(&state.home, "tools", false, true).await.unwrap();

    assert!(crate::lifecycle::change(&state.home, "tools", true).await.is_err());
    assert!(crate::lifecycle::change(&state.home, "missing", false).await.is_err());
    assert!(crate::lifecycle::change(&state.home, "../bad", false).await.is_err());

    std::fs::remove_dir_all(&state.home).unwrap();
}

#[tokio::test]
async fn status_reports_all_live_channels_without_cli_credentials() {
    let state = Arc::new(super::tests::state_at("status-channels"));

    for (id, capability) in [("ollama", "model"), ("telegram", "channel"), ("whatsapp", "channel")]
    {
        install(&state.home, id, capability);

        let binary = crate::binary_at(id, &state.home).unwrap();
        let process = krabb_core::plugin::Process::start(&binary).await.unwrap();

        state.plugins.insert(crate::Live::new(process)).await;
    }

    std::fs::write(state.home.join("config.toml"), "provider = 'ollama'\n").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    std::fs::write(state.home.join("ipc.port"), listener.local_addr().unwrap().port().to_string())
        .unwrap();
    std::fs::write(state.home.join("ipc.token"), "secret").unwrap();

    let server = tokio::spawn(serve(listener, Arc::clone(&state)));
    let status = crate::status_at(&state.home).await.unwrap();

    assert_eq!(status["daemon"], "running");
    assert_eq!(status["health"], "healthy");
    assert_eq!(status["channels"]["plugins"], json!(["telegram", "whatsapp"]));
    assert_eq!(status["intelligence"]["plugins"], json!(["ollama"]));
    assert!(status.get("messaging").is_none());

    assert!(
        crate::status_text(&status, 3)
            .contains("Channels:           configured (telegram, whatsapp)")
    );

    for (id, _) in state.plugins.all().await {
        state.plugins.remove(&id).await.unwrap().stop().await.unwrap();
    }

    let status = crate::status_at(&state.home).await.unwrap();

    assert_eq!(status["daemon"], "running");
    assert_eq!(status["health"], "unhealthy");
    assert_eq!(status["channels"]["plugins"], json!([]));
    assert_eq!(status["intelligence"]["plugins"], json!([]));

    state.stop.signal();
    server.await.unwrap().unwrap();
    std::fs::remove_dir_all(&state.home).unwrap();
}

#[tokio::test]
async fn offline_status_accepts_whatsapp_without_default_telegram() {
    let state = super::tests::state_at("status-offline");

    install(&state.home, "ollama", "model");
    std::fs::write(state.home.join("config.toml"), "provider = 'ollama'\n").unwrap();

    let status = crate::status_at(&state.home).await.unwrap();

    assert_eq!(status["daemon"], "stopped");
    assert_eq!(status["health"], "healthy");
    assert_eq!(status["channels"]["status"], "not configured");

    install(&state.home, "whatsapp", "channel");

    crate::save_file(
        state.home.join("credentials.json"),
        serde_json::to_vec(&json!({
            "KRABB_WHATSAPP_TOKEN": "synthetic",
            "KRABB_WHATSAPP_APP_SECRET": "synthetic",
            "KRABB_WHATSAPP_VERIFY": "synthetic",
            "KRABB_WHATSAPP_PHONE": "123",
            "KRABB_WHATSAPP_GRAPH_URL": "http://127.0.0.1:9",
        }))
        .unwrap(),
    )
    .unwrap();

    let status = crate::status_at(&state.home).await.unwrap();

    assert_eq!(status["daemon"], "stopped");
    assert_eq!(status["health"], "healthy");
    assert_eq!(status["channels"]["plugins"], json!(["whatsapp"]));

    std::fs::remove_dir_all(&state.home).unwrap();
}

#[tokio::test(start_paused = true)]
async fn control_timeouts_preserve_state() {
    let state = super::tests::state_at("control-timeouts");

    enqueue(&mut state.sessions.lock().unwrap(), "one", "telegram");

    let _work = state.plugins.lifecycle.read().await;
    let request = IpcRequest::call(1, "secret", "delivery.drop", json!({"all": true, "yes": true}));

    assert!(drop_deliveries(&request, &state).await.error.is_some());

    let request = IpcRequest::call(2, "secret", "plugin.unload", json!({"id": "tools"}));

    assert!(unload(&request, &state).await.error.is_some());
    assert_eq!(state.sessions.lock().unwrap().outbox.len(), 1);
}

#[tokio::test]
async fn schedules_cannot_reattach_an_explicitly_detached_provider() {
    let state = super::tests::state_at("detached-provider");

    install(&state.home, "fixture", "model");

    let request = IpcRequest::call(1, "secret", "plugin.detach", json!({"id": "fixture"}));

    assert!(unload(&request, &state).await.error.is_none());

    let result = crate::execute_scheduled_task(
        1,
        &json!({}),
        &state.plugins,
        &state.sessions,
        &state.stop,
        &state.home,
        &state.root,
        &state.root,
        "fixture",
        "default",
        Arc::new(crate::Cancellation::new()),
        Arc::clone(&state.pending),
        &state.config,
        None,
        &mut 1,
    )
    .await;

    assert!(result.unwrap_err().to_string().contains("is detached"));
    assert!(state.plugins.get("fixture").await.is_none());

    let request = IpcRequest::call(2, "secret", "plugin.attach", json!({"id": "fixture"}));

    assert!(attach(&request, &state).await.error.is_none());
    assert!(!state.plugins.detached.read().await.contains("fixture"));

    state.plugins.get("fixture").await.unwrap().stop().await.unwrap();
    std::fs::remove_dir_all(&state.home).unwrap();
}

#[test]
fn bulk_drop_rolls_back_if_catalog_cannot_be_saved() {
    let state = super::tests::state_at("bulk-rollback");
    let mut store = crate::sessions_at(&state.home).unwrap();

    enqueue(&mut store, "one", "telegram");

    let directory = state.home.join("data/sessions");
    let backup = state.home.join("data/backup");

    std::fs::rename(&directory, &backup).unwrap();
    std::fs::write(&directory, "not a directory").unwrap();

    assert!(store.drop_deliveries(None).is_err());
    assert_eq!(store.outbox.len(), 1);
    assert!(store.dead.is_empty());

    std::fs::remove_file(&directory).unwrap();
    std::fs::rename(&backup, &directory).unwrap();

    assert_eq!(crate::sessions_at(&state.home).unwrap().outbox.len(), 1);

    std::fs::remove_dir_all(&state.home).unwrap();
}
