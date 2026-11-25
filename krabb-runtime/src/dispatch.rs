//! Executes plugin and host tool calls for a turn.

use crate::*;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn tool_with_registry(
    plugins: &Plugins,
    registry: &ToolRegistry,
    id: u64,
    name: &str,
    mut args: serde_json::Value,
    approve: bool,
    workspace: Option<&Path>,
    memory_scope: &str,
    host_session: &str,
    turn_id: &str,
    failed_tool: &mut Option<String>,
) -> Result<ToolResult, Box<dyn std::error::Error + Send + Sync>> {
    let isolated = workspace.filter(|path| worktree::isolated(path));

    let source = registry.source.clone().unwrap_or_else(|| origin::principal(memory_scope));

    if name == "data" {
        let plugin = plugins
            .get("sqlite")
            .await
            .filter(|plugin| plugin.supports(Capability::Store))
            .ok_or("The SQLite data capability is not installed.")?;

        let serde_json::Value::Object(values) = &mut args else {
            return Err("Data tool arguments must be an object.".into());
        };

        let scope = memory_scope.to_owned();

        let visibility = sharing::params(
            &plugins.config,
            &source,
            &scope,
            isolated.is_some(),
            sharing::Data::Records,
        );

        values.insert("scope".into(), serde_json::Value::String(scope));
        values.insert("visibility".into(), visibility["visibility"].clone());
        values.insert("excluded".into(), visibility["excluded"].clone());

        let response = match registry
            .cleanup
            .call(Arc::clone(&plugin.process), scoped_request(id, "data", args, isolated))
            .await
        {
            Ok(response) => response,

            Err(error) => {
                *failed_tool = Some("sqlite".into());
                return Err(error.into());
            }
        };

        if let Some(error) = response.error {
            return Err(error.message.into());
        }

        let value = response.result.ok_or("The SQLite plugin returned no data result.")?;

        return Ok(ToolResult::text(clip(value.to_string(), TEXT_LIMIT)));
    }

    if let Some(action) = name.strip_prefix("schedule_")
        && action != "propose"
    {
        let (plugin_id, plugin) = plugins
            .find(Capability::Scheduler)
            .await
            .ok_or("The scheduler plugin is not installed.")?;
        let (method, params) = match action {
            "list" => ("list", args.clone()),
            "cancel" => ("remove", serde_json::json!({"id": args["id"]})),
            "update" => (
                "update",
                serde_json::json!({
                    "id": args["id"], "destinations": args["destinations"], "approved": approve
                }),
            ),
            "pause" | "resume" | "run" | "retry" => (action, serde_json::json!({"id": args["id"]})),
            _ => return Err("Unknown schedule action.".into()),
        };

        let (request_method, params) = if let Some(caller) = args.get("caller") {
            let mut params = params;
            params["caller"] = caller.clone();
            params["action"] = serde_json::json!(method);
            ("control", params)
        } else {
            (method, params)
        };

        let response = match registry
            .cleanup
            .call(Arc::clone(&plugin.process), Request::call(id, request_method, params))
            .await
        {
            Ok(response) => response,

            Err(error) => {
                *failed_tool = Some(plugin_id);
                return Err(error.into());
            }
        };

        if let Some(error) = response.error {
            return Err(error.message.into());
        }

        let value = response.result.ok_or("Scheduler returned no result.")?;

        return Ok(ToolResult::text(match method {
            "list" => format_schedule_list(&value),
            "remove" if value["cancel_requested"] == true => format!(
                "Cancellation requested for schedule {}. Its current action will be interrupted; a side effect may already have happened.",
                args["id"]
            ),
            "remove" if value["deleted"] == true => format!("Schedule {} cancelled.", args["id"]),
            "remove" => format!("Schedule {} was not found.", args["id"]),
            "pause" if value["pause_requested"] == true => format!(
                "Pause requested for schedule {}.\nThe current run will finish, then the \
                schedule will stay paused.",
                args["id"]
            ),
            _ => format!(
                "Schedule {} is now {}.",
                args["id"],
                value["status"].as_str().unwrap_or(method)
            ),
        }));
    }

    if name == "schedule_propose" {
        let (plugin_id, plugin) = plugins
            .find(Capability::Scheduler)
            .await
            .ok_or("The scheduler plugin is not installed.")?;
        let mut params = args;
        let schedule_id = schedule_id()?;
        let serde_json::Value::Object(values) = &mut params else {
            return Err("Schedule proposal arguments must be an object.".into());
        };

        let instruction = values
            .get("instruction")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= TEXT_LIMIT)
            .ok_or("A schedule instruction is required and must be smaller than 256 KiB.")?
            .to_owned();

        if let Some(name) = values
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            if name.chars().count() > 80 || !valid_schedule_name(name) {
                return Err("Schedule names must start with a letter or number and contain only letters, numbers, hyphens, underscores, or dots (up to 80 characters).".into());
            }

            values.insert("name".into(), serde_json::json!(name));
        }

        let delay = values.get("delay_seconds").and_then(serde_json::Value::as_u64);
        let repeat = values.get("repeat_seconds").and_then(serde_json::Value::as_u64);
        let run_at = values
            .get("run_at")
            .and_then(serde_json::Value::as_str)
            .map(str::parse::<jiff::Timestamp>)
            .transpose()?;
        let has_cron = values.get("cron").and_then(serde_json::Value::as_str).is_some();
        let has_delay = delay.is_some() || (repeat.is_some() && run_at.is_none());

        let selected_times = [has_delay, run_at.is_some(), has_cron]
            .into_iter()
            .filter(|selected| *selected)
            .count();

        if selected_times != 1 {
            return Err("Choose exactly one schedule time: delay_seconds, run_at, or cron.".into());
        }

        if has_cron && repeat.is_some() {
            return Err("Choose either a cron schedule or a fixed repeat interval.".into());
        }

        if repeat.is_some_and(|value| !(5..=31_536_000).contains(&value)) {
            return Err(
                "Schedule repeat interval must be between five seconds and one year.".into()
            );
        }

        let delay = delay.or_else(|| repeat.filter(|_| run_at.is_none()));

        if delay.is_some_and(|value| !(5..=31_536_000).contains(&value)) {
            return Err("Schedule delay must be between five seconds and one year.".into());
        }

        let at = run_at.map(|timestamp| timestamp.as_second());
        let at = at.map(u64::try_from).transpose()?;

        if at.is_some_and(|value| value < unix_seconds().saturating_add(5)) {
            return Err("Exact scheduled times must be at least five seconds in the future.".into());
        }

        let retries = values.get("retries").and_then(serde_json::Value::as_u64).unwrap_or(0);

        if retries > 5 {
            return Err("Scheduled tasks can be configured for at most five retries.".into());
        }

        let on_failure = values
            .get("on_failure")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("pause")
            .to_owned();

        if !matches!(on_failure.as_str(), "pause" | "continue") {
            return Err("Failure policy must be pause or continue.".into());
        }

        if on_failure == "continue" && repeat.is_none() && !has_cron {
            return Err("Failure policy continue requires a recurring schedule.".into());
        }

        values.insert("id".into(), serde_json::json!(schedule_id));
        values.insert("text".into(), serde_json::json!(instruction));
        values.insert("delay".into(), serde_json::json!(delay.unwrap_or_default()));
        values.insert("activate".into(), serde_json::json!(true));

        if let Some(repeat) = repeat {
            values.insert("repeat".into(), serde_json::json!(repeat));
        }

        if let Some(at) = at {
            values.insert("at".into(), serde_json::json!(at));
        }

        if let Some(retries) = values.get("retries").cloned() {
            values.insert("retries".into(), retries);
        }

        if let Some(zone) =
            values.get("timezone").and_then(serde_json::Value::as_str).map(str::to_owned)
        {
            values.insert("zone".into(), serde_json::json!(zone));
        }

        let workspace_tools_enabled = params["context"]["tools"] == true;
        let shell_enabled = workspace_tools_enabled && params["context"]["shell"] == true;

        let response = match registry
            .cleanup
            .call(Arc::clone(&plugin.process), Request::call(schedule_id, "add", params))
            .await
        {
            Ok(response) => response,

            Err(error) => {
                *failed_tool = Some(plugin_id);
                return Err(error.into());
            }
        };

        if let Some(error) = response.error {
            return Err(error.message.into());
        }

        let schedule = response.result.unwrap_or_default();
        let due = schedule["due"]
            .as_u64()
            .map(|due| {
                krabb_date::format_datetime_in_timezone(
                    due,
                    schedule["zone"].as_str().unwrap_or("UTC"),
                )
            })
            .unwrap_or_else(|| "unknown time".into());
        let frequency = if let Some(cron) = schedule["cron"].as_str() {
            format!("\nFrequency: {cron} ({})", schedule["zone"].as_str().unwrap_or("UTC"))
        } else if let Some(repeat) = schedule["repeat"].as_u64().filter(|repeat| *repeat > 0) {
            format!("\nFrequency: every {}", krabb_date::format_duration(repeat))
        } else {
            String::new()
        };

        let tools_note = if workspace_tools_enabled { "enabled" } else { "disabled" };

        let shell_note = if shell_enabled { "enabled" } else { "disabled" };

        let retry_note = if retries == 0 {
            "Automatic retries are off."
        } else {
            "Automatic retries use bounded exponential backoff."
        };

        let failure_note = if on_failure == "continue" {
            "Failed recurring occurrences are skipped after retries, and the next run stays active."
        } else {
            "The schedule pauses if retries are exhausted."
        };

        info!(schedule = schedule_id, "Scheduled task activated.");

        return Ok(ToolResult::text(format!(
            "Schedule {} ({schedule_id}) is active.\n\nNext run: {due}\nTools: \
            {tools_note}\nShell: {shell_note}{frequency}\nAutomatic retries: \
            {retries}\n{retry_note}\n{failure_note}\n\nCheck it with /schedule list\nPause it \
            with /schedule pause {schedule_id}",
            schedule["name"].as_str().unwrap_or("Scheduled task")
        )));
    }

    if let Some(method) = name.strip_prefix("memory_") {
        let (id_plugin, plugin) =
            plugins.find(Capability::Memory).await.ok_or("The memory plugin is not installed.")?;
        let mut params = args;

        let serde_json::Value::Object(values) = &mut params else {
            return Err("Memory tool arguments must be an object.".into());
        };

        values.insert("scope".into(), memory_scope.into());
        let policy = sharing::params(
            &plugins.config,
            &source,
            memory_scope,
            isolated.is_some(),
            sharing::Data::Memory,
        );

        values.insert("visibility".into(), policy["visibility"].clone());
        values.insert("excluded".into(), policy["excluded"].clone());

        if method == "remember" {
            values.insert("approved".into(), serde_json::Value::Bool(approve));
        }

        let response = match registry
            .cleanup
            .call(Arc::clone(&plugin.process), scoped_request(id, method, params, isolated))
            .await
        {
            Ok(response) => response,

            Err(error) => {
                *failed_tool = Some(id_plugin);
                return Err(error.into());
            }
        };

        if let Some(error) = response.error {
            return Err(error.message.into());
        }

        let value = response
            .result
            .map(|mut value| {
                if let Some(items) = value["items"].as_array_mut() {
                    items.retain(|entry| {
                        entry["scope"]
                            .as_str()
                            .is_some_and(|scope| sharing::visible(&policy, scope))
                    });
                }

                clip(value.to_string(), TEXT_LIMIT)
            })
            .ok_or_else(|| "The memory plugin returned no result.".to_owned())?;

        return Ok(ToolResult::text(value));
    }

    let id_plugin =
        registry.owners.get(name).ok_or_else(|| format!("Tool `{name}` is not registered."))?;

    let plugin = plugins
        .get(id_plugin)
        .await
        .filter(|plugin| plugin.supports(Capability::Tool))
        .ok_or_else(|| format!("Tool provider {id_plugin} is unavailable."))?;

    if id_plugin == "tools"
        && let serde_json::Value::Object(values) = &mut args
    {
        values.insert("approve".into(), serde_json::Value::Bool(approve));

        if let Some(workspace) = workspace {
            values.insert(
                "workspace".into(),
                serde_json::Value::String(workspace.display().to_string()),
            );
        }
    } else if !args.is_object() {
        return Err("Tool arguments must be an object.".into());
    }

    let params = if id_plugin == "tools" {
        args
    } else {
        serde_json::json!({
            "arguments": args,
            "host": {"session": host_session, "turn": turn_id}
        })
    };

    let request = if id_plugin == "tools" && isolated.is_none() && worktree::tool(name) {
        Request::call(
            id,
            "visible",
            serde_json::json!({
                "method": name, "params": params,
                "access": sharing::access(
                    &plugins.config,
                    &source,
                    host_session,
                    sharing::Data::Files,
                )?
            }),
        )
    } else {
        Request::call(id, name, params)
    };

    let response = registry.cleanup.call(Arc::clone(&plugin.process), request).await;

    let response = match response {
        Ok(response) => response,

        Err(error) => {
            *failed_tool = Some(id_plugin.clone());
            return Err(error.into());
        }
    };

    if let Some(error) = response.error {
        return Err(error.message.into());
    }

    let value = response.result.ok_or("The tool returned no result.")?;

    decode_tool_result(value)
}

