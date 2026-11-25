use super::*;
use krabb_core::task::{Action, Command, ControlError, Receipt};

impl Store {
    pub fn control_task(&mut self, id: &str, command: &Command) -> std::io::Result<Receipt> {
        command.validate().map_err(std::io::Error::other)?;

        let task = self.task(id)?;

        if task.deferrals.iter().any(|receipt| receipt.command.id == command.id) {
            return Err(std::io::Error::other(ControlError::Conflict));
        }

        if let Some(receipt) = task.commands.iter().find(|item| item.command.id == command.id) {
            return if receipt.command == *command {
                Ok(receipt.clone())
            } else {
                Err(std::io::Error::other(ControlError::Conflict))
            };
        }

        if command.revision != task.revision {
            return Err(std::io::Error::other(ControlError::Stale));
        }

        if matches!(command.action, Action::Resume | Action::Steer) && self.conflicting_input(task)
        {
            return Err(std::io::Error::other(ControlError::Identity));
        }

        if matches!(command.action, Action::Resume | Action::Steer) && self.missing_input(task) {
            return Err(std::io::Error::other(ControlError::Input));
        }

        let deciding = matches!(command.action, Action::Approve | Action::Deny);

        if task.uncertain && (deciding || matches!(command.action, Action::Resume | Action::Steer))
        {
            return Err(std::io::Error::other(ControlError::Uncertain));
        }

        let awaiting = task.approval.as_ref().is_some_and(|approval| approval.decision.is_none());

        if deciding && !task.status.terminal() && task.status != TaskStatus::Cancelling && !awaiting
        {
            return Err(std::io::Error::other(ControlError::Approval));
        }

        let mut next = if command.action == Action::Pause {
            task.pause().map_err(std::io::Error::other)?
        } else if awaiting
            && command.action == Action::Resume
            && matches!(task.status, TaskStatus::Paused | TaskStatus::Blocked)
        {
            TaskStatus::Blocked
        } else if task.observing()
            && task.status == TaskStatus::Pausing
            && command.action == Action::Resume
        {
            TaskStatus::Waiting
        } else {
            task.status.request(command.action).map_err(std::io::Error::other)?
        };

        if task.observing() && !task.status.terminal() {
            let external = task.operation.as_ref().and_then(|operation| operation.receipt.as_ref());

            if command.action == Action::Cancel
                && external.is_none_or(|receipt| !receipt.state.terminal())
            {
                next = TaskStatus::Cancelling;
            } else if matches!(command.action, Action::Pause | Action::Steer)
                && external.is_none_or(|receipt| {
                    !receipt.state.terminal()
                        && receipt.state != krabb_core::operation::State::Waiting
                })
            {
                next = TaskStatus::Pausing;
            }
        }

        let instruction = command.instruction.trim();

        if !instruction.is_empty()
            && task.instruction.len().saturating_add(instruction.len()).saturating_add(32)
                > crate::TOOL_ARGS_LIMIT
        {
            return Err(std::io::Error::other(ControlError::Invalid));
        }

        let receipt = Receipt {
            command: command.clone(),
            revision: task.revision.saturating_add(1),
            state: next,
        };

        self.change_tasks(|tasks| {
            let task = tasks.get_mut(id).unwrap();
            let previous = task.status;
            task.status = next;
            task.revision = receipt.revision;
            task.updated = now();

            if !matches!(previous, TaskStatus::Reporting | TaskStatus::Completed) {
                task.notice = None;
            }

            if matches!(command.action, Action::Pause | Action::Resume | Action::Cancel) {
                task.deferred = None;
            }

            match command.action {
                Action::Pause | Action::Cancel => task.restart = false,

                Action::Steer if next == TaskStatus::Pausing => {
                    task.restart = task.deferred.is_none()
                }

                _ => {}
            }

            match command.action {
                Action::Approve | Action::Deny => {
                    if let Some(approval) = task.approval.as_mut() {
                        approval.decision = Some(command.action == Action::Approve);
                    }
                }

                Action::Cancel => task.approval = None,

                _ => {}
            }

            if next == TaskStatus::Blocked {
                Store::approval_notice(task);
            }

            if !instruction.is_empty() {
                task.instruction.push_str("\n\nUser clarification:\n");
                task.instruction.push_str(instruction);
            }

            if next == TaskStatus::Queued || task.observing() {
                task.failures = 0;
                task.next = now();
            }

            if task.commands.len() == 32 {
                task.commands.remove(0);
            }

            task.commands.push(receipt.clone());
        })?;

        Ok(receipt)
    }

