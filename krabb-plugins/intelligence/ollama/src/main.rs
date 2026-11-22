#![forbid(unsafe_code)]

mod context;
mod execution;
mod failure;
mod model;

#[cfg(test)]
use krabb_log as _;

use krabb_core::{
    plugin::Emitter,
    types::{Content, Event, ModelReply, ModelRequest, Request, Response},
};

#[cfg(not(test))]
use krabb_core::{
    plugin::serve_events_concurrent,
    types::{Capability, Hello, Protocol},
};

use futures_util::{Stream, StreamExt};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

const BODY_LIMIT: usize = krabb_core::jsonl::MAX / 2;

#[cfg(not(test))]
#[tokio::main]
async fn main() -> std::process::ExitCode {
    krabb_log::run_plugin("ollama", plugin_main()).await
}

#[cfg(not(test))]
async fn plugin_main() -> krabb_core::Result<()> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| krabb_core::Error::Denied(format!("Ollama client failed: {error}.")))?;

    serve_events_concurrent(
        Hello {
            formats: vec![],
            protocol: Protocol { major: 0, minor: 1 },
            id: "ollama".into(),
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

        Request::Note { method, params, .. } => {
            if method == krabb_core::execution::RENEW {
                execution::renew(&params)?;
            }

            return Ok(None);
        }
    };

    if method == krabb_core::execution::INFO {
        let host =
            std::env::var("KRABB_OLLAMA_HOST").unwrap_or_else(|_| "https://ollama.com".into());

        let profile = execution::profile(
            &host,
            &model::resolve(params["model"].as_str().unwrap_or("default"), &host),
            &std::env::var("KRABB_OLLAMA_DEPLOYMENT").unwrap_or_default(),
            &std::env::var("KRABB_OLLAMA_REVISION").unwrap_or_default(),
            &std::env::var("KRABB_OLLAMA_THINK").unwrap_or_default(),
            &std::env::var("KRABB_OLLAMA_CONTEXT_LENGTH").unwrap_or_default(),
        )?;

        return Ok(Some(Response::ok(id, serde_json::to_value(profile)?)));
    }

    if method == "model.info" {
        let host =
            std::env::var("KRABB_OLLAMA_HOST").unwrap_or_else(|_| "https://ollama.com".into());

        let model = model::resolve(params["model"].as_str().unwrap_or("default"), &host);

        let profile = context::profile(
            client,
            &model,
            &host,
            &std::env::var("KRABB_OLLAMA_CONTEXT_LENGTH").unwrap_or_default(),
            credential().as_deref(),
        )
        .await?;

        return Ok(Some(Response::ok(id, serde_json::to_value(profile)?)));
    }

    if method != "generate" {
        return Ok(None);
    }

    let input: ModelRequest = serde_json::from_value(params)?;

    let host = std::env::var("KRABB_OLLAMA_HOST").unwrap_or_else(|_| "https://ollama.com".into());

    generate_at(client, id, input, &host, &mut emitter).await
}

async fn generate_at(
    client: &reqwest::Client,
    id: u64,
    input: ModelRequest,
    host: &str,
    emitter: &mut Emitter,
) -> krabb_core::Result<Option<Response>> {
    let limits = execution::limits(&input)?;
    let stream = input.stream;

    // Only a stream shows the progress that earns renewals, so only it watches for them.
    let watch =
        limits.filter(|_| stream).map(|limits| execution::Watch::new(id, limits)).transpose()?;

    let call = async {
        if stream {
            stream_request(client, id, input, host, emitter, watch).await
        } else {
            let request = generate_request(client, id, input, host);

            let Some(limits) = limits else { return request.await };

            // A plain reply's first output is the whole answer, so host admission sets its
            // first-output limit to the total for non-streaming work.
            tokio::time::timeout(Duration::from_millis(limits.first_millis), request)
                .await
                .map_err(|_| failure::timeout(krabb_core::execution::Expiry::First.to_string()))?
        }
    };

    let Some(limits) = limits else { return call.await };

    let bound = if stream {
        limits.ceiling_millis.unwrap_or(limits.total_millis)
    } else {
        limits.total_millis
    };

    tokio::time::timeout(Duration::from_millis(bound), call)
        .await
        .map_err(|_| failure::timeout(krabb_core::execution::Expiry::Total.to_string()))?
}

