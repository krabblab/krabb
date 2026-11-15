use super::*;
use sha2::Digest;

const USAGE: &str = "Use /task to list tasks, /task pause|resume|cancel|approve|deny <id>, \
    /task steer <id> <instruction>, /task defer <id> <time> <timezone>, or \
    /task extend <id> <minutes>.";

const NO_TASKS: &str = "There are no unfinished tasks in this conversation.";

pub(crate) fn key(session: &str, message: &str) -> String {
    format!(
        "dialog-{}",
        &hex_digest(Sha256::digest(format!("{session}:{message}").as_bytes()))[..40]
    )
}

/// Applies an exact `/task` command once per message; other text is left for the model.
pub(crate) fn message(
    store: &mut state::Store,
    input: &Message,
    signals: &Signals,
) -> Result<Option<String>, PluginError> {
    let text = input.content.iter().map(Content::render).collect::<Vec<_>>().join("\n");

    if !krabb_core::task::command(&text) {
        return Ok(None);
    }

    let key = key(&input.session, &input.id);
    let author = input.sender.as_deref().unwrap_or("user");

    if let Some(reply) = store.control_reply(&input.session, &key, author, &text)? {
        return Ok(Some(reply));
    }

    let reply = apply(store, &input.session, input.sender.as_deref(), &key, &text, signals);

    store.record_control(&input.session, &key, author, &text, &reply)?;
    Ok(Some(reply))
}

fn apply(
    store: &mut state::Store,
    session: &str,
    sender: Option<&str>,
    key: &str,
    text: &str,
    signals: &Signals,
) -> String {
    let rest = text.trim()[krabb_core::task::PREFIX.len()..].trim();
    let (action, rest) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let (id, argument) = rest.trim().split_once(char::is_whitespace).unwrap_or((rest.trim(), ""));
    let argument = argument.trim();

    let mut args = serde_json::json!({"action":action, "id":id, "command":key});

    match action {
        "" | "list" if id.is_empty() => return list(store, session, sender, signals),

        "pause" | "resume" | "cancel" | "approve" | "deny" if argument.is_empty() => {}
        "steer" if !argument.is_empty() => args["instruction"] = argument.into(),
        "defer" if !argument.is_empty() => args["when"] = argument.into(),

        "extend" => match argument.parse::<u64>() {
            Ok(minutes) => args["minutes"] = minutes.into(),
            Err(_) => return USAGE.into(),
        },

        _ => return USAGE.into(),
    }

    if id.is_empty() {
        return USAGE.into();
    }

    let result = refusal(control(store, &args, session, sender, signals, true));

    match result.content.first() {
        Some(ToolContent::Text { text }) => text.lines().next().unwrap_or_default().into(),
        _ => USAGE.into(),
    }
}

fn list(
    store: &mut state::Store,
    session: &str,
    sender: Option<&str>,
    signals: &Signals,
) -> String {
    let args = serde_json::json!({"action":"list"});
    let items = control(store, &args, session, sender, signals, true)
        .ok()
        .and_then(|result| match result.content.into_iter().next() {
            Some(ToolContent::Text { text }) => serde_json::from_str::<Vec<Value>>(&text).ok(),
            _ => None,
        })
        .unwrap_or_default();

    let active = items
        .iter()
        .filter(|item| {
            serde_json::from_value::<JobStatus>(item["status"].clone())
                .is_ok_and(|status| !status.terminal())
        })
        .map(|item| {
            let approval = item["approval"]
                .as_str()
                .map_or(String::new(), |tool| format!("; waiting for approval to run {tool}"));
            let resume = serde_json::from_value(item["resume"].clone())
                .map_or(String::new(), |when| {
                    format!("; resumes no earlier than {}", super::defer::describe(&when))
                });

            format!(
                "{}: {}{}{} — {}",
                item["id"].as_str().unwrap_or_default(),
                item["status"].as_str().unwrap_or("unknown"),
                approval,
                resume,
                item["goal"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();

    if active.is_empty() { NO_TASKS.into() } else { active.join("\n") }
}

pub(crate) fn channel(
    store: &mut state::Store,
    signals: &Signals,
    integration: &str,
    event: &Value,
) -> Result<bool, PluginError> {
    let Some(text) = event["text"].as_str().filter(|text| krabb_core::task::command(text)) else {
        return Ok(false);
    };

    let id = event_id(&event["id"]).ok_or("Control message has no identity.")?;
    let chat = event_id(&event["chat"]).ok_or("Control message has no destination.")?;
    let thread = event_id(&event["thread"]);
    let session = session_id(integration, &chat, thread.as_deref());

    if !store.sessions.contains_key(&session) {
        store.queue_delivery(
            key(&session, &id),
            integration,
            chat,
            event["private"] == true,
            thread,
            NO_TASKS,
        )?;
        return Ok(true);
    }

    store.route(&session, integration, &chat, thread.as_deref(), event["private"] == true)?;

    let input = Message {
        id: id.clone(),
        session: session.clone(),
        role: Role::User,
        sender: event_id(&event["sender"]),
        content: vec![Content::Text { text: text.into() }],
    };

    let Some(reply) = message(store, &input, signals)? else { return Ok(false) };

    store.queue_delivery(
        key(&session, &id),
        integration,
        chat,
        event["private"] == true,
        thread,
        reply,
    )?;
    Ok(true)
}
