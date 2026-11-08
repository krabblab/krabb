use super::*;
use krabb_core::{context::Generation, types::ToolSpec};

fn input() -> ModelRequest {
    ModelRequest {
        model: "google/gemini-3.5-flash".into(),
        workspace: None,
        stream: false,
        generation: Some(Generation { output: 2048, summary: false, search: false }),
        tools: vec![ToolSpec {
            name: "read".into(),
            description: None,
            schema: json!({"type":"object"}),
        }],
        messages: vec![message(Role::User, vec![Content::Text { text: "Read notes".into() }])],
    }
}

fn message(role: Role, content: Vec<Content>) -> Message {
    Message { id: format!("{role:?}"), session: "fixture".into(), role, sender: None, content }
}

fn call() -> Event {
    Event::Tool {
        name: "read".into(),
        args: json!({"path":"notes"}),
        id: Some("read-1".into()),
        thought_signature: None,
    }
}

fn assistant(reply: ModelReply) -> Message {
    let content = reply
        .events
        .into_iter()
        .map(|event| match event {
            Event::Tool { name, args, id, thought_signature } => {
                Content::ToolCall { name, args, id, thought_signature }
            }

            Event::Context { state } => Content::Context { state },
            _ => panic!("Unexpected event"),
        })
        .collect();

    message(Role::Assistant, content)
}

#[tokio::test]
async fn reasoning_survives_both_http_transports_and_tool_history_without_becoming_visible() {
    let details = json!([
        {"type":"reasoning.text", "text":"private reasoning", "index":0},
        {"type":"reasoning.encrypted", "data":"opaque-signature", "index":1},
    ]);

    let tool = json!({"id":"read-1", "type":"function", "function":{
        "name":"read", "arguments":"{\"path\":\"notes\"}",
    }});

    for streaming in [false, true] {
        let mut request = input();

        let raw = if streaming {
            let mut chunks = String::new();

            for detail in details.as_array().unwrap() {
                chunks.push_str(&format!(
                    "data: {}\n\n",
                    json!({"choices":[{"delta":{
                        "reasoning_details":[detail],
                    }}]})
                ));
            }

            let mut tool = tool.clone();
            tool["index"] = 0.into();
            chunks.push_str(&format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{
                    "delta":{"tool_calls":[tool]}, "finish_reason":"tool_calls",
                }]})
            ));
            chunks
        } else {
            json!({"choices":[{"message":{
                "content":null, "tool_calls":[tool], "reasoning_details":details,
            }, "finish_reason":"tool_calls"}]})
            .to_string()
        };

        let (base, server) = krabb_test::http::responses(vec![(200, raw)]).await;
        let client = reqwest::Client::new();
        let (output, _events) = tokio::sync::mpsc::channel(8);
        let mut emitter = Emitter::new(output);

        let result = if streaming {
            stream(&client, 1, request.clone(), "fixture", &base, &mut emitter).await
        } else {
            complete(&client, 1, request.clone(), "fixture", &base).await
        }
        .unwrap()
        .unwrap();

        let reply: ModelReply = serde_json::from_value(result.result.unwrap()).unwrap();

        server.await.unwrap();

        assert!(reply.text.is_empty());

        let saved = serde_json::to_string(&assistant(reply)).unwrap();

        request.messages.push(serde_json::from_str(&saved).unwrap());
        request.messages.push(message(Role::Tool, vec![Content::Text { text: "notes".into() }]));

        let body = request_body(&request, false).unwrap();

        assert_eq!(body["messages"][1]["reasoning_details"], details);
        assert_eq!(body["messages"][1]["tool_calls"], json!([tool]));
        assert_eq!(body["messages"][2]["tool_call_id"], "read-1");

        assert!(
            !request.messages[1]
                .content
                .iter()
                .map(Content::render)
                .collect::<String>()
                .contains("private")
        );

        request.messages[1].content.retain(|part| !matches!(part, Content::ToolCall { .. }));

        let pruned = request_body(&request, false).unwrap();

        assert_eq!(pruned["messages"][1]["reasoning_details"], details);
        assert!(pruned["messages"][2]["content"].as_str().unwrap().contains("do not retry"));

        request.model = "other/model".into();

        assert!(!request_body(&request, false).unwrap().to_string().contains("opaque-signature"));
    }
}

#[test]
fn reasoning_is_bounded_and_replay_rejects_changed_calls() {
    let mut reasoning = Reasoning::default();

    reasoning.collect(&json!({"reasoning":"private"})).unwrap();

    let mut events = vec![call()];

    reasoning.attach(&input().model, &mut events).unwrap();

    let mut message = assistant(ModelReply {
        text: String::new(),
        events,
        stop: "tool".into(),
        input: None,
        output: None,
        context_usage: None,
    });

    let mut wire = json!({});

    restore(&message, &input().model, &mut wire).unwrap();

    assert_eq!(wire["reasoning"], "private");

    if let Content::ToolCall { args, .. } = &mut message.content[0] {
        *args = json!({"path":"changed"});
    }

    assert!(restore(&message, &input().model, &mut wire).is_err());
    assert!(Reasoning::default().collect(&json!({"reasoning_details":"bad"})).is_err());
    assert!(Reasoning::default().collect(&json!({"reasoning":"x".repeat(240*1024+1)})).is_err());

    let mut reasoning = Reasoning::default();

    reasoning.collect(&json!({"reasoning":"private"})).unwrap();

    let mut missing = vec![Event::Tool {
        name: "read".into(),
        args: json!({}),
        id: None,
        thought_signature: None,
    }];

    assert!(reasoning.attach("model", &mut missing).is_err());

    let mut request = input();

    request.generation.as_mut().unwrap().summary = true;

    let body = request_body(&request, false).unwrap();

    assert_eq!(body["tools"], json!([]));
    assert!(body["reasoning"].is_null());
}
