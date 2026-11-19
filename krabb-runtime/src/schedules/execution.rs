//! Runs due schedules inside the daemon and reports their results.

use crate::*;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_scheduler(
    plugins: Plugins,
    sessions: Arc<Mutex<state::Store>>,
    stop: Arc<Stop>,
    home_root: PathBuf,
    config: Config,
    configured_workspace: PathBuf,
    media_root: PathBuf,
    default_provider: String,
    default_model: String,
    approvals: Arc<AsyncMutex<approval::Gate>>,
) {
    let mut scheduler_call_id = 1_u64 << 62;
    let mut task_call_id = 1_u64 << 60;
    let started_at = unix_seconds();
    let mut recovered_scheduler: Option<(Arc<Live>, u64)> = None;
    let mut workers = tokio::task::JoinSet::<u64>::new();
    let mut active_tasks = BTreeSet::new();
    let mut worker_tasks = HashMap::new();
    let workspace_locks = Arc::clone(&plugins.workspaces);

    loop {
        let lifecycle = Arc::new(Arc::clone(&plugins.lifecycle).read_owned().await);
        let Some((scheduler_id, scheduler)) = plugins.find(Capability::Scheduler).await else {
            drop(lifecycle);
            tokio::select! {
                _ = stop.notified() => return,
                _ = plugins.changed.notified() => continue,

                result = workers.join_next_with_id(), if !workers.is_empty() => {
                    match result {
                        Some(Ok((worker_id, task_id))) => {
                            worker_tasks.remove(&worker_id);
                            active_tasks.remove(&task_id);
                        }

                        Some(Err(error)) => {
                            let task_id = worker_tasks.remove(&error.id());

                            if let Some(task_id) = task_id {
                                active_tasks.remove(&task_id);
                                recovered_scheduler = None;
                                warn!(schedule = task_id, error = %error, "Scheduled task worker stopped unexpectedly.");
                            } else {
                                warn!(error = %error, "Scheduled task worker stopped unexpectedly.");
                            }
                        }

                        None => {}
                    }
                }
            }

            continue;
        };

        let generation = scheduler.restart_generation();
        let needs_recovery =
            recovered_scheduler.as_ref().is_none_or(|(previous, previous_generation)| {
                !Arc::ptr_eq(previous, &scheduler) || *previous_generation != generation
            });

        if needs_recovery {
            let Some(result) = scheduler_call(
                &scheduler,
                scheduler_call_id,
                "recover",
                serde_json::json!({"active": active_tasks}),
                &stop,
            )
            .await
            else {
                return;
            };

            match result {
                Ok(response) if response.error.is_none() && response.result.is_some() => {
                    recovered_scheduler = Some((Arc::clone(&scheduler), generation));
                }

                Ok(response) => {
                    let error = response.error.map_or_else(
                        || "Scheduler returned no result.".into(),
                        |error| error.message,
                    );
                    warn!(plugin = scheduler_id, error = %error, "Scheduler recovery check failed.");

                    scheduler_call_id = scheduler_call_id.saturating_add(1);
                    tokio::select! {
                        _ = stop.notified() => return,

                        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}

                        _ = plugins.changed.notified() => {}
                    }

                    continue;
                }

                Err(error) => {
                    warn!(plugin = scheduler_id, error = %diagnostic(sentence(error.to_string())), "Scheduler recovery check failed.");

                    scheduler_call_id = scheduler_call_id.saturating_add(1);
                    tokio::select! {
                        _ = stop.notified() => return,

                        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}

                        _ = plugins.changed.notified() => {}
                    }

                    continue;
                }
            }

            scheduler_call_id = scheduler_call_id.saturating_add(1);
        }

        process_pending_schedule_reports(
            &plugins,
            &sessions,
            &home_root,
            &stop,
            &mut scheduler_call_id,
        )
        .await;

        let mut claimed_work = false;

        while workers.len() < SCHEDULE_CONCURRENCY {
            let Some((scheduler_id, scheduler)) = plugins.find(Capability::Scheduler).await else {
                break;
            };

            let lease = match schedule_id() {
                Ok(lease) => lease,

                Err(error) => {
                    warn!(%error, "Could not generate a scheduler lease ID.");
                    return;
                }
            };

            let Some(claimed) = scheduler_call(
                &scheduler,
                scheduler_call_id,
                "claim",
                serde_json::json!({
                    "at": unix_seconds(),
                    "since": started_at,
                    "lease": lease
                }),
                &stop,
            )
            .await
            else {
                return;
            };

            scheduler_call_id = scheduler_call_id.saturating_add(1);

            let task = match claimed {
                Ok(response) if response.error.is_none() => response
                    .result
                    .and_then(|value| value.get("item").cloned())
                    .filter(|value| !value.is_null()),

                Ok(response) => {
                    warn!(plugin = scheduler_id, error = %response.error.map_or_else(|| "Scheduler returned no result.".into(), |error| error.message), "Scheduler task claim failed.");
                    recovered_scheduler = None;
                    None
                }

                Err(error) => {
                    warn!(plugin = scheduler_id, error = %diagnostic(sentence(error.to_string())), "Scheduler task claim failed.");
                    recovered_scheduler = None;

                    if stop.is_signaled() {
                        return;
                    }

                    None
                }
            };

            let Some(task) = task else {
                break;
            };

            claimed_work = true;
            let task_id = task["id"].as_u64().unwrap_or_default();

            if task["skipped"] == true {
                process_skipped_schedule(
                    &plugins,
                    &sessions,
                    &home_root,
                    &task,
                    &stop,
                    &mut scheduler_call_id,
                )
                .await;
                info!(schedule = task_id, "Missed scheduled occurrence skipped by policy.");
                continue;
            }

            let worker_call_id = task_call_id;
            task_call_id = task_call_id.saturating_add(SCHEDULE_CALL_RANGE);
            let worker_plugins = plugins.clone();
            let worker_sessions = Arc::clone(&sessions);
            let worker_stop = Arc::clone(&stop);
            let worker_home = home_root.clone();
            let worker_config = config.clone();
            let worker_workspace = configured_workspace.clone();
            let worker_media = media_root.clone();
            let worker_provider = default_provider.clone();
            let worker_model = default_model.clone();
            let worker_approvals = Arc::clone(&approvals);
            let worker_workspace_locks = Arc::clone(&workspace_locks);
            active_tasks.insert(task_id);

            let worker_lifecycle = Arc::clone(&lifecycle);
            let worker_id = workers
                .spawn(async move {
                    let _lifecycle = worker_lifecycle;
                    run_scheduled_task(
                        task_id,
                        &task,
                        &worker_plugins,
                        &worker_sessions,
                        &worker_stop,
                        &worker_home,
                        &worker_config,
                        &worker_workspace,
                        &worker_media,
                        &worker_provider,
                        &worker_model,
                        worker_approvals,
                        &worker_workspace_locks,
                        worker_call_id,
                    )
                    .await;

                    task_id
                })
                .id();
            worker_tasks.insert(worker_id, task_id);
        }

        drop(lifecycle);

        tokio::select! {
            _ = stop.notified() => return,

            _ = tokio::time::sleep(if claimed_work {
                std::time::Duration::from_millis(10)
            } else {
                std::time::Duration::from_secs(1)

            }) => {}

            _ = plugins.changed.notified() => {}

            result = workers.join_next_with_id(), if !workers.is_empty() => {
                match result {
                    Some(Ok((worker_id, task_id))) => {
                        worker_tasks.remove(&worker_id);
                        active_tasks.remove(&task_id);
                    }

                    Some(Err(error)) => {
                        let task_id = worker_tasks.remove(&error.id());

                        if let Some(task_id) = task_id {
                            active_tasks.remove(&task_id);
                            recovered_scheduler = None;
                            warn!(schedule = task_id, error = %error, "Scheduled task worker stopped unexpectedly.");
                        } else {
                            warn!(error = %error, "Scheduled task worker stopped unexpectedly.");
                        }
                    }

                    None => {}
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_scheduled_task(
    task_id: u64,
    task: &serde_json::Value,
    plugins: &Plugins,
    sessions: &Arc<Mutex<state::Store>>,
    stop: &Arc<Stop>,
    home_root: &Path,
    config: &Config,
    configured_workspace: &Path,
    media_root: &Path,
    default_provider: &str,
    default_model: &str,
    approvals: Arc<AsyncMutex<approval::Gate>>,
    workspace_locks: &WorkspaceLocks,
    mut call: u64,
) {
    let cancellation = Arc::new(Cancellation::new());
    let cancellation_monitor = tokio::spawn(monitor_schedule_cancellation(
        plugins.clone(),
        task_id,
        Arc::clone(&cancellation),
        Arc::clone(stop),
        (1_u64 << 61) | call,
    ));

    let workspace = scheduled_workspace(&task["context"], configured_workspace)
        .unwrap_or_else(|_| configured_workspace.to_owned());
    let _workspace_guard = lock_workspace(workspace_locks, workspace).await;

    let result = execute_scheduled_task(
        task_id,
        task,
        plugins,
        sessions,
        stop,
        home_root,
        configured_workspace,
        media_root,
        default_provider,
        default_model,
        cancellation,
        approvals,
        config,
        task["run_id"].as_u64(),
        &mut call,
    )
    .await;
    cancellation_monitor.abort();

    let (success, pause, cancelled, text) = match result {
        Ok(text) => scheduled_outcome(text),

        Err(error) => {
            let may_have_mutated = sessions
                .lock()
                .ok()
                .and_then(|store| {
                    store
                        .sessions
                        .get(&format!("schedule-{task_id}"))
                        .map(|item| item.phase == "unsafe")
                })
                .unwrap_or(true);
            scheduled_error_outcome(&error.to_string(), stop.is_signaled(), may_have_mutated)
        }
    };

    let finish_params = serde_json::json!({
        "id": task_id,
        "run_id": task["run_id"],
        "success": success,
        "pause": pause,
        "result": text,
        "manual": task["manual"] == true,
        "retry": task["retry"],
        "attempt": task["attempt"],
        "missed": task["missed"],
        "scheduled_for": task["scheduled_for"],
        "cancelled": cancelled
    });

    let Some((_scheduler_id, finish)) =
        retry_schedule_finish(plugins, task_id, finish_params, stop, &mut call).await
    else {
        warn!(schedule = task_id, "Daemon stopped before the task result could be saved.");
        return;
    };

    match finish {
        response if response.error.is_none() => {
            let finish_result = response.result.unwrap_or_default();
            let status = finish_result["status"].as_str().unwrap_or(if success {
                "completed"
            } else {
                "paused"
            });

            let has_retry = finish_result["retry_at"].as_u64().is_some();
            let status_text = if status == "active" && !success {
                if has_retry { "retrying" } else { "continuing" }
            } else {
                status
            };

            let report_text = if status_text == "cancelled" && success {
                format!(
                    "The task finished before cancellation took effect. Future runs were cancelled. Result: {text}"
                )
            } else if status_text == "retrying" {
                format!(
                    "{text}\nRetry scheduled for {}.",
                    krabb_date::format_datetime(
                        finish_result["retry_at"].as_u64().unwrap_or_default()
                    )
                )
            } else if status_text == "continuing" {
                format!(
                    "{text}\nThis occurrence was skipped after retries. The next run is scheduled for {}.",
                    krabb_date::format_datetime(finish_result["due"].as_u64().unwrap_or_default())
                )
            } else {
                text.clone()
            };

            scheduler_log(home_root, task, status_text, &report_text, Some(&finish_result));

            if matches!(
                report_scheduled_result(
                    home_root,
                    task,
                    task_id,
                    task["run_id"].as_u64(),
                    finish_result["finished_at"].as_u64(),
                    status_text,
                    &report_text,
                    sessions,
                ),
                ScheduleReportOutcome::Delivered
            ) {
                acknowledge_schedule_report(
                    plugins,
                    sessions,
                    task_id,
                    task["run_id"].as_u64(),
                    stop,
                    &mut call,
                )
                .await;
            }

            if status_text == "cancelled" {
                info!(schedule = task_id, "Scheduled agent task was cancelled.");
            } else if success {
                info!(schedule = task_id, "Scheduled agent task completed.");
            } else if status_text == "retrying" {
                info!(
                    schedule = task_id,
                    retry_at = finish_result["retry_at"].as_u64().unwrap_or_default(),
                    "Scheduled agent task failed; a retry was queued."
                );
            } else if status_text == "continuing" {
                info!(
                    schedule = task_id,
                    next_run = finish_result["due"].as_u64().unwrap_or_default(),
                    "Scheduled agent task failed; the occurrence was skipped and the next run remains active."
                );
            } else {
                warn!(schedule = task_id, error = %diagnostic(sentence(text)), "Scheduled agent task paused.");
            }
        }

        _ => unreachable!("schedule finish retries return only successful responses"),
    }
}

pub(crate) async fn retry_schedule_finish(
    plugins: &Plugins,
    task_id: u64,
    params: serde_json::Value,
    stop: &Stop,
    call: &mut u64,
) -> Option<(String, Response)> {
    let mut retry_delay = std::time::Duration::from_secs(1);

    loop {
        let (scheduler_id, scheduler) = wait_for_scheduler(plugins, stop).await?;
        let request_id = *call;
        *call = call.saturating_add(1);

        match scheduler_call(&scheduler, request_id, "finish", params.clone(), stop).await {
            Some(Ok(response)) if response.error.is_none() && response.result.is_some() => {
                return Some((scheduler_id, response));
            }

            Some(Ok(response)) => {
                warn!(
                    schedule = task_id,
                    plugin = scheduler_id,
                    error = %response.error.map_or_else(
                        || "Scheduler returned no finish result.".into(),
                        |error| error.message
                    ),
                    "Scheduled task result could not be saved; retrying the completion record."
                );
            }

            Some(Err(error)) => {
                warn!(
                    schedule = task_id,
                    plugin = scheduler_id,
                    error = %diagnostic(sentence(error.to_string())),
                    "Scheduled task result could not be saved; retrying the completion record."
                );
            }

            None => return None,
        }

        tokio::select! {
            _ = stop.notified() => return None,

            _ = plugins.changed.notified() => {},

            _ = tokio::time::sleep(retry_delay) => {
                retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(30));
            }
        }
    }
}

pub(crate) async fn process_pending_schedule_reports(
    plugins: &Plugins,
    sessions: &Arc<Mutex<state::Store>>,
    home_root: &Path,
    stop: &Stop,
    call: &mut u64,
) -> bool {
    schedules::reconcile(plugins, sessions, stop, call).await;
    let mut after = None;

    loop {
        let Some((scheduler_id, scheduler)) = plugins.find(Capability::Scheduler).await else {
            return false;
        };

        let request_id = *call;
        *call = call.saturating_add(1);

        let Some(Ok(response)) = scheduler_call(
            &scheduler,
            request_id,
            "reports",
            serde_json::json!({"after": after}),
            stop,
        )
        .await
        else {
            return false;
        };

        if let Some(error) = response.error {
            warn!(plugin = scheduler_id, error = %error.message, "Pending schedule reports could not be loaded.");

            return error.code == -32601;
        }

        let reports =
            response.result.unwrap_or_default()["items"].as_array().cloned().unwrap_or_default();

        if reports.is_empty() {
            return false;
        }

        let Some(next_after) = reports.last().and_then(|report| report["id"].as_u64()) else {
            warn!(plugin = scheduler_id, "Pending schedule report page had no valid final ID.");
            return false;
        };

        if after.is_some_and(|after| next_after <= after) {
            warn!(plugin = scheduler_id, "Pending schedule report page did not advance.");
            return false;
        }

        after = Some(next_after);

        for report in reports {
            let task_id = report["id"].as_u64().unwrap_or_default();
            let run_id = report["run_id"].as_u64();
            let finish = &report["finish"];
            let success = finish["success"] == true;
            let status = finish["status"].as_str().unwrap_or("paused");
            let has_retry = finish["retry_at"].as_u64().is_some();
            let status_text = if status == "active" && !success {
                if has_retry { "retrying" } else { "continuing" }
            } else {
                status
            };

            let result = finish["result"].as_str().unwrap_or_default();
            let report_text = if status_text == "cancelled" && success {
                format!(
                    "The task finished before cancellation took effect. Future runs were cancelled. Result: {result}"
                )
            } else if status_text == "retrying" {
                format!(
                    "{result}\nRetry scheduled for {}.",
                    krabb_date::format_datetime(finish["retry_at"].as_u64().unwrap_or_default())
                )
            } else if status_text == "continuing" {
                format!(
                    "{result}\nThis occurrence was skipped after retries. The next run is scheduled for {}.",
                    krabb_date::format_datetime(finish["due"].as_u64().unwrap_or_default())
                )
            } else {
                result.to_owned()
            };

            scheduler_log(home_root, &report, status_text, &report_text, Some(finish));

            if !matches!(
                report_scheduled_result(
                    home_root,
                    &report,
                    task_id,
                    run_id,
                    finish["finished_at"].as_u64(),
                    status_text,
                    &report_text,
                    sessions,
                ),
                ScheduleReportOutcome::Retry
            ) {
                acknowledge_schedule_report(plugins, sessions, task_id, run_id, stop, call).await;
            }
        }
    }
}

pub(crate) async fn process_skipped_schedule(
    plugins: &Plugins,
    sessions: &Arc<Mutex<state::Store>>,
    home_root: &Path,
    task: &Value,
    stop: &Stop,
    call: &mut u64,
) {
    if process_pending_schedule_reports(plugins, sessions, home_root, stop, call).await {
        let task_id = task["id"].as_u64().unwrap_or_default();
        let result = "This occurrence was skipped because the daemon was offline.";
        scheduler_log(home_root, task, "skipped", result, None);
        report_scheduled_result(
            home_root,
            task,
            task_id,
            None,
            task["skipped_at"].as_u64(),
            "skipped",
            result,
            sessions,
        );
    }
}

pub(crate) async fn acknowledge_schedule_report(
    plugins: &Plugins,
    sessions: &Arc<Mutex<state::Store>>,
    task_id: u64,
    run_id: Option<u64>,
    stop: &Stop,
    call: &mut u64,
) {
    let Some(run_id) = run_id else {
        return;
    };

    let Some((_, scheduler)) = plugins.find(Capability::Scheduler).await else {
        return;
    };

    let request_id = *call;
    *call = call.saturating_add(1);
    let Some(Ok(response)) = scheduler_call(
        &scheduler,
        request_id,
        "report",
        serde_json::json!({"id": task_id, "run_id": run_id}),
        stop,
    )
    .await
    else {
        return;
    };

    if let Some(error) = response.error {
        warn!(schedule = task_id, error = %error.message, "Schedule report acknowledgement failed.");
    } else if let Ok(mut store) = sessions.lock()
        && let Err(error) = store.report_run(task_id, run_id)
    {
        warn!(schedule = task_id, error = %error, "Schedule acknowledgement will be reconciled after restart.");
    }
}

pub(crate) async fn wait_for_scheduler(
    plugins: &Plugins,
    stop: &Stop,
) -> Option<(String, Arc<Live>)> {
    loop {
        if let Some(scheduler) = plugins.find(Capability::Scheduler).await {
            return Some(scheduler);
        }

        tokio::select! {
            _ = stop.notified() => return None,

            _ = plugins.changed.notified() => {}

            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
        }
    }
}

pub(crate) fn scheduled_outcome(text: String) -> (bool, bool, bool, String) {
    let text = text.trim_start();

    if let Some(text) = text.strip_prefix("DONE:")
        && !text.trim().is_empty()
    {
        return (true, false, false, sentence(text.trim()));
    }

    if let Some(text) = text.strip_prefix("CANCELLED:") {
        return (false, false, true, text.trim().to_owned());
    }

    if let Some(text) = text.strip_prefix("PAUSED:") {
        return (false, true, false, text.trim().to_owned());
    }

    if let Some(text) = text.strip_prefix("RETRYABLE:") {
        return (false, false, false, text.trim().to_owned());
    }

    (
        false,
        true,
        false,
        format!(
            "The agent did not confirm task completion; review the response before retrying. {}",
            sentence(text.trim())
        ),
    )
}

pub(crate) fn scheduled_visible_text(text: &str) -> &str {
    let text = text.trim_start();

    ["DONE:", "CANCELLED:", "PAUSED:", "RETRYABLE:"]
        .into_iter()
        .find_map(|prefix| text.strip_prefix(prefix).map(str::trim_start))
        .unwrap_or(text)
}

pub(crate) fn scheduled_error_outcome(
    error: &str,
    interrupted: bool,
    may_have_mutated: bool,
) -> (bool, bool, bool, String) {
    if interrupted {
        return (
            false,
            false,
            true,
            "The scheduled action was interrupted. A side effect may already have happened.".into(),
        );
    }

    if let Some(reason) = error.strip_prefix("CANCELLED:") {
        return (false, false, true, sentence(reason.trim()));
    }

    if let Some(reason) = error.strip_prefix("PAUSED:") {
        return (false, true, false, sentence(reason.trim()));
    }

    if may_have_mutated {
        return (
            false,
            true,
            false,
            format!(
                "A mutating action may have completed; review it before retrying. {}",
                sentence(error)
            ),
        );
    }

    let error = error.strip_prefix("RETRYABLE:").unwrap_or(error).trim();
    (false, false, false, sentence(error))
}

pub(crate) fn scheduled_tool_failure(error: &str, may_have_mutated: bool) -> String {
    let error = sentence(diagnostic(error));

    if may_have_mutated {
        format!(
            "PAUSED: A scheduled tool failed after a mutating action may have completed; review it before retrying. {error}"
        )
    } else {
        format!("RETRYABLE: A scheduled tool failed. {error}")
    }
}

pub(crate) fn scheduled_result_heading(status: &str) -> &'static str {
    match status {
        "completed" => "Scheduled task completed",
        "skipped" => "Scheduled task skipped",
        "retrying" => "Scheduled task failed; retry queued",
        "continuing" => "Scheduled task failed; next run remains active",
        "cancelled" => "Scheduled task cancelled",
        _ => "Scheduled task paused",
    }
}

pub(crate) async fn scheduler_call(
    scheduler: &Live,
    id: u64,
    method: &str,
    params: serde_json::Value,
    stop: &Stop,
) -> Option<krabb_core::Result<Response>> {
    tokio::select! {
        _ = stop.notified() => None,

        result = tokio::time::timeout(
            SCHEDULER_CALL_TIME,
            scheduler.call(Request::call(id, method, params)),
        ) => Some(result.unwrap_or_else(|_| {
            Err(krabb_core::Error::Protocol(format!(
                "Scheduler {method} call timed out."
            )))
        })),
    }
}

pub(crate) async fn monitor_schedule_cancellation(
    plugins: Plugins,
    task_id: u64,
    cancellation: Arc<Cancellation>,
    stop: Arc<Stop>,
    mut call: u64,
) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let Some((_, scheduler)) = plugins.find(Capability::Scheduler).await else {
            continue;
        };

        let Some(response) = scheduler_call(
            &scheduler,
            call,
            "cancelled",
            serde_json::json!({"id": task_id}),
            &stop,
        )
        .await
        else {
            return;
        };

        call = call.saturating_add(1);

        if response.is_ok_and(|response| {
            response.error.is_none()
                && response.result.is_some_and(|value| value["cancelled"] == true)
        }) {
            cancellation.request();
            return;
        }
    }
}

