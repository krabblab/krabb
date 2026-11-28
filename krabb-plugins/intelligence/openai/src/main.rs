#![forbid(unsafe_code)]

mod model;

use krabb_core::{
    plugin::{Emitter, serve_events_concurrent},
    types::{Capability, Hello, ModelRequest, Protocol, Request, Response},
};

use krabb_openai::chat;
use serde_json::{Value, json};
use tokio::time::Duration;

const PROVIDER: &str = "OpenAI";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    krabb_log::run_plugin("openai", plugin_main()).await
}

async fn plugin_main() -> krabb_core::Result<()> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| krabb_core::Error::Denied(format!("OpenAI client failed: {error}.")))?;

    serve_events_concurrent(
        Hello {
            formats: vec![],
            protocol: Protocol::CURRENT,
            id: "openai".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec![Capability::Model, Capability::Vision, Capability::Context],
            commands: vec![],
        },
        move |request, emitter| {
            let client = client.clone();
            async move { generate(&client, request, emitter).await }
        },
    )
    .await
}

async fn generate(
    client: &reqwest::Client,
    request: Request,
    mut emitter: Emitter,
) -> krabb_core::Result<Option<Response>> {
    let (id, method, params) = match request {
        Request::Call { id, method, params, .. } => (id, method, params),
        Request::Note { .. } => return Ok(None),
    };

    if method == "model.info" {
        let model = model::resolve(
            params["model"].as_str().unwrap_or("default"),
            std::env::var("KRABB_OPENAI_MODEL").ok().as_deref(),
        );

        let profile = krabb_openai::profile(
            &model,
            &std::env::var("KRABB_OPENAI_CONTEXT_LENGTH").unwrap_or_default(),
        )?;

        return Ok(Some(Response::ok(id, serde_json::to_value(profile)?)));
    }

    if method == "check" {
        let request = client.get(format!("{}/models", base_url()?)).bearer_auth(credential()?);

        krabb_http::accepted(request, "OpenAI").await?;

        return Ok(Some(Response::ok(id, json!({}))));
    }

    if method != "generate" {
        return Ok(None);
    }

    let input: ModelRequest = serde_json::from_value(params)?;
    let key = credential()?;
    let base = base_url()?;

    let model = model::resolve(&input.model, std::env::var("KRABB_OPENAI_MODEL").ok().as_deref());

    let request = ModelRequest { model, ..input };

    if request.stream {
        stream(client, id, request, &key, &base, &mut emitter).await
    } else {
        complete(client, id, request, &key, &base).await
    }
}

fn base_url() -> krabb_core::Result<String> {
    let value = std::env::var("KRABB_OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".into());

    validate_base_url(&value)
}

fn validate_base_url(value: &str) -> krabb_core::Result<String> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| krabb_core::Error::Denied("OpenAI base URL is invalid.".into()))?;

    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));

    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(krabb_core::Error::Denied(
            "OpenAI base URL must use HTTPS outside loopback.".into(),
        ));
    }

    Ok(value.trim_end_matches('/').into())
}

fn request_body(input: &ModelRequest, stream: bool) -> krabb_core::Result<serde_json::Value> {
    let mut body = chat::body(PROVIDER, input, &model::resolve(&input.model, None), stream)?;

    model::configure(&mut body);

    Ok(body)
}

async fn complete(
    client: &reqwest::Client,
    id: u64,
    input: ModelRequest,
    key: &str,
    base: &str,
) -> krabb_core::Result<Option<Response>> {
    reply(client, id, input, key, base, None).await
}

async fn stream(
    client: &reqwest::Client,
    id: u64,
    input: ModelRequest,
    key: &str,
    base: &str,
    emitter: &mut Emitter,
) -> krabb_core::Result<Option<Response>> {
    reply(client, id, input, key, base, Some(emitter)).await
}

async fn reply(
    client: &reqwest::Client,
    id: u64,
    input: ModelRequest,
    key: &str,
    base: &str,
    emitter: Option<&mut Emitter>,
) -> krabb_core::Result<Option<Response>> {
    let model = model::resolve(&input.model, None);

    if input
        .generation
        .is_some_and(|value| value.search || (value.summary && krabb_openai::search(&model)))
    {
        let api = krabb_openai::responses::Api { client, key, base, provider: "openai" };

        return api.generate(id, &input, &model, emitter).await;
    }

    let body = request_body(&input, emitter.is_some())?;
    let api = chat::Api { client, key: Some(key), base, provider: PROVIDER };

    api.generate(id, &body, emitter).await
}

