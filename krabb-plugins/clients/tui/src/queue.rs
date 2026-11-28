//! Queues messages typed while a turn runs in the session's queue, as chat channels do.

use krabb_core::{
    client,
    types::{Content, Message},
};

use serde_json::Value;
use std::future::Future;

/// Explains a refused message, which stays in the input box, and the one way forward.
pub(crate) const FULL: &str = "The queue is full, so the message stayed in the box.\nSend it \
    after a queued message starts, or cancel one with /unqueue <position>.";

const USAGE: &str = "Usage: /unqueue <position>.\nUse /queue to see the positions.";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Queued {
    pub(crate) id: String,
    pub(crate) text: String,
}

/// What became of a message sent to the queue.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Added {
    Queued { position: usize, queue: Vec<Queued> },
    Full,
    Unsupported,
    Failed(String),
}

/// Reads the queued messages of a session snapshot, in the order their turns start.
pub(crate) fn parse(value: &Value) -> Vec<Queued> {
    value["queue"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(Queued {
                        id: item["id"].as_str()?.to_owned(),
                        text: item["text"].as_str().unwrap_or_default().to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The text of a started message, as the person typed it.
pub(crate) fn text(message: &Message) -> String {
    message.content.iter().map(Content::render).collect::<Vec<_>>().join("\n")
}

/// Whether an older daemon lacks the queue, so the caller keeps its earlier behavior.
pub(crate) fn unsupported(error: &krabb_core::Error) -> bool {
    let message = error.to_string();

    message.contains("Method not found") || message.contains("does not support this operation")
}

/// A request identity no other window of this session can produce.
pub(crate) fn request(sequence: u64) -> String {
    crate::message_id(&format!("user-{}", std::process::id()), sequence)
}

/// Queues TEXT behind the running turn and earlier queued messages.
pub(crate) async fn add<C, F>(
    home: &str,
    session: &str,
    request: String,
    text: &str,
    host: C,
) -> Added
where
    C: Fn(String, String, Value) -> F,
    F: Future<Output = krabb_core::Result<Value>>,
{
    let params = serde_json::json!({
        "id": session, "client": client::TUI, "request": request, "text": text,
    });

    match host(home.into(), "session.queue".into(), params).await {
        Ok(value) => {
            let queue = parse(&value);

            match value["position"].as_u64() {
                Some(position) => Added::Queued { position: position as usize, queue },
                None if value["started"] == true => Added::Queued { position: 0, queue },
                None => Added::Failed("The daemon returned no queue position.".into()),
            }
        }

        Err(error) if error.to_string().contains(krabb_core::types::QUEUE_FULL) => Added::Full,
        Err(error) if unsupported(&error) => Added::Unsupported,
        Err(error) => Added::Failed(crate::sentence(error.to_string())),
    }
}

/// The receipt shown for a queued message.
pub(crate) fn receipt(position: usize) -> String {
    if position == 0 {
        return "Message started.".into();
    }

    format!(
        "Queued at position {position}.\nUse /unqueue {position} to cancel it before it starts."
    )
}

/// Cancels the queued message at POSITION, returning a notice and the remaining queue.
pub(crate) async fn remove<C, F>(
    home: &str,
    session: &str,
    position: &str,
    host: C,
) -> (String, Option<Vec<Queued>>)
where
    C: Fn(String, String, Value) -> F + Copy,
    F: Future<Output = krabb_core::Result<Value>>,
{
    let Ok(position) = position.trim().parse::<usize>() else {
        return (USAGE.into(), None);
    };

    let snapshot =
        match host(home.into(), "session.get".into(), serde_json::json!({"id": session})).await {
            Ok(value) => parse(&value),
            Err(error) => return (format!("{}.", crate::sentence(error.to_string())), None),
        };

    let Some(item) = position.checked_sub(1).and_then(|index| snapshot.get(index)) else {
        return (
            format!("No queued message is at position {position}.\nUse /queue to see the queue."),
            Some(snapshot),
        );
    };

    let params = serde_json::json!({"id": session, "client": client::TUI, "request": item.id});

    match host(home.into(), "session.unqueue".into(), params).await {
        Ok(value) if value["cancelled"] == true => {
            (format!("Cancelled queued message {position}."), Some(parse(&value)))
        }

        Ok(value) => {
            ("That message already started, so it was not cancelled.".into(), Some(parse(&value)))
        }

        Err(error) if unsupported(&error) => {
            ("This Krabb daemon cannot cancel queued messages.\nUpdate Krabb.".into(), None)
        }

        Err(error) => (format!("{}.", crate::sentence(error.to_string())), None),
    }
}

/// Lists the session's queued messages with their positions.
pub(crate) async fn list<C, F>(home: &str, session: &str, host: C) -> (String, Option<Vec<Queued>>)
where
    C: Fn(String, String, Value) -> F,
    F: Future<Output = krabb_core::Result<Value>>,
{
    match host(home.into(), "session.get".into(), serde_json::json!({"id": session})).await {
        Ok(value) => {
            let queue = parse(&value);

            (format_queue(&queue), Some(queue))
        }

        Err(error) => (format!("{}.", crate::sentence(error.to_string())), None),
    }
}

pub(crate) fn format_queue(queue: &[Queued]) -> String {
    if queue.is_empty() {
        return "Queue: empty.".into();
    }

    let rows = queue
        .iter()
        .enumerate()
        .map(|(index, item)| format!("{}. {}", index + 1, line(&item.text)))
        .collect::<Vec<_>>();

    format!("Queue:\n{}\nUse /unqueue <position> to cancel one.", rows.join("\n"))
}

/// The first line of TEXT, marked when more follows.
pub(crate) fn line(text: &str) -> String {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();

    if lines.next().is_some() { format!("{first} …") } else { first.to_owned() }
}

/// Starts the queue's first message when the session is idle, returning its reservation owner.
pub(crate) async fn next<C, F>(home: &str, session: &str, host: C) -> Option<(String, Message)>
where
    C: Fn(String, String, Value) -> F,
    F: Future<Output = krabb_core::Result<Value>>,
{
    let params = serde_json::json!({"id": session, "client": client::TUI});
    let value = host(home.into(), "session.next".into(), params).await.ok()?;

    let owner = value["owner"].as_str()?.to_owned();
    let message = serde_json::from_value::<Message>(value["message"].clone()).ok()?;

    Some((owner, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_queues_and_formats_positions() {
        let value = json!({"queue": [
            {"id": "a", "sender": "tui", "text": "first"},
            {"id": "b", "text": "second\nmore"},
            {"text": "no id"},
        ]});

        let queue = parse(&value);

        assert_eq!(queue.len(), 2);
        assert_eq!(
            format_queue(&queue),
            "Queue:\n1. first\n2. second …\nUse /unqueue <position> \
            to cancel one."
        );

        assert_eq!(format_queue(&[]), "Queue: empty.");
        assert!(parse(&json!({})).is_empty());
        assert_eq!(receipt(0), "Message started.");
        assert!(receipt(2).contains("/unqueue 2"));
    }

    #[tokio::test]
    async fn adds_maps_full_unsupported_and_failed_queues() {
        async fn host(_: String, _: String, params: Value) -> krabb_core::Result<Value> {
            match params["text"].as_str() {
                Some("full") => {
                    Err(krabb_core::Error::Denied(krabb_core::types::QUEUE_FULL.into()))
                }

                Some("old") => Err(krabb_core::Error::Denied("Method not found.".into())),
                Some("broken") => Err(krabb_core::Error::Denied("disk failed".into())),
                Some("blank") => Ok(json!({})),
                Some("started") => Ok(json!({"position": null, "started": true})),
                _ => Ok(json!({"position": 2, "queue": [{"id": "a"}, {"id": "b"}]})),
            }
        }

        let added = add("home", "main", "b".into(), "hello", host).await;

        assert!(matches!(added, Added::Queued { position: 2, ref queue } if queue.len() == 2));
        assert_eq!(add("home", "main", "c".into(), "full", host).await, Added::Full);
        assert_eq!(add("home", "main", "c".into(), "old", host).await, Added::Unsupported);
        assert_eq!(
            add("home", "main", "c".into(), "broken", host).await,
            Added::Failed("Disk failed".into())
        );

        assert!(matches!(add("home", "main", "c".into(), "blank", host).await, Added::Failed(_)));
        assert!(matches!(
            add("home", "main", "c".into(), "started", host).await,
            Added::Queued { position: 0, .. }
        ));
    }

    #[tokio::test]
    async fn removes_by_position_and_reports_each_outcome() {
        async fn host(_: String, method: String, params: Value) -> krabb_core::Result<Value> {
            match method.as_str() {
                "session.get" => Ok(json!({"queue": [{"id": "a", "text": "one"},
                    {"id": "b", "text": "two"}, {"id": "c", "text": "three"}]})),

                "session.unqueue" if params["request"] == "a" => {
                    Ok(json!({"cancelled": true, "queue": [{"id": "b", "text": "two"}]}))
                }

                "session.unqueue" if params["request"] == "b" => {
                    Ok(json!({"cancelled": false, "queue": []}))
                }

                _ => Err(krabb_core::Error::Denied("Method not found.".into())),
            }
        }

        let (text, queue) = remove("home", "main", "1", host).await;

        assert_eq!(text, "Cancelled queued message 1.");
        assert_eq!(queue.unwrap().len(), 1);
        assert!(remove("home", "main", "2", host).await.0.contains("already started"));
        assert!(remove("home", "main", "3", host).await.0.contains("Update Krabb"));
        assert!(remove("home", "main", "9", host).await.0.contains("position 9"));
        assert!(remove("home", "main", "x", host).await.0.starts_with("Usage"));

        let (listed, _) = list("home", "main", host).await;

        assert!(listed.starts_with("Queue:\n1. one\n2. two\n3. three"));
    }

    #[tokio::test]
    async fn next_starts_only_a_valid_reserved_message() {
        async fn host(home: String, _: String, _: Value) -> krabb_core::Result<Value> {
            match home.as_str() {
                "started" => Ok(json!({"owner": "token", "message": {
                    "id": "q1", "session": "main", "role": "user", "sender": "tui",
                    "content": [{"kind": "text", "text": "hello"}]
                }})),

                "idle" => Ok(json!({"message": null})),
                _ => Err(krabb_core::Error::Denied("Method not found.".into())),
            }
        }

        let (owner, message) = next("started", "main", host).await.unwrap();

        assert_eq!(owner, "token");
        assert_eq!(text(&message), "hello");
        assert!(next("idle", "main", host).await.is_none());
        assert!(next("old", "main", host).await.is_none());
    }
}
