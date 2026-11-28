//! Implements the `krabb schedule` command family.

use crate::*;

pub(crate) async fn schedule_command(
    args: &[String],
    root: &Path,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (args, target) = composition::target(args)?;
    let args = args.as_slice();
    let plugin = target.as_deref();

    let Some(action) = args.first().map(String::as_str) else {
        println!("{}", style_cli_help(schedule_help()));
        return Ok(());
    };

    if matches!(action, "help" | "--help" | "-h") {
        println!("{}", style_cli_help(schedule_help()));
        return Ok(());
    }

    if action == "history" {
        let selector = args.get(1).ok_or("Usage: krabb schedule history <id-or-name>.")?;
        let mut page = 1_usize;
        let mut page_seen = false;
        let mut raw = false;
        let mut all = false;

        for argument in &args[2..] {
            match argument.as_str() {
                "--raw" if !raw => raw = true,
                "--all" if !all => all = true,

                value if !page_seen => {
                    page = value.parse::<usize>().ok().filter(|page| *page > 0).ok_or(
                        "Usage: krabb schedule history <id-or-name> [PAGE] [--all|--raw].",
                    )?;
                    page_seen = true;
                }

                _ => {
                    return Err(
                        "Usage: krabb schedule history <id-or-name> [PAGE] [--all|--raw].".into()
                    );
                }
            }
        }

        if raw && all {
            return Err("Choose --all or --raw, not both.".into());
        }

        if page_seen && (raw || all) {
            return Err("Choose a history page or --all/--raw, not both.".into());
        }

        if raw && json {
            return Err("Choose --raw or --json, not both.".into());
        }

        let id = resolve_schedule_id(root, plugin, selector).await?;
        let path = schedule_log_path(root, id)?;

        if raw {
            match std::fs::File::open(&path) {
                Ok(mut file) => {
                    std::io::copy(&mut file, &mut std::io::stdout().lock())?;
                }

                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}

                Err(error) => return Err(error.into()),
            }
        } else if all {
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error.into()),
            };

            let records = parse_schedule_history(&content)?;

            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else {
                print!(
                    "{}",
                    format_schedule_history(&records, records.len(), &path, selector, 1, true)
                );
            }
        } else {
            let (records, total) = read_schedule_history_page(&path, page)?;

            if page > total.div_ceil(SCHEDULE_HISTORY_PAGE_SIZE).max(1) {
                return Err(format!(
                    "Schedule history has {} page(s), so page {page} is out of range.",
                    total.div_ceil(SCHEDULE_HISTORY_PAGE_SIZE).max(1)
                )
                .into());
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else {
                print!(
                    "{}",
                    format_schedule_history(&records, total, &path, selector, page, false)
                );
            }
        }

        info!(schedule = id, "Schedule history displayed.");
        return Ok(());
    }

    if action == "add" {
        let (params, instruction) = parse_schedule_add(&args[1..], root)?;
        let provider = params["context"]["provider"]
            .as_str()
            .ok_or("Schedule creation requires --provider and --model.")?
            .to_owned();
        let model = params["context"]["model"]
            .as_str()
            .filter(|model| !model.trim().is_empty() && *model != "unset")
            .ok_or("Schedule creation requires --provider and --model.")?
            .to_owned();
        let provider = model_plugin_at(root, &provider)?;
        let id = schedule_id()?;
        let mut params = params;
        params["id"] = serde_json::json!(id);
        params["text"] = serde_json::json!(instruction);
        let workspace = current_schedule_workspace()?;
        params["context"]["workspace"] = serde_json::json!(workspace.display().to_string());

        params["context"]["provider"] = serde_json::json!(provider);

        params["context"]["origin"] = serde_json::json!({"channel": "cli", "session": ""});
        params["activate"] = serde_json::json!(true);
        let workspace_tools_enabled = params["context"]["tools"] == true;
        let shell_enabled = workspace_tools_enabled && params["context"]["shell"] == true;
        let value = schedule_call(root, plugin, "add", params).await?;
        info!(schedule = id, "Scheduled task activated.");

        if json {
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            let name = value["name"].as_str().unwrap_or("Scheduled task");
            println!("Schedule {name} ({id}) is active.\n");
            println!("Instruction: {instruction}");
            println!("Provider: {provider}");
            println!("Model: {model}");
            println!("Workspace: {}", workspace.display());
            println!("Tools: {}", if workspace_tools_enabled { "enabled" } else { "disabled" });

            println!("Shell: {}", if shell_enabled { "enabled" } else { "disabled" });

            if let Some(due) = value["due"].as_u64() {
                println!(
                    "Next run: {}",
                    krabb_date::format_datetime_in_timezone(
                        due,
                        value["zone"].as_str().unwrap_or("UTC")
                    )
                );
            }

            if let Some(repeat) = value["repeat"].as_u64().filter(|repeat| *repeat > 0) {
                println!("Frequency: every {}", krabb_date::format_duration(repeat));
            } else if let Some(cron) = value["cron"].as_str() {
                println!("Frequency: {cron} ({})", value["zone"].as_str().unwrap_or("UTC"));
            }

            println!("Automatic retries: {}", value["retries"].as_u64().unwrap_or_default());

            if value["on_failure"] == "continue" {
                println!("After retries: skip the failed occurrence and keep the next run.");
            }

            println!(
                "\nCheck it with `krabb schedule list`\nPause it with `krabb schedule pause {id}`"
            );
        }

        return Ok(());
    }

    if action == "update" {
        let selector = args
            .get(1)
            .ok_or("Usage: krabb schedule update <id-or-name> --to <destination>... | --clear.")?;
        let destinations = schedules::update(&args[2..])?;
        let id = resolve_schedule_id(root, plugin, selector).await?;
        let value = schedule_call(
            root,
            plugin,
            "update",
            serde_json::json!({
                "id": id, "destinations": destinations, "approved": true
            }),
        )
        .await?;

        if json {
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            println!("Schedule {id} destinations updated.");
        }

        return Ok(());
    }

    if action == "list" {
        let page = args.get(1).map_or(Ok(1), |value| value.parse::<u64>())?;

        if page == 0 || args.len() > 2 {
            return Err("Usage: krabb schedule list [PAGE].".into());
        }

        let value = schedule_call(root, plugin, "list", serde_json::json!({"page": page})).await?;
        info!("Schedule list displayed.");

        if json {
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            println!("{}", format_schedule_list(&value));
        }

        return Ok(());
    }

    if matches!(action, "run" | "retry") {
        let selector = args.get(1).ok_or_else(|| schedule_usage(action))?;

        if args.len() > 2 {
            return Err(schedule_usage(action).into());
        }

        let id = resolve_schedule_id(root, plugin, selector).await?;
        let name = resolve_schedule_name(root, plugin, id).await?;
        let value = schedule_call(root, plugin, action, serde_json::json!({"id": id})).await?;
        info!(schedule = id, action, "Scheduled task execution requested.");

        if json {
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else if action == "retry" {
            println!("Schedule {name} ({id}) retry queued.");
        } else {
            println!("Schedule {name} ({id}) queued to run now.");
        }

        if !json {
            println!("Check the outcome with `krabb schedule history {selector}`.");
        }

        return Ok(());
    }

    let selector = args.get(1).ok_or_else(|| schedule_usage(action))?;

    if args.len() > 3 {
        return Err(schedule_usage(action).into());
    }

    let id = resolve_schedule_id(root, plugin, selector).await?;
    let name = resolve_schedule_name(root, plugin, id).await?;

    if action == "approve" {
        let mut page = 1;
        let task = loop {
            let value =
                schedule_call(root, plugin, "list", serde_json::json!({"page": page})).await?;
            let pages = value["pages"].as_u64().unwrap_or(1);

            if let Some(task) = value["items"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|item| item["id"].as_u64() == Some(id))
                .cloned()
            {
                break task;
            }

            if page >= pages {
                return Err(format!("Schedule {id} was not found.").into());
            }

            page += 1;
        };

        if task["status"] != "pending" {
            return Err(format!("Schedule {id} is not awaiting approval.").into());
        }

        if !json {
            let name = task["name"].as_str().unwrap_or("Scheduled task");
            println!("Approve schedule {name} ({id})?");
            println!("Instruction: {}", task["text"].as_str().unwrap_or_default());
            println!(
                "Next run: {}",
                task["due"]
                    .as_u64()
                    .map(|due| {
                        krabb_date::format_datetime_in_timezone(
                            due,
                            task["zone"].as_str().unwrap_or("UTC"),
                        )
                    })
                    .unwrap_or_else(|| "unknown".into())
            );

            if let Some(cron) = task["cron"].as_str() {
                println!(
                    "Cron: {cron} ({})",
                    task["zone"].as_str().unwrap_or("timezone unavailable")
                );
            }

            println!(
                "Workspace: {}",
                task["context"]["workspace"].as_str().unwrap_or("configured workspace")
            );

            println!(
                "Workspace tools: {}",
                if task["context"]["tools"] == true { "enabled" } else { "disabled" }
            );

            println!("Automatic retries: {}", task["retries"].as_u64().unwrap_or_default());
        }

        let yes = args.iter().any(|argument| argument == "--yes" || argument == "-y");

        if !confirm_action(yes, json, &format!("Approve scheduled task {id}?"))? {
            print_cancelled("Schedule approval", json);
            return Ok(());
        }
    }

    let method = match action {
        "approve" | "pause" | "resume" => action,
        "cancel" | "remove" => "remove",
        _ => return Err(schedule_usage(action).into()),
    };

    let value = schedule_call(root, plugin, method, serde_json::json!({"id": id})).await?;
    info!(schedule = id, action, "Scheduled task updated.");

    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else if method == "remove" {
        println!(
            "{}",
            if value["cancel_requested"] == true {
                format!(
                    "Cancellation requested for schedule {name} ({id}).\nIts current action will \
                    be interrupted, and a side effect may already have happened."
                )
            } else if value["deleted"] == true {
                format!("Schedule {name} ({id}) cancelled.")
            } else {
                format!("Schedule {name} ({id}) was not found.")
            }
        );
    } else if method == "pause" && value["pause_requested"] == true {
        println!(
            "Pause requested for schedule {name} ({id}).\nThe current run will finish, then the \
             schedule will stay paused."
        );
    } else {
        println!("Schedule {name} ({id}) is now {}.", value["status"].as_str().unwrap_or(action));
    }

    Ok(())
}

