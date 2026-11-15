use super::*;

const CONCURRENCY: usize = 2;
const POLL_SECONDS: u64 = 60;
/// Recent operation results replayed into each background step.
const OPERATION_HISTORY: usize = 8;

/// Standing instructions for an unattended background step.
const JOB_PROMPT: &str = concat!(
    "You are executing an authorized background goal. ",
    "There is no overall job lifetime deadline. ",
    "The host admits bounded inference and operation allowances. ",
    "Checkpoint before a limit; waiting does not extend an active allowance. ",
    "Work in concrete increments, then call checkpoint with completed work and the exact next \
        action before a step limit. ",
    "Use wait for external work with an appropriate delay. ",
    "Save important milestones with checkpoint.update; do not send routine waiting messages. ",
    "Continue automatically until the goal is achieved. ",
    "Use blocked when input or permission is required. ",
    "A normal reply is an internal draft, not completion. ",
    "Finish only by calling checkpoint with state complete and the full result. ",
    "Omit message to publish the most recent draft from this step. ",
    "If more work remains, checkpoint and continue without another human prompt. ",
    "The host delivers updates and completion to the original conversation. ",
    "Do not create schedules or new background jobs, and do not send messages to other recipients. \
        ",
    "Never repeat a completed mutation; if an action's outcome is uncertain, stop as blocked.",
);

