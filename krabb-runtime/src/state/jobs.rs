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

pub(crate) use krabb_core::task::State as JobStatus;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Notice {
    id: String,
    text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Job {
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
    pub status: JobStatus,
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

impl Job {
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
    pub(crate) fn job(&self, id: &str) -> std::io::Result<&Job> {
        self.jobs.get(id).ok_or_else(|| std::io::Error::other(NOT_FOUND))
    }

    pub(crate) fn session(&self, id: &str) -> std::io::Result<&Session> {
        self.sessions.get(id).ok_or_else(|| std::io::Error::other("Conversation not found."))
    }

    pub(super) fn validate_jobs(&self) -> std::io::Result<()> {
        if self.jobs.len() > LIMIT {
            return Err(std::io::Error::other("Background job capacity exceeded."));
        }

        for (id, job) in &self.jobs {
            if let Some(approval) = &job.approval {
                approval.validate()?;

                if job.status.terminal() || approval.generation > job.step {
                    return Err(std::io::Error::other("Invalid pending approval."));
                }
            }

            if job.request.is_some() != job.input.is_some()
                || job.input.as_ref().is_some_and(|input| !input.valid())
                || job.request.as_ref().is_some_and(|request| request.is_empty())
            {
                return Err(std::io::Error::other("Task input identity is missing or invalid."));
            }

            job.budget
                .as_ref()
                .ok_or_else(|| std::io::Error::other("Task execution accounting is missing."))?
                .validate()
                .map_err(std::io::Error::other)?;

            if job.deferrals.len() > 32
                || job.deferrals.iter().any(|receipt| {
                    receipt.command.validate().is_err()
                        || job
                            .commands
                            .iter()
                            .any(|control| control.command.id == receipt.command.id)
                        || !matches!(receipt.state, JobStatus::Paused | JobStatus::Pausing)
                        || defer::validate(&receipt.command.when).is_err()
                        || receipt.command.revision >= receipt.revision
                        || receipt.revision > job.revision
                })
                || job
                    .deferrals
                    .iter()
                    .map(|receipt| &receipt.command.id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != job.deferrals.len()
                || job.deferred.as_ref().is_some_and(|receipt| {
                    !job.deferrals.contains(receipt)
                        || job.uncertain
                        || !matches!(job.status, JobStatus::Paused | JobStatus::Pausing)
                        || krabb_date::validate_timezone(&receipt.command.when.timezone).is_err()
                })
            {
                return Err(std::io::Error::other("Invalid deferred task control."));
            }

            if let Some(operation) = &job.operation {
                operation.validate()?;
            }

            if let Some(retry) = &job.retry {
                retry.failure.validate().map_err(std::io::Error::other)?;

                if !retry.failure.kind.retryable()
                    && retry.failure.kind != krabb_core::failure::Kind::Unknown
                {
                    return Err(std::io::Error::other("Invalid saved retry outcome."));
                }
            }

            if job.failures > 0 && job.retry.is_none() {
                return Err(std::io::Error::other(
                    "The saved retry reason and deadline are missing.",
                ));
            }

            if id != &job.id
                || !id.strip_prefix("job-").is_some_and(|value| {
                    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
                })
                || !valid(id)
                || !valid(&job.provider)
                || !valid(&job.origin.session)
                || job.instruction.is_empty()
                || job.instruction.len() > crate::TOOL_ARGS_LIMIT
                || job.revision == u64::MAX
                || job.step == u64::MAX
                || job.grants.len() > 32
                || job.grants.iter().any(|receipt| {
                    receipt.grant.validate().is_err()
                        || receipt.grant.revision >= receipt.revision
                        || receipt.revision > job.revision
                })
                || job
                    .grants
                    .iter()
                    .map(|receipt| &receipt.grant.id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != job.grants.len()
                || job.commands.len() > 32
                || job.commands.iter().any(|receipt| {
                    receipt.command.validate().is_err()
                        || receipt.command.revision >= receipt.revision
                        || receipt.revision > job.revision
                })
                || job
                    .commands
                    .iter()
                    .map(|receipt| &receipt.command.id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != job.commands.len()
                || job.checkpoint.len() > CHECKPOINT_LIMIT
                || job.model.is_empty()
                || job.model.len() > crate::META_LIMIT
                || !job.workspace.is_absolute()
                || job.request.as_ref().is_some_and(|id| id.len() > crate::META_LIMIT)
                || job.result.as_ref().is_some_and(|text| text.len() > crate::TEXT_LIMIT)
                || job.last_notice.len() > crate::TEXT_LIMIT
                || job.notice.as_ref().is_some_and(|notice| {
                    !valid(&notice.id) || notice.text.len() > crate::TEXT_LIMIT
                })
                || job.origin.source.is_none()
                || job.destination().is_err()
            {
                return Err(std::io::Error::other("Invalid background job record."));
            }
        }

        Ok(())
    }

    pub(super) fn recover_jobs(&mut self) -> std::io::Result<bool> {
        let mut changed = self.restore_inputs()?;

        for job in self.jobs.values_mut() {
            if let Some(budget) = &mut job.budget {
                changed |= budget.recover().map_err(std::io::Error::other)?;
            }

            if let Some(operation) = &mut job.operation {
                if operation.phase == OperationPhase::Dispatching {
                    operation.phase = OperationPhase::Uncertain;
                    job.uncertain = true;
                    job.deferred = None;
                    job.unsafe_step = false;

                    if !job.status.terminal() {
                        job.status = JobStatus::Blocked;
                    }

                    job.revision = job.revision.saturating_add(1);
                    job.notice = Some(Notice {
                        id: format!("{}-event-{}", job.id, job.revision),
                        text: "The operation's start receipt was lost. Review its effects \
                               before another start."
                            .into(),
                    });

                    changed = true;
                    continue;
                }

                if operation.active() {
                    if job.status == JobStatus::Running {
                        job.status = JobStatus::Waiting;
                    }

                    job.unsafe_step = false;
                    job.next = job.next.min(now().saturating_add(300));
                    changed = true;
                    continue;
                }
            }

            if job.status == JobStatus::Paused && job.commands.is_empty() && job.deferred.is_none()
            {
                job.status = JobStatus::Blocked;
                changed = true;
            }

            if job.status != JobStatus::Running && !job.status.stopping() {
                continue;
            }

            let stopping = job.status.stopping();
            job.uncertain |= job.unsafe_step;
            job.status = if job.status == JobStatus::Cancelling {
                JobStatus::Cancelled
            } else if job.uncertain {
                JobStatus::Blocked
            } else if !stopping
                && job.approval.as_ref().is_some_and(|approval| approval.decision.is_none())
            {
                Store::approval_notice(job);
                JobStatus::Blocked
            } else if stopping && !job.restart {
                JobStatus::Paused
            } else {
                job.next = now();
                JobStatus::Queued
            };

            if job.uncertain || job.status.terminal() {
                job.deferred = None;
            }

            job.restart = false;
            job.revision = job.revision.saturating_add(1);

            if job.uncertain {
                job.notice = Some(Notice {
                    id: format!("{}-event-{}", job.id, job.revision),
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

    pub fn enqueue_job(&mut self, job: Job) -> std::io::Result<()> {
        self.insert_job(job, true)
    }

    pub(super) fn insert_job(&mut self, job: Job, handoff: bool) -> std::io::Result<()> {
        if self.jobs.contains_key(&job.id) {
            return Err(std::io::Error::other("The background job already exists."));
        }

        let retire = if self.jobs.len() >= LIMIT {
            Some(
                self.jobs
                    .values()
                    .filter(|job| !job.protected())
                    .min_by_key(|job| job.updated)
                    .ok_or_else(|| std::io::Error::other("Background job capacity is full."))?
                    .id
                    .clone(),
            )
        } else {
            None
        };

        self.change(|store| {
            if let Some(id) = retire {
                store.retire_job(&id);
            }

            if handoff {
                host_receipt(
                    store,
                    &job.origin.session,
                    "task",
                    &format!("{}-handoff", job.id),
                    "The background goal is saved. Its result will be delivered automatically.",
                );
            }

            store.jobs.insert(job.id.clone(), job);
        })
    }

    pub fn claim_job(&mut self, id: &str, at: u64) -> std::io::Result<Job> {
        let job = self.job(id)?;

        if !job.observing() && (self.conflicting_input(job) || self.missing_input(job)) {
            let error = if self.conflicting_input(job) {
                krabb_core::task::ControlError::Identity
            } else {
                krabb_core::task::ControlError::Input
            };

            self.change(|store| {
                store.fence_inputs();
            })?;
            return Err(std::io::Error::other(error));
        }

        if !(matches!(job.status, JobStatus::Queued | JobStatus::Waiting)
            || job.observing()
                && (job.status.stopping()
                    || matches!(
                        job.status,
                        JobStatus::Paused | JobStatus::Cancelled | JobStatus::Blocked
                    )))
            || job.ready_at() > at
            || job.notice.is_some() && !job.observing()
        {
            return Err(std::io::Error::other("The background job is not ready."));
        }

        if job.foreground
            && !job.observing()
            && self
                .sessions
                .get(&job.origin.session)
                .is_some_and(|session| session.status == "working" || session.inflight.is_some())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "The originating conversation is still active.",
            ));
        }

        if job.foreground
            && !job.observing()
            && self.jobs.values().any(|other| {
                other.id != job.id
                    && other.foreground
                    && !other.observing()
                    && other.origin.session == job.origin.session
                    && (other.status == JobStatus::Running || other.status.stopping())
            })
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "The previous foreground request is still active.",
            ));
        }

        let mut budget = job.account()?;
        budget.recover().map_err(std::io::Error::other)?;

        self.change(|store| {
            let job = store.jobs.get_mut(id).unwrap();

            if !job.observing() {
                job.status = JobStatus::Running;
                job.revision = job.revision.saturating_add(1);
            }

            job.unsafe_step = false;
            job.budget = Some(budget);
            job.step = job.step.saturating_add(1);
            job.updated = at;

            if let Some(session) = store.sessions.get_mut(id) {
                session.inflight = None;
                session.queued.clear();
                session.queue_roles.clear();
                session.status = "idle".into();
            }
        })?;

        Ok(self.jobs[id].clone())
    }

    pub fn recover_job(&mut self, id: &str) -> std::io::Result<()> {
        self.recover_budget(id)?;
        let job = self.job(id)?;

        if let Some(operation) = &job.operation {
            if operation.phase == OperationPhase::Dispatching {
                let key = operation.start.id.clone();
                return self.abandon_operation(id, &key);
            }

            if operation.active() {
                if job.status == JobStatus::Running {
                    return self.change(|store| {
                        store.jobs.get_mut(id).unwrap().status = JobStatus::Waiting;
                    });
                }

                return Ok(());
            }
        }

        if job.status.stopping() {
            return self.finish_job_control(id, job.step);
        }

        if job.status != JobStatus::Running {
            return Ok(());
        }

        if job.unsafe_step {
            let checkpoint = job.checkpoint.clone();
            self.uncertain_job(id)?;
            return self.checkpoint_job(
                id,
                &checkpoint,
                JobStatus::Blocked,
                now(),
                Some(
                    "The background job was interrupted during an action and is paused to avoid \
                    repeating its effects.",
                ),
            );
        }

        if job.approval.as_ref().is_some_and(|approval| approval.decision.is_none()) {
            return self.change(|store| {
                let job = store.jobs.get_mut(id).unwrap();
                job.status = JobStatus::Blocked;
                job.revision = job.revision.saturating_add(1);
                job.updated = now();
                Store::approval_notice(job);
            });
        }

        self.change(|store| {
            let job = store.jobs.get_mut(id).unwrap();
            job.status = JobStatus::Queued;
            job.revision = job.revision.saturating_add(1);
            job.next = now();
        })
    }

    pub fn checkpoint_job(
        &mut self,
        id: &str,
        checkpoint: &str,
        status: JobStatus,
        next: u64,
        message: Option<&str>,
    ) -> std::io::Result<()> {
        if checkpoint.len() > CHECKPOINT_LIMIT
            || message.is_some_and(|text| text.is_empty() || text.len() > crate::TEXT_LIMIT)
        {
            return Err(std::io::Error::other("Background checkpoint exceeds its limits."));
        }

        let job = self.job(id)?;

        if job.status != JobStatus::Running {
            return Err(std::io::Error::other("The background job is not running."));
        }

        if job.uncertain && status != JobStatus::Blocked {
            return Err(std::io::Error::other("A tool outcome is uncertain; this job must pause."));
        }

        self.change(|store| {
            let job = store.jobs.get_mut(id).unwrap();

            if status == JobStatus::Reporting {
                job.result = message.map(str::to_owned);
            }

            job.checkpoint = checkpoint.into();
            job.status = status;
            job.next = next;
            job.unsafe_step = false;
            job.failures = 0;
            job.retry = None;
            job.updated = now();
            job.revision = job.revision.saturating_add(1);

            if let Some(message) = message
                && (matches!(status, JobStatus::Blocked | JobStatus::Reporting)
                    || (message != job.last_notice
                        && now().saturating_sub(job.last_notified) >= UPDATE_INTERVAL))
            {
                job.notice = Some(Notice {
                    id: format!("{id}-event-{}", job.revision),
                    text: message.into(),
                });
            }

            if let Some(session) = store.sessions.get_mut(id) {
                session.phase = "safe".into();
            }

            let receipt = format!("{id}-checkpoint-{}", store.jobs[id].revision);
            host_receipt(
                store,
                id,
                "checkpoint",
                &receipt,
                "Checkpoint saved; this step has ended.",
            );
        })
    }

    pub fn uncertain_job(&mut self, id: &str) -> std::io::Result<()> {
        if !self.jobs.get(id).is_some_and(|job| job.unsafe_step) {
            return Ok(());
        }

        self.change(|store| {
            let job = store.jobs.get_mut(id).unwrap();
            job.uncertain = true;
            job.deferred = None;
        })
    }

    #[cfg(test)]
    pub fn resume_job(&mut self, id: &str, instruction: &str) -> std::io::Result<()> {
        self.control_job(
            id,
            &krabb_core::task::Command {
                id: format!("resume-{}", crate::schedule_id().map_err(std::io::Error::other)?),
                action: krabb_core::task::Action::Resume,
                revision: self.job(id)?.revision,
                instruction: instruction.into(),
            },
        )?;
        Ok(())
    }

    pub fn cancel_jobs(&mut self, session: &str) -> std::io::Result<Vec<String>> {
        let ids: Vec<_> = self
            .jobs
            .values()
            .filter(|job| {
                (job.origin.session == session || job.id == session)
                    && !job.status.terminal()
                    && job.status != JobStatus::Reporting
            })
            .map(|job| job.id.clone())
            .collect();

        if ids.is_empty() {
            return Ok(ids);
        }

        self.change(|store| {
            for id in &ids {
                let job = store.jobs.get_mut(id).unwrap();
                job.status = if job.observing() {
                    JobStatus::Cancelling
                } else {
                    job.status.request(krabb_core::task::Action::Cancel).unwrap_or(job.status)
                };

                job.restart = false;
                job.deferred = None;
                job.revision = job.revision.saturating_add(1);
                job.notice = None;
                job.updated = now();

                if let Some(session) = store.sessions.get_mut(id) {
                    session.status = "cancelled".into();
                }
            }
        })?;

        Ok(ids)
    }

    pub fn publish_job_notice(&mut self, id: &str) -> std::io::Result<()> {
        let job = self.job(id)?;
        let Some(notice) = job.notice.clone() else { return Ok(()) };

        let destination = job.destination().map_err(std::io::Error::other)?;
        let origin = job.origin.session.clone();

        if !self.sessions.contains_key(&origin) {
            return Err(std::io::Error::other(
                "The background job's conversation no longer exists.",
            ));
        }

        if matches!(destination, Destination::Channel(_)) && self.outbox.len() >= super::LIMIT {
            return Err(std::io::Error::other("The delivery outbox is full."));
        }

        self.change(|store| {
            {
                let session = store.sessions.get_mut(&origin).unwrap();
                session.messages.push(Message {
                    id: format!("job-notice-at-{}-{}", now().saturating_mul(1_000), notice.id),
                    session: origin.clone(),
                    role: Role::Assistant,
                    sender: Some("job".into()),
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

            let job = store.jobs.get_mut(id).unwrap();
            job.last_notice = crate::clip(notice.text, 2_000);
            job.last_notified = now();
            job.notice = None;

            if job.status == JobStatus::Reporting {
                job.status = JobStatus::Completed;
                job.revision = job.revision.saturating_add(1);
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

        store.create("job", "model").unwrap();

        let message = |id: &str, role: Role, sender: Option<&str>, content| Message {
            id: id.into(),
            session: "job".into(),
            role,
            sender: sender.map(str::to_owned),
            content,
        };

        store.sessions.get_mut("job").unwrap().messages = vec![
            message("assistant", Role::Assistant, None, vec![call("task"), call("read")]),
            message("read-result", Role::Tool, Some("read"), vec![]),
        ];

        host_receipt(&mut store, "job", "task", "receipt", "Saved.");

        let messages = &store.sessions["job"].messages;

        assert_eq!(
            messages.iter().map(|message| message.id.as_str()).collect::<Vec<_>>(),
            ["assistant", "receipt"]
        );

        assert_eq!(messages[0].content, vec![call("task")]);
    }
}
