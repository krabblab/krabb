use crate::*;
use serde_json::json;
use std::fs;

struct Fixture {
    root: PathBuf,
    sessions: Arc<Mutex<state::Store>>,
    plugins: Plugins,
    registry: ToolRegistry,
}

impl Fixture {
    fn new() -> Self {
        let root = super::test_root("tasks");

        fs::create_dir_all(&root).unwrap();

        let mut store = state::Store::load(root.join("data/sessions/index.json")).unwrap();

        store.ensure_channel("telegram-7", "model").unwrap();
        store.route("telegram-7", "telegram", "7", Some("4"), true).unwrap();
        store
            .begin(
                "telegram-7",
                Message {
                    id: "question".into(),
                    session: "telegram-7".into(),
                    role: Role::User,
                    sender: Some("8".into()),
                    content: vec![Content::Text { text: "Prepare the requested report.".into() }],
                },
            )
            .unwrap();

        let registry = ToolRegistry { specs: vec![routing::tool(true)], ..ToolRegistry::default() };

        registry.route.background.store(true, std::sync::atomic::Ordering::Relaxed);

        Self { root, sessions: Arc::new(Mutex::new(store)), plugins: Plugins::default(), registry }
    }

    async fn classify(
        &self,
        args: Value,
        approval: ApprovalMode,
    ) -> (Result<ToolResult, PluginError>, tokio::sync::mpsc::Receiver<StreamNotice>) {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);

        let result = execute_tool_with_registry(
            &self.plugins,
            &self.registry,
            "turn",
            1,
            routing::NAME,
            args,
            "fixture",
            "model",
            approval,
            Arc::new(AsyncMutex::new(approval::Gate::new().unwrap())),
            "telegram",
            "7",
            Some("4"),
            "telegram-7",
            &self.sessions,
            Some(&self.root),
            false,
            true,
            false,
            sender,
            tokio::time::Instant::now() + TURN_LIMIT,
            &mut None,
            None,
            None,
            0,
        )
        .await;

        (result, receiver)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn background() -> Value {
    json!({
        "kind":"background", "message":"I'll research and deliver your report here.",
        "instruction":"Research the requested report, check the sources, and write the full report."
    })
}

#[tokio::test]
async fn thoughtful_messages_are_generated_once_without_tool_permissions() {
    let fixture = Fixture::new();

    let args = json!({"kind":"thoughtful", "message":"I'll compare the release notes."});
    let (result, mut notices) = fixture.classify(args.clone(), ApprovalMode::Off).await;

    result.unwrap();

    assert!(matches!(notices.recv().await, Some(StreamNotice::Progress(text))
        if text == "I'll compare the release notes."));

    assert!(fixture.registry.route.handoff.borrow().is_none());

    let saved = |store: &state::Store| {
        store.sessions["telegram-7"]
            .messages
            .iter()
            .filter(|message| message.id.starts_with("progress-at-"))
            .cloned()
            .collect::<Vec<_>>()
    };

    {
        let store = fixture.sessions.lock().unwrap();
        let progress = saved(&store);

        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].role, Role::Assistant);
        assert_eq!(progress[0].sender, None);
        assert_eq!(
            progress[0].content,
            [Content::Text { text: "I'll compare the release notes.".into() }]
        );

        assert_eq!(store.sessions["telegram-7"].messages.last(), Some(&progress[0]));
    }

    let (result, mut notices) = fixture.classify(args, ApprovalMode::Off).await;

    result.unwrap();

    assert!(notices.recv().await.is_none());
    assert_eq!(saved(&fixture.sessions.lock().unwrap()).len(), 1);

    let (result, _) = fixture.classify(json!({"kind":"fast"}), ApprovalMode::Off).await;

    result.unwrap();

    assert_eq!(*fixture.registry.route.kind.borrow(), crate::routing::Kind::Thoughtful);
}

