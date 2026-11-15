use super::*;

pub(crate) const NAME: &str = "input";
pub(crate) const INLINE_LIMIT: usize = 16 * 1024;
const EXCERPT: usize = 8 * 1024;
const PAGE_LIMIT: usize = 16 * 1024;

pub(super) fn text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replaces an oversized original request with a bounded excerpt in step context.
pub(crate) fn bound(messages: &mut [Message], request: Option<&str>) -> bool {
    let Some(request) = request else { return false };

    let mut bounded = false;

    for message in
        messages.iter_mut().filter(|message| message.role == Role::User && message.id == request)
    {
        let full = text(message);
        let total = full.chars().count();

        if total <= INLINE_LIMIT {
            continue;
        }

        let excerpt = full.chars().take(EXCERPT).collect::<String>();
        let note = format!(
            concat!(
                "{}\n\n[The original request continues: {} of {} characters are shown. ",
                "Call `input` with an offset to read the rest before relying on it.]"
            ),
            excerpt, EXCERPT, total
        );

        let attachments =
            message.content.iter().filter(|content| !matches!(content, Content::Text { .. }));
        message.content =
            std::iter::once(Content::Text { text: note }).chain(attachments.cloned()).collect();
        bounded = true;
    }

    bounded
}

pub(crate) fn tool() -> ToolSpec {
    ToolSpec {
        name: NAME.into(),
        description: Some(
            "Read a part of this task's saved original request when it was too long to include in \
                full."
                .into(),
        ),
        schema: serde_json::json!({
            "type":"object",
            "properties":{
                "offset":{"type":"integer","minimum":0},
                "length":{"type":"integer","minimum":1,"maximum":PAGE_LIMIT}
            },
            "required":["offset"],
            "additionalProperties":false
        }),
    }
}

pub(crate) fn oversized(store: &state::Store, id: &str) -> bool {
    store
        .sessions
        .get(id)
        .and_then(|session| session.original.as_ref())
        .is_some_and(|message| text(message).chars().count() > INLINE_LIMIT)
}

pub(crate) fn read(store: &state::Store, id: &str, args: &Value) -> Result<String, PluginError> {
    let original = store
        .sessions
        .get(id)
        .and_then(|session| session.original.as_ref())
        .ok_or("This task has no saved original request.")?;

    let offset = args["offset"].as_u64().ok_or("Include a character offset.")?;

    let length = match args.get("length") {
        Some(value) => value.as_u64().ok_or("The length must be a positive integer.")?,
        None => PAGE_LIMIT as u64,
    };

    if length == 0 || length > PAGE_LIMIT as u64 {
        return Err(format!("Read between 1 and {PAGE_LIMIT} characters.").into());
    }

    let full = text(original);
    let total = full.chars().count() as u64;

    if offset >= total {
        return Err(format!("The original request has {total} characters.").into());
    }

    let end = offset.saturating_add(length).min(total);
    let part = full.chars().skip(offset as usize).take((end - offset) as usize).collect::<String>();

    Ok(format!("Characters {offset} to {end} of {total} from the original request:\n{part}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str, text: String) -> Message {
        Message {
            id: id.into(),
            session: "job-1".into(),
            role: Role::User,
            sender: Some("human".into()),
            content: vec![
                Content::Text { text },
                Content::Image { uri: "media/diagram.png".into(), alt: None },
            ],
        }
    }

    #[test]
    fn oversized_requests_are_excerpted_and_read_back_exactly_in_bounded_pages() {
        let full = format!("{}é tail requirement", "a".repeat(INLINE_LIMIT + 10));

        let mut messages = vec![message("request", full.clone()), message("other", full.clone())];

        assert!(bound(&mut messages, Some("request")));

        let excerpt = messages[0].content[0].render();

        assert!(excerpt.chars().count() < INLINE_LIMIT);
        assert!(excerpt.contains(&format!("{EXCERPT} of {} characters", full.chars().count())));
        assert!(matches!(messages[0].content[1], Content::Image { .. }));
        assert_eq!(messages[1].content[0].render(), full);

        let mut store = state::Store::default();

        store.ensure("task-input", "model").unwrap();

        store.sessions.get_mut("task-input").unwrap().original =
            Some(message("request", full.clone()));

        assert!(oversized(&store, "task-input"));

        let offset = INLINE_LIMIT as u64 + 10;
        let tail = read(&store, "task-input", &serde_json::json!({"offset":offset})).unwrap();

        assert!(tail.ends_with("é tail requirement"), "{tail}");
        assert!(tail.starts_with(&format!("Characters {offset} to ")));

        for args in [
            serde_json::json!({}),
            serde_json::json!({"offset":0,"length":0}),
            serde_json::json!({"offset":0,"length":PAGE_LIMIT + 1}),
            serde_json::json!({"offset":full.chars().count()}),
        ] {
            assert!(read(&store, "task-input", &args).is_err(), "{args}");
        }

        let mut short = vec![message("request", "Short goal.".into())];

        assert!(!bound(&mut short, Some("request")));
        assert!(!bound(&mut messages, None));
    }
}
