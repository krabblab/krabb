use krabb_core::{
    plugin::Emitter,
    types::{Event, ModelReply, Response},
};

use serde_json::json;

pub trait Stream {
    fn parse(
        chunks: Vec<Result<Vec<u8>, std::io::Error>>,
        emitter: &mut Emitter,
    ) -> impl std::future::Future<Output = krabb_core::Result<Option<Response>>>;

    fn fixture() -> Fixture;
}

pub struct Fixture {
    pub text: String,
    pub tools: String,
    pub done: String,
}

pub fn text() -> String {
    "héllo 🦀 ".repeat(16)
}

pub fn arguments() -> serde_json::Value {
    json!({"path": "café/🦀.txt", "label": "say \"hi\"", "count": 2})
}

async fn run<S: Stream>(
    chunks: Vec<Result<Vec<u8>, std::io::Error>>,
) -> (krabb_core::Result<ModelReply>, String) {
    let (output, mut events) = tokio::sync::mpsc::channel(256);
    let mut emitter = Emitter::new(output);

    let result = S::parse(chunks, &mut emitter).await.map(|response| {
        let response = response.expect("stream must return a response");

        assert!(response.error.is_none());
        serde_json::from_value(response.result.expect("stream must return a result")).unwrap()
    });

    let mut streamed = String::new();

    while let Ok(event) = events.try_recv() {
        assert_eq!(event["params"]["event"]["kind"], "text");

        streamed.push_str(event["params"]["event"]["text"].as_str().unwrap());
    }

    (result, streamed)
}

fn assert_reply(reply: &ModelReply, streamed: &str) {
    assert_eq!(reply.text, format!("{}fin", text()));
    assert_eq!(streamed, reply.text);
    assert_eq!(reply.events.len(), 1);

    match &reply.events[0] {
        Event::Tool { name, args, .. } => {
            assert_eq!(name, "read");
            assert_eq!(*args, arguments());
        }

        other => panic!("expected a normalized tool call, got {other:?}"),
    }
}

fn body<S: Stream>() -> Vec<u8> {
    let fixture = S::fixture();

    format!("{}{}{}", fixture.text, fixture.tools, fixture.done).into_bytes()
}

pub async fn transport_whole_body_preserves_text_and_tools<S: Stream>() {
    let (reply, streamed) = run::<S>(vec![Ok(body::<S>())]).await;

    assert_reply(&reply.unwrap(), &streamed);
}

pub async fn transport_partitions_preserve_text_and_tools<S: Stream>() {
    let body = body::<S>();

    let (reply, streamed) =
        run::<S>(body.chunks(1).map(|chunk| Ok(chunk.to_vec())).collect()).await;

    assert_reply(&reply.expect("byte-sized chunks"), &streamed);

    // Exhaust every two-part split, including UTF-8, escaped JSON and CRLF boundaries.
    for split in 0..=body.len() {
        let (reply, streamed) =
            run::<S>(vec![Ok(body[..split].to_vec()), Ok(body[split..].to_vec())]).await;

        let reply = reply.unwrap_or_else(|error| panic!("split {split}: {error}"));

        assert_reply(&reply, &streamed);
    }
}

async fn assert_transport_error<S: Stream>(with_tools: bool) {
    let fixture = S::fixture();
    let mut chunks = vec![Ok(fixture.text.into_bytes())];

    if with_tools {
        chunks.push(Ok(fixture.tools.into_bytes()));
    }

    chunks.push(Err(std::io::Error::new(
        std::io::ErrorKind::ConnectionReset,
        "injected transport reset",
    )));

    let (reply, streamed) = run::<S>(chunks).await;
    let error = reply.expect_err("transport failure must not return accumulated content");

    assert!(error.to_string().contains("injected transport reset"), "{error}");
    assert!(streamed.starts_with(&text()), "text was accepted before the transport failed");
    assert!(format!("{}fin", text()).starts_with(&streamed));

    // A subsequent completed request must still normalize the full response.
    let (reply, streamed) = run::<S>(vec![Ok(body::<S>())]).await;

    assert_reply(&reply.unwrap(), &streamed);
}

pub async fn transport_error_after_text_fails_generation<S: Stream>() {
    assert_transport_error::<S>(false).await;
}

pub async fn transport_error_after_tools_fails_generation<S: Stream>() {
    assert_transport_error::<S>(true).await;
}

pub async fn ordered_tools<S: Stream>(body: &str, text: &str, tools: &[(&str, serde_json::Value)]) {
    let expected: Vec<_> = tools
        .iter()
        .map(|(name, args)| Event::Tool {
            name: (*name).into(),
            args: args.clone(),
            id: None,
            thought_signature: None,
        })
        .collect();

    // Whole body, single bytes (including UTF-8), and two bounded framing partitions.
    for width in [body.len(), 1, 7, 31] {
        let chunks = body.as_bytes().chunks(width).map(|chunk| Ok(chunk.to_vec())).collect();
        let (reply, streamed) = run::<S>(chunks).await;
        let reply = reply.unwrap_or_else(|error| panic!("chunk width {width}: {error}"));

        assert_eq!(reply.text, text, "chunk width {width}");
        assert_eq!(streamed, text, "chunk width {width}");
        assert_eq!(reply.events, expected, "chunk width {width}");
    }
}
