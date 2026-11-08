use super::*;
use krabb_core::types::Message;
use std::collections::VecDeque;

pub(super) fn messages(input: &ModelRequest) -> krabb_core::Result<Vec<Value>> {
    let mut output = Vec::new();
    let mut pending = VecDeque::<(String, String)>::new();
    let mut images = Vec::new();

    for (index, message) in input.messages.iter().enumerate() {
        if message.role != Role::Tool {
            finish(&mut output, &mut pending, &mut images);
        }

        let mut value = legacy_messages(std::slice::from_ref(message))?.remove(0);
        let mut restored = false;

        if message.role == Role::Assistant {
            let mut calls = Vec::new();
            let mut content = Vec::new();

            for item in &message.content {
                if let Content::ToolCall { name, args, id, .. } = item {
                    let id = id.clone().unwrap_or_else(|| format!("call_{index}_{}", calls.len()));
                    pending.push_back((name.clone(), id.clone()));
                    calls.push(json!({"id": id, "type": "function", "function": {
                        "name": name, "arguments": args.to_string()
                    }}));
                } else {
                    content.push(item.clone());
                }
            }

            if !calls.is_empty() {
                let filtered = Message { content, ..message.clone() };
                value = legacy_messages(&[filtered])?.remove(0);
                value["tool_calls"] = json!(calls);
            }

            if let Some(calls) =
                continuation::restore(message, &model::resolve(&input.model, None), &mut value)?
            {
                pending = calls.into();
                restored = true;
            }
        } else if message.role == Role::Tool {
            let position = pending
                .iter()
                .position(|(name, _)| message.sender.as_ref().is_none_or(|sender| sender == name));

            if let Some((_, id)) = position.and_then(|position| pending.remove(position)) {
                for item in &message.content {
                    if let Content::Image { uri, .. } = item {
                        images
                            .push(json!({"type": "image_url", "image_url": {"url": image(uri)?}}));
                    }
                }

                let text = message
                    .content
                    .iter()
                    .filter(|item| !matches!(item, Content::Context { .. }))
                    .map(|item| match item {
                        Content::Image { alt, .. } => {
                            alt.clone().unwrap_or_else(|| "[Image attachment.]".into())
                        }

                        item => item.render(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                value = json!({"role": "tool", "tool_call_id": id, "content": text});
            } else {
                finish(&mut output, &mut pending, &mut images);
            }
        }

        output.push(value);

        if restored {
            let mut retained = VecDeque::new();

            for (name, id) in pending.drain(..) {
                if message.content.iter().any(|part| {
                    matches!(part, Content::ToolCall { id:Some(expected), .. } if expected == &id)
                }) {
                    retained.push_back((name, id));
                } else {
                    output.push(json!({"role":"tool", "tool_call_id":id, "content":
                        "Execution status is unknown; do not retry automatically."
                    }));
                }
            }

            pending = retained;
        }

        if pending.is_empty() {
            finish(&mut output, &mut pending, &mut images);
        }
    }

    finish(&mut output, &mut pending, &mut images);
    Ok(output)
}

fn finish(
    output: &mut Vec<Value>,
    pending: &mut VecDeque<(String, String)>,
    images: &mut Vec<Value>,
) {
    for (_, id) in pending.drain(..) {
        output.push(json!({"role": "tool", "tool_call_id": id, "content":
            "No tool result was recorded. Execution status is unknown; do not retry automatically."
        }));
    }

    if !images.is_empty() {
        output.push(json!({"role": "user", "content": std::mem::take(images)}));
    }
}

fn legacy_messages(input: &[Message]) -> krabb_core::Result<Vec<Value>> {
    input.iter().map(|message| {
        let role = match message.role { Role::System => "system", Role::User => "user", Role::Assistant => "assistant", Role::Tool => "user" };
        let mut parts = Vec::new();

        if message.role == Role::Tool {
            parts.push(json!({"type":"text","text":"[Tool result]"}));
        }

        for item in &message.content {
            match item {
                Content::Context { .. } => {},
                Content::Text { text } => parts.push(json!({"type":"text","text":text})),

                Content::Image { uri, .. } if message.role != Role::System => {
                    parts.push(json!({"type":"image_url","image_url":{"url":image(uri)?}}));
                }

                Content::Image { alt, .. } => parts.push(json!({"type":"text","text":alt.as_deref().unwrap_or("[Image attachment.]")})),
                item => parts.push(json!({"type":"text","text":item.render()})),
            }
        }

        let content = if parts.iter().all(|part| part["type"] == "text") {
            Value::String(parts.iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n"))
        } else { Value::Array(parts) };

        Ok(json!({"role":role,"content":content}))
    }).collect()
}
