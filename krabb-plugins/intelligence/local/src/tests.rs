use super::{NO_TOOLS, generate, handle, profile, refused, server};

use krabb_core::{
    Error,
    failure::{Failure, Kind},
    plugin::Emitter,
    types::{Content, Message, ModelRequest, Role, ToolSpec},
};

use serde_json::json;

fn request(model: &str, stream: bool, tools: bool) -> ModelRequest {
    ModelRequest {
        generation: None,
        model: model.into(),
        workspace: None,
        stream,
        tools: match tools {
            true => vec![ToolSpec {
                name: "read".into(),
                description: Some("Read a file.".into()),
                schema: json!({"type": "object"}),
            }],

            false => vec![],
        },
        messages: vec![Message {
            id: "1".into(),
            session: "fixture".into(),
            role: Role::User,
            sender: None,
            content: vec![Content::Text { text: "Hello.".into() }],
        }],
    }
}

fn emitter() -> (Emitter, tokio::sync::mpsc::Receiver<serde_json::Value>) {
    let (output, events) = tokio::sync::mpsc::channel(64);

    (Emitter::new(output), events)
}

fn models() -> String {
    json!({"object": "list", "data": [
        {"id": "qwen3-8b", "max_model_len": 32768},
        {"id": "llama-3.2-3b"},
    ]})
    .to_string()
}

fn reply(text: &str) -> String {
    json!({"choices": [{"message": {"content": text}, "finish_reason": "stop"}]}).to_string()
}

/// A loopback port nothing listens on.
async fn closed() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();

    let address = listener.local_addr().unwrap();

    drop(listener);

    format!("http://{address}/v1")
}

#[test]
fn plain_http_is_allowed_only_close_to_this_machine() {
    for url in [
        "https://inference.example.com/v1",
        "http://127.0.0.1:8000/v1",
        "http://localhost:1234/v1",
        "http://[::1]:8080/v1",
        "http://192.168.1.20:8000/v1",
        "http://10.0.0.5/v1",
        "http://172.16.4.2/v1",
        "http://100.101.102.103:8000/v1",
        "http://[fd12::1]/v1",
        "http://gpu-box:8000/v1",
        "http://gpu-box.local:8000/v1",
        "http://studio.lan/v1",
        "http://localhost./v1",
        "http://gpu-box.local.:8000/v1",
    ] {
        assert!(server::validate(url).is_ok(), "{url}");
    }

    for url in [
        "http://8.8.8.8/v1",
        "http://inference.example.com/v1",
        "http://172.32.0.1/v1",
        "http://user:secret@127.0.0.1/v1",
        "http://127.0.0.1/v1?key=x",
        "ftp://127.0.0.1/v1",
        "not a url",
    ] {
        assert!(server::validate(url).is_err(), "{url}");
    }

    assert_eq!(server::validate("http://127.0.0.1:8000/v1/").unwrap(), "http://127.0.0.1:8000/v1");
}

#[test]
fn models_resolve_contexts_and_list_for_setup() {
    let entries: Vec<serde_json::Value> = serde_json::from_str::<serde_json::Value>(&models())
        .unwrap()["data"]
        .as_array()
        .unwrap()
        .clone();

    assert_eq!(server::resolve("default", &entries).unwrap(), "qwen3-8b");
    assert_eq!(server::resolve("", &entries).unwrap(), "qwen3-8b");
    assert_eq!(server::resolve("llama-3.2-3b", &entries).unwrap(), "llama-3.2-3b");
    assert!(server::resolve("default", &[]).unwrap_err().to_string().contains("no model"));

    assert_eq!(server::context(entries.first()), 32768);
    assert_eq!(server::context(entries.get(1)), 4096);
    assert_eq!(server::context(None), 4096);
    assert_eq!(server::context(Some(&json!({"context_length": 8192}))), 8192);
    assert_eq!(server::context(Some(&json!({"max_context_length": 16384}))), 16384);
    assert_eq!(server::context(Some(&json!({"meta": {"n_ctx_train": 131072}}))), 4096);
    assert_eq!(server::context(Some(&json!({"max_model_len": 1, "context_length": 8192}))), 8192);
    assert_eq!(server::context(Some(&json!({"max_model_len": 1}))), 4096);

    let listing = server::listing("http://127.0.0.1:8000/v1", &entries);

    assert_eq!(
        listing,
        "Local models at http://127.0.0.1:8000/v1\n  qwen3-8b - qwen3-8b\n  llama-3.2-3b - llama-3.2-3b"
    );
}

