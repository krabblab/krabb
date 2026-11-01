use krabb_core::{plugin::Emitter, types::Response};
use serde_json::json;

use krabb_test::stream as contract;

async fn parse(
    chunks: Vec<Result<Vec<u8>, std::io::Error>>,
    emitter: &mut Emitter,
) -> krabb_core::Result<Option<Response>> {
    let stream = futures_util::stream::iter(chunks);

    crate::stream_body(7, stream, emitter).await
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

#[tokio::test]
async fn terminal_marker_completes_without_transport_eof() {
    use futures_util::{FutureExt, StreamExt, stream};
    use krabb_core::types::{Event, ModelReply};

    let fixture = fixture();
    let prefix = format!("{}{}", fixture.text, fixture.tools).into_bytes();

    for split in 0..=fixture.done.len() {
        let chunks = vec![
            Ok::<_, std::io::Error>(prefix.clone()),
            Ok(fixture.done.as_bytes()[..split].to_vec()),
            Ok(fixture.done.as_bytes()[split..].to_vec()),
        ];

        let stream = stream::iter(chunks).chain(stream::pending());
        let (output, mut events) = tokio::sync::mpsc::channel(256);
        let mut emitter = Emitter::new(output);

        let response = crate::stream_body(7, stream, &mut emitter)
            .now_or_never()
            .expect("terminal marker must finish while the transport is still pending")
            .unwrap()
            .unwrap();

        let reply: ModelReply = serde_json::from_value(response.result.unwrap()).unwrap();

        assert_eq!(response.id, 7);
        assert!(response.error.is_none());
        assert_eq!(reply.text, format!("{}fin", contract::text()));
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
}

#[tokio::test]
async fn terminal_marker_ignores_trailing_data() {
    let fixture = fixture();
    let body = format!("{}{}{}data: invalid\n", fixture.text, fixture.tools, fixture.done);
    let (output, _events) = tokio::sync::mpsc::channel(256);
    let mut emitter = Emitter::new(output);
    let response = parse(vec![Ok(body.into_bytes())], &mut emitter).await.unwrap().unwrap();

    assert_eq!(response.result.unwrap()["text"], format!("{}fin", contract::text()));
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
