use super::*;
use futures_util::{Stream, StreamExt};

#[derive(Default)]
pub(super) struct Reply {
    model: String,
    parts: Vec<Value>,
    citations: Vec<String>,
    text: String,
    events: Vec<Event>,
    finish: Option<String>,
    input: Option<u64>,
    output: Option<u64>,
}

impl Reply {
    pub(super) fn new(model: &str) -> Self {
        Self { model: model.into(), ..Self::default() }
    }

    pub(super) fn absorb(&mut self, value: &Value) -> krabb_core::Result<()> {
        if value.get("error").is_some() || value["promptFeedback"].get("blockReason").is_some() {
            return Err(krabb_core::Error::Denied(
                "Gemini rejected or blocked the response.".into(),
            ));
        }

        self.input = value["usageMetadata"]["promptTokenCount"].as_u64().or(self.input);
        self.output = value["usageMetadata"]["candidatesTokenCount"].as_u64().or(self.output);
        let candidate = &value["candidates"][0];

        if let Some(reason) = candidate["finishReason"].as_str() {
            if !matches!(reason, "STOP" | "MAX_TOKENS") {
                return Err(krabb_core::Error::Denied(
                    "Gemini generation did not finish successfully.".into(),
                ));
            }

            self.finish = Some(reason.into());
        }

        for part in candidate["content"]["parts"].as_array().into_iter().flatten() {
            self.parts.push(part.clone());

            if part["thought"] == true {
                continue;
            }

            if let Some(text) = part["text"].as_str() {
                self.text.push_str(text);
            }

            if let Some(call) = part.get("functionCall") {
                let name =
                    call["name"].as_str().filter(|name| !name.is_empty()).ok_or_else(|| {
                        krabb_core::Error::Denied(
                            "Gemini returned a tool call without a name.".into(),
                        )
                    })?;
                let args = call.get("args").cloned().unwrap_or_else(|| json!({}));

                if !args.is_object() || self.events.len() >= 16 {
                    return Err(krabb_core::Error::Denied(
                        "Gemini returned invalid or too many tool calls.".into(),
                    ));
                }

                self.events.push(Event::Tool {
                    name: name.into(),
                    args,
                    id: call["id"].as_str().map(str::to_owned),
                    thought_signature: part["thoughtSignature"].as_str().map(str::to_owned),
                });
            }
        }

        for link in search::citations(candidate) {
            if !self.citations.contains(&link) && self.citations.len() < 32 {
                self.text.push(' ');
                self.text.push_str(&link);
                self.citations.push(link);
            }
        }

        Ok(())
    }

    pub(super) fn finish(mut self, streamed: bool) -> krabb_core::Result<ModelReply> {
        if streamed && self.finish.is_none() {
            return Err(krabb_core::Error::Denied(
                "Gemini stream ended before its completion marker.".into(),
            ));
        }

        if self.text.is_empty() && self.events.is_empty() {
            return Err(krabb_core::Error::Denied(
                "Gemini response contained no text or tools.".into(),
            ));
        }

        if self.finish.as_deref() == Some("MAX_TOKENS") && !self.events.is_empty() {
            return Err(krabb_core::Error::Denied(
                "Gemini tool generation exceeded its token limit.".into(),
            ));
        }

        let stop = if !self.events.is_empty() {
            "tool"
        } else if self.finish.as_deref() == Some("MAX_TOKENS") {
            "length"
        } else {
            "stop"
        };

        if !self.events.is_empty()
            && let Some(state) = search::state(&self.model, self.parts)?
        {
            self.events.push(Event::Context { state });
        }

        Ok(ModelReply {
            text: self.text,
            events: self.events,
            stop: stop.into(),
            input: self.input,
            output: self.output,
            context_usage: None,
        })
    }
}

pub(super) fn response(id: u64, reply: ModelReply) -> krabb_core::Result<Option<Response>> {
    let response = Response::ok(id, serde_json::to_value(reply)?);

    if serde_json::to_vec(&response)?.len().saturating_add(1) > krabb_core::jsonl::MAX {
        return Err(krabb_core::Error::Limit(
            "Gemini response exceeds the protocol frame limit.".into(),
        ));
    }

    Ok(Some(response))
}

pub(super) async fn generate(
    client: &reqwest::Client,
    id: u64,
    input: &ModelRequest,
    key: &str,
    base: &str,
    emitter: &mut Emitter,
) -> krabb_core::Result<Option<Response>> {
    let model = model_name(input)?;
    let response = client
        .post(format!("{base}/models/{model}:streamGenerateContent"))
        .query(&[("alt", "sse")])
        .header("x-goog-api-key", key)
        .json(&request_body(input)?)
        .send()
        .await
        .map_err(|error| {
            krabb_core::Error::Denied(format!("Gemini stream failed: {}.", error.without_url()))
        })?;

    if !response.status().is_success() {
        return Err(krabb_core::Error::Denied(format!(
            "Gemini stream was rejected with {}.",
            response.status()
        )));
    }

    parse_model(id, response.bytes_stream(), emitter, &model).await
}

#[cfg(test)]
pub(super) async fn parse<S, C, E>(
    id: u64,
    stream: S,
    emitter: &mut Emitter,
) -> krabb_core::Result<Option<Response>>
where
    S: Stream<Item = Result<C, E>> + Unpin,
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    parse_model(id, stream, emitter, "fixture").await
}

async fn parse_model<S, C, E>(
    id: u64,
    mut stream: S,
    emitter: &mut Emitter,
    model: &str,
) -> krabb_core::Result<Option<Response>>
where
    S: Stream<Item = Result<C, E>> + Unpin,
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut buffer = Vec::new();
    let mut data = String::new();
    let mut bytes = 0_usize;
    let mut reply = Reply::new(model);
    let mut pending = String::new();
    let mut emitted = 0;
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(80));
    ticker.tick().await;

    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };

                let chunk = chunk.map_err(|_| krabb_core::Error::Denied("Gemini response transport failed.".into()))?;
                bytes = bytes.saturating_add(chunk.as_ref().len());

                if bytes > BODY_LIMIT {
                    return Err(krabb_core::Error::Limit("Gemini stream exceeded the response limit.".into()));
                }

                buffer.extend_from_slice(chunk.as_ref());

                while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
                    let line = buffer.drain(..=end).collect::<Vec<_>>();
                    let line = std::str::from_utf8(&line).map_err(|_| krabb_core::Error::Denied("Gemini stream was not UTF-8.".into()))?.trim_end_matches(['\r', '\n']);

                    if line.is_empty() && !data.is_empty() {
                        let value: Value = serde_json::from_str(&data)?;
                        let previous = reply.text.len();
                        reply.absorb(&value)?;
                        pending.push_str(&reply.text[previous..]);
                        data.clear();

                        if pending.len() >= 128 || reply.finish.is_some() {
                            emit(&mut pending, &mut emitted, emitter).await?;
                        }

                        if reply.finish.is_some() {
                            return response(id, reply.finish(true)?);
                        }
                    } else if let Some(part) = line.strip_prefix("data:") {
                        data.push_str(part.strip_prefix(' ').unwrap_or(part));
                        data.push('\n');
                    }
                }
            }

            _ = ticker.tick(), if !pending.is_empty() => emit(&mut pending, &mut emitted, emitter).await?,
        }
    }

    // An SSE event is complete only after its blank-line delimiter.
    response(id, reply.finish(true)?)
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