#[allow(clippy::too_many_arguments)]
#[cfg(all(test, unix))]
pub(crate) async fn tool(
    plugins: &Plugins,
    id: u64,
    name: &str,
    args: serde_json::Value,
    approve: bool,
    workspace: Option<&Path>,
    memory_scope: &str,
    failed_tool: &mut Option<String>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let registry = legacy_tool_registry(Some(name));

    let result = tool_with_registry(
        plugins,
        &registry,
        id,
        name,
        args,
        approve,
        workspace,
        memory_scope,
        "test",
        "test-turn",
        failed_tool,
    )
    .await?;

    Ok(tool_result_text(&result))
}

#[cfg(test)]
pub(crate) fn legacy_tool_registry(name: Option<&str>) -> ToolRegistry {
    let mut registry = ToolRegistry {
        specs: tools(true),
        owners: tools(true).into_iter().map(|spec| (spec.name, "tools".into())).collect(),
        ..ToolRegistry::default()
    };

    if let Some(name) = name {
        registry.owners.insert(name.into(), "tools".into());
    }

    registry
}

pub(crate) fn tool_result_text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|item| match item {
            ToolContent::Text { text } => Some(text.as_str()),
            ToolContent::Image { alt, .. } => alt.as_deref(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn decode_tool_result(
    value: serde_json::Value,
) -> Result<ToolResult, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(content) = value.get("content") {
        let content = serde_json::from_value::<Vec<ToolContent>>(content.clone())?;

        if content.len() > 8 {
            return Err("Tool result contains too many content items.".into());
        }

        for item in &content {
            match item {
                ToolContent::Text { text } if text.len() > TEXT_LIMIT => {
                    return Err("Tool result text exceeds the size limit.".into());
                }

                ToolContent::Image { uri, .. }
                    if !uri.starts_with("data:image/")
                        || image_data_size(uri).is_none_or(|size| size > IMAGE_LIMIT) =>
                {
                    return Err("Tool result image is invalid or exceeds the image limit.".into());
                }

                ToolContent::Text { .. } | ToolContent::Image { .. } => {}
            }
        }

        let metadata = value.get("metadata").cloned();

        if metadata.as_ref().is_some_and(|metadata| {
            serde_json::to_vec(metadata).is_ok_and(|data| data.len() > 16 * 1024)
        }) {
            return Err("Tool result metadata exceeds the size limit.".into());
        }

        return Ok(ToolResult { content, metadata });
    }

    let text = value["text"].as_str().map(str::to_owned).unwrap_or_else(|| value.to_string());

    Ok(ToolResult::text(clip(text, TEXT_LIMIT)))
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) async fn execute_tool(
    plugins: &Plugins,
    id: u64,
    name: &str,
    args: serde_json::Value,
    provider: &str,
    model: &str,
    approval_mode: ApprovalMode,
    approvals: Arc<AsyncMutex<approval::Gate>>,
    channel: &str,
    chat: &str,
    thread: Option<&str>,
    session: &str,
    sessions: &Arc<Mutex<state::Store>>,
    workspace: Option<&Path>,
    tools_enabled: bool,
    schedule_enabled: bool,
    shell_enabled: bool,
    notices: tokio::sync::mpsc::Sender<StreamNotice>,
    deadline: tokio::time::Instant,
    failed_tool: &mut Option<String>,
    scheduled_destination: Option<&AuthorizedDestination>,
    schedule_run_id: Option<u64>,
    tool_call: usize,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut registry = legacy_tool_registry(Some(name));

    if channel == "scheduler" {
        registry.execution = state::SessionKind::Schedule;
    }

    let result = execute_tool_with_registry(
        plugins,
        &registry,
        "test-turn",
        id,
        name,
        args,
        provider,
        model,
        approval_mode,
        approvals,
        channel,
        chat,
        thread,
        session,
        sessions,
        workspace,
        tools_enabled,
        schedule_enabled,
        shell_enabled,
        notices,
        deadline,
        failed_tool,
        scheduled_destination,
        schedule_run_id,
        tool_call,
    )
    .await?;

    Ok(tool_result_text(&result))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_tool_with_registry(
    plugins: &Plugins,
    registry: &ToolRegistry,
    turn_id: &str,
    id: u64,
    name: &str,
    mut args: serde_json::Value,
    provider: &str,
    model: &str,
    approval_mode: ApprovalMode,
    approvals: Arc<AsyncMutex<approval::Gate>>,
    channel: &str,
    chat: &str,
    thread: Option<&str>,
    session: &str,
    sessions: &Arc<Mutex<state::Store>>,
    workspace: Option<&Path>,
    tools_enabled: bool,
    schedule_enabled: bool,
    shell_enabled: bool,
    notices: tokio::sync::mpsc::Sender<StreamNotice>,
    deadline: tokio::time::Instant,
    failed_tool: &mut Option<String>,
    scheduled_destination: Option<&AuthorizedDestination>,
    schedule_run_id: Option<u64>,
    tool_call: usize,
) -> Result<ToolResult, Box<dyn std::error::Error + Send + Sync>> {
    if registry.route.handoff.borrow().is_some() {
        return Err("The background task is already saved; do not repeat its work.".into());
    }

    if let Some(run) = &registry.run {
        sessions.lock().map_err(|_| state::POISONED)?.check_job_step(&run.id, run.generation)?;
    }

    if registry.execution == state::SessionKind::Foreground
        && approval_mode == ApprovalMode::Prompt
        && mutating(name, &args)
    {
        let text = approval_text(name, &args);

        if text.len() > APPROVAL_TEXT_LIMIT {
            return Err(failure::blocked(concat!(
                "The proposed action exceeds the approval display limit. ",
                "Its goal and context are saved."
            )));
        }

        let run = registry.run.as_ref().ok_or("The task worker generation is missing.")?;

        sessions
            .lock()
            .map_err(|_| state::POISONED)?
            .request_approval(run, id, name, &args, &text)?;
        registry.route.handoff.send_replace(Some(String::new()));
        plugins.jobs.wake.notify_one();

        return Ok(ToolResult::text(
            "Approval was requested. Nothing ran; the host continues after the decision.",
        ));
    }

    let _mutation = if (mutating(name, &args) && name != "task") || name == "checkpoint" {
        Some(registry.route.gate.lock().await)
    } else {
        None
    };

    if let Some(run) = &registry.run {
        sessions.lock().map_err(|_| state::POISONED)?.check_job_step(&run.id, run.generation)?;
    }

    let fast = *registry.route.kind.borrow() == routing::Kind::Fast;

    if mutating(name, &args) && fast {
        registry.route.kind.send_replace(routing::Kind::Thoughtful);
    }

    if name == jobs::input::NAME {
        let run = registry.run.as_ref().ok_or("This conversation has no saved task input.")?;
        let store = sessions.lock().map_err(|_| state::POISONED)?;

        return Ok(ToolResult::text(jobs::input::read(&store, &run.id, &args)?));
    }

    if name == "checkpoint" {
        if registry.execution != state::SessionKind::Job {
            return Err("Checkpoints are available only inside a background job.".into());
        }

        {
            let mut store = sessions.lock().map_err(|_| state::POISONED)?;
            let run = registry.run.as_ref().ok_or("Task generation is missing.")?;
            store.check_job_step(&run.id, run.generation)?;
            jobs::apply(&mut store, session, args)?;
        }

        registry.route.handoff.send_replace(Some(String::new()));

        plugins.jobs.wake.notify_one();

        return Ok(ToolResult::text("Checkpoint saved; this execution step has ended."));
    }

    if name == "job" {
        if matches!(channel, "job" | "scheduler") || !registry.execution.interactive() {
            return Err("Unattended work cannot control other background jobs.".into());
        }

        if args["action"] == "list" {
            return control_job(sessions, session, &args, plugins);
        }
    }

    let task = if name == "task" { Some(routing::Route::parse(args.clone())?) } else { None };

    let _task_guard = if let Some(task) = &task {
        if !registry.execution.interactive() {
            return Err("Scheduled execution cannot classify or create background tasks.".into());
        }

        match task.kind {
            routing::Kind::Fast => return Ok(ToolResult::text("Answer the request directly.")),

            routing::Kind::Thoughtful => {
                registry.route.kind.send_replace(routing::Kind::Thoughtful);

                if !registry.route.announced.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    let saved = sessions
                        .lock()
                        .map_err(|_| state::POISONED)?
                        .append_progress(session, &task.message);

                    if let Err(error) = saved {
                        warn!(session, error = %error, "The progress message could not be saved.");
                    }

                    notices
                        .send(StreamNotice::Progress(task.message.clone()))
                        .await
                        .map_err(|_| "The progress receiver is unavailable.")?;
                }

                return Ok(ToolResult::text("Continue the work and return the complete answer."));
            }

            routing::Kind::Background => {
                if !registry.route.background.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(ToolResult::text(
                        "Background tasks are unavailable here; no task was saved. Answer \
                            directly.",
                    ));
                }

                if sessions
                    .lock()
                    .map_err(|_| state::POISONED)?
                    .sessions
                    .get(session)
                    .is_some_and(|session| session.phase == "unsafe")
                {
                    return Err("Start background work before performing mutating actions.".into());
                }

                let guard = registry.route.gate.lock().await;

                if registry.route.handoff.borrow().is_some() {
                    return Err("The background task is already saved.".into());
                }

                args["context"] = if let Some(run) = &registry.run {
                    let store = sessions.lock().map_err(|_| state::POISONED)?;
                    let job = store.job(&run.id)?;
                    serde_json::json!({"provider":job.provider, "model":job.model,
                        "workspace":job.workspace, "origin":job.origin,
                        "tools":job.tools, "shell":job.shell})
                } else {
                    jobs::capture(
                        &*sessions.lock().map_err(|_| state::POISONED)?,
                        provider,
                        model,
                        workspace.ok_or("A background workspace is required.")?,
                        channel,
                        chat,
                        thread,
                        session,
                        tools_enabled
                            && registry
                                .specs
                                .iter()
                                .any(|spec| !HOST_TOOLS.contains(&spec.name.as_str())),
                        shell_enabled,
                    )?
                };

                Some(guard)
            }
        }
    } else {
        None
    };

    if workspace.is_some_and(worktree::isolated)
        && !(worktree::tool(name)
            && registry.owners.get(name).is_some_and(|owner| owner == "tools")
            || matches!(name, "memory_search" | "memory_remember" | "data" | "task" | "job"))
    {
        return Err("This tool cannot enforce the isolated workspace boundary.".into());
    }

    if matches!(name, "shell" | "git" | "patch") && !shell_enabled {
        return Err("Shell access is not authorized for this turn.".into());
    }

    if name.starts_with("schedule_") && !schedule_enabled {
        return Err("Scheduling is not authorized for this sender.".into());
    }

    if name == "schedule_list" && !schedule_listing_allowed(registry.execution) {
        return Err("Schedule listing is available only to local administrators.".into());
    }

    if matches!(
        name,
        "schedule_list"
            | "schedule_cancel"
            | "schedule_pause"
            | "schedule_resume"
            | "schedule_run"
            | "schedule_retry"
            | "schedule_update"
    ) {
        let destinations = if name == "schedule_update" {
            Some(schedules::parse(&args["destinations"])?)
        } else {
            None
        };

        let source = registry.source.clone().unwrap_or_else(|| sharing::principal(channel));

        args = serde_json::json!({
            "id": args["id"], "page": args["page"],
            "caller": {
                "channel": channel, "chat": chat, "thread": thread, "session": session,
                "source": source,
                "access": sharing::access(&plugins.config, &source, session,
                    sharing::Data::Schedules)?
            }
        });

        if let Some(destinations) = destinations {
            args["destinations"] = serde_json::to_value(destinations)?;
        }
    }

    if name == "schedule_propose"
        && let serde_json::Value::Object(values) = &mut args
    {
        values.retain(|key, _| {
            matches!(
                key.as_str(),
                "instruction"
                    | "message_destination"
                    | "destinations"
                    | "delay_seconds"
                    | "run_at"
                    | "cron"
                    | "repeat_seconds"
                    | "timezone"
                    | "missed"
                    | "retries"
                    | "on_failure"
            )
        });

        let instruction = values
            .get("instruction")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let destinations =
            values.remove("destinations").map(|value| schedules::parse(&value)).transpose()?;

        let message_destination = values
            .remove("message_destination")
            .map(|value| {
                authorized_destination(&value)
                    .ok_or("The scheduled message destination is invalid.")
            })
            .transpose()?
            .or_else(|| {
                if destinations.is_none() { requested_tui_destination(&instruction) } else { None }
            });

        if destinations.is_some() && message_destination.is_some() {
            return Err("Choose destinations or the legacy message_destination, not both.".into());
        }

        if message_destination.as_ref().is_some_and(|destination| {
            destination.channel == krabb_core::client::TUI
                && destination.chat != session
                && !(destination.chat == "tui-default"
                    && requested_tui_destination(&instruction).is_some())
        }) {
            return Err("A TUI message must target the originating TUI session, or the explicitly requested default TUI chat.".into());
        }

        let schedule_tools_enabled = tools_enabled
            && registry.specs.iter().any(|spec| !HOST_TOOLS.contains(&spec.name.as_str()));

        let (private, sender, roles, source) = {
            let store = sessions.lock().map_err(|_| state::POISONED)?;
            let current = store.sessions.get(session);
            (
                current.map(|session| session.private),
                current
                    .and_then(|session| session.inflight.as_ref())
                    .and_then(|message| message.sender.clone()),
                current.map(|session| session.inflight_roles.clone()).unwrap_or_default(),
                current
                    .and_then(|session| session.source.clone())
                    .unwrap_or_else(|| sharing::principal(channel)),
            )
        };

        values.insert(
            "context".into(),
            serde_json::json!({
                "provider": provider,
                "model": model,
                "workspace": workspace.map(|value| value.display().to_string()),
                "origin": {
                    "source": source,
                    "channel": channel,
                    "chat": chat,
                    "thread": thread,
                    "session": session,
                    "private": private,
                    "sender": sender,
                    "roles": roles
                },

                "message_destination": message_destination,
                "tools": schedule_tools_enabled,
                "shell": shell_enabled
            }),
        );

        if let Some(destinations) = destinations {
            values.get_mut("context").expect("captured context")["destinations"] =
                serde_json::to_value(destinations)?;
        }
    }

    let changes = mutating(name, &args);

    let approval_mode = match approval_mode {
        ApprovalMode::Auto if name == settings::TOOL => ApprovalMode::Prompt,
        mode => mode,
    };

    let approve = match (changes, approval_mode) {
        (false, _) => false,

        (true, ApprovalMode::Off) => {
            return Err("Approval policy blocks mutating tools.".into());
        }

        (true, ApprovalMode::Auto) => true,

        (true, ApprovalMode::Prompt) => {
            let arguments = approval_arguments(name, &args)?;

            let text = if name == settings::TOOL {
                let source =
                    registry.source.as_ref().ok_or("This conversation has no settings section.")?;

                settings::describe(source, &args)?
            } else {
                approval_text(name, &args)
            };

            if text.len() > APPROVAL_TEXT_LIMIT {
                return Err(concat!(
                    "The request exceeds the approval display limit. ",
                    "Shorten it so it can be reviewed in full."
                )
                .into());
            }

            let target = approval::Target {
                source: registry.source.clone().unwrap_or_else(|| sharing::principal(channel)),
                channel: channel.into(),
                chat: chat.into(),
                thread: thread.map(str::to_owned),
                session: session.into(),
                tool: name.into(),
                args: args.clone(),
            };

            let permission_scope = sharing::approvals(&plugins.config, &target.source);
            let approval_workspace =
                workspace.map(Path::to_path_buf).unwrap_or_else(workspace_root);
            let approval_workspace = Some(
                std::fs::canonicalize(&approval_workspace)
                    .unwrap_or(approval_workspace)
                    .display()
                    .to_string(),
            );
            let cached = if workspace.is_some_and(worktree::isolated) || name == settings::TOOL {
                None
            } else {
                approvals.lock().await.decision(&target, &approval_workspace, |previous| {
                    let source = previous.source.clone();
                    permission_scope.contains(&source)
                        && sharing::configured(&plugins.config, &source)
                        && !sharing::isolated(&plugins.config, &source)
                })
            };

            if let Some(approved) = cached {
                if !approved {
                    return Ok(ToolResult::text(
                        "The matching operation was denied under the shared approval policy.",
                    ));
                }

                true
            } else {
                let challenge = approvals.lock().await.issue_in(target, approval_workspace)?;

                // A settings change is never saved to resume later, so no stored approval can
                // grant authority after the conversation has moved on.
                let saved_run = registry.run.as_ref().filter(|_| name != settings::TOOL);

                if let Some(run) = saved_run {
                    let saved = sessions
                        .lock()
                        .map_err(|_| state::POISONED)?
                        .propose_approval(run, id, name, &args, &text);

                    if let Err(error) = saved {
                        approvals.lock().await.cancel(&challenge.approve);
                        return Err(error.into());
                    }
                }

                let delivered = notices
                    .send(StreamNotice::Approval {
                        chat: chat.into(),
                        thread: thread.map(str::to_owned),
                        tool: name.into(),
                        arguments,
                        command: args["command"].as_str().map(str::to_owned),
                        text,
                        approve: challenge.approve.clone(),
                        deny: challenge.deny,
                        deadline,
                    })
                    .await
                    .is_ok();

                if !delivered {
                    approvals.lock().await.cancel(&challenge.approve);
                }

                let answer = if delivered { challenge.answer.await.ok() } else { None };

                match (answer, saved_run) {
                    (Some(approved), Some(run)) => {
                        sessions.lock().map_err(|_| state::POISONED)?.withdraw_approval(run, id)?;

                        if !approved {
                            return Ok(ToolResult::text(
                                "The request was denied. No changes were made.",
                            ));
                        }

                        true
                    }

                    (Some(true), None) => true,

                    (Some(false), None) => {
                        return Ok(ToolResult::text(
                            "The request was denied. No changes were made.",
                        ));
                    }

                    (None, Some(run)) => {
                        sessions.lock().map_err(|_| state::POISONED)?.escalate_approval(run)?;
                        registry.route.handoff.send_replace(Some(String::new()));
                        plugins.jobs.wake.notify_one();

                        return Ok(ToolResult::text(
                            "The approval is saved with the task. Nothing ran; it continues after \
                                the decision.",
                        ));
                    }

                    (None, None) if !delivered => {
                        return Err("The channel approval handler is unavailable.".into());
                    }

                    (None, None) => {
                        return Err("The approval request is no longer available.".into());
                    }
                }
            }
        }
    };

    if let Some(offer) = registry.operations.get(name) {
        return operations::start(offer, &args, id, registry, plugins, sessions).await;
    }

    if changes && approve {
        let mut store = sessions.lock().map_err(|_| state::POISONED)?;

        store.phase_turn(session, registry.run.as_ref(), "unsafe")?;
    }

    if let Some(task) = task {
        let id = if let Some(run) = &registry.run {
            sessions
                .lock()
                .map_err(|_| state::POISONED)?
                .promote_foreground(run, &task.instruction)?;
            run.id.clone()
        } else {
            jobs::enqueue(
                &mut *sessions.lock().map_err(|_| state::POISONED)?,
                &task,
                &args["context"],
            )?
        };

        registry.route.handoff.send_replace(Some(task.message));
        plugins.jobs.wake.notify_one();

        return Ok(ToolResult::text(format!("Background job {id} is saved.")));
    }

    if name == "job" {
        return control_job(sessions, session, &args, plugins);
    }

    if name == settings::TOOL {
        let source =
            registry.source.as_ref().ok_or("This conversation has no settings section.")?;

        if !settings::offered(
            &plugins.config,
            source,
            approval_mode,
            registry.execution == state::SessionKind::Conversation,
            workspace.is_some_and(worktree::isolated),
        ) {
            return Err("Settings cannot be changed from this conversation.".into());
        }

        let (text, restart) = settings::apply(&home(), source, &args)?;

        if restart {
            registry.restart.store(true, std::sync::atomic::Ordering::Release);
        }

        return Ok(ToolResult::text(text));
    }

    if name == slash::TOOL {
        let cancels = Arc::new(Mutex::new(BTreeMap::new()));
        let stop = Stop::new();

        let host = slash::Host {
            plugins,
            sessions,
            cancels: &cancels,
            approvals: &approvals,
            stop: &stop,
            provider,
            model,
            call: id.saturating_add(1),
            answering: false,
            home: None,
        };

        let caller = slash::Caller {
            integration: channel,
            chat,
            thread,
            session,
            sender: None,
            tools: tools_enabled,
            owner: false,
            origin: slash::Origin::Tool,
        };

        return Ok(ToolResult::text(slash::call(&host, &caller, &args).await?));
    }

    if name == "message_send" {
        if registry.execution != state::SessionKind::Schedule {
            return Err("Message sending is available only to scheduled tasks.".into());
        }

        let message = scheduled_message(&args)?;

        let Some(destination) = scheduled_destination else {
            return Err("This schedule has no authorized message destination.".into());
        };

        if !matches_authorized_destination(&message, destination) {
            return Err("Message destination does not match the schedule's authorization.".into());
        }

        if message.channel == krabb_core::client::TUI {
            let run_id = schedule_run_id.ok_or("Scheduled run ID is missing.")?;
            let delivery_id = scheduled_delivery_id(session, run_id, tool_call);

            let message_id =
                scheduled_tui_notice_id(session, unix_timestamp_millis(), run_id, tool_call);

            let mut store = sessions.lock().map_err(|_| state::POISONED)?;

            store.ensure(&message.chat, model)?;

            if store.sessions[&message.chat]
                .messages
                .iter()
                .any(|existing| existing.id == delivery_id || existing.id.ends_with(&delivery_id))
            {
                return Ok(ToolResult::text(format!(
                    "Message was already added to TUI session {}.",
                    message.chat
                )));
            }

            store.append_notice(
                &message.chat,
                Message {
                    id: message_id,
                    session: message.chat.clone(),
                    role: Role::Assistant,
                    sender: Some("scheduler".into()),
                    content: vec![Content::Text { text: message.text }],
                },
            )?;

            info!(session = %message.chat, schedule = %session, "Scheduled message added to a TUI session.");

            return Ok(ToolResult::text(format!("Message added to TUI session {}.", message.chat)));
        }

        let plugin = plugins
            .get(&message.channel)
            .await
            .filter(|plugin| plugin.supports(Capability::Channel))
            .ok_or_else(|| format!("Channel plugin {} is not available.", message.channel))?;

        drop(plugin);

        let run_id = schedule_run_id.ok_or("Scheduled run ID is missing.")?;
        let delivery_id = scheduled_delivery_id(session, run_id, tool_call);

        sessions.lock().map_err(|_| state::POISONED)?.queue_delivery(
            delivery_id,
            &message.channel,
            &message.chat,
            message.private,
            message.thread,
            &message.text,
        )?;

        info!(channel = %message.channel, schedule = %session, "Scheduled message queued for delivery.");

        return Ok(ToolResult::text(format!(
            "Message queued for delivery through {}.",
            message.channel
        )));
    }

    let origin = if !registry.execution.interactive() {
        sessions
            .lock()
            .map_err(|_| state::POISONED)?
            .sessions
            .get(session)
            .and_then(|session| session.origin.clone())
    } else {
        None
    };

    let source = registry.source.clone().unwrap_or_else(|| sharing::principal(channel));

    let scope = origin
        .map_or_else(|| origin::scope(&source, session, chat, thread), |origin| origin.scope());

    tool_with_registry(
        plugins,
        registry,
        id,
        name,
        args,
        approve,
        workspace,
        &scope,
        session,
        turn_id,
        failed_tool,
    )
    .await
}