pub(crate) fn schedule_help() -> &'static str {
    r#"Schedule autonomous agent work that becomes active immediately.

Usage: krabb schedule <COMMAND>

Commands:
  add <INSTRUCTION>         Add a one-time or recurring task
  list [PAGE]               List scheduled tasks
  approve <ID> [-y|--yes]   Approve a pending task without prompting
  run <ID|NAME>             Run a task now
  update <ID|NAME>          Replace destinations with --to, or remove with --clear
  retry <ID|NAME>           Retry a failed or offline-skipped task
  pause <ID|NAME>           Pause a task
  resume <ID|NAME>          Resume a paused task
  cancel <ID|NAME>          Cancel a task
  history <ID|NAME> [PAGE]  Show task execution history
  help                      Show this help

Options:
  --after <DURATION>        Run after a delay, such as 5m or "1 month"
  --at <DATE-TIME>          Run at "YYYY-MM-DD HH:MM" locally or RFC 3339
  --every <DURATION>        Repeat every 5 minutes to 1 year
  --cron <EXPRESSION>       Repeat on a five-field cron expression
  --timezone <AREA/CITY>    Timezone for local --at and cron; uses scheduler config
  --provider <ID>           Installed intelligence provider for each run
  --model <NAME>            Model to use for each run
  --name <NAME>             Selectable name: letters, numbers, hyphens, underscores, dots
  --missed <POLICY>         Downtime policy: skip, coalesce, or replay (max 5)
  --retries <COUNT>         Automatic retries with backoff (0 to 5, default: 0)
  --on-failure <POLICY>     Recurring task policy: pause or continue (default: pause)
  --shell                   Enable shell if configured and tools are installed
  --to <DESTINATION>       Deliver the result to channel:private|group:chat[:thread]; repeatable
  --message-channel <ID>    Channel for an authorized message; use tui for a local session
  --message-chat <ID>       Recipient or TUI session ID (use tui-default for the default chat)
  --message-kind <KIND>     Recipient type: private or group (use group for tui)
  --message-thread <ID>     Optional message thread
  --plugin <ID>             Send the action to this scheduler instead of the primary

