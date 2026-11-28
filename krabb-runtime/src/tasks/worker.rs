use super::*;

const CONCURRENCY: usize = 2;
const POLL_SECONDS: u64 = 60;
/// How long a stopping daemon waits for running steps before abandoning them.
const STOP_GRACE: Duration = Duration::from_secs(10);
/// Recent operation results replayed into each background step.
const OPERATION_HISTORY: usize = 8;

/// Standing instructions for an unattended background step.
const TASK_PROMPT: &str = concat!(
    "You are executing an authorized background goal. ",
    "There is no overall task lifetime deadline. ",
    "The host admits bounded inference and operation allowances. ",
    "Checkpoint before a limit; waiting does not extend an active allowance. ",
    "Work in concrete increments, then call task_checkpoint with completed work and the exact \
        next action before a step limit. ",
    "Use wait for external work with an appropriate delay. ",
    "Save important milestones with task_checkpoint update; do not send routine waiting messages. ",
    "Continue automatically until the goal is achieved. ",
    "Use blocked when input or permission is required. ",
    "A normal reply is an internal draft, not completion. ",
    "Finish only by calling task_checkpoint with state complete and the full result. ",
    "Omit message to publish the most recent draft from this step. ",
    "If more work remains, checkpoint and continue without another human prompt. ",
    "The host delivers updates and completion to the original conversation. ",
    "Do not create schedules or new background tasks, ",
    "and do not send messages to other recipients. ",
    "Never repeat a completed mutation; if an action's outcome is uncertain, stop as blocked.",
);

/// Whether the worker may take this task up: queued work, or a monitored operation that
/// still needs inspecting while the task stops, pauses or blocks.
fn runnable(task: &Task) -> bool {
    (matches!(task.status, TaskStatus::Queued | TaskStatus::Waiting)
        || task.observing()
            && (task.status.stopping()
                || matches!(
                    task.status,
                    TaskStatus::Paused | TaskStatus::Cancelled | TaskStatus::Blocked
                )))
        && (task.notice.is_none() || task.observing())
}

#[derive(Clone)]
pub(crate) struct Context {
    pub plugins: Plugins,
    pub sessions: Arc<Mutex<state::Store>>,
    pub stop: Arc<Stop>,
    pub home: PathBuf,
    pub config: Config,
    pub approvals: Arc<AsyncMutex<approval::Gate>>,
}