fn request_body(
    input: &ModelRequest,
    host: &str,
    stream: bool,
) -> krabb_core::Result<serde_json::Value> {
    let mut body = json!({
        "model": model::resolve(&input.model, host),
        "messages": messages(input)?,
        "stream": stream,
        "tools": tools(input),
    });

    model::configure(
        &mut body,
        &std::env::var("KRABB_OLLAMA_THINK").unwrap_or_default(),
        &std::env::var("KRABB_OLLAMA_CONTEXT_LENGTH").unwrap_or_default(),
    )?;

    if let Some(generation) = input.generation {
        body["options"]["num_predict"] = generation.output.into();

        if generation.summary {
            body["think"] = false.into();
            body["tools"] = json!([]);
        }
    }

    Ok(body)
}

async fn generate_request(
    client: &reqwest::Client,
    id: u64,
    input: ModelRequest,
    host: &str,
) -> krabb_core::Result<Option<Response>> {
    request(client, id, input, host, false).await
}

async fn request(
    client: &reqwest::Client,
    id: u64,
    input: ModelRequest,
    host: &str,
    stream: bool,
) -> krabb_core::Result<Option<Response>> {
    let key = credential();
    let host = validate_host(host, key.is_some())?;
    let limits = execution::limits(&input)?;
    let mut post = execution::request(client.post(format!("{host}/api/chat")), limits);

    if let Some(key) = key {
        post = post.bearer_auth(key);
    }

    let response = post
        .json(&request_body(&input, &host, stream)?)
        .send()
        .await
        .map_err(|error| krabb_http::transport(&error, "Ollama"))?;

    if !response.status().is_success() {
        return Err(krabb_http::rejected(response, "Ollama").await);
    }

    let body = read(response, "Ollama").await?;

    if stream {
        return stream_body(id, &body);
    }

    let body: serde_json::Value = serde_json::from_str(&body)
        .map_err(|error| krabb_core::Error::Denied(format!("Ollama response failed: {error}.")))?;

    response_body(id, body)
}

async fn read(response: reqwest::Response, provider: &str) -> krabb_core::Result<String> {
    collect(response.bytes_stream(), provider).await
}

async fn collect<S, C, E>(mut stream: S, provider: &str) -> krabb_core::Result<String>
where
    S: Stream<Item = Result<C, E>> + Unpin,
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut bytes = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            krabb_core::failure::Failure::new(krabb_core::failure::Kind::Transient)
                .error(format!("{provider} response failed: {error}."))
        })?;

        if chunk.as_ref().len() > BODY_LIMIT.saturating_sub(bytes.len()) {
            return Err(krabb_core::Error::Denied(format!(
                "{provider} response exceeded the {BODY_LIMIT}-byte limit."
            )));
        }

        bytes.extend_from_slice(chunk.as_ref());
    }

    String::from_utf8(bytes).map_err(|error| {
        krabb_core::Error::Denied(format!("{provider} response was not UTF-8: {error}."))
    })
}

#[derive(Default)]
struct ToolDelta {
    name: String,
    arguments: serde_json::Value,
}

#[cfg(test)]
async fn live_stream<S, C, E>(
    id: u64,
    stream: S,
    emitter: &mut Emitter,
) -> krabb_core::Result<Option<Response>>
where
    S: Stream<Item = Result<C, E>> + Unpin,
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    live_stream_with_watch(id, stream, emitter, None).await
}

