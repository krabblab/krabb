use super::*;
use serde_json::Value;

pub(super) fn enabled(input: &ModelRequest) -> bool {
    input.generation.is_some_and(|value| value.search && !value.summary)
}

pub(super) fn text(blocks: &[Value]) -> String {
    let mut text = String::new();

    for block in blocks {
        if let Some(part) = block["text"].as_str() {
            text.push_str(part);

            for citation in block["citations"].as_array().into_iter().flatten().take(32) {
                if let Some(link) = krabb_text::citation(
                    citation["url"].as_str().unwrap_or_default(),
                    citation["title"].as_str().unwrap_or_default(),
                ) {
                    text.push(' ');
                    text.push_str(&link);
                }
            }
        }
    }

    text
}

pub(super) async fn generate(
    client: &reqwest::Client,
    id: u64,
    input: &ModelRequest,
    key: &str,
    base: &str,
) -> krabb_core::Result<Option<Response>> {
    let base = validate_base(base)?;
    // A paused server turn needs its original blocks, including encrypted search results.
    // Buffer this path so no provider state or unfinished search becomes a host tool call.
    let mut body = request_body(input, false)?;
    let mut content = Vec::new();
    let mut bytes = 0_usize;
    let mut input_tokens = 0_u64;
    let mut output_tokens = 0_u64;

    for _ in 0..4 {
        let response = client
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                krabb_core::Error::Denied(format!(
                    "Anthropic search request failed: {}.",
                    error.without_url()
                ))
            })?;
        let status = response.status();

        if !status.is_success() {
            return Err(krabb_core::Error::Denied(format!(
                "Anthropic search was rejected with {status}."
            )));
        }

        let raw = read(response, "Anthropic").await?;
        bytes = bytes.saturating_add(raw.len());

        if bytes > BODY_LIMIT {
            return Err(krabb_core::Error::Limit(
                "Anthropic search exceeded the response limit.".into(),
            ));
        }

        let mut reply: Value = serde_json::from_str(&raw)?;
        input_tokens = input_tokens
            .saturating_add(reply["usage"]["input_tokens"].as_u64().unwrap_or_default());
        output_tokens = output_tokens
            .saturating_add(reply["usage"]["output_tokens"].as_u64().unwrap_or_default());
        let blocks = reply["content"].as_array().ok_or_else(|| {
            krabb_core::Error::Protocol("Anthropic search returned no content blocks.".into())
        })?;
        content.extend_from_slice(blocks);

        if reply["stop_reason"] != "pause_turn" {
            reply["content"] = content.into();
            reply["usage"]["input_tokens"] = input_tokens.into();
            reply["usage"]["output_tokens"] = output_tokens.into();
            return response_body(id, status, reply);
        }

        if blocks.iter().any(|block| block["type"] == "tool_use") {
            return Err(krabb_core::Error::Protocol(
                "Anthropic paused with an unresolved client tool call.".into(),
            ));
        }

        let remaining = body["max_tokens"]
            .as_u64()
            .unwrap_or_default()
            .saturating_sub(reply["usage"]["output_tokens"].as_u64().unwrap_or_default());

        if remaining == 0 {
            return Err(krabb_core::Error::Limit(
                "Anthropic search exhausted its output budget.".into(),
            ));
        }

        body["max_tokens"] = remaining.into();
        body["messages"].as_array_mut().unwrap().push(json!({
            "role": "assistant", "content": reply["content"],
        }));

        if serde_json::to_vec(&body)?.len() > BODY_LIMIT {
            return Err(krabb_core::Error::Limit(
                "Anthropic search continuation exceeded the request limit.".into(),
            ));
        }
    }

    Err(krabb_core::Error::Limit("Anthropic search exceeded its continuation limit.".into()))
}

#[cfg(test)]
mod tests;
