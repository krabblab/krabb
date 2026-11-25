use super::*;

use krabb_core::task::{
    Action, ControlError,
    defer::{self, Receipt},
};

pub(super) fn validate(when: &defer::When) -> std::io::Result<()> {
    when.validate().map_err(std::io::Error::other)?;
    jiff::Timestamp::from_second(when.at as i64).map_err(std::io::Error::other)?;
    krabb_date::validate_timezone(&when.timezone).map_err(std::io::Error::other)
}

impl Task {
    pub(super) fn pause(&self) -> Result<TaskStatus, ControlError> {
        let next = self.status.request(Action::Pause)?;

        if self.observing()
            && self.operation.as_ref().and_then(|operation| operation.receipt.as_ref()).is_none_or(
                |receipt| {
                    !receipt.state.terminal()
                        && receipt.state != krabb_core::operation::State::Waiting
                },
            )
        {
            Ok(TaskStatus::Pausing)
        } else {
            Ok(next)
        }
    }
}

impl Store {
    pub fn defer_task(
        &mut self,
        id: &str,
        command: &defer::Command,
        at: u64,
    ) -> std::io::Result<Receipt> {
        command.validate().map_err(std::io::Error::other)?;
        let task = self.task(id)?;

        if self.conflicting_input(task) {
            return Err(std::io::Error::other(ControlError::Identity));
        }

        if self.missing_input(task) {
            return Err(std::io::Error::other(ControlError::Input));
        }

        if task.commands.iter().any(|receipt| receipt.command.id == command.id) {
            return Err(std::io::Error::other(ControlError::Conflict));
        }

        if let Some(receipt) =
            task.deferrals.iter().find(|receipt| receipt.command.id == command.id)
        {
            return if receipt.command == *command {
                Ok(receipt.clone())
            } else {
                Err(std::io::Error::other(ControlError::Conflict))
            };
        }

        if task.revision != command.revision {
            return Err(std::io::Error::other(ControlError::Stale));
        }

        if task.uncertain {
            return Err(std::io::Error::other(ControlError::Uncertain));
        }

        validate(&command.when)?;

        if command.when.at <= at {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Choose a future time to resume the task.",
            ));
        }

        let state = task.pause().map_err(std::io::Error::other)?;
        let revision = task
            .revision
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("Task revision is exhausted."))?;
        let receipt = Receipt { command: command.clone(), revision, state };
        self.change_tasks(|tasks| {
            let task = tasks.get_mut(id).unwrap();
            task.status = state;
            task.revision = revision;
            task.restart = false;
            task.updated = at;
            task.notice = None;

            if task.deferrals.len() == 32 {
                task.deferrals.remove(0);
            }

            task.deferrals.push(receipt.clone());
            task.deferred = Some(receipt.clone());
        })?;
        Ok(receipt)
    }

    pub fn release_deferred(&mut self, at: u64) -> std::io::Result<usize> {
        let due = self
            .tasks
            .values()
            .filter(|task| {
                task.status == TaskStatus::Paused
                    && !task.uncertain
                    && task.deferred.as_ref().is_some_and(|receipt| receipt.command.when.at <= at)
            })
            .map(|task| task.id.clone())
            .collect::<Vec<_>>();

        if due.is_empty() {
            return Ok(0);
        }

        self.change_tasks(|tasks| {
            for id in &due {
                let task = tasks.get_mut(id).unwrap();
                let awaiting =
                    task.approval.as_ref().is_some_and(|approval| approval.decision.is_none());

                task.deferred = None;
                task.status = if awaiting { TaskStatus::Blocked } else { TaskStatus::Queued };

                task.next = now();
                task.updated = at;
                task.restart = false;
                task.failures = 0;
                task.notice = None;
                task.revision = task.revision.saturating_add(1);

                if awaiting {
                    Store::approval_notice(task);
                }
            }
        })?;
        Ok(due.len())
    }
}
