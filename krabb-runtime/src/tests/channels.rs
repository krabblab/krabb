use super::*;
use serde_json::json;
use std::collections::BTreeSet;

const CHANNEL: &str = r#"
import json, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
polled = False
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if 'id' not in request:
        continue
    with (root / 'channel.jsonl').open('a') as log:
        log.write(json.dumps(request) + '\n')
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': 'telegram',
                  'version': '0.1.0', 'capabilities': ['channel']}
    elif method == 'poll':
        result = {'events': [] if polled else [
            {'id': n, 'chat': 7, 'private': True, 'sender': 8, 'thread': '4', 'text': 'hello'}
            for n in [1, 2]]}
        polled = True
        time.sleep(0.01)
    elif method == 'typing':
        result = True
    else:
        result = {'message_id': 99, 'acknowledged': True}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
    if method == 'shutdown':
        break
"#;

const MODEL: &str = r#"
import json, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
generated = 0
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request['method']
    response = {'jsonrpc': '2.0', 'id': request['id']}
    if method == 'hello':
        response['result'] = {'protocol': {'major': 0, 'minor': 1}, 'id': 'fixture',
                              'version': '0.1.0', 'capabilities': ['model']}
    elif method == 'generate':
        generated += 1
        (root / 'model.json').write_text(json.dumps(request))
        (root / 'generated').write_text(str(generated))
        time.sleep(0.8)
        if generated == 1:
            response['error'] = {'code': -32000, 'message': 'Unsupported model: synthetic'}
        else:
            response['result'] = {'text': '**Recovered**', 'stop': 'stop'}
    else:
        response['result'] = {}
    print(json.dumps(response), flush=True)
    if method == 'shutdown':
        break
"#;

