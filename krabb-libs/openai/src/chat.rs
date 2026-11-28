//! Chat Completions transport shared by OpenAI-compatible providers.
mod history;

use std::collections::BTreeMap;

use futures_util::{Stream, StreamExt};

use krabb_core::{
    Error, Result,
    failure::{Failure, Kind},
    plugin::Emitter,
    types::{Event, ModelReply, ModelRequest, Response},
};

use serde_json::{Value, json};

pub use history::messages;

pub const BODY_LIMIT: usize = krabb_core::jsonl::MAX / 2;
pub const TOOL_CALLS: usize = 16;
const TEXT_EVENTS: usize = 256;

/// One provider's Chat Completions endpoint at BASE, with an optional bearer KEY.
pub struct Api<'a> {
    pub client: &'a reqwest::Client,
    pub key: Option<&'a str>,
    pub base: &'a str,
    pub provider: &'a str,
}

impl Api<'_> {
    /// Sends BODY and reads the reply, streaming text through EMITTER when there is one.
    pub async fn generate(
        &self,
        id: u64,
        body: &Value,
        emitter: Option<&mut Emitter>,
    ) -> Result<Option<Response>> {
        let mut request = self.client.post(format!("{}/chat/completions", self.base)).json(body);

        if let Some(key) = self.key {
            request = request.bearer_auth(key);
        }

        let response =
            request.send().await.map_err(|error| krabb_http::transport(&error, self.provider))?;

        if !response.status().is_success() {
            return Err(krabb_http::rejected(response, self.provider).await);
        }

        match emitter {
            Some(emitter) => stream_body(self.provider, id, response.bytes_stream(), emitter).await,

            None => {
                let bytes = collect(self.provider, response.bytes_stream()).await?;

                response_body(self.provider, id, serde_json::from_slice(&bytes)?)
            }
        }
    }
}

/// The request body for INPUT on MODEL, without provider-specific settings.
pub fn body(provider: &str, input: &ModelRequest, model: &str, stream: bool) -> Result<Value> {
    let mut body = json!({
        "model": model,
        "messages": messages(provider, input)?,
        "stream": stream,
        "tools": tools(input),
    });

    if let Some(generation) = input.generation {
        body["max_completion_tokens"] = generation.output.into();

        if generation.summary {
            body["tools"] = json!([]);
        }
    }

    Ok(body)
}

pub fn tools(input: &ModelRequest) -> Vec<Value> {
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
                },
            })
        })
        .collect()
}

pub fn image<'a>(provider: &str, uri: &'a str) -> Result<&'a str> {
    if uri.starts_with("https://") || uri.starts_with("data:image/") {
        Ok(uri)
    } else {
        Err(Error::Denied(format!("{provider} received an unsupported image reference.")))
    }
}

pub async fn collect<S, C, E>(provider: &str, mut stream: S) -> Result<Vec<u8>>
where
    S: Stream<Item = std::result::Result<C, E>> + Unpin,
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut bytes = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            Failure::new(Kind::Transient).error(format!("{provider} response failed: {error}."))
        })?;

        if chunk.as_ref().len() > BODY_LIMIT.saturating_sub(bytes.len()) {
            return Err(Error::Denied(format!("{provider} response exceeded the frame budget.")));
        }

        bytes.extend_from_slice(chunk.as_ref());
    }

    Ok(bytes)
}

