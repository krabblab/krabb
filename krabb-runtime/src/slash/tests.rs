use super::*;

struct Fixture {
    root: PathBuf,
    plugins: Plugins,
    sessions: Arc<Mutex<state::Store>>,
    cancels: Arc<Mutex<BTreeMap<String, Arc<Cancellation>>>>,
    approvals: Arc<AsyncMutex<approval::Gate>>,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = crate::tests::test_root(label);
        let store = state::Store::load(root.join("data/sessions/index.json")).unwrap();

        Self {
            root,
            plugins: Plugins::default(),
            sessions: Arc::new(Mutex::new(store)),
            cancels: Arc::default(),
            approvals: Arc::new(AsyncMutex::new(approval::Gate::new().unwrap())),
        }
    }

    fn host(&self) -> Host<'_> {
        Host {
            plugins: &self.plugins,
            sessions: &self.sessions,
            cancels: &self.cancels,
            approvals: &self.approvals,
            provider: "openai",
            model: "default-model",
            call: 1,
        }
    }

    fn conversation(&self, id: &str, messages: usize) {
        let mut store = self.sessions.lock().unwrap();

        store.ensure_channel(id, "default-model").unwrap();

        for index in 0..messages {
            store
                .push(
                    id,
                    Message {
                        id: format!("{id}-{index}"),
                        session: id.into(),
                        role: Role::User,
                        sender: Some("7".into()),
                        content: vec![Content::Text { text: "hello".into() }],
                    },
                )
                .unwrap();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn caller<'a>(session: &'a str, origin: Origin) -> Caller<'a> {
    Caller {
        integration: "telegram",
        chat: "7",
        thread: None,
        session,
        sender: Some("7"),
        tools: false,
        origin,
    }
}

async fn send(fixture: &Fixture, caller: &Caller<'_>, text: &str) -> String {
    run(&fixture.host(), caller, krabb_core::slash::parse(text).unwrap()).await.unwrap()
}

#[tokio::test]
async fn help_lists_only_commands_this_caller_can_use() {
    let fixture = Fixture::new("slash-help");

    let text = send(&fixture, &caller("telegram-7", Origin::Text), "/help").await;

    for shown in ["/status", "/session", "/task", "/plugin", "plain words"] {
        assert!(text.contains(shown), "{shown}: {text}");
    }

    for hidden in ["/model", "/memory", "/data", "/schedule"] {
        assert!(!text.contains(hidden), "{hidden}: {text}");
    }

    let usage = send(&fixture, &caller("telegram-7", Origin::Text), "/help session").await;

    assert!(usage.starts_with("Usage: /session [new | cancel | clear]"));

    let unknown = send(&fixture, &caller("telegram-7", Origin::Text), "/krabb nope").await;

    assert!(unknown.starts_with("Unknown command nope."));

    let refused = send(&fixture, &caller("telegram-7", Origin::Text), "/memory tea").await;

    assert!(refused.contains("not available here"));
}

#[tokio::test]
async fn status_and_model_stay_scoped_to_the_conversation() {
    let fixture = Fixture::new("slash-model");
    let caller = caller("telegram-7", Origin::Text);

    assert!(send(&fixture, &caller, "/status").await.contains("new conversation"));

    let shown = model(&fixture.host(), &caller, "").await.unwrap();

    assert!(shown.contains("openai / default-model"));

    let changed = model_change(&fixture, &caller, "gpt-test").await;

    assert!(changed.contains("now uses openai / gpt-test"));
    assert_eq!(fixture.sessions.lock().unwrap().sessions["telegram-7"].model, "gpt-test");
    assert!(send(&fixture, &caller, "/status").await.contains("openai / gpt-test"));
    assert!(model_change(&fixture, &caller, "two words").await.starts_with("Send /model"));
    assert!(model(&fixture.host(), &caller, "list").await.unwrap().contains("not running"));
}

async fn model_change(fixture: &Fixture, caller: &Caller<'_>, name: &str) -> String {
    model(&fixture.host(), caller, name).await.unwrap()
}

#[tokio::test]
async fn new_conversations_archive_history_and_wait_for_running_replies() {
    let fixture = Fixture::new("slash-new");
    let text = caller("telegram-7", Origin::Text);

    assert!(send(&fixture, &text, "/session new").await.contains("already a new"));

    fixture.conversation("telegram-7", 2);
    fixture.sessions.lock().unwrap().set_status("telegram-7", "working").unwrap();

    assert_eq!(send(&fixture, &text, "/session new").await, WORKING);

    fixture.sessions.lock().unwrap().set_status("telegram-7", "idle").unwrap();

    assert!(send(&fixture, &text, "/session").await.contains("2 message(s)"));
    assert!(send(&fixture, &text, "/session new").await.starts_with("Started a new"));

    let store = fixture.sessions.lock().unwrap();

    assert!(!store.sessions.contains_key("telegram-7"));
    assert!(store.sessions.values().any(|session| session.archived));
}