async fn live_stream_with_watch<S, C, E>(
    id: u64,
    mut stream: S,
    emitter: &mut Emitter,
    mut watch: Option<execution::Watch>,
) -> krabb_core::Result<Option<Response>>
where
    S: Stream<Item = Result<C, E>> + Unpin,
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut buffered = Vec::new();
    let mut bytes = 0_usize;
    let mut text = String::new();
    let mut pending = String::new();
    let mut emitted = 0_usize;
    let mut complete = false;
    let mut tools = BTreeMap::<usize, ToolDelta>::new();
    let mut stop = "stop".to_string();
    let mut timing = None;
    let mut input_tokens = None;
    let mut output_tokens = None;
    let mut ticker = tokio::time::interval(Duration::from_millis(80));
    ticker.tick().await;

    'stream: loop {
        let deadline = watch.as_ref().map(execution::Watch::deadline).unwrap_or_else(|| {
            tokio::time::Instant::now() + Duration::from_millis(krabb_core::execution::MAX_MILLIS)
        });

        tokio::select! {
            biased;

            _ = tokio::time::sleep_until(deadline), if watch.is_some() => {
                watch.as_mut().unwrap().observe(false)?;
            }

            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };

                let chunk = chunk.map_err(|error| {
                    krabb_core::failure::Failure::new(krabb_core::failure::Kind::interrupted(emitted))
                        .error(format!("Ollama stream failed: {error}."))
                })?;

                if chunk.as_ref().len() > BODY_LIMIT.saturating_sub(bytes) {
                    return Err(krabb_core::Error::Denied(format!(
                        "Ollama stream exceeded the {BODY_LIMIT}-byte limit."
                    )));
                }

                bytes = bytes.saturating_add(chunk.as_ref().len());
                buffered.extend_from_slice(chunk.as_ref());

                while let Some(end) = buffered.iter().position(|byte| *byte == b'\n') {
                    let line = buffered.drain(..=end).collect::<Vec<_>>();

                    let (finished, progress, reported) = ollama_line(
                        &line,
                        &mut text,
                        &mut pending,
                        &mut tools,
                        &mut stop,
                        &mut input_tokens,
                        &mut output_tokens,
                    )?;

                    timing = reported.or(timing);

                    if let Some(watch) = &mut watch { watch.observe(progress)?; }

                    if finished {
                        complete = true;
                        buffered.clear();
                        break 'stream;
                    }

                    if pending.len() >= 128 {
                        emit(&mut pending, &mut emitted, emitter).await?;
                    }
                }
            }

            _ = ticker.tick(), if !pending.is_empty() => {
                emit(&mut pending, &mut emitted, emitter).await?;
            }
        }
    }

    if !buffered.is_empty() {
        let (finished, progress, reported) = ollama_line(
            &buffered,
            &mut text,
            &mut pending,
            &mut tools,
            &mut stop,
            &mut input_tokens,
            &mut output_tokens,
        )?;

        timing = reported.or(timing);

        if let Some(watch) = &mut watch {
            watch.observe(progress)?;
        }

        complete = finished;
    }

    if !complete {
        return Err(krabb_core::Error::Denied(
            "Ollama stream ended before its completion marker.".into(),
        ));
    }

    emit(&mut pending, &mut emitted, emitter).await?;

    let events = tools
        .into_values()
        .map(|tool| {
            if tool.name.is_empty() {
                return Err(krabb_core::Error::Denied(
                    "Ollama stream returned a tool call without a name.".into(),
                ));
            }

            let arguments = if let Some(value) = tool.arguments.as_str() {
                serde_json::from_str(value).map_err(|error| {
                    krabb_core::Error::Denied(format!(
                        "Ollama stream returned invalid tool arguments: {error}."
                    ))
                })?
            } else {
                tool.arguments
            };

            Ok(Event::Tool { name: tool.name, args: arguments, id: None, thought_signature: None })
        })
        .collect::<krabb_core::Result<Vec<_>>>()?;

    if text.trim().is_empty() && events.is_empty() {
        return Err(krabb_core::Error::Denied(
            "Ollama stream contained no text or tools, so check the context and thinking settings."
                .into(),
        ));
    }

    response(
        id,
        serde_json::to_value(ModelReply {
            timing,
            text,
            stop,
            input: input_tokens,
            output: output_tokens,
            context_usage: None,
            events,
        })?,
    )
}

