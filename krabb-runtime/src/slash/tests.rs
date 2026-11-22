use super::*;

struct Fixture {
    root: PathBuf,
    plugins: Plugins,
    sessions: Arc<Mutex<state::Store>>,
    cancels: Arc<Mutex<BTreeMap<String, Arc<Cancellation>>>>,
    approvals: Arc<AsyncMutex<approval::Gate>>,
    stop: Stop,
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
            stop: Stop::new(),
        }
    }

    fn host(&self) -> Host<'_> {
        Host {
            plugins: &self.plugins,
            sessions: &self.sessions,
            cancels: &self.cancels,
            approvals: &self.approvals,
            stop: &self.stop,
            provider: "openai",
            model: "default-model",
            call: 1,
            answering: false,
            home: Some(&self.root),
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
        owner: false,
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

    assert!(send(&fixture, &text, "/session").await.contains("2 messages"));
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

    assert!(matches!(
        channel(&host, "telegram", &event, &policy, ApprovalMode::Prompt).await.unwrap(),
        Command::Handled
    ));

    assert!(matches!(
        channel(&host, "telegram", &event, &policy, ApprovalMode::Prompt).await.unwrap(),
        Command::Handled
    ));

    {
        let store = fixture.sessions.lock().unwrap();

        assert_eq!(store.outbox.len(), 1);
        assert!(store.outbox[0].text.contains("Model: openai / default-model"));
        assert_eq!((store.outbox[0].chat.as_str(), store.outbox[0].private), ("7", true));
    }

    for text in ["hello", "/task", "/usr/bin"] {
        let event = serde_json::json!({"id": 42, "chat": 7, "text": text});

        assert!(matches!(
            channel(&host, "telegram", &event, &policy, ApprovalMode::Prompt).await.unwrap(),
            Command::Pass
        ));
    }

    let anonymous = serde_json::json!({"chat": 7, "text": "/help"});

    assert!(channel(&host, "telegram", &anonymous, &policy, ApprovalMode::Prompt).await.is_err());
}

const MODEL: &str = r#"
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request.get('method')
    result = {}
    if method == 'hello':
        result = {'protocol':{'major':0,'minor':1},'id':'openai','version':'0.1.0',
                  'capabilities':['model']}
    elif method == 'generate':
        if sys.argv[1] == 'hang':
            continue
        if sys.argv[1] == 'fail':
            print(json.dumps({'jsonrpc':'2.0','id':request['id'],
                              'error':{'code':-32000,'message':'Rate limited.'}}), flush=True)
            continue
        if sys.argv[1] == 'garbage':
            result = {'text':'I would rather not.','stop':'stop'}
        elif sys.argv[1] == 'hatch':
            wishes = request['params']['messages'][-1]['content'][0]['text']
            name = 'Pinch' if 'Pinch' in wishes else 'Krabb'
            text = ('=== KRAB.md ===\n# Personality Instructions\n\nYou are ' + name +
                    '.\n=== KLAW.md ===\n# Behavioral Instructions\n\nAsk first.\n'
                    '=== SUMMARY ===\nYou are now ' + name + '.')
            result = {'text':text,'stop':'stop'}
        else:
            result = {'text':'Keep the user goals.','stop':'stop'}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
    if method == 'shutdown':
        break
"#;

async fn with_model(fixture: &Fixture) {
    with_mode(fixture, "answer").await;
}

/// Starts the model plugin, retrying a handshake that a loaded CI runner delays.
async fn with_mode(fixture: &Fixture, mode: &str) {
    let mut attempts = 0;

    let process = loop {
        attempts += 1;

        match krabb_core::plugin::Process::start_with("python3", ["-u", "-c", MODEL, mode]).await {
            Ok(process) => break process,

            Err(krabb_core::Error::Handshake(reason)) if reason == "timeout" && attempts < 3 => {}
            Err(error) => panic!("the model plugin did not start: {error}"),
        }
    };

    fixture.plugins.insert(Live::new(process)).await;
}

fn exchange(fixture: &Fixture, session: &str, index: usize, reply: Vec<Message>) {
    let mut store = fixture.sessions.lock().unwrap();

    store.ensure_channel(session, "default-model").unwrap();

    store
        .push(
            session,
            Message {
                id: format!("user-{index}"),
                session: session.into(),
                role: Role::User,
                sender: Some("7".into()),
                content: vec![Content::Text { text: format!("question {index}") }],
            },
        )
        .unwrap();

    for message in reply {
        store.push(session, message).unwrap();
    }
}

