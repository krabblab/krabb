use super::*;
use crate::{ApprovalMode, AsyncMutex, ChannelConfig, approval, channels, state};
use serde_json::Value;

const FIXTURE: &str = r#"
import json, pathlib, sys, time
root, name, capability, mode = pathlib.Path(sys.argv[1]), *sys.argv[2:]
initial, callback, token, generated = False, False, None, 0
for line in sys.stdin:
    request = json.loads(line)
    if request.get('method') == 'visible':
        request['method'], request['params'] = request['params']['method'], request['params']['params']
    if 'id' not in request:
        continue
    method = request['method']
    with (root / (name + '.jsonl')).open('a') as log:
        log.write(json.dumps(request) + '\n')
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': name,
                  'version': '0.1.0', 'capabilities': [capability]}
    elif method == 'poll':
        if mode == 'blocked':
            (root / (name + '.polling')).touch()
            time.sleep(30)
        time.sleep(0.02)
        events = []
        if mode == 'approval' and not initial:
            initial = True
            events = [{'id': 1, 'chat': 7, 'private': True, 'sender': 8, 'text': 'write a note'}]
        elif token and not callback and (root / (name + '.allow')).exists():
            callback = True
            events = [{'id': 2, 'kind': 'callback', 'chat': 7, 'private': True,
                       'sender': 8, 'callback_id': 'callback', 'data': token}]
        result = {'events': events}
    elif method == 'approval':
        token = request['params']['approve']
        (root / (name + '.pending')).touch()
        result = {'sent': True}
    elif method == 'generate':
        generated += 1
        if mode == 'approval' and generated == 1:
            result = {'text': '', 'stop': 'tool', 'events': [{'kind': 'tool', 'name': 'write',
                      'args': {'path': name + '.txt', 'text': name}}]}
        else:
            result = {'text': request['params']['messages'][-1]['content'][0]['text'], 'stop': 'stop'}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'write', 'schema': {'type': 'object', 'properties': {
                  'path': {'type': 'string'}, 'text': {'type': 'string'}}}}]}
    elif method == 'write':
        (root / request['params']['path']).touch()
        result = {'text': 'Written'}
    elif method == 'typing':
        result = True
    else:
        result = {'message_id': 99, 'acknowledged': True}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
    if method == 'shutdown':
        break
"#;

async fn plugin(root: &Path, id: &str, capability: &str, mode: &str) -> Process {
    Process::start_with(
        "python3",
        ["-u", "-c", FIXTURE, root.to_str().unwrap(), id, capability, mode],
    )
    .await
    .unwrap()
}

fn requests(root: &Path, id: &str, method: &str) -> Vec<Value> {
    fs::read_to_string(root.join(format!("{id}.jsonl")))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|request| request["method"] == method)
        .collect()
}