fn requests(root: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(root.join("channel.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[test]
fn sends_edits_and_queued_deliveries_negotiate_formatting() {
    let text = "**Krabb**: [repository](https://github.com/krabblab/krabb)";

    let expected = "Krabb: repository (https://github.com/krabblab/krabb)";

    for channel in ["telegram", "discord", "signal", "slack", "whatsapp", "custom"] {
        let params = crate::send_params(channel, "delivery", "7", text, Some("4")).unwrap();

        assert_eq!(params["text"], text);
        assert_eq!(params["format"], "markdown");

        let request = crate::messaging::prepare(Request::call(1, "send", params.clone()), &[]);
        let Request::Call { params: fallback, .. } = request else { panic!() };

        assert_eq!(fallback["text"], expected);
        assert!(fallback.get("format").is_none());

        let request = crate::messaging::prepare(
            Request::call(1, "send", params.clone()),
            &["markdown".into()],
        );

        let Request::Call { params: rich, .. } = request else { panic!() };

        assert_eq!(rich, params);
    }

    for channel in ["telegram", "discord"] {
        let params = crate::edit_params(channel, "delivery", "7", "42", text, Some("4")).unwrap();

        assert_eq!(params["text"], text);
        assert_eq!(params["format"], "markdown");
    }

    let mut store = crate::state::Store::default();

    store.queue_delivery("queued", "telegram", "7", true, Some("4".into()), text).unwrap();

    let Request::Call { params, .. } =
        crate::delivery_request("telegram", 1, &store.outbox[0]).unwrap()
    else {
        panic!()
    };

    assert_eq!(params["text"], text);
    assert_eq!(params["format"], "markdown");
    assert_eq!(params["thread"], "4");
    assert_eq!(store.outbox[0].text, text);

    let references =
        format!("{}\n\n[s]: https://example.com/{}", "[s][s] ".repeat(50), "x".repeat(100));

    assert!(references.len() < 4096);
    assert!(!crate::stream_fits("telegram", &references));
}

#[tokio::test]
async fn channel_turns_type_and_do_not_replay_model_rejections() {
    let root = test_root("channel-rejection");

    fs::create_dir_all(&root).unwrap();

    let channel = Process::start_with("python3", ["-u", "-c", CHANNEL, root.to_str().unwrap()])
        .await
        .unwrap();

    let model =
        Process::start_with("python3", ["-u", "-c", MODEL, root.to_str().unwrap()]).await.unwrap();

    let plugins = registry([channel, model]).await;

    let sessions =
        Arc::new(Mutex::new(crate::state::Store::load(root.join("sessions.json")).unwrap()));

    let stop = Arc::new(Stop::new());
    let monitor_root = root.clone();
    let signal = Arc::clone(&stop);

    let monitor = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if requests(&monitor_root).iter().filter(|value| value["method"] == "send").count()
                    >= 2
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    break;
                }

                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        signal.signal();
    });

    tokio::time::timeout(
        Duration::from_secs(10),
        crate::bridge(
            &plugins,
            "telegram",
            "fixture",
            "unset",
            Arc::clone(&sessions),
            Arc::clone(&stop),
            Arc::new(Mutex::new(BTreeMap::new())),
            BTreeMap::new(),
            crate::ApprovalMode::Off,
        ),
    )
    .await
    .unwrap()
    .unwrap();

    monitor.await.unwrap();
    stop_registry(&plugins).await;

    assert_eq!(fs::read_to_string(root.join("generated")).unwrap(), "2");

    let request = fs::read_to_string(root.join("model.json")).unwrap();

    assert!(request.contains("portable Markdown"));
    assert!(request.contains("exact URL supplied by the user"));

    let calls = requests(&root);
    let typing: Vec<_> = calls.iter().filter(|value| value["method"] == "typing").collect();

    assert_eq!(typing.len(), 2);

    assert!(typing.iter().all(|value| value["params"]
        == json!({"chat":7,"thread":"4","message":value["params"]["message"],"private":true})));

    assert_eq!(typing[0]["params"]["message"], "1");
    assert_eq!(typing[1]["params"]["message"], "2");

    let sent: Vec<_> = calls.iter().filter(|value| value["method"] == "send").collect();

    assert_eq!(sent.len(), 2);
    assert!(sent.iter().any(|value| value["params"]["text"] == "Recovered"));
    assert!(sent.iter().any(|value| value["params"]["text"] == "I could not complete that turn."));
    assert!(sessions.lock().unwrap().sessions["telegram-7-thread-4"].queued.is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_context_rejection_sends_once_per_message_without_restarting_or_requeueing() {
    let root = test_root("channel-local-limit");

    fs::create_dir_all(&root).unwrap();

    let script = MODEL.replace("['model']", "['model','context']")
        .replace("    elif method == 'generate':", "    elif method == 'model.info':\n        response['result'] = {'window': {'tokens': 64, 'output': 32}}\n    elif method == 'generate':")
        .replace("    if method == 'hello':", "    if method == 'hello':\n        with (root / 'starts').open('a') as log: log.write('start\\n')");

    let channel = Process::start_with("python3", ["-u", "-c", CHANNEL, root.to_str().unwrap()])
        .await
        .unwrap();

    let model = Process::start_with("python3", ["-u", "-c", &script, root.to_str().unwrap()])
        .await
        .unwrap();

    let plugins = registry([channel, model]).await;

    let sessions =
        Arc::new(Mutex::new(crate::state::Store::load(root.join("sessions.json")).unwrap()));

    let stop = Arc::new(Stop::new());
    let monitor_root = root.clone();
    let signal = Arc::clone(&stop);

    let monitor = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(8), async {
            while requests(&monitor_root).iter().filter(|item| item["method"] == "send").count() < 2
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }

            tokio::time::sleep(Duration::from_millis(250)).await;
        })
        .await
        .unwrap();
        signal.signal();
    });

    crate::bridge(
        &plugins,
        "telegram",
        "fixture",
        "small",
        Arc::clone(&sessions),
        Arc::clone(&stop),
        Arc::new(Mutex::new(BTreeMap::new())),
        BTreeMap::new(),
        crate::ApprovalMode::Off,
    )
    .await
    .unwrap();

    monitor.await.unwrap();
    crate::tests::stop_registry(&plugins).await;

    assert_eq!(requests(&root).iter().filter(|item| item["method"] == "send").count(), 2);
    assert_eq!(fs::read_to_string(root.join("starts")).unwrap(), "start\n");
    assert!(!root.join("generated").exists());
    assert!(sessions.lock().unwrap().sessions["telegram-7-thread-4"].queued.is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn typing_is_optional_and_bounded() {
    for (name, handler) in [
        ("unsupported", "response = {'error': {'code': -32601, 'message': 'Unknown method'}}"),
        ("stalled", "time.sleep(10); response = {'result': True}"),
    ] {
        let script = format!(
            r#"
import json, sys, time
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    if request['method'] == 'hello':
        response = {{'result': {{'protocol': {{'major': 0, 'minor': 1}}, 'id': 'fixture', 'version': '0.1.0', 'capabilities': ['channel']}}}}
    elif request['method'] == 'typing':
        {handler}
    else:
        break
    response.update({{'jsonrpc': '2.0', 'id': request['id']}})
    print(json.dumps(response), flush=True)
"#
        );

        let channel =
            Live::new(Process::start_with("python3", ["-u", "-c", &script]).await.unwrap());
        let mut call = 7;

        assert!(
            !crate::indicate_typing(&channel, "telegram", "invalid", None, &json!({}), &mut call)
                .await
        );

        assert_eq!(call, 7);
        let outcome = tokio::time::timeout(
            Duration::from_secs(3),
            crate::indicate_typing(&channel, "telegram", "7", None, &json!({}), &mut call),
        )
        .await;

        assert!(!outcome.unwrap(), "{name}");
        assert_eq!(call, 8);
        channel.stop().await.unwrap();
    }
}

const CONCURRENT: &str = r#"
import json, pathlib, sys, time
root, name, capability, peer = pathlib.Path(sys.argv[1]), *sys.argv[2:]
for line in sys.stdin:
    request = json.loads(line)

    if 'id' not in request:
        continue

    method = request['method']
    with (root / (name + '.jsonl')).open('a') as log:
        log.write(json.dumps(request) + '\n')

    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': name,
                  'version': '0.1.0', 'capabilities': [capability]}
    elif method == 'poll':
        if peer == 'blocked':
            (root / (name + '.polling')).touch()
            time.sleep(30)

        time.sleep(0.02)
        result = {'events': [] if request['params'].get('offset', 0) else [
            {'id': 1, 'chat': 7, 'private': True, 'sender': 8, 'text': name}]}
    elif method == 'generate':
        (root / (name + '.started')).touch()
        deadline = time.monotonic() + 10

        while not (root / (peer + '.started')).exists():
            if time.monotonic() > deadline:
                raise RuntimeError('The other provider never ran concurrently')
            time.sleep(0.01)
        result = {'text': name + ' answered', 'stop': 'stop'}
    else:
        result = {'message_id': 99, 'acknowledged': True}

    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
    if method == 'shutdown':
        break
