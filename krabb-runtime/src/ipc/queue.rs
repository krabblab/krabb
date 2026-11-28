//! Lets client conversations queue messages in the session's bounded queue, like channels.

use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Addition {
    id: String,
    client: String,
    request: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Removal {
    id: String,
    client: String,
    request: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    id: String,
    client: String,
}

/// Queues a client message behind the running turn and earlier queued messages.
pub(super) fn add(params: &Value, state: &State) -> io::Result<Value> {
    let request: Addition =
        serde_json::from_value(params.clone()).map_err(|error| invalid(&error.to_string()))?;

    if request.request.is_empty()
        || request.request.len() > crate::META_LIMIT
        || request.text.trim().is_empty()
        || request.text.len() > crate::TEXT_LIMIT
    {
        return Err(invalid("Invalid queued message."));
    }

    let mut store = state.sessions.lock().map_err(lock)?;

    owned(&store, state, &request.id, &request.client)?;

    let started =
        store.sessions[&request.id].messages.iter().any(|message| message.id == request.request);

    if !started {
        let message = Message {
            id: request.request.clone(),
            session: request.id.clone(),
            role: Role::User,
            sender: Some(request.client.clone()),
            content: vec![Content::Text { text: request.text }],
        };

        store.queue_with_roles(&request.id, message, Vec::new())?;
    }

    let session = &store.sessions[&request.id];
    let position = session.queued.iter().position(|message| message.id == request.request);

    Ok(json!({
        "id": request.id,
        "request": request.request,
        "position": position.map(|index| index + 1),
        "started": started,
        "queue": view(session),
    }))
}

/// Cancels a queued client message that has not started.
pub(super) fn remove(params: &Value, state: &State) -> io::Result<Value> {
    let request: Removal =
        serde_json::from_value(params.clone()).map_err(|error| invalid(&error.to_string()))?;

    let mut store = state.sessions.lock().map_err(lock)?;

    owned(&store, state, &request.id, &request.client)?;

    let cancelled = store.unqueue(&request.id, &request.request)?;

    Ok(json!({
        "id": request.id,
        "request": request.request,
        "cancelled": cancelled,
        "queue": view(&store.sessions[&request.id]),
    }))
}

/// Reserves an idle session for the queue's first client message, which it returns.
pub(super) fn next(params: &Value, state: &State) -> io::Result<Value> {
    let request: Start =
        serde_json::from_value(params.clone()).map_err(|error| invalid(&error.to_string()))?;

    let mut store = state.sessions.lock().map_err(lock)?;

    owned(&store, state, &request.id, &request.client)?;

    let owner = token()?;
    let message = store.reserve_next(&request.id, owner.clone(), &request.client)?;

    let started = message.as_ref().map(message_value);
    let queue = view(&store.sessions[&request.id]);

    Ok(match started {
        Some(message) => json!({"id": request.id, "owner": owner, "message": message,
            "queue": queue}),
        None => json!({"id": request.id, "message": null, "queue": queue}),
    })
}

/// The queued messages of a session in the order their turns start.
pub(super) fn view(session: &crate::state::Session) -> Value {
    Value::Array(
        session
            .queued
            .iter()
            .map(|message| {
                let text =
                    message.content.iter().map(Content::render).collect::<Vec<_>>().join("\n");

                json!({
                    "id": crate::clip(message.id.clone(), SUMMARY_LIMIT),
                    "sender": message.sender.as_ref()
                        .map(|sender| crate::clip(sender.clone(), SUMMARY_LIMIT)),
                    "text": crate::clip(text, SUMMARY_LIMIT),
                })
            })
            .collect(),
    )
}

fn owned(store: &Store, state: &State, id: &str, client: &str) -> io::Result<()> {
    if !state.known_client(client) {
        return Err(invalid("The client is unknown."));
    }

    let session = store.sessions.get(id).ok_or_else(|| not_found("Session was not found."))?;
    let principal = crate::sharing::Principal::Client(client.into());

    if !super::requests::owned(session, &principal) {
        return Err(invalid("The client does not own this conversation."));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LIMIT;

    fn fixture(label: &str) -> State {
        let state = super::super::tests::state_at(&format!("queue-{label}"));
        let mut store = state.sessions.lock().unwrap();

        store.ensure("chat", "model").unwrap();
        store.client("chat", krabb_core::client::TUI).unwrap();
        drop(store);
        state
    }

    fn queue(state: &State, request: &str) -> io::Result<Value> {
        add(
            &json!({"id": "chat", "client": krabb_core::client::TUI, "request": request,
                "text": format!("text {request}")}),
            state,
        )
    }

    fn start(state: &State) -> Value {
        next(&json!({"id": "chat", "client": krabb_core::client::TUI}), state).unwrap()
    }

    fn ids(value: &Value) -> Vec<String> {
        value["queue"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_owned())
            .collect()
    }

    fn detail(state: &State) -> Value {
        super::super::detail(&state.sessions.lock().unwrap().sessions["chat"])
    }

    #[test]
    fn two_windows_share_one_ordered_queue_that_starts_after_the_running_turn() {
        let state = fixture("order");

        state.sessions.lock().unwrap().reserve("chat", "running".into()).unwrap();

        assert_eq!(queue(&state, "a").unwrap()["position"], 1);
        assert_eq!(queue(&state, "b").unwrap()["position"], 2);
        assert_eq!(queue(&state, "c").unwrap()["position"], 3);
        assert_eq!(queue(&state, "b").unwrap()["position"], 2);
        assert_eq!(start(&state)["message"], Value::Null);
        assert_eq!(ids(&detail(&state)), ["a", "b", "c"]);

        state.sessions.lock().unwrap().set_status("chat", "idle").unwrap();

        let first = start(&state);

        assert_eq!(first["message"]["id"], "a");
        assert!(first["owner"].as_str().is_some_and(|owner| !owner.is_empty()));
        assert_eq!(ids(&first), ["b", "c"]);
        assert_eq!(start(&state)["message"], Value::Null);

        let retried = queue(&state, "a").unwrap();

        assert_eq!(retried["started"], true);
        assert_eq!(retried["position"], Value::Null);
        assert_eq!(ids(&retried), ["b", "c"]);

        let conversation = detail(&state)["messages"].as_array().unwrap().clone();

        assert_eq!(conversation.iter().filter(|message| message["id"] == "a").count(), 1);
    }

    #[test]
    fn cancels_waiting_messages_only() {
        let state = fixture("cancel");
        let removal = |request: &str| json!({"id": "chat", "client": krabb_core::client::TUI, "request": request});

        queue(&state, "a").unwrap();
        queue(&state, "b").unwrap();
        start(&state);

        assert_eq!(remove(&removal("a"), &state).unwrap()["cancelled"], false);

        let cancelled = remove(&removal("b"), &state).unwrap();

        assert_eq!(cancelled["cancelled"], true);
        assert!(ids(&cancelled).is_empty());
        assert_eq!(remove(&removal("b"), &state).unwrap()["cancelled"], false);
    }

    #[test]
    fn a_full_queue_refuses_without_dropping_and_keeps_its_order_after_restart() {
        let state = fixture("full");

        state.sessions.lock().unwrap().reserve("chat", "running".into()).unwrap();

        for index in 0..LIMIT {
            queue(&state, &format!("m{index:03}")).unwrap();
        }

        let error = queue(&state, "over").unwrap_err();

        assert_eq!(error.to_string(), krabb_core::types::QUEUE_FULL);

        let saved = state.sessions.lock().unwrap().sessions["chat"].queued.clone();
        let restarted = Store::load(super::super::tests::test_store_path("queue-full")).unwrap();

        assert_eq!(restarted.sessions["chat"].queued, saved);
        assert_eq!(saved.len(), LIMIT);
        assert_eq!(saved.first().unwrap().id, "m000");
        assert_eq!(saved.last().unwrap().id, format!("m{:03}", LIMIT - 1));
    }

    #[test]
    fn refuses_invalid_foreign_and_channel_requests() {
        let state = fixture("refuse");
        let blank = json!({"id": "chat", "client": krabb_core::client::TUI, "request": "a",
            "text": "  "});

        assert!(add(&blank, &state).is_err());
        assert!(add(&json!({"id": "chat"}), &state).is_err());

        let stranger = json!({"id": "chat", "client": "stranger", "request": "a", "text": "hi"});

        assert!(add(&stranger, &state).is_err());

        let missing = json!({"id": "gone", "client": krabb_core::client::TUI, "request": "a",
            "text": "hi"});

        assert_eq!(add(&missing, &state).unwrap_err().kind(), io::ErrorKind::NotFound);

        state.sessions.lock().unwrap().sessions.get_mut("chat").unwrap().channel =
            Some("telegram".into());

        assert!(queue(&state, "a").is_err());
        assert!(next(&json!({"id": "chat", "client": krabb_core::client::TUI}), &state).is_err());
    }

    #[test]
    fn older_clients_keep_reserving_an_idle_session_and_wait_behind_a_queue() {
        let state = fixture("compatible");

        assert_eq!(detail(&state)["queue"], json!([]));

        let reserve = IpcRequest::call(1, "secret", "session.reserve", json!({"id": "chat"}));
        let reserved = super::super::dispatch(&reserve, &state).unwrap();

        assert!(reserved.result.unwrap()["owner"].as_str().is_some());

        state.sessions.lock().unwrap().set_status("chat", "idle").unwrap();
        queue(&state, "a").unwrap();

        let refused = super::super::dispatch(&reserve, &state).unwrap_err();

        assert_eq!(refused.to_string(), krabb_core::types::SESSION_BUSY);

        let unknown = IpcRequest::call(
            2,
            "secret",
            "session.queue",
            json!({"id": "chat",
            "client": krabb_core::client::TUI, "request": "b", "text": "hi", "future": 1}),
        );

        assert!(super::super::dispatch(&unknown, &state).is_err());
    }

    #[test]
    fn a_burst_from_many_windows_stays_bounded_without_duplicates() {
        let state = Arc::new(fixture("burst"));

        state.sessions.lock().unwrap().reserve("chat", "running".into()).unwrap();

        let windows = (0..4)
            .map(|window| {
                let state = Arc::clone(&state);

                std::thread::spawn(move || {
                    (0..LIMIT)
                        .filter(|index| queue(&state, &format!("w{window}-{index:03}")).is_ok())
                        .count()
                })
            })
            .collect::<Vec<_>>();

        let accepted = windows.into_iter().map(|window| window.join().unwrap()).sum::<usize>();
        let saved = ids(&detail(&state));
        let unique = saved.iter().collect::<std::collections::BTreeSet<_>>();

        assert_eq!(accepted, LIMIT);
        assert_eq!(saved.len(), LIMIT);
        assert_eq!(unique.len(), LIMIT);

        for window in 0..4 {
            let own = saved
                .iter()
                .filter(|id| id.starts_with(&format!("w{window}-")))
                .collect::<Vec<_>>();

            assert!(own.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }
}
