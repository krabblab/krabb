use krabb_core::{plugin::Emitter, types::Response};
use serde_json::json;

use krabb_test::stream as contract;

async fn parse(
    chunks: Vec<Result<Vec<u8>, std::io::Error>>,
    emitter: &mut Emitter,
) -> krabb_core::Result<Option<Response>> {
    let stream = futures_util::stream::iter(chunks);

    crate::live_stream(7, stream, emitter).await
}

fn fixture() -> contract::Fixture {
    let text = json!({"type": "content_block_delta", "index": 0,
        "delta": {"type": "text_delta", "text": contract::text()}});

    let start = json!({"type": "content_block_start", "index": 1,
        "content_block": {"type": "tool_use", "name": "read", "input": {}}});

    let args = contract::arguments().to_string();
    let split = args.find('🦀').unwrap();

    let first = json!({"type": "content_block_delta", "index": 1,
        "delta": {"type": "input_json_delta", "partial_json": &args[..split]}});

    let second = json!({"type": "content_block_delta", "index": 1,
        "delta": {"type": "input_json_delta", "partial_json": &args[split..]}});

    let end = json!({"type": "content_block_delta", "index": 0,
        "delta": {"type": "text_delta", "text": "fin"}});

    contract::Fixture {
        text: format!("data: {text}\r\n\r\n"),
        tools: format!(
            "data: {start}\r\n\r\ndata: {first}\r\n\r\ndata: {second}\r\n\r\ndata: {end}\r\n\r\n"
        ),
        done: "data: {\"type\":\"message_stop\"}\r\n".into(),
    }
}

struct Provider;

impl contract::Stream for Provider {
    async fn parse(
        chunks: Vec<Result<Vec<u8>, std::io::Error>>,
        emitter: &mut Emitter,
    ) -> krabb_core::Result<Option<Response>> {
        parse(chunks, emitter).await
    }

    fn fixture() -> contract::Fixture {
        fixture()
    }
}

#[tokio::test]
async fn transport_whole_body_preserves_text_and_tools() {
    contract::transport_whole_body_preserves_text_and_tools::<Provider>().await;
}

#[tokio::test]
async fn transport_partitions_preserve_text_and_tools() {
    contract::transport_partitions_preserve_text_and_tools::<Provider>().await;
}

#[tokio::test]
async fn transport_error_after_text_fails_generation() {
    contract::transport_error_after_text_fails_generation::<Provider>().await;
}

#[tokio::test]
async fn transport_error_after_tools_fails_generation() {
    contract::transport_error_after_tools_fails_generation::<Provider>().await;
}

fn assert_completed(
    response: Response,
    events: &mut tokio::sync::mpsc::Receiver<serde_json::Value>,
) {
    use krabb_core::types::{Event, ModelReply};

    assert_eq!(response.id, 7);
    assert!(response.error.is_none());
    let reply: ModelReply = serde_json::from_value(response.result.unwrap()).unwrap();

    assert_eq!(reply.text, format!("{}fin", contract::text()));
    assert_eq!(reply.stop, "tool_use");
    assert_eq!(reply.events.len(), 1);
    assert!(matches!(
        &reply.events[0],
        Event::Tool { name, args, .. } if name == "read" && *args == contract::arguments()
    ));

    let mut streamed = String::new();

    while let Ok(event) = events.try_recv() {
        assert_eq!(event["params"]["event"]["kind"], "text");
        streamed.push_str(event["params"]["event"]["text"].as_str().unwrap());
    }

    assert_eq!(streamed, reply.text);
}

fn completed_prefix(fixture: &contract::Fixture) -> Vec<u8> {
    format!(
        "{}{}data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}}}}\n",
        fixture.text, fixture.tools
    )
    .into_bytes()
}

#[tokio::test]
async fn message_stop_completes_without_transport_eof() {
    use futures_util::{StreamExt, stream};

    let fixture = fixture();

    for split in 0..=fixture.done.len() {
        let chunks = vec![
            Ok::<_, std::io::Error>(completed_prefix(&fixture)),
            Ok(fixture.done.as_bytes()[..split].to_vec()),
            Ok(fixture.done.as_bytes()[split..].to_vec()),
        ];

        let stream = stream::iter(chunks).chain(stream::pending());
        let (output, mut events) = tokio::sync::mpsc::channel(256);
        let mut emitter = Emitter::new(output);
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            crate::live_stream(7, stream, &mut emitter),
        )
        .await
        .expect("message_stop must finish while the transport is still pending")
        .unwrap()
        .unwrap();

        assert_completed(response, &mut events);
    }
}

#[tokio::test]
async fn message_stop_preserves_reply_before_trailing_errors() {
    let fixture = fixture();

    for trailing in ["", "data: invalid\n", "data: {\"type\":\"error\"}\n"] {
        let mut body = completed_prefix(&fixture);
        body.extend_from_slice(fixture.done.as_bytes());
        body.extend_from_slice(trailing.as_bytes());
        let chunks = vec![Ok(body), Err(std::io::Error::other("reset after completion"))];
        let (output, mut events) = tokio::sync::mpsc::channel(256);
        let mut emitter = Emitter::new(output);
        let response = parse(chunks, &mut emitter).await.unwrap().unwrap();

        assert_completed(response, &mut events);
    }
}

#[tokio::test]
async fn message_delta_does_not_complete_before_message_stop() {
    let fixture = fixture();
    let chunks = vec![
        Ok(completed_prefix(&fixture)),
        Err(std::io::Error::other("reset before message_stop")),
    ];

    let (output, _events) = tokio::sync::mpsc::channel(256);
    let mut emitter = Emitter::new(output);
    let error = parse(chunks, &mut emitter).await.expect_err("stop_reason is not message_stop");

    assert!(error.to_string().contains("reset before message_stop"));
}

#[tokio::test]
async fn multiple_tools_preserve_block_arguments_and_order() {
    let frames = [
        json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": "héllo "}}),
        json!({"type": "content_block_start", "index": 1,
            "content_block": {"type": "tool_use", "name": "read", "input": {}}}),
        json!({"type": "content_block_delta", "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{\"path\":\"first"}}),
        json!({"type": "content_block_delta", "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": ".txt\",\"limit\":2}"}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "content_block_start", "index": 2,
            "content_block": {"type": "tool_use", "name": "write", "input": {}}}),
        json!({"type": "content_block_delta", "index": 2,
            "delta": {"type": "input_json_delta", "partial_json": "{\"path\":\"second"}}),
        json!({"type": "content_block_delta", "index": 2,
            "delta": {"type": "input_json_delta",
                "partial_json": ".txt\",\"content\":\"two\"}"}}),
        json!({"type": "content_block_stop", "index": 2}),
        json!({"type": "content_block_delta", "index": 3,
            "delta": {"type": "text_delta", "text": "🦀"}}),
        json!({"type": "message_stop"}),
    ];

    let body: String = frames.iter().map(|frame| format!("data: {frame}\r\n\r\n")).collect();

    contract::ordered_tools::<Provider>(
        &body,
        "héllo 🦀",
        &[
            ("read", json!({"path": "first.txt", "limit": 2})),
            ("write", json!({"path": "second.txt", "content": "two"})),
        ],
    )
    .await;
}