"#;

async fn concurrent_plugin(root: &Path, id: &str, capability: &str, peer: &str) -> Process {
    Process::start_with(
        "python3",
        ["-u", "-c", CONCURRENT, root.to_str().unwrap(), id, capability, peer],
    )
    .await
    .unwrap()
}

fn concurrent_requests(root: &Path, id: &str, method: &str) -> Vec<serde_json::Value> {
    fs::read_to_string(root.join(format!("{id}.jsonl")))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|request| request["method"] == method)
        .collect()
}

#[tokio::test]
async fn simultaneous_channels_route_providers_and_claim_deliveries_once() {
    let root = test_root("simultaneous-channels");

    fs::create_dir_all(&root).unwrap();

    let plugins = Plugins::default();
    let sessions = Arc::new(Mutex::new(crate::state::Store::default()));

    sessions
        .lock()
        .unwrap()
        .queue_delivery("queued", "telegram", "7", true, None, "queued")
        .unwrap();

    let stop = Arc::new(Stop::new());

    let policies = [("telegram", "alpha"), ("discord", "beta")]
        .into_iter()
        .map(|(channel, provider)| {
            (
                channel.into(),
                crate::ChannelConfig {
                    provider: Some(provider.into()),
                    model: Some(format!("{provider}-model")),
                    ..Default::default()
                },
            )
        })
        .collect();

    let worker = tokio::spawn(crate::channels::run(crate::channels::Context {
        plugins: plugins.clone(),
        sessions: Arc::clone(&sessions),
        stop: Arc::clone(&stop),
        cancels: Arc::new(Mutex::new(BTreeMap::new())),
        policies,
        provider: "unavailable".into(),
        model: "wrong-default".into(),
        approval_mode: crate::ApprovalMode::Off,
        home: root.clone(),
        media: root.join("media"),
        approvals: Arc::new(crate::AsyncMutex::new(crate::approval::Gate::new().unwrap())),
    }));

    // Start with no plugins, then add channels before their providers are ready.
    tokio::time::sleep(Duration::from_millis(300)).await;

    for id in ["telegram", "discord"] {
        plugins.insert(Live::new(concurrent_plugin(&root, id, "channel", "").await)).await;
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    for (id, peer) in [("alpha", "beta"), ("beta", "alpha")] {
        plugins.insert(Live::new(concurrent_plugin(&root, id, "model", peer).await)).await;
    }

    let completed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if concurrent_requests(&root, "telegram", "send").len() == 2
                && concurrent_requests(&root, "discord", "send").len() == 1
                && sessions.lock().unwrap().outbox.is_empty()
            {
                break;
            }

            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    // Unloading one channel leaves the other worker polling; reloading preserves its cursor.
    let removed = plugins.remove("telegram").await.unwrap();

    tokio::time::sleep(Duration::from_millis(350)).await;
    removed.stop().await.unwrap();

    let polls = concurrent_requests(&root, "discord", "poll").len();

    plugins.insert(Live::new(concurrent_plugin(&root, "telegram", "channel", "").await)).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    stop.signal();

    tokio::time::timeout(Duration::from_secs(5), worker).await.unwrap().unwrap().unwrap();
    stop_registry(&plugins).await;
    completed.unwrap();

    assert!(concurrent_requests(&root, "discord", "poll").len() > polls);
    assert_eq!(concurrent_requests(&root, "telegram", "send").len(), 2);
    assert_eq!(concurrent_requests(&root, "discord", "send").len(), 1);

    let alpha = concurrent_requests(&root, "alpha", "generate");
    let beta = concurrent_requests(&root, "beta", "generate");

    assert_eq!(alpha.len(), 1);
    assert_eq!(beta.len(), 1);
    assert_ne!(alpha[0]["id"], beta[0]["id"]);
    assert_eq!(alpha[0]["params"]["model"], "alpha-model");
    assert_eq!(beta[0]["params"]["model"], "beta-model");

    let store = sessions.lock().unwrap();

    assert_eq!(store.offset("telegram"), 2);
    assert_eq!(store.offset("discord"), 2);
    assert!(store.sessions.contains_key("telegram-7"));
    assert!(store.sessions.contains_key("discord-7"));

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stalled_channel_and_missing_provider_do_not_block_other_channels_or_shutdown() {
    let root = test_root("channel-isolation");

    fs::create_dir_all(&root).unwrap();

    let plugins = registry([
        concurrent_plugin(&root, "telegram", "channel", "").await,
        concurrent_plugin(&root, "discord", "channel", "blocked").await,
        concurrent_plugin(&root, "whatsapp", "channel", "").await,
        concurrent_plugin(&root, "alpha", "model", "alpha").await,
    ])
    .await;

    let stop = Arc::new(Stop::new());

    let policies = BTreeMap::from([(
        "whatsapp".into(),
        crate::ChannelConfig { provider: Some("missing".into()), ..Default::default() },
    )]);

    let worker = tokio::spawn(crate::channels::run(crate::channels::Context {
        plugins: plugins.clone(),
        sessions: Arc::new(Mutex::new(crate::state::Store::default())),
        stop: Arc::clone(&stop),
        cancels: Arc::new(Mutex::new(BTreeMap::new())),
        policies,
        provider: "alpha".into(),
        model: "unset".into(),
        approval_mode: crate::ApprovalMode::Off,
        home: root.clone(),
        media: root.join("media"),
        approvals: Arc::new(crate::AsyncMutex::new(crate::approval::Gate::new().unwrap())),
    }));

    let completed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !concurrent_requests(&root, "telegram", "send").is_empty()
                && root.join("discord.polling").is_file()
            {
                break;
            }

            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    stop.signal();

    tokio::time::timeout(Duration::from_secs(3), worker).await.unwrap().unwrap().unwrap();
    stop_registry(&plugins).await;
    completed.unwrap();

    assert!(concurrent_requests(&root, "whatsapp", "poll").is_empty());
    assert_eq!(concurrent_requests(&root, "telegram", "send").len(), 1);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn group_workspace_failures_reply_without_running_the_model() {
    check_group_workspace(true);
}

#[test]
fn group_chat_works_in_a_fresh_workspace_by_default() {
    check_group_workspace(false);
}

#[test]
fn private_worktree_failures_reply_without_running_the_model() {
    check_workspace(true, true, false);
}

#[test]
fn private_chats_have_distinct_worktrees_and_durable_local_history() {
    check_workspace(true, true, true);
}

#[test]
fn group_chats_have_distinct_worktrees_and_durable_local_history() {
    check_workspace(true, false, true);
}

fn check_group_workspace(isolated: bool) {
    check_workspace(isolated, false, false);
}

fn check_workspace(isolated: bool, private: bool, prepared: bool) {
    let root = test_root("group-workspace-failure");

    crate::init_at(&root).unwrap();

    assert!(!root.join("workspace/.git").exists());
    assert!(crate::isolate_at("telegram-7", &root).is_err());

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tests::channels::workspace_failure_worker", "--ignored", "--nocapture"])
        .env("KRABB_HOME", &root)
        .env("KRABB_ROOT", root.join("workspace"))
        .env("KRABB_TEST_ISOLATION", if isolated { "1" } else { "0" })
        .env("KRABB_TEST_PRIVATE", if private { "1" } else { "0" })
        .env("KRABB_TEST_PREPARED", if prepared { "1" } else { "0" })
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "isolated workspace fixture"]
async fn workspace_failure_worker() {
    let root = crate::home();
    let private = std::env::var("KRABB_TEST_PRIVATE").unwrap() == "1";
    let prepared = std::env::var("KRABB_TEST_PREPARED").unwrap() == "1";

    let mut script = if private {
        CHANNEL.to_owned()
    } else {
        CHANNEL.replace("'private': True", "'private': False")
    };

    if prepared {
        use clap::Parser;

        let cli = crate::Cli::try_parse_from(["krabb", "worktree", "setup", "telegram", "--yes"])
            .unwrap();

        let crate::Command::Worktree { command } = cli.command else { panic!() };

        crate::worktree::run(command, &root, true).await.unwrap();
        fs::write(root.join("data/system/KRAB.md"), "host-system-fixture").unwrap();

        for id in ["telegram-7", "telegram-8"] {
            let checkout = crate::worktree::conversation(id, &root).unwrap();

            fs::write(checkout.join("KRAB.md"), "untrusted-workspace-identity").unwrap();
        }

        script = script.replace("'chat': 7", "'chat': (7+n%2)");
    }

    let channel = Process::start_with("python3", ["-u", "-c", &script, root.to_str().unwrap()])
        .await
        .unwrap();

    let model =
        Process::start_with("python3", ["-u", "-c", MODEL, root.to_str().unwrap()]).await.unwrap();

    let plugins = registry([channel, model]).await;

    let sessions = Arc::new(Mutex::new(
        crate::state::Store::load(root.join("data/sessions/index.json")).unwrap(),
    ));

    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);
    let observed = root.clone();

    let monitor = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if requests(&observed).iter().filter(|request| request["method"] == "send").count()
                    >= 2
                {
                    break;
                }

                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        signal.signal();
    });

    let isolated = std::env::var("KRABB_TEST_ISOLATION").unwrap() == "1";

    let mut policy =
        crate::ChannelConfig { allow: vec!["7".into(), "8".into()], ..Default::default() };

    if isolated {
        policy.worktree = true;
    }

    tokio::time::timeout(
        Duration::from_secs(10),
        crate::bridge(
            &plugins,
            "telegram",
            "fixture",
            "unset",
            Arc::clone(&sessions),
            Arc::clone(&stop),
            Arc::new(Mutex::new(BTreeMap::new())),
            BTreeMap::from([("telegram".into(), policy.clone())]),
            crate::ApprovalMode::Off,
        ),
    )
    .await
    .unwrap()
    .unwrap();

    monitor.await.unwrap();

    let sent: Vec<_> =
        requests(&root).into_iter().filter(|request| request["method"] == "send").collect();

    assert_eq!(sent.len(), 2);

    if !isolated || prepared {
        if prepared {
            let stored = crate::state::Store::load(root.join("data/sessions/index.json")).unwrap();

            assert_eq!(stored.sessions.len(), 2);

            let request = fs::read_to_string(root.join("model.json")).unwrap();

            assert!(request.contains("host-system-fixture"));
            assert!(!request.contains("untrusted-workspace-identity"));

            let mut paths = BTreeSet::new();

            for session in stored.sessions.values() {
                let path = session.isolated.as_ref().unwrap();

                paths.insert(path.clone());

                assert!(
                    root.join("data/workspaces/conversations")
                        .join(&session.id)
                        .join("sessions/records")
                        .is_dir()
                );

                assert!(!path.join(".krabb").exists());
                assert_eq!(session.private, private);
                assert!(!session.messages.is_empty());
            }

            assert_eq!(paths.len(), 2);
            assert_eq!(fs::read_dir(root.join("data/sessions/records")).unwrap().count(), 0);
        } else {
            assert!(!root.join("workspace/.git").exists());
            assert!(!root.join("workspace/.krabb/worktrees").exists());
        }

        assert_eq!(fs::read_to_string(root.join("generated")).unwrap(), "2");
        assert!(sent.iter().any(|request| request["params"]["text"] == "Recovered"));

        stop_registry(&plugins).await;
        return;
    }

    assert!(sent.iter().all(|request| {
        request["params"]["text"].as_str().unwrap().contains("channels.telegram.worktree=false")
    }));

    assert!(sent.iter().all(|request| request["params"]["thread"] == "4"));
    assert!(!root.join("generated").exists());
    assert!(sessions.lock().unwrap().sessions.is_empty());

    let channel = plugins.get("telegram").await.unwrap();
    let event = json!({"id":3,"chat":7,"private":false,"sender":8,"thread":"4","text":"queued"});

    assert!(
        crate::queue_during_turn(
            &channel, "telegram", "unset", &event, &policy, &sessions, &mut 0, &mut 100
        )
        .await
        .unwrap()
    );

    let stored = crate::state::Store::load(root.join("data/sessions/index.json")).unwrap();

    let delivery =
        stored.outbox.iter().find(|delivery| delivery.id == "workspace-telegram-3").unwrap();

    assert_eq!(delivery.thread.as_deref(), Some("4"));
    assert!(delivery.text.contains("isolated workspace"));
    assert!(stored.sessions.is_empty());
    assert!(stored.known("telegram", "3"));

    stop_registry(&plugins).await;
}

