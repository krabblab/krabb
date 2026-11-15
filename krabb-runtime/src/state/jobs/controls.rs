use super::*;
use krabb_core::task::{Action, Command, ControlError, Receipt};

impl Store {
    pub fn control_job(&mut self, id: &str, command: &Command) -> std::io::Result<Receipt> {
        command.validate().map_err(std::io::Error::other)?;

        let job = self.job(id)?;

        if job.deferrals.iter().any(|receipt| receipt.command.id == command.id) {
            return Err(std::io::Error::other(ControlError::Conflict));
        }

        if let Some(receipt) = job.commands.iter().find(|item| item.command.id == command.id) {
            return if receipt.command == *command {
                Ok(receipt.clone())
            } else {
                Err(std::io::Error::other(ControlError::Conflict))
            };
        }

        if command.revision != job.revision {
            return Err(std::io::Error::other(ControlError::Stale));
        }

        if matches!(command.action, Action::Resume | Action::Steer) && self.conflicting_input(job) {
            return Err(std::io::Error::other(ControlError::Identity));
        }

        if matches!(command.action, Action::Resume | Action::Steer) && self.missing_input(job) {
            return Err(std::io::Error::other(ControlError::Input));
        }

        let deciding = matches!(command.action, Action::Approve | Action::Deny);

        if job.uncertain && (deciding || matches!(command.action, Action::Resume | Action::Steer)) {
            return Err(std::io::Error::other(ControlError::Uncertain));
        }

        let awaiting = job.approval.as_ref().is_some_and(|approval| approval.decision.is_none());

        if deciding && !job.status.terminal() && job.status != JobStatus::Cancelling && !awaiting {
            return Err(std::io::Error::other(ControlError::Approval));
        }

        let mut next = if command.action == Action::Pause {
            job.pause().map_err(std::io::Error::other)?
        } else if awaiting
            && command.action == Action::Resume
            && matches!(job.status, JobStatus::Paused | JobStatus::Blocked)
        {
            JobStatus::Blocked
        } else if job.observing()
            && job.status == JobStatus::Pausing
            && command.action == Action::Resume
        {
            JobStatus::Waiting
        } else {
            job.status.request(command.action).map_err(std::io::Error::other)?
        };

        if job.observing() && !job.status.terminal() {
            let external = job.operation.as_ref().and_then(|operation| operation.receipt.as_ref());

            if command.action == Action::Cancel
                && external.is_none_or(|receipt| !receipt.state.terminal())
            {
                next = JobStatus::Cancelling;
            } else if matches!(command.action, Action::Pause | Action::Steer)
                && external.is_none_or(|receipt| {
                    !receipt.state.terminal()
                        && receipt.state != krabb_core::operation::State::Waiting
                })
            {
                next = JobStatus::Pausing;
            }
        }

        let instruction = command.instruction.trim();

        if !instruction.is_empty()
            && job.instruction.len().saturating_add(instruction.len()).saturating_add(32)
                > crate::TOOL_ARGS_LIMIT
        {
            return Err(std::io::Error::other(ControlError::Invalid));
        }

        let receipt = Receipt {
            command: command.clone(),
            revision: job.revision.saturating_add(1),
            state: next,
        };

        self.change_jobs(|jobs| {
            let job = jobs.get_mut(id).unwrap();
            let previous = job.status;
            job.status = next;
            job.revision = receipt.revision;
            job.updated = now();

            if !matches!(previous, JobStatus::Reporting | JobStatus::Completed) {
                job.notice = None;
            }

            if matches!(command.action, Action::Pause | Action::Resume | Action::Cancel) {
                job.deferred = None;
            }

            match command.action {
                Action::Pause | Action::Cancel => job.restart = false,
                Action::Steer if next == JobStatus::Pausing => job.restart = job.deferred.is_none(),

                _ => {}
            }

            match command.action {
                Action::Approve | Action::Deny => {
                    if let Some(approval) = job.approval.as_mut() {
                        approval.decision = Some(command.action == Action::Approve);
                    }
                }

                Action::Cancel => job.approval = None,

                _ => {}
            }

            if next == JobStatus::Blocked {
                Store::approval_notice(job);
            }

            if !instruction.is_empty() {
                job.instruction.push_str("\n\nUser clarification:\n");
                job.instruction.push_str(instruction);
            }

            if next == JobStatus::Queued || job.observing() {
                job.failures = 0;
                job.next = now();
            }

            if job.commands.len() == 32 {
                job.commands.remove(0);
            }

            job.commands.push(receipt.clone());
        })?;

        Ok(receipt)
    }

