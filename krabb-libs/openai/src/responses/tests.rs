use super::*;

use krabb_core::{
    context::Generation,
    types::{Content, Message, Role, ToolSpec},
};

fn input() -> ModelRequest {
    ModelRequest {
        model: "gpt-6-luna".into(),
        workspace: None,
        stream: false,
        generation: Some(Generation { output: 2048, search: true, summary: false }),
        tools: vec![ToolSpec {
            name: "read".into(),
            description: Some("Read a file".into()),
            schema: json!({"type":"object"}),
        }],
        messages: vec![message(
            Role::User,
            vec![Content::Text { text: "Search the release notes.".into() }],
        )],
    }
}

fn message(role: Role, content: Vec<Content>) -> Message {
    Message { id: "fixture".into(), session: "fixture".into(), sender: None, role, content }
}

fn answer() -> Value {
    json!({"status":"completed", "output":[
        {"type":"web_search_call", "id":"search", "status":"completed", "action":{"type":"search", "query":"release"}},
        {"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"Found 🦀.", "annotations":[
            {"type":"url_citation", "url":"https://example.com/release", "title":"Release"}
        ]}]},
    ], "usage":{"input_tokens":100,"output_tokens":20}})
}

fn tool_answer() -> Value {
    let mut response = answer();

    response["output"].as_array_mut().unwrap().extend([
        json!({"type":"reasoning", "encrypted_content":"opaque", "summary":[]}),
        json!({"type":"function_call", "call_id":"read-1", "name":"read", "arguments":"{\"path\":\"release.md\"}"}),
    ]);
    response
}

fn assistant(reply: ModelReply) -> Message {
    let mut content = vec![Content::Text { text: reply.text }];

    for event in reply.events {
        match event {
            Event::Context { state } => content.push(Content::Context { state }),

            Event::Tool { name, args, id, thought_signature } => {
                content.push(Content::ToolCall { name, args, id, thought_signature })
            }

            _ => panic!("unexpected event"),
        }
    }

    message(Role::Assistant, content)
}

#[tokio::test]
async fn native_search_uses_responses_and_returns_citations_for_both_transports() {
    for streaming in [false, true] {
        let response = answer();

        let raw = if streaming {
            format!("data: {}\n\n", json!({"type":"response.completed", "response":response}))
        } else {
            response.to_string()
        };

        let (base, server) = krabb_test::http::responses(vec![(200, raw)]).await;
        let client = reqwest::Client::new();
        let api = Api { client: &client, key: "fixture", base: &base, provider: "openai" };
        let (output, _events) = tokio::sync::mpsc::channel(16);
        let mut emitter = Emitter::new(output);

        let result = api
            .generate(7, &input(), "gpt-6-luna", streaming.then_some(&mut emitter))
            .await
            .unwrap()
            .unwrap();

        let parsed: ModelReply = serde_json::from_value(result.result.unwrap()).unwrap();

        assert_eq!(parsed.text, "Found 🦀. [Release](https://example.com/release)");
        assert!(parsed.events.is_empty());
        assert_eq!(parsed.input, Some(100));

        let requests = server.await.unwrap();
        let body = &requests[0].body;

        assert!(requests[0].headers.starts_with("POST /responses "));
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], streaming);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][1]["type"], "web_search");
        assert_eq!(body["max_output_tokens"], 2048);
        assert_eq!(body["reasoning"]["effort"], "none");
    }
}