fn answer(session: &str, id: &str, role: Role, content: Content) -> Message {
    Message { id: id.into(), session: session.into(), role, sender: None, content: vec![content] }
}

fn text_answer(session: &str, index: usize) -> Vec<Message> {
    let text = Content::Text { text: format!("answer {index}") };

    vec![answer(session, &format!("assistant-{index}"), Role::Assistant, text)]
}

#[tokio::test]
async fn status_reports_context_usage() {
    let fixture = Fixture::new("slash-usage");
    let caller = caller("telegram-7", Origin::Text);

    fixture.conversation("telegram-7", 1);
    fixture
        .sessions
        .lock()
        .unwrap()
        .set_context_usage(
            "telegram-7",
            krabb_core::types::ContextUsage { used: 42_500, limit: 128_000 },
        )
        .unwrap();

    let status = send(&fixture, &caller, "/status").await;

    assert!(status.contains("Context: 42k of 128k tokens (33%)"), "{status}");
    assert_eq!(tokens(999), "999");
}

#[tokio::test]
async fn compact_summarizes_older_turns_and_keeps_history() {
    let fixture = Fixture::new("slash-compact");
    let session = "telegram-7";
    let text = caller(session, Origin::Text);

    assert_eq!(compact(&fixture.host(), &text).await.unwrap(), SHORT);

    for index in 0..4 {
        exchange(&fixture, session, index, text_answer(session, index));
    }

    assert!(compact(&fixture.host(), &text).await.unwrap().contains("not running"));

    with_model(&fixture).await;

    let tool = caller(session, Origin::Tool);

    assert!(compact(&fixture.host(), &tool).await.unwrap().starts_with("Tell the user"));

    let reply = compact(&fixture.host(), &text).await.unwrap();

    assert!(reply.starts_with("Summarized 4 earlier messages"), "{reply}");

    {
        let store = fixture.sessions.lock().unwrap();
        let session = &store.sessions[session];
        let context = session.context();

        assert_eq!(session.messages.len(), 8);
        assert_eq!(context.len(), 5);
        assert!(context[0].content[0].render().contains("Keep the user goals."));
        assert_eq!(context[1].id, "user-2");
    }

    assert!(compact(&fixture.host(), &text).await.unwrap().contains("already compacted"));

    for status in ["working"] {
        fixture.sessions.lock().unwrap().sessions.get_mut(session).unwrap().status = status.into();

        assert_eq!(compact(&fixture.host(), &text).await.unwrap(), WORKING);
    }

    fixture.sessions.lock().unwrap().sessions.get_mut(session).unwrap().status = "cancelled".into();

    assert!(
        compact(&fixture.host(), &text).await.unwrap().contains("already compacted"),
        "a cancelled reply leaves the conversation idle"
    );
}

#[tokio::test]
async fn compact_answers_failures_and_yields_to_shutdown() {
    let fixture = Fixture::new("slash-compact-failure");
    let session = "telegram-7";
    let text = caller(session, Origin::Text);

    for index in 0..4 {
        exchange(&fixture, session, index, text_answer(session, index));
    }

    with_mode(&fixture, "fail").await;

    let busy = Host { answering: true, ..fixture.host() };
    let compacting = krabb_core::slash::parse("/compact").unwrap();

    assert!(run(&busy, &text, compacting).await.unwrap().contains("busy with another message"));

    assert!(compact(&fixture.host(), &text).await.unwrap().starts_with("Summarizing failed"));

    with_mode(&fixture, "hang").await;

    let host = fixture.host();
    let pending = compact(&host, &text);

    tokio::pin!(pending);

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut pending).await.is_err()
    );

    fixture.stop.signal();

    let reply =
        tokio::time::timeout(std::time::Duration::from_secs(5), pending).await.unwrap().unwrap();

    assert!(reply.starts_with("Summarizing failed"), "{reply}");

    let store = fixture.sessions.lock().unwrap();

    assert!(store.sessions[session].compacted_context.is_none());
}