/// Whether the worker may take this job up: queued work, or a monitored operation that
/// still needs inspecting while the task stops, pauses or blocks.
fn runnable(job: &Job) -> bool {
    (matches!(job.status, JobStatus::Queued | JobStatus::Waiting)
        || job.observing()
            && (job.status.stopping()
                || matches!(
                    job.status,
                    JobStatus::Paused | JobStatus::Cancelled | JobStatus::Blocked
                )))
        && (job.notice.is_none() || job.observing())
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
        let notified = context.plugins.jobs.wake.notified();

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
                .jobs
                .attached
                .lock()
                .map(|attached| attached.clone())
                .unwrap_or_default();
            let orphaned: Vec<_> = store
                .jobs
                .values()
                .filter(|job| {
                    (job.status == JobStatus::Running
                        || job.budget.as_ref().is_some_and(|budget| budget.pending.is_some())
                        || job.status.stopping()
                        || job.operation.as_ref().is_some_and(|operation| {
                            operation.phase == state::OperationPhase::Dispatching
                        }))
                        && !active.contains(&job.id)
                        && !attached.contains(&job.id)
                })
                .map(|job| job.id.clone())
                .collect();

            for id in orphaned {
                if let Err(error) = store.recover_job(&id) {
                    warn!(job = %id, %error, "Background recovery remains pending.");
                }
            }

            let notices: Vec<_> = store
                .jobs
                .values()
                .filter(|job| job.notice.is_some() && !attached.contains(&job.id))
                .map(|job| job.id.clone())
                .collect();

            for id in notices {
                if let Err(error) = store.publish_job_notice(&id) {
                    debug!(job = %id, error = %diagnostic(error.to_string()),
                        "Background notification remains pending.");
                }
            }

            let ready = store
                .jobs
                .values()
                .filter(|job| {
                    runnable(job)
                        && job.ready_at() <= unix_seconds()
                        && (!job.foreground
                            || job.observing()
                            || !store.sessions.get(&job.origin.session).is_some_and(|session| {
                                session.status == "working" || session.inflight.is_some()
                            }))
                        && !active.contains(&job.id)
                        && !attached.contains(&job.id)
                })
                .take(CONCURRENCY.saturating_sub(workers.len()))
                .map(|job| job.id.clone())
                .collect::<Vec<_>>();

            (ready, attached)
        };

        if !context.stop.is_signaled() {
            for id in ready {
                let claimed = context
                    .sessions
                    .lock()
                    .ok()
                    .and_then(|mut store| store.claim_job(&id, unix_seconds()).ok());

                if let Some(job) = claimed {
                    let worker = context.clone();

                    active.insert(id.clone());

                    let worker_id = id.clone();

                    let handle = workers.spawn(async move {
                        if let Err(error) = step(&worker, job).await {
                            warn!(job = %id, error = %diagnostic(error.to_string()),
                                "Background step could not be committed.");
                        }

                        id.clone()
                    });

                    handles.insert(handle.id(), worker_id);
                }
            }
        }

        if context.stop.is_signaled() && workers.is_empty() {
            return;
        }

        let full = workers.len() >= CONCURRENCY;
        let now = unix_seconds();

        let wait = context
            .sessions
            .lock()
            .map(|store| {
                store
                    .jobs
                    .values()
                    .filter(|job| {
                        let idle = runnable(job)
                            && !active.contains(&job.id)
                            && !attached.contains(&job.id);

                        let pending = !full || job.ready_at() > now;

                        idle && pending
                            || job.status == JobStatus::Paused
                                && !job.uncertain
                                && job.deferred.is_some()
                    })
                    .map(|job| {
                        job.deferred
                            .as_ref()
                            .filter(|_| job.status == JobStatus::Paused)
                            .map_or(job.ready_at(), |receipt| {
                                if job.observing() {
                                    job.ready_at().min(receipt.command.when.at)
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

            _ = context.stop.notified(), if !context.stop.is_signaled() => {}

            _ = &mut notified, if !context.stop.is_signaled() => {}

            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
        }
    }
}

async fn step(context: &Context, job: Job) -> Result<(), PluginError> {
    let cancel = Arc::new(Cancellation::new());

    context
        .plugins
        .jobs
        .cancels
        .lock()
        .map_err(|_| "Job cancellation lock is poisoned.")?
        .insert(job.id.clone(), Arc::clone(&cancel));

    let monitoring = job.observing();

    let result = if monitoring {
        operations::monitor(context, &job, &cancel).await.map(|_| tasks::reply(String::new()))
    } else {
        execute(context, &job, &cancel).await
    };

    context
        .plugins
        .jobs
        .cancels
        .lock()
        .map_err(|_| "Job cancellation lock is poisoned.")?
        .remove(&job.id);

    cancel.acknowledge();

    let mut store = context.sessions.lock().map_err(|_| crate::state::POISONED)?;
    let current = store.jobs.get(&job.id).ok_or("Background job disappeared.")?;

    if current.step != job.step {
        return Ok(());
    }

    if monitoring {
        if current.status == JobStatus::Running {
            store.recover_job(&job.id)?;
        }

        context.plugins.jobs.wake.notify_one();
        return result.map(|_| ());
    }

    if current.status.stopping() {
        store.finish_job_control(&job.id, job.step)?;
        context.plugins.jobs.wake.notify_one();
        return Ok(());
    }

    if current.status != JobStatus::Running {
        if store.sessions.contains_key(&job.id) {
            store.clear(&job.id, "idle")?;
        }

        return Ok(());
    }

    let unsafe_step = current.unsafe_step;

    if context.stop.is_signaled() && !unsafe_step {
        store.recover_job(&job.id)?;

        if store.sessions.contains_key(&job.id) {
            store.clear(&job.id, "idle")?;
        }

        return Ok(());
    }

    match result {
        Ok(reply) if job.foreground => {
            store.foreground_result(
                &krabb_core::task::Run { id: job.id.clone(), generation: job.step },
                &reply.text,
            )?;
        }

        result if unsafe_step => {
            store.uncertain_job(&job.id)?;

            let reason = result.err().map_or_else(
                || "No result was returned.".into(),
                |error| sentence(error.to_string()),
            );

            let text = format!(
                "The background job is paused because an action may have completed. Review it \
                    before retrying. {reason}"
            );

            store.checkpoint_job(
                &job.id,
                &job.checkpoint,
                JobStatus::Blocked,
                unix_seconds(),
                Some(&text),
            )?;
        }

        Err(error) => {
            debug!(job=%job.id, error=%diagnostic(error.to_string()), "Background step failed.");
            crate::failure::settle(&mut store, &job.id, error.as_ref())?;
        }

        Ok(_) => {
            store.retry_job(&job.id)?;
        }
    }

    if store.sessions.contains_key(&job.id) {
        store.clear(&job.id, "idle")?;
    }

    Ok(())
}

async fn execute(
    context: &Context,
    job: &Job,
    cancel: &Cancellation,
) -> Result<ModelReply, PluginError> {
    super::policy::validate(job, &context.config, &context.plugins.clients)?;

    if context.plugins.detached.read().await.contains(&job.provider) {
        return Err(crate::failure::blocked("The job's intelligence provider is detached."));
    }

    let provider = context
        .plugins
        .get(&job.provider)
        .await
        .filter(|provider| provider.supports(Capability::Model))
        .ok_or_else(|| {
            krabb_core::failure::Failure::new(krabb_core::failure::Kind::Missing)
                .error("The job's intelligence provider is unavailable.")
        })?;

    let _boundary = context.plugins.lifecycle.read().await;

    let _workspace = tokio::select! {
        biased;
        _ = context.stop.notified() => return Err("The turn was interrupted.".into()),
        _ = cancel.cancelled() => return Err("The turn was cancelled.".into()),
        guard = lock_workspace(&context.plugins.workspaces, job.workspace.clone()) => guard,
    };

    let user = Message {
        id: format!("{}-step-{}", job.id, job.step),
        session: job.id.clone(),
        role: Role::User,
        sender: Some("job".into()),
        content: vec![Content::Text {
            text: format!(
                "Goal:\n{}\n\nDurable checkpoint:\n{}\n\nContinue from this checkpoint. Completed \
                    actions must not be repeated.",
                job.instruction, job.checkpoint
            ),
        }],
    };

    let history = {
        let mut store = context.sessions.lock().map_err(|_| crate::state::POISONED)?;

        if store.check_job_step(&job.id, job.step).is_err() {
            return Err("The turn was cancelled.".into());
        }

        if !store.sessions.contains_key(&job.origin.session) {
            return Err(crate::failure::blocked("The original conversation no longer exists."));
        }

        if job.foreground {
            if !store
                .sessions
                .get(&job.id)
                .is_some_and(|session| session.kind == state::SessionKind::Foreground)
            {
                return Err(crate::failure::blocked("The saved task journal is missing."));
            }
        } else {
            if job.operation.is_some() && !store.sessions.contains_key(&job.id) {
                return Err(crate::failure::blocked("The saved task journal is missing."));
            }

            store.ensure_job(&job.id, &job.model)?;
        }

        store.origin(&job.id, Some(job.origin.clone()))?;
        store.prepare_queued_context(&job.id)?;

        let history = if job.foreground {
            store.foreground_history(&job.id)?
        } else {
            let original = store.sessions[&job.id].original.iter().cloned();
            let mut operations = store.sessions[&job.id]
                .context()
                .into_iter()
                .filter(|message| {
                    message.sender.as_deref() == Some("operation")
                        && Some(message.id.as_str()) != job.request.as_deref()
                })
                .collect::<Vec<_>>();

            let recent = operations.split_off(operations.len().saturating_sub(OPERATION_HISTORY));

            original.chain(recent).collect()
        };

        store.begin_with_roles(&job.id, user.clone(), Vec::new())?;

        history.into_iter().chain([user]).collect()
    };

    let mut messages = turn_messages(&job.id, &context.home, history);

    super::input::bound(&mut messages, job.request.as_deref());

    if !job.foreground {
        messages.insert(
            0,
            Message {
                id: "krabb-job".into(),
                session: job.id.clone(),
                role: Role::System,
                sender: None,
                content: vec![Content::Text { text: JOB_PROMPT.into() }],
            },
        );
    }

    if !job.foreground && !worktree::isolated(&job.workspace) {
        context::origin(
            &mut messages,
            &*context.sessions.lock().map_err(|_| crate::state::POISONED)?,
            &job.id,
            &job.origin,
            &context.config,
        );
    }

    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
    let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });

    let mut call = job.step.saturating_mul(1_000).saturating_add(1);
    let mut failed = None;
    let failures = Arc::new(Mutex::new(Vec::new()));

    let reply = answer(
        &provider,
        &context.plugins,
        &job.model,
        messages,
        &job.id,
        &job.origin.integration,
        &job.origin.chat,
        job.origin.thread.as_deref(),
        &context.sessions,
        Some(&job.workspace),
        &if worktree::isolated(&job.workspace) {
            worktree::media(&job.workspace, &job.origin.integration)?
        } else {
            media_root_at(&context.home)
        },
        job.tools,
        false,
        job.shell,
        if job.foreground {
            sharing::approval(&context.config, &job.origin.source(), context.config.approval_mode())
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
        Some(krabb_core::task::Run { id: job.id.clone(), generation: job.step }),
    )
    .await;

    drain.abort();

    if let Some(error) = failures.lock().map_err(|_| "Job failure lock is poisoned.")?.first() {
        return Err(error.clone().into());
    }

    reply.map(|reply| reply.model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> Job {
        let task = tasks::Task {
            kind: tasks::Kind::Background,
            message: "Working on it.".into(),
            instruction: "Write the report.".into(),
        };

        record(
            &task,
            &serde_json::json!({"provider":"fixture", "model":"model", "workspace":"/",
                "origin":{"channel":"tui", "chat":"tui-chat", "session":"tui-chat",
                    "thread":null, "private":null, "sender":null}}),
        )
        .unwrap()
    }

    #[test]
    fn a_pending_notice_holds_queued_work_until_it_is_delivered() {
        let mut queued = job();

        assert!(runnable(&queued));

        queued.notice =
            serde_json::from_value(serde_json::json!({"id":"n", "text":"Done."})).unwrap();

        assert!(!runnable(&queued));

        let paused = Job { status: JobStatus::Paused, ..job() };

        assert!(!runnable(&paused));
    }
}