#[test]
fn context_length_is_reported_or_configured() {
    assert_eq!(profile("auto", 32768).unwrap().window.unwrap().tokens, 32768);
    assert_eq!(profile("", 8192).unwrap().window.unwrap().output, 4096);
    assert_eq!(profile("4096", 32768).unwrap().window.unwrap().tokens, 4096);
    assert!(profile("lots", 4096).is_err());
    assert!(profile("1", 4096).is_err());
}

#[test]
fn only_a_refusal_that_names_tools_becomes_the_tool_message() {
    let invalid = || Failure::new(Kind::Invalid).error("rejected: tools are not supported here");
    let schema = Failure::new(Kind::Invalid).error("Invalid schema for function in tools");

    assert!(refused(invalid(), true).to_string().starts_with(NO_TOOLS));
    assert!(refused(invalid(), true).to_string().ends_with("tools are not supported here"));
    assert!(!refused(invalid(), false).to_string().starts_with(NO_TOOLS));
    assert!(!refused(schema, true).to_string().starts_with(NO_TOOLS));

    let busy = Failure::new(Kind::Transient).error("The local server tool_choice failed");

    assert!(!refused(busy, true).to_string().starts_with(NO_TOOLS));
    assert!(!refused(Error::Denied("tool_choice".into()), true).to_string().starts_with(NO_TOOLS));
}

#[tokio::test]
async fn discovery_takes_the_first_server_that_answers() {
    let client = reqwest::Client::new();
    let (live, server) = krabb_test::http::responses(vec![(200, models())]).await;
    let dead = closed().await;
    let candidates = [("vLLM", dead.as_str()), ("LM Studio", live.as_str())];

    let found = server::Server::new("auto", &candidates);
    let (base, key) = found.base(&client, Some("secret")).await.unwrap();

    assert_eq!((base.as_str(), key), (live.as_str(), None));

    let requests = server.await.unwrap();

    assert!(requests[0].headers.starts_with("GET /models "));
    assert!(!requests[0].headers.to_ascii_lowercase().contains("authorization"));
    assert_eq!(found.base(&client, None).await.unwrap().0, live);

    let only = [("vLLM", dead.as_str())];
    let error = server::Server::new("", &only).base(&client, None).await.unwrap_err().to_string();

    assert!(error.contains("No local inference server answered") && error.contains("vLLM"));

    let configured = server::Server::new("http://127.0.0.1:9/v1/", &only);

    assert_eq!(
        configured.base(&client, Some("k")).await.unwrap(),
        ("http://127.0.0.1:9/v1".to_owned(), Some("k"))
    );
}

#[tokio::test]
async fn a_default_model_uses_the_first_listed_one_without_a_key() {
    let client = reqwest::Client::new();

    let (base, server) =
        krabb_test::http::responses(vec![(200, models()), (200, reply("Hi there."))]).await;

    let (mut emitter, _events) = emitter();

    let response =
        generate(&client, 4, request("default", false, false), &base, None, &mut emitter)
            .await
            .unwrap()
            .unwrap();

    assert_eq!(response.result.unwrap()["text"], "Hi there.");

    let requests = server.await.unwrap();

    assert!(requests[1].headers.starts_with("POST /chat/completions "));
    assert!(!requests[1].headers.to_ascii_lowercase().contains("authorization"));
    assert_eq!(requests[1].body["model"], "qwen3-8b");
    assert_eq!(requests[1].body["stream"], false);
}

#[tokio::test]
async fn a_key_and_a_chosen_model_go_straight_to_the_server() {
    let client = reqwest::Client::new();
    let (base, server) = krabb_test::http::responses(vec![(200, reply("Done."))]).await;
    let (mut emitter, _events) = emitter();

    generate(&client, 5, request("llama-3.2-3b", false, true), &base, Some("secret"), &mut emitter)
        .await
        .unwrap();

    let requests = server.await.unwrap();

    assert!(requests[0].headers.contains("authorization: Bearer secret"));
    assert_eq!(requests[0].body["model"], "llama-3.2-3b");
    assert_eq!(requests[0].body["tools"][0]["function"]["name"], "read");
}