pub(crate) fn scheduled_workspace(
    context: &serde_json::Value,
    configured_workspace: &Path,
) -> std::result::Result<PathBuf, krabb_core::Error> {
    if let Some(value) = context["workspace"].as_str() {
        let workspace = std::fs::canonicalize(value).map_err(|error| {
            krabb_core::Error::Denied(format!(
                "The captured schedule workspace is unavailable: {error}."
            ))
        })?;

        if !workspace.is_dir() {
            return Err(krabb_core::Error::Denied(
                "The captured schedule workspace is not a directory.".into(),
            ));
        }

        Ok(workspace)
    } else {
        Ok(configured_workspace.to_owned())
    }
}

pub(crate) fn current_schedule_workspace() -> Result<PathBuf, std::io::Error> {
    let workspace = std::env::current_dir()?.canonicalize()?;

    if !workspace.is_dir() {
        return Err(std::io::Error::other("The current directory is unavailable."));
    }

    Ok(workspace)
}

pub(crate) fn scheduled_policy(
    workspace: &Path,
    origin: &serde_json::Value,
    shell_enabled: bool,
) -> String {
    let discovery = if shell_enabled {
        concat!(
            "Workspace file tools stay inside the captured workspace. Because local shell access is ",
            "enabled, you may also explore the daemon machine when a requested folder, file, or ",
            "binary is not there. Check the current directory and home first, then search by name ",
            "across other readable locations before declaring it absent. For binaries, check PATH ",
            "with command -v or type -a, then search likely install locations if needed. Use bounded, ",
            "read-only discovery commands such as find and file; suppress permission errors and skip ",
            "/proc, /sys, /dev, and /run. Inspect only paths relevant to the instruction and do not ",
            "read credentials, secret stores, or unrelated private files. Do not make changes outside ",
            "the captured workspace unless the scheduled instruction explicitly requests that ",
            "specific change."
        )
    } else {
        concat!(
            "Workspace file tools stay inside the captured workspace. Shell access is unavailable, ",
            "so search the captured workspace with the list and search tools; if the requested path ",
            "or item is not there, explain that the required discovery capability is unavailable ",
            "rather than guessing."
        )
    };

    format!(
        concat!(
            "This is an authorized scheduled task. Follow only the scheduled instruction and use ",
            "only capabilities enabled for this task and current configuration. The captured ",
            "workspace root is {workspace}. Resolve paths against actual entries instead ",
            "of guessing. If the ",
            "requested folder is the workspace itself, use the workspace root; do not append its ",
            "name again. For a request to send or describe a folder's contents, inspect the folder ",
            "first and read only files relevant to the request. Do not claim a path is unavailable ",
            "before searching for it. {discovery} Do not ask for interactive approval. Send messages ",
            "only when the instruction explicitly names an installed channel and recipient, or says ",
            "to use its originating conversation. The originating conversation metadata is {origin}. ",
            "Use that exact destination when the instruction says this conversation or channel; ",
            "never infer a different recipient or send extra messages. Start your final response ",
            "with DONE: only when the requested action is complete. If you cannot complete it, a ",
            "required capability or detail is missing, or the outcome of a side effect is uncertain, ",
            "begin with PAUSED: followed by the reason. If an attempt failed before any side effect ",
            "may have happened and an automatic retry could help, begin with RETRYABLE: followed by ",
            "the reason. Do not report success without DONE:."
        ),
        workspace = workspace.display(),
        discovery = discovery,
        origin = origin
    )
}