#[tokio::test]
async fn again_reasks_text_questions_once_and_never_repeats_tools() {
    let fixture = Fixture::new("slash-again");
    let session = "telegram-7";
    let policy = ChannelConfig { allow: vec!["7".into()], ..Default::default() };

    let event =
        |id: u64, text: &str| serde_json::json!({"id": id, "chat": 7, "sender": 7, "text": text});

    let host = fixture.host();

    let mode = ApprovalMode::Prompt;

    assert!(matches!(
        channel(&host, "telegram", &event(1, "hello"), &policy, mode).await.unwrap(),
        Command::Pass
    ));

    assert!(matches!(
        channel(&host, "telegram", &event(2, "/again"), &policy, mode).await.unwrap(),
        Command::Handled
    ));

    let pending = {
        let mut store = fixture.sessions.lock().unwrap();

        assert!(store.outbox.iter().any(|delivery| delivery.text.contains("not available")));
        store.outbox.remove(0)
    };

    with_model(&fixture).await;
    exchange(&fixture, session, 0, text_answer(session, 0));
    exchange(&fixture, session, 1, text_answer(session, 1));
    fixture.sessions.lock().unwrap().outbox.push(pending);

    assert!(matches!(
        channel(&host, "telegram", &event(9, "/again"), &policy, mode).await.unwrap(),
        Command::Handled
    ));

    {
        let mut store = fixture.sessions.lock().unwrap();

        assert_eq!(store.sessions[session].messages.len(), 4, "an undelivered reply blocks it");
        assert_eq!(store.outbox.last().unwrap().text, AGAIN_WORKING);
        store.outbox.clear();
    }

    for _ in 0..2 {
        let outcome = channel(&host, "telegram", &event(3, "/again"), &policy, mode).await.unwrap();

        assert!(matches!(outcome, Command::Ask(question) if question == "question 1"));
        assert_eq!(fixture.sessions.lock().unwrap().sessions[session].messages.len(), 2);
    }

    assert!(matches!(
        channel(&host, "telegram", &event(3, "/again"), &policy, mode).await.unwrap(),
        Command::Ask(_)
    ));

    assert!(fixture.sessions.lock().unwrap().outbox.iter().all(|item| item.text != "question 1"));

    fixture.sessions.lock().unwrap().sessions.get_mut(session).unwrap().status = "working".into();

    assert!(matches!(
        channel(&host, "telegram", &event(5, "/again"), &policy, mode).await.unwrap(),
        Command::Handled
    ));

    assert_eq!(fixture.sessions.lock().unwrap().sessions[session].messages.len(), 2);

    {
        let mut store = fixture.sessions.lock().unwrap();

        store.outbox.clear();
        store.sessions.get_mut(session).unwrap().status = "idle".into();
    }

    let call = answer(
        session,
        "assistant-2",
        Role::Assistant,
        Content::ToolCall {
            name: "write".into(),
            args: serde_json::json!({}),
            id: None,
            thought_signature: None,
        },
    );

    let result = answer(session, "tool-2", Role::Tool, Content::Text { text: "Written.".into() });

    exchange(&fixture, session, 2, vec![call, result]);

    assert!(matches!(
        channel(&host, "telegram", &event(4, "/again"), &policy, mode).await.unwrap(),
        Command::Handled
    ));

    let store = fixture.sessions.lock().unwrap();

    assert_eq!(store.sessions[session].messages.len(), 5);
    assert!(store.outbox.iter().any(|delivery| delivery.text.contains("used tools")));
}

#[tokio::test]
async fn again_belongs_to_the_asker_and_keeps_its_refusals() {
    let fixture = Fixture::new("slash-again-owner");
    let session = "telegram-7";
    let policy = ChannelConfig { allow: vec!["7".into(), "8".into()], ..Default::default() };

    let event = |id: u64, sender: u64| {
        serde_json::json!({"id": id, "chat": 7, "sender": sender, "text": "/again",
            "content": [{"kind": "text", "text": "/again"}]})
    };

    let host = fixture.host();
    let mode = ApprovalMode::Prompt;

    with_model(&fixture).await;
    exchange(&fixture, session, 0, text_answer(session, 0));

    assert!(matches!(
        channel(&host, "telegram", &event(1, 8), &policy, mode).await.unwrap(),
        Command::Handled
    ));

    fixture.sessions.lock().unwrap().sessions.get_mut(session).unwrap().status = "working".into();

    assert!(matches!(
        channel(&host, "telegram", &event(2, 7), &policy, mode).await.unwrap(),
        Command::Handled
    ));

    fixture.sessions.lock().unwrap().sessions.get_mut(session).unwrap().status = "idle".into();

    assert!(matches!(
        channel(&host, "telegram", &event(2, 7), &policy, mode).await.unwrap(),
        Command::Handled
    ));

    {
        let store = fixture.sessions.lock().unwrap();

        assert_eq!(store.sessions[session].messages.len(), 2);
        assert!(store.outbox.iter().any(|item| item.text.contains("Only the person who asked")));
    }

    fixture.sessions.lock().unwrap().outbox.clear();

    let mut asked = event(3, 7);

    let Command::Ask(question) = channel(&host, "telegram", &asked, &policy, mode).await.unwrap()
    else {
        panic!("the asker can answer again");
    };

    reask(&mut asked, question);

    assert_eq!(crate::inbound::content(&asked), vec![Content::Text { text: "question 0".into() }]);

    let local = Caller { integration: "tui", ..caller(session, Origin::Text) };

    assert!(send(&fixture, &local, "/again").await.contains("not available"));
    assert!(!send(&fixture, &local, "/help").await.contains("/again"));
}

