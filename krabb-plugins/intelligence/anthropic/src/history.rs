use super::*;
use krabb_core::types::{Message, Role};
use std::collections::VecDeque;

pub(super) fn messages(
    input: &ModelRequest,
) -> krabb_core::Result<(String, Vec<serde_json::Value>)> {
    let (system, _) = legacy_messages(&input.messages)?;
    let mut output: Vec<serde_json::Value> = Vec::new();
    let mut pending = VecDeque::<(String, String)>::new();

    for (index, message) in input.messages.iter().enumerate() {
        if message.role == krabb_core::types::Role::System {
            continue;
        }

        if message.role != Role::Tool {
            finish(&mut output, &mut pending);
        }

        let (_, mut values) = legacy_messages(std::slice::from_ref(message))?;
        let mut value = values.remove(0);

        if message.role == krabb_core::types::Role::Assistant {
            let mut blocks = Vec::new();
            let mut has_calls = false;

            for (position, item) in message.content.iter().enumerate() {
                match item {
                    Content::Context { .. } => {}

                    Content::ToolCall { name, args, id, .. } => {
                        let id = id.clone().unwrap_or_else(|| format!("call_{index}_{position}"));
                        pending.push_back((name.clone(), id.clone()));
                        blocks.push(
                            json!({"type": "tool_use", "id": id, "name": name, "input": args}),
                        );
                        has_calls = true;
                    }

                    Content::Image { uri, .. } => blocks.push(image_block(uri)?),
                    item => blocks.push(json!({"type": "text", "text": item.render()})),
                }
            }

            if has_calls {
                value["content"] = json!(blocks);
            }
        } else if message.role == krabb_core::types::Role::Tool {
            let position = pending
                .iter()
                .position(|(name, _)| message.sender.as_ref().is_none_or(|sender| sender == name));

            if let Some((_, id)) = position.and_then(|position| pending.remove(position)) {
                let blocks = message
                    .content
                    .iter()
                    .map(|item| match item {
                        Content::Image { uri, .. } => image_block(uri),
                        item => Ok(json!({"type": "text", "text": item.render()})),
                    })
                    .collect::<krabb_core::Result<Vec<_>>>()?;

                let block = json!({"type": "tool_result", "tool_use_id": id, "content": blocks});

                result(&mut output, block);
                continue;
            }

            finish(&mut output, &mut pending);
        }

        output.push(value);
    }

    finish(&mut output, &mut pending);
    Ok((system, output))
}

fn finish(output: &mut Vec<serde_json::Value>, pending: &mut VecDeque<(String, String)>) {
    for (_, id) in pending.drain(..) {
        result(
            output,
            json!({"type": "tool_result", "tool_use_id": id, "content":
                "No tool result was recorded. Execution status is unknown; do not retry automatically."
            }),
        );
    }
}

fn result(output: &mut Vec<serde_json::Value>, result: serde_json::Value) {
    // Parallel tool results must be adjacent blocks in the same user message.
    if let Some(previous) = output.last_mut()
        && previous["role"] == "user"
        && previous["content"].as_array().is_some_and(|blocks| {
            blocks.first().is_some_and(|block| block["type"] == "tool_result")
        })
    {
        previous["content"].as_array_mut().unwrap().push(result);
    } else {
        output.push(json!({"role": "user", "content": [result]}));
    }
}

fn legacy_messages(input: &[Message]) -> krabb_core::Result<(String, Vec<serde_json::Value>)> {
    let mut system = Vec::new();
    let mut messages = Vec::new();

    for message in input {
        let mut blocks = Vec::new();

        for content in &message.content {
            match content {
                Content::Context { .. } => {}
                Content::Text { text } => blocks.push(json!({"type": "text", "text": text})),

                Content::Image { uri, alt } if message.role != krabb_core::types::Role::System => {
                    blocks.push(image_block(uri)?);

                    if let Some(alt) = alt {
                        blocks.push(json!({"type": "text", "text": alt}));
                    }
                }

                Content::Image { alt, .. } => blocks.push(json!({
                    "type": "text",
                    "text": alt.as_deref().unwrap_or("[Image attachment.]"),
                })),

                content => blocks.push(json!({"type": "text", "text": content.render()})),
            }
        }

        let has_image = blocks.iter().any(|block| block["type"] == "image");
        let text =
            blocks.iter().filter_map(|block| block["text"].as_str()).collect::<Vec<_>>().join("\n");

        match message.role {
            krabb_core::types::Role::System => system.push(text),
            krabb_core::types::Role::User => messages.push(json!({
                "role": "user",
                "content": if has_image { json!(blocks) } else { json!(text) }
            })),

            krabb_core::types::Role::Assistant => messages.push(json!({
                "role": "assistant",
                "content": if has_image { json!(blocks) } else { json!(text) }
            })),

            krabb_core::types::Role::Tool => {
                let prefix = format!(
                    "[Tool result{}]",
                    message.sender.as_deref().map_or(String::new(), |name| format!(" from {name}"))
                );

                let content = if has_image {
                    let mut tool_blocks = vec![json!({"type": "text", "text": prefix})];
                    tool_blocks.extend(blocks);
                    json!(tool_blocks)
                } else {
                    json!(format!("{prefix}\n{text}"))
                };

                messages.push(json!({"role": "user", "content": content}));
            }
        }
    }

    Ok((system.join("\n"), messages))
}