pub(crate) async fn lock_workspace(
    workspace_locks: &WorkspaceLocks,
    workspace: PathBuf,
) -> tokio::sync::OwnedMutexGuard<()> {
    let workspace = std::fs::canonicalize(&workspace).unwrap_or(workspace);
    let lock = {
        let mut locks = workspace_locks.lock().await;
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        Arc::clone(locks.entry(workspace).or_default())
    };

    lock.lock_owned().await
}

pub(crate) fn scheduled_messages(
    task_id: u64,
    session: &str,
    home_root: &Path,
    instruction: Message,
    workspace: &Path,
    origin: &Value,
    shell_enabled: bool,
) -> Vec<Message> {
    let mut messages = turn_messages(session, home_root, vec![instruction]);
    messages.insert(
        0,
        Message {
            id: format!("schedule-policy-{task_id}"),
            session: session.into(),
            role: Role::System,
            sender: None,
            content: vec![Content::Text {
                text: scheduled_policy(workspace, origin, shell_enabled),
            }],
        },
    );

    messages
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_scheduled_task(
    task_id: u64,
    task: &serde_json::Value,
    plugins: &Plugins,
    sessions: &Arc<Mutex<state::Store>>,
    stop: &Stop,
    home_root: &Path,
    configured_workspace: &Path,
    media_root: &Path,
    default_provider: &str,
    default_model: &str,
    cancellation: Arc<Cancellation>,
    approvals: Arc<AsyncMutex<approval::Gate>>,
    config: &Config,
    run_id: Option<u64>,
    call: &mut u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let context = &task["context"];
    let provider_id = context["provider"].as_str().unwrap_or(default_provider);
    let model = context["model"].as_str().unwrap_or(default_model);

    if plugins.detached.read().await.contains(provider_id) {
        return Err(format!(
            "PAUSED: Configured model provider {provider_id} is detached. Use `krabb plugin attach {provider_id}` to resume."
        ).into());
    }

    let provider = match plugins.get(provider_id).await {
        Some(provider) => provider,

        None if binary_at(provider_id, home_root).is_some() => {
            load_plugin(home_root, provider_id, Some(Capability::Model), false, config, plugins)
                .await
                .map_err(|error| {
                    format!("PAUSED: Configured model provider {provider_id} could not be loaded: {error}")
                })?;

            plugins.get(provider_id).await.ok_or_else(|| {
                format!("PAUSED: Configured model provider {provider_id} is not available.")
            })?
        }

        None => {
            return Err(format!(
                "PAUSED: Configured model provider {provider_id} is not installed."
            )
            .into());
        }
    };

    if !provider.supports(Capability::Model) {
        return Err(
            format!("PAUSED: Configured provider {provider_id} cannot run agent turns.").into()
        );
    }

    let workspace = scheduled_workspace(context, configured_workspace)?;

    schedules::policy(config, context, &workspace)?;
    let origin = schedules::origin(context)?;

    let tools_enabled = context["tools"].as_bool().unwrap_or(false);
    let shell_enabled = context["shell"].as_bool().unwrap_or(false);

    if tools_enabled && plugins.find(Capability::Tool).await.is_none() {
        return Err("PAUSED: The tools plugin used by this schedule is unavailable.".into());
    }

    let session = format!("schedule-{task_id}");
    let instruction = task["text"].as_str().ok_or("Scheduled instruction is missing.")?;
    let user = Message {
        id: format!("schedule-user-{}", unix_timestamp_millis()),
        session: session.clone(),
        role: Role::User,
        sender: Some("scheduler".into()),
        content: vec![Content::Text { text: instruction.to_owned() }],
    };

    {
        let mut store = sessions.lock().map_err(|_| state::POISONED)?;
        store.ensure_schedule(&session, model)?;
        store.origin(&session, origin.clone())?;
        store.begin_with_roles(&session, user.clone(), Vec::new())?;
    }

    let mut messages = scheduled_messages(
        task_id,
        &session,
        home_root,
        user,
        &workspace,
        &context["origin"],
        shell_enabled,
    );

    if !schedules::destinations(context)?.is_empty() {
        messages.insert(0, Message {
            id: format!("schedule-destinations-{task_id}"), session: session.clone(),
            role: Role::System, sender: None,
            content: vec![Content::Text { text: "The host will deliver your final result to the approved destinations. Produce the complete message after DONE:; do not call message_send or claim delivery has already completed.".into() }],
        });
    }

    if let Some(origin) = &origin {
        context::origin(
            &mut messages,
            &*sessions.lock().map_err(|_| state::POISONED)?,
            &session,
            origin,
            config,
        );
        append_conversation_memory(
            plugins,
            &mut messages,
            &origin.scope(),
            &origin.source(),
            None,
            call,
        )
        .await;
    } else {
        append_memory_context(plugins, &mut messages, "global", call).await;
    }

    let (notices, mut received) = tokio::sync::mpsc::channel(32);
    let drain = tokio::spawn(async move { while received.recv().await.is_some() {} });

    let deadline = tokio::time::Instant::now() + config.inference.envelope();
    let mut failed_tool = None;
    let tool_failures = Arc::new(Mutex::new(Vec::new()));
    let reply = answer(
        &provider,
        plugins,
        model,
        messages,
        &session,
        "scheduler",
        &task_id.to_string(),
        None,
        sessions,
        Some(&workspace),
        media_root,
        tools_enabled,
        false,
        shell_enabled,
        ApprovalMode::Auto,
        approvals,
        &cancellation,
        stop,
        deadline,
        call,
        &mut failed_tool,
        Some(Arc::clone(&tool_failures)),
        notices,
        authorized_destination(&context["message_destination"]),
        run_id,
        None,
    )
    .await;
    drain.abort();

    let may_have_mutated = sessions
        .lock()
        .map_err(|_| state::POISONED)?
        .sessions
        .get(&session)
        .is_some_and(|value| value.phase == "unsafe");

    let tool_failure = tool_failures
        .lock()
        .map_err(|_| "Scheduled tool failure lock is poisoned.")?
        .first()
        .cloned();
    let text = match (reply, tool_failure) {
        (Ok(_), Some(error)) => scheduled_tool_failure(&error, may_have_mutated),

        (Ok(reply), None)
            if may_have_mutated && reply.text.trim_start().starts_with("RETRYABLE:") =>
        {
            format!(
                "PAUSED: A mutating action may already have completed; review it before retrying. {}",
                scheduled_visible_text(&reply.text)
            )
        }

        (Ok(reply), None) => reply.model.text,

        (Err(_), _) if cancellation.requested() => {
            "CANCELLED: The action was interrupted. A side effect may already have happened.".into()
        }

        (Err(error), _) if may_have_mutated => format!(
            "PAUSED: A mutating action may already have completed; review it before retrying. {}",
            sentence(error.to_string())
        ),

        (Err(error), _) => format!("RETRYABLE: {}", sentence(error.to_string())),
    };

    let assistant = Message {
        id: format!("schedule-assistant-{}", now()),
        session: session.clone(),
        role: Role::Assistant,
        sender: None,
        content: vec![Content::Text { text: scheduled_visible_text(&text).into() }],
    };

    let mut store = sessions.lock().map_err(|_| state::POISONED)?;
    // Keep mutation evidence until the reply is durable, including on save rollback.
    store.append(&session, assistant)?;
    store.clear(&session, "idle")?;

    Ok(text)
}

pub(crate) fn schedule_log_filename(name: &str, task_id: u64) -> String {
    let safe_name = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || "-_.".contains(*character))
        .take(80)
        .collect::<String>();
    let name = if safe_name.is_empty() { "schedule" } else { &safe_name };

    format!("{name}-{task_id}.jsonl")
}