pub(crate) async fn run(context: Context) {
    let mut workers = tokio::task::JoinSet::new();

    let mut active = BTreeSet::new();
    let mut handles = HashMap::new();

    loop {
        let notified = context.plugins.tasks.wake.notified();

        tokio::pin!(notified);
        notified.as_mut().enable();

        let (ready, attached) = {
            let Ok(mut store) = context.sessions.lock() else { return };

            if let Err(error) = store.release_deferred(unix_seconds()) {
                warn!(error=%diagnostic(error.to_string()), "Deferred task release remains \
                    pending.");
            }

            if let Err(error) = store.recover_reservations() {
                debug!(error=%diagnostic(error.to_string()), "Expired client reservation recovery \
                    remains pending.");
            }

            let attached = context
                .plugins
                .tasks
                .attached
                .lock()
                .map(|attached| attached.clone())
                .unwrap_or_default();
            let orphaned: Vec<_> = store
                .tasks
                .values()
                .filter(|task| {
                    (task.status == TaskStatus::Running
                        || task.budget.as_ref().is_some_and(|budget| budget.pending.is_some())
                        || task.status.stopping()
                        || task.operation.as_ref().is_some_and(|operation| {
                            operation.phase == state::OperationPhase::Dispatching
                        }))
                        && !active.contains(&task.id)
                        && !attached.contains(&task.id)
                })
                .map(|task| task.id.clone())
                .collect();

            for id in orphaned {
                if let Err(error) = store.recover_task(&id) {
                    warn!(task = %id, %error, "Background recovery remains pending.");
                }
            }

            let notices: Vec<_> = store
                .tasks
                .values()
                .filter(|task| task.notice.is_some() && !attached.contains(&task.id))
                .map(|task| task.id.clone())
                .collect();

            for id in notices {
                if let Err(error) = store.publish_task_notice(&id) {
                    debug!(task = %id, error = %diagnostic(error.to_string()),
                        "Background notification remains pending.");
                }
            }

            let ready = store
                .tasks
                .values()
                .filter(|task| {
                    runnable(task)
                        && task.ready_at() <= unix_seconds()
                        && (!task.foreground
                            || task.observing()
                            || !store.sessions.get(&task.origin.session).is_some_and(|session| {
                                session.status == "working" || session.inflight.is_some()
                            }))
                        && !active.contains(&task.id)
                        && !attached.contains(&task.id)
                })
                .take(CONCURRENCY.saturating_sub(workers.len()))
                .map(|task| task.id.clone())
                .collect::<Vec<_>>();

            (ready, attached)
        };

        if !context.stop.is_signaled() {
            for id in ready {
                let claimed = context
                    .sessions
                    .lock()
                    .ok()
                    .and_then(|mut store| store.claim_task(&id, unix_seconds()).ok());

                if let Some(task) = claimed {
                    let worker = context.clone();

                    active.insert(id.clone());

                    let worker_id = id.clone();

                    let handle = workers.spawn(async move {
                        if let Err(error) = step(&worker, task).await {
                            warn!(task = %id, error = %diagnostic(error.to_string()),
                                "Background step could not be committed.");
                        }

                        id.clone()
                    });

                    handles.insert(handle.id(), worker_id);
                }
            }
        }

        if context.stop.is_signaled() {
            for id in drain(&mut workers, &mut handles, STOP_GRACE).await {
                warn!(task = %id, "Background step did not stop in time; it resumes from its \
                    checkpoint on the next start.");
            }

            return;
        }

        let full = workers.len() >= CONCURRENCY;
        let now = unix_seconds();

        let wait = context
            .sessions
            .lock()
            .map(|store| {
                store
                    .tasks
                    .values()
                    .filter(|task| {
                        let idle = runnable(task)
                            && !active.contains(&task.id)
                            && !attached.contains(&task.id);

                        let pending = !full || task.ready_at() > now;

                        idle && pending
                            || task.status == TaskStatus::Paused
                                && !task.uncertain
                                && task.deferred.is_some()
                    })
                    .map(|task| {
                        task.deferred
                            .as_ref()
                            .filter(|_| task.status == TaskStatus::Paused)
                            .map_or(task.ready_at(), |receipt| {
                                if task.observing() {
                                    task.ready_at().min(receipt.command.when.at)
                                } else {
                                    receipt.command.when.at
                                }
                            })
                            .saturating_sub(now)
                            .max(1)
                    })
                    .min()
                    .unwrap_or(POLL_SECONDS)
                    .min(POLL_SECONDS)
            })
            .unwrap_or(POLL_SECONDS);

        tokio::select! {
            Some(result) = workers.join_next_with_id(), if !workers.is_empty() => {
                match result {
                    Ok((handle, id)) => {
                        handles.remove(&handle);
                        active.remove(&id);
                    }

                    Err(error) => {
                        if let Some(id) = handles.remove(&error.id()) { active.remove(&id); }
                        warn!(%error, "Background worker stopped unexpectedly.");
                    }
                }
            }

            // Unguarded: a stop signaled after the check above must still end this wait, and
            // `notified` returns at once for a stop that already happened.
            _ = context.stop.notified() => {}

            _ = &mut notified, if !context.stop.is_signaled() => {}

            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
        }
    }
}

/// Waits up to GRACE for running steps after a stop, then aborts the rest and returns their
/// tasks. An aborted step is recovered from its checkpoint on the next start, as after a crash.
pub(super) async fn drain(
    workers: &mut tokio::task::JoinSet<String>,
    handles: &mut HashMap<tokio::task::Id, String>,
    grace: Duration,
) -> Vec<String> {
    let deadline = tokio::time::sleep(grace);

    tokio::pin!(deadline);

    loop {
        tokio::select! {
            joined = workers.join_next_with_id() => match joined {
                Some(Ok((handle, _))) => {
                    handles.remove(&handle);
                }

                Some(Err(error)) => {
                    handles.remove(&error.id());
                }

                None => return Vec::new(),
            },

            _ = &mut deadline => break,
        }
    }

    workers.abort_all();

    while workers.join_next().await.is_some() {}

    let mut stuck: Vec<_> = handles.drain().map(|(_, id)| id).collect();

    stuck.sort();
    stuck
}

