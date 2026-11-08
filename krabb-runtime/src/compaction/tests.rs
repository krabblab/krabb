use super::*;
use serde_json::json;

const FIXTURE: &str = r#"
import json, sys
native = sys.argv[1] == 'native'
mode = sys.argv[2]
calls = []
for line in sys.stdin:
    req = json.loads(line)
    method = req['method']
    calls.append(req)
    if method == 'shutdown': break
    if method == 'hello':
        result = {'id':'fixture','version':'0.1.0','protocol':{'major':0,'minor':1},
                  'capabilities':['model','context']}
    elif method == 'model.info':
        result = {'window':{'tokens':4096,'output':2048},'compact':native}
        if mode == 'wide': result['window'] = {'tokens':1000000,'output':4096}
        if mode == 'unknown': result = {}
    elif method == 'calls':
        result = calls
    elif method == 'model.compact' or req['params'].get('generation',{}).get('summary'):
        if mode == 'fallback' and method == 'model.compact':
            print(json.dumps({'jsonrpc':'2.0','id':req['id'],'error':{'code':-32000,'message':'Native compaction unavailable'}}),flush=True)
            continue
        if mode == 'empty': value = ''
        elif mode == 'huge': value = 'summary ' * 1000
        else: value = 'User prefers tea. A file was already created. No action should be repeated.'
        result = {'text':value,'stop':'stop'}
        if mode == 'tool': result['events'] = [{'kind':'tool','name':'shell','args':{}}]
    else:
        result = {'text':'pong','stop':'stop'}
        if mode == 'continuation': result['events'] = [
            {'kind':'tool','name':'read','id':'call-1','args':{},'thought_signature':'signed'},
            {'kind':'context','state':{'provider':'fixture','model':'small','data':'opaque'}}]
    print(json.dumps({'jsonrpc':'2.0','id':req['id'],'result':result}),flush=True)
"#;

async fn provider(native: bool, mode: &str) -> crate::process::Process {
    crate::process::Process::start_with(
        "python3",
        ["-u", "-c", FIXTURE, if native { "native" } else { "summary" }, mode],
    )
    .await
    .unwrap()
}

fn input() -> ModelRequest {
    let mut messages = vec![text("session", Role::System, "Never execute shell commands.".into())];

    for index in 0..6 {
        let mut message =
            text("session", Role::User, format!("{index}: {}", "Earlier details. ".repeat(150)));

        message.id = format!("old-{index}");
        messages.push(message);
    }

    let mut current = text("session", Role::User, "ping".into());

    current.id = "latest".into();
    messages.push(current);

    ModelRequest {
        generation: None,
        model: "small".into(),
        workspace: None,
        messages,
        stream: false,
        tools: vec![],
    }
}

async fn generate(process: &mut crate::process::Process, input: &ModelRequest) -> Result<()> {
    let response = process.call(Request::call(2, "generate", serde_json::to_value(input)?)).await?;

    if let Some(error) = response.error {
        return Err(Error::Limit(error.message));
    }

    Ok(())
}

