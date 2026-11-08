//! Provider-neutral context budgets and conversation boundaries.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Content, Error, Message, Result, Role, types::ToolSpec};

/// Opaque continuation data, interpreted only by its originating provider and model.
/// It is stored with the assistant turn and never displayed or treated as instructions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct State {
    pub provider: String,
    pub model: String,
    pub data: String,
}

impl State {
    pub fn valid(&self) -> bool {
        !self.provider.is_empty()
            && self.provider.len() <= 64
            && !self.model.is_empty()
            && self.model.len() <= 256
            && self.data.len() <= 256 * 1024
    }
}

/// Host-selected generation bounds. Summary turns must not search or execute tools.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Generation {
    pub output: u64,
    #[serde(default)]
    pub summary: bool,
    #[serde(default)]
    pub search: bool,
}

/// A provider's total context capacity and the space reserved for generation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Window {
    pub tokens: u64,
    pub output: u64,
}

impl Window {
    pub fn input(self) -> Result<u64> {
        if self.tokens == 0
            || self.tokens > u32::MAX as u64
            || self.output == 0
            || self.output >= self.tokens
        {
            return Err(Error::Protocol("Invalid model context budget.".into()));
        }

        Ok(self.tokens - self.output)
    }
}

/// Optional model metadata returned by `model.info`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Profile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
    #[serde(default)]
    pub compact: bool,
    #[serde(default)]
    pub search: bool,
}

/// A request for provider-native compaction, when advertised by the provider.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompactRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u64,
    /// Total generation allowance, including any provider-internal summarization work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
}

/// Portable compacted history; providers normalize their native results here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompactReply {
    pub text: String,
}

/// A conservative estimate, not a substitute for a provider's tokenizer.
pub fn text_tokens(text: &str) -> u64 {
    let compact = text
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric() || byte.is_ascii_whitespace())
        .count() as u64;

    compact.div_ceil(3).saturating_add(text.len() as u64 - compact)
}

pub fn value_tokens(value: &Value) -> u64 {
    match value {
        Value::Null => 0,
        Value::String(text) => text_tokens(text),
        Value::Array(items) => items
            .iter()
            .fold(0_u64, |sum, item| sum.saturating_add(value_tokens(item)).saturating_add(1)),
        Value::Object(fields) => fields.iter().fold(0_u64, |sum, (key, value)| {
            sum.saturating_add(text_tokens(key))
                .saturating_add(value_tokens(value))
                .saturating_add(4)
        }),
        _ => 4,
    }
}

pub fn estimate(messages: &[Message], tools: &[ToolSpec]) -> u64 {
    let tools = tools.iter().fold(256_u64, |sum, tool| {
        sum.saturating_add(text_tokens(&tool.name))
            .saturating_add(tool.description.as_deref().map_or(0, text_tokens))
            .saturating_add(value_tokens(&tool.schema))
            .saturating_add(16)
    });

    messages.iter().fold(tools, |sum, message| {
        message.content.iter().fold(sum.saturating_add(16), |sum, content| {
            let tokens = match content {
                Content::Text { text } => text_tokens(text),
                Content::Image { .. } => 768,
                Content::Context { state } => text_tokens(&state.data),

                Content::ToolCall { name, args, .. } => {
                    text_tokens(name).saturating_add(value_tokens(args)).saturating_add(16)
                }

                content => text_tokens(&content.render()),
            };

            sum.saturating_add(tokens)
        })
    })
}

/// Retain complete recent user turns; never split an assistant/tool exchange.
pub fn boundary(messages: &[Message], retain: usize) -> Option<usize> {
    let mut users = messages.iter().enumerate().rev().filter(|(_, item)| item.role == Role::User);

    let boundary = users.nth(retain.saturating_sub(1))?.0;

    users.next().map(|_| boundary)
}

/// Last resort after compaction: remove one complete old turn, retaining system messages.
pub fn truncate(messages: &mut Vec<Message>) -> bool {
    let Some(protected) = messages.iter().rposition(|message| message.role == Role::User) else {
        return false;
    };

    let Some(start) = messages[..protected].iter().position(|message| message.role != Role::System)
    else {
        return false;
    };

    let end = (start + 1..=protected)
        .find(|index| messages[*index].role == Role::User)
        .unwrap_or(protected);

    let mut index = 0;

    messages.retain(|message| {
        let keep = index < start || index >= end || message.role == Role::System;
        index += 1;
        keep
    });

    true
}

#[cfg(test)]
mod tests;