History options:
  --raw                     Print the original JSONL history
  --all                     Show every recorded run

Notes:
  CLI-created tasks require --provider and --model; TUI tasks use the current
  provider and model. CLI tasks capture the current directory as their workspace.
  Names contain no spaces and can be used instead of IDs.
  Scheduling requests activate immediately without a separate approval step.
  Use seconds (60s), minutes (5m), hours (2h), days (1d), weeks (1w), months
  (1mo), or years (1y); written units such as "1 month" also work. Months are
  30 days and years are 365 days. Quote --at values containing spaces. Local
  --at values use --timezone, scheduler.timezone, or the machine's timezone.
  Choose --after or --at for a one-time task,
  --cron for cron recurrence, or --every for a fixed interval. Fixed intervals
  start after one interval unless --after or --at sets the first run. Cron needs
  --timezone unless one is configured. Set all three message destination options
  together. Phrases such as "default TUI chat" capture tui-default as the local
  message destination. Uncertain side effects pause for review. A recurring task
  continues after exhausted retries only with --on-failure continue."#
}

pub(crate) fn schedule_usage(action: &str) -> String {
    format!("Unknown schedule command {action}.\nRun `krabb schedule help` for usage.")
}

pub(crate) fn parse_schedule_add(
    args: &[String],
    root: &Path,
) -> Result<(serde_json::Value, String), Box<dyn std::error::Error + Send + Sync>> {
    let mut delay = None;
    let mut repeat = None;
    let mut at_input = None;
    let mut cron = None;
    let mut timezone = None;
    let mut missed = None;
    let mut on_failure = None;
    let mut retries = None;
    let mut shell = false;
    let mut destinations = Vec::new();
    let mut message_channel = None;
    let mut message_chat = None;
    let mut message_kind = None;
    let mut message_thread = None;
    let mut name = None;
    let mut provider = None;
    let mut model = None;
    let mut instruction = Vec::new();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--after" => {
                index += 1;
                delay = Some(parse_schedule_duration(
                    args.get(index).ok_or("--after requires a duration.")?,
                    "--after",
                )?);
            }

            "--every" => {
                index += 1;
                repeat = Some(parse_schedule_duration(
                    args.get(index).ok_or("--every requires a duration.")?,
                    "--every",
                )?);
            }

            "--at" => {
                index += 1;
                at_input = Some(
                    args.get(index)
                        .ok_or("--at requires a date and time, such as \"2026-10-20 09:00\".")?
                        .clone(),
                );
            }

            "--cron" => {
                index += 1;
                cron = Some(
                    args.get(index).ok_or("--cron requires a five-field expression.")?.clone(),
                );
            }

            "--timezone" => {
                index += 1;
                timezone =
                    Some(args.get(index).ok_or("--timezone requires an IANA timezone.")?.clone());
            }

            "--missed" => {
                index += 1;
                missed = Some(
                    args.get(index).ok_or("--missed requires skip, coalesce, or replay.")?.clone(),
                );
            }

            "--on-failure" => {
                index += 1;
                on_failure = Some(
                    args.get(index).ok_or("--on-failure requires pause or continue.")?.clone(),
                );
            }

            "--retries" => {
                index += 1;
                let retries_value = args
                    .get(index)
                    .ok_or("--retries requires a number from 0 to 5.")?
                    .parse::<u8>()?;

                if retries_value > 5 {
                    return Err(
                        "Scheduled tasks can be configured for at most five retries.".into()
                    );
                }

                retries = Some(retries_value);
            }

            "--to" => {
                index += 1;
                destinations.push(schedules::destination(
                    args.get(index).ok_or("--to requires channel:private|group:chat[:thread].")?,
                )?);
            }

            "--message-channel" => {
                index += 1;
                message_channel = Some(
                    args.get(index).ok_or("--message-channel requires a channel ID.")?.clone(),
                );
            }

            "--message-chat" => {
                index += 1;
                message_chat =
                    Some(args.get(index).ok_or("--message-chat requires a chat ID.")?.clone());
            }

            "--message-kind" => {
                index += 1;
                message_kind = Some(
                    args.get(index).ok_or("--message-kind requires private or group.")?.clone(),
                );
            }

            "--message-thread" => {
                index += 1;
                message_thread =
                    Some(args.get(index).ok_or("--message-thread requires a thread ID.")?.clone());
            }

            "--name" => {
                index += 1;
                name =
                    Some(args.get(index).ok_or("--name requires a short schedule name.")?.clone());
            }

            "--provider" => {
                index += 1;
                provider = Some(
                    args.get(index).ok_or("--provider requires an installed provider ID.")?.clone(),
                );
            }

            "--model" => {
                index += 1;
                model = Some(args.get(index).ok_or("--model requires a model name.")?.clone());
            }

            "--shell" => shell = true,

            "--" => {
                instruction.extend_from_slice(&args[index + 1..]);
                break;
            }

            argument if argument.starts_with('-') => {
                return Err(format!("Unknown schedule option {argument}.").into());
            }

            _ => instruction.push(args[index].clone()),
        }

        index += 1;
    }

    let instruction = instruction.join(" ");

    if instruction.trim().is_empty() {
        return Err("A scheduled instruction is required.".into());
    }

    let message_destination = match (message_channel, message_chat, message_kind) {
        (None, None, None) if message_thread.is_none() => {
            if destinations.is_empty() {
                requested_tui_destination(&instruction)
            } else {
                None
            }
        }

        (Some(channel), Some(chat), Some(kind)) => {
            if message_thread.as_ref().is_some_and(|thread| thread.trim().is_empty()) {
                return Err("The scheduled message thread is invalid.".into());
            }

            let value = serde_json::json!({
                "channel": channel,
                "chat": chat,
                "private": match kind.as_str() {
                    "private" => true,
                    "group" => false,
                    _ => return Err("Message kind must be private or group.".into()),
                },

                "thread": message_thread,
            });

            Some(
                authorized_destination(&value)
                    .ok_or("The scheduled message destination is invalid.")?,
            )
        }

        _ => {
            return Err(
                "Set --message-channel, --message-chat, and --message-kind together, with an \
                optional --message-thread."
                    .into(),
            );
        }
    };

    krabb_core::schedule::validate_destinations(&destinations)?;

    if !destinations.is_empty() && message_destination.is_some() {
        return Err("Choose --to or the legacy message options, not both.".into());
    }

    if cron.is_some() && repeat.is_some() {
        return Err("Choose either --cron or --every for a recurring schedule.".into());
    }

    let interval_starts_now = repeat.is_some() && at_input.is_none() && cron.is_none();
    let selected_times =
        [delay.is_some() || interval_starts_now, at_input.is_some(), cron.is_some()]
            .into_iter()
            .filter(|selected| *selected)
            .count();

    if selected_times != 1 {
        return Err("Choose exactly one of --after, --at, or --cron.".into());
    }

    if delay.is_some_and(|value| !(5..=31_536_000).contains(&value)) {
        return Err("Schedule delay must be between five seconds and one year.".into());
    }

    if repeat.is_some_and(|value| !(5..=31_536_000).contains(&value)) {
        return Err("Schedule repeat interval must be between five seconds and one year.".into());
    }

    if delay.is_none() && at_input.is_none() && cron.is_none() && repeat.is_some() {
        delay = repeat;
    }

    let config: Config = load_config_at(root)?;

    let timezone = timezone.or_else(|| config.scheduler.timezone.clone());

    let at = at_input
        .as_deref()
        .map(|value| krabb_date::parse_datetime(value, timezone.as_deref()))
        .transpose()?;

    if at.is_some_and(|seconds| seconds < unix_seconds().saturating_add(5)) {
        return Err("Scheduled times must be at least five seconds in the future.".into());
    }

    if cron.is_some() && timezone.is_none() {
        return Err("Cron schedules need a timezone.\nSet scheduler.timezone in config.toml or \
                   pass --timezone."
            .into());
    }

    if missed.as_deref().is_some_and(|value| !matches!(value, "skip" | "coalesce" | "replay")) {
        return Err("Missed-run policy must be skip, coalesce, or replay.".into());
    }

    if on_failure.as_deref().is_some_and(|value| !matches!(value, "pause" | "continue")) {
        return Err("Failure policy must be pause or continue.".into());
    }

    if on_failure.as_deref() == Some("continue") && repeat.is_none() && cron.is_none() {
        return Err("Failure policy continue requires a recurring schedule.".into());
    }

    if cron.is_some() && missed.is_none() && config.scheduler.missed == "replay" {
        return Err("The global missed-run policy cannot be replay, so choose it per schedule \
                   with --missed replay."
            .into());
    }

    let mut value = serde_json::json!({
        "delay": delay.unwrap_or(0),
        "zone": timezone.unwrap_or_else(|| "UTC".into())
    });

    if let Some(repeat) = repeat {
        value["repeat"] = serde_json::json!(repeat);
    }

    if let Some(at) = at {
        value["at"] = serde_json::json!(at);
    }

    if let Some(cron) = cron {
        value["cron"] = serde_json::Value::String(cron);
    }

    if let Some(missed) = missed {
        value["missed"] = serde_json::Value::String(missed);
    }

    if let Some(on_failure) = on_failure {
        value["on_failure"] = serde_json::Value::String(on_failure);
    }

    if let Some(retries) = retries {
        value["retries"] = serde_json::json!(retries);
    }

    let lock = load_lock_at(root)?;
    let tools_enabled = lock.plugins.iter().any(|(id, entry)| {
        entry.capabilities.iter().any(|capability| capability == "tool")
            && binary_at(id, root).is_some()
    });

    if shell && !tools_enabled {
        return Err("--shell requires an installed tools plugin.".into());
    }

    if shell && !config.tui_shell() {
        return Err(
            "--shell requires shell access to be enabled in the Krabb configuration.".into()
        );
    }

    value["context"] = serde_json::json!({
        "tools": tools_enabled,
        "shell": shell,
        "message_destination": message_destination,
    });

    if !destinations.is_empty() {
        value["context"]["destinations"] = serde_json::to_value(destinations)?;
    }

    capture_model_selection(&mut value["context"], provider, model);

    if let Some(name) = name {
        if name.chars().count() > 80 || !valid_schedule_name(name.trim()) {
            return Err("Schedule names must start with a letter or number and contain only letters, numbers, hyphens, underscores, or dots (up to 80 characters).".into());
        }

        value["name"] = serde_json::Value::String(name.trim().to_owned());
    }

    Ok((value, instruction.trim().to_owned()))
}