pub async fn stream_body<S, C, E>(
    provider: &str,
    id: u64,
    mut stream: S,
    emitter: &mut Emitter,
) -> Result<Option<Response>>
where
    S: Stream<Item = std::result::Result<C, E>> + Unpin,
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut buffer = Vec::new();

    let mut text = String::new();
    let mut pending = String::new();
    let mut calls = BTreeMap::<usize, (String, String, String)>::new();
    let mut bytes: usize = 0;
    let mut emitted = 0;
    let mut complete = false;

    'stream: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            Failure::new(Kind::interrupted(emitted))
                .error(format!("{provider} stream failed: {error}."))
        })?;

        bytes = bytes.saturating_add(chunk.as_ref().len());

        if bytes > BODY_LIMIT {
            return Err(Error::Denied(format!("{provider} stream exceeded the frame budget.")));
        }

        buffer.extend_from_slice(chunk.as_ref());

        while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            let line = buffer.drain(..=end).collect::<Vec<_>>();

            if stream_line(provider, &line, &mut text, &mut pending, &mut calls)
                .map_err(|error| interrupted(error, emitted))?
            {
                complete = true;
                buffer.clear();
                break 'stream;
            }

            if pending.len() >= 128 {
                emit(&mut pending, &mut emitted, emitter).await?;
            }
        }
    }

    if !buffer.is_empty() {
        complete = stream_line(provider, &buffer, &mut text, &mut pending, &mut calls)
            .map_err(|error| interrupted(error, emitted))?;
    }

    if !complete {
        return Err(Failure::new(Kind::interrupted(emitted))
            .error(format!("{provider} stream ended before its completion marker.")));
    }

    emit(&mut pending, &mut emitted, emitter).await?;

    let events = calls
        .into_values()
        .map(|(name, args, call_id)| {
            let args = if args.is_empty() { json!({}) } else { serde_json::from_str(&args)? };

            if name.is_empty() || !args.is_object() {
                return Err(Error::Denied(format!("{provider} returned an invalid tool call.")));
            }

            Ok(Event::Tool {
                name,
                args,
                id: (!call_id.is_empty()).then_some(call_id),
                thought_signature: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if text.is_empty() && events.is_empty() {
        return Err(Error::Denied(format!("{provider} stream contained no text or tools.")));
    }

    response(
        provider,
        id,
        serde_json::to_value(ModelReply {
            timing: None,
            text,
            stop: "stream".into(),
            input: None,
            output: None,
            context_usage: None,
            events,
        })?,
    )
}

/// Whether a stream error event reports a busy server rather than a refused request.
fn overloaded(error: &Value) -> bool {
    [&error["type"], &error["code"]].into_iter().filter_map(Value::as_str).any(|kind| {
        kind.contains("overloaded") || kind == "server_error" || kind == "service_unavailable"
    })
}

/// A transient failure after text reached the person is uncertain, since a replay repeats it.
fn interrupted(error: Error, emitted: usize) -> Error {
    match error {
        Error::Failed { message, failure } if failure.kind == Kind::Transient => {
            Failure::new(Kind::interrupted(emitted)).error(message)
        }

        error => error,
    }
}

pub fn stream_line(
    provider: &str,
    line: &[u8],
    text: &mut String,
    pending: &mut String,
    calls: &mut BTreeMap<usize, (String, String, String)>,
) -> Result<bool> {
    let line = std::str::from_utf8(line)
        .map_err(|_| Error::Denied(format!("{provider} stream was not UTF-8.")))?;

    let Some(data) = line.trim().strip_prefix("data:") else { return Ok(false) };

    let data = data.trim();

    if data == "[DONE]" {
        return Ok(true);
    }

    let value: Value = serde_json::from_str(data)?;

    if overloaded(&value["error"]) {
        return Err(Failure::new(Kind::Transient).error(format!("{provider} is overloaded.")));
    }

    if value.get("error").is_some() {
        return Err(Error::Denied(format!("{provider} stream returned an error event.")));
    }

    let delta = &value["choices"][0]["delta"];

    if let Some(value) = delta["content"].as_str() {
        text.push_str(value);
        pending.push_str(value);
    }

    if let Some(values) = delta["tool_calls"].as_array() {
        for value in values {
            let index = value["index"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| *value < TOOL_CALLS)
                .ok_or_else(|| {
                    Error::Denied(format!("{provider} stream exceeded the tool-call limit."))
                })?;

            let entry =
                calls.entry(index).or_insert_with(|| (String::new(), String::new(), String::new()));

            if let Some(id) = value["id"].as_str() {
                entry.2.push_str(id);
            }

            if let Some(value) = value["function"]["name"].as_str() {
                entry.0.push_str(value);
            }

            if let Some(value) = value["function"]["arguments"].as_str() {
                entry.1.push_str(value);
            }
        }
    }

    Ok(false)
}

async fn emit(pending: &mut String, emitted: &mut usize, emitter: &mut Emitter) -> Result<()> {
    if !pending.is_empty() && *emitted < TEXT_EVENTS {
        emitter.event(json!({"kind":"text","text":std::mem::take(pending)})).await?;
        *emitted += 1;
    }

    pending.clear();
    Ok(())
}

pub fn response_body(provider: &str, id: u64, body: Value) -> Result<Option<Response>> {
    let choice = body["choices"]
        .as_array()
        .and_then(|values| values.first())
        .ok_or_else(|| Error::Denied(format!("{provider} response contained no choices.")))?;

    let text = choice["message"]["content"].as_str().unwrap_or_default().to_owned();
    let mut events = Vec::new();

    for value in choice["message"]["tool_calls"].as_array().into_iter().flatten() {
        let name = value["function"]["name"].as_str().filter(|name| !name.is_empty()).ok_or_else(
            || Error::Denied(format!("{provider} returned a tool call without a name.")),
        )?;

        let args = match value["function"]["arguments"].as_str() {
            Some(args) => serde_json::from_str(args)?,
            None => value["function"]["arguments"].clone(),
        };

        if !args.is_object() || events.len() >= TOOL_CALLS {
            return Err(Error::Denied(format!(
                "{provider} returned invalid or too many tool calls."
            )));
        }

        events.push(Event::Tool {
            name: name.into(),
            args,
            id: value["id"].as_str().map(str::to_owned),
            thought_signature: None,
        });
    }

    if text.is_empty() && events.is_empty() {
        return Err(Error::Denied(format!("{provider} response contained no text or tools.")));
    }

    response(
        provider,
        id,
        serde_json::to_value(ModelReply {
            timing: None,
            text,
            stop: choice["finish_reason"].as_str().unwrap_or("stop").into(),
            input: body["usage"]["prompt_tokens"].as_u64(),
            output: body["usage"]["completion_tokens"].as_u64(),
            context_usage: None,
            events,
        })?,
    )
}

pub fn response(provider: &str, id: u64, result: Value) -> Result<Option<Response>> {
    let response = Response::ok(id, result);

    if serde_json::to_vec(&response)?.len().saturating_add(1) > krabb_core::jsonl::MAX {
        return Err(Error::Denied(format!(
            "{provider} response exceeds the protocol frame limit."
        )));
    }

    Ok(Some(response))
}