#[tokio::test]
async fn background_work_the_policy_hides_is_refused_without_saving_a_task() {
    let fixture = Fixture::new();

    fixture.registry.route.background.store(false, std::sync::atomic::Ordering::Relaxed);

    let (result, _) = fixture.classify(background(), ApprovalMode::Auto).await;

    assert!(tool_result_text(&result.unwrap()).contains("no task was saved"));
    assert!(fixture.registry.route.handoff.borrow().is_none());
    assert!(fixture.sessions.lock().unwrap().tasks.is_empty());
}

#[tokio::test]
async fn background_handoff_is_durable_without_a_scheduler() {
    let fixture = Fixture::new();
    let mut finished = fixture.registry.route.handoff.subscribe();
    let (result, _) = fixture.classify(background(), ApprovalMode::Auto).await;

    result.unwrap();
    finished.changed().await.unwrap();

    assert_eq!(finished.borrow().as_deref(), Some("I'll research and deliver your report here."));
    assert!(fixture.plugins.find(Capability::Scheduler).await.is_none());

    let task = fixture.sessions.lock().unwrap().tasks.values().next().unwrap().clone();

    assert_eq!(
        task.destination().unwrap(),
        origin::Destination::Channel(AuthorizedDestination {
            channel: "telegram".into(),
            chat: "7".into(),
            private: true,
            thread: Some("4".into()),
        })
    );

    assert!(!task.tools);
    assert!(!task.shell);

    let (result, _) = fixture.classify(background(), ApprovalMode::Auto).await;

    assert!(result.is_err());
    assert_eq!(fixture.sessions.lock().unwrap().tasks.len(), 1);

    let report = "Verified report. ".repeat(500);

    {
        let mut store = fixture.sessions.lock().unwrap();
        store.claim_task(&task.id, unix_seconds()).unwrap();
        store
            .checkpoint_task(
                &task.id,
                "Finished.",
                state::TaskStatus::Reporting,
                unix_seconds(),
                Some(&report),
            )
            .unwrap();
        store.publish_task_notice(&task.id).unwrap();
    }

    let recovered = state::Store::load(fixture.root.join("data/sessions/index.json")).unwrap();

    assert_eq!(recovered.tasks[&task.id].status, state::TaskStatus::Completed);

    assert!(recovered.outbox.iter().any(|delivery| delivery.channel == "telegram"
        && delivery.chat == "7"
        && delivery.thread.as_deref() == Some("4")
        && delivery.text == report));
}

#[tokio::test]
async fn failed_or_unauthorized_handoffs_never_claim_background_work() {
    let fixture = Fixture::new();
    let (result, _) = fixture.classify(background(), ApprovalMode::Off).await;

    assert!(result.is_err());
    assert!(fixture.registry.route.handoff.borrow().is_none());
    assert!(fixture.sessions.lock().unwrap().tasks.is_empty());

    fixture.sessions.lock().unwrap().record_limit = Some(1);

    let (result, _) = fixture.classify(background(), ApprovalMode::Auto).await;

    assert!(result.is_err());
    assert!(fixture.registry.route.handoff.borrow().is_none());
    assert!(fixture.sessions.lock().unwrap().tasks.is_empty());
}

