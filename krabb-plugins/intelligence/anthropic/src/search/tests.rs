use super::*;

use krabb_core::{
    context::Generation,
    types::{Message, Role},
};

fn input() -> ModelRequest {
    ModelRequest {
        model: "claude-fixture".into(),
        workspace: None,
        stream: true,
        generation: Some(Generation { output: 4096, summary: false, search: true }),
        tools: vec![],
        messages: vec![Message {
            id: "1".into(),
            session: "fixture".into(),
            sender: None,
            role: Role::User,
            content: vec![Content::Text { text: "Find the latest release.".into() }],
        }],
    }
}

fn answer() -> Value {
    json!({"content": [{"type":"text", "text":"Released.", "citations":[{
        "url":"https://example.com/release", "title":"Release", "encrypted_index":"private",
    }]}], "stop_reason":"end_turn", "usage":{"input_tokens":40,"output_tokens":10}})
}

fn paused() -> Value {
    json!({"content": [
        {"type":"server_tool_use", "id":"search", "name":"web_search", "input":{"query":"release"}},
        {"type":"web_search_tool_result", "tool_use_id":"search", "content":[{"encrypted_content":"opaque"}]},
    ], "stop_reason":"pause_turn", "usage":{"output_tokens":20}})
}

#[tokio::test]
async fn native_search_continues_pauses_and_normalizes_citations() {
    let pause = paused();

    let (base, server) =
        krabb_test::http::responses(vec![(200, pause.to_string()), (200, answer().to_string())])
            .await;

    let (output, _events) = tokio::sync::mpsc::channel(8);
    let mut emitter = Emitter::new(output);

    let result = generate_at(&reqwest::Client::new(), 7, input(), "fixture", &base, &mut emitter)
        .await
        .unwrap()
        .unwrap();

    let reply: ModelReply = serde_json::from_value(result.result.unwrap()).unwrap();

    assert_eq!(reply.text, "Released. [Release](https://example.com/release)");
    assert!(reply.events.is_empty());
    assert_eq!(reply.input, Some(40));
    assert_eq!(reply.output, Some(30));
    assert!(!reply.text.contains("opaque"));

    let requests = server.await.unwrap();

    assert!(requests[0].headers.starts_with("POST /v1/messages "));
    assert_eq!(requests[0].body["tools"][0]["type"], "web_search_20250305");
    assert_eq!(requests[0].body["tools"][0]["max_uses"], 3);
    assert_eq!(requests[0].body["stream"], false);
    assert_eq!(requests[1].body["max_tokens"], 4076);
    assert_eq!(requests[1].body["messages"][1]["content"], pause["content"]);
}

#[tokio::test]
async fn search_errors_and_continuations_are_bounded() {
    let mut exhausted = paused();

    exhausted["usage"]["output_tokens"] = 4096.into();

    let mut unresolved = paused();

    unresolved["content"][0]["type"] = "tool_use".into();

    for (replies, expected) in [
        (vec![(403, "{}".into())], "403"),
        (vec![(200, "{}".into())], "no content"),
        (vec![(200, "invalid".into())], "JSON"),
        (vec![(200, exhausted.to_string())], "output budget"),
        (vec![(200, unresolved.to_string())], "unresolved client"),
        (vec![(200, paused().to_string()); 4], "continuation limit"),
    ] {
        let (base, server) = krabb_test::http::responses(replies).await;
        let error = generate(&reqwest::Client::new(), 1, &input(), "fixture", &base)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.to_lowercase().contains(&expected.to_lowercase()), "{error}");
        server.await.unwrap();
    }
}

#[test]
fn summaries_disable_search_and_server_deltas_never_become_host_tools() {
    let mut input = input();

    input.generation.as_mut().unwrap().summary = true;

    let body = request_body(&input, false).unwrap();

    assert_eq!(body["tools"], json!([]));
    assert_eq!(body["thinking"]["type"], "disabled");
    assert!(!enabled(&input));

    let mut text = String::new();
    let mut pending = String::new();
    let mut tools = BTreeMap::new();
    let mut stop = String::new();

    for value in [
        json!({"type":"content_block_start", "index":30,
            "content_block":{"type":"server_tool_use", "name":"web_search"}}),
        json!({"type":"content_block_delta", "index":30,
            "delta":{"partial_json":"{\"query\":\"release\"}"}}),
    ] {
        anthropic_line(
            format!("data: {value}").as_bytes(),
            &mut text,
            &mut pending,
            &mut tools,
            &mut stop,
        )
        .unwrap();
    }

    assert!(tools.is_empty());

    assert!(text.is_empty());

    assert_eq!(
        super::text(&[json!({"text":"text", "citations":[{"url":"javascript:alert(1)"}]})]),
        "text"
    );
}
