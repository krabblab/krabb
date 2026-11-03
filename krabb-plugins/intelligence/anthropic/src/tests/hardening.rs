use super::*;
use krabb_core::types::{Content, Message, ModelRequest, Role};
use serde_json::json;

fn history() -> ModelRequest {
    let message = |role, sender, content| Message {
        id: "fixture".into(),
        session: "fixture".into(),
        role,
        sender,
        content,
    };

    ModelRequest {
        model: "fixture".into(),
        workspace: None,
        stream: true,
        tools: vec![],
        messages: vec![
            message(
                Role::Assistant,
                None,
                vec![
                    Content::ToolCall {
                        name: "read".into(),
                        args: json!({"path": "one"}),
                        id: Some("call_one".into()),
                        thought_signature: None,
                    },
                    Content::ToolCall {
                        name: "read".into(),
                        args: json!({"path": "two"}),
                        id: Some("call_two".into()),
                        thought_signature: None,
                    },
                ],
            ),
            message(
                Role::Tool,
                Some("read".into()),
                vec![Content::Text { text: "first result".into() }],
            ),
            message(
                Role::Tool,
                Some("read".into()),
                vec![
                    Content::Text { text: "second result".into() },
                    Content::Image { uri: "data:image/png;base64,aW1hZ2U=".into(), alt: None },
                ],
            ),
        ],
    }
}

#[test]
fn preserves_parallel_calls_and_matches_repeated_tool_results() {
    let (_, values) = messages(&history()).unwrap();

    assert_eq!(values[0]["content"][0]["type"], "tool_use");
    assert_eq!(values[0]["content"][1]["input"], json!({"path": "two"}));
    assert_eq!(values[1]["content"][0]["tool_use_id"], "call_one");
    assert_eq!(values[1]["content"][1]["tool_use_id"], "call_two");
    assert_eq!(values[1]["content"][1]["content"][1]["type"], "image");
    assert_eq!(values.len(), 2);
}

#[tokio::test]
async fn clean_disconnect_before_completion_is_an_error() {
    let (sender, _receiver) = tokio::sync::mpsc::channel(256);
    let mut emitter = Emitter::new(sender);

    let chunks = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(
        br#"data: {"type":"content_block_delta","index":0,"delta":{"text":"partial"}}

"#
        .to_vec(),
    )]);

    let error = live_stream(1, chunks, &mut emitter).await.unwrap_err();

    assert!(error.to_string().contains("completion marker"));
}

#[test]
fn rejects_unsafe_authenticated_endpoints_before_network_access() {
    for url in [
        "http://example.com",
        "https://user:password@example.com",
        "https://example.com?key=private",
        "https://example.com#fragment",
        "ftp://localhost",
        "not-a-url",
    ] {
        assert!(validate_base(url).is_err());
    }

    assert!(validate_base("http://127.0.0.1:1234").is_ok());

    assert!(validate_base("http://[::1]:1234").is_ok());
    assert!(validate_base("https://example.com").is_ok());
}

#[test]
fn interrupted_calls_do_not_capture_results_from_later_turns() {
    for retained in [1, 2] {
        let mut input = history();

        input.messages.truncate(retained);

        let original = input.clone();
        let (_, incomplete) = messages(&input).unwrap();

        assert_eq!(incomplete.last().unwrap()["content"].as_array().unwrap().len(), 2);

        assert!(
            serde_json::to_string(&incomplete).unwrap().contains("Execution status is unknown")
        );

        assert_eq!(serde_json::to_value(&input).unwrap(), serde_json::to_value(original).unwrap());

        input.messages.extend([
            Message {
                id: "next-user".into(),
                session: "fixture".into(),
                role: Role::User,
                sender: None,
                content: vec![Content::Text { text: "Next request".into() }],
            },
            Message {
                id: "next-call".into(),
                session: "fixture".into(),
                role: Role::Assistant,
                sender: None,
                content: vec![Content::ToolCall {
                    name: "read".into(),
                    args: json!({}),
                    id: Some("call_next".into()),
                    thought_signature: None,
                }],
            },
            Message {
                id: "next-result".into(),
                session: "fixture".into(),
                role: Role::Tool,
                sender: Some("read".into()),
                content: vec![Content::Text { text: "new result".into() }],
            },
        ]);

        let (_, values) = messages(&input).unwrap();
        let missing = &values[1]["content"][1];

        assert_eq!(missing["tool_use_id"], "call_two");
        assert!(missing["content"].as_str().unwrap().contains("do not retry automatically"));
        assert_eq!(values[2]["content"], "Next request");

        let next = &values.last().unwrap()["content"][0];

        assert_eq!(next["tool_use_id"], "call_next");
        assert_eq!(next["content"][0]["text"], "new result");
    }
}

#[test]
fn unmatched_tool_results_do_not_leave_native_calls_open() {
    let mut input = history();

    input.messages.truncate(2);
    input.messages[1].sender = Some("other_tool".into());

    let (_, values) = messages(&input).unwrap();
    let encoded = serde_json::to_string(&values).unwrap();

    assert_eq!(encoded.matches("Execution status is unknown").count(), 2);
    assert_eq!(values.last().unwrap()["role"], "user");
    assert!(values.last().unwrap()["content"].as_str().unwrap().contains("first result"));
}