    pub(super) fn change_jobs(
        &mut self,
        change: impl FnOnce(&mut BTreeMap<String, Job>),
    ) -> std::io::Result<()> {
        let previous = self.jobs.clone();

        change(&mut self.jobs);

        let saved = (|| {
            self.validate_jobs()?;
            let Some(path) = &self.path else { return Ok(()) };

            if !session_catalog(path) {
                return self.save();
            }

            let bytes = load_file(path, BYTE_LIMIT)?
                .ok_or_else(|| std::io::Error::other("Task catalog is missing."))?;
            let mut index: SessionIndex =
                serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
            index.version = CATALOG_VERSION;
            index.jobs = self.jobs.clone();
            save_json(path, &index)
        })();

        if let Err(error) = saved {
            self.jobs = previous;
            return Err(error);
        }

        Ok(())
    }

    pub fn check_job_step(&self, id: &str, step: u64) -> std::io::Result<()> {
        let job = self.job(id)?;

        if self.conflicting_input(job) {
            return Err(std::io::Error::other(ControlError::Identity));
        }

        if self.missing_input(job) {
            return Err(std::io::Error::other(ControlError::Input));
        }

        if job.step != step || job.status != JobStatus::Running || job.observing() {
            return Err(std::io::Error::other("The task worker is stopped or superseded."));
        }

        Ok(())
    }

    pub fn push_turn(
        &mut self,
        id: &str,
        run: Option<&krabb_core::task::Run>,
        message: Message,
    ) -> std::io::Result<()> {
        if let Some(run) = run {
            self.check_job_step(&run.id, run.generation)?;
        } else if self.jobs.contains_key(id) {
            return Err(std::io::Error::other("The task worker generation is missing."));
        }

        self.push_with_status(id, message, false, true, run.map(|run| run.id.as_str()))
    }

    pub fn uncertain_job_step(&mut self, id: &str, step: u64) -> std::io::Result<()> {
        if self.check_job_step(id, step).is_ok() {
            self.uncertain_job(id)?;
        }

        Ok(())
    }

    pub fn finish_job_control(&mut self, id: &str, step: u64) -> std::io::Result<()> {
        let job = self.job(id)?;

        if job.step != step {
            return Err(std::io::Error::other("The task worker is superseded."));
        }

        if !job.status.stopping() || job.observing() {
            return Ok(());
        }

        self.change(|store| {
            let job = store.jobs.get_mut(id).unwrap();
            job.uncertain |= job.unsafe_step;
            job.status = if job.status == JobStatus::Cancelling {
                JobStatus::Cancelled
            } else if job.uncertain {
                JobStatus::Blocked
            } else if job.restart {
                job.next = now();
                JobStatus::Queued
            } else {
                JobStatus::Paused
            };

            if job.uncertain || job.status.terminal() {
                job.deferred = None;
            }

            job.restart = false;
            job.updated = now();
            job.revision = job.revision.saturating_add(1);

            let text = if job.uncertain {
                "The task has stopped, but an action's outcome is uncertain. Review its effects \
                    before starting related work."
            } else if job.status == JobStatus::Paused {
                "Task paused. Its goal and saved context are preserved for later."
            } else if job.status == JobStatus::Cancelled {
                "Task cancelled. This does not undo effects of earlier actions."
            } else {
                "The updated instruction is saved. Work will continue from its checkpoint."
            };

            job.notice =
                Some(Notice { id: format!("{id}-event-{}", job.revision), text: text.into() });

            if let Some(session) = store.sessions.get_mut(id) {
                session.status =
                    if job.status == JobStatus::Cancelled { "cancelled" } else { "idle" }.into();
                session.inflight = None;
                session.queued.clear();
                session.queue_roles.clear();
            }
        })
    }
}
