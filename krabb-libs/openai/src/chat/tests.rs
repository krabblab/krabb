use super::*;

use krabb_core::types::{Content, Message, Role, ToolSpec};

const PROVIDER: &str = "Fixture";

fn message(role: Role, content: Vec<Content>) -> Message {
    Message { id: "1".into(), session: "s".into(), role, sender: None, content }
}

fn request(messages: Vec<Message>) -> ModelRequest {
    ModelRequest {
        generation: None,
        model: "m".into(),
        workspace: None,
        stream: false,
        tools: vec![ToolSpec {
            name: "read".into(),
            description: Some("Read.".into()),
            schema: json!({"type": "object"}),
        }],
        messages,
    }
}

fn emitter() -> (Emitter, tokio::sync::mpsc::Receiver<Value>) {
    let (output, events) = tokio::sync::mpsc::channel(64);

    (Emitter::new(output), events)
}

#[test]
fn bodies_carry_messages_tools_and_the_output_budget() {
    let mut input = request(vec![message(Role::User, vec![Content::Text { text: "hi".into() }])]);

    input.generation = Some(krabb_core::context::Generation {
        limits: None,
        output: 512,
        search: false,
        summary: true,
    });

    let body = body(PROVIDER, &input, "model-x", true).unwrap();

    assert_eq!(body["model"], "model-x");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_completion_tokens"], 512);
    assert_eq!(body["tools"], json!([]));
    assert_eq!(body["messages"][0], json!({"role": "user", "content": "hi"}));
    assert_eq!(tools(&input)[0]["function"]["name"], "read");
}

#[test]
fn tool_calls_and_results_pair_up_and_images_need_safe_references() {
    let call = Content::ToolCall {
        name: "read".into(),
        args: json!({"path": "a"}),
        id: Some("call-1".into()),
        thought_signature: None,
    };

    let mut result = message(Role::Tool, vec![Content::Text { text: "contents".into() }]);

    result.sender = Some("read".into());

    let input = request(vec![message(Role::Assistant, vec![call.clone()]), result]);
    let values = messages(PROVIDER, &input).unwrap();

    assert_eq!(values[0]["tool_calls"][0]["id"], "call-1");
    assert_eq!(values[1], json!({"role": "tool", "tool_call_id": "call-1", "content": "contents"}));

    let unanswered =
        messages(PROVIDER, &request(vec![message(Role::Assistant, vec![call])])).unwrap();

    assert!(unanswered[1]["content"].as_str().unwrap().contains("No tool result was recorded"));
    assert!(image(PROVIDER, "https://cdn.test/a.png").is_ok());
    assert!(image(PROVIDER, "data:image/png;base64,AA").is_ok());
    assert!(image(PROVIDER, "file:///etc/passwd").unwrap_err().to_string().starts_with("Fixture"));
}

#[tokio::test]
async fn streams_assemble_text_and_tools_and_require_their_end() {
    let (mut emitter, mut events) = emitter();

    let text = json!({"choices": [{"delta": {"content": "Hello"}}]});
    let tool = json!({"choices": [{"delta": {"tool_calls": [{
        "index": 0, "id": "c1", "function": {"name": "read", "arguments": "{\"path\":\"a\"}"}
    }]}}]});

    let body = format!("data: {text}\n\ndata: {tool}\n\ndata: [DONE]\n\n");
    let chunks = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(body.into_bytes())]);
    let reply = stream_body(PROVIDER, 3, chunks, &mut emitter).await.unwrap().unwrap();
    let result = reply.result.unwrap();

    assert_eq!(result["text"], "Hello");
    assert_eq!(result["events"][0]["name"], "read");
    assert_eq!(events.recv().await.unwrap()["params"]["event"]["text"], "Hello");

    let open = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(
        format!("data: {text}\n").into_bytes(),
    )]);
    let error = stream_body(PROVIDER, 4, open, &mut emitter).await.unwrap_err();

    assert!(error.to_string().contains("ended before its completion marker"));

    let mut calls = BTreeMap::new();
    let (mut seen, mut pending) = (String::new(), String::new());
    let busy = br#"data: {"error": {"type": "overloaded_error"}}"#;
    let refused = br#"data: {"error": {"type": "invalid_request"}}"#;

    assert!(
        stream_line(PROVIDER, busy, &mut seen, &mut pending, &mut calls)
            .unwrap_err()
            .to_string()
            .contains("overloaded")
    );
    assert!(stream_line(PROVIDER, refused, &mut seen, &mut pending, &mut calls).is_err());
    assert!(stream_line(PROVIDER, &[0xff], &mut seen, &mut pending, &mut calls).is_err());

    let many = json!({"choices": [{"delta": {"tool_calls": [{"index": TOOL_CALLS}]}}]});

    assert!(
        stream_line(
            PROVIDER,
            format!("data: {many}").as_bytes(),
            &mut seen,
            &mut pending,
            &mut calls
        )
        .is_err()
    );

    let failing = futures_util::stream::iter(vec![Err::<Vec<u8>, _>("cut")]);

    assert!(stream_body(PROVIDER, 5, failing, &mut emitter).await.is_err());
}

#[tokio::test]
async fn replies_parse_text_tools_and_reject_empty_or_oversized_bodies() {
    let reply = json!({"choices": [{"message": {"content": "ok", "tool_calls": [{"id": "c",
        "function": {"name": "read", "arguments": "{\"path\":\"a\"}"}}]}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 2, "completion_tokens": 3}});

    let result = response_body(PROVIDER, 1, reply).unwrap().unwrap().result.unwrap();

    assert_eq!((result["text"].clone(), result["input"].clone()), (json!("ok"), json!(2)));
    assert!(response_body(PROVIDER, 1, json!({"choices": []})).is_err());
    assert!(response_body(PROVIDER, 1, json!({"choices": [{"message": {}}]})).is_err());

    let nameless =
        json!({"choices": [{"message": {"tool_calls": [{"function": {"arguments": "{}"}}]}}]});

    assert!(response_body(PROVIDER, 1, nameless).is_err());
    assert!(response(PROVIDER, 1, json!({"x": "y".repeat(krabb_core::jsonl::MAX)})).is_err());

    let large =
        futures_util::stream::iter(vec![Ok::<_, std::io::Error>(vec![b'x'; BODY_LIMIT + 1])]);

    assert!(collect(PROVIDER, large).await.is_err());
    assert!(
        collect(PROVIDER, futures_util::stream::iter(vec![Err::<Vec<u8>, _>("cut")]))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_api_posts_with_or_without_a_key_and_reports_refusals() {
    let client = reqwest::Client::new();
    let reply = json!({"choices": [{"message": {"content": "done"}}]}).to_string();
    let (base, server) = krabb_test::http::responses(vec![(200, reply), (401, "{}".into())]).await;

    let keyed = Api { client: &client, key: Some("k"), base: &base, provider: PROVIDER };
    let body = json!({"model": "m", "messages": []});

    assert_eq!(
        keyed.generate(1, &body, None).await.unwrap().unwrap().result.unwrap()["text"],
        "done"
    );

    let open = Api { key: None, ..keyed };

    assert!(open.generate(2, &body, None).await.is_err());

    let requests = server.await.unwrap();

    assert!(requests[0].headers.contains("authorization: Bearer k"));
    assert!(!requests[1].headers.to_ascii_lowercase().contains("authorization"));
}