async fn step(context: &Context, task: Task) -> Result<(), PluginError> {
    let cancel = Arc::new(Cancellation::new());

    context
        .plugins
        .tasks
        .cancels
        .lock()
        .map_err(|_| "Task cancellation lock is poisoned.")?
        .insert(task.id.clone(), Arc::clone(&cancel));

    let monitoring = task.observing();

    let result = if monitoring {
        operations::monitor(context, &task, &cancel).await.map(|_| routing::reply(String::new()))
    } else {
        execute(context, &task, &cancel).await
    };

    context
        .plugins
        .tasks
        .cancels
        .lock()
        .map_err(|_| "Task cancellation lock is poisoned.")?
        .remove(&task.id);

    cancel.acknowledge();

    let mut store = context.sessions.lock().map_err(|_| crate::state::POISONED)?;
    let current = store.tasks.get(&task.id).ok_or("Background task disappeared.")?;

    if current.step != task.step {
        return Ok(());
    }

    if monitoring {
        if current.status == TaskStatus::Running {
            store.recover_task(&task.id)?;
        }

        context.plugins.tasks.wake.notify_one();
        return result.map(|_| ());
    }

    if current.status.stopping() {
        store.finish_stop(&task.id, task.step)?;
        context.plugins.tasks.wake.notify_one();
        return Ok(());
    }

    if current.status != TaskStatus::Running {
        if store.sessions.contains_key(&task.id) {
            store.clear(&task.id, "idle")?;
        }

        return Ok(());
    }

    let unsafe_step = current.unsafe_step;

    if context.stop.is_signaled() && !unsafe_step {
        store.recover_task(&task.id)?;

        if store.sessions.contains_key(&task.id) {
            store.clear(&task.id, "idle")?;
        }

        return Ok(());
    }

    match result {
        Ok(reply) if task.foreground => {
            store.foreground_result(
                &krabb_core::task::Run { id: task.id.clone(), generation: task.step },
                &reply.text,
            )?;
        }

        result if unsafe_step => {
            store.uncertain_task(&task.id)?;

            let reason = result.err().map_or_else(
                || "No result was returned.".into(),
                |error| sentence(error.to_string()),
            );

            let text = format!(
                "The background task is paused because an action may have completed. Review it \
                    before retrying. {reason}"
            );

            store.checkpoint_task(
                &task.id,
                &task.checkpoint,
                TaskStatus::Blocked,
                unix_seconds(),
                Some(&text),
            )?;
        }

        Err(error) => {
            debug!(task=%task.id, error=%diagnostic(error.to_string()), "Background step failed.");
            crate::failure::settle(&mut store, &task.id, error.as_ref())?;
        }

        Ok(_) => {
            store.retry_task(&task.id)?;
        }
    }

    if store.sessions.contains_key(&task.id) {
        store.clear(&task.id, "idle")?;
    }

    Ok(())
}

