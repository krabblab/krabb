mod checkpoint;
mod control;
mod defer;
pub(super) mod dialog;
mod foreground;
pub(super) mod input;
mod policy;
#[cfg(all(test, unix))]
mod tests;
mod worker;

pub(super) use checkpoint::{apply, tool as checkpoint_tool};

pub(super) use control::{
    LEGACY_NAME as LEGACY_CONTROL, NAME as CONTROL, canonical, control, refusal, sender,
    tool as control_tool,
};

pub(super) use foreground::{Answer, Failure, Foreground};
pub(super) use worker::{Context, run};

use super::*;
use state::{Task, TaskStatus};

#[derive(Default)]
pub(super) struct Signals {
    pub wake: Notify,
    pub attached: Mutex<BTreeSet<String>>,
    pub cancels: Mutex<BTreeMap<String, Arc<Cancellation>>>,
    pub ceiling: Option<u64>,
}

impl Signals {
    pub fn cancel(&self, id: &str) {
        if let Ok(cancels) = self.cancels.lock()
            && let Some(cancel) = cancels.get(id)
        {
            cancel.request();
        }

        self.wake.notify_one();
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn capture(
    store: &state::Store,
    provider: &str,
    model: &str,
    workspace: &Path,
    integration: &str,
    chat: &str,
    thread: Option<&str>,
    session: &str,
    tools: bool,
    shell: bool,
) -> Result<Value, PluginError> {
    let current = store.sessions.get(session).ok_or("The originating session was not found.")?;
    let source =
        current.source.clone().ok_or("The session has no registered delivery endpoint.")?;
    let (id, local) = match &source {
        sharing::Principal::Client(id) => (id, true),
        sharing::Principal::Channel(id) => (id, false),

        sharing::Principal::Agent(_) => {
            return Err("An agent is not a user delivery endpoint.".into());
        }
    };

    if id != integration {
        return Err("The session endpoint changed during this turn.".into());
    }

    Ok(serde_json::json!({
        "provider":provider, "model":model, "workspace":workspace.canonicalize()?,
        "tools":tools, "shell":shell,
        "origin": {
            "source":source, "channel":id, "chat":if local { session } else { chat },
            "private":!local && current.private,
            "thread":if local { None } else { thread }, "session":session,
            "sender":current.inflight.as_ref().and_then(|message| message.sender.clone()),
            "roles":current.inflight_roles,
        }
    }))
}

pub(super) fn goal(text: String) -> String {
    if text.len() > TOOL_ARGS_LIMIT {
        format!(
            "Original request preview; consult its full text in the saved context:\n{}",
            clip(text, TOOL_ARGS_LIMIT - 128)
        )
    } else {
        text
    }
}

pub(super) fn enqueue(
    store: &mut state::Store,
    route: &routing::Route,
    context: &Value,
) -> Result<String, PluginError> {
    let task = record(route, context)?;
    let id = task.id.clone();

    store.enqueue_task(task)?;
    Ok(id)
}

pub(super) fn record(route: &routing::Route, context: &Value) -> Result<Task, PluginError> {
    let origin: origin::Origin = serde_json::from_value(context["origin"].clone())?;

    Ok(Task {
        id: format!("task-{}", schedule_id()?),
        instruction: route.instruction.clone(),
        checkpoint: String::new(),
        provider: context["provider"].as_str().ok_or("Task provider is missing.")?.into(),
        model: context["model"].as_str().ok_or("Task model is missing.")?.into(),
        workspace: serde_json::from_value(context["workspace"].clone())?,
        origin,
        foreground: false,
        operation: None,
        approval: None,
        budget: Some(krabb_core::execution::budget::Budget::default()),
        grants: Vec::new(),
        deferred: None,
        deferrals: Vec::new(),
        request: None,
        input: None,
        result: None,
        tools: context["tools"] == true,
        shell: context["shell"] == true,
        status: TaskStatus::Queued,
        unsafe_step: false,
        uncertain: false,
        commands: Vec::new(),
        restart: false,
        step: 0,
        revision: 0,
        failures: 0,
        retry: None,
        next: unix_seconds(),
        created: unix_seconds(),
        updated: unix_seconds(),
        notice: None,
        last_notice: String::new(),
        last_notified: 0,
    })
}

pub(super) fn checkpoint_prompt(store: &state::Store, id: &str) -> Result<String, PluginError> {
    let task = store.tasks.get(id).ok_or("The background goal was not found.")?;

    Ok(format!(
        "Your last reply is an internal draft and has not been delivered. The goal \
            remains:\n{}\n\nThe durable checkpoint is:\n{}\n\nIf the whole goal is complete, call \
            checkpoint with state complete and a summary; omit message to deliver your last draft, \
            or provide the complete result in message. Otherwise checkpoint completed work and the \
            exact next action, then continue automatically. Do not ask the user to ping you or \
            continue the work manually.",
        task.instruction, task.checkpoint,
    ))
}