    pub(super) fn change_tasks(
        &mut self,
        change: impl FnOnce(&mut BTreeMap<String, Task>),
    ) -> std::io::Result<()> {
        let previous = self.tasks.clone();

        change(&mut self.tasks);

        let saved = (|| {
            self.validate_tasks()?;
            let Some(path) = &self.path else { return Ok(()) };

            if !session_catalog(path) {
                return self.save();
            }

            let bytes = load_file(path, BYTE_LIMIT)?
                .ok_or_else(|| std::io::Error::other("Task catalog is missing."))?;
            let mut index: SessionIndex =
                serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
            index.version = CATALOG_VERSION;
            index.tasks = self.tasks.clone();
            save_json(path, &index)
        })();

        if let Err(error) = saved {
            self.tasks = previous;
            return Err(error);
        }

        Ok(())
    }

    pub fn check_task_step(&self, id: &str, step: u64) -> std::io::Result<()> {
        let task = self.task(id)?;

        if self.conflicting_input(task) {
            return Err(std::io::Error::other(ControlError::Identity));
        }

        if self.missing_input(task) {
            return Err(std::io::Error::other(ControlError::Input));
        }

        if task.step != step || task.status != TaskStatus::Running || task.observing() {
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
            self.check_task_step(&run.id, run.generation)?;
        } else if self.tasks.contains_key(id) {
            return Err(std::io::Error::other("The task worker generation is missing."));
        }

        self.push_with_status(id, message, false, true, run.map(|run| run.id.as_str()))
    }

    pub fn uncertain_task_step(&mut self, id: &str, step: u64) -> std::io::Result<()> {
        if self.check_task_step(id, step).is_ok() {
            self.uncertain_task(id)?;
        }

        Ok(())
    }

    pub fn finish_stop(&mut self, id: &str, step: u64) -> std::io::Result<()> {
        let task = self.task(id)?;

        if task.step != step {
            return Err(std::io::Error::other("The task worker is superseded."));
        }

        if !task.status.stopping() || task.observing() {
            return Ok(());
        }

        self.change(|store| {
            let task = store.tasks.get_mut(id).unwrap();
            task.uncertain |= task.unsafe_step;
            task.status = if task.status == TaskStatus::Cancelling {
                TaskStatus::Cancelled
            } else if task.uncertain {
                TaskStatus::Blocked
            } else if task.restart {
                task.next = now();
                TaskStatus::Queued
            } else {
                TaskStatus::Paused
            };

            if task.uncertain || task.status.terminal() {
                task.deferred = None;
            }

            task.restart = false;
            task.updated = now();
            task.revision = task.revision.saturating_add(1);

            let text = if task.uncertain {
                "The task has stopped, but an action's outcome is uncertain. Review its effects \
                    before starting related work."
            } else if task.status == TaskStatus::Paused {
                "Task paused. Its goal and saved context are preserved for later."
            } else if task.status == TaskStatus::Cancelled {
                "Task cancelled. This does not undo effects of earlier actions."
            } else {
                "The updated instruction is saved. Work will continue from its checkpoint."
            };

            task.notice =
                Some(Notice { id: format!("{id}-event-{}", task.revision), text: text.into() });

            if let Some(session) = store.sessions.get_mut(id) {
                session.status =
                    if task.status == TaskStatus::Cancelled { "cancelled" } else { "idle" }.into();
                session.inflight = None;
                session.queued.clear();
                session.queue_roles.clear();
            }
        })
    }
}