#[tokio::test]
async fn typing_preserves_incoming_message_identity_and_conversation_scope() {
    for (channel_id, chat, private, expected) in [
        ("discord", "123", false, json!({"channel":"123","message":"incoming","private":false})),
        (
            "whatsapp",
            "123",
            true,
            json!({"chat":"123","thread":"456","message":"incoming","private":true}),
        ),
        (
            "signal",
            "group",
            false,
            json!({"chat":"group","thread":"456","message":"incoming","private":false}),
        ),
        (
            "slack",
            "D123",
            true,
            json!({"chat":"D123","thread":"456","message":"incoming","private":true}),
        ),
    ] {
        let script = r#"
import json, sys
expected = json.loads(sys.argv[1])

for line in sys.stdin:
    request = json.loads(line)

    if 'id' not in request: continue
    method = request['method']

    if method == 'hello':
        result = {'protocol': {'major':0, 'minor':1}, 'id':'fixture', 'version':'0.1.0', 'capabilities':['channel']}
    elif method == 'typing':
        result = request['params'] == expected
    else:
        result = {}

    print(json.dumps({'jsonrpc':'2.0', 'id':request['id'], 'result':result}), flush=True)
    if method == 'shutdown': break
"#;

        let expected = expected.to_string();

        let channel = Live::new(
            Process::start_with("python3", ["-u", "-c", script, &expected]).await.unwrap(),
        );

        assert!(
            crate::indicate_typing(
                &channel,
                channel_id,
                chat,
                Some("456"),
                &json!({"id":"incoming","private":private}),
                &mut 7
            )
            .await
        );

        channel.stop().await.unwrap();
    }
}
