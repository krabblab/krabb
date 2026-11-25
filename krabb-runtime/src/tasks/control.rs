use super::*;
use krabb_core::execution::budget::Grant;

const EXTENSION_MINUTES: u64 = 24 * 60;

/// The control actions a model may take, each offered as its own `task_<action>` tool.
pub(crate) const ACTIONS: [&str; 7] =
    ["list", "pause", "resume", "cancel", "steer", "defer", "extend"];

/// The tool names of the control actions, in the order of `ACTIONS`.
pub(crate) const NAMES: [&str; 7] = [
    "task_list",
    "task_pause",
    "task_resume",
    "task_cancel",
    "task_steer",
    "task_defer",
    "task_extend",
];

/// The control action a tool name stands for.
pub(crate) fn action(name: &str) -> Option<&'static str> {
    NAMES.iter().position(|tool| *tool == name).map(|index| ACTIONS[index])
}

/// Returns a refused task control as text so the caller can clarify instead of failing.
pub(crate) fn refusal(result: Result<ToolResult, PluginError>) -> ToolResult {
    result.unwrap_or_else(|error| {
        ToolResult::text(format!("The task control was not applied: {error}"))
    })
}

/// The sender of the conversation's active request; channel tasks belong to their sender.
pub(crate) fn sender(store: &state::Store, session: &str) -> Option<String> {
    store
        .sessions
        .get(session)
        .and_then(|session| session.inflight.as_ref())
        .and_then(|message| message.sender.clone())
}

/// Applies one task control in this conversation. Only `user` commands may approve or deny.
pub(crate) fn control(
    store: &mut state::Store,
    args: &Value,
    session: &str,
    sender: Option<&str>,
    signals: &Signals,
    user: bool,
) -> Result<ToolResult, PluginError> {
    let source = store.sessions.get(session).and_then(|session| session.source.as_ref()).cloned();
    let owned = |task: &&Task| {
        task.origin.source == source
            && task.origin.session == session
            && (!matches!(task.origin.source(), sharing::Principal::Channel(_))
                || task.origin.sender.is_none()
                || task.origin.sender.as_deref() == sender)
    };

    let action = args["action"].as_str().unwrap_or_default();

    if action == "list" {
        let items: Vec<_> = store
            .tasks
            .values()
            .filter(owned)
            .map(|task| {
                serde_json::json!({
                    "id":task.id, "status":task.status, "goal":clip(task.instruction.clone(), 200),
                    "checkpoint":clip(task.checkpoint.clone(), 500), "steps":task.step,
                    "next":task.next, "revision":task.revision,
                    "resume":task.deferred.as_ref().map(|receipt| &receipt.command.when),
                    "approval":task.approval.as_ref()
                        .filter(|approval| approval.decision.is_none())
                        .map(|approval| &approval.tool),
                    "inference":task.budget.as_ref().map(crate::accounting::view),
                })
            })
            .collect();

        return Ok(ToolResult::text(serde_json::to_string(&items)?));
    }

    if matches!(action, "approve" | "deny") && !user {
        return Err("Only the user can approve or deny a saved action. Ask them to send \
            `/task approve <id>` or `/task deny <id>`."
            .into());
    }

    let id = args["id"].as_str().ok_or("A task ID is required.")?;
    let task = store
        .tasks
        .get(id)
        .filter(owned)
        .ok_or("The task does not belong to this conversation.")?;

    if matches!(action, "approve" | "deny")
        && task.status == TaskStatus::Paused
        && task.approval.as_ref().is_some_and(|approval| approval.decision.is_none())
    {
        return Err(format!(
            "The task is paused, so send `/task resume {id}` first, and it then waits for this \
             decision."
        )
        .into());
    }

    let command = match args.get("command") {
        Some(Value::String(command)) => command.clone(),
        Some(_) => return Err("A command ID must be text.".into()),
        None => format!("control-{}", schedule_id()?),
    };

    let saved = task
        .commands
        .iter()
        .map(|receipt| (receipt.command.revision, &receipt.command.id))
        .chain(task.deferrals.iter().map(|receipt| (receipt.command.revision, &receipt.command.id)))
        .find(|(_, saved)| **saved == command)
        .map(|(revision, _)| revision);

    let revision = match args.get("revision") {
        Some(value) => value.as_u64().ok_or("A task revision must be an unsigned integer.")?,
        None => saved.unwrap_or(task.revision),
    };

    let text = match action {
        "defer" => defer(store, id, command, revision, args)?,
        "extend" => extend(store, id, command, revision, args, signals.ceiling)?,

        "pause" | "resume" | "cancel" | "steer" | "approve" | "deny" => {
            act(store, id, command, revision, action, args)?
        }

        _ => return Err("Use list, pause, resume, cancel, steer, defer, or extend.".into()),
    };

    if store.tasks[id].status.stopping() {
        signals.cancel(id);
    }

    signals.wake.notify_one();
    Ok(ToolResult::text(text))
}

