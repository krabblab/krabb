use super::*;

use krabb_core::{
    context::Generation,
    types::{ModelReply, ToolSpec},
};

fn input() -> ModelRequest {
    ModelRequest {
        model: "gemini-3.5-flash-lite".into(),
        workspace: None,
        stream: false,
        generation: Some(Generation { output: 2048, search: true, summary: false }),
        tools: vec![ToolSpec {
            name: "read".into(),
            description: None,
            schema: json!({"type":"object"}),
        }],
        messages: vec![Message {
            id: "user".into(),
            session: "fixture".into(),
            role: Role::User,
            sender: None,
            content: vec![Content::Text { text: "Search, then read the release notes.".into() }],
        }],
    }
}

fn candidate() -> Value {
    json!({"candidates":[{
        "content":{"role":"model", "parts":[
            {"toolCall":{"toolType":"GOOGLE_SEARCH", "args":{"queries":["release"]}}, "thoughtSignature":"search-signature"},
            {"toolResponse":{"toolType":"GOOGLE_SEARCH", "response":{"result":"found"}}},
            {"text":"Found the release."},
            {"functionCall":{"id":"call", "name":"read", "args":{"path":"release.md"}}, "thoughtSignature":"function-signature"},
        ]}, "finishReason":"STOP",
        "groundingMetadata":{"groundingChunks":[{"web":{"uri":"https://example.com/release", "title":"Release"}}]},
    }]})
}

fn assistant(reply: ModelReply) -> Message {
    let mut content = vec![Content::Text { text: reply.text }];

    for event in reply.events {
        match event {
            Event::Tool { name, args, id, thought_signature } => {
                content.push(Content::ToolCall { name, args, id, thought_signature })
            }

            Event::Context { state } => content.push(Content::Context { state }),
            _ => panic!("unexpected event"),
        }
    }

    Message {
        id: "assistant".into(),
        session: "fixture".into(),
        role: Role::Assistant,
        sender: None,
        content,
    }
}

#[tokio::test]
async fn native_search_preserves_signed_context_across_function_calls_and_serialization() {
    let response = candidate();

    let (base, server) = krabb_test::http::responses(vec![(200, response.to_string())]).await;

    let result =
        generate_at(&reqwest::Client::new(), 1, input(), "fixture", &base).await.unwrap().unwrap();

    let reply: ModelReply = serde_json::from_value(result.result.unwrap()).unwrap();

    assert!(reply.text.contains("[Release](https://example.com/release)"));
    assert_eq!(reply.events.len(), 2);

    let saved = serde_json::to_string(&assistant(reply)).unwrap();
    let assistant: Message = serde_json::from_str(&saved).unwrap();
    let mut followup = input();

    followup.messages.push(assistant.clone());

    followup.messages.push(Message {
        id: "tool".into(),
        session: "fixture".into(),
        role: Role::Tool,
        sender: Some("read".into()),
        content: vec![Content::Text { text: "Release notes".into() }],
    });

    let body = request_body(&followup).unwrap();

    assert_eq!(body["contents"][1]["parts"], response["candidates"][0]["content"]["parts"]);
    assert_eq!(body["contents"][2]["parts"][0]["functionResponse"]["id"], "call");
    assert_eq!(body["toolConfig"]["includeServerSideToolInvocations"], true);
    assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "VALIDATED");
    assert!(replay(&assistant, "other-model").unwrap().is_none());

    let mut damaged = assistant.clone();

    damaged.content.retain(|part| !matches!(part, Content::ToolCall { .. }));

    let skipped = replay(&damaged, &followup.model).unwrap().unwrap();

    assert!(skipped.calls.is_empty());
    assert_eq!(skipped.skipped[0]["functionResponse"]["id"], "call");
    assert!(skipped.skipped[0].to_string().contains("do not retry automatically"));

    damaged.content.push(Content::ToolCall {
        name: "write".into(),
        args: json!({}),
        id: Some("call".into()),
        thought_signature: None,
    });

    assert!(replay(&damaged, &followup.model).is_err());

    let requests = server.await.unwrap();

    assert!(requests[0].headers.contains(":generateContent "));
    assert_eq!(requests[0].body["tools"][1], json!({"googleSearch":{}}));
}

#[tokio::test]
async fn streamed_search_preserves_context_and_attribution() {
    let mut reply = streaming::Reply::new("fixture");
    let response = candidate();

    reply.absorb(&response).unwrap();
    reply.absorb(&json!({"candidates":[{"groundingMetadata":response["candidates"][0]["groundingMetadata"]}]})).unwrap();

    let reply = reply.finish(true).unwrap();

    assert_eq!(reply.text.matches("[Release]").count(), 1);

    let (output, mut events) = tokio::sync::mpsc::channel(16);
    let mut emitter = Emitter::new(output);

    let stream = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(
        format!("data: {response}\n\n").into_bytes(),
    )]);

    let result = streaming::parse(1, stream, &mut emitter).await.unwrap().unwrap();
    let streamed: ModelReply = serde_json::from_value(result.result.unwrap()).unwrap();

    assert_eq!(streamed, reply);

    assert!(
        events.try_recv().unwrap()["params"]["event"]["text"]
            .as_str()
            .unwrap()
            .contains("[Release]")
    );
}

#[test]
fn unsupported_combinations_and_summaries_do_not_enable_search() {
    let mut request = input();

    request.model = "gemini-2.5-flash".into();

    let body = request_body(&request).unwrap();

    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert!(body["tools"][0].get("googleSearch").is_none());

    request.tools.clear();

    assert_eq!(request_body(&request).unwrap()["tools"], json!([{"googleSearch":{}}]));

    request.generation.as_mut().unwrap().summary = true;

    assert!(request_body(&request).unwrap().get("tools").is_none());
    assert!(state("model", vec![json!({"text":"ordinary"})]).unwrap().is_none());
    assert!(state("model", vec![json!({"toolResponse":{"data":"x".repeat(256 * 1024)}})]).is_err());

    assert!(
        citations(
            &json!({"groundingMetadata":{"groundingChunks":[{"web":{"uri":"javascript:bad"}}]}})
        )
        .is_empty()
    );
}
