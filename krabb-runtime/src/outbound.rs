//! Sends, edits, and commits channel replies, typing, and queued turns.

use crate::*;

pub(crate) fn send_params(
    channel_id: &str,
    delivery_id: &str,
    chat: &str,
    text: &str,
    thread: Option<&str>,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    if channel_id == "discord" {
        return Ok(
            serde_json::json!({"delivery": delivery_id, "channel": chat, "format": "markdown", "text": text}),
        );
    }

    if channel_id != "telegram" {
        let mut value = serde_json::json!({"delivery": delivery_id, "chat": chat, "format": "markdown", "text": text});

        if let Some(thread) = thread {
            value["thread"] = serde_json::Value::String(thread.into());
        }

        return Ok(value);
    }

    let mut value = serde_json::json!({
        "chat": chat.parse::<i64>().map_err(|_| "Telegram chat ID was invalid.")?,
        "delivery": delivery_id,
        "format": "markdown", "text": text
    });

    if let Some(thread) = thread {
        value["thread"] = serde_json::Value::String(thread.into());
    }

    Ok(value)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Typing {
    Shown,
    Unsupported,
    Failed,
}

/// Shows a typing indicator; a failed attempt is retried on the next tick because the channel
/// can be busy with intake, while an unsupported one stops the indicator for the turn.
pub(crate) async fn indicate_typing(
    channel: &Live,
    channel_id: &str,
    chat: &str,
    thread: Option<&str>,
    event: &serde_json::Value,
    call: &mut u64,
) -> Typing {
    let Ok(mut params) = send_params(channel_id, "", chat, "", thread) else {
        return Typing::Unsupported;
    };

    if let Some(params) = params.as_object_mut() {
        params.remove("text");
        params.remove("format");
        params.remove("delivery");

        if let Some(message) = event_id(&event["id"]) {
            params.insert("message".into(), serde_json::json!(message));
        }

        params.insert("private".into(), serde_json::json!(event["private"] == true));
    }

    let request = Request::call(*call, "typing", params);

    *call = call.saturating_add(1);

    let Ok(Ok(response)) =
        tokio::time::timeout(Duration::from_secs(2), channel.call(request)).await
    else {
        return Typing::Failed;
    };

    match (response.error, response.result) {
        (Some(error), _) if error.code == -32601 => Typing::Unsupported,
        (Some(_), _) => Typing::Failed,
        (None, Some(serde_json::Value::Bool(true))) => Typing::Shown,
        (None, _) => Typing::Unsupported,
    }
}

pub(crate) fn send_request(
    channel: &str,
    call: u64,
    delivery: &str,
    chat: &str,
    private: bool,
    text: &str,
    thread: Option<&str>,
) -> Result<Request, Box<dyn std::error::Error + Send + Sync>> {
    let mut params = send_params(channel, delivery, chat, text, thread)?;

    if channel == "signal" {
        params["private"] = serde_json::Value::Bool(private);
    }

    Ok(Request::call(call, "send", params))
}

pub(crate) fn delivery_request(
    channel: &str,
    call: u64,
    delivery: &state::Delivery,
) -> Result<Request, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(message_id) = delivery.message_id.as_deref() {
        Ok(Request::call(
            call,
            "edit",
            edit_params(
                channel,
                &delivery.id,
                &delivery.chat,
                message_id,
                &delivery.text,
                delivery.thread.as_deref(),
            )?,
        ))
    } else {
        send_request(
            channel,
            call,
            &delivery.id,
            &delivery.chat,
            delivery.private,
            &delivery.text,
            delivery.thread.as_deref(),
        )
    }
}

pub(crate) fn edit_params(
    channel: &str,
    delivery: &str,
    chat: &str,
    message: &str,
    text: &str,
    thread: Option<&str>,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    if channel == "discord" {
        return Ok(serde_json::json!({
            "delivery": delivery,
            "channel": chat,
            "message": message,
            "format": "markdown", "text": text,
        }));
    }

    if channel != "telegram" {
        return Err("This channel does not support message edits.".into());
    }

    let mut params = serde_json::json!({
        "delivery": delivery,
        "chat": chat.parse::<i64>().map_err(|_| "Telegram chat ID was invalid.")?,
        "message": message.parse::<i64>().map_err(|_| "Telegram message ID was invalid.")?,
        "format": "markdown", "text": text,
    });

    if let Some(thread) = thread {
        params["thread"] = serde_json::Value::String(thread.into());
    }

    Ok(params)
}

pub(crate) fn status(
    sessions: &Arc<Mutex<state::Store>>,
    id: &str,
    value: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sessions.lock().map_err(|_| state::POISONED)?.set_status(id, value)?;
    Ok(())
}