fn ollama_line(
    line: &[u8],
    text: &mut String,
    pending: &mut String,
    tools: &mut BTreeMap<usize, ToolDelta>,
    stop: &mut String,
    input_tokens: &mut Option<u64>,
    output_tokens: &mut Option<u64>,
) -> krabb_core::Result<(bool, bool, Option<krabb_core::execution::Timing>)> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok((false, false, None));
    }

    let line = std::str::from_utf8(line).map_err(|error| {
        krabb_core::Error::Denied(format!("Ollama stream was not UTF-8: {error}."))
    })?;

    let value: serde_json::Value = serde_json::from_str(line.trim())?;

    if let Some(error) = value["error"].as_str() {
        return Err(krabb_core::Error::Denied(format!("Ollama stream failed: {error}.")));
    }

    let mut progress = value["message"]["thinking"].as_str().is_some_and(|text| !text.is_empty());

    if let Some(part) = value["message"]["content"].as_str() {
        append_text(part, text, pending)?;
        progress |= !part.is_empty();
    }

    if let Some(calls) = value["message"]["tool_calls"].as_array() {
        if calls.len() > 16 {
            return Err(krabb_core::Error::Limit(
                "Ollama stream exceeded the tool-call limit.".into(),
            ));
        }

        for (position, call) in calls.iter().enumerate() {
            let index = call["index"]
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .unwrap_or(position);

            if index >= 16 {
                return Err(krabb_core::Error::Limit(
                    "Ollama stream exceeded the tool-call limit.".into(),
                ));
            }

            let tool = tools.entry(index).or_default();

            if let Some(name) = call["function"]["name"].as_str() {
                progress |= tool.name != name;
                tool.name = name.into();
            }

            if !call["function"]["arguments"].is_null() {
                progress |= tool.arguments != call["function"]["arguments"];
                tool.arguments = call["function"]["arguments"].clone();
            }
        }
    }

    if let Some(reason) = value["done_reason"].as_str() {
        *stop = reason.into();
    }

    *input_tokens = value["prompt_eval_count"].as_u64().or(*input_tokens);
    *output_tokens = value["eval_count"].as_u64().or(*output_tokens);
    let done = value["done"] == true;
    let timing = if done { execution::timing(&value)? } else { None };

    Ok((done, progress, timing))
}

fn append_text(part: &str, text: &mut String, pending: &mut String) -> krabb_core::Result<()> {
    if part.len() > BODY_LIMIT.saturating_sub(text.len()) {
        return Err(krabb_core::Error::Limit("Ollama stream exceeded the response limit.".into()));
    }

    text.push_str(part);
    pending.push_str(part);
    Ok(())
}

async fn emit(
    pending: &mut String,
    emitted: &mut usize,
    emitter: &mut Emitter,
) -> krabb_core::Result<()> {
    if !pending.is_empty() && *emitted < 256 {
        emitter.event(json!({"kind": "text", "text": std::mem::take(pending)})).await?;
        *emitted += 1;
    }

    pending.clear();
    Ok(())
}

fn validate_host(host: &str, authenticated: bool) -> krabb_core::Result<String> {
    let url = reqwest::Url::parse(host)
        .map_err(|_| krabb_core::Error::Denied("Ollama host is invalid.".into()))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));

    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || authenticated && url.scheme() != "https" && !local
    {
        return Err(krabb_core::Error::Denied("Ollama credentials require HTTPS outside loopback, and the host cannot embed credentials, a query, or a fragment.".into()));
    }

    Ok(url.as_str().trim_end_matches('/').into())
}

