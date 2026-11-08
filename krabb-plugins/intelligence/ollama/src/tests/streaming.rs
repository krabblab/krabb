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
    let text = json!({"message": {"content": contract::text()}, "done": false});

    let tool = json!({"message": {"content": "fin", "tool_calls": [{
        "function": {"name": "read", "arguments": contract::arguments()}
    }]}, "done": false});

    contract::Fixture {
        text: format!("{text}\r\n"),
        tools: format!("{tool}\r\n"),
        done: format!(
            "{}\r\n",
            json!({"done": true, "done_reason": "length", "prompt_eval_count": 11, "eval_count": 7})
        ),
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
async fn empty_completed_streams_fail_without_exposing_thinking() {
    for content in ["", " \n\t"] {
        let frames = [
            json!({"message": {"thinking": "private reasoning"}, "done": false}),
            json!({"message": {"content": content}, "done": true,
                "done_reason": "length", "prompt_eval_count": 3839, "eval_count": 257}),
        ];

        let body: String = frames.iter().map(|frame| format!("{frame}\n")).collect();
        let (output, mut events) = tokio::sync::mpsc::channel(8);
        let mut emitter = Emitter::new(output);

        let error = parse(vec![Ok(body.into_bytes())], &mut emitter).await.unwrap_err();

        assert!(error.to_string().contains("no text or tools"));
        assert!(!error.to_string().contains("private reasoning"));

        while let Ok(event) = events.try_recv() {
            assert!(!event.to_string().contains("private reasoning"));
        }
    }
}

#[tokio::test]
async fn tool_only_streams_remain_valid() {
    let frame = json!({"message": {"content": "", "tool_calls": [{
        "function": {"name": "read", "arguments": {"path": "README.md"}}
    }]}, "done": true});

    let (output, _events) = tokio::sync::mpsc::channel(8);
    let mut emitter = Emitter::new(output);

    let response = parse(vec![Ok(format!("{frame}\n").into_bytes())], &mut emitter)
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();

    assert_eq!(response["text"], "");
    assert_eq!(response["events"][0]["name"], "read");
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
    assert_eq!(reply.stop, "length");
    assert_eq!(reply.input, Some(11));
    assert_eq!(reply.output, Some(7));
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
    for suffix in [b"invalid\n".as_slice(), b"invalid", &[0xff]] {
        assert_suffix_ignored(suffix).await;
    }
}

#[tokio::test]
async fn terminal_marker_ignores_late_text() {
    let delta = json!({"message": {"content": "late"}, "done": false});

    assert_suffix_ignored(format!("{delta}\n").as_bytes()).await;
}

#[tokio::test]
async fn terminal_marker_ignores_late_tool() {
    let delta = json!({"message": {"tool_calls": [{
        "function": {"name": "write", "arguments": {"path": "late.txt"}}
    }]}, "done": false});

    assert_suffix_ignored(format!("{delta}\n").as_bytes()).await;
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
            crate::live_stream(7, stream, &mut emitter),
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
    let body = format!("{}{}{}", fixture.text, fixture.tools, fixture.done.trim_end());
    let (output, mut events) = tokio::sync::mpsc::channel(256);
    let mut emitter = Emitter::new(output);
    let response = parse(vec![Ok(body.into_bytes())], &mut emitter).await.unwrap().unwrap();

    assert_completed(response, &mut events);
}

#[tokio::test]
async fn eof_rejects_malformed_final_line() {
    let fixture = fixture();
    let body = format!("{}{}invalid", fixture.text, fixture.tools);
    let (output, _events) = tokio::sync::mpsc::channel(256);
    let mut emitter = Emitter::new(output);

    assert!(parse(vec![Ok(body.into_bytes())], &mut emitter).await.is_err());
}

#[tokio::test]
async fn terminal_marker_ignores_late_metadata() {
    let delta = json!({
        "done": true, "done_reason": "stop", "prompt_eval_count": 100, "eval_count": 200
    });

    assert_suffix_ignored(format!("{delta}\n").as_bytes()).await;
}

#[tokio::test]
async fn multiple_tools_preserve_explicit_indexes_and_implicit_positions() {
    let first = json!({"function": {
        "name": "read", "arguments": {"path": "first.txt", "limit": 2}
    }});

    let second = json!({"function": {
        "name": "write", "arguments": "{\"path\":\"second.txt\",\"content\":\"two\"}"
    }});

    let implicit = vec![first.clone(), second.clone()];
    let mut indexed_first = first;
    let mut indexed_second = second;
    indexed_first["index"] = json!(0);
    indexed_second["index"] = json!(1);

    for calls in [implicit, vec![indexed_second, indexed_first]] {
        let frames = [
            json!({"message": {"content": "héllo "}, "done": false}),
            json!({"message": {"content": "🦀", "tool_calls": calls}, "done": false}),
            json!({"done": true}),
        ];

        let body: String = frames.iter().map(|frame| format!("{frame}\r\n")).collect();

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
}