fn credential() -> krabb_core::Result<String> {
    if let Ok(value) = std::env::var("KRABB_OPENAI_KEY")
        && !value.trim().is_empty()
    {
        return Ok(value);
    }

    if std::env::var("KRABB_KEYRING").ok().as_deref() == Some("1")
        && let Ok(entry) = keyring::Entry::new("it.airscript.krabb", "openai")
        && let Ok(value) = entry.get_password()
        && !value.trim().is_empty()
    {
        return Ok(value);
    }

    let Some(path) = std::env::var_os("KRABB_CREDENTIALS") else {
        return Err(krabb_core::Error::Denied("KRABB_OPENAI_KEY is not configured.".into()));
    };

    krabb_file::private(&path)?;

    let text = std::fs::read_to_string(path).map_err(|error| {
        krabb_core::Error::Denied(format!("Credentials could not be read: {error}."))
    })?;

    let value: Value = serde_json::from_str(&text)
        .map_err(|error| krabb_core::Error::Denied(format!("Credentials are invalid: {error}.")))?;

    value["KRABB_OPENAI_KEY"]
        .as_str()
        .filter(|key| !key.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| krabb_core::Error::Denied("KRABB_OPENAI_KEY is not configured.".into()))
}

#[cfg(test)]
mod tests {
    mod streaming;

    use super::{PROVIDER, base_url, credential, generate, validate_base_url};

    use krabb_openai::chat::{
        BODY_LIMIT, TOOL_CALLS, collect, image, messages, response, response_body, stream_body,
        stream_line, tools,
    };