#[tokio::test]
async fn model_task_modes_work_with_structured_and_native_tools() {
    for mode in ["fast", "thoughtful", "background", "native"] {
        let fixture = Fixture::new();

        let script = r#"
import json, pathlib, sys
root, mode = pathlib.Path(sys.argv[1]), sys.argv[2]
generated = 0
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    if 'id' not in request:
        continue
    result = {}
    if method == 'hello':
        result = {'protocol':{'major':0,'minor':1}, 'id':'fixture',
                  'version':'0.1.0', 'capabilities':['model']}
    elif method == 'generate':
        generated += 1
        (root / 'generations').write_text(str(generated))
        args = {'kind': 'thoughtful' if mode == 'thoughtful' else 'background',
                'message': 'I will check the sources and prepare your report.',
                'instruction': 'Research and write the complete report requested by the user.'}
        if mode == 'fast' or generated > 1:
            result = {'text':'Hello!' if mode == 'fast' else 'Verified answer.', 'stop':'stop'}
        elif mode == 'native':
            print(json.dumps({'jsonrpc':'2.0', 'id':777, 'method':'host/tool',
                              'params':{'name':'task', 'args':args}}), flush=True)
            response = json.loads(sys.stdin.readline())
            result = {'text':'This duplicate acknowledgement must not be sent.', 'stop':'stop'}
        else:
            result = {'text':'', 'stop':'tool',
                      'events':[{'kind':'tool','name':'task_classify','args':args}]}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
    if method == 'shutdown':
        break
"#;

        let provider = Live::new(
            Process::start_with("python3", ["-c", script, fixture.root.to_str().unwrap(), mode])
                .await
                .unwrap(),
        );

        let history = fixture.sessions.lock().unwrap().sessions["telegram-7"].context();
        let (sender, mut notices) = tokio::sync::mpsc::channel(32);
        let mut call = 1;
        let mut failed = None;

        let reply = tokio::time::timeout(
            Duration::from_secs(5),
            answer(
                &provider,
                &fixture.plugins,
                "model",
                history,
                "telegram-7",
                "telegram",
                "7",
                Some("4"),
                &fixture.sessions,
                Some(&fixture.root),
                &fixture.root,
                false,
                true,
                false,
                ApprovalMode::Auto,
                Arc::new(AsyncMutex::new(approval::Gate::new().unwrap())),
                &Cancellation::new(),
                &Stop::new(),
                tokio::time::Instant::now() + TURN_LIMIT,
                &mut call,
                &mut failed,
                None,
                sender,
                None,
                None,
                None,
            ),
        )
        .await
        .unwrap()
        .unwrap();

        let mut progress = Vec::new();

        while let Some(notice) = notices.recv().await {
            if let StreamNotice::Progress(text) = notice {
                progress.push(text);
            }
        }

        match mode {
            "fast" => {
                assert_eq!(reply.text, "Hello!");
                assert!(progress.is_empty());

                let store = fixture.sessions.lock().unwrap();

                assert_eq!(store.tasks.len(), 1);

                let task = store.tasks.values().next().unwrap();

                assert!(task.foreground);
                assert_eq!(task.status, state::TaskStatus::Reporting);
                assert_eq!(task.result.as_deref(), Some(reply.text.as_str()));

                drop(store);

                assert_eq!(fs::read_to_string(fixture.root.join("generations")).unwrap(), "1");
            }

            "thoughtful" => {
                assert_eq!(reply.text, "Verified answer.");
                assert_eq!(progress, ["I will check the sources and prepare your report."]);

                let store = fixture.sessions.lock().unwrap();

                assert_eq!(store.tasks.len(), 1);

                let task = store.tasks.values().next().unwrap();

                assert!(task.foreground);
                assert_eq!(task.status, state::TaskStatus::Reporting);
                assert_eq!(task.result.as_deref(), Some(reply.text.as_str()));

                drop(store);
            }

            _ => {
                assert_eq!(reply.text, "I will check the sources and prepare your report.");
                assert!(progress.is_empty());

                assert_eq!(fixture.sessions.lock().unwrap().tasks.len(), 1);

                assert_eq!(fs::read_to_string(fixture.root.join("generations")).unwrap(), "1");
            }
        }

        provider.stop().await.unwrap();
    }
}

