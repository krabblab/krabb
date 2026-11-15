use super::*;
use krabb_core::execution::budget::Grant;

const EXTENSION_MINUTES: u64 = 24 * 60;

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
    let owned = |job: &&Job| {
        job.origin.source == source
            && job.origin.session == session
            && (!matches!(job.origin.source(), sharing::Principal::Channel(_))
                || job.origin.sender.is_none()
                || job.origin.sender.as_deref() == sender)
    };

    let action = args["action"].as_str().unwrap_or_default();

    if action == "list" {
        let items: Vec<_> = store
            .jobs
            .values()
            .filter(owned)
            .map(|job| {
                serde_json::json!({
                    "id":job.id, "status":job.status, "goal":clip(job.instruction.clone(), 200),
                    "checkpoint":clip(job.checkpoint.clone(), 500), "steps":job.step,
                    "next":job.next, "revision":job.revision,
                    "resume":job.deferred.as_ref().map(|receipt| &receipt.command.when),
                    "approval":job.approval.as_ref()
                        .filter(|approval| approval.decision.is_none())
                        .map(|approval| &approval.tool),
                    "inference":job.budget.as_ref().map(crate::accounting::view),
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
    let job =
        store.jobs.get(id).filter(owned).ok_or("The task does not belong to this conversation.")?;

    if matches!(action, "approve" | "deny")
        && job.status == JobStatus::Paused
        && job.approval.as_ref().is_some_and(|approval| approval.decision.is_none())
    {
        return Err(format!(
            "The task is paused. Send `/task resume {id}` first; it then waits for this decision."
        )
        .into());
    }

    let command = match args.get("command") {
        Some(Value::String(command)) => command.clone(),
        Some(_) => return Err("A command ID must be text.".into()),
        None => format!("control-{}", schedule_id()?),
    };

    let saved = job
        .commands
        .iter()
        .map(|receipt| (receipt.command.revision, &receipt.command.id))
        .chain(job.deferrals.iter().map(|receipt| (receipt.command.revision, &receipt.command.id)))
        .find(|(_, saved)| **saved == command)
        .map(|(revision, _)| revision);

    let revision = match args.get("revision") {
        Some(value) => value.as_u64().ok_or("A task revision must be an unsigned integer.")?,
        None => saved.unwrap_or(job.revision),
    };

    let text = match action {
        "defer" => defer(store, id, command, revision, args)?,
        "extend" => extend(store, id, command, revision, args, signals.ceiling)?,

        "pause" | "resume" | "cancel" | "steer" | "approve" | "deny" => {
            act(store, id, command, revision, action, args)?
        }

        _ => return Err("Use list, pause, resume, cancel, steer, defer, or extend.".into()),
    };

    if store.jobs[id].status.stopping() {
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
    let previous = store.jobs[id].status;
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

    let receipt = store.control_job(id, &command)?;
    let current = store.jobs[id].status;
    let waiting =
        store.jobs[id].approval.as_ref().is_some_and(|approval| approval.decision.is_none());

    let text = match current {
        JobStatus::Pausing => "Pause requested. The active step is stopping safely.",
        JobStatus::Cancelling => "Cancellation requested. The active step is stopping safely.",
        JobStatus::Paused => "Task paused. Its goal and checkpoint are saved.",
        JobStatus::Cancelled => "Task cancelled. Already completed effects are preserved.",

        JobStatus::Reporting | JobStatus::Completed => {
            "The task has already completed; its result is preserved."
        }

        JobStatus::Queued if action == "approve" => {
            "Approved. The task will run the saved action once and continue in this conversation."
        }

        JobStatus::Queued if action == "deny" => {
            "Denied. The saved action will not run; the task continues without it."
        }

        JobStatus::Queued | JobStatus::Waiting | JobStatus::Running
            if action == "resume" || matches!(previous, JobStatus::Paused | JobStatus::Blocked) =>
        {
            "Task resumed from its saved checkpoint."
        }

        _ => "Task instruction updated; its current state is preserved.",
    };

    let text = if current == JobStatus::Blocked && waiting {
        format!(
            "This task is waiting for your approval. Send `/task approve {id}` or \
             `/task deny {id}`."
        )
    } else {
        text.into()
    };

    let text = match &store.jobs[id].retry {
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
    let saved = store.jobs[id].deferrals.iter().find(|receipt| receipt.command.id == command);
    let when = match saved {
        Some(receipt) => receipt.command.when.clone(),

        None => {
            let when = args["when"].as_str().ok_or("A resume time is required.")?;
            super::defer::parse(when)?
        }
    };

    let command = krabb_core::task::defer::Command { id: command, revision, when };

    store.defer_job(id, &command, unix_seconds())?;

    let job = &store.jobs[id];
    let Some(deferred) = &job.deferred else {
        return Ok("That timed resume was already recorded.".into());
    };

    Ok(format!(
        "Resume is saved for {}. {}",
        super::defer::describe(&deferred.command.when),
        if job.status == JobStatus::Pausing {
            "The current step is still stopping; work resumes only after it settles."
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
    let job = &store.jobs[id];
    let command = grant_id(command);

    let grant = match job.grants.iter().find(|receipt| receipt.grant.id == command) {
        Some(receipt) => receipt.grant.clone(),

        None => {
            let minutes = args["minutes"]
                .as_u64()
                .filter(|minutes| (1..=EXTENSION_MINUTES).contains(minutes))
                .ok_or("Extend by 1 to 1440 minutes.")?;
            let budget = job.budget.as_ref().ok_or("This task has no execution account.")?;
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

pub(crate) fn tool() -> ToolSpec {
    ToolSpec {
        name: "job".into(),
        description: Some(
            "Inspect tasks in this conversation or pause, resume, cancel, steer, defer, or \
             extend one when the user asks, in any language. A running task stops only after its \
             active step settles. Defer takes an exact time with a UTC offset or IANA timezone, \
             such as 2030-11-12T09:00 Europe/Rome; resolve relative times yourself and ask when \
             the timezone is unknown. Extend adds minutes to the task's inference allowance. \
             Include command and revision from a previous receipt for safe retries. Completion \
             and important updates arrive automatically; polling is unnecessary."
                .into(),
        ),
        schema: serde_json::json!({
            "type":"object", "properties": {
                "action":{"type":"string",
                    "enum":["list","pause","resume","cancel","steer","defer","extend"]},
                "id":{"type":"string"}, "instruction":{"type":"string"},
                "when":{"type":"string"}, "minutes":{"type":"integer","minimum":1},
                "command":{"type":"string"}, "revision":{"type":"integer","minimum":0}
            }, "required":["action"], "additionalProperties":false
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