    use futures_util::stream;
    use krabb_core::types::{Content, Message, ModelRequest, Request, Role, ToolSpec};
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn loopback_listener() -> Option<tokio::net::TcpListener> {
        match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => Some(listener),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => None,
            Err(error) => panic!("Could not bind the OpenAI test listener: {error}."),
        }
    }

    fn model() -> ModelRequest {
        ModelRequest {
            generation: None,
            model: "test/model".into(),
            workspace: None,
            messages: vec![
                Message {
                    id: "system".into(),
                    session: "session".into(),
                    role: Role::System,
                    sender: None,
                    content: vec![Content::Text { text: "rules".into() }],
                },
                Message {
                    id: "user".into(),
                    session: "session".into(),
                    role: Role::User,
                    sender: None,
                    content: vec![
                        Content::Text { text: "hello".into() },
                        Content::Image { uri: "https://cdn.example/image.png".into(), alt: None },
                    ],
                },
            ],
            stream: false,
            tools: vec![ToolSpec {
                name: "read".into(),
                description: Some("Read a file.".into()),
                schema: json!({"type":"object"}),
            }],
        }
    }

    #[test]
    fn builds_messages_tools_and_images() {
        let input = model();

        let values = messages(PROVIDER, &input).unwrap();

        assert_eq!(values[0], json!({"role":"system","content":"rules"}));
        assert_eq!(values[1]["content"][1]["type"], "image_url");
        assert_eq!(tools(&input).len(), 1);
        assert!(image(PROVIDER, "file:///outside").is_err());
        assert!(image(PROVIDER, "data:image/png;base64,abc").is_ok());

        let mut tool = input.clone();

        tool.messages.push(Message {
            id: "tool".into(),
            session: "session".into(),
            role: Role::Tool,
            sender: None,
            content: vec![Content::Text { text: "result".into() }],
        });

        assert_eq!(messages(PROVIDER, &tool).unwrap().last().unwrap()["role"], "user");

        let mut catalog = input;

        catalog.tools = (0..TOOL_CALLS + 2)
            .map(|index| ToolSpec {
                name: format!("tool-{index}"),
                description: None,
                schema: json!({"type":"object"}),
            })
            .collect();

        let definitions = tools(&catalog);

        assert_eq!(definitions.len(), catalog.tools.len());

        for (definition, tool) in definitions.iter().zip(&catalog.tools) {
            assert_eq!(definition["function"]["name"], tool.name);
            assert_eq!(definition["function"]["parameters"], tool.schema);
        }
    }

    #[test]
    fn maps_assistant_and_mixed_content_for_the_provider() {
        let mut input = model();

        input.messages = vec![
            Message {
                id: "assistant".into(),
                session: "session".into(),
                role: Role::Assistant,
                sender: None,
                content: vec![Content::Text { text: "answer".into() }],
            },
            Message {
                id: "system-image".into(),
                session: "session".into(),
                role: Role::System,
                sender: None,
                content: vec![Content::Image {
                    uri: "file:///not-forwarded".into(),
                    alt: Some("diagram".into()),
                }],
            },
            Message {
                id: "attachments".into(),
                session: "session".into(),
                role: Role::User,
                sender: None,
                content: vec![
                    Content::File {
                        uri: "file:///notes.txt".into(),
                        name: "notes.txt".into(),
                        mime: Some("text/plain".into()),
                    },
                    Content::Audio { uri: "file:///voice.ogg".into(), mime: None },
                    Content::Image { uri: "https://cdn.example/attachment.png".into(), alt: None },
                ],
            },
        ];

        let values = messages(PROVIDER, &input).unwrap();

        assert_eq!(values[0], json!({"role":"assistant","content":"answer"}));
        assert_eq!(values[1]["content"], "diagram");

        assert_eq!(
            values[2]["content"],
            json!([
                {"type":"text","text":"[File: notes.txt]"},
                {"type":"text","text":"[Audio attachment.]"},
                {"type":"image_url","image_url":{"url":"https://cdn.example/attachment.png"}}
            ])
        );

        input.messages[2].content[2] = Content::Image { uri: "file:///bad.png".into(), alt: None };

        assert!(messages(PROVIDER, &input).is_err());

        input.messages[2].content[0] = Content::ToolCall {
            name: "read".into(),
            args: json!({"path":"README.md"}),
            id: None,
            thought_signature: None,
        };

        input.messages[2].content[2] =
            Content::Image { uri: "https://cdn.example/attachment.png".into(), alt: None };

        assert!(
            messages(PROVIDER, &input).unwrap()[2]["content"]
                .to_string()
                .contains("[Tool call read]")
        );
    }

    #[test]
    fn bounds_and_validates_tool_definitions_and_image_references() {
        let mut input = model();

        input.tools[0].description = None;

        assert_eq!(tools(&input)[0]["function"]["description"], Value::Null);
        assert!(image(PROVIDER, "data:text/plain;base64,abc").is_err());
        assert!(image(PROVIDER, "https://cdn.example/image.png").is_ok());
    }

    #[tokio::test]
    async fn ignores_notes_and_unknown_calls_without_provider_access() {
        let (output, _) = tokio::sync::mpsc::channel(1);
        let emitter = krabb_core::plugin::Emitter::new(output);

        assert!(
            generate(
                &reqwest::Client::new(),
                Request::Note { jsonrpc: "2.0".into(), method: "note".into(), params: json!({}) },
                emitter,
            )
            .await
            .unwrap()
            .is_none()
        );

        let (output, _) = tokio::sync::mpsc::channel(1);
        let emitter = krabb_core::plugin::Emitter::new(output);

        assert!(
            generate(&reqwest::Client::new(), Request::call(1, "other", json!({})), emitter,)
                .await
                .unwrap()
                .is_none()
        );

        let (output, _) = tokio::sync::mpsc::channel(1);
        let emitter = krabb_core::plugin::Emitter::new(output);

        assert!(
            generate(&reqwest::Client::new(), Request::call(1, "generate", json!({})), emitter,)
                .await
                .is_err()
        );
    }

    #[test]
    fn validates_provider_configuration_and_protocol_size() {
        assert!(base_url().is_ok());

        assert_eq!(
            validate_base_url("http://localhost:8080/v1").unwrap(),
            "http://localhost:8080/v1"
        );

        assert_eq!(validate_base_url("https://api.example/v1/").unwrap(), "https://api.example/v1");
        assert!(validate_base_url("not a URL").is_err());
        assert!(validate_base_url("http://api.example/v1").is_err());
        assert!(credential().is_err());
        assert!(response(PROVIDER, 1, json!({"ok": true})).unwrap().is_some());
        assert!(response_body(PROVIDER, 1, json!({"choices": []})).is_err());
    }

    #[test]
    fn parses_complete_and_tool_replies() {
        let response = response_body(
            PROVIDER,
            1,
            json!({"choices":[{"message":{"content":"done","tool_calls":[{"function":{"name":"read","arguments":"{\"path\":\"note\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":3}}),
        ).unwrap().unwrap();

        assert_eq!(response.result.as_ref().unwrap()["text"], "done");
        assert_eq!(response.result.as_ref().unwrap()["events"][0]["name"], "read");
        assert!(response_body(PROVIDER, 1, json!({})).is_err());

        assert!(response_body(
            PROVIDER,
            1,
            json!({"choices":[{"message":{"tool_calls":[{"function":{"name":"read","arguments":"not-json"}}]}}]}),
        ).is_err());

        assert!(response_body(PROVIDER, 1, json!({"choices":[{"message":{}}]})).is_err());

        let default_stop =
            response_body(PROVIDER, 2, json!({"choices":[{"message":{"content":"ok"}}]}))
                .unwrap()
                .unwrap();

        assert_eq!(default_stop.result.unwrap()["stop"], "stop");
    }

    #[tokio::test]
    async fn handles_sparse_and_malformed_stream_deltas() {
        let mut text = String::new();
        let mut pending = String::new();
        let mut calls = std::collections::BTreeMap::new();

        assert!(
            !stream_line(PROVIDER, b"event: ping\n", &mut text, &mut pending, &mut calls).unwrap()
        );

        assert!(
            !stream_line(PROVIDER, b"data: {}\n", &mut text, &mut pending, &mut calls).unwrap()
        );

        assert!(stream_line(
            PROVIDER,
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{"}}]}}]}"#,
            &mut text,
            &mut pending,
            &mut calls,
        )
        .is_ok());

        let (output, _) = tokio::sync::mpsc::channel(1);
        let mut emitter = krabb_core::plugin::Emitter::new(output);

        let incomplete = stream::iter(vec![Ok::<_, std::io::Error>(
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{"}}]}}]}
data: [DONE]
"#.to_vec(),
        )]);

        assert!(stream_body(PROVIDER, 3, incomplete, &mut emitter).await.is_err());
    }

    #[tokio::test]
    async fn completes_streams_beyond_the_notification_budget() {
        for (fragment, count) in [("x".to_owned(), 600), ("x".repeat(128), 300)] {
            let line = format!("data: {}\n", json!({"choices":[{"delta":{"content":fragment}}]}));

            let chunks = stream::iter(
                (0..count)
                    .map(|_| Ok::<_, std::io::Error>(line.as_bytes().to_vec()))
                    .chain(std::iter::once(Ok(b"data: [DONE]\n".to_vec()))),
            );

            let (output, mut events) = tokio::sync::mpsc::channel(256);
            let mut emitter = krabb_core::plugin::Emitter::new(output);
            let response = stream_body(PROVIDER, 2, chunks, &mut emitter).await.unwrap().unwrap();
            let expected = fragment.repeat(count);

            assert_eq!(response.result.unwrap()["text"], expected);

            let mut notifications = 0;
            let mut streamed = String::new();

            while let Ok(event) = events.try_recv() {
                notifications += 1;
                streamed.push_str(event["params"]["event"]["text"].as_str().unwrap());
            }

            assert!(notifications > 0 && notifications <= 256);
            assert!(notifications < count);
            assert!(expected.starts_with(&streamed));

            if fragment.len() == 1 {
                assert_eq!(streamed, expected);
            } else {
                assert_eq!(notifications, 256);
                assert!(streamed.len() < expected.len());
            }
        }
    }

    #[tokio::test]
    async fn bounds_and_parses_streams() {
        let chunks = stream::iter(vec![
            Ok::<_, std::io::Error>(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n".to_vec(),
            ),
            Ok(b"data: [DONE]\n".to_vec()),
        ]);

        let (output, mut events) = tokio::sync::mpsc::channel(4);
        let mut emitter = krabb_core::plugin::Emitter::new(output);
        let response = stream_body(PROVIDER, 2, chunks, &mut emitter).await.unwrap().unwrap();

        assert_eq!(response.result.unwrap()["text"], "ok");
        assert_eq!(events.recv().await.unwrap()["params"]["event"]["text"], "ok");

        let mut text = String::new();
        let mut pending = String::new();
        let mut calls = std::collections::BTreeMap::new();

        assert!(
            stream_line(PROVIDER, b"data: [DONE]\n", &mut text, &mut pending, &mut calls).unwrap()
        );

        assert!(
            stream_line(PROVIDER, b"data: nope\n", &mut text, &mut pending, &mut calls).is_err()
        );

        let chunks = stream::iter(vec![Ok::<_, std::io::Error>(vec![b'x'; BODY_LIMIT + 1])]);

        assert!(collect(PROVIDER, chunks).await.is_err());

        let mut text = String::new();
        let mut pending = String::new();
        let mut calls = std::collections::BTreeMap::new();

        assert!(stream_line(
            PROVIDER,
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"read","arguments":"{\"path\":\"note\"}"}}]}}]}"#,
            &mut text,
            &mut pending,
            &mut calls,
        )
        .is_ok());

        assert_eq!(calls[&0].0, "read");

        let tool_delta = br#"data: {"choices":[{"delta":{"content":"!","tool_calls":[{"index":0,"function":{"name":"File","arguments":"}"}}]}}]}"#;

        assert!(stream_line(PROVIDER, tool_delta, &mut text, &mut pending, &mut calls).is_ok());
        assert_eq!(text, "!");
        assert_eq!(calls[&0].0, "readFile");
        assert!(stream_line(PROVIDER, b"invalid", &mut text, &mut pending, &mut calls).is_ok());
        assert!(stream_line(PROVIDER, &[0xff], &mut text, &mut pending, &mut calls).is_err());
        assert!(stream_line(PROVIDER, b"data: {", &mut text, &mut pending, &mut calls).is_err());

        assert!(
            stream_line(
                PROVIDER,
                br#"data: {"choices":[{"delta":{"tool_calls":[{"index":16}]}}]}"#,
                &mut text,
                &mut pending,
                &mut calls,
            )
            .is_err()
        );

        let (output, _) = tokio::sync::mpsc::channel(1);
        let mut emitter = krabb_core::plugin::Emitter::new(output);
        let chunks = stream::iter(vec![Ok::<_, std::io::Error>(b"data: nope\n".to_vec())]);

        assert!(stream_body(PROVIDER, 2, chunks, &mut emitter).await.is_err());

        let (output, _) = tokio::sync::mpsc::channel(4);
        let mut emitter = krabb_core::plugin::Emitter::new(output);

        let chunks = stream::iter(vec![Ok::<_, std::io::Error>(
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"read","arguments":"{\"path\":\"x\"}"}}]}}]}
data: [DONE]
"#.to_vec(),
        )]);

        let response = stream_body(PROVIDER, 3, chunks, &mut emitter).await.unwrap().unwrap();

        assert_eq!(response.result.unwrap()["events"][0]["name"], "read");
    }

    #[tokio::test]
    async fn completes_against_a_bounded_http_response() {
        let Some(listener) = loopback_listener().await else {
            return;
        };

        let address = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            let body = br#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}"#;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
        });

        let result = super::complete(
            &reqwest::Client::new(),
            1,
            model(),
            "key",
            &format!("http://{address}"),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(result.result.unwrap()["text"], "done");

        server.await.unwrap();
    }

    #[tokio::test]
    async fn streams_against_a_bounded_http_response() {
        let Some(listener) = loopback_listener().await else {
            return;
        };

        let address = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            let body =
                b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
        });

        let (output, mut events) = tokio::sync::mpsc::channel(4);
        let mut emitter = krabb_core::plugin::Emitter::new(output);
        let mut input = model();

        input.stream = true;

        let response = super::stream(
            &reqwest::Client::new(),
            1,
            input,
            "key",
            &format!("http://{address}"),
            &mut emitter,
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(response.result.unwrap()["text"], "ok");
        assert_eq!(events.recv().await.unwrap()["params"]["event"]["text"], "ok");

        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_provider_failures_and_oversized_results() {
        let Some(listener) = loopback_listener().await else {
            return;
        };

        let address = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request).await.unwrap();
                let body = br#"{"error":{"message":"nope"}}"#;

                let header = format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );

                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(body).await.unwrap();
            }
        });

        let client = reqwest::Client::new();
        let base = format!("http://{address}");

        assert!(super::complete(&client, 1, model(), "key", &base).await.is_err());

        let (output, _) = tokio::sync::mpsc::channel(1);
        let mut emitter = krabb_core::plugin::Emitter::new(output);
        let mut input = model();

        input.stream = true;

        assert!(super::stream(&client, 2, input, "key", &base, &mut emitter).await.is_err());

        assert!(collect(PROVIDER, stream::iter(vec![Err::<Vec<u8>, _>("broken")])).await.is_err());

        assert!(
            response(PROVIDER, 1, json!({"payload": "x".repeat(krabb_core::jsonl::MAX)})).is_err()
        );

        server.await.unwrap();
    }

    #[test]
    fn renders_non_image_content_and_rejects_invalid_configuration() {
        let mut input = model();

        input.messages[1].content = vec![
            Content::Image {
                uri: "https://cdn.example/image.png".into(),
                alt: Some("diagram".into()),
            },
            Content::ToolCall {
                name: "read".into(),
                args: json!({"path": "README.md"}),
                id: Some("call-1".into()),
                thought_signature: None,
            },
        ];

        let values = messages(PROVIDER, &input).unwrap();

        assert_eq!(values[1]["content"][0]["type"], "image_url");
        assert_eq!(values[1]["content"][1]["text"], "[Tool call read]: {\"path\":\"README.md\"}");
        assert!(base_url().is_ok());
        assert!(credential().is_err());
    }
}

#[cfg(test)]
#[path = "tests/hardening.rs"]
mod hardening;

#[cfg(test)]
mod defaults;