#[test]
fn again_needs_a_text_question_with_a_reply() {
    let session = "telegram-7";
    let question = |content| answer(session, "user", Role::User, content);
    let reply = answer(session, "reply", Role::Assistant, Content::Text { text: "4".into() });

    assert_eq!(last_exchange(&[]).unwrap_err(), NO_REPLY);

    assert_eq!(
        last_exchange(&[question(Content::Text { text: "2+2?".into() })]).unwrap_err(),
        NO_REPLY
    );

    let image = Content::Image { uri: "data:image/png;base64,AA==".into(), alt: None };

    assert!(last_exchange(&[question(image), reply.clone()]).unwrap_err().contains("text"));

    let notice = Message {
        sender: Some("job".into()),
        ..answer(session, "notice", Role::Assistant, Content::Text { text: "Done.".into() })
    };

    assert!(
        last_exchange(&[question(Content::Text { text: "2+2?".into() }), reply.clone(), notice])
            .unwrap_err()
            .contains("posted after")
    );

    assert_eq!(
        last_exchange(&[question(Content::Text { text: "2+2?".into() }), reply]).unwrap(),
        (0, "2+2?".into())
    );

    let failed = Message {
        sender: Some(krabb_core::types::SYSTEM.into()),
        ..answer(session, "failed", Role::Assistant, Content::Text { text: "Timed out.".into() })
    };

    let parts = Message {
        content: vec![
            Content::Text { text: "Summarize this".into() },
            Content::Text { text: "Attached file: notes.txt".into() },
        ],
        ..question(Content::Text { text: String::new() })
    };

    assert_eq!(
        last_exchange(&[parts, failed]).unwrap(),
        (0, "Summarize this\nAttached file: notes.txt".into()),
        "a failed reply can be answered again"
    );

    let first = question(Content::Text { text: "Here's my config.".into() });
    let second = question(Content::Text { text: "Why does it fail?".into() });
    let reply = answer(session, "reply", Role::Assistant, Content::Text { text: "4".into() });

    assert_eq!(
        last_exchange(&[first.clone(), second.clone(), reply.clone()]).unwrap(),
        (0, "Here's my config.\nWhy does it fail?".into()),
        "messages answered together are asked again together"
    );

    let other = Message { sender: Some("8".into()), ..first };

    assert!(last_exchange(&[other, second, reply]).unwrap_err().contains("several people"));
}

#[tokio::test]
async fn again_during_another_chats_reply_queues_the_question() {
    let fixture = Fixture::new("slash-again-intake");
    let session = "telegram-7";
    let policy = ChannelConfig { allow: vec!["7".into()], ..Default::default() };
    let host = Host { answering: true, ..fixture.host() };
    let event = serde_json::json!({"id": 5, "chat": 7, "sender": 7, "text": "/again"});

    with_model(&fixture).await;
    exchange(&fixture, session, 0, text_answer(session, 0));

    let channel = fixture.plugins.get("openai").await.unwrap();
    let (mut offset, mut call) = (0, 10);

    let processed = crate::queue_during_turn(
        &channel,
        "telegram",
        "default-model",
        &event,
        &policy,
        &fixture.sessions,
        &mut offset,
        &mut call,
        None,
        Some((&host, ApprovalMode::Prompt)),
    )
    .await
    .unwrap();

    assert!(processed);

    let store = fixture.sessions.lock().unwrap();
    let conversation = &store.sessions[session];

    assert!(conversation.messages.is_empty());
    assert_eq!(conversation.queued.len(), 1);
    assert_eq!(conversation.queued[0].content, vec![Content::Text { text: "question 0".into() }]);
}

