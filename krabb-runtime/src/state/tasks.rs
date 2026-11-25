mod approval;
pub(crate) use approval::Approval;
mod budget;
pub(crate) use budget::TokensExhausted;
mod controls;
mod defer;
mod operation;
pub(crate) use operation::{Operation, Phase as OperationPhase};
mod foreground;
mod input;
pub(crate) use input::Input;
pub(super) mod retired;
mod retry;
mod transcript;
pub(crate) use retry::Retry;

use super::*;
use crate::origin::{Destination, Origin};

pub(crate) const LIMIT: usize = 64;
pub(crate) const CHECKPOINT_LIMIT: usize = 32 * 1024;
const UPDATE_INTERVAL: u64 = 60;
const NOT_FOUND: &str = "Task not found.";

pub(crate) use krabb_core::task::State as TaskStatus;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Notice {
    id: String,
    text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Task {
    pub id: String,
    pub instruction: String,
    pub checkpoint: String,
    pub provider: String,
    pub model: String,
    pub workspace: PathBuf,
    pub origin: Origin,
    #[serde(default)]
    pub foreground: bool,
    #[serde(default)]
    pub request: Option<String>,
    #[serde(default)]
    pub input: Option<Input>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub operation: Option<Operation>,
    #[serde(default)]
    pub approval: Option<Approval>,
    #[serde(default)]
    pub budget: Option<krabb_core::execution::budget::Budget>,
    #[serde(default)]
    pub grants: Vec<krabb_core::execution::budget::GrantReceipt>,
    #[serde(default)]
    pub deferred: Option<krabb_core::task::defer::Receipt>,
    #[serde(default)]
    pub deferrals: Vec<krabb_core::task::defer::Receipt>,
    pub tools: bool,
    pub shell: bool,
    pub status: TaskStatus,
    pub unsafe_step: bool,
    #[serde(default)]
    pub uncertain: bool,
    #[serde(default)]
    pub commands: Vec<krabb_core::task::Receipt>,
    #[serde(default)]
    pub restart: bool,
    pub step: u64,
    pub revision: u64,
    pub failures: u32,
    #[serde(default)]
    pub retry: Option<Retry>,
    pub next: u64,
    pub created: u64,
    pub updated: u64,
    pub notice: Option<Notice>,
    pub last_notice: String,
    pub last_notified: u64,
}

impl Task {
    /// A working copy of the task's execution account.
    pub(super) fn account(&self) -> std::io::Result<krabb_core::execution::budget::Budget> {
        self.budget.clone().ok_or_else(|| std::io::Error::other("Task accounting is missing."))
    }

    pub fn protected(&self) -> bool {
        !self.status.terminal()
            || self.notice.is_some()
            || self.uncertain
            || self.observing()
            || self.budget.as_ref().is_some_and(|budget| budget.pending.is_some())
    }

    pub fn destination(&self) -> Result<Destination, crate::PluginError> {
        self.origin.destination()
    }
}

impl Store {
    pub(crate) fn task(&self, id: &str) -> std::io::Result<&Task> {
        self.tasks.get(id).ok_or_else(|| std::io::Error::other(NOT_FOUND))
    }

    pub(crate) fn session(&self, id: &str) -> std::io::Result<&Session> {
        self.sessions.get(id).ok_or_else(|| std::io::Error::other("Conversation not found."))
    }

    pub(super) fn validate_tasks(&self) -> std::io::Result<()> {
        if self.tasks.len() > LIMIT {
            return Err(std::io::Error::other("Background task capacity exceeded."));
        }

        for (id, task) in &self.tasks {
            if let Some(approval) = &task.approval {
                approval.validate()?;

                if task.status.terminal() || approval.generation > task.step {
                    return Err(std::io::Error::other("Invalid pending approval."));
                }
            }

            if task.request.is_some() != task.input.is_some()
                || task.input.as_ref().is_some_and(|input| !input.valid())
                || task.request.as_ref().is_some_and(|request| request.is_empty())
            {
                return Err(std::io::Error::other("Task input identity is missing or invalid."));
            }

            task.budget
                .as_ref()
                .ok_or_else(|| std::io::Error::other("Task execution accounting is missing."))?
                .validate()
                .map_err(std::io::Error::other)?;

            if task.deferrals.len() > 32
                || task.deferrals.iter().any(|receipt| {
                    receipt.command.validate().is_err()
                        || task
                            .commands
                            .iter()
                            .any(|control| control.command.id == receipt.command.id)
                        || !matches!(receipt.state, TaskStatus::Paused | TaskStatus::Pausing)
                        || defer::validate(&receipt.command.when).is_err()
                        || receipt.command.revision >= receipt.revision
                        || receipt.revision > task.revision
                })
                || task
                    .deferrals
                    .iter()
                    .map(|receipt| &receipt.command.id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != task.deferrals.len()
                || task.deferred.as_ref().is_some_and(|receipt| {
                    !task.deferrals.contains(receipt)
                        || task.uncertain
                        || !matches!(task.status, TaskStatus::Paused | TaskStatus::Pausing)
                        || krabb_date::validate_timezone(&receipt.command.when.timezone).is_err()
                })
            {
                return Err(std::io::Error::other("Invalid deferred task control."));
            }

            if let Some(operation) = &task.operation {
                operation.validate()?;
            }

            if let Some(retry) = &task.retry {
                retry.failure.validate().map_err(std::io::Error::other)?;

                if !retry.failure.kind.retryable()
                    && retry.failure.kind != krabb_core::failure::Kind::Unknown
                {
                    return Err(std::io::Error::other("Invalid saved retry outcome."));
                }
            }

            if task.failures > 0 && task.retry.is_none() {
                return Err(std::io::Error::other(
                    "The saved retry reason and deadline are missing.",
                ));
            }

            if id != &task.id
                || !is_task_session(id)
                || !valid(id)
                || !valid(&task.provider)
                || !valid(&task.origin.session)
                || task.instruction.is_empty()
                || task.instruction.len() > crate::TOOL_ARGS_LIMIT
                || task.revision == u64::MAX
                || task.step == u64::MAX
                || task.grants.len() > 32
                || task.grants.iter().any(|receipt| {
                    receipt.grant.validate().is_err()
                        || receipt.grant.revision >= receipt.revision
                        || receipt.revision > task.revision
                })
                || task
                    .grants
                    .iter()
                    .map(|receipt| &receipt.grant.id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != task.grants.len()
                || task.commands.len() > 32
                || task.commands.iter().any(|receipt| {
                    receipt.command.validate().is_err()
                        || receipt.command.revision >= receipt.revision
                        || receipt.revision > task.revision
                })
                || task
                    .commands
                    .iter()
                    .map(|receipt| &receipt.command.id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != task.commands.len()
                || task.checkpoint.len() > CHECKPOINT_LIMIT
                || task.model.is_empty()
                || task.model.len() > crate::META_LIMIT
                || !task.workspace.is_absolute()
                || task.request.as_ref().is_some_and(|id| id.len() > crate::META_LIMIT)
                || task.result.as_ref().is_some_and(|text| text.len() > crate::TEXT_LIMIT)
                || task.last_notice.len() > crate::TEXT_LIMIT
                || task.notice.as_ref().is_some_and(|notice| {
                    !valid(&notice.id) || notice.text.len() > crate::TEXT_LIMIT
                })
                || task.origin.source.is_none()
                || task.destination().is_err()
            {
                return Err(std::io::Error::other("Invalid background task record."));
            }
        }

        Ok(())
    }

    pub(super) fn recover_tasks(&mut self) -> std::io::Result<bool> {
        let mut changed = self.restore_inputs()?;

        for task in self.tasks.values_mut() {
            if let Some(budget) = &mut task.budget {
                changed |= budget.recover().map_err(std::io::Error::other)?;
            }

            if let Some(operation) = &mut task.operation {
                if operation.phase == OperationPhase::Dispatching {
                    operation.phase = OperationPhase::Uncertain;
                    task.uncertain = true;
                    task.deferred = None;
                    task.unsafe_step = false;

                    if !task.status.terminal() {
                        task.status = TaskStatus::Blocked;
                    }

                    task.revision = task.revision.saturating_add(1);
                    task.notice = Some(Notice {
                        id: format!("{}-event-{}", task.id, task.revision),
                        text: "The operation's start receipt was lost. Review its effects \
                               before another start."
                            .into(),
                    });

                    changed = true;
                    continue;
                }

                if operation.active() {
                    if task.status == TaskStatus::Running {
                        task.status = TaskStatus::Waiting;
                    }

                    task.unsafe_step = false;
                    task.next = task.next.min(now().saturating_add(300));
                    changed = true;
                    continue;
                }
            }

            if task.status == TaskStatus::Paused
                && task.commands.is_empty()
                && task.deferred.is_none()
            {
                task.status = TaskStatus::Blocked;
                changed = true;
            }

            if task.status != TaskStatus::Running && !task.status.stopping() {
                continue;
            }

            let stopping = task.status.stopping();
            task.uncertain |= task.unsafe_step;
            task.status = if task.status == TaskStatus::Cancelling {
                TaskStatus::Cancelled
            } else if task.uncertain {
                TaskStatus::Blocked
            } else if !stopping
                && task.approval.as_ref().is_some_and(|approval| approval.decision.is_none())
            {
                Store::approval_notice(task);
                TaskStatus::Blocked
            } else if stopping && !task.restart {
                TaskStatus::Paused
            } else {
                task.next = now();
                TaskStatus::Queued
            };

            if task.uncertain || task.status.terminal() {
                task.deferred = None;
            }

            task.restart = false;
            task.revision = task.revision.saturating_add(1);

            if task.uncertain {
                task.notice = Some(Notice {
                    id: format!("{}-event-{}", task.id, task.revision),
                    text:
                        "A task was interrupted during an action. Its effects need review before \
                        related work resumes."
                            .into(),
                });
            }

            changed = true;
        }

        changed |= self.fence_inputs();
        Ok(changed)
    }

    pub fn enqueue_task(&mut self, task: Task) -> std::io::Result<()> {
        self.insert_task(task, true)
    }

    pub(super) fn insert_task(&mut self, task: Task, handoff: bool) -> std::io::Result<()> {
        if self.tasks.contains_key(&task.id) {
            return Err(std::io::Error::other("The background task already exists."));
        }

        let retire = if self.tasks.len() >= LIMIT {
            Some(
                self.tasks
                    .values()
                    .filter(|task| !task.protected())
                    .min_by_key(|task| task.updated)
                    .ok_or_else(|| std::io::Error::other("Background task capacity is full."))?
                    .id
                    .clone(),
            )
        } else {
            None
        };

        self.change(|store| {
            if let Some(id) = retire {
                store.retire_task(&id);
            }

            if handoff {
                host_receipt(
                    store,
                    &task.origin.session,
                    "task",
                    &format!("{}-handoff", task.id),
                    "The background goal is saved. Its result will be delivered automatically.",
                );
            }

            store.tasks.insert(task.id.clone(), task);
        })
    }

    pub fn claim_task(&mut self, id: &str, at: u64) -> std::io::Result<Task> {
        let task = self.task(id)?;

        if !task.observing() && (self.conflicting_input(task) || self.missing_input(task)) {
            let error = if self.conflicting_input(task) {
                krabb_core::task::ControlError::Identity
            } else {
                krabb_core::task::ControlError::Input
            };

            self.change(|store| {
                store.fence_inputs();
            })?;
            return Err(std::io::Error::other(error));
        }

        if !(matches!(task.status, TaskStatus::Queued | TaskStatus::Waiting)
            || task.observing()
                && (task.status.stopping()
                    || matches!(
                        task.status,
                        TaskStatus::Paused | TaskStatus::Cancelled | TaskStatus::Blocked
                    )))
            || task.ready_at() > at
            || task.notice.is_some() && !task.observing()
        {
            return Err(std::io::Error::other("The background task is not ready."));
        }

        if task.foreground
            && !task.observing()
            && self
                .sessions
                .get(&task.origin.session)
                .is_some_and(|session| session.status == "working" || session.inflight.is_some())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "The originating conversation is still active.",
            ));
        }

        if task.foreground
            && !task.observing()
            && self.tasks.values().any(|other| {
                other.id != task.id
                    && other.foreground
                    && !other.observing()
                    && other.origin.session == task.origin.session
                    && (other.status == TaskStatus::Running || other.status.stopping())
            })
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "The previous foreground request is still active.",
            ));
        }

        let mut budget = task.account()?;
        budget.recover().map_err(std::io::Error::other)?;

        self.change(|store| {
            let task = store.tasks.get_mut(id).unwrap();

            if !task.observing() {
                task.status = TaskStatus::Running;
                task.revision = task.revision.saturating_add(1);
            }

            task.unsafe_step = false;
            task.budget = Some(budget);
            task.step = task.step.saturating_add(1);
            task.updated = at;

            if let Some(session) = store.sessions.get_mut(id) {
                session.inflight = None;
                session.queued.clear();
                session.queue_roles.clear();
                session.status = "idle".into();
            }
        })?;

        Ok(self.tasks[id].clone())
    }

    pub fn recover_task(&mut self, id: &str) -> std::io::Result<()> {
        self.recover_budget(id)?;
        let task = self.task(id)?;

        if let Some(operation) = &task.operation {
            if operation.phase == OperationPhase::Dispatching {
                let key = operation.start.id.clone();
                return self.abandon_operation(id, &key);
            }

            if operation.active() {
                if task.status == TaskStatus::Running {
                    return self.change(|store| {
                        store.tasks.get_mut(id).unwrap().status = TaskStatus::Waiting;
                    });
                }

                return Ok(());
            }
        }

        if task.status.stopping() {
            return self.finish_task_control(id, task.step);
        }

        if task.status != TaskStatus::Running {
            return Ok(());
        }

        if task.unsafe_step {
            let checkpoint = task.checkpoint.clone();
            self.uncertain_task(id)?;
            return self.checkpoint_task(
                id,
                &checkpoint,
                TaskStatus::Blocked,
                now(),
                Some(
                    "The background task was interrupted during an action and is paused to avoid \
                    repeating its effects.",
                ),
            );
        }

        if task.approval.as_ref().is_some_and(|approval| approval.decision.is_none()) {
            return self.change(|store| {
                let task = store.tasks.get_mut(id).unwrap();
                task.status = TaskStatus::Blocked;
                task.revision = task.revision.saturating_add(1);
                task.updated = now();
                Store::approval_notice(task);
            });
        }

        self.change(|store| {
            let task = store.tasks.get_mut(id).unwrap();
            task.status = TaskStatus::Queued;
            task.revision = task.revision.saturating_add(1);
            task.next = now();
        })
    }

    pub fn checkpoint_task(
        &mut self,
        id: &str,
        checkpoint: &str,
        status: TaskStatus,
        next: u64,
        message: Option<&str>,
    ) -> std::io::Result<()> {
        if checkpoint.len() > CHECKPOINT_LIMIT
            || message.is_some_and(|text| text.is_empty() || text.len() > crate::TEXT_LIMIT)
        {
            return Err(std::io::Error::other("Background checkpoint exceeds its limits."));
        }

        let task = self.task(id)?;

        if task.status != TaskStatus::Running {
            return Err(std::io::Error::other("The background task is not running."));
        }

        if task.uncertain && status != TaskStatus::Blocked {
            return Err(std::io::Error::other(
                "A tool outcome is uncertain; this task must pause.",
            ));
        }

        self.change(|store| {
            let task = store.tasks.get_mut(id).unwrap();

            if status == TaskStatus::Reporting {
                task.result = message.map(str::to_owned);
            }

            task.checkpoint = checkpoint.into();
            task.status = status;
            task.next = next;
            task.unsafe_step = false;
            task.failures = 0;
            task.retry = None;
            task.updated = now();
            task.revision = task.revision.saturating_add(1);

            if let Some(message) = message
                && (matches!(status, TaskStatus::Blocked | TaskStatus::Reporting)
                    || (message != task.last_notice
                        && now().saturating_sub(task.last_notified) >= UPDATE_INTERVAL))
            {
                task.notice = Some(Notice {
                    id: format!("{id}-event-{}", task.revision),
                    text: message.into(),
                });
            }

            if let Some(session) = store.sessions.get_mut(id) {
                session.phase = "safe".into();
            }

            let receipt = format!("{id}-checkpoint-{}", store.tasks[id].revision);
            host_receipt(
                store,
                id,
                "checkpoint",
                &receipt,
                "Checkpoint saved; this step has ended.",
            );
        })
    }

    pub fn uncertain_task(&mut self, id: &str) -> std::io::Result<()> {
        if !self.tasks.get(id).is_some_and(|task| task.unsafe_step) {
            return Ok(());
        }

        self.change(|store| {
            let task = store.tasks.get_mut(id).unwrap();
            task.uncertain = true;
            task.deferred = None;
        })
    }

    #[cfg(all(test, unix))]
    pub fn resume_task(&mut self, id: &str, instruction: &str) -> std::io::Result<()> {
        self.control_task(
            id,
            &krabb_core::task::Command {
                id: format!("resume-{}", crate::schedule_id().map_err(std::io::Error::other)?),
                action: krabb_core::task::Action::Resume,
                revision: self.task(id)?.revision,
                instruction: instruction.into(),
            },
        )?;
        Ok(())
    }

    pub fn cancel_tasks(&mut self, session: &str) -> std::io::Result<Vec<String>> {
        let ids: Vec<_> = self
            .tasks
            .values()
            .filter(|task| {
                (task.origin.session == session || task.id == session)
                    && !task.status.terminal()
                    && task.status != TaskStatus::Reporting
            })
            .map(|task| task.id.clone())
            .collect();

        if ids.is_empty() {
            return Ok(ids);
        }

        self.change(|store| {
            for id in &ids {
                let task = store.tasks.get_mut(id).unwrap();
                task.status = if task.observing() {
                    TaskStatus::Cancelling
                } else {
                    task.status.request(krabb_core::task::Action::Cancel).unwrap_or(task.status)
                };

                task.restart = false;
                task.deferred = None;
                task.revision = task.revision.saturating_add(1);
                task.notice = None;
                task.updated = now();

                if let Some(session) = store.sessions.get_mut(id) {
                    session.status = "cancelled".into();
                }
            }
        })?;

        Ok(ids)
    }

    pub fn publish_task_notice(&mut self, id: &str) -> std::io::Result<()> {
        let task = self.task(id)?;
        let Some(notice) = task.notice.clone() else { return Ok(()) };

        let destination = task.destination().map_err(std::io::Error::other)?;
        let origin = task.origin.session.clone();

        if !self.sessions.contains_key(&origin) {
            return Err(std::io::Error::other(
                "The background task's conversation no longer exists.",
            ));
        }

        if matches!(destination, Destination::Channel(_)) && self.outbox.len() >= super::LIMIT {
            return Err(std::io::Error::other("The delivery outbox is full."));
        }

        self.change(|store| {
            {
                let session = store.sessions.get_mut(&origin).unwrap();
                session.messages.push(Message {
                    id: format!("task-notice-at-{}-{}", now().saturating_mul(1_000), notice.id),
                    session: origin.clone(),
                    role: Role::Assistant,
                    sender: Some("task".into()),
                    content: vec![krabb_core::types::Content::Text { text: notice.text.clone() }],
                });

                compact_messages(&mut session.messages);
                session.updated = now();
            }

            if let Destination::Channel(destination) = destination {
                store.outbox.push(Delivery {
                    id: notice.id.clone(),
                    channel: destination.channel,
                    chat: destination.chat,
                    private: destination.private,
                    thread: destination.thread,
                    text: notice.text.clone(),
                    attempts: 0,
                    created: now(),
                    status: DeliveryStatus::Pending,
                    last_error: None,
                    message_id: None,
                    updated: now(),
                });
            }

            let task = store.tasks.get_mut(id).unwrap();
            task.last_notice = crate::clip(notice.text, 2_000);
            task.last_notified = now();
            task.notice = None;

            if task.status == TaskStatus::Reporting {
                task.status = TaskStatus::Completed;
                task.revision = task.revision.saturating_add(1);
            }
        })
    }
}