pub(crate) fn parse_schedule_duration(value: &str, option: &str) -> Result<u64, String> {
    let seconds = match value.parse::<u64>() {
        Ok(seconds) => seconds,
        Err(_) => krabb_date::parse_duration(value)
            .map_err(|_| format!("{option} needs a duration such as 5m or \"1 month\"."))?,
    };

    if !(5..=31_536_000).contains(&seconds) {
        return Err(format!("{option} must be between 5 seconds and 1 year."));
    }

    Ok(seconds)
}

pub(crate) fn valid_schedule_name(name: &str) -> bool {
    let mut characters = name.chars();

    characters.next().is_some_and(|character| character.is_ascii_alphanumeric())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

pub(crate) fn requested_tui_destination(instruction: &str) -> Option<AuthorizedDestination> {
    let normalized = instruction.to_ascii_lowercase();

    ["default tui chat", "default tui session", "default chat in tui"]
        .iter()
        .any(|phrase| normalized.contains(phrase))
        .then(|| AuthorizedDestination {
            channel: "tui".into(),
            chat: "tui-default".into(),
            private: false,
            thread: None,
        })
}

pub(crate) async fn schedule_call(
    root: &Path,
    plugin: Option<&str>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut request =
        serde_json::json!({"service": "scheduler", "method": method, "params": params});

    if let Some(plugin) = plugin {
        request["plugin"] = plugin.into();
    }

    ipc::call(root, "capability.call", request).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::ConnectionRefused
            || error.kind() == std::io::ErrorKind::NotFound
        {
            "The daemon is not running.\nStart it with `krabb service start` to manage schedules."
                .into()
        } else {
            error.into()
        }
    })
}