pub(crate) fn schedule_log_path(home_root: &Path, task_id: u64) -> std::io::Result<PathBuf> {
    let directory = home_root.join("logs/schedules");
    let suffix = format!("-{task_id}.jsonl");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,

        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(directory.join(format!("{task_id}.jsonl")));
        }

        Err(error) => return Err(error),
    };

    if let Some(path) = entries.filter_map(|entry| entry.ok()).find_map(|entry| {
        let path = entry.path();
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(&suffix))
            .then_some(path)
    }) {
        return Ok(path);
    }

    Ok(directory.join(format!("{task_id}.jsonl")))
}

pub(crate) static SCHEDULE_LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) const SCHEDULE_HISTORY_LIMIT: u64 = 256 * 1024;

pub(crate) fn schedule_history_tail(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(SCHEDULE_HISTORY_LIMIT);

    file.seek(SeekFrom::Start(start))?;

    let mut reader = BufReader::new(file);

    if start > 0 {
        let mut previous = [0_u8; 1];

        reader.get_mut().seek(SeekFrom::Start(start - 1))?;
        reader.get_mut().read_exact(&mut previous)?;
        reader.get_mut().seek(SeekFrom::Start(start))?;

        if previous[0] != b'\n' {
            let mut remaining = SCHEDULE_HISTORY_LIMIT;

            loop {
                let (count, ended_record) = {
                    let buffer = reader.fill_buf()?;

                    if buffer.is_empty() {
                        return Ok(Vec::new());
                    }

                    match buffer.iter().position(|byte| *byte == b'\n') {
                        Some(index) => (index + 1, true),
                        None => (buffer.len(), false),
                    }
                };

                reader.consume(count);
                remaining = remaining.saturating_sub(count as u64);

                if ended_record {
                    break;
                }

                if remaining == 0 {
                    return Ok(Vec::new());
                }
            }
        }
    }

    let mut tail = Vec::new();

    reader.read_to_end(&mut tail)?;

    Ok(tail)
}

