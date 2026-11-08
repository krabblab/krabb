use super::super::*;
use futures_util::{StreamExt, stream};

fn fixture() -> String {
    let text = json!({"candidates": [{"content": {"parts": [
        {"text": "internal", "thought": true}, {"text": "Hello 🦀"}
    ]}}]});

    let tool = json!({"candidates": [{"content": {"parts": [
        {"functionCall": {"id": "call_one", "name": "read", "args": {"path": "one"}}, "thoughtSignature": "opaque"}
    ]}}]});

    let done = json!({"candidates": [{"finishReason": "STOP"}], "usageMetadata": {"promptTokenCount": 7, "candidatesTokenCount": 9}});

    format!("data: {text}\r\n\r\ndata: {tool}\r\n\r\ndata: {done}\r\n\r\n")
}

#[tokio::test]
async fn every_transport_split_preserves_text_tools_signatures_and_usage() {
    let body = fixture();

    for split in 0..=body.len() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(256);
        let mut emitter = Emitter::new(sender);

        let chunks = stream::iter(vec![
            Ok::<_, std::io::Error>(body.as_bytes()[..split].to_vec()),
            Ok(body.as_bytes()[split..].to_vec()),
        ])
        .chain(stream::pending());

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            streaming::parse(7, chunks, &mut emitter),
        )
        .await
        .expect("must finish without waiting for transport close")
        .unwrap()
        .unwrap()
        .result
        .unwrap();

        assert_eq!(result["text"], "Hello 🦀");
        assert_eq!(result["input"], 7);
        assert_eq!(result["output"], 9);
        assert_eq!(result["events"][0]["id"], "call_one");
        assert_eq!(result["events"][0]["thought_signature"], "opaque");
        assert_eq!(receiver.try_recv().unwrap()["params"]["event"]["text"], "Hello 🦀");
    }
}

#[tokio::test]
async fn errors_blocked_responses_and_truncation_fail_closed() {
    for body in [
        "data: {}\n\n",
        "data: {\"error\":{\"message\":\"private detail\"}}\n\n",
        "data: {\"promptFeedback\":{\"blockReason\":\"SAFETY\"}}\n\n",
        "data: {\"candidates\":[{\"finishReason\":\"SAFETY\"}]}\n\n",
        "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n",
        "data: not-json\n\n",
    ] {
        let (sender, _receiver) = tokio::sync::mpsc::channel(256);
        let mut emitter = Emitter::new(sender);
        let chunks = stream::iter(vec![Ok::<_, std::io::Error>(body.as_bytes().to_vec())]);
        let error = streaming::parse(1, chunks, &mut emitter).await.unwrap_err().to_string();

        assert!(!error.contains("private detail"));
    }
}

#[tokio::test]
async fn bounds_streams_and_handles_transport_and_consumer_failures() {
    for body in [vec![b'x'; BODY_LIMIT + 1], vec![0xff, b'\n']] {
        let (sender, _receiver) = tokio::sync::mpsc::channel(256);
        let chunks = stream::iter(vec![Ok::<_, std::io::Error>(body)]);

        assert!(streaming::parse(1, chunks, &mut Emitter::new(sender)).await.is_err());
    }

    let (sender, _receiver) = tokio::sync::mpsc::channel(256);

    let chunks = stream::iter(vec![Err::<Vec<u8>, _>(std::io::Error::other("fixture"))]);

    assert!(streaming::parse(1, chunks, &mut Emitter::new(sender)).await.is_err());

    let (sender, receiver) = tokio::sync::mpsc::channel(256);

    drop(receiver);

    let chunks = stream::iter(vec![Ok::<_, std::io::Error>(fixture().into_bytes())]);

    assert!(streaming::parse(1, chunks, &mut Emitter::new(sender)).await.is_err());
}

#[test]
fn rejects_malformed_and_excessive_calls_and_preserves_stop_reason() {
    for part in [json!({"functionCall": {}}), json!({"functionCall": {"name": "read", "args": []}})]
    {
        assert!(
            streaming::Reply::default()
                .absorb(&json!({"candidates": [{"content": {"parts": [part]}}]}))
                .is_err()
        );
    }

    let parts = vec![json!({"functionCall": {"name": "read"}}); 17];

    assert!(
        streaming::Reply::default()
            .absorb(&json!({"candidates": [{"content": {"parts": parts}}]}))
            .is_err()
    );

    let mut reply = streaming::Reply::default();

    reply.absorb(&json!({"candidates": [{"content": {"parts": [{"text": "partial"}]}, "finishReason": "MAX_TOKENS"}]})).unwrap();

    assert_eq!(reply.finish(false).unwrap().stop, "length");

    let mut reply = streaming::Reply::default();

    reply.absorb(&json!({"candidates": [{"content": {"parts": [{"functionCall": {"name": "read"}}]}, "finishReason": "MAX_TOKENS"}]})).unwrap();

    assert!(reply.finish(false).is_err());
}

#[tokio::test]
async fn coalesces_notifications_without_losing_final_text() {
    let frame = json!({"candidates": [{"content": {"parts": [{"text": "x".repeat(128)}]}}]});

    let body = format!("data: {frame}\n\n").repeat(300)
        + "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n";

    let chunks = stream::iter(vec![Ok::<_, std::io::Error>(body.into_bytes())]);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(256);
    let result = streaming::parse(1, chunks, &mut Emitter::new(sender)).await.unwrap().unwrap();

    assert_eq!(result.result.unwrap()["text"], "x".repeat(128 * 300));

    let mut count = 0;

    while receiver.try_recv().is_ok() {
        count += 1;
    }

    assert_eq!(count, 256);
}

#[tokio::test]
async fn streams_over_http_without_credentials_in_the_url() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("loopback required");
    let base = format!("http://{}", listener.local_addr().unwrap());

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        let n = socket.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..n]);

        assert!(
            request.starts_with("POST /models/fixture:streamGenerateContent?alt=sse HTTP/1.1\r\n")
        );

        assert!(request.contains("x-goog-api-key: fixture-key\r\n"));
        assert!(!request.lines().next().unwrap().contains("fixture-key"));
        let body = fixture();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    });

    let input = ModelRequest {
        generation: None,
        model: "fixture".into(),
        workspace: None,
        messages: vec![],
        tools: vec![],
        stream: true,
    };

    let (sender, _receiver) = tokio::sync::mpsc::channel(256);

    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();

    assert!(
        streaming::generate(&client, 1, &input, "fixture-key", &base, &mut Emitter::new(sender))
            .await
            .unwrap()
            .is_some()
    );

    server.await.unwrap();
}