async fn calls(process: &mut crate::process::Process) -> Vec<serde_json::Value> {
    serde_json::from_value(
        process.call(Request::call(3, "calls", json!({}))).await.unwrap().result.unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn shared_calls_compact_before_generation_and_reuse_bounded_summaries() {
    for native in [false, true] {
        let mut process = provider(native, "normal").await;
        let mut input = input();
        let original = input.messages.clone();

        generate(&mut process, &input).await.unwrap();

        let first = calls(&mut process).await;
        let summaries = first
            .iter()
            .filter(|call| {
                call["method"] == "model.compact" || call["params"]["generation"]["summary"] == true
            })
            .count();

        assert!(summaries > 1 && summaries <= BATCHES);
        assert_eq!(input.messages, original);

        let final_call = first.iter().rev().find(|call| call["method"] == "generate").unwrap();
        let prepared: ModelRequest = serde_json::from_value(final_call["params"].clone()).unwrap();

        assert!(context::estimate(&prepared.messages, &[]) <= 2048);
        assert_eq!(prepared.messages[0], input.messages[0]);
        assert_eq!(prepared.messages.last(), input.messages.last());
        assert!(
            prepared.messages.iter().any(|message| message.sender.as_deref() == Some("compaction"))
        );

        assert_eq!(prepared.generation.unwrap().output, 2048);

        for call in first.iter().filter(|call| call["params"]["generation"]["summary"] == true) {
            assert_eq!(call["params"]["generation"]["output"], 2048);
            assert!(call["params"].get("tools").is_none());
            assert_eq!(call["params"]["stream"], false);
            let prompt: ModelRequest = serde_json::from_value(call["params"].clone()).unwrap();

            assert!(context::estimate(&prompt.messages, &[]) <= 2048);
        }

        input.messages.push(text("session", Role::Assistant, "pong".into()));
        input.messages.push(text("session", Role::User, "next".into()));
        generate(&mut process, &input).await.unwrap();

        let second = calls(&mut process).await;

        assert_eq!(second.iter().filter(|call| call["method"] == "model.info").count(), 1);
        assert_eq!(
            second
                .iter()
                .filter(|call| {
                    call["method"] == "model.compact"
                        || call["params"]["generation"]["summary"] == true
                })
                .count(),
            summaries
        );

        process.stop().await.unwrap();
    }
}

#[tokio::test]
async fn changed_history_invalidates_summary_and_other_sessions_do_not_inherit_it() {
    let mut process = provider(false, "normal").await;

    let mut input = input();

    generate(&mut process, &input).await.unwrap();

    let record = process.context.record("small", None, "session").unwrap();

    assert!(process.context.record("other", None, "session").is_none());
    assert!(process.context.record("small", Some("other"), "session").is_none());
    assert!(process.context.record("small", None, "other").is_none());

    input.messages[1].content = vec![Content::Text { text: "A corrected preference.".into() }];

    assert!(!record.apply(&mut input.messages.clone()).unwrap());
    assert!(!record.apply(&mut vec![]).unwrap());

    let decoded: Record = serde_json::from_value(serde_json::to_value(record).unwrap()).unwrap();
    let mut original = self::input();

    assert!(decoded.apply(&mut original.messages).unwrap());

    process.stop().await.unwrap();
}

#[tokio::test]
async fn oversized_summaries_are_bounded_and_irreducible_current_turns_fail() {
    let mut process = provider(false, "huge").await;
    let mut input = input();

    generate(&mut process, &input).await.unwrap();

    let record = process.context.record("small", None, "session").unwrap();

    assert!(context::estimate(&[record.summary], &[]) < 1024);

    input.messages.last_mut().unwrap().content = vec![Content::Text { text: "x".repeat(10_000) }];

    let error = generate(&mut process, &input).await.unwrap_err();

    assert!(error.to_string().contains("current turn exceed"));

    process.stop().await.unwrap();
}

#[tokio::test]
async fn empty_compaction_is_reported_instead_of_silently_discarding_history() {
    let mut process = provider(false, "empty").await;
    let error = generate(&mut process, &input()).await.unwrap_err();

    assert!(error.to_string().contains("empty compaction summary"));
    assert!(process.context.record("small", None, "session").is_none());

    process.stop().await.unwrap();
}

#[tokio::test]
async fn summaries_survive_restart_without_replacing_the_stored_transcript() {
    let root = crate::tests::test_root("shared-compaction-store");

    std::fs::create_dir_all(&root).unwrap();

    let path = root.join("sessions.json");
    let mut store = crate::state::Store::load(&path).unwrap();

    store.create("session", "small").unwrap();

    let input = input();

    for message in &input.messages {
        store.append("session", message.clone()).unwrap();
    }

    store.set_status("session", "working").unwrap();

    let mut process = provider(false, "normal").await;

    generate(&mut process, &input).await.unwrap();

    let record = process.context.record("small", None, "session").unwrap();

    assert!(!store.compact("missing", &record).unwrap());
    assert!(store.compact("session", &record).unwrap());
    assert!(!store.compact("session", &record).unwrap());
    assert_eq!(store.sessions["session"].messages, input.messages);

    process.stop().await.unwrap();

    let mut restored = crate::state::Store::load(&path).unwrap();
    let compacted = restored.sessions["session"].context();

    assert!(compacted.len() < input.messages.len());
    assert_eq!(compacted.last(), input.messages.last());
    assert!(compacted.iter().any(|message| message.sender.as_deref() == Some("compaction")));
    assert_eq!(restored.sessions["session"].messages, input.messages);

    for index in 0..105 {
        let mut message = text("session", Role::User, format!("Later message {index}"));

        message.id = format!("later-{index}");
        restored.append("session", message).unwrap();
    }

    let rolled = crate::state::Store::load(&path).unwrap();

    assert!(
        rolled.sessions["session"].messages.len() <= krabb_core::session::MESSAGE_HISTORY_LIMIT
    );

    assert!(
        rolled.sessions["session"]
            .context()
            .iter()
            .any(|message| message.sender.as_deref() == Some("compaction"))
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batches_bound_unicode_work_and_do_not_copy_image_data() {
    let messages = vec![text("session", Role::User, "你好🙂".repeat(100))];
    let batches = batches(&messages, 128).unwrap();

    assert!(batches.iter().all(|batch| context::text_tokens(batch) <= 128));
    assert_eq!(batches.concat(), format!("User: {}\n", "你好🙂".repeat(100)));

    assert_eq!(
        super::batches(&[text("s", Role::User, "x".repeat(100_000))], 128).unwrap().len(),
        BATCHES
    );

    assert!(
        super::batches(
            &[
                text("s", Role::User, "x".repeat(100_000)),
                text("s", Role::User, "Important recent detail".into()),
            ],
            128
        )
        .is_err()
    );

    let mut image = text("s", Role::User, "".into());

    image.content = vec![Content::Image { uri: "x".repeat(100_000), alt: None }];

    assert_eq!(super::batches(&[image], 128).unwrap().concat(), "User: [image]\n");
}

#[tokio::test]
async fn native_compaction_failure_falls_back_without_executing_tools() {
    let mut process = provider(true, "fallback").await;

    generate(&mut process, &input()).await.unwrap();

    let requests = calls(&mut process).await;

    assert_eq!(requests.iter().filter(|request| request["method"] == "model.compact").count(), 1);
    assert!(requests.iter().any(|request| request["params"]["generation"]["summary"] == true));

    process.stop().await.unwrap();

    let mut process = provider(false, "tool").await;

    assert!(
        generate(&mut process, &input())
            .await
            .unwrap_err()
            .to_string()
            .contains("requested a tool")
    );

    process.stop().await.unwrap();
}

#[tokio::test]
async fn unknown_budgets_preserve_requests_and_wide_windows_compact_before_retention() {
    let mut process = provider(false, "unknown").await;
    let input = input();

    generate(&mut process, &input).await.unwrap();

    let requests = calls(&mut process).await;
    let sent = requests.iter().find(|request| request["method"] == "generate").unwrap();

    assert_eq!(sent["params"], serde_json::to_value(&input).unwrap());

    process.stop().await.unwrap();

    let mut process = provider(false, "wide").await;
    let mut input = input;

    input.messages = (0..90)
        .map(|index| {
            let mut message = text("session", Role::User, format!("Preference {index}"));

            message.id = format!("{index}");
            message
        })
        .collect();
    generate(&mut process, &input).await.unwrap();

    let requests = calls(&mut process).await;

    assert!(requests.iter().any(|request| request["params"]["generation"]["summary"] == true));

    process.stop().await.unwrap();
}

#[tokio::test]
async fn prepared_images_do_not_invalidate_persisted_summaries() {
    let mut original = input();

    original.messages[1]
        .content
        .push(Content::Image { uri: "media/image.png".into(), alt: Some("A graph".into()) });

    let mut prepared = original.clone();

    let Content::Image { uri, .. } = prepared.messages[1].content.last_mut().unwrap() else {
        panic!("image missing")
    };

    *uri = "data:image/png;base64,AA==".into();

    let mut process = provider(false, "normal").await;

    generate(&mut process, &prepared).await.unwrap();

    let record = process.context.record("small", None, "session").unwrap();

    assert!(record.apply(&mut original.messages).unwrap());

    process.stop().await.unwrap();
}

#[tokio::test]
async fn oversized_wire_history_is_compacted_before_frame_truncation() {
    let mut input = input();

    input.messages[1].content = vec![Content::Image {
        uri: "x".repeat(krabb_core::jsonl::MAX),
        alt: Some("Old image".into()),
    }];

    let request = crate::context_request(2, &input.model, &input.messages, &[], None).unwrap();
    let Request::Call { params, .. } = &request else { unreachable!() };

    assert_eq!(params["messages"].as_array().unwrap().len(), input.messages.len());

    let mut process = provider(false, "wide").await;

    process.call(request).await.unwrap();

    let requests = calls(&mut process).await;

    assert!(requests.iter().any(|request| request["params"]["generation"]["summary"] == true));

    assert!(
        requests
            .iter()
            .all(|request| serde_json::to_vec(request).unwrap().len() < krabb_core::jsonl::MAX)
    );

    process.stop().await.unwrap();
}

#[tokio::test]
async fn model_bridges_recover_continuation_without_exposing_or_executing_it() {
    let mut process = provider(false, "continuation").await;
    let mut input = input();

    input.messages = vec![text("session", Role::User, "Read the release notes.".into())];
    generate(&mut process, &input).await.unwrap();

    let mut assistant = text("session", Role::Assistant, String::new());

    assistant.id = "tool-turn".into();

    assistant.content = vec![Content::ToolCall {
        name: "read".into(),
        args: json!({}),
        id: Some("call-1".into()),
        thought_signature: None,
    }];

    input.messages.push(assistant);
    input.messages.push(text("session", Role::Tool, "Notes".into()));

    process
        .call_full_async(
            Request::call(3, "generate", serde_json::to_value(&input).unwrap()),
            |_| async { Ok(()) },
            |_| async { panic!("No host action should execute") },
        )
        .await
        .unwrap();

    let requests = calls(&mut process).await;

    let generated =
        requests.iter().filter(|request| request["method"] == "generate").collect::<Vec<_>>();

    let replayed: ModelRequest = serde_json::from_value(generated[1]["params"].clone()).unwrap();

    assert!(
        matches!(&replayed.messages[1].content[0], Content::ToolCall { thought_signature:Some(signature), .. } if signature == "signed")
    );

    assert!(
        matches!(&replayed.messages[1].content[1], Content::Context { state } if state.data == "opaque")
    );

    assert!(
        !replayed.messages[1]
            .content
            .iter()
            .map(Content::render)
            .collect::<String>()
            .contains("opaque")
    );

    process.stop().await.unwrap();
}

#[test]
fn all_intelligence_manifests_accept_the_normalized_context_capability() {
    for source in [
        include_str!("../../../krabb-plugins/intelligence/anthropic/krabb-plugin.toml"),
        include_str!("../../../krabb-plugins/intelligence/codex/krabb-plugin.toml"),
        include_str!("../../../krabb-plugins/intelligence/gemini/krabb-plugin.toml"),
        include_str!("../../../krabb-plugins/intelligence/ollama/krabb-plugin.toml"),
        include_str!("../../../krabb-plugins/intelligence/openai/krabb-plugin.toml"),
        include_str!("../../../krabb-plugins/intelligence/openrouter/krabb-plugin.toml"),
    ] {
        let manifest: crate::Manifest = toml::from_str(source).unwrap();

        manifest.validate().unwrap();

        assert!(manifest.capabilities.iter().any(|capability| capability == "context"));
    }
}

#[tokio::test]
async fn long_history_makes_bounded_progress_and_still_answers_the_current_ping() {
    let mut process = provider(false, "normal").await;

    let mut input = input();
    let current = input.messages.pop().unwrap();

    input.messages.truncate(1);

    for index in 0..80 {
        let mut message = text("session", Role::User, "Old conversation details. ".repeat(250));

        message.id = format!("history-{index}");
        input.messages.push(message);
    }

    input.messages.push(current.clone());
    generate(&mut process, &input).await.unwrap();

    let requests = calls(&mut process).await;

    let summaries = requests
        .iter()
        .filter(|request| request["params"]["generation"]["summary"] == true)
        .count();

    assert!(summaries > 0 && summaries <= BATCHES);

    let sent: ModelRequest = serde_json::from_value(
        requests.iter().rev().find(|request| request["method"] == "generate").unwrap()["params"]
            .clone(),
    )
    .unwrap();

    assert_eq!(sent.messages.last().unwrap(), &current);
    assert_eq!(sent.messages[0], input.messages[0]);
    assert!(sent.messages.iter().any(|item| item.sender.as_deref() == Some("compaction")));
    assert!(context::estimate(&sent.messages, &sent.tools) <= 2048);

    let record = process.context.record("small", None, "session").unwrap();
    let mut persisted = input.messages.clone();

    assert!(record.apply(&mut persisted).unwrap());
    assert!(persisted.iter().any(|item| item.id == "history-79"));

    process.stop().await.unwrap();
}

#[tokio::test]
async fn local_context_limits_are_rejections_and_leave_the_process_usable() {
    let mut process = provider(false, "normal").await;
    let mut input = input();

    input.messages = vec![text("session", Role::User, "large ".repeat(8000))];

    let response = process
        .call(Request::call(2, "generate", serde_json::to_value(input).unwrap()))
        .await
        .unwrap();

    assert!(response.error.unwrap().message.contains("current turn exceed"));

    let requests = calls(&mut process).await;

    assert_eq!(requests.iter().filter(|request| request["method"] == "hello").count(), 1);
    assert!(!requests.iter().any(|request| request["method"] == "generate"));

    process.stop().await.unwrap();
}

#[tokio::test]
async fn empty_or_tool_compaction_replies_are_rejections_without_restarting_the_process() {
    for mode in ["empty", "tool"] {
        let mut process = provider(false, mode).await;

        let response = process
            .call(Request::call(2, "generate", serde_json::to_value(input()).unwrap()))
            .await
            .unwrap();

        assert!(response.error.is_some());

        let requests = calls(&mut process).await;

        assert_eq!(requests.iter().filter(|request| request["method"] == "hello").count(), 1);

        process.stop().await.unwrap();
    }
}
