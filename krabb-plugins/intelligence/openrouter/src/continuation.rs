use super::*;
use krabb_core::{context::State, types::Message};

#[derive(Default)]
pub(super) struct Reasoning {
    details: Vec<Value>,
    text: String,
    bytes: usize,
}

impl Reasoning {
    pub fn collect(&mut self, message: &Value) -> krabb_core::Result<()> {
        let details = message["reasoning_details"].as_array();

        if !message["reasoning_details"].is_null() && details.is_none() {
            return Err(krabb_core::Error::Protocol(
                "Invalid OpenRouter reasoning details.".into(),
            ));
        }

        let text = message["reasoning"].as_str().unwrap_or_default();
        let size = details.map_or(Ok(0), |values| serde_json::to_vec(values).map(|v| v.len()))?;

        self.bytes = self.bytes.saturating_add(size).saturating_add(text.len());

        if self.bytes > 240 * 1024 {
            return Err(krabb_core::Error::Limit(
                "OpenRouter reasoning exceeded its limit.".into(),
            ));
        }

        if let Some(details) = details {
            self.details.extend_from_slice(details);
        }

        self.text.push_str(text);
        Ok(())
    }

    pub fn attach(self, model: &str, events: &mut Vec<Event>) -> krabb_core::Result<()> {
        if events.is_empty() || self.details.is_empty() && self.text.is_empty() {
            return Ok(());
        }

        if events.iter().any(|event| {
            matches!(event,
                Event::Tool { id, .. } if id.as_ref().is_none_or(|id| id.is_empty())
            )
        }) {
            return Err(krabb_core::Error::Protocol(
                "OpenRouter continuation omitted a call ID.".into(),
            ));
        }

        let calls = events.iter().filter(|event| matches!(event, Event::Tool { .. }));
        let state = State {
            provider: "openrouter".into(),
            model: model.into(),
            data: serde_json::to_string(&json!({
                "calls":calls.collect::<Vec<_>>(),
                "reasoning_details":self.details,
                "reasoning":self.text,
            }))?,
        };

        if !state.valid() {
            return Err(krabb_core::Error::Limit("OpenRouter continuation is too large.".into()));
        }

        events.push(Event::Context { state });
        Ok(())
    }
}

/// Restore the original reasoning and tool sequence, scoped to the selected model.
pub(super) fn restore(
    message: &Message,
    model: &str,
    value: &mut Value,
) -> krabb_core::Result<Option<Vec<(String, String)>>> {
    let state = message.content.iter().find_map(|part| match part {
        Content::Context { state } if state.provider == "openrouter" && state.model == model => {
            Some(state)
        }

        _ => None,
    });

    let Some(state) = state else { return Ok(None) };

    if !state.valid() {
        return Err(krabb_core::Error::Protocol("Invalid OpenRouter continuation.".into()));
    }

    let data: Value = serde_json::from_str(&state.data)?;
    let calls: Vec<Event> = serde_json::from_value(data["calls"].clone())?;
    let mut wire = Vec::new();
    let mut pending = Vec::new();

    if calls.is_empty() || calls.len() > TOOL_CALLS {
        return Err(krabb_core::Error::Protocol("Invalid OpenRouter continuation calls.".into()));
    }

    for call in &calls {
        let Event::Tool { name, args, id: Some(id), .. } = call else {
            return Err(krabb_core::Error::Protocol(
                "Invalid OpenRouter continuation call.".into(),
            ));
        };

        if name.is_empty() || id.is_empty() || !args.is_object() {
            return Err(krabb_core::Error::Protocol(
                "Invalid OpenRouter continuation call.".into(),
            ));
        }

        wire.push(json!({"id":id, "type":"function", "function": {
            "name":name, "arguments":args.to_string(),
        }}));
        pending.push((name.clone(), id.clone()));
    }

    for part in &message.content {
        if let Content::ToolCall { name, args, id, .. } = part
            && !calls.iter().any(|call| {
                matches!(call,
                    Event::Tool { name: original, args: arguments, id: identifier, .. }

                    if name == original && args == arguments && id == identifier
                )
            })
        {
            return Err(krabb_core::Error::Protocol(
                "OpenRouter continuation does not match its tool calls.".into(),
            ));
        }
    }

    let details = data["reasoning_details"].as_array().ok_or_else(|| {
        krabb_core::Error::Protocol("Invalid OpenRouter reasoning details.".into())
    })?;

    value["tool_calls"] = wire.into();

    if !details.is_empty() {
        value["reasoning_details"] = details.clone().into();
    } else if let Some(text) = data["reasoning"].as_str().filter(|text| !text.is_empty()) {
        value["reasoning"] = text.into();
    }

    Ok(Some(pending))
}

#[cfg(test)]
mod tests;
