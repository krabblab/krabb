use super::*;
use krabb_core::types::{Content, Message, Role};
use std::collections::VecDeque;

const UNKNOWN: &str =
    "No tool result was recorded. Execution status is unknown; do not retry automatically.";

pub(super) fn input(input: &ModelRequest, model: &str, provider: &str) -> Result<Vec<Value>> {
    let mut output = Vec::new();
    let mut pending = VecDeque::<(String, String)>::new();

    for (index, message) in input.messages.iter().enumerate() {
        if message.role != Role::Tool {
            finish(&mut output, &mut pending);
        }

        if message.role == Role::Assistant
            && let Some(items) = replay(message, model, provider)?
        {
            for item in &items {
                if item["type"] == "function_call" {
                    pending.push_back((
                        item["name"].as_str().unwrap_or_default().into(),
                        item["call_id"].as_str().unwrap_or_default().into(),
                    ));
                }
            }

            output.extend(items);

            let mut retained = VecDeque::new();

            for (name, id) in pending.drain(..) {
                let retained_call = message.content.iter().any(|part| {
                    matches!(part, Content::ToolCall { id: Some(expected), .. } if expected == &id)
                });

                if retained_call {
                    retained.push_back((name, id));
                } else {
                    output.push(json!({
                        "type":"function_call_output", "call_id":id, "output":UNKNOWN,
                    }));
                }
            }

            pending = retained;
            continue;
        }

        let parts = parts(message)?;

        if message.role == Role::Tool {
            let matched = pending
                .iter()
                .position(|(name, _)| message.sender.as_ref().is_none_or(|sender| sender == name));

            if let Some((_, id)) = matched.and_then(|index| pending.remove(index)) {
                output.push(json!({"type":"function_call_output", "call_id":id, "output":parts}));
                continue;
            }

            finish(&mut output, &mut pending);
        }

        if !parts.is_empty() {
            let role = match message.role {
                Role::System => "system",
                Role::Assistant => "assistant",
                _ => "user",
            };

            output.push(json!({"role":role, "content":parts}));
        }

        if message.role == Role::Assistant {
            for part in &message.content {
                if let Content::ToolCall { name, args, id, .. } = part {
                    let id =
                        id.clone().unwrap_or_else(|| format!("call_{index}_{}", pending.len()));
                    output.push(json!({
                        "type":"function_call", "call_id":id, "name":name,
                        "arguments":args.to_string(),
                    }));
                    pending.push_back((name.clone(), id));
                }
            }
        }
    }

    finish(&mut output, &mut pending);
    Ok(output)
}

fn parts(message: &Message) -> Result<Vec<Value>> {
    let mut parts = Vec::new();

    for content in &message.content {
        match content {
            Content::Context { .. } | Content::ToolCall { .. } => {}

            Content::Image { uri, .. } if message.role != Role::System => {
                if !(uri.starts_with("https://") || uri.starts_with("data:image/")) {
                    return Err(Error::Denied(
                        "OpenAI received an unsupported image reference.".into(),
                    ));
                }

                parts.push(json!({"type":"input_image", "image_url":uri}));
            }

            content => {
                let text = content.render();

                if !text.is_empty() {
                    parts.push(json!({"type":"input_text", "text":text}));
                }
            }
        }
    }

    Ok(parts)
}

fn finish(output: &mut Vec<Value>, pending: &mut VecDeque<(String, String)>) {
    for (_, id) in pending.drain(..) {
        output.push(json!({"type":"function_call_output", "call_id":id, "output":UNKNOWN}));
    }
}

fn replay(message: &Message, model: &str, provider: &str) -> Result<Option<Vec<Value>>> {
    let Some(state) = message.content.iter().find_map(|part| match part {
        Content::Context { state } if state.provider == provider && state.model == model => {
            Some(state)
        }

        _ => None,
    }) else {
        return Ok(None);
    };

    if !state.valid() {
        return Err(Error::Limit("OpenAI continuation exceeded the context limit.".into()));
    }

    let items: Vec<Value> = serde_json::from_str(&state.data)?;

    if items.iter().any(|item| match item["type"].as_str() {
        Some("message") => item["role"] != "assistant",
        Some("reasoning" | "web_search_call" | "function_call") => false,
        _ => true,
    }) {
        return Err(Error::Protocol("OpenAI continuation contains unsupported items.".into()));
    }

    for part in &message.content {
        if let Content::ToolCall { name, args, id, .. } = part {
            let found = items.iter().any(|item| {
                item["type"] == "function_call"
                    && item["name"].as_str() == Some(name.as_str())
                    && item["call_id"].as_str() == id.as_deref()
                    && item["arguments"]
                        .as_str()
                        .and_then(|args| serde_json::from_str::<Value>(args).ok())
                        .as_ref()
                        == Some(args)
            });

            if !found {
                return Err(Error::Protocol(
                    "OpenAI continuation does not match its tool calls.".into(),
                ));
            }
        }
    }

    Ok(Some(items))
}
