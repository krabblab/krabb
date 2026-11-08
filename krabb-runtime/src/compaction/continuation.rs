//! Recover opaque provider context for bridges whose client protocol only echoes tool calls.
use krabb_core::{
    Content, Role,
    types::{Event, ModelRequest, Request, Response},
};

use serde_json::Value;
use std::collections::VecDeque;

#[derive(PartialEq)]
pub(crate) struct Key {
    model: String,
    workspace: Option<String>,
    session: String,
}

impl Key {
    pub fn from_request(request: &Request) -> Option<Self> {
        let Request::Call { method, params, .. } = request else { return None };

        if method != "generate" {
            return None;
        }

        Some(Self {
            model: params["model"].as_str()?.into(),
            workspace: params["workspace"].as_str().map(str::to_owned),
            session: params["messages"].as_array()?.last()?["session"].as_str()?.into(),
        })
    }
}

struct Record {
    key: Key,
    calls: Vec<Content>,
    context: Vec<Content>,
}

#[derive(Default)]
pub(super) struct Cache(VecDeque<Record>);

impl Cache {
    pub fn capture(&mut self, key: Key, response: &Response) {
        let Some(events) = response.result.as_ref().and_then(|value| value["events"].as_array())
        else {
            return;
        };

        if !events
            .iter()
            .any(|event| event["kind"] == "context" || event["thought_signature"].is_string())
        {
            return;
        }

        if !serde_json::to_vec(events).is_ok_and(|bytes| bytes.len() <= 512 * 1024) {
            return;
        }

        let mut record = Record { key, calls: vec![], context: vec![] };

        for event in events {
            match serde_json::from_value(event.clone()) {
                Ok(Event::Tool { name, args, id: Some(id), thought_signature }) => {
                    record.calls.push(Content::ToolCall {
                        name,
                        args,
                        id: Some(id),
                        thought_signature,
                    });
                }

                Ok(Event::Context { state }) if state.valid() => {
                    record.context.push(Content::Context { state })
                }

                _ => {}
            }
        }

        if record.calls.is_empty() {
            return;
        }

        self.0.push_back(record);

        if self.0.len() > 8 {
            self.0.pop_front();
        }
    }

    pub fn restore(&self, input: &mut ModelRequest) {
        for message in input.messages.iter_mut().filter(|message| message.role == Role::Assistant) {
            let record = self.0.iter().rev().find(|record| {
                record.key.model == input.model
                    && record.key.workspace == input.workspace
                    && record.key.session == message.session
                    && message
                        .content
                        .iter()
                        .any(|part| record.calls.iter().any(|call| same_call(part, call)))
                    && message
                        .content
                        .iter()
                        .filter(|part| matches!(part, Content::ToolCall { .. }))
                        .all(|part| record.calls.iter().any(|call| same_call(part, call)))
            });

            let Some(record) = record else { continue };

            for part in &mut message.content {
                if let Some(Content::ToolCall { thought_signature, .. }) =
                    record.calls.iter().find(|call| same_call(part, call))
                    && let Content::ToolCall { thought_signature: target, .. } = part
                    && target.is_none()
                {
                    *target = thought_signature.clone();
                }
            }

            for context in &record.context {
                if !message.content.contains(context) {
                    message.content.push(context.clone());
                }
            }
        }
    }
}

fn same_call(left: &Content, right: &Content) -> bool {
    fn call(content: &Content) -> Option<(&str, &Value, &str)> {
        if let Content::ToolCall { name, args, id: Some(id), .. } = content {
            Some((name, args, id))
        } else {
            None
        }
    }

    match (call(left), call(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use krabb_core::{context::State, types::Message};
    use serde_json::json;

    fn input(id: &str) -> ModelRequest {
        ModelRequest {
            generation: None,
            model: "small".into(),
            workspace: Some("work".into()),
            stream: false,
            tools: vec![],
            messages: vec![Message {
                id: "assistant".into(),
                session: "session".into(),
                role: Role::Assistant,
                sender: None,
                content: vec![Content::ToolCall {
                    name: "read".into(),
                    args: json!({}),
                    id: Some(id.into()),
                    thought_signature: None,
                }],
            }],
        }
    }

    fn key(input: &ModelRequest) -> Key {
        Key::from_request(&Request::call(1, "generate", serde_json::to_value(input).unwrap()))
            .unwrap()
    }

    fn response(id: &str) -> Response {
        Response::ok(
            1,
            json!({"events":[
                Event::Tool { name:"read".into(), args:json!({}), id:Some(id.into()), thought_signature:Some("signed".into()) },
                Event::Context { state:State { provider:"fixture".into(), model:"small".into(), data:"opaque".into() } },
            ]}),
        )
    }

    #[test]
    fn replay_is_bounded_scoped_and_matches_the_original_tool_call() {
        let mut cache = Cache::default();

        for index in 0..9 {
            let id = index.to_string();

            cache.capture(key(&input(&id)), &response(&id));
        }

        assert_eq!(cache.0.len(), 8);

        let mut expired = input("0");

        cache.restore(&mut expired);

        assert_eq!(expired.messages[0].content.len(), 1);

        let mut current = input("8");

        cache.restore(&mut current);
        cache.restore(&mut current);

        assert_eq!(current.messages[0].content.len(), 2);

        let mut variants = vec![input("8"); 5];

        variants[0].model = "other".into();
        variants[1].workspace = None;
        variants[2].messages[0].session = "other".into();
        variants[3].messages[0].role = Role::User;

        variants[4].messages[0].content = vec![Content::ToolCall {
            name: "write".into(),
            args: json!({}),
            id: Some("8".into()),
            thought_signature: None,
        }];

        for mut variant in variants {
            cache.restore(&mut variant);

            assert_eq!(variant.messages[0].content.len(), 1);
        }

        let before = cache.0.len();

        cache.capture(key(&input("9")), &Response::ok(1, json!({})));

        cache.capture(
            key(&input("9")),
            &Response::ok(1, json!({"events":[{"kind":"text","text":"answer"}]})),
        );

        let mut oversized = response("9");

        oversized.result.as_mut().unwrap()["padding"] = "x".into();

        oversized.result.as_mut().unwrap()["events"][1]["state"]["data"] =
            "x".repeat(512 * 1024).into();

        cache.capture(key(&input("9")), &oversized);

        assert_eq!(cache.0.len(), before);
        assert!(Key::from_request(&Request::call(1, "model.info", json!({}))).is_none());
    }
}