fn credential() -> Option<String> {
    std::env::var("KRABB_OLLAMA_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| keyring("ollama"))
        .or_else(|| {
            let path = std::env::var_os("KRABB_CREDENTIALS")?;
            krabb_file::private(&path).ok()?;
            let text = std::fs::read_to_string(path).ok()?;
            let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
            value["KRABB_OLLAMA_API_KEY"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        })
}

fn keyring(name: &str) -> Option<String> {
    if std::env::var("KRABB_KEYRING").ok().as_deref() != Some("1") {
        return None;
    }

    keyring::Entry::new("it.airscript.krabb", name)
        .ok()?
        .get_password()
        .ok()
        .filter(|value| !value.trim().is_empty())
}

async fn stream_request(
    client: &reqwest::Client,
    id: u64,
    input: ModelRequest,
    host: &str,
    emitter: &mut Emitter,
    mut watch: Option<execution::Watch>,
) -> krabb_core::Result<Option<Response>> {
    let key = credential();
    let host = validate_host(host, key.is_some())?;
    let limits = execution::limits(&input)?;
    let mut post = execution::request(client.post(format!("{host}/api/chat")), limits);

    if let Some(key) = key {
        post = post.bearer_auth(key);
    }

    if let Some(watch) = &mut watch {
        watch.observe(false)?;
    }

    let pending = post.json(&request_body(&input, &host, true)?).send();
    let response = if let Some(watch) = &mut watch {
        tokio::pin!(pending);

        loop {
            match tokio::time::timeout_at(watch.deadline(), &mut pending).await {
                Ok(response) => break response,

                Err(_) => watch.observe(false)?,
            }
        }
    } else {
        pending.await
    }
    .map_err(|error| krabb_http::transport(&error, "Ollama"))?;

    let status = response.status();

    if !status.is_success() {
        return Err(krabb_http::rejected(response, "Ollama").await);
    }

    live_stream_with_watch(id, response.bytes_stream(), emitter, watch).await
}

fn messages(input: &ModelRequest) -> krabb_core::Result<Vec<serde_json::Value>> {
    input
        .messages
        .iter()
        .map(|message| {
            let role = match message.role {
                krabb_core::types::Role::System => "system",
                krabb_core::types::Role::User => "user",
                krabb_core::types::Role::Assistant => "assistant",
                krabb_core::types::Role::Tool => "tool",
            };

            let mut text = Vec::new();
            let mut images = Vec::new();
            let mut calls = Vec::new();

            for content in &message.content {
                match content {
                    Content::Text { text: value } => text.push(value.clone()),
                    Content::Image { uri, .. } => images.push(image_data(uri)?),
                    Content::ToolCall { name, args, .. }
                        if message.role == krabb_core::types::Role::Assistant =>
                    {
                        calls.push(json!({"function": {"name": name, "arguments": args}}));
                    }

                    Content::Context { .. } => {}
                    content => text.push(content.render()),
                }
            }

            let mut value = json!({"role": role, "content": text.join("\n")});

            if !calls.is_empty() {
                value["tool_calls"] = json!(calls);
            }

            if message.role == krabb_core::types::Role::Tool
                && let Some(name) = &message.sender
            {
                value["tool_name"] = json!(name);
            }

            if !images.is_empty() {
                value["images"] = json!(images);
            }

            Ok(value)
        })
        .collect()
}

fn image_data(uri: &str) -> krabb_core::Result<&str> {
    let Some((mime, encoded)) =
        uri.strip_prefix("data:").and_then(|value| value.split_once(";base64,"))
    else {
        return Err(krabb_core::Error::Denied(
            "Ollama received an unsupported image reference.".into(),
        ));
    };

    if !matches!(mime, "image/png" | "image/jpeg" | "image/gif" | "image/webp")
        || encoded.is_empty()
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
    {
        return Err(krabb_core::Error::Denied("Ollama received an invalid image data URL.".into()));
    }

    Ok(encoded)
}

fn tools(input: &ModelRequest) -> Vec<serde_json::Value> {
    input
        .tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.schema,
                }
            })
        })
        .collect()
}

fn stream_body(id: u64, body: &str) -> krabb_core::Result<Option<Response>> {
    let mut text = String::new();
    let mut stop = "stream".to_string();

    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let value: serde_json::Value = serde_json::from_str(line.trim())?;

        if let Some(part) = value["message"]["content"].as_str() {
            text.push_str(part);
        }

        if let Some(reason) = value["done_reason"].as_str() {
            stop = reason.into();
        }
    }

    response(
        id,
        serde_json::to_value(ModelReply {
            timing: None,
            text,
            stop,
            input: None,
            output: None,
            context_usage: None,
            events: Vec::new(),
        })?,
    )
}

