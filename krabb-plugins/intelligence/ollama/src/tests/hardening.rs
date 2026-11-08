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
        generation: None,
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
    let values = messages(&history()).unwrap();

    assert_eq!(values[0]["tool_calls"][0]["function"]["name"], "read");
    assert_eq!(values[0]["tool_calls"][1]["function"]["arguments"], json!({"path": "two"}));
    assert_eq!(values[1]["role"], "tool");
    assert_eq!(values[1]["tool_name"], "read");
    assert_eq!(values[2]["tool_name"], "read");
    assert_eq!(values[2]["images"][0], "aW1hZ2U=");
}

#[tokio::test]
async fn clean_disconnect_before_completion_is_an_error() {
    let (sender, _receiver) = tokio::sync::mpsc::channel(256);
    let mut emitter = Emitter::new(sender);

    let chunks = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(
        br#"{"message":{"content":"partial"},"done":false}
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
        assert!(validate_host(url, true).is_err());
    }

    assert!(validate_host("http://127.0.0.1:1234", true).is_ok());

    assert!(validate_host("http://[::1]:1234", true).is_ok());
    assert!(validate_host("https://example.com", true).is_ok());
}