pub(crate) async fn resolve_schedule_id(
    root: &Path,
    plugin: Option<&str>,
    selector: &str,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(id) = selector.parse::<u64>() {
        return Ok(id);
    }

    let mut page = 1;

    loop {
        let value = schedule_call(root, plugin, "list", serde_json::json!({"page": page})).await?;
        let items =
            value["items"].as_array().ok_or("The scheduler returned an invalid task list.")?;

        if let Some(item) = items.iter().find(|item| item["name"].as_str() == Some(selector)) {
            return item["id"].as_u64().ok_or_else(|| "The schedule has an invalid ID.".into());
        }

        if page >= value["pages"].as_u64().unwrap_or(1) {
            return Err(format!("No schedule named {selector} was found.").into());
        }

        page += 1;
    }
}

pub(crate) async fn resolve_schedule_name(
    root: &Path,
    plugin: Option<&str>,
    id: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut page = 1;

    loop {
        let value = schedule_call(root, plugin, "list", serde_json::json!({"page": page})).await?;
        let items =
            value["items"].as_array().ok_or("The scheduler returned an invalid task list.")?;

        if let Some(item) = items.iter().find(|item| item["id"].as_u64() == Some(id)) {
            return Ok(item["name"].as_str().unwrap_or("Scheduled task").to_owned());
        }

        if page >= value["pages"].as_u64().unwrap_or(1) {
            return Err(format!("Schedule {id} was not found.").into());
        }

        page += 1;
    }
}

