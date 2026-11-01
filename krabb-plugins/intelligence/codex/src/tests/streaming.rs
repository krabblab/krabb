use krabb_core::{plugin::Emitter, types::Response};
use serde_json::json;

use krabb_test::stream as contract;

async fn parse(
    chunks: Vec<Result<Vec<u8>, std::io::Error>>,
    emitter: &mut Emitter,
) -> krabb_core::Result<Option<Response>> {
    let stream = futures_util::stream::iter(chunks);

    crate::stream_body(7, reqwest::StatusCode::OK, stream, emitter).await
}

fn fixture() -> contract::Fixture {
    let text = json!({"choices": [{"delta": {"content": contract::text()}}]});
    let args = contract::arguments().to_string();
    let split = args.find('🦀').unwrap();

    let first = json!({"choices": [{"delta": {"tool_calls": [{
        "index": 0, "function": {"name": "re", "arguments": &args[..split]}
    }]}}]});

    let second = json!({"choices": [{"delta": {"tool_calls": [{
        "index": 0, "function": {"name": "ad", "arguments": &args[split..]}
    }], "content": "fin"}}]});

    contract::Fixture {
        text: format!("data: {text}\r\n\r\n"),
        tools: format!("data: {first}\r\n\r\ndata: {second}\r\n\r\n"),
        done: "data: [DONE]\r\n".into(),
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
    assert_eq!(reply.stop, "stream");
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

async fn assert_suffix_ignored(suffix: &[u8]) {
    let fixture = fixture();
    let body = format!("{}{}{}", fixture.text, fixture.tools, fixture.done).into_bytes();

    // Split before completion, at its boundary, and throughout the trailing data.
    for split in std::iter::once(body.len())
        .chain(body.len() - fixture.done.len()..=body.len() + suffix.len())
    {
        let mut bytes = body.clone();
        bytes.extend_from_slice(suffix);
        let chunks = vec![
            Ok(bytes[..split].to_vec()),
            Ok(bytes[split..].to_vec()),
            Err(std::io::Error::other("reset after completion")),
        ];

        let (output, mut events) = tokio::sync::mpsc::channel(256);
        let mut emitter = Emitter::new(output);
        let response = parse(chunks, &mut emitter)
            .await
            .unwrap_or_else(|error| panic!("split {split}: {error}"))
            .unwrap();

        assert_completed(response, &mut events);
    }
}

#[tokio::test]
async fn terminal_marker_ignores_malformed_suffix() {
    for suffix in [b"data: invalid\n".as_slice(), b"data: invalid", &[0xff]] {
        assert_suffix_ignored(suffix).await;
    }
}

#[tokio::test]
async fn terminal_marker_ignores_late_text() {
    let delta = json!({"choices": [{"delta": {"content": "late"}}]});

    assert_suffix_ignored(format!("data: {delta}\n").as_bytes()).await;
}

#[tokio::test]
async fn terminal_marker_ignores_late_tool() {
    let delta = json!({"choices": [{"delta": {"tool_calls": [{
        "index": 1, "function": {"name": "write", "arguments": "{\"path\":\"late.txt\"}"}
    }]}}]});

    assert_suffix_ignored(format!("data: {delta}\n").as_bytes()).await;
}

#[tokio::test]
async fn terminal_marker_completes_with_pending_transport() {
    use futures_util::{StreamExt, stream};

    let fixture = fixture();

    for split in 0..=fixture.done.len() {
        let chunks = vec![
            Ok::<_, std::io::Error>(format!("{}{}", fixture.text, fixture.tools).into_bytes()),
            Ok(fixture.done.as_bytes()[..split].to_vec()),
            Ok(fixture.done.as_bytes()[split..].to_vec()),
        ];

        let stream = stream::iter(chunks).chain(stream::pending());
        let (output, mut events) = tokio::sync::mpsc::channel(256);
        let mut emitter = Emitter::new(output);
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            crate::stream_body(7, reqwest::StatusCode::OK, stream, &mut emitter),
        )
        .await
        .expect("terminal marker must complete without transport EOF")
        .unwrap()
        .unwrap();

        assert_completed(response, &mut events);
    }
}

#[tokio::test]
async fn eof_preserves_unterminated_final_line() {
    let fixture = fixture();
    let body = format!("{}{}", fixture.text, fixture.tools.trim_end());
    let (output, mut events) = tokio::sync::mpsc::channel(256);
    let mut emitter = Emitter::new(output);
    let response = parse(vec![Ok(body.into_bytes())], &mut emitter).await.unwrap().unwrap();

    assert_completed(response, &mut events);
}

#[tokio::test]
async fn eof_rejects_malformed_final_line() {
    let fixture = fixture();
    let body = format!("{}{}data: invalid", fixture.text, fixture.tools);
    let (output, _events) = tokio::sync::mpsc::channel(256);
    let mut emitter = Emitter::new(output);

    assert!(parse(vec![Ok(body.into_bytes())], &mut emitter).await.is_err());
}

#[tokio::test]
async fn multiple_tools_preserve_indexes_and_interleaved_fragments() {
    let frames = [
        json!({"choices": [{"delta": {"content": "héllo ", "tool_calls": [{
            "index": 1, "function": {"name": "wr", "arguments": "{\"path\":\"second"}
        }]}}]}),
        json!({"choices": [{"delta": {"tool_calls": [{
            "index": 0, "function": {"name": "re", "arguments": "{\"path\":\"first"}
        }]}}]}),
        json!({"choices": [{"delta": {"content": "🦀", "tool_calls": [
            {"index": 0, "function": {"name": "ad", "arguments": ".txt\",\"limit\":2}"}},
            {"index": 1, "function": {
                "name": "ite", "arguments": ".txt\",\"content\":\"two\"}"
            }}
        ]}}]}),
    ];

    let mut body: String = frames.iter().map(|frame| format!("data: {frame}\r\n\r\n")).collect();
    body.push_str("data: [DONE]\r\n");

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