fn act(
    store: &mut state::Store,
    id: &str,
    command: String,
    revision: u64,
    action: &str,
    args: &Value,
) -> Result<String, PluginError> {
    let previous = store.tasks[id].status;
    let command = krabb_core::task::Command {
        id: command,
        action: serde_json::from_value(serde_json::json!(action))?,
        revision,
        instruction: args
            .get("instruction")
            .map(|value| value.as_str().ok_or("A task instruction must be text."))
            .transpose()?
            .unwrap_or_default()
            .into(),
    };

    let receipt = store.control_task(id, &command)?;
    let current = store.tasks[id].status;
    let waiting =
        store.tasks[id].approval.as_ref().is_some_and(|approval| approval.decision.is_none());

    let text = match current {
        TaskStatus::Pausing => "Pause requested. The active step is stopping safely.",
        TaskStatus::Cancelling => "Cancellation requested. The active step is stopping safely.",
        TaskStatus::Paused => "Task paused. Its goal and checkpoint are saved.",
        TaskStatus::Cancelled => "Task cancelled. Already completed effects are preserved.",

        TaskStatus::Reporting | TaskStatus::Completed => {
            "The task has already completed, and its result is preserved."
        }

        TaskStatus::Queued if action == "approve" => {
            "Approved. The task will run the saved action once and continue in this conversation."
        }

        TaskStatus::Queued if action == "deny" => {
            "Denied, so the saved action will not run, and the task continues without it."
        }

        TaskStatus::Queued | TaskStatus::Waiting | TaskStatus::Running
            if action == "resume"
                || matches!(previous, TaskStatus::Paused | TaskStatus::Blocked) =>
        {
            "Task resumed from its saved checkpoint."
        }

        _ => "Task instruction updated, and its current state is preserved.",
    };

    let text = if current == TaskStatus::Blocked && waiting {
        format!(
            "This task is waiting for your approval. Send `/task approve {id}` or \
             `/task deny {id}`."
        )
    } else {
        text.into()
    };

    let text = match &store.tasks[id].retry {
        Some(retry) if action == "resume" && retry.at > unix_seconds() => format!(
            "{text} The saved retry wait remains until {}.",
            krabb_date::format_datetime(retry.at)
        ),

        _ => text,
    };

    Ok(format!(
        "{text}\n{}",
        serde_json::to_string(&serde_json::json!({"receipt":receipt,"current":current}))?
    ))
}

fn defer(
    store: &mut state::Store,
    id: &str,
    command: String,
    revision: u64,
    args: &Value,
) -> Result<String, PluginError> {
    let saved = store.tasks[id].deferrals.iter().find(|receipt| receipt.command.id == command);
    let when = match saved {
        Some(receipt) => receipt.command.when.clone(),

        None => {
            let when = args["when"].as_str().ok_or("A resume time is required.")?;
            super::defer::parse(when)?
        }
    };

    let command = krabb_core::task::defer::Command { id: command, revision, when };

    store.defer_task(id, &command, unix_seconds())?;

    let task = &store.tasks[id];
    let Some(deferred) = &task.deferred else {
        return Ok("That timed resume was already recorded.".into());
    };

    Ok(format!(
        "Resume is saved for {}. {}",
        super::defer::describe(&deferred.command.when),
        if task.status == TaskStatus::Pausing {
            "The current step is still stopping, and work resumes only after it settles."
        } else {
            "The task is paused until then."
        }
    ))
}