pub(crate) fn format_schedule_list(value: &serde_json::Value) -> String {
    let Some(items) = value["items"].as_array() else {
        return NO_SCHEDULES.into();
    };

    if items.is_empty() {
        return NO_SCHEDULES.into();
    }

    let total = value["total"].as_u64().unwrap_or(items.len() as u64);
    let page = value["page"].as_u64().unwrap_or(1);
    let pages = value["pages"].as_u64().unwrap_or(1);
    let from = value["from"].as_u64().unwrap_or(1);
    let to = value["to"].as_u64().unwrap_or(items.len() as u64);
    let rows = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let id = item["id"].as_u64()?;
            let status = item["status"].as_str().unwrap_or("pending");
            let due = item["due"].as_u64().map(|due| {
                krabb_date::format_datetime_in_timezone(due, item["zone"].as_str().unwrap_or("UTC"))
            });

            let mut details = Vec::new();

            if let Some(due) = due {
                details.push(format!("Next run: {due}"));
            } else {
                details.push("No upcoming run".into());
            }

            if let Some(cron) = item["cron"].as_str() {
                details.push(format!(
                    "Repeats on cron {cron} in {}",
                    item["zone"].as_str().unwrap_or("UTC")
                ));
            } else if let Some(repeat) = item["repeat"].as_u64().filter(|repeat| *repeat > 0) {
                details.push(format!("Repeats every {}", krabb_date::format_duration(repeat)));
            }

            let retries = item["retries"].as_u64().unwrap_or_default();

            if retries > 0 {
                let used =
                    item["attempts"].as_u64().unwrap_or_default().saturating_sub(1).min(retries);
                details.push(format!("Automatic retries: {used} of {retries} used"));
            } else {
                details.push("Automatic retries are off".into());
            }

            if item["on_failure"] == "continue" {
                details.push("Continues after failed occurrences".into());
            }

            if let Some(retry_at) = item["retry_at"].as_u64() {
                details.push(format!(
                    "Next retry: {}",
                    krabb_date::format_datetime_in_timezone(
                        retry_at,
                        item["zone"].as_str().unwrap_or("UTC"),
                    )
                ));
            }

            if item["run_requested"] == true {
                details.push("A manual run is queued".into());
            }

            if item["pause_requested"] == true {
                details.push(
                    "Pause requested, and the current run will finish before the schedule pauses"
                        .into(),
                );
            }

            if let Some(outcome) = item["last_outcome"].as_str() {
                details.push(format!(
                    "Last result: {}, details in krabb schedule history {}",
                    schedule_outcome_label(outcome),
                    item["name"].as_str().unwrap_or("Scheduled task")
                ));
            }

            let name = item["name"].as_str().unwrap_or("Scheduled task");
            let title = match item["scheduler"].as_str() {
                Some(scheduler) => format!("{id} | {name} | {status} | {scheduler}"),
                None => format!("{id} | {name} | {status}"),
            };

            Some(format_tree_entry(&title, &details, index + 1 == items.len()))
        })
        .collect::<Vec<_>>();

    format!("Schedules ({from}–{to} of {total}, page {page} of {pages})\n{}", rows.join("\n\n"))
}