#[test]
fn native_continuation_and_function_results_survive_serialization() {
    let response = tool_answer();
    let mut request = input();

    let saved =
        serde_json::to_string(&assistant(reply(&response, &request.model, "openai").unwrap()))
            .unwrap();

    request.messages.push(serde_json::from_str(&saved).unwrap());

    request.messages.push(message(
        Role::Tool,
        vec![
            Content::Text { text: "notes".into() },
            Content::Image { uri: "data:image/png;base64,AA==".into(), alt: None },
        ],
    ));

    let body = body(&request, &request.model, "openai", false).unwrap();
    let items = body["input"].as_array().unwrap();

    assert_eq!(&items[1..5], response["output"].as_array().unwrap());
    assert_eq!(items[5]["type"], "function_call_output");
    assert_eq!(items[5]["call_id"], "read-1");
    assert_eq!(items[5]["output"][1]["type"], "input_image");

    assert!(
        !request.messages[1]
            .content
            .iter()
            .map(Content::render)
            .collect::<String>()
            .contains("opaque")
    );

    let other = super::body(&request, &request.model, "codex", false).unwrap();

    assert!(!other.to_string().contains("opaque"));

    request.messages.pop();

    let interrupted = super::body(&request, &request.model, "openai", false).unwrap();

    assert!(interrupted.to_string().contains("do not retry automatically"));

    request.messages[1].content.retain(|content| !matches!(content, Content::ToolCall { .. }));

    assert!(
        super::body(&request, &request.model, "openai", false)
            .unwrap()
            .to_string()
            .contains("Execution status is unknown")
    );
}

#[tokio::test]
async fn stream_boundaries_preserve_unicode_tools_and_completion() {
    let response = tool_answer();

    let raw = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({"type":"response.output_text.delta", "delta":"Found 🦀."}),
        json!({"type":"response.completed", "response":response})
    );

    let expected = reply(&response, "gpt-6-luna", "openai").unwrap();

    for split in 0..=raw.len() {
        let chunks = vec![
            Ok::<_, std::io::Error>(raw.as_bytes()[..split].to_vec()),
            Ok(raw.as_bytes()[split..].to_vec()),
        ];

        let stream = futures_util::stream::iter(chunks).chain(futures_util::stream::pending());
        let (output, mut events) = tokio::sync::mpsc::channel(16);
        let mut emitter = Emitter::new(output);

        let parsed = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            stream::parse(stream, "gpt-6-luna", "openai", &mut emitter),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(parsed, expected);
        assert_eq!(events.try_recv().unwrap()["params"]["event"]["text"], "Found 🦀.");
    }
}

#[tokio::test]
async fn incomplete_streams_and_http_errors_fail_without_executing_tools() {
    for raw in [
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        "data: {\"type\":\"error\"}\n\n",
        "data: invalid\n\n",
    ] {
        let (output, _events) = tokio::sync::mpsc::channel(16);
        let mut emitter = Emitter::new(output);

        assert!(
            stream::parse(
                futures_util::stream::iter(vec![Ok::<_, std::io::Error>(raw.as_bytes())]),
                "model",
                "openai",
                &mut emitter
            )
            .await
            .is_err()
        );
    }

    let (base, server) = krabb_test::http::responses(vec![(403, "{}".into())]).await;

    let client = reqwest::Client::new();
    let api = Api { client: &client, key: "fixture", base: &base, provider: "openai" };

    assert!(
        api.generate(1, &input(), "model", None).await.unwrap_err().to_string().contains("403")
    );

    server.await.unwrap();
}

#[test]
fn summaries_disable_tools_and_malformed_replies_fail_closed() {
    let mut request = input();

    request.generation.as_mut().unwrap().summary = true;

    assert_eq!(body(&request, &request.model, "openai", false).unwrap()["tools"], json!([]));

    assert!(body(&request, "gpt-6-astra", "openai", false).unwrap()["reasoning"].is_null());

    for value in [
        json!({}),
        json!({"status":"completed"}),
        json!({"status":"completed","output":[]}),
        json!({"status":"completed","output":[{"type":"shell_call"}]}),
        json!({"status":"incomplete","incomplete_details":{"reason":"content_filter"}}),
    ] {
        assert!(reply(&value, "model", "openai").is_err());
    }

    let mut response = answer();

    response["status"] = "incomplete".into();
    response["incomplete_details"] = json!({"reason":"max_output_tokens"});

    assert_eq!(reply(&response, "model", "openai").unwrap().stop, "length");

    response["output"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"function_call","name":"read","call_id":"id","arguments":"{}"}));

    assert!(reply(&response, "model", "openai").is_err());
}