fn hatch_caller(session: &str, origin: Origin) -> Caller<'_> {
    Caller { owner: true, ..caller(session, origin) }
}

#[tokio::test]
async fn hatch_needs_the_owner_a_provider_and_the_owners_own_words() {
    let fixture = Fixture::new("slash-hatch-guard");
    let session = "telegram-7";
    let directory = fixture.root.join("data/system");
    let owner = hatch_caller(session, Origin::Text);
    let member = Caller { tools: true, ..caller(session, Origin::Text) };

    std::fs::create_dir_all(&directory).unwrap();

    assert!(send(&fixture, &member, "/hatch be brief").await.contains("not available here"));
    assert!(!send(&fixture, &member, "/help").await.contains("/hatch"));
    assert!(send(&fixture, &owner, "/help").await.contains("/hatch"));
    assert!(send(&fixture, &owner, "/hatch").await.starts_with("Tell me how you'd like me to be"));

    let missing = send(&fixture, &owner, "/hatch be brief").await;

    assert!(missing.contains("needs an intelligence provider"), "{missing}");
    assert!(!missing.contains(&fixture.root.display().to_string()));

    let local = Caller { integration: "tui", ..caller("tui-main", Origin::Text) };
    let missing = send(&fixture, &local, "/hatch be brief").await;

    assert!(missing.contains(&directory.display().to_string()), "{missing}");

    with_model(&fixture).await;

    let tool = send(&fixture, &hatch_caller(session, Origin::Tool), "/hatch be brief").await;

    assert!(tool.contains("needs the user's own message"));

    let long = format!("/hatch {}", "x".repeat(crate::hatch::WISH_LIMIT + 1));

    assert!(send(&fixture, &owner, &long).await.contains("at most 2000 characters"));
    assert!(send(&fixture, &owner, "/hatch be brief").await.contains("Nothing changed"));
    assert!(!directory.join("KRAB.md").exists());
}

#[tokio::test]
async fn hatch_rewrites_both_files_and_undo_switches_back() {
    let fixture = Fixture::new("slash-hatch");
    let session = "telegram-7";
    let directory = fixture.root.join("data/system");
    let text = hatch_caller(session, Origin::Text);

    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("KRAB.md"), "# Personality Instructions\n\nYou are Krabb.\n")
        .unwrap();

    with_mode(&fixture, "hatch").await;

    let reply = send(&fixture, &text, "/hatch call yourself Pinch").await;
    let hatched = crate::hatch::read(&directory).unwrap();

    assert_eq!(reply, "You are now Pinch.\nSend /hatch undo to go back.");
    assert_eq!(hatched.krab, "# Personality Instructions\n\nYou are Pinch.\n");
    assert_eq!(hatched.klaw, "# Behavioral Instructions\n\nAsk first.\n");

    let unchanged = send(&fixture, &text, "/hatch call yourself Pinch").await;

    assert_eq!(unchanged, "You are now Pinch. Nothing needed to change.");

    assert!(send(&fixture, &text, "/hatch undo").await.starts_with("I'm back"));

    assert_eq!(
        crate::hatch::read(&directory).unwrap().krab,
        "# Personality Instructions\n\nYou are Krabb.\n"
    );

    assert_eq!(crate::hatch::read(&directory).unwrap().klaw, "");

    assert!(send(&fixture, &text, "/hatch UNDO").await.starts_with("I'm back"));
    assert_eq!(crate::hatch::read(&directory).unwrap(), hatched);
}

#[tokio::test]
async fn hatch_keeps_the_files_when_the_draft_is_unusable() {
    let fixture = Fixture::new("slash-hatch-garbage");
    let directory = fixture.root.join("data/system");
    let text = hatch_caller("telegram-7", Origin::Text);

    std::fs::create_dir_all(&directory).unwrap();
    with_mode(&fixture, "garbage").await;

    let reply = send(&fixture, &text, "/hatch be brief").await;

    assert!(reply.starts_with("The draft is missing === KRAB.md ==="), "{reply}");
    assert!(reply.ends_with("Nothing changed; send /hatch again to retry."));
    assert!(send(&fixture, &text, "/hatch undo").await.contains("no previous personality"));
    assert_eq!(crate::hatch::read(&directory).unwrap(), crate::hatch::Files::default());
}
