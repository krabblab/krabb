use super::*;
use krabb_core::{context::State, types::Message};

pub(super) fn configure(body: &mut Value, input: &ModelRequest) {
    if !input.generation.is_some_and(|value| value.search && !value.summary) {
        return;
    }

    let combined = effective_model(input).starts_with("gemini-3");

    // Gemini 2.x cannot combine search with client functions in one request.
    if !combined && !input.tools.is_empty() {
        return;
    }

    if !body["tools"].is_array() {
        body["tools"] = json!([]);
    }

    body["tools"].as_array_mut().unwrap().push(json!({"googleSearch": {}}));

    if combined && !input.tools.is_empty() {
        body["toolConfig"] = json!({
            "includeServerSideToolInvocations": true,
            "functionCallingConfig": {"mode": "VALIDATED"},
        });
    }
}

pub(super) fn citations(candidate: &Value) -> Vec<String> {
    candidate["groundingMetadata"]["groundingChunks"]
        .as_array()
        .into_iter()
        .flatten()
        .take(32)
        .filter_map(|chunk| {
            krabb_text::citation(
                chunk["web"]["uri"].as_str()?,
                chunk["web"]["title"].as_str().unwrap_or_default(),
            )
        })
        .collect()
}

pub(super) fn state(model: &str, parts: Vec<Value>) -> krabb_core::Result<Option<State>> {
    if !parts
        .iter()
        .any(|part| part.get("toolCall").is_some() || part.get("toolResponse").is_some())
    {
        return Ok(None);
    }

    let state = State {
        provider: "gemini".into(),
        model: model.into(),
        data: serde_json::to_string(&parts)?,
    };

    if !state.valid() {
        return Err(krabb_core::Error::Limit(
            "Gemini continuation exceeded the context limit.".into(),
        ));
    }

    Ok(Some(state))
}

pub(super) struct Replay {
    pub parts: Vec<Value>,
    pub calls: Vec<FunctionCall>,
    pub skipped: Vec<Value>,
}

pub(super) fn replay(message: &Message, model: &str) -> krabb_core::Result<Option<Replay>> {
    let Some(state) = message.content.iter().find_map(|content| match content {
        Content::Context { state } if state.provider == "gemini" && state.model == model => {
            Some(state)
        }

        _ => None,
    }) else {
        return Ok(None);
    };

    if !state.valid() {
        return Err(krabb_core::Error::Limit(
            "Gemini continuation exceeded the context limit.".into(),
        ));
    }

    let parts: Vec<Value> = serde_json::from_str(&state.data)?;
    let expected: Vec<_> = message
        .content
        .iter()
        .filter_map(|content| match content {
            Content::ToolCall { name, args, id, .. } => Some((name, args, id)),
            _ => None,
        })
        .collect();
    let wire: Vec<_> = parts.iter().filter_map(|part| part.get("functionCall")).collect();

    if expected.iter().any(|(name, args, id)| {
        !wire.iter().any(|call| {
            call["name"].as_str() == Some(name.as_str())
                && call.get("args") == Some(*args)
                && call["id"].as_str() == id.as_deref()
        })
    }) {
        return Err(krabb_core::Error::Protocol(
            "Gemini continuation does not match its tool calls.".into(),
        ));
    }

    let mut calls = Vec::new();
    let mut skipped = Vec::new();

    for call in wire {
        if !expected.iter().any(|(_, _, id)| id.as_deref() == call["id"].as_str()) {
            skipped.push(json!({"functionResponse": {
                "id":call["id"], "name":call["name"],
                "response":{"output":concat!(
                    "No tool result was recorded. Execution status is unknown; ",
                    "do not retry automatically.",
                )},
            }}));
            continue;
        }

        calls.push(FunctionCall {
            id: call["id"].as_str().unwrap_or_default().into(),
            name: call["name"].as_str().unwrap_or_default().into(),
            legacy: false,
        });
    }

    Ok(Some(Replay { parts, calls, skipped }))
}

#[cfg(test)]
mod tests;
