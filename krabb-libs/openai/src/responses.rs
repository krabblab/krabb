//! Stateless Responses transport with native search and portable host tool calls.
mod citations;
mod history;
mod stream;

use futures_util::StreamExt;

use krabb_core::{
    Error, Result,
    context::State,
    plugin::Emitter,
    types::{Event, ModelReply, ModelRequest, Response},
};

use serde_json::{Value, json};

const LIMIT: usize = krabb_core::jsonl::MAX / 2;

pub struct Api<'a> {
    pub client: &'a reqwest::Client,
    pub key: &'a str,
    pub base: &'a str,
    pub provider: &'a str,
}

impl Api<'_> {
    pub async fn generate(
        &self,
        id: u64,
        input: &ModelRequest,
        model: &str,
        emitter: Option<&mut Emitter>,
    ) -> Result<Option<Response>> {
        let body = body(input, model, self.provider, emitter.is_some())?;

        let response = self
            .client
            .post(format!("{}/responses", self.base.trim_end_matches('/')))
            .bearer_auth(self.key)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                Error::Denied(format!("OpenAI Responses request failed: {}.", error.without_url()))
            })?;

        if !response.status().is_success() {
            return Err(Error::Denied(format!(
                "OpenAI Responses request was rejected with {}.",
                response.status()
            )));
        }

        let reply = if let Some(emitter) = emitter {
            stream::parse(response.bytes_stream(), model, self.provider, emitter).await?
        } else {
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();

            while let Some(chunk) = stream.next().await {
                let chunk =
                    chunk.map_err(|_| Error::Denied("OpenAI response transport failed.".into()))?;

                if chunk.len() > LIMIT.saturating_sub(bytes.len()) {
                    return Err(Error::Limit("OpenAI response exceeded the frame budget.".into()));
                }

                bytes.extend_from_slice(&chunk);
            }

            reply(&serde_json::from_slice(&bytes)?, model, self.provider)?
        };

        let response = Response::ok(id, serde_json::to_value(reply)?);

        if serde_json::to_vec(&response)?.len().saturating_add(1) > krabb_core::jsonl::MAX {
            return Err(Error::Limit("OpenAI response exceeded the frame budget.".into()));
        }

        Ok(Some(response))
    }
}

fn body(input: &ModelRequest, model: &str, provider: &str, stream: bool) -> Result<Value> {
    let summary = input.generation.is_some_and(|value| value.summary);

    let mut tools: Vec<Value> = if summary {
        vec![]
    } else {
        input
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type":"function", "name":tool.name, "description":tool.description,
                    "parameters":tool.schema, "strict":false,
                })
            })
            .collect()
    };

    if input.generation.is_some_and(|value| value.search) && !summary {
        tools.push(json!({"type":"web_search"}));
    }

    let mut body = json!({
        "model":model, "input":history::input(input, model, provider)?, "tools":tools,
        "stream":stream, "store":false, "include":["reasoning.encrypted_content"],
        "max_output_tokens":input.generation.map_or(4096, |value| value.output),
    });

    if model.starts_with("gpt-6-luna") {
        body["reasoning"] = json!({"effort":"none"});
    }

    Ok(body)
}

fn reply(value: &Value, model: &str, provider: &str) -> Result<ModelReply> {
    let incomplete = value["status"] == "incomplete";

    if value["status"] != "completed"
        && !(incomplete && value["incomplete_details"]["reason"] == "max_output_tokens")
    {
        return Err(Error::Denied("OpenAI response did not finish successfully.".into()));
    }

    let output = value["output"]
        .as_array()
        .ok_or_else(|| Error::Protocol("OpenAI response omitted its output.".into()))?;

    let mut text = String::new();
    let mut events = Vec::new();

    for item in output {
        match item["type"].as_str().unwrap_or_default() {
            "message" => {
                for part in item["content"].as_array().into_iter().flatten() {
                    text.push_str(&citations::text(part));
                }
            }

            "function_call" => {
                let name = item["name"].as_str().filter(|name| !name.is_empty());
                let id = item["call_id"].as_str().filter(|id| !id.is_empty());

                let args: Value =
                    serde_json::from_str(item["arguments"].as_str().unwrap_or_default())?;

                if incomplete
                    || name.is_none()
                    || id.is_none()
                    || !args.is_object()
                    || events.len() >= 16
                {
                    return Err(Error::Protocol(
                        "OpenAI returned an incomplete or invalid function call.".into(),
                    ));
                }

                events.push(Event::Tool {
                    name: name.unwrap().into(),
                    args,
                    id: id.map(str::to_owned),
                    thought_signature: None,
                });
            }

            "reasoning" | "web_search_call" => {}
            _ => return Err(Error::Protocol("OpenAI returned an unsupported output item.".into())),
        }
    }

    if text.is_empty() && events.is_empty() {
        return Err(Error::Denied("OpenAI response contained no text or tools.".into()));
    }

    let called = !events.is_empty();

    if called {
        let state = State {
            provider: provider.into(),
            model: model.into(),
            data: serde_json::to_string(output)?,
        };

        if !state.valid() {
            return Err(Error::Limit("OpenAI continuation exceeded the context limit.".into()));
        }

        events.push(Event::Context { state });
    }

    Ok(ModelReply {
        text,
        events,
        stop: if called {
            "tool"
        } else if incomplete {
            "length"
        } else {
            "stop"
        }
        .into(),
        input: value["usage"]["input_tokens"].as_u64(),
        output: value["usage"]["output_tokens"].as_u64(),
        context_usage: None,
    })
}

#[cfg(test)]
mod tests;