pub(crate) fn schedule_outcome_label(outcome: &str) -> &'static str {
    match outcome {
        "completed" => "completed successfully",
        "retrying" => "failed, retry queued",
        "continuing" => "failed, next occurrence remains scheduled",
        "cancelled" => "cancelled",
        "skipped" => "skipped",
        _ => "failed, task paused",
    }
}

pub(crate) fn parse_schedule_history(content: &str) -> Result<Vec<Value>, String> {
    let lines = content.split_terminator('\n').collect::<Vec<_>>();

    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .filter_map(|(index, line)| match parse_schedule_record(line, index + 1) {
            Ok(record) => Some(Ok(record)),
            Err(_) if index + 1 == lines.len() && !content.ends_with('\n') => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

pub(crate) fn parse_schedule_record(line: &str, line_number: usize) -> Result<Value, String> {
    let record: Value = serde_json::from_str(line).map_err(|error| {
        format!("Schedule history line {line_number} is invalid JSON: {error}.")
    })?;

    if !record.is_object() {
        return Err(format!("Schedule history line {line_number} is not a JSON object."));
    }

    Ok(record)
}

pub(crate) fn schedule_history_text(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());

    for character in value.chars() {
        if character.is_control() {
            if character.is_whitespace() {
                safe.push(' ');
            }
        } else if character == '—' {
            safe.push('-');
        } else {
            safe.push(character);
        }
    }

    safe.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn format_schedule_recorded_at(value: &str) -> String {
    value
        .parse::<jiff::Timestamp>()
        .ok()
        .and_then(|timestamp| u64::try_from(timestamp.as_second()).ok())
        .map(krabb_date::format_datetime)
        .unwrap_or_else(|| schedule_history_text(value))
}

pub(crate) fn read_schedule_history_page(
    path: &Path,
    page: usize,
) -> Result<(Vec<Value>, usize), String> {
    let retained_limit = page
        .checked_mul(SCHEDULE_HISTORY_PAGE_SIZE)
        .ok_or_else(|| "Schedule history page is too large.".to_owned())?;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,

        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), 0));
        }

        Err(error) => {
            return Err(format!("Could not open schedule history {}: {error}.", path.display()));
        }
    };

    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut records = std::collections::VecDeque::new();
    let mut line_number = 0;
    let mut total = 0_usize;

    loop {
        line.clear();

        let read = (&mut reader)
            .take((SCHEDULE_HISTORY_LINE_LIMIT + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|error| {
                format!("Could not read schedule history {}: {error}.", path.display())
            })?;

        if read == 0 {
            break;
        }

        line_number += 1;

        if read > SCHEDULE_HISTORY_LINE_LIMIT {
            return Err(format!(
                "Schedule history line {line_number} exceeds the {} KiB limit.",
                SCHEDULE_HISTORY_LINE_LIMIT / 1024
            ));
        }

        let terminated = line.last() == Some(&b'\n');

        if terminated {
            line.pop();
        }

        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }

        let line_text = match std::str::from_utf8(&line) {
            Ok(line_text) => line_text,
            Err(_) if !terminated => break,

            Err(error) => {
                return Err(format!(
                    "Schedule history line {line_number} is not valid UTF-8: {error}."
                ));
            }
        };

        let record = match parse_schedule_record(line_text, line_number) {
            Ok(record) => record,
            Err(_) if !terminated => break,
            Err(error) => return Err(error),
        };

        total += 1;

        if records.len() == retained_limit {
            records.pop_front();
        }

        records.push_back(record);
    }

    let skip = (page - 1).saturating_mul(SCHEDULE_HISTORY_PAGE_SIZE);
    let count = total.saturating_sub(skip).min(SCHEDULE_HISTORY_PAGE_SIZE);
    let start = records.len().saturating_sub(skip.saturating_add(count));
    let page_records = records.into_iter().skip(start).take(count).collect::<Vec<_>>();

    Ok((page_records, total))
}

