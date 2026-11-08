use super::*;
use serde_json::json;

fn message(role: Role, text: &str) -> Message {
    Message {
        id: text.into(),
        session: "fixture".into(),
        role,
        sender: None,
        content: vec![Content::Text { text: text.into() }],
    }
}

#[test]
fn metadata_defaults_and_budgets_are_compatible_and_bounded() {
    assert_eq!(serde_json::from_value::<Profile>(json!({})).unwrap(), Profile::default());

    assert_eq!(Window { tokens: 4096, output: 2048 }.input().unwrap(), 2048);

    for window in [
        Window { tokens: 0, output: 0 },
        Window { tokens: 4096, output: 4096 },
        Window { tokens: u64::MAX, output: 1 },
    ] {
        assert!(window.input().is_err());
    }

    let profile = Profile {
        window: Some(Window { tokens: 4096, output: 2048 }),
        compact: true,
        search: true,
    };

    assert_eq!(
        serde_json::from_value::<Profile>(serde_json::to_value(&profile).unwrap()).unwrap(),
        profile
    );
}

#[test]
fn truncation_keeps_instructions_and_the_entire_current_turn() {
    let mut messages = vec![
        message(Role::System, "policy"),
        message(Role::User, "old question"),
        message(Role::Assistant, "old call"),
        message(Role::Tool, "old result"),
        message(Role::System, "later policy"),
        message(Role::User, "current question"),
        message(Role::Assistant, "current call"),
        message(Role::Tool, "current result"),
    ];

    assert_eq!(boundary(&messages, 1), Some(5));
    assert_eq!(boundary(&messages, 2), None);
    assert!(truncate(&mut messages));

    assert_eq!(
        messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["policy", "later policy", "current question", "current call", "current result"]
    );

    assert!(!truncate(&mut messages));
    assert!(!truncate(&mut vec![message(Role::Assistant, "no user boundary")]));
}

#[test]
fn summaries_and_orphaned_old_prefixes_can_be_trimmed_without_touching_the_request() {
    let mut messages = vec![message(Role::Assistant, "summary"), message(Role::User, "latest")];

    assert!(truncate(&mut messages));
    assert_eq!(messages, [message(Role::User, "latest")]);
}

#[test]
fn estimates_include_schemas_tool_calls_and_media_without_counting_image_bytes() {
    let mut messages = vec![message(Role::User, "hello")];
    let original = estimate(&messages, &[]);

    let tools = vec![ToolSpec {
        name: "read".into(),
        description: Some("Read a file".into()),
        schema: json!({"type":"object","required":["path"],"count":2,"enabled":true,"empty":null}),
    }];

    assert!(estimate(&messages, &tools) > original);

    messages[0].content.push(Content::Image { uri: "A".repeat(10000), alt: None });

    assert_eq!(estimate(&messages, &[]), original + 768);

    messages[0].content.push(Content::ToolCall {
        name: "read".into(),
        args: json!({"path":"hello"}),
        id: None,
        thought_signature: None,
    });

    messages[0].content.push(Content::File { uri: "file".into(), name: "file".into(), mime: None });

    assert!(estimate(&messages, &[]) > original + 768);
    assert_eq!(text_tokens("abc"), 1);
    assert_eq!(text_tokens("你好"), 6);
}

#[test]
fn compaction_contract_round_trips_without_vendor_payloads() {
    let request = CompactRequest {
        model: "model".into(),
        messages: vec![message(Role::User, "earlier")],
        max_tokens: 256,
        output: None,
    };

    let decoded: CompactRequest =
        serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();

    assert_eq!(decoded, request);

    let reply = CompactReply { text: "The user prefers short answers.".into() };

    let decoded: CompactReply =
        serde_json::from_value(serde_json::to_value(&reply).unwrap()).unwrap();

    assert_eq!(decoded, reply);
}

#[test]
fn opaque_continuations_are_bounded_hidden_and_counted_in_context() {
    let state = State { provider: "fixture".into(), model: "small".into(), data: "opaque".into() };
    let content = Content::Context { state: state.clone() };
    let event = crate::Event::Context { state: state.clone() };

    assert!(state.valid());
    assert!(content.render().is_empty());

    assert_eq!(
        serde_json::from_value::<Content>(serde_json::to_value(&content).unwrap()).unwrap(),
        content
    );

    assert_eq!(
        serde_json::from_value::<crate::Event>(serde_json::to_value(&event).unwrap()).unwrap(),
        event
    );

    let mut messages = vec![message(Role::Assistant, "answer")];
    let before = estimate(&messages, &[]);

    messages[0].content.push(content);

    assert!(estimate(&messages, &[]) > before);

    for invalid in [
        State { provider: String::new(), ..state.clone() },
        State { provider: "x".repeat(65), ..state.clone() },
        State { model: String::new(), ..state.clone() },
        State { model: "x".repeat(257), ..state.clone() },
        State { data: "x".repeat(256 * 1024 + 1), ..state },
    ] {
        assert!(!invalid.valid());
    }
}