async fn until(mut ready: impl FnMut() -> bool) {
    while !ready() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn context(root: &Path, plugins: Plugins, store: state::Store) -> channels::Context {
    channels::Context {
        plugins,
        sessions: Arc::new(Mutex::new(store)),
        stop: Arc::new(Stop::new()),
        cancels: Arc::new(Mutex::new(BTreeMap::new())),
        policies: BTreeMap::new(),
        provider: "alpha".into(),
        model: "default".into(),
        approval_mode: ApprovalMode::Off,
        home: root.into(),
        media: root.join("media"),
        approvals: Arc::new(AsyncMutex::new(approval::Gate::new().unwrap())),
    }
}

#[tokio::test]
async fn queued_messages_require_exact_channel_ownership() {
    let root = test_root("queue-ownership");

    fs::create_dir_all(&root).unwrap();

    let plugins = registry([
        plugin(&root, "telegram", "channel", "queue").await,
        plugin(&root, "telegram-alt", "channel", "queue").await,
        plugin(&root, "alpha", "model", "queue").await,
    ])
    .await;

    let mut store = state::Store::default();

    for (id, channel, chat, text) in [
        ("telegram-alt-7", Some("telegram-alt"), Some("7"), "alternate private text"),
        ("renamed", Some("telegram"), Some("8"), "owned text"),
        ("telegram-9", None, None, "legacy private text"),
        ("telegram-incomplete", Some("telegram"), None, "incomplete route"),
    ] {
        store.create(id, "default").unwrap();

        if let (Some(channel), Some(chat)) = (channel, chat) {
            store.route(id, channel, chat, None, true).unwrap();
        } else {
            store.sessions.get_mut(id).unwrap().channel = channel.map(str::to_owned);
        }

        store
            .queue(
                id,
                Message {
                    id: format!("queued-{id}"),
                    session: id.into(),
                    role: Role::User,
                    sender: Some("8".into()),
                    content: vec![Content::Text { text: text.into() }],
                },
            )
            .unwrap();
    }

    let mut context = context(&root, plugins.clone(), store);

    context.policies.insert(
        "telegram-alt".into(),
        ChannelConfig { provider: Some("beta".into()), ..Default::default() },
    );

    let sessions = Arc::clone(&context.sessions);
    let stop = Arc::clone(&context.stop);
    let worker = tokio::spawn(channels::run(context));

    let outcome = tokio::time::timeout(Duration::from_secs(8), async {
        until(|| requests(&root, "telegram", "poll").len() >= 3).await;

        assert_eq!(requests(&root, "alpha", "generate").len(), 1);
        assert_eq!(requests(&root, "telegram", "send")[0]["params"]["text"], "owned text");
        assert_eq!(sessions.lock().unwrap().sessions["telegram-alt-7"].queued.len(), 1);

        plugins.insert(Live::new(plugin(&root, "beta", "model", "queue").await)).await;
        until(|| {
            let store = sessions.lock().unwrap();
            !requests(&root, "telegram-alt", "send").is_empty()
                && store.sessions["telegram-alt-7"].queued.is_empty()
                && store.outbox.is_empty()
        })
        .await;
    })
    .await;

    stop.signal();
    tokio::time::timeout(Duration::from_secs(5), worker).await.unwrap().unwrap().unwrap();
    stop_registry(&plugins).await;
    outcome.unwrap();

    assert_eq!(requests(&root, "alpha", "generate").len(), 1);
    assert_eq!(requests(&root, "beta", "generate").len(), 1);
    assert_eq!(requests(&root, "telegram", "send").len(), 1);

    assert_eq!(
        requests(&root, "telegram-alt", "send")[0]["params"]["text"],
        "alternate private text"
    );

    let store = sessions.lock().unwrap();

    assert_eq!(store.sessions["telegram-9"].queued.len(), 1);
    assert_eq!(store.sessions["telegram-incomplete"].queued.len(), 1);
    assert!(store.sessions["renamed"].queued.is_empty());
    assert_eq!(store.sessions["renamed"].messages.len(), 2);
    assert!(!store.sessions.contains_key("telegram-8"));

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_resolution_wakes_a_blocked_approval_poll() {
    let root = test_root("local-approval-wakeup");

    fs::create_dir_all(&root).unwrap();

    let channel = Arc::new(Live::new(plugin(&root, "telegram", "channel", "blocked").await));
    let approvals = Arc::new(AsyncMutex::new(approval::Gate::new().unwrap()));

    let target = approval::Target {
        channel: "telegram".into(),
        chat: "7".into(),
        thread: None,
        session: "telegram-7".into(),
        tool: "write".into(),
        args: serde_json::json!({}),
    };

    let challenge = approvals.lock().await.issue(target.clone()).unwrap();

    let unrelated = approvals
        .lock()
        .await
        .issue(approval::Target { channel: "discord".into(), ..target })
        .unwrap();

    let id = approvals
        .lock()
        .await
        .list()
        .unwrap()
        .into_iter()
        .find(|view| view.target.channel == "telegram")
        .unwrap()
        .id;

    let gate = Arc::clone(&approvals);
    let worker_channel = Arc::clone(&channel);

    let worker = tokio::spawn(async move {
        crate::wait_approval(
            &worker_channel,
            "telegram",
            "default",
            "7",
            None,
            "write a note",
            &challenge.approve,
            &challenge.deny,
            &ChannelConfig::default(),
            ApprovalMode::Prompt,
            &gate,
            &Arc::new(Mutex::new(state::Store::default())),
            &mut 0,
            &mut 1,
            tokio::time::Instant::now() + Duration::from_secs(10),
            &Cancellation::new(),
            &Stop::new(),
        )
        .await
    });

    let outcome = tokio::time::timeout(Duration::from_secs(5), async {
        until(|| root.join("telegram.polling").exists()).await;

        assert_eq!(approvals.lock().await.resolve_local(&id, false), Some(false));

        worker.await.unwrap().unwrap();

        assert!(!challenge.answer.await.unwrap());
        assert_eq!(approvals.lock().await.list().unwrap().len(), 1);
    })
    .await;

    channel.stop().await.unwrap();
    outcome.unwrap();
    approvals.lock().await.cancel(&unrelated.approve);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn approved_channel_resumes_while_another_approval_is_pending() {
    let root = test_root("independent-approvals");

    fs::create_dir_all(&root).unwrap();

    let plugins = registry([
        plugin(&root, "telegram", "channel", "approval").await,
        plugin(&root, "discord", "channel", "approval").await,
        plugin(&root, "alpha", "model", "approval").await,
        plugin(&root, "beta", "model", "approval").await,
        plugin(&root, "tools", "tool", "approval").await,
    ])
    .await;

    let mut context = context(&root, plugins.clone(), state::Store::default());

    context.approval_mode = ApprovalMode::Prompt;
    context.policies = [("telegram", "alpha"), ("discord", "beta")]
        .into_iter()
        .map(|(channel, provider)| {
            (
                channel.into(),
                ChannelConfig {
                    provider: Some(provider.into()),
                    tools: true,
                    owner: Some("8".into()),
                    ..Default::default()
                },
            )
        })
        .collect();

    let approvals = Arc::clone(&context.approvals);
    let stop = Arc::clone(&context.stop);
    let worker = tokio::spawn(channels::run(context));

    let outcome = tokio::time::timeout(Duration::from_secs(8), async {
        until(|| root.join("telegram.pending").exists() && root.join("discord.pending").exists())
            .await;
        fs::write(root.join("telegram.allow"), "").unwrap();
        until(|| {
            root.join("alpha.txt").exists() && !requests(&root, "telegram", "send").is_empty()
        })
        .await;

        assert!(!root.join("beta.txt").exists());
        let pending = approvals.lock().await.list().unwrap();

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].target.channel, "discord");

        fs::write(root.join("discord.allow"), "").unwrap();
        until(|| root.join("beta.txt").exists() && !requests(&root, "discord", "send").is_empty())
            .await;
    })
    .await;

    stop.signal();
    tokio::time::timeout(Duration::from_secs(5), worker).await.unwrap().unwrap().unwrap();
    stop_registry(&plugins).await;
    outcome.unwrap();

    assert_eq!(requests(&root, "tools", "write").len(), 2);
    assert!(approvals.lock().await.list().unwrap().is_empty());

    fs::remove_dir_all(root).unwrap();
}