fn response_body(id: u64, body: serde_json::Value) -> krabb_core::Result<Option<Response>> {
    let text = body["message"]["content"].as_str().unwrap_or_default().to_string();
    let events = body["message"]["tool_calls"]
        .as_array()
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| {
                    let name = call["function"]["name"].as_str()?.to_owned();
                    Some(krabb_core::types::Event::Tool {
                        name,
                        args: call["function"]["arguments"].clone(),
                        id: None,
                        thought_signature: None,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if text.is_empty() && events.is_empty() {
        return Err(krabb_core::Error::Denied(
            "Ollama response contained no text or tools.".into(),
        ));
    }

    response(
        id,
        serde_json::to_value(ModelReply {
            timing: execution::timing(&body)?,
            text,
            stop: body["done_reason"].as_str().unwrap_or("stop").into(),
            input: body["prompt_eval_count"].as_u64(),
            output: body["eval_count"].as_u64(),
            context_usage: None,
            events,
        })?,
    )
}

fn response(id: u64, result: serde_json::Value) -> krabb_core::Result<Option<Response>> {
    let response = Response::ok(id, result);

    if serde_json::to_vec(&response)?.len().saturating_add(1) > krabb_core::jsonl::MAX {
        return Err(krabb_core::Error::Denied(
            "Ollama response exceeds the protocol frame limit.".into(),
        ));
    }

    Ok(Some(response))
}

#[cfg(test)]
mod tests {
    mod streaming;

    use super::{
        BODY_LIMIT, Emitter, collect, credential, generate, generate_at, generate_request,
        image_data, keyring, live_stream, messages, response_body, stream_body, tools,
    };

    use krabb_core::types::{Content, Message, ModelRequest, Request, Role, ToolSpec};
    use serde_json::json;

    fn test_emitter() -> Emitter {
        let (output, _) = tokio::sync::mpsc::channel(8);
        Emitter::new(output)
    }

    fn model() -> ModelRequest {
        ModelRequest {
            generation: None,
            model: "test".into(),
            workspace: None,
            messages: vec![
                Message {
                    id: "1".into(),
                    session: "test".into(),
                    role: Role::System,
                    sender: None,
                    content: vec![Content::Text { text: "rules".into() }],
                },
                Message {
                    id: "2".into(),
                    session: "test".into(),
                    role: Role::User,
                    sender: None,
                    content: vec![
                        Content::Text { text: "hello".into() },
                        Content::File {
                            uri: "file://note".into(),
                            name: "note".into(),
                            mime: None,
                        },
                        Content::Image { uri: "data:image/png;base64,aW1hZ2U=".into(), alt: None },
                    ],
                },
                Message {
                    id: "3".into(),
                    session: "test".into(),
                    role: Role::Assistant,
                    sender: None,
                    content: vec![Content::Text { text: "prior".into() }],
                },
                Message {
                    id: "4".into(),
                    session: "test".into(),
                    role: Role::Tool,
                    sender: None,
                    content: vec![Content::Text { text: "result".into() }],
                },
            ],
            stream: false,
            tools: vec![ToolSpec {
                name: "read".into(),
                description: Some("Read a file".into()),
                schema: json!({"type": "object"}),
            }],
        }
    }

    #[tokio::test]
    async fn completes_streams_beyond_the_notification_budget() {
        let text = "x".repeat(128);
        let mut body =
            format!("{}\n", json!({"message":{"content":text},"done":false})).repeat(300);
        body.push_str("{\"message\":{},\"done\":true,\"done_reason\":\"stop\"}\n");

        let chunks =
            futures_util::stream::iter(vec![Ok::<_, std::io::Error>(body.as_bytes().to_vec())]);

        let (output, mut events) = tokio::sync::mpsc::channel(256);
        let mut emitter = Emitter::new(output);
        let result = live_stream(1, chunks, &mut emitter).await.unwrap().unwrap();

        assert_eq!(result.result.unwrap()["text"], text.repeat(300));
        let mut count = 0;

        while events.try_recv().is_ok() {
            count += 1;
        }

        assert_eq!(count, 256);
    }

    #[test]
    fn parses_success_and_failure_responses() {
        let result = response_body(3, json!({"message":{"content":"done"},"done_reason":"stop","prompt_eval_count":2,"eval_count":4})).unwrap().unwrap();

        assert_eq!(result.result.unwrap()["text"], "done");
        assert!(response_body(3, json!({})).is_err());
        let tool = response_body(
            3,
            json!({"message":{"tool_calls":[{"function":{"name":"read","arguments":{"path":"note"}}}]}}),
        )
        .unwrap()
        .unwrap();

        assert_eq!(tool.result.unwrap()["events"][0]["name"], "read");
    }

    #[test]
    fn builds_messages_for_each_role() {
        let messages = messages(&model()).unwrap();

        assert_eq!(messages[0], json!({"role": "system", "content": "rules"}));
        assert_eq!(
            messages[1],
            json!({
                "role": "user",
                "content": "hello\n[File: note]",
                "images": ["aW1hZ2U="]
            })
        );

        assert_eq!(messages[2], json!({"role": "assistant", "content": "prior"}));
        assert_eq!(messages[3], json!({"role": "tool", "content": "result"}));
        assert_eq!(tools(&model())[0]["function"]["name"], "read");
    }

    #[test]
    fn rejects_image_paths() {
        let mut input = model();
        input.messages[1].content[2] =
            Content::Image { uri: "file:///private/image.png".into(), alt: None };
        assert!(messages(&input).is_err());
        assert_eq!(image_data("data:image/png;base64,aW1hZ2U=").unwrap(), "aW1hZ2U=");
        assert!(image_data("data:image/bmp;base64,abc").is_err());
        assert!(image_data("data:image/png;base64,").is_err());
        assert!(keyring("ollama").is_none());
        assert!(credential().is_none());
    }

    #[tokio::test]
    async fn bounds_provider_bodies() {
        use futures_util::stream;

        let chunks = stream::iter(vec![Ok::<_, std::io::Error>(b"ok".to_vec())]);

        assert_eq!(collect(chunks, "Ollama").await.unwrap(), "ok");
        let chunks = stream::iter(vec![Ok::<_, std::io::Error>(vec![0xff])]);

        assert!(collect(chunks, "Ollama").await.is_err());
        let chunks = stream::iter(vec![Ok::<_, std::io::Error>(vec![b'x'; BODY_LIMIT + 1])]);

        assert!(collect(chunks, "Ollama").await.is_err());
    }

    #[test]
    fn parses_stream_events() {
        let result = stream_body(
            4,
            "{\"message\":{\"content\":\"hel\"},\"done\":false}\n{\"message\":{\"content\":\"lo\"},\"done_reason\":\"stop\"}\n",
        )
        .unwrap()
        .unwrap();

        let reply = result.result.unwrap();

        assert_eq!(reply["text"], "hello");
        assert_eq!(reply["stop"], "stop");
        assert!(stream_body(4, "invalid\n").is_err());
    }

    #[tokio::test]
    async fn validates_requests_and_network_failures() {
        let client = reqwest::Client::new();

        assert!(
            generate(&client, Request::call(1, "unknown", json!({})), test_emitter())
                .await
                .unwrap()
                .is_none()
        );
        let note =
            Request::Note { jsonrpc: "2.0".into(), method: "generate".into(), params: json!({}) };

        assert!(generate(&client, note, test_emitter()).await.unwrap().is_none());
        assert!(generate_request(&client, 1, model(), "http://127.0.0.1:1").await.is_err());
        let mut emitter = test_emitter();

        assert!(
            generate_at(&client, 1, model(), "http://127.0.0.1:1", &mut emitter).await.is_err()
        );
        let mut stream = model();
        stream.stream = true;

        assert!(generate_at(&client, 1, stream, "http://127.0.0.1:1", &mut emitter).await.is_err());
        assert!(
            generate(&client, Request::call(1, "generate", json!({})), test_emitter())
                .await
                .is_err()
        );

        assert!(response_body(3, json!({"message":{}})).is_err());
    }

    #[test]
    fn parses_stream_chunks() {
        let result = stream_body(
            1,
            "{\"message\":{\"content\":\"hello\"},\"done\":false}\n{\"message\":{},\"done\":true,\"done_reason\":\"stop\"}\n",
        )
        .unwrap()
        .unwrap();

        assert_eq!(result.result.unwrap()["text"], "hello");
    }

    #[tokio::test]
    async fn streams_text_and_tool_calls() {
        use futures_util::stream;

        let (output, mut events) = tokio::sync::mpsc::channel(8);
        let mut emitter = Emitter::new(output);
        let body = concat!(
            "{\"message\":{\"content\":\"Ready.\"},\"done\":false}\n",
            "{\"message\":{\"tool_calls\":[{\"function\":{\"name\":\"read\",\"arguments\":{\"path\":\"README.md\"}}}]},\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":4,\"eval_count\":2}\n",
        );

        let chunks = stream::iter(vec![Ok::<_, std::io::Error>(body.as_bytes().to_vec())]);
        let response = live_stream(5, chunks, &mut emitter).await.unwrap().unwrap();

        assert_eq!(response.result.as_ref().unwrap()["text"], "Ready.");
        assert_eq!(response.result.as_ref().unwrap()["events"][0]["name"], "read");
        assert_eq!(response.result.as_ref().unwrap()["events"][0]["args"]["path"], "README.md");
        assert_eq!(response.result.as_ref().unwrap()["input"], 4);
        let event: krabb_core::types::Request =
            serde_json::from_value(events.try_recv().unwrap()).unwrap();

        assert!(matches!(
            event,
            krabb_core::types::Request::Note { params, .. }

                if params["event"]["text"] == "Ready."
        ));
    }
}

#[cfg(test)]
#[path = "tests/hardening.rs"]
mod hardening;

#[cfg(test)]
mod defaults;