pub(crate) fn scheduler_log(
    home_root: &Path,
    task: &Value,
    status: &str,
    text: &str,
    finish: Option<&Value>,
) {
    let task_id = task["id"].as_u64().unwrap_or_default();
    let run_id = task["run_id"].as_u64();
    let name = task["name"].as_str().unwrap_or_default();
    let directory = home_root.join("logs/schedules");
    let filename = schedule_log_filename(name, task_id);
    let path = directory.join(filename);
    let legacy = directory.join(format!("{task_id}.jsonl"));

    if !path.exists()
        && legacy.exists()
        && let Err(error) = std::fs::rename(&legacy, &path)
    {
        warn!(
            schedule = task_id,
            error = %diagnostic(sentence(error.to_string())),
            "Legacy schedule log could not be renamed."
        );
    }

    let context = &task["context"];

    let empty_finish = Value::Null;
    let finish = finish.unwrap_or(&empty_finish);

    let _guard = match SCHEDULE_LOG_LOCK.lock() {
        Ok(guard) => guard,

        Err(_) => {
            warn!(schedule = task_id, "Schedule history lock is poisoned.");
            return;
        }
    };

    if let Some(run_id) = run_id {
        match schedule_history_tail(&path) {
            Ok(history)
                if history.split(|byte| *byte == b'\n').any(|line| {
                    serde_json::from_slice::<Value>(line).is_ok_and(|entry| {
                        entry["schedule"]["id"].as_u64() == Some(task_id)
                            && entry["run"]["id"].as_u64() == Some(run_id)
                    })
                }) =>
            {
                return;
            }

            Ok(_) => {}

            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}

            Err(error) => {
                warn!(schedule = task_id, error = %diagnostic(sentence(error.to_string())), "Schedule history could not be checked for a duplicate run.");
                return;
            }
        }
    }

    let entry = serde_json::json!({
        "schema_version": 1,
        "timestamp": jiff::Timestamp::now().to_string(),
        "schedule": {
            "id": task_id,
            "name": name,
            "timezone": task["zone"]
        },

        "run": {
            "id": task["run_id"].as_u64(),
            "status": status,
            "success": finish.get("success").and_then(Value::as_bool),
            "attempt": finish["attempt"].as_u64().or_else(|| task["attempt"].as_u64()).or_else(|| finish["attempts"].as_u64()),
            "attempts": finish["attempts"].as_u64(),
            "retry_limit": finish["retries"].as_u64(),
            "manual": finish["manual"].as_bool().or_else(|| task["manual"].as_bool()),
            "retry": finish["retry"].as_bool().or_else(|| task["retry"].as_bool()),
            "missed_policy": finish.get("missed").filter(|value| !value.is_null()).unwrap_or(&task["missed"]),
            "scheduled_for": task["scheduled_for"].as_u64().or_else(|| finish["scheduled_for"].as_u64()),
            "next_run_at": (finish["status"] == "active" && finish["retry_at"].is_null())
                .then(|| finish["due"].as_u64())
                .flatten(),
            "retry_at": finish["retry_at"].as_u64()
        },

        "agent": {
            "provider": context["provider"],
            "model": context["model"],
            "workspace": context["workspace"]
        },

        "result": clip(redact_diagnostic(text), 4_000)
    });

    let mut content = entry.to_string();

    content.push('\n');

    match append_jsonl_file(&path, content.as_bytes()) {
        Ok(()) => match std::fs::metadata(&path) {
            Ok(metadata) if metadata.len() > SCHEDULE_HISTORY_LIMIT => {
                match schedule_history_tail(&path) {
                    Ok(history) => {
                        if let Err(error) = save_file(&path, history) {
                            warn!(schedule = task_id, error = %diagnostic(sentence(error.to_string())), "Schedule history could not be rotated.");
                        }
                    }

                    Err(error) => {
                        warn!(schedule = task_id, error = %diagnostic(sentence(error.to_string())), "Schedule history could not be read for rotation.");
                    }
                }
            }

            Ok(_) => {}

            Err(error) => {
                warn!(schedule = task_id, error = %diagnostic(sentence(error.to_string())), "Schedule history size could not be checked.");
            }
        },

        Err(error) => {
            warn!(schedule = task_id, error = %diagnostic(sentence(error.to_string())), "Schedule history could not be written.");
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScheduleReportOutcome {
    Delivered,
    Retry,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn report_scheduled_result(
    home: &Path,
    task: &serde_json::Value,
    task_id: u64,
    run_id: Option<u64>,
    finished_at: Option<u64>,
    status: &str,
    text: &str,
    sessions: &Arc<Mutex<state::Store>>,
) -> ScheduleReportOutcome {
    let mut store = match sessions.lock() {
        Ok(store) => store,
        Err(_) => return ScheduleReportOutcome::Retry,
    };

    match schedules::publish(home, task, task_id, run_id, finished_at, status, text, &mut store) {
        Ok(()) => ScheduleReportOutcome::Delivered,

        Err(error) => {
            warn!(schedule = task_id, error = %diagnostic(sentence(error.to_string())), "Schedule result remains pending delivery publication.");
            ScheduleReportOutcome::Retry
        }
    }
}