/// A grant identity derived from a command ID, which may contain any characters.
fn grant_id(command: String) -> String {
    let valid = !command.is_empty()
        && command.len() <= krabb_core::task::COMMAND_LIMIT
        && command.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');

    if valid { command } else { format!("extend-{}", &crate::digest::sha256(command)[..32]) }
}

fn extend(
    store: &mut state::Store,
    id: &str,
    command: String,
    revision: u64,
    args: &Value,
    ceiling: Option<u64>,
) -> Result<String, PluginError> {
    let task = &store.tasks[id];
    let command = grant_id(command);

    let grant = match task.grants.iter().find(|receipt| receipt.grant.id == command) {
        Some(receipt) => receipt.grant.clone(),

        None => {
            let minutes = args["minutes"]
                .as_u64()
                .filter(|minutes| (1..=EXTENSION_MINUTES).contains(minutes))
                .ok_or("Extend by 1 to 1440 minutes.")?;
            let budget = task.budget.as_ref().ok_or("This task has no execution account.")?;
            let limit = if budget.legacy {
                budget.spent_millis.saturating_add(budget.held_millis)
            } else {
                budget.limit_millis.ok_or("This task has no inference limit to extend.")?
            };

            Grant {
                id: command,
                revision,
                sequence: budget.sequence,
                limit_millis: limit.saturating_add(minutes.saturating_mul(60_000)),
            }
        }
    };

    let receipt = store.grant_budget(id, &grant, ceiling)?;

    Ok(format!(
        "The task's total inference allowance is now {}. Resume it to continue if it is paused \
         or blocked.",
        krabb_date::format_duration(receipt.grant.limit_millis / 1_000)
    ))
}

/// One tool per control action, each with only the fields that action reads.
pub(crate) fn tools() -> Vec<ToolSpec> {
    let list = ToolSpec {
        name: NAMES[0].into(),
        description: Some(
            "List the tasks in this conversation with their status, checkpoint, and revision. \
             Completion and important updates arrive automatically, so polling is unnecessary."
                .into(),
        ),
        schema: serde_json::json!({"type":"object", "properties":{}, "additionalProperties":false}),
    };

    let text = serde_json::json!({"type":"string"});

    vec![
        list,
        spec(
            "pause",
            "Pause a task when the user asks, in any language. A running task stops only after \
             its active step settles.",
            None,
        ),
        spec("resume", "Resume a paused or blocked task when the user asks.", None),
        spec(
            "cancel",
            "Cancel a task when the user asks. Already completed effects are preserved.",
            None,
        ),
        spec(
            "steer",
            "Replace a task's instruction when the user asks to change its goal.",
            Some(("instruction", text.clone())),
        ),
        spec(
            "defer",
            "Pause a task until an exact time when the user asks. Give the time with a UTC offset \
             or IANA timezone, such as 2030-11-12T09:00 Europe/Rome. Resolve relative times \
             yourself and ask when the timezone is unknown.",
            Some(("when", text)),
        ),
        spec(
            "extend",
            "Add minutes to a task's inference allowance when the user asks.",
            Some(("minutes", serde_json::json!({"type":"integer","minimum":1}))),
        ),
    ]
}

fn spec(action: &str, description: &str, field: Option<(&str, Value)>) -> ToolSpec {
    let mut properties = serde_json::json!({
        "id":{"type":"string"}, "command":{"type":"string"},
        "revision":{"type":"integer","minimum":0}
    });

    let mut required = vec!["id"];

    if let Some((key, value)) = field {
        properties[key] = value;
        required.push(key);
    }

    ToolSpec {
        name: format!("task_{action}"),
        description: Some(format!(
            "{description} Include command and revision from a previous receipt for safe retries."
        )),
        schema: serde_json::json!({
            "type":"object", "properties":properties, "required":required,
            "additionalProperties":false
        }),
    }
}

#[cfg(test)]
mod tests {
    use krabb_core::execution::budget::Grant;

    #[test]
    fn any_command_identity_yields_a_stable_valid_grant_identity() {
        assert_eq!(super::grant_id("extend-1".into()), "extend-1");

        let derived = super::grant_id("extend_1".into());

        assert_eq!(derived, super::grant_id("extend_1".into()));
        assert_ne!(derived, super::grant_id("extend.1".into()));

        let grant = Grant { id: derived, revision: 1, sequence: 1, limit_millis: 1 };

        grant.validate().unwrap();
    }
}