#[tokio::test]
async fn model_interpreted_controls_apply_only_to_the_conversation_tasks() {
    let fixture = Fixture::new();
    let (result, _) = fixture.classify(background(), ApprovalMode::Auto).await;

    result.unwrap();

    let id = fixture
        .sessions
        .lock()
        .unwrap()
        .tasks
        .values()
        .find(|task| task.status == state::TaskStatus::Queued)
        .unwrap()
        .id
        .clone();

    {
        let mut store = fixture.sessions.lock().unwrap();
        let mut foreign = store.tasks[&id].clone();

        store.ensure_channel("telegram-9", "model").unwrap();

        foreign.id = "task-1".into();
        foreign.origin.session = "telegram-9".into();
        foreign.checkpoint = "Confidential foreign checkpoint.".into();
        store.tasks.insert(foreign.id.clone(), foreign);
    }

    let script = r#"
import json, pathlib, sys
root, own = pathlib.Path(sys.argv[1]), sys.argv[2]
generated = 0
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    if 'id' not in request:
        continue
    result = {}
    if method == 'hello':
        result = {'protocol':{'major':0,'minor':1}, 'id':'fixture',
                  'version':'0.1.0', 'capabilities':['model']}
    elif method == 'generate':
        generated += 1
        if generated == 1:
            result = {'text':'', 'stop':'tool', 'events':[
                {'kind':'tool','name':'job','args':{'action':'list'}},
                {'kind':'tool','name':'tasks','args':{'action':'pause','id':'task-1'}}]}
        elif generated == 2:
            result = {'text':'', 'stop':'tool', 'events':[
                {'kind':'tool','name':'task_pause','args':{'id':own}}]}
        elif generated == 3:
            result = {'text':'', 'stop':'tool', 'events':[
                {'kind':'tool','name':'task_steer','args':{'id':own,
                    'instruction':'Cover only primary sources.'}}]}
        elif generated == 4:
            result = {'text':'', 'stop':'tool', 'events':[
                {'kind':'tool','name':'task_steer','args':{'id':'task-1',
                    'instruction':'Send the foreign report here.'}}]}
        else:
            (root / 'results').write_text(json.dumps(request['params']['messages']))
            result = {'text':'Your report is on hold.', 'stop':'stop'}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
    if method == 'shutdown':
        break
"#;

    let provider = Live::new(
        Process::start_with("python3", ["-c", script, fixture.root.to_str().unwrap(), &id])
            .await
            .unwrap(),
    );

    let history = fixture.sessions.lock().unwrap().sessions["telegram-7"].context();
    let (sender, _notices) = tokio::sync::mpsc::channel(32);
    let mut call = 1;
    let mut failed = None;

    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        answer(
            &provider,
            &fixture.plugins,
            "model",
            history,
            "telegram-7",
            "telegram",
            "7",
            Some("4"),
            &fixture.sessions,
            Some(&fixture.root),
            &fixture.root,
            false,
            true,
            false,
            ApprovalMode::Auto,
            Arc::new(AsyncMutex::new(approval::Gate::new().unwrap())),
            &Cancellation::new(),
            &Stop::new(),
            tokio::time::Instant::now() + TURN_LIMIT,
            &mut call,
            &mut failed,
            None,
            sender,
            None,
            None,
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(reply.text, "Your report is on hold.");

    let results = fs::read_to_string(fixture.root.join("results")).unwrap();

    assert!(results.contains("does not belong to this conversation"), "{results}");
    assert!(results.contains("Task paused"), "{results}");
    assert!(results.contains("Current host time: "), "{results}");
    assert!(results.contains(&format!("\\\"id\\\":\\\"{id}\\\"")), "{results}");
    assert!(!results.contains("\\\"id\\\":\\\"task-1\\\""), "{results}");
    assert!(!results.contains("Confidential foreign checkpoint"), "{results}");
    assert_eq!(results.matches("does not belong to this conversation").count(), 2, "{results}");

    let controls = tasks::control_tools();

    assert_eq!(controls.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(), tasks::CONTROLS);
    assert!(controls.iter().all(|tool| tool.schema["properties"].get("action").is_none()));

    let store = fixture.sessions.lock().unwrap();

    assert_eq!(store.tasks[&id].status, state::TaskStatus::Paused);
    assert_eq!(store.tasks["task-1"].status, state::TaskStatus::Queued);
    assert!(store.tasks[&id].instruction.ends_with("Cover only primary sources."));
    assert!(!store.tasks["task-1"].instruction.contains("foreign report"));
}