pub(crate) struct ScheduledMessage {
    pub(crate) channel: String,
    pub(crate) chat: String,
    pub(crate) private: bool,
    pub(crate) thread: Option<String>,
    pub(crate) text: String,
}

/// Applies a model-issued task control for the conversation's current sender.
pub(crate) fn control_job(
    sessions: &Arc<Mutex<state::Store>>,
    session: &str,
    args: &serde_json::Value,
    plugins: &Plugins,
) -> Result<ToolResult, Box<dyn std::error::Error + Send + Sync>> {
    let mut store = sessions.lock().map_err(|_| state::POISONED)?;
    let sender = jobs::sender(&store, session);

    Ok(jobs::refusal(jobs::control(
        &mut store,
        args,
        session,
        sender.as_deref(),
        &plugins.jobs,
        false,
    )))
}

pub(crate) fn scheduled_message(
    args: &serde_json::Value,
) -> Result<ScheduledMessage, &'static str> {
    let channel = args["channel"]
        .as_str()
        .filter(|value| valid(value))
        .ok_or("A valid installed channel ID is required.")?;

    let chat = args["chat"]
        .as_str()
        .filter(|value| {
            !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
        })
        .ok_or("A recipient ID of at most 256 visible characters is required.")?;

    let private = args["private"].as_bool().ok_or("The recipient type must be specified.")?;

    let text = args["text"]
        .as_str()
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= TEXT_LIMIT
                && !value.chars().any(|character| {
                    character.is_control() && character != '\n' && character != '\t'
                })
        })
        .ok_or("Message text is required and must fit within the message limit.")?;

    let thread =
        args["thread"].as_str().map(str::to_owned).filter(|value| !value.trim().is_empty());

    if channel == krabb_core::client::TUI
        && !valid_tui_destination(&AuthorizedDestination {
            channel: channel.into(),
            chat: chat.into(),
            private,
            thread: thread.clone(),
        })
    {
        return Err("TUI messages must target a TUI session without a thread.");
    }

    if thread.as_ref().is_some_and(|value| value.len() > 256 || value.chars().any(char::is_control))
    {
        return Err("Thread IDs must contain at most 256 visible characters.");
    }

    Ok(ScheduledMessage {
        channel: channel.into(),
        chat: chat.into(),
        private,
        thread,
        text: text.into(),
    })
}