pub(crate) fn status_if_present(
    sessions: &Arc<Mutex<state::Store>>,
    id: &str,
    value: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut store = sessions.lock().map_err(|_| state::POISONED)?;

    if store.sessions.contains_key(id) {
        store.set_status(id, value)?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn commit_event(
    channel_process: &Live,
    sessions: &Arc<Mutex<state::Store>>,
    channel: &str,
    id: &str,
    offset: Option<i64>,
    current: &mut i64,
    gateway_sequence: Option<u64>,
    call: &mut u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    commit_event_inner(
        channel_process,
        sessions,
        channel,
        id,
        offset,
        current,
        gateway_sequence,
        call,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn commit_retained_event(
    channel_process: &Live,
    sessions: &Arc<Mutex<state::Store>>,
    channel: &str,
    id: &str,
    offset: Option<i64>,
    current: &mut i64,
    gateway_sequence: Option<u64>,
    call: &mut u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    commit_event_inner(
        channel_process,
        sessions,
        channel,
        id,
        offset,
        current,
        gateway_sequence,
        call,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn commit_event_inner(
    channel_process: &Live,
    sessions: &Arc<Mutex<state::Store>>,
    channel: &str,
    id: &str,
    offset: Option<i64>,
    current: &mut i64,
    gateway_sequence: Option<u64>,
    call: &mut u64,
    retained: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (was_retained, acknowledgement_complete) = {
        let mut store = sessions.lock().map_err(|_| state::POISONED)?;
        let was_retained = store.has_deferred_event(channel, id);
        let acknowledgement_complete = was_retained && store.deferred_acknowledged(channel, id);

        if retained {
            store.commit_retained(channel, id, offset)?;
        } else {
            store.commit(channel, id, offset)?;
        }

        (was_retained, acknowledgement_complete)
    };

    if let Some(next) = offset {
        *current = (*current).max(next);
    }

    if let Some(sequence) = gateway_sequence.filter(|_| !was_retained || !acknowledgement_complete)
    {
        let mut attempts = 0;

        loop {
            let response = channel_process
                .call(Request::call(*call, "ack", serde_json::json!({"sequence": sequence})))
                .await;

            *call += 1;

            match response {
                Ok(response) if response.error.is_none() && response.result.is_some() => {
                    sessions
                        .lock()
                        .map_err(|_| state::POISONED)?
                        .mark_deferred_acknowledged(channel, id)?;
                    break;
                }

                Ok(response) => {
                    let message = response.error.map_or_else(
                        || "Channel acknowledgement was empty.".into(),
                        |error| {
                            format!("Channel acknowledgement failed: {}", sentence(error.message))
                        },
                    );

                    warn!(error = %diagnostic(&message), "Channel acknowledgement failed.");
                }

                Err(error) => warn!(
                    error = %diagnostic(sentence(error.to_string())),
                    "Channel acknowledgement transport failed."
                ),
            }

            attempts += 1;

            if attempts >= ACK_ATTEMPTS {
                return Err("Channel acknowledgement failed after recovery attempts.".into());
            }

            tokio::time::sleep(retry_delay(attempts)).await;
            channel_process.restart().await?;
            info!("Channel plugin restarted after an acknowledgement failure.");
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_callback(
    channel: &Live,
    channel_id: &str,
    event: &serde_json::Value,
    policy: &ChannelConfig,
    mode: ApprovalMode,
    approvals: &Arc<AsyncMutex<approval::Gate>>,
    sessions: &Arc<Mutex<state::Store>>,
    offset: &mut i64,
    call: &mut u64,
    advance_offset: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let id = event_id(&event["id"]).ok_or("Callback event has no ID.")?;

    let next_offset = event["id"].as_i64().map(|value| value.saturating_add(1));
    let gateway_sequence = event["gateway_sequence"].as_u64();

    if sessions.lock().map_err(|_| state::POISONED)?.known(channel_id, &id) {
        let offset_is_ready = next_offset.is_some_and(|next| next <= offset.saturating_add(1));

        if advance_offset {
            return commit_event(
                channel,
                sessions,
                channel_id,
                &id,
                offset_is_ready.then_some(next_offset).flatten(),
                offset,
                gateway_sequence,
                call,
            )
            .await;
        }

        return Ok(());
    }

    let commit_offset = advance_offset.then_some(next_offset).flatten();

    let Some(chat) = event_id(&event["chat"]) else {
        return commit_event(
            channel,
            sessions,
            channel_id,
            &id,
            commit_offset,
            offset,
            gateway_sequence,
            call,
        )
        .await;
    };

    let Some(callback_id) = event["callback_id"]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= META_LIMIT)
    else {
        return commit_event(
            channel,
            sessions,
            channel_id,
            &id,
            commit_offset,
            offset,
            gateway_sequence,
            call,
        )
        .await;
    };

    let thread = event_id(&event["thread"]);
    let token = event["data"].as_str().filter(|value| value.len() <= 64).unwrap_or_default();
    let mut authority = event.clone();

    if let Some(mention) = policy.mention.as_deref() {
        authority["text"] = serde_json::Value::String(mention.into());
    }

    let authorized = tools_allowed(policy, &authority, &chat, mode.enabled());

    let decision =
        approvals.lock().await.resolve(token, channel_id, &chat, thread.as_deref(), authorized);

    let message = match decision {
        Some(true) => "Approval accepted.",
        Some(false) => "The request was denied.",
        None => "This approval is invalid or expired.",
    };

    let callback = channel
        .call(Request::call(
            *call,
            "callback",
            serde_json::json!({"id": callback_id, "text": message}),
        ))
        .await;

    *call = (*call).saturating_add(1);

    if let Err(error) = callback {
        warn!(error = %diagnostic(sentence(error.to_string())), "Channel approval acknowledgement failed.");
    }

    commit_event(channel, sessions, channel_id, &id, commit_offset, offset, gateway_sequence, call)
        .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn queue_during_turn(
    channel: &Live,
    channel_id: &str,
    model: &str,
    event: &serde_json::Value,
    policy: &ChannelConfig,
    sessions: &Arc<Mutex<state::Store>>,
    offset: &mut i64,
    call: &mut u64,
    control: Option<&jobs::Signals>,
    host: Option<(&slash::Host<'_>, ApprovalMode)>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let Some(id) = event_id(&event["id"]) else {
        return Ok(true);
    };

    let next_offset = event["id"].as_i64().map(|value| value.saturating_add(1));
    let gateway_sequence = event["gateway_sequence"].as_u64();

    if id.len() > META_LIMIT
        || event["text"].as_str().is_some_and(|value| value.len() > TEXT_LIMIT)
        || event["sender"].as_str().is_some_and(|value| value.len() > META_LIMIT)
        || event["roles"].as_array().is_some_and(|roles| {
            roles.len() > CONTENT_LIMIT
                || roles
                    .iter()
                    .any(|role| role.as_str().is_some_and(|value| value.len() > META_LIMIT))
        })
    {
        commit_event(
            channel,
            sessions,
            channel_id,
            &id,
            next_offset,
            offset,
            gateway_sequence,
            call,
        )
        .await?;

        return Ok(true);
    }

    let retained =
        sessions.lock().map_err(|_| state::POISONED)?.has_deferred_event(channel_id, &id);

    if !retained && sessions.lock().map_err(|_| state::POISONED)?.known(channel_id, &id) {
        commit_event(
            channel,
            sessions,
            channel_id,
            &id,
            next_offset,
            offset,
            gateway_sequence,
            call,
        )
        .await?;

        return Ok(true);
    }

    let Some(chat) = event["chat"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| event["chat"].as_i64().map(|value| value.to_string()))
    else {
        commit_event(
            channel,
            sessions,
            channel_id,
            &id,
            next_offset,
            offset,
            gateway_sequence,
            call,
        )
        .await?;
        return Ok(true);
    };

    let thread = event_id(&event["thread"]);

    if chat.len() > META_LIMIT
        || thread.as_ref().is_some_and(|value| value.len() > META_LIMIT)
        || !allowed(policy, event, &chat)
    {
        commit_event(
            channel,
            sessions,
            channel_id,
            &id,
            next_offset,
            offset,
            gateway_sequence,
            call,
        )
        .await?;
        return Ok(true);
    }

    let content = content(event);

    if content.is_empty() {
        commit_event(
            channel,
            sessions,
            channel_id,
            &id,
            next_offset,
            offset,
            gateway_sequence,
            call,
        )
        .await?;

        return Ok(true);
    }

    let session = session_id(channel_id, &chat, thread.as_deref());

    if !valid(&session) {
        commit_event(
            channel,
            sessions,
            channel_id,
            &id,
            next_offset,
            offset,
            gateway_sequence,
            call,
        )
        .await?;

        return Ok(true);
    }

    let command = match host {
        Some((host, mode)) => slash::channel(host, channel_id, event, policy, mode).await?,
        None => slash::Command::Pass,
    };

    let (commanded, content) = match command {
        slash::Command::Pass => (false, content),
        slash::Command::Handled => (true, content),
        slash::Command::Ask(question) => (false, vec![Content::Text { text: question }]),
    };

    let controlled = if commanded {
        true
    } else if let Some(signals) = control {
        jobs::dialog::channel(
            &mut *sessions.lock().map_err(|_| state::POISONED)?,
            signals,
            channel_id,
            event,
        )?
    } else {
        false
    };

    if controlled {
        commit_event(
            channel,
            sessions,
            channel_id,
            &id,
            next_offset,
            offset,
            gateway_sequence,
            call,
        )
        .await?;

        return Ok(true);
    }

    let roles = event["roles"]
        .as_array()
        .into_iter()
        .flatten()
        .take(CONTENT_LIMIT)
        .filter_map(event_id)
        .collect::<Vec<_>>();

    let mut message = Message {
        id: id.clone(),
        session: session.clone(),
        role: Role::User,
        sender: event["sender"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| event["sender"].as_i64().map(|value| value.to_string())),
        content,
    };

    let mut rejected = false;
    let mut deferred = false;

    {
        let mut store = sessions.lock().map_err(|_| state::POISONED)?;

        if !store.sessions.contains_key(&session) && store.sessions.len() >= state::LIMIT {
            deferred = true;
        } else {
            if !store.sessions.contains_key(&session)
                && policy.worktree
                && let Err(error) = isolate(&session)
            {
                warn!(
                    event = %id,
                    error = %diagnostic(sentence(error.to_string())),
                    "Conversation workspace isolation failed."
                );
                workspace_failure(&mut store, channel_id, &id, &chat, thread.clone())?;
                rejected = true;
            }

            if !rejected {
                if let Err(error) = store.ensure_channel(&session, model) {
                    if error.kind() == std::io::ErrorKind::WouldBlock {
                        deferred = true;
                    } else {
                        return Err(error.into());
                    }
                }

                if !deferred {
                    let isolated = if policy.worktree {
                        Some(worktree::conversation(&session, &home())?)
                    } else {
                        None
                    };

                    if policy.worktree {
                        store.storage(&session, isolated.as_deref(), true)?;
                    }

                    store.route(
                        &session,
                        channel_id,
                        &chat,
                        thread.as_deref(),
                        event["private"] == true,
                    )?;

                    reference(&mut store, &session, thread.as_deref(), event, &mut message)?;

                    if let Err(error) = store.queue_with_roles(&session, message, roles) {
                        if error.kind() == std::io::ErrorKind::WouldBlock {
                            deferred = true;
                        } else {
                            return Err(error.into());
                        }
                    }

                    if !deferred {
                        debug!(channel = channel_id, "Inbound message queued for processing.");
                    }
                }
            }
        }
    }

    if deferred {
        if retained {
            return Ok(false);
        }

        let result =
            sessions.lock().map_err(|_| state::POISONED)?.defer_event(channel_id, event.clone());

        if let Err(error) = result {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                warn!(event = %id, "Message remains uncommitted because deferred storage is full.");
                return Ok(false);
            }

            return Err(error.into());
        }

        warn!(event = %id, "Message was retained because the session queue is full.");
        commit_retained_event(
            channel,
            sessions,
            channel_id,
            &id,
            next_offset,
            offset,
            gateway_sequence,
            call,
        )
        .await?;

        return Ok(true);
    }

    if rejected {
        warn!(event = %id, "Message was rejected because the session queue is unavailable.");
    }

    commit_event(channel, sessions, channel_id, &id, next_offset, offset, gateway_sequence, call)
        .await?;

    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn wait_approval(
    channel: &Live,
    channel_id: &str,
    chat: &str,
    thread: Option<&str>,
    text: &str,
    approve: &str,
    deny: &str,
    approvals: &Arc<AsyncMutex<approval::Gate>>,
    call: &mut u64,
    deadline: tokio::time::Instant,
    cancel: &Cancellation,
    stop: &Stop,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(mut completion) = approvals.lock().await.subscribe(approve) else {
        return Ok(());
    };

    let params = approval_params(channel_id, chat, thread, text, approve, deny)?;
    let sent = channel.call(Request::call(*call, "approval", params)).await;

    *call = (*call).saturating_add(1);

    match sent {
        Ok(response) if response.error.is_none() && response.result.is_some() => {}

        Ok(response) => {
            approvals.lock().await.cancel(approve);

            return Err(response
                .error
                .map_or_else(
                    || "The channel rejected the approval request.".into(),
                    |error| error.message,
                )
                .into());
        }

        Err(error) => {
            approvals.lock().await.cancel(approve);
            return Err(error.into());
        }
    }

    tokio::select! {
        biased;
        _ = stop.notified() => Err("The turn was interrupted.".into()),
        _ = cancel.cancelled() => Err("The turn was cancelled.".into()),
        _ = completion.changed() => Ok(()),
        _ = tokio::time::sleep_until(deadline) => Err("The approval request expired.".into()),
    }
}