pub(crate) fn format_schedule_history(
    records: &[Value],
    total: usize,
    path: &Path,
    selector: &str,
    page: usize,
    all: bool,
) -> String {
    let schedule = records
        .last()
        .and_then(|record| record["schedule"]["name"].as_str())
        .filter(|name| !name.is_empty())
        .map(schedule_history_text)
        .unwrap_or_else(|| schedule_history_text(selector));

    let mut output = format!("Schedule history: {schedule}\n");

    if records.is_empty() {
        if all {
            output.push_str("Runs | 0–0 of 0 | all runs (newest first)\n");
        } else {
            output.push_str(&format!("Runs | 0–0 of 0 | page {page}/1 (newest first)\n"));
        }

        output.push_str("No execution history has been recorded.\n");
    } else {
        let start = if all || page == 1 { 1 } else { (page - 1) * SCHEDULE_HISTORY_PAGE_SIZE + 1 };

        let end = if all { total } else { (start + records.len() - 1).min(total) };

        let pages = total.div_ceil(SCHEDULE_HISTORY_PAGE_SIZE).max(1);

        if all {
            output
                .push_str(&format!("Runs | {start}–{end} of {total} | all runs (newest first)\n"));
        } else {
            output.push_str(&format!(
                "Runs | {start}–{end} of {total} | page {page}/{pages} (newest first)\n"
            ));
        }

        output.push('\n');

        let displayed = records.iter().rev().collect::<Vec<_>>();

        for (display_index, record) in displayed.iter().enumerate() {
            let run = &record["run"];
            let run_id =
                run["id"].as_u64().map_or_else(|| "Run".to_owned(), |id| format!("Run {id}"));
            let outcome = run["status"]
                .as_str()
                .or_else(|| record["status"].as_str())
                .map(schedule_outcome_label)
                .unwrap_or("outcome unknown");

            let mut details = Vec::new();

            if let Some(timestamp) = record["timestamp"].as_str() {
                details.push(format!("Recorded: {}", format_schedule_recorded_at(timestamp)));
            }

            let timezone = record["schedule"]["timezone"].as_str().unwrap_or("UTC");

            for (field, label) in [
                ("scheduled_for", "Scheduled for"),
                ("next_run_at", "Next run"),
                ("retry_at", "Retry at"),
            ] {
                if let Some(timestamp) = run[field].as_u64() {
                    let formatted = krabb_date::format_datetime_in_timezone(timestamp, timezone);

                    details.push(format!("{label}: {formatted}"));
                }
            }

            if let Some(attempt) = run["attempt"].as_u64() {
                let retries = run["retry_limit"].as_u64().unwrap_or_default();
                details.push(format!("Attempt: {attempt} (retries configured: {retries})"));
            }

            if run["manual"] == true {
                details.push("Trigger: manual run".into());
            } else if run["retry"] == true {
                details.push("Trigger: manual retry".into());
            }

            if let Some(policy) = run["missed_policy"].as_str() {
                details.push(format!("Missed-run policy: {}", schedule_history_text(policy)));
            }

            if let Some(provider) = record["agent"]["provider"].as_str() {
                let provider = schedule_history_text(provider);
                let model = record["agent"]["model"].as_str().map(schedule_history_text);
                details.push(format!(
                    "Agent: {provider}{}",
                    model.map_or_else(String::new, |model| format!(" ({model})"))
                ));
            }

            if let Some(workspace) = record["agent"]["workspace"].as_str() {
                details.push(format!("Workspace: {}", schedule_history_text(workspace)));
            }

            let result = schedule_history_text(record["result"].as_str().unwrap_or_default());

            if result.is_empty() {
                details.push("Result: (no result text)".into());
            } else {
                details.push(format!("Result: {result}"));
            }

            output.push_str(&format_tree_entry(
                &format!("{run_id} | {outcome}"),
                &details,
                display_index + 1 == displayed.len(),
            ));

            if display_index + 1 < displayed.len() {
                output.push_str("\n\n");
            }
        }
    }

    output.push_str(&format!(
        "\nRaw JSONL: {}\n",
        schedule_history_text(&path.display().to_string())
    ));
    output.push_str("Use `krabb schedule history <ID|NAME> --raw` to print the raw log.\n");
    output
}

/// The empty schedule list, which ends with how to add the first one.
const NO_SCHEDULES: &str =
    "No schedules yet.\nAdd one with `krabb schedule add`, or ask your Krabb to schedule a task.";