pub(crate) fn approval_text(name: &str, args: &serde_json::Value) -> String {
    let details = match name {
        "job" => {
            let detail = match args["action"].as_str() {
                Some("steer") => format!(
                    " with instruction: {}",
                    args["instruction"].as_str().unwrap_or_default()
                ),
                Some("defer") => format!(" until {}", args["when"].as_str().unwrap_or_default()),
                Some("extend") => format!(" by {} minutes", args["minutes"]),
                _ => String::new(),
            };

            format!(
                "{} task {}{detail}",
                args["action"].as_str().unwrap_or_default(),
                args["id"].as_str().unwrap_or_default()
            )
        }

        "task" => format!(
            "start a background goal with automatic continuation and updates to {} / {}; \
                unattended workspace tools: {}, shell: {}, workspace: {}",
            args["context"]["origin"]["channel"],
            args["context"]["origin"]["chat"],
            args["context"]["tools"] == true,
            args["context"]["shell"] == true,
            args["context"]["workspace"],
        ),

        "schedule_propose" => {
            let destination = if let Some(destinations) = args["context"].get("destinations") {
                format!("outbound destinations {destinations}")
            } else {
                args["context"]
                    .get("message_destination")
                    .filter(|value| !value.is_null())
                    .map_or_else(
                        || "no outbound message destination".into(),
                        |value| {
                            format!(
                                "outbound messages to channel {}, chat {}, private {}, thread {}",
                                value["channel"].as_str().unwrap_or("invalid"),
                                value["chat"].as_str().unwrap_or("invalid"),
                                value["private"].as_bool().unwrap_or(false),
                                value["thread"].as_str().unwrap_or("none"),
                            )
                        },
                    )
            };

            format!(
                "create a scheduled task with {destination}; unattended workspace tools: {}, shell: {}, workspace: {}",
                args["context"]["tools"] == true,
                args["context"]["shell"] == true,
                args["context"]["workspace"],
            )
        }

        "write" => format!("write to {}", args["path"].as_str().unwrap_or("a workspace path")),

        "memory_remember" => {
            format!("save memory {}", args["key"].as_str().unwrap_or("with the requested key"))
        }

        "patch" => {
            format!(
                "apply a Git patch containing {} bytes, which may execute repository-configured programs",
                args["text"].as_str().map_or(0, str::len)
            )
        }

        "shell" => {
            format!("run this shell command: {}", args["command"].as_str().unwrap_or("unknown"))
        }

        "git" => format!(
            "run this Git operation, which may execute repository-configured programs: {}",
            args["args"]
        ),
        _ => format!("run the {name} tool"),
    };

    format!("Krabb requests approval to {details}.\nArguments: {args}")
}