#[tokio::test]
async fn streamed_replies_reach_the_person_as_they_arrive() {
    let client = reqwest::Client::new();
    let chunk = json!({"choices": [{"delta": {"content": "Streamed."}}]});
    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let (base, server) = krabb_test::http::responses(vec![(200, body)]).await;
    let (mut emitter, mut events) = emitter();

    let response =
        generate(&client, 6, request("qwen3-8b", true, false), &base, None, &mut emitter)
            .await
            .unwrap()
            .unwrap();

    assert_eq!(response.result.unwrap()["text"], "Streamed.");
    assert_eq!(server.await.unwrap()[0].body["stream"], true);
    assert_eq!(events.recv().await.unwrap()["params"]["event"]["text"], "Streamed.");
}

#[tokio::test]
async fn a_server_without_tool_calling_says_so() {
    let client = reqwest::Client::new();

    let refusal =
        json!({"error": {"message": "\"auto\" tool choice requires --enable-auto-tool-choice"}});

    let (base, _server) = krabb_test::http::responses(vec![(400, refusal.to_string())]).await;
    let (mut emitter, _events) = emitter();

    let error = generate(&client, 7, request("qwen3-8b", false, true), &base, None, &mut emitter)
        .await
        .unwrap_err();

    assert!(error.to_string().starts_with(NO_TOOLS));
}

#[tokio::test]
async fn missing_models_and_unreachable_servers_fail_plainly() {
    let client = reqwest::Client::new();

    let (base, _server) =
        krabb_test::http::responses(vec![(200, json!({"data": []}).to_string())]).await;

    let (mut emitter, _events) = emitter();

    let error = generate(&client, 8, request("default", false, false), &base, None, &mut emitter)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("no model loaded"));

    let dead = closed().await;
    let error = server::models(&client, &dead, None).await.unwrap_err();

    assert!(matches!(error, Error::Failed { .. }));
}

#[tokio::test]
async fn host_requests_check_list_describe_and_generate() {
    use krabb_core::types::Request;

    let client = reqwest::Client::new();
    let replies = vec![(200, models()), (200, models()), (200, models()), (200, reply("Hi."))];
    let (base, _server) = krabb_test::http::responses(replies).await;

    let configured = server::Server::new(&base, &[]);
    let call = |id, method: &str, params| Request::call(id, method, params);

    let run = |request| {
        let (emitter, _events) = emitter();

        handle(&client, request, emitter, &configured)
    };

    let checked = run(call(1, "check", json!({}))).await.unwrap().unwrap();

    assert_eq!(checked.result.unwrap(), json!({}));

    let listed = run(call(2, "command", json!({"args": ["models"]}))).await.unwrap().unwrap();

    assert!(listed.result.unwrap()["text"].as_str().unwrap().contains("  qwen3-8b - qwen3-8b"));

    let info = run(call(3, "model.info", json!({"model": "default"}))).await.unwrap().unwrap();

    assert_eq!(info.result.unwrap()["window"]["tokens"], 32768);

    let input = serde_json::to_value(request("qwen3-8b", false, false)).unwrap();
    let answer = run(call(4, "generate", input)).await.unwrap().unwrap();

    assert_eq!(answer.result.unwrap()["text"], "Hi.");

    let unknown = run(call(5, "command", json!({"args": ["dance"]}))).await.unwrap_err();

    assert!(unknown.to_string().contains("krabb local models"));
    assert!(run(call(6, "other", json!({}))).await.unwrap().is_none());

    let note = Request::Note { jsonrpc: "2.0".into(), method: "event".into(), params: json!({}) };

    assert!(run(note).await.unwrap().is_none());
}

#[tokio::test]
async fn a_found_server_that_stops_is_forgotten_and_the_next_one_found() {
    use krabb_core::types::Request;

    let client = reqwest::Client::new();
    let (first, _gone) = krabb_test::http::responses(vec![(200, models())]).await;

    let (second, _live) =
        krabb_test::http::responses(vec![(200, models()), (200, reply("From the second."))]).await;

    let candidates = [("first", first.as_str()), ("second", second.as_str())];
    let found = server::Server::new("auto", &candidates);
    let input = serde_json::to_value(request("qwen3-8b", false, false)).unwrap();

    let (emitter, _events) = emitter();

    let failed =
        handle(&client, Request::call(1, "generate", input.clone()), emitter, &found).await;

    assert!(failed.is_err());

    let (emitter, _events) = emitter();
    let answer = handle(&client, Request::call(2, "generate", input), emitter, &found).await;

    assert_eq!(answer.unwrap().unwrap().result.unwrap()["text"], "From the second.");
}
