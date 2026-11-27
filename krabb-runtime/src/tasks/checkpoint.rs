use super::*;

const WAIT_SECONDS: std::ops::RangeInclusive<u64> = 5..=86_400;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    state: String,
    summary: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    wait_seconds: Option<u64>,
    #[serde(default)]
    update: Option<Update>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    importance: String,
    text: String,
}

pub(crate) fn apply(
    store: &mut state::Store,
    session: &str,
    args: Value,
) -> Result<(), PluginError> {
    let checkpoint: Checkpoint = serde_json::from_value(args)?;

    if checkpoint.summary.trim().is_empty()
        || checkpoint.summary.len() > state::TASK_CHECKPOINT_LIMIT
    {
        return Err("A checkpoint summary of at most 32 KiB is required.".into());
    }

    if let Some(update) = &checkpoint.update
        && (!matches!(update.importance.as_str(), "milestone" | "decision")
            || update.text.trim().is_empty()
            || update.text.len() > routing::MESSAGE_LIMIT)
    {
        return Err("An update must describe a meaningful milestone or decision.".into());
    }

    let draft = if checkpoint.state == "complete" && checkpoint.message.is_none() {
        last_draft(store, session)
    } else {
        None
    };

    let (status, delay, message) = match checkpoint.state.as_str() {
        "continue" => {
            (TaskStatus::Queued, 1, checkpoint.update.as_ref().map(|update| update.text.as_str()))
        }

        "wait" => {
            let delay = checkpoint
                .wait_seconds
                .filter(|delay| WAIT_SECONDS.contains(delay))
                .ok_or("A waiting checkpoint needs a delay between five seconds and one day.")?;

            (
                TaskStatus::Waiting,
                delay,
                checkpoint.update.as_ref().map(|update| update.text.as_str()),
            )
        }

        "complete" => (
            TaskStatus::Reporting,
            0,
            Some(
                checkpoint
                    .message
                    .as_deref()
                    .filter(|text| !text.trim().is_empty())
                    .or(draft.as_deref())
                    .ok_or("A complete result is required.")?,
            ),
        ),

        "blocked" => (
            TaskStatus::Blocked,
            0,
            Some(
                checkpoint
                    .message
                    .as_deref()
                    .filter(|text| !text.trim().is_empty())
                    .ok_or("Explain what blocks this task.")?,
            ),
        ),

        _ => return Err("Invalid checkpoint state.".into()),
    };

    if checkpoint.state != "wait" && checkpoint.wait_seconds.is_some() {
        return Err("Only waiting checkpoints accept a delay.".into());
    }

    store.checkpoint_task_from_model(
        session,
        &checkpoint.summary,
        status,
        unix_seconds().saturating_add(delay),
        message,
    )?;
    Ok(())
}

pub(crate) const NAME: &str = "task_checkpoint";

pub(crate) fn tool() -> ToolSpec {
    ToolSpec {
        name: NAME.into(),
        description: Some(
            "Persist completed work and the exact next action, then yield this \
            execution step. The host resumes automatically without an overall lifetime deadline. \
            Use continue for more work, wait for an external outcome, complete only after the \
            entire goal is achieved, or blocked when user input or permission is needed. For \
            complete, supply the full result in message or omit message to publish your most \
            recent draft from this step. Never repeat completed mutations. Provide update only for \
            a new important milestone or decision in the user's language, never routine waiting \
            messages. Completion and blockers are always delivered. Checkpoint before step, token, \
            and tool limits and after meaningful actions. This call ends the step."
                .into(),
        ),
        schema: serde_json::json!({
            "type":"object", "properties": {
                "state":{"type":"string","enum":["continue","wait","complete","blocked"]},
                "summary":{"type":"string"}, "message":{"type":"string"},
                "wait_seconds":{"type":"integer","minimum":WAIT_SECONDS.start(),
                    "maximum":WAIT_SECONDS.end()},
                "update":{"type":"object","properties": {
                    "importance":{"type":"string","enum":["milestone","decision"]},
                    "text":{"type":"string"}
                },"required":["importance","text"],"additionalProperties":false}
            }, "required":["state","summary"], "additionalProperties":false
        }),
    }
}

fn last_draft(store: &state::Store, id: &str) -> Option<String> {
    let task = store.tasks.get(id)?;
    let messages = &store.sessions.get(id)?.messages;
    let start =
        messages.iter().rposition(|message| message.id == format!("{id}-step-{}", task.step))?;

    messages[start..].iter().rev().find_map(|message| {
        if message.role != Role::Assistant
            || message.content.iter().any(|content| matches!(content, Content::ToolCall { .. }))
        {
            return None;
        }

        let text = super::input::text(message);

        (!text.trim().is_empty()).then_some(text)
    })
}