pub(crate) fn approval_params(
    channel: &str,
    chat: &str,
    thread: Option<&str>,
    text: &str,
    approve: &str,
    deny: &str,
) -> krabb_core::Result<serde_json::Value> {
    let mut params = match channel {
        "telegram" => serde_json::json!({
            "chat": chat
                .parse::<i64>()
                .map_err(|_| krabb_core::Error::Denied("Telegram chat ID was invalid.".into()))?,
            "text": text,
            "approve": approve,
            "deny": deny,
        }),

        "discord" => serde_json::json!({
            "channel": chat,
            "text": text,
            "approve": approve,
            "deny": deny,
        }),

        _ => {
            return Err(krabb_core::Error::Denied(
                "The channel does not support inline approvals.".into(),
            ));
        }
    };

    if channel == "telegram"
        && let Some(thread) = thread
    {
        params["thread"] = serde_json::Value::String(thread.into());
    }

    Ok(params)
}

#[allow(clippy::too_many_arguments)]
#[cfg(all(test, unix))]
pub(crate) async fn host_tool(
    request: Request,
    plugins: &Plugins,
    sessions: &Arc<Mutex<state::Store>>,
    session: &str,
    provider: &str,
    model: &str,
    workspace: Option<&Path>,
    tools_enabled: bool,
    schedule_enabled: bool,
    shell_enabled: bool,
    approval_mode: ApprovalMode,
    approvals: Arc<AsyncMutex<approval::Gate>>,
    channel: &str,
    chat: &str,
    thread: Option<&str>,
    notices: tokio::sync::mpsc::Sender<StreamNotice>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    deadline: tokio::time::Instant,
    scheduled_destination: Option<AuthorizedDestination>,
    schedule_run_id: Option<u64>,
) -> krabb_core::Result<Response> {
    let mut registry = legacy_tool_registry(None);

    if channel == "scheduler" {
        registry.execution = state::SessionKind::Schedule;
    }

    host_tool_with_registry(
        request,
        plugins,
        sessions,
        session,
        provider,
        model,
        workspace,
        tools_enabled,
        schedule_enabled,
        shell_enabled,
        approval_mode,
        approvals,
        channel,
        chat,
        thread,
        notices,
        calls,
        registry,
        "test-turn".into(),
        deadline,
        scheduled_destination,
        schedule_run_id,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn host_tool_with_registry(
    request: Request,
    plugins: &Plugins,
    sessions: &Arc<Mutex<state::Store>>,
    session: &str,
    provider: &str,
    model: &str,
    workspace: Option<&Path>,
    tools_enabled: bool,
    schedule_enabled: bool,
    shell_enabled: bool,
    approval_mode: ApprovalMode,
    approvals: Arc<AsyncMutex<approval::Gate>>,
    channel: &str,
    chat: &str,
    thread: Option<&str>,
    notices: tokio::sync::mpsc::Sender<StreamNotice>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    registry: ToolRegistry,
    turn_id: String,
    deadline: tokio::time::Instant,
    scheduled_destination: Option<AuthorizedDestination>,
    schedule_run_id: Option<u64>,
) -> krabb_core::Result<Response> {
    let Request::Call { id, method, params, .. } = request else {
        return Err(krabb_core::Error::Protocol("Host tool requests require an ID.".into()));
    };

    if method != "host/tool" {
        return Ok(Response::fail(id, -32601, "method not found"));
    }

    let Some(name) = params["name"].as_str() else {
        return Ok(Response::fail(id, -32602, "A tool name is required."));
    };

    if name.starts_with("schedule_") && !registry.execution.interactive() {
        return Ok(Response::fail(
            id,
            -32000,
            "Scheduled tasks cannot create or change schedules.",
        ));
    }

    if name.starts_with("schedule_") && !schedule_enabled {
        return Ok(Response::fail(id, -32000, "Scheduling is not authorized for this sender."));
    }

    if name == "schedule_list" && !schedule_listing_allowed(registry.execution) {
        return Ok(Response::fail(
            id,
            -32000,
            "Schedule listing is only available in the local CLI and TUI.",
        ));
    }

    let scheduler_tool = name.starts_with("schedule_") && registry.execution.interactive();

    let scheduled_message_tool =
        name == "message_send" && registry.execution == state::SessionKind::Schedule;

    if !tools_enabled && !scheduler_tool && !scheduled_message_tool && !HOST_TOOLS.contains(&name) {
        return Ok(Response::fail(id, -32000, "Tool access is disabled for this channel."));
    }

    let tool_call = calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    if tool_call >= TOOL_CALLS {
        return Ok(Response::fail(id, -32000, "The turn exceeded its tool-call limit."));
    }

    if cancelled(sessions, session).unwrap_or(true) {
        return Ok(Response::fail(id, -32000, "The turn was cancelled."));
    }

    let args = params.get("args").cloned().unwrap_or(serde_json::Value::Null);

    if let Err(message) = validate_tool_args(&args) {
        return Ok(Response::fail(id, -32602, message));
    }

    if let Err(message) = validate_registered_args(&registry, name, &args) {
        return Ok(Response::fail(id, -32602, format!("Tool arguments are invalid: {message}")));
    }

    let mut failed_tool = None;

    let output = match tokio::time::timeout_at(
        deadline,
        execute_tool_with_registry(
            plugins,
            &registry,
            &turn_id,
            id,
            name,
            args,
            provider,
            model,
            approval_mode,
            approvals,
            channel,
            chat,
            thread,
            session,
            sessions,
            workspace,
            tools_enabled,
            schedule_enabled,
            shell_enabled,
            notices,
            deadline,
            &mut failed_tool,
            scheduled_destination.as_ref(),
            schedule_run_id,
            tool_call,
        ),
    )
    .await
    {
        Ok(Ok(output)) => output,

        Ok(Err(error)) => {
            if let Some(run) = &registry.run {
                sessions
                    .lock()
                    .map_err(|_| krabb_core::Error::Protocol(state::POISONED.into()))?
                    .uncertain_job_step(&run.id, run.generation)
                    .map_err(krabb_core::Error::Io)?;
            }

            if let Some(failed) = failed_tool {
                restart_tool(plugins, &failed).await;
            }

            return Ok(Response::fail(id, -32000, sentence(error.to_string())));
        }

        Err(_) => {
            if let Some(run) = &registry.run {
                sessions
                    .lock()
                    .map_err(|_| krabb_core::Error::Protocol(state::POISONED.into()))?
                    .uncertain_job_step(&run.id, run.generation)
                    .map_err(krabb_core::Error::Io)?;
            }

            if let Some(failed) = registry.owners.get(name) {
                restart_tool(plugins, failed).await;
            }

            return Ok(Response::fail(id, -32000, "The turn exceeded its time limit."));
        }
    };

    Ok(Response::ok(id, tool_result_value(output)?))
}

pub(crate) fn tool_result_value(result: ToolResult) -> krabb_core::Result<serde_json::Value> {
    let mut text = tool_result_text(&result);

    if let Some(metadata) = &result.metadata {
        text.push_str("\nTool metadata: ");
        text.push_str(&metadata.to_string());
    }

    let mut value = serde_json::to_value(result)?;

    if let serde_json::Value::Object(fields) = &mut value {
        fields.insert("text".into(), serde_json::Value::String(text));
    }

    Ok(value)
}

pub(crate) fn track_scheduled_tool_response(
    response: Response,
    failures: Option<&Arc<Mutex<Vec<String>>>>,
) -> Response {
    if let (Some(failures), Some(error)) = (failures, response.error.as_ref())
        && let Ok(mut failures) = failures.lock()
        && failures.len() < 8
    {
        failures.push(clip(diagnostic(&error.message), 500));
    }

    response
}

pub(crate) async fn restart_tool(plugins: &Plugins, id: &str) {
    let Some(process) = plugins.get(id).await else {
        return;
    };

    match process.restart().await {
        Ok(()) => info!(plugin = id, "Plugin restarted after a tool transport failure."),
        Err(restart) => warn!(
            plugin = id,
            error = %diagnostic(sentence(restart.to_string())),
            "Plugin could not restart after a tool transport failure."
        ),
    }
}