#[tokio::test]
async fn the_model_can_only_defer_a_new_conversation() {
    let fixture = Fixture::new("slash-tool");
    let tool = caller("telegram-7", Origin::Tool);

    fixture.conversation("telegram-7", 1);

    let args = serde_json::json!({"name": "session", "arguments": "new"});

    assert!(call(&fixture.host(), &tool, &args).await.unwrap().contains("after this reply"));

    {
        let mut store = fixture.sessions.lock().unwrap();

        store.set_status("telegram-7", "working").unwrap();
        settle(&mut store, "telegram-7").unwrap();

        assert!(store.sessions.contains_key("telegram-7"));

        store.set_status("telegram-7", "idle").unwrap();
        settle(&mut store, "telegram-7").unwrap();

        assert!(!store.sessions.contains_key("telegram-7"));
    }

    let cancel = serde_json::json!({"name": "session", "arguments": "cancel"});

    assert!(call(&fixture.host(), &tool, &cancel).await.unwrap().contains("/session cancel"));

    let client = Caller { integration: krabb_core::client::TUI, ..caller("tui-1", Origin::Tool) };

    fixture.conversation("tui-1", 1);

    assert!(call(&fixture.host(), &client, &args).await.unwrap().contains("/new"));

    let schedules = serde_json::json!({"name": "schedule"});

    let reply = call(&fixture.host(), &client, &schedules).await.unwrap();

    assert!(reply.starts_with("/schedule is not available"), "{reply}");

    for invalid in [
        serde_json::json!({"name": "launch"}),
        serde_json::json!({"name": "model", "arguments": "a\nb"}),
    ] {
        assert!(call(&fixture.host(), &tool, &invalid).await.is_err());
    }

    let names = spec().schema["properties"]["name"]["enum"].clone();

    assert!(names.as_array().unwrap().iter().any(|name| name == "session"));
}

#[tokio::test]
async fn clearing_history_needs_the_same_sender_to_repeat_a_fresh_code() {
    let fixture = Fixture::new("slash-clear");
    let text = caller("telegram-9", Origin::Text);

    assert!(send(&fixture, &text, "/session clear").await.contains("no history"));

    fixture.conversation("telegram-9", 2);

    let prompt = send(&fixture, &text, "/session clear").await;
    let code = prompt.split("/session clear ").nth(1).unwrap()[..4].to_owned();

    assert!(send(&fixture, &text, "/session clear 99999").await.contains("cannot be undone"));

    let fresh = send(&fixture, &text, "/session clear").await;
    let code = fresh.split("/session clear ").nth(1).map_or(code, |rest| rest[..4].to_owned());

    let other = Caller { sender: Some("8"), ..caller("telegram-9", Origin::Text) };

    assert!(send(&fixture, &other, &format!("/session clear {code}")).await.contains("undone"));

    let tool = caller("telegram-9", Origin::Tool);
    let args = serde_json::json!({"name": "session", "arguments": format!("clear {code}")});

    assert!(call(&fixture.host(), &tool, &args).await.unwrap().contains("undone"));
    assert_eq!(fixture.sessions.lock().unwrap().sessions["telegram-9"].conversation().count(), 2);

    let fresh = send(&fixture, &text, "/session clear").await;
    let code = fresh.split("/session clear ").nth(1).unwrap()[..4].to_owned();

    assert_eq!(
        send(&fixture, &text, &format!("/session clear  {code} ")).await,
        "Deleted this conversation's history."
    );

    assert_eq!(fixture.sessions.lock().unwrap().sessions["telegram-9"].conversation().count(), 0);
}

#[tokio::test]
async fn read_only_commands_describe_this_conversation() {
    let fixture = Fixture::new("slash-read");
    let text = caller("telegram-5", Origin::Text);

    assert!(send(&fixture, &text, "/session cancel").await.starts_with("Nothing is running"));
    assert!(send(&fixture, &text, "/workspace").await.contains("shared workspace"));
    assert!(send(&fixture, &text, "/approvals").await.starts_with("Nothing is waiting"));
    assert_eq!(send(&fixture, &text, "/plugin").await, "Active plugins:\n");
    assert!(send(&fixture, &text, "/task").await.contains("no unfinished tasks"));
    assert!(send(&fixture, &text, "/session bogus").await.starts_with("Usage: /session"));

    let tool = caller("telegram-5", Origin::Tool);

    for action in ["approve job-1", "deny job-1"] {
        let args = serde_json::json!({"name": "task", "arguments": action});

        assert!(call(&fixture.host(), &tool, &args).await.unwrap().starts_with("Only the user"));
    }

    fixture.conversation("telegram-5", 1);
    fixture.sessions.lock().unwrap().set_status("telegram-5", "working").unwrap();

    assert_eq!(send(&fixture, &text, "/session cancel").await, "Stopping the current reply.");
    assert!(cancellation(&fixture.cancels, "telegram-5").requested());
}

#[tokio::test]
async fn channel_commands_reply_once_without_reaching_the_model() {
    let fixture = Fixture::new("slash-channel");
    let policy = ChannelConfig { allow: vec!["7".into()], ..Default::default() };

    let event = serde_json::json!({"id": 41, "chat": 7, "sender": 7, "private": true,
        "text": "/status"});

    fixture.conversation("telegram-7", 1);

    let host = fixture.host();

    assert!(channel(&host, "telegram", &event, &policy, ApprovalMode::Prompt).await.unwrap());
    assert!(channel(&host, "telegram", &event, &policy, ApprovalMode::Prompt).await.unwrap());

    {
        let store = fixture.sessions.lock().unwrap();

        assert_eq!(store.outbox.len(), 1);
        assert!(store.outbox[0].text.contains("Model: openai / default-model"));
        assert_eq!((store.outbox[0].chat.as_str(), store.outbox[0].private), ("7", true));
    }

    for text in ["hello", "/task", "/usr/bin"] {
        let event = serde_json::json!({"id": 42, "chat": 7, "text": text});

        assert!(!channel(&host, "telegram", &event, &policy, ApprovalMode::Prompt).await.unwrap());
    }

    let anonymous = serde_json::json!({"chat": 7, "text": "/help"});

    assert!(channel(&host, "telegram", &anonymous, &policy, ApprovalMode::Prompt).await.is_err());
}
