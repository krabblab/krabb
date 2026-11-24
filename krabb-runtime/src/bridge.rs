//! Relays channel events into turns and replies back to their channel.

use crate::*;

#[allow(clippy::too_many_arguments)]
#[cfg(all(test, unix))]
pub(crate) async fn bridge(
    plugins: &Plugins,
    channel_id: &str,
    model_id: &str,
    model: &str,
    sessions: Arc<Mutex<state::Store>>,
    stop: Arc<Stop>,
    cancels: Arc<Mutex<BTreeMap<String, Arc<Cancellation>>>>,
    channels: BTreeMap<String, ChannelConfig>,
    approval_mode: ApprovalMode,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = std::env::var_os("KRABB_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| tests::test_root("bridge-home"));
    init_at(&home)?;

    bridge_with_media(
        plugins,
        channel_id,
        model_id,
        model,
        sessions,
        stop,
        cancels,
        channels,
        approval_mode,
        false,
        home.clone(),
        home.join("workspace"),
        media_root_at(&home),
        Arc::new(AsyncMutex::new(approval::Gate::new()?)),
        10,
    )
    .await
}

pub(crate) fn media_root_at(home: &Path) -> PathBuf {
    std::env::var_os("KRABB_MEDIA")
        .map(PathBuf::from)
        .unwrap_or_else(|| krabb_file::location(home, "data/media", "media"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn bridge_with_media(
    plugins: &Plugins,
    channel_id: &str,
    model_id: &str,
    model: &str,
    sessions: Arc<Mutex<state::Store>>,
    stop: Arc<Stop>,
    cancels: Arc<Mutex<BTreeMap<String, Arc<Cancellation>>>>,
    channels: BTreeMap<String, ChannelConfig>,
    approval_mode: ApprovalMode,
    shell_enabled: bool,
    home_root: PathBuf,
    conversation_root: PathBuf,
    media_root: PathBuf,
    approvals: Arc<AsyncMutex<approval::Gate>>,
    mut call: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    std::fs::create_dir_all(&conversation_root)?;
    let mut offset = sessions.lock().map_err(|_| state::POISONED)?.offset(channel_id);
    sessions.lock().map_err(|_| state::POISONED)?.media_root = Some(media_root.clone());
    let mut channel_failures = 0_u32;
    let mut provider_failures = 0_u32;

    loop {
        let lifecycle = plugins.lifecycle.read().await;
        let Some(channel) =
            plugins.get(channel_id).await.filter(|plugin| plugin.supports(Capability::Channel))
        else {
            drop(lifecycle);
            tokio::select! {
                biased;
                _ = tokio::signal::ctrl_c() => return Ok(()),
                _ = stop.notified() => return Ok(()),
                _ = plugins.changed.notified() => continue,
                _ = tokio::time::sleep(Duration::from_millis(250)) => continue,
            }
        };

        let (selected_provider, _) = plugins.channel_selection(channel_id, model_id, model)?;

        let provider_ready = plugins
            .get(&selected_provider)
            .await
            .is_some_and(|plugin| plugin.supports(Capability::Model));

        let delivery_channels = plugins
            .all()
            .await
            .into_iter()
            .filter(|(_, plugin)| plugin.supports(Capability::Channel))
            .collect::<Vec<_>>();
        let pending = {
            let mut store = sessions.lock().map_err(|_| state::POISONED)?;
            let delivery = store
                .outbox
                .iter()
                .find(|delivery| {
                    delivery.status == state::DeliveryStatus::Pending
                        && delivery_channels.iter().any(|(id, _)| id == &delivery.channel)
                })
                .cloned();

            match delivery {
                Some(delivery) => {
                    store.sending(&delivery.id)?;
                    channels::intake::request(&mut store, &delivery.channel, call, &delivery)?
                        .map(|request| (delivery, request))
                }

                None => None,
            }
        };

        if let Some((delivery, request)) = pending {
            let Some((delivery_channel_id, delivery_channel)) =
                delivery_channels.iter().find(|(id, _)| id == &delivery.channel)
            else {
                continue;
            };

            let sent = tokio::select! {
                biased;
                _ = tokio::signal::ctrl_c() => return Ok(()),
                _ = stop.notified() => return Ok(()),
                result = delivery_channel.call(request) => result,
            };

            call += 1;

            match sent {
                Ok(response) => {
                    let mut store = sessions.lock().map_err(|_| state::POISONED)?;

                    match channels::intake::settle(&mut store, &delivery.id, response)? {
                        None => {
                            debug!(channel = delivery_channel_id, "Channel delivery acknowledged.");
                        }

                        Some(message) => {
                            warn!(channel = delivery_channel_id, error = %diagnostic(&message), "Channel delivery is uncertain.");
                        }
                    }
                }

                Err(error) => {
                    let message = sentence(error.to_string());
                    warn!(channel = delivery_channel_id, error = %diagnostic(&message), "Channel delivery is uncertain.");
                    sessions
                        .lock()
                        .map_err(|_| state::POISONED)?
                        .uncertain(&delivery.id, message)?;

                    if !recover(
                        delivery_channel,
                        &mut channel_failures,
                        delivery_channel_id,
                        stop.as_ref(),
                    )
                    .await?
                    {
                        return Ok(());
                    }
                }
            }

            continue;
        }

        let queued = if provider_ready {
            let store = sessions.lock().map_err(|_| state::POISONED)?;
            store
                .sessions
                .values()
                .filter(|session| {
                    session.channel.as_deref() == Some(channel_id)
                        && session.chat.is_some()
                        && session.status != "cancelled"
                        && !session.has_live_tui_reservation()
                        && !session.queued.is_empty()
                })
                .find_map(|session| {
                    session.queued.first().cloned().map(|mut message| {
                        message.session = session.id.clone();
                        let roles =
                            session.queue_roles.get(&message.id).cloned().unwrap_or_default();
                        (
                            message,
                            session.chat.clone().unwrap_or_default(),
                            session.thread.clone(),
                            session.private,
                            roles,
                        )
                    })
                })
        } else {
            None
        };

        let deferred = if provider_ready {
            sessions.lock().map_err(|_| state::POISONED)?.deferred_event(channel_id)
        } else {
            None
        };

        let response = if let Some((message, chat, thread, private, roles)) = queued.as_ref() {
            channel_failures = 0;
            let text = message.content.iter().map(Content::render).collect::<Vec<_>>().join("\n");
            Response::ok(
                call,
                serde_json::json!({
                    "events": [{
                        "id": message.id,
                        "chat": chat,
                        "private": private,
                        "sender": message.sender,
                        "roles": roles,
                        "text": text,
                        "content": message.content,
                        "thread": thread,
                        "topic": thread,
                    }]
                }),
            )
        } else if let Some(event) = deferred {
            Response::ok(call, serde_json::json!({"events": [event]}))
        } else {
            let polled = {
                let poll = channel.call(Request::call(
                    call,
                    "poll",
                    serde_json::json!({"offset": offset}),
                ));
                tokio::pin!(poll);
                tokio::select! {
                    biased;
                    _ = tokio::signal::ctrl_c() => return Ok(()),
                    _ = stop.notified() => return Ok(()),
                    result = tokio::time::timeout(
                        std::time::Duration::from_secs(45),
                        &mut poll,
                    ) => match result {
                        Ok(Ok(response)) => Ok(response),
                        Ok(Err(error)) => Err(format!("Channel polling failed: {}", sentence(error.to_string()))),
                        Err(_) => Err("Channel polling timed out.".into()),
                    },
                }
            };

            match polled {
                Ok(response) => response,

                Err(message) => {
                    warn!(channel = channel_id, error = %diagnostic(&message), "Channel polling failed.");

                    drop(lifecycle);

                    if !recovery::channel(
                        plugins,
                        channel_id,
                        &channel,
                        &mut channel_failures,
                        &stop,
                    )
                    .await
                    {
                        return Ok(());
                    }

                    continue;
                }
            }
        };

        call += 1;

        let Some(value) = response.result else {
            if let Some(error) = response.error {
                warn!(channel = channel_id, error = %diagnostic(sentence(error.message)), "Channel polling failed.");
            }

            drop(lifecycle);

            if !recovery::channel(plugins, channel_id, &channel, &mut channel_failures, &stop).await
            {
                return Ok(());
            }

            continue;
        };

        channel_failures = 0;

        let events = value["events"].as_array().cloned().unwrap_or_default();
        let channel_policy = channels.get(channel_id).cloned().unwrap_or_default();

        for mut event in events {
            let Some(event_key) = event_id(&event["id"]) else {
                continue;
            };

            let (selected_provider, selected_model) =
                plugins.channel_selection(channel_id, model_id, model)?;

            let gateway_sequence = event["gateway_sequence"].as_u64();
            let next_offset = event["id"].as_i64().map(|value| value.saturating_add(1));

            if event["kind"] == "callback" {
                handle_callback(
                    &channel,
                    channel_id,
                    &event,
                    &channel_policy,
                    approval_mode,
                    &approvals,
                    &sessions,
                    &mut offset,
                    &mut call,
                    true,
                )
                .await?;
                continue;
            }

            if event_key.len() > META_LIMIT {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            let queued_event = queued.as_ref().is_some_and(|(queued, ..)| queued.id == event_key);

            let retained_event = sessions
                .lock()
                .map_err(|_| state::POISONED)?
                .has_deferred_event(channel_id, &event_key);

            if !queued_event
                && !retained_event
                && sessions.lock().map_err(|_| state::POISONED)?.known(channel_id, &event_key)
            {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            let Some(chat) = event["chat"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| event["chat"].as_i64().map(|value| value.to_string()))
            else {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            };

            if chat.len() > META_LIMIT {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            let thread = event_id(&event["thread"]);

            if thread.as_ref().is_some_and(|value| value.len() > META_LIMIT) {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            if event["topic"].as_str().is_some_and(|value| value.len() > META_LIMIT)
                || event["text"].as_str().is_some_and(|value| value.len() > TEXT_LIMIT)
            {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            if event["sender"].as_str().is_some_and(|value| value.len() > META_LIMIT)
                || event["roles"].as_array().is_some_and(|roles| {
                    roles.len() > CONTENT_LIMIT
                        || roles
                            .iter()
                            .any(|role| role.as_str().is_some_and(|value| value.len() > META_LIMIT))
                })
            {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            // Queue entries already passed channel policy; preserve that decision across turns.
            if !queued_event && !allowed(&channel_policy, &event, &chat) {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            let session_name = queued.as_ref().filter(|_| queued_event).map_or_else(
                || session_id(channel_id, &chat, thread.as_deref()),
                |(message, ..)| message.session.clone(),
            );

            if !valid(&session_name) {
                warn!(channel = channel_id, event = %event_key, "Message has invalid routing metadata.");
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            let host = slash::Host {
                plugins,
                sessions: &sessions,
                cancels: &cancels,
                approvals: &approvals,
                stop: &stop,
                provider: &selected_provider,
                model: &selected_model,
                call,
                answering: false,
                home: Some(&home_root),
            };

            // Summarizing can take a while, so other chats keep being received meanwhile.
            let summarizing = event["text"]
                .as_str()
                .and_then(krabb_core::slash::parse)
                .is_some_and(|invocation| invocation.spec.lengthy());

            // Committed first so the intake does not receive it again;
            // a lost summary changes nothing.
            if summarizing {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
            }

            let intake = summarizing.then(|| {
                channels::intake::Intake::start(channels::intake::Context {
                    channel: Arc::clone(&channel),
                    id: channel_id.into(),
                    model: selected_model.clone(),
                    policy: channel_policy.clone(),
                    mode: approval_mode,
                    sessions: Arc::clone(&sessions),
                    approvals: Arc::clone(&approvals),
                    signals: Arc::clone(&plugins.jobs),
                    shutdown: Arc::clone(&stop),
                    offset,
                    call: call.saturating_add(channels::INTAKE_CALLS),
                    plugins: plugins.clone(),
                    cancels: Arc::clone(&cancels),
                    provider: selected_provider.clone(),
                    home: home_root.clone(),
                })
            });

            let command =
                slash::channel(&host, channel_id, &event, &channel_policy, approval_mode).await;

            // Lengthy commands always answer, and their event is already committed; the intake
            // finishes before any error propagates so it never outlives this loop.
            if let Some(intake) = intake {
                offset = channels::intake::finish(intake, &sessions, channel_id).await?;
                command?;
                call = call.saturating_add(1);
                continue;
            }

            let commanded = match command? {
                slash::Command::Pass => false,
                slash::Command::Handled => true,

                slash::Command::Ask(question) => {
                    slash::reask(&mut event, question);
                    false
                }
            };

            if commanded {
                call = call.saturating_add(1);
            }

            let controlled = commanded
                || jobs::dialog::channel(
                    &mut *sessions.lock().map_err(|_| state::POISONED)?,
                    &plugins.jobs,
                    channel_id,
                    &event,
                )?;

            if controlled {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            let Some(provider) = plugins
                .get(&selected_provider)
                .await
                .filter(|plugin| plugin.supports(Capability::Model))
            else {
                if !queue_during_turn(
                    &channel,
                    channel_id,
                    &selected_model,
                    &event,
                    &channel_policy,
                    &sessions,
                    &mut offset,
                    &mut call,
                    Some(&plugins.jobs),
                    None,
                )
                .await?
                {
                    tokio::select! {
                        biased;
                        _ = stop.notified() => return Ok(()),

                        _ = plugins.changed.notified() => {}

                        _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                    }

                    break;
                }

                continue;
            };

            let accepts_workspace = {
                let store = sessions.lock().map_err(|_| state::POISONED)?;
                store
                    .sessions
                    .get(&session_name)
                    .map_or(store.sessions.len() < state::LIMIT, |session| {
                        session.status != "cancelled"
                    })
            };

            let workspace = if !accepts_workspace {
                None
            } else if channel_policy.worktree {
                match worktree::conversation(&session_name, &home_root) {
                    Ok(path) => Some(path),

                    Err(error) => {
                        warn!(error = %diagnostic(error.to_string()), "Conversation workspace isolation failed.");
                        workspace_failure(
                            &mut *sessions.lock().map_err(|_| state::POISONED)?,
                            channel_id,
                            &event_key,
                            &chat,
                            thread.clone(),
                        )?;
                        commit_event(
                            &channel,
                            &sessions,
                            channel_id,
                            &event_key,
                            next_offset,
                            &mut offset,
                            gateway_sequence,
                            &mut call,
                        )
                        .await?;
                        continue;
                    }
                }
            } else {
                sharing::workspace(&conversation_root)?
            };

            let conversation_media = workspace
                .as_deref()
                .map(|workspace| worktree::media(workspace, channel_id))
                .transpose()?;

            let active_media = conversation_media.as_deref().unwrap_or(&media_root);
            let resolved = resolve_media(channel_id, &channel, content(&event), &mut call).await;
            let admitted = {
                let store = sessions.lock().map_err(|_| state::POISONED)?;
                let candidate = Message {
                    id: event_key.clone(),
                    session: session_name.clone(),
                    role: Role::User,
                    sender: None,
                    content: Vec::new(),
                };

                (!store.sessions.contains_key(&session_name) && store.sessions.len() < state::LIMIT)
                    || media::admitted(&store, &candidate, queued_event)
            };

            let archived = if admitted {
                media::archive(resolved.clone(), &media_root, active_media)?
            } else {
                resolved.clone()
            };

            if admitted
                && let Some(workspace) =
                    workspace.as_deref().filter(|path| !worktree::isolated(path))
            {
                storage::attachments(&home_root, workspace, channel_id, &archived)?;
            }

            let voice = archived.iter().any(|item| matches!(item, Content::Audio { .. }));
            let content = expand_media_files(archived, active_media).await;
            let content =
                transcribe_media(plugins, channel_id, content, active_media, &mut call).await;

            // Transcription deletes raw voice files, so their ownership entries go with them.
            if voice
                && let Some(workspace) =
                    workspace.as_deref().filter(|path| !worktree::isolated(path))
            {
                storage::prune(&home_root, workspace);
            }

            if content.is_empty() {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            let mut message = Message {
                id: event_key.clone(),
                session: session_name.clone(),
                role: Role::User,
                sender: event["sender"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| event["sender"].as_i64().map(|id| id.to_string())),
                content,
            };

            let roles = event["roles"]
                .as_array()
                .into_iter()
                .flatten()
                .take(CONTENT_LIMIT)
                .filter_map(event_id)
                .collect::<Vec<_>>();

            let mut retry_message = message.clone();
            let event_id = message.id.clone();
            let (workspace, prepared, commit, defer) = {
                let mut store = sessions.lock().map_err(|_| state::POISONED)?;
                let mut commit = false;
                let mut defer = false;
                let workspace = if store
                    .sessions
                    .get(&session_name)
                    .is_some_and(|session| session.status == "cancelled")
                {
                    commit = true;
                    None
                } else if !store.sessions.contains_key(&session_name)
                    && store.sessions.len() >= state::LIMIT
                {
                    warn!(session = %session_name, event = %event_id, "Session capacity reached, message deferred.");
                    defer = true;
                    None
                } else {
                    workspace.clone()
                };

                let prepared = if commit {
                    None
                } else if let Err(error) = store.ensure_channel(&session_name, &selected_model) {
                    if error.kind() == std::io::ErrorKind::WouldBlock {
                        warn!(session = %session_name, event = %event_id, "Session capacity reached, message deferred.");
                        defer = true;
                        None
                    } else {
                        error!(session = %session_name, error = %diagnostic(sentence(error.to_string())), "Session creation failed.");
                        break;
                    }
                } else if let Err(error) =
                    store.storage(&session_name, workspace.as_deref(), channel_policy.worktree)
                {
                    return Err(error.into());
                } else if let Err(error) = store.route(
                    &session_name,
                    channel_id,
                    &chat,
                    thread.as_deref(),
                    event["private"] == true,
                ) {
                    error!(session = %session_name, error = %diagnostic(sentence(error.to_string())), "Message routing failed.");
                    break;
                } else {
                    if !queued_event {
                        reference(
                            &mut store,
                            &session_name,
                            thread.as_deref(),
                            &event,
                            &mut message,
                        )?;

                        retry_message = message.clone();
                    }

                    if provider.supports(Capability::Vision)
                        && media::admitted(&store, &message, queued_event)
                    {
                        message.content =
                            media::pin(std::mem::take(&mut message.content), active_media);
                        retry_message = message.clone();
                    }

                    let existing = store.sessions.get(&session_name).is_some_and(|session| {
                        session.messages.iter().any(|item| item.id == message.id)
                    });

                    if !queued_event
                        && store.sessions.get(&session_name).is_some_and(|session| {
                            existing || session.queued.iter().any(|item| item.id == message.id)
                        })
                    {
                        None
                    } else if !queued_event
                        && store
                            .sessions
                            .get(&session_name)
                            .is_some_and(|session| session.status == "working")
                    {
                        if let Err(error) =
                            store.queue_with_roles(&session_name, message, roles.clone())
                        {
                            if error.kind() == std::io::ErrorKind::WouldBlock {
                                warn!(session = %session_name, event = %event_id, "Session queue is full, message deferred.");
                                defer = true;
                                None
                            } else {
                                error!(
                                    session = %session_name,
                                    error = %diagnostic(sentence(error.to_string())),
                                    "Message queueing failed."
                                );
                                break;
                            }
                        } else {
                            None
                        }
                    } else {
                        let prepared = store.sessions.get(&session_name).map(|session| {
                            let mut history = session.context();

                            if !existing {
                                history.push(message.clone());

                                state::compact_messages(&mut history);
                            }

                            (history, session.model.clone())
                        });

                        if prepared.is_some()
                            && let Err(error) = if queued_event {
                                store.begin_queued_with_roles(
                                    &session_name,
                                    message.clone(),
                                    roles.clone(),
                                    channel_id,
                                )
                            } else {
                                store.begin_with_roles(
                                    &session_name,
                                    message.clone(),
                                    roles.clone(),
                                )
                            }
                        {
                            error!(
                                session = %session_name,
                                error = %diagnostic(sentence(error.to_string())),
                                "Session turn could not start."
                            );
                            break;
                        }

                        prepared
                    }
                };

                (workspace, prepared, commit, defer)
            };

            if defer {
                if retained_event {
                    tokio::select! {
                        biased;
                        _ = tokio::signal::ctrl_c() => return Ok(()),
                        _ = stop.notified() => return Ok(()),

                        _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
                    }

                    continue;
                }

                let deferred = {
                    let mut store = sessions.lock().map_err(|_| state::POISONED)?;
                    store.defer_event(channel_id, event.clone())
                };

                if let Err(error) = deferred {
                    if error.kind() == std::io::ErrorKind::WouldBlock {
                        warn!(
                            session = %session_name,
                            event = %event_id,
                            "Message remains uncommitted because deferred storage is full."
                        );
                        break;
                    }

                    return Err(error.into());
                }

                commit_retained_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            if commit {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            }

            let Some((history, session_model)) = prepared else {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
                continue;
            };

            media::release(&resolved, &media_root, active_media);
            let mut messages = turn_messages(&session_name, &home_root, history);

            if !channel_policy.worktree {
                context::append(
                    &mut messages,
                    &*sessions.lock().map_err(|_| state::POISONED)?,
                    &session_name,
                    &sharing::sources(
                        &plugins.config,
                        &sharing::principal(channel_id),
                        sharing::Data::Context,
                    ),
                    &plugins.config,
                );
            }

            let source = sharing::Principal::Channel(channel_id.into());
            let memory_scope = origin::scope(&source, &session_name, &chat, thread.as_deref());
            append_conversation_memory(
                plugins,
                &mut messages,
                &memory_scope,
                &source,
                channel_policy.worktree.then_some(workspace.as_deref()).flatten(),
                &mut call,
            )
            .await;

            if !queued_event {
                commit_event(
                    &channel,
                    &sessions,
                    channel_id,
                    &event_key,
                    next_offset,
                    &mut offset,
                    gateway_sequence,
                    &mut call,
                )
                .await?;
            }

            let delivery_id = format!("{channel_id}-{event_id}");
            let effective_policy = channel_policy.clone();

            let turn_approval =
                sharing::approval(&plugins.config, &sharing::principal(channel_id), approval_mode);
            let channel_tools_enabled =
                tools_allowed(&effective_policy, &event, &chat, turn_approval.enabled());
            let channel_schedule_enabled = schedule_allowed(&effective_policy, &event, &chat);
            let channel_shell_enabled = !channel_policy.worktree
                && channel_shell_allowed(
                    &effective_policy,
                    &event,
                    &chat,
                    turn_approval.enabled(),
                    shell_enabled,
                );
            let start_intake = |offset, call: u64| {
                channels::intake::Intake::start(channels::intake::Context {
                    channel: Arc::clone(&channel),
                    id: channel_id.into(),
                    model: session_model.clone(),
                    policy: channel_policy.clone(),
                    mode: approval_mode,
                    sessions: Arc::clone(&sessions),
                    approvals: Arc::clone(&approvals),
                    signals: Arc::clone(&plugins.jobs),
                    shutdown: Arc::clone(&stop),
                    offset,
                    call: call.saturating_add(channels::INTAKE_CALLS),
                    plugins: plugins.clone(),
                    cancels: Arc::clone(&cancels),
                    provider: selected_provider.clone(),
                    home: home_root.clone(),
                })
            };

            let mut failed_tool = None;
            let model_result = {
                let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel(32);
                let turn_cancel = cancellation(&cancels, &session_name);
                let mut stream_call = call.saturating_add(channels::STREAM_CALLS);
                let turn_deadline =
                    tokio::time::Instant::now() + plugins.config.inference.envelope();
                let intake = start_intake(offset, call);

                let answer = answer(
                    &provider,
                    plugins,
                    &session_model,
                    messages,
                    &session_name,
                    channel_id,
                    &chat,
                    thread.as_deref(),
                    &sessions,
                    workspace.as_deref(),
                    active_media,
                    channel_tools_enabled,
                    channel_schedule_enabled,
                    channel_shell_enabled,
                    turn_approval,
                    Arc::clone(&approvals),
                    &turn_cancel,
                    &stop,
                    turn_deadline,
                    &mut call,
                    &mut failed_tool,
                    None,
                    stream_tx,
                    None,
                    None,
                    None,
                );
                tokio::pin!(answer);
                let mut output = StreamOutput::new();
                let mut typing = true;
                let mut activity = tokio::time::interval_at(
                    tokio::time::Instant::now() + Duration::from_millis(500),
                    Duration::from_secs(4),
                );
                activity.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut ticker = tokio::time::interval_at(
                    tokio::time::Instant::now() + std::time::Duration::from_millis(500),
                    std::time::Duration::from_millis(500),
                );
                let result = loop {
                    tokio::select! {
                        biased;
                        result = &mut answer => break result,

                        Some(notice) = stream_rx.recv() => {
                            match notice {
                                StreamNotice::Approval { chat: approval_chat, thread: approval_thread, text, approve, deny, deadline, .. } => {
                                    if let Err(error) = wait_approval(
                                        &channel,
                                        channel_id,
                                        &approval_chat,
                                        approval_thread.as_deref(),
                                        &text,
                                        &approve,
                                        &deny,
                                        &approvals,
                                        &mut stream_call,
                                        deadline,
                                        &turn_cancel,
                                        &stop,
                                    ).await {
                                        approvals.lock().await.cancel(&approve);
                                        warn!(error = %diagnostic(sentence(error.to_string())), "Inline approval failed.");
                                    }
                                }

                                notice => {
                                    output.apply(notice);

                                    if output.ready()
                                        && let Err(error) = flush_stream(
                                            &mut output, &channel, channel_id, &sessions, &session_name,
                                            &delivery_id, &chat, thread.as_deref(), &mut stream_call,
                                        ).await {
                                        output.disabled = true;
                                        warn!(error = %diagnostic(sentence(error.to_string())), "Channel streaming paused.");
                                    }
                                }
                            }
                        }

                        _ = ticker.tick(), if output.dirty => {
                            if let Err(error) = flush_stream(
                                &mut output, &channel, channel_id, &sessions, &session_name,
                                &delivery_id, &chat, thread.as_deref(), &mut stream_call,
                            ).await {
                                output.disabled = true;
                                warn!(error = %diagnostic(sentence(error.to_string())), "Channel streaming paused.");
                            }
                        }

                        _ = activity.tick() => {
                            if !typing {
                                continue;
                            }

                            // `answer` observes the cancellation and settles its task; dropping it
                            // here could abandon a running action before its outcome is recorded.
                            tokio::select! {
                                biased;
                                _ = stop.notified() => typing = false,
                                _ = turn_cancel.cancelled() => typing = false,
                                outcome = indicate_typing(&channel, channel_id, &chat, thread.as_deref(), &event, &mut stream_call) => typing = outcome != Typing::Unsupported,
                            }
                        }
                    }
                };

                offset = channels::intake::finish(intake, &sessions, channel_id).await?;

                result
            };

            let model = match model_result {
                Ok(model) => model,

                Err(error) => {
                    if let Some(id) = failed_tool.take()
                        && !stop.is_signaled()
                    {
                        restart_tool(plugins, &id).await;
                    }

                    if let Some(failure) = error.downcast_ref::<jobs::Failure>() {
                        {
                            let mut store = sessions.lock().map_err(|_| state::POISONED)?;
                            store.publish_job_notice(&failure.id)?;
                            store.clear(
                                &session_name,
                                if stop.is_signaled() { "interrupted" } else { "idle" },
                            )?;
                        }

                        acknowledge(&cancels, &session_name)?;

                        if !stop.is_signaled()
                            && failure::restarts(failure)
                            && !recover(&provider, &mut provider_failures, "model", stop.as_ref())
                                .await?
                        {
                            return Ok(());
                        }

                        continue;
                    }

                    let cancelled = sessions
                        .lock()
                        .map_err(|_| state::POISONED)?
                        .sessions
                        .get(&session_name)
                        .is_some_and(|session| session.status == "cancelled");

                    if cancelled || error.to_string() == "The turn was cancelled." {
                        let _ = sessions
                            .lock()
                            .map_err(|_| state::POISONED)?
                            .clear(&session_name, "cancelled");
                        acknowledge(&cancels, &session_name)?;
                        continue;
                    }

                    if error.to_string() == "The turn was interrupted." {
                        let _ = status(&sessions, &session_name, "interrupted");
                        return Ok(());
                    }

                    warn!(session = %session_name, error = %diagnostic(sentence(error.to_string())), "Model turn failed.");
                    let retry = failure::retry(error.as_ref(), provider_failures.saturating_add(1));
                    let mut queued = false;

                    {
                        let mut store = sessions.lock().map_err(|_| state::POISONED)?;
                        let safe = store
                            .sessions
                            .get(&session_name)
                            .is_none_or(|session| session.phase != "unsafe");

                        if safe && retry != failure::Retry::Never {
                            match store.queue_with_roles(&session_name, retry_message, roles) {
                                Ok(()) => queued = true,

                                Err(error) => error!(
                                    session = %session_name,
                                    error = %diagnostic(sentence(error.to_string())),
                                    "Model retry could not be queued."
                                ),
                            }
                        } else if !safe {
                            warn!(
                                session = %session_name,
                                "Failed turn contained a mutating tool, and it will not be retried."
                            );
                        }

                        let text = failure::reply(error.as_ref(), queued);

                        let failure = Message {
                            id: format!("failure-{call}"),
                            session: session_name.clone(),
                            role: Role::Assistant,
                            sender: Some(krabb_core::types::SYSTEM.into()),
                            content: vec![Content::Text { text: text.clone() }],
                        };

                        let replied = match store.reply(
                            &session_name,
                            failure,
                            &delivery_id,
                            channel_id,
                            &chat,
                            thread.clone(),
                            &text,
                        ) {
                            Ok(()) => true,

                            Err(error) => {
                                error!(
                                    session = %session_name,
                                    error = %diagnostic(sentence(error.to_string())),
                                    "Model failure could not be persisted."
                                );
                                false
                            }
                        };

                        if !replied && let Err(error) = store.clear(&session_name, "interrupted") {
                            return Err(format!(
                                "Could not clear the failed turn lease: {}",
                                sentence(error.to_string())
                            )
                            .into());
                        }
                    }

                    match retry {
                        failure::Retry::Never => provider_failures = 0,

                        failure::Retry::After(_) if !queued => provider_failures = 0,

                        failure::Retry::After(delay) => {
                            provider_failures = provider_failures.saturating_add(1);

                            let intake = start_intake(offset, call);

                            let stopped = tokio::select! {
                                biased;
                                _ = stop.notified() => true,
                                _ = tokio::time::sleep(delay) => false,
                            };

                            offset =
                                channels::intake::finish(intake, &sessions, channel_id).await?;

                            if stopped {
                                return Ok(());
                            }
                        }

                        failure::Retry::Restart => {
                            if !recover(&provider, &mut provider_failures, "model", stop.as_ref())
                                .await?
                            {
                                return Ok(());
                            }
                        }
                    }

                    continue;
                }
            };

            provider_failures = 0;
            let restart = model.restart;
            let assistant = Message {
                id: format!("assistant-{call}"),
                session: session_name.clone(),
                role: Role::Assistant,
                sender: None,
                content: vec![Content::Text { text: model.text.clone() }],
            };

            let request = {
                let mut store = sessions.lock().map_err(|_| state::POISONED)?;

                if store
                    .sessions
                    .get(&session_name)
                    .is_some_and(|session| session.status == "cancelled")
                {
                    store.clear(&session_name, "cancelled")?;
                    drop(store);
                    acknowledge(&cancels, &session_name)?;
                    continue;
                }

                store.reply_task(
                    model.foreground.as_ref().map(|foreground| foreground.run.id.as_str()),
                    &session_name,
                    assistant,
                    &delivery_id,
                    channel_id,
                    &chat,
                    thread.clone(),
                    &model.text,
                )?;
                let delivery = store
                    .outbox
                    .iter()
                    .find(|delivery| delivery.id == delivery_id)
                    .cloned()
                    .ok_or("Persisted delivery was not found.")?;

                if delivery.status == state::DeliveryStatus::Uncertain {
                    None
                } else {
                    store.sending(&delivery_id)?;
                    channels::intake::request(&mut store, channel_id, call, &delivery)?
                }
            };

            let Some(request) = request else {
                warn!(
                    session = %session_name,
                    "Channel delivery is uncertain, so retry it only after checking the channel."
                );
                continue;
            };

            let sent = tokio::select! {
                biased;

                _ = tokio::signal::ctrl_c() => {
                    let _ = status(&sessions, &session_name, "interrupted");
                    return Ok(());
                },

                _ = stop.notified() => {
                    let _ = status(&sessions, &session_name, "interrupted");
                    return Ok(());
                },

                result = channel.call(request) => match result {
                    Ok(sent) => sent,

                    Err(error) => {
                        let message = sentence(error.to_string());
                        warn!(channel = channel_id, error = %diagnostic(&message), "Channel delivery is uncertain.");
                        sessions
                            .lock()
                            .map_err(|_| state::POISONED)?
                            .uncertain(&delivery_id, message)?;
                        status_if_present(&sessions, &session_name, "idle")?;

                        if !recover(&channel, &mut channel_failures, "channel", stop.as_ref()).await? {
                            return Ok(());
                        }

                        continue;
                    }
                },
            };

            call += 1;

            let settled = {
                let mut store = sessions.lock().map_err(|_| state::POISONED)?;

                channels::intake::settle(&mut store, &delivery_id, sent)?
            };

            let delivered = settled.is_none();

            match settled {
                None => debug!(channel = channel_id, "Channel delivery acknowledged."),

                Some(message) => {
                    warn!(channel = channel_id, error = %diagnostic(&message), "Channel delivery is uncertain.");
                }
            }

            status_if_present(&sessions, &session_name, "idle")?;
            slash::settle(&mut *sessions.lock().map_err(|_| state::POISONED)?, &session_name)?;

            if restart && delivered {
                settings::restart();
            }
        }
    }
}

pub(crate) fn retry_delay(attempts: u32) -> std::time::Duration {
    let seconds = 1_u64 << attempts.min(5);
    std::time::Duration::from_secs(seconds.min(30))
}

pub(crate) fn cancellation(
    cancels: &Arc<Mutex<BTreeMap<String, Arc<Cancellation>>>>,
    session: &str,
) -> Arc<Cancellation> {
    let mut cancels = cancels.lock().expect("Cancellation lock is poisoned.");
    Arc::clone(cancels.entry(session.into()).or_insert_with(|| Arc::new(Cancellation::new())))
}

pub(crate) fn acknowledge(
    cancels: &Arc<Mutex<BTreeMap<String, Arc<Cancellation>>>>,
    session: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(cancel) =
        cancels.lock().map_err(|_| "Cancellation lock is poisoned.")?.remove(session)
    {
        cancel.acknowledge();
    }

    Ok(())
}

pub(crate) fn cancelled(
    sessions: &Arc<Mutex<state::Store>>,
    session: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sessions
        .lock()
        .map_err(|_| state::POISONED)?
        .sessions
        .get(session)
        .is_some_and(|value| value.status == "cancelled"))
}

pub(crate) async fn recover(
    process: &Live,
    failures: &mut u32,
    name: &str,
    stop: &Stop,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    loop {
        *failures = failures.saturating_add(1);
        let delay = retry_delay(*failures);

        warn!(
            plugin = name,
            attempt = *failures,
            delay_seconds = delay.as_secs(),
            "Plugin transport failed, retrying after backoff."
        );

        tokio::select! {
            biased;
            _ = stop.notified() => return Ok(false),

            _ = tokio::time::sleep(delay) => {}
        }

        let result = tokio::select! {
            biased;
            _ = stop.notified() => return Ok(false),

            result = process.restart() => result,
        };

        match result {
            Ok(()) => {
                info!(plugin = name, "Plugin restarted after a transport failure.");
                return Ok(true);
            }

            Err(error) => {
                warn!(
                    plugin = name,
                    error = %diagnostic(error.to_string()),
                    "Plugin restart failed, retrying."
                );
            }
        }
    }
}