fn host_receipt(store: &mut Store, session: &str, tool: &str, id: &str, text: &str) {
    use krabb_core::types::Content;

    let Some(session) = store.sessions.get_mut(session) else { return };

    let Some(assistant) =
        session.messages.iter().rposition(|message| message.role == Role::Assistant)
    else {
        return;
    };

    if session
        .messages
        .iter()
        .rposition(|message| message.role == Role::User)
        .is_some_and(|user| user > assistant)
        || session.messages[assistant + 1..]
            .iter()
            .any(|message| message.role == Role::Tool && message.sender.as_deref() == Some(tool))
    {
        return;
    }

    let Some(position) = session.messages[assistant]
        .content
        .iter()
        .position(|content| matches!(content, Content::ToolCall { name, .. } if name == tool))
    else {
        return;
    };

    let mut offset = 0;
    let mut removed = Vec::new();

    session.messages[assistant].content.retain(|content| {
        let keep = offset <= position || !matches!(content, Content::ToolCall { .. });

        if let (false, Content::ToolCall { name, .. }) = (keep, content) {
            removed.push(name.clone());
        }

        offset += 1;
        keep
    });

    for name in removed {
        if let Some(result) = session.messages[assistant + 1..].iter().position(|message| {
            message.role == Role::Tool && message.sender.as_deref() == Some(name.as_str())
        }) {
            session.messages.remove(assistant + 1 + result);
        }
    }

    session.messages.push(Message {
        id: id.into(),
        session: session.id.clone(),
        role: Role::Tool,
        sender: Some(tool.into()),
        content: vec![Content::Text { text: text.into() }],
    });

    compact_messages(&mut session.messages);
}

#[cfg(test)]
mod tests {
    use super::*;
    use krabb_core::types::Content;

    fn call(name: &str) -> Content {
        Content::ToolCall {
            name: name.into(),
            args: serde_json::json!({}),
            id: None,
            thought_signature: None,
        }
    }

    #[test]
    fn a_host_receipt_drops_the_results_of_the_calls_it_removes() {
        let mut store = Store::default();

        store.create("task", "model").unwrap();

        let message = |id: &str, role: Role, sender: Option<&str>, content| Message {
            id: id.into(),
            session: "task".into(),
            role,
            sender: sender.map(str::to_owned),
            content,
        };

        store.sessions.get_mut("task").unwrap().messages = vec![
            message("assistant", Role::Assistant, None, vec![call("task"), call("read")]),
            message("read-result", Role::Tool, Some("read"), vec![]),
        ];

        host_receipt(&mut store, "task", "task", "receipt", "Saved.");

        let messages = &store.sessions["task"].messages;

        assert_eq!(
            messages.iter().map(|message| message.id.as_str()).collect::<Vec<_>>(),
            ["assistant", "receipt"]
        );

        assert_eq!(messages[0].content, vec![call("task")]);
    }
}