async fn execute(
    context: &Context,
    task: &Task,
    cancel: &Cancellation,
) -> Result<ModelReply, PluginError> {
    super::policy::validate(task, &context.config, &context.plugins.clients)?;

    if context.plugins.detached.read().await.contains(&task.provider) {
        return Err(crate::failure::blocked("The task's intelligence provider is detached."));
    }

    let provider = context
        .plugins
        .get(&task.provider)
        .await
        .filter(|provider| provider.supports(Capability::Model))
        .ok_or_else(|| {
            krabb_core::failure::Failure::new(krabb_core::failure::Kind::Missing)
                .error("The task's intelligence provider is unavailable.")
        })?;

    let _boundary = context.plugins.lifecycle.read().await;

    let _workspace = tokio::select! {
        biased;
        _ = context.stop.notified() => return Err("The turn was interrupted.".into()),
        _ = cancel.cancelled() => return Err("The turn was cancelled.".into()),
        guard = lock_workspace(&context.plugins.workspaces, task.workspace.clone()) => guard,
    };

    let user = Message {
        id: format!("{}-step-{}", task.id, task.step),
        session: task.id.clone(),
        role: Role::User,
        sender: Some("task".into()),
        content: vec![Content::Text {
            text: format!(
                "Goal:\n{}\n\nDurable checkpoint:\n{}\n\nContinue from this checkpoint. Completed \
                    actions must not be repeated.",
                task.instruction, task.checkpoint
            ),
        }],
    };

    let history = {
        let mut store = context.sessions.lock().map_err(|_| crate::state::POISONED)?;

        if store.check_task_step(&task.id, task.step).is_err() {
            return Err("The turn was cancelled.".into());
        }

        if !store.sessions.contains_key(&task.origin.session) {
            return Err(crate::failure::blocked("The original conversation no longer exists."));
        }

        if task.foreground {
            if !store
                .sessions
                .get(&task.id)
                .is_some_and(|session| session.kind == state::SessionKind::Foreground)
            {
                return Err(crate::failure::blocked("The saved task journal is missing."));
            }
        } else {
            if task.operation.is_some() && !store.sessions.contains_key(&task.id) {
                return Err(crate::failure::blocked("The saved task journal is missing."));
            }

            store.ensure_task(&task.id, &task.model)?;
        }

        store.origin(&task.id, Some(task.origin.clone()))?;
        store.prepare_queued_context(&task.id)?;

        let history = if task.foreground {
            store.foreground_history(&task.id)?
        } else {
            let original = store.sessions[&task.id].original.iter().cloned();
            let mut operations = store.sessions[&task.id]
                .context()
                .into_iter()
                .filter(|message| {
                    message.sender.as_deref() == Some("operation")
                        && Some(message.id.as_str()) != task.request.as_deref()
                })
                .collect::<Vec<_>>();

            let recent = operations.split_off(operations.len().saturating_sub(OPERATION_HISTORY));

            original.chain(recent).collect()
        };

        store.begin_with_roles(&task.id, user.clone(), Vec::new())?;

        history.into_iter().chain([user]).collect()
    };

    let mut messages = turn_messages(&task.id, &context.home, history);

    super::input::bound(&mut messages, task.request.as_deref());

    if !task.foreground {
        messages.insert(
            0,
            Message {
                id: "krabb-background-task".into(),
                session: task.id.clone(),
                role: Role::System,
                sender: None,
                content: vec![Content::Text { text: TASK_PROMPT.into() }],
            },
        );
    }

    if !task.foreground && !worktree::isolated(&task.workspace) {
        context::origin(
            &mut messages,
            &*context.sessions.lock().map_err(|_| crate::state::POISONED)?,
            &task.id,
            &task.origin,
            &context.config,
        );
    }

    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
    let updates = Arc::clone(&context.sessions);
    let update_id = task.id.clone();

    let drain = tokio::spawn(async move {
        while let Some(notice) = receiver.recv().await {
            let (text, model) = match notice {
                StreamNotice::Progress(text) => (text, true),
                StreamNotice::Status(text) => (text, false),
                _ => continue,
            };

            let posted = updates
                .lock()
                .map_err(|_| std::io::Error::other(crate::state::POISONED))
                .and_then(|mut store| store.post_task_update(&update_id, &text, model));

            if let Err(error) = posted {
                debug!(task = %update_id, %error, "A task update was not delivered.");
            }
        }
    });

    let mut call = task.step.saturating_mul(1_000).saturating_add(1);
    let mut failed = None;
    let failures = Arc::new(Mutex::new(Vec::new()));

    let reply = answer(
        &provider,
        &context.plugins,
        &task.model,
        messages,
        &task.id,
        &task.origin.integration,
        &task.origin.chat,
        task.origin.thread.as_deref(),
        &context.sessions,
        Some(&task.workspace),
        &if worktree::isolated(&task.workspace) {
            worktree::media(&task.workspace, &task.origin.integration)?
        } else {
            media_root_at(&context.home)
        },
        task.tools,
        false,
        task.shell,
        if task.foreground {
            sharing::approval(
                &context.config,
                &task.origin.source(),
                context.config.approval_mode(),
            )
        } else {
            ApprovalMode::Auto
        },
        Arc::clone(&context.approvals),
        cancel,
        &context.stop,
        tokio::time::Instant::now() + context.config.inference.envelope(),
        &mut call,
        &mut failed,
        Some(Arc::clone(&failures)),
        sender,
        None,
        None,
        Some(krabb_core::task::Run { id: task.id.clone(), generation: task.step }),
    )
    .await;

    drain.abort();

    if let Some(error) = failures.lock().map_err(|_| "Task failure lock is poisoned.")?.first() {
        return Err(error.clone().into());
    }

    reply.map(|reply| reply.model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> Task {
        let route = routing::Route {
            kind: routing::Kind::Background,
            message: "Working on it.".into(),
            instruction: "Write the report.".into(),
            language: None,
        };

        record(
            &route,
            &serde_json::json!({"provider":"fixture", "model":"model", "workspace":"/",
                "origin":{"channel":"tui", "chat":"tui-chat", "session":"tui-chat",
                    "thread":null, "private":null, "sender":null}}),
        )
        .unwrap()
    }

    #[test]
    fn a_pending_notice_holds_queued_work_until_it_is_delivered() {
        let mut queued = task();

        assert!(runnable(&queued));

        queued.notice =
            serde_json::from_value(serde_json::json!({"id":"n", "text":"Done."})).unwrap();

        assert!(!runnable(&queued));

        let paused = Task { status: TaskStatus::Paused, ..task() };

        assert!(!runnable(&paused));
    }
}
