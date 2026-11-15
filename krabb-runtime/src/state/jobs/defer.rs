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

impl Job {
    pub(super) fn pause(&self) -> Result<JobStatus, ControlError> {
        let next = self.status.request(Action::Pause)?;

        if self.observing()
            && self.operation.as_ref().and_then(|operation| operation.receipt.as_ref()).is_none_or(
                |receipt| {
                    !receipt.state.terminal()
                        && receipt.state != krabb_core::operation::State::Waiting
                },
            )
        {
            Ok(JobStatus::Pausing)
        } else {
            Ok(next)
        }
    }
}

impl Store {
    pub fn defer_job(
        &mut self,
        id: &str,
        command: &defer::Command,
        at: u64,
    ) -> std::io::Result<Receipt> {
        command.validate().map_err(std::io::Error::other)?;
        let job = self.job(id)?;

        if self.conflicting_input(job) {
            return Err(std::io::Error::other(ControlError::Identity));
        }

        if self.missing_input(job) {
            return Err(std::io::Error::other(ControlError::Input));
        }

        if job.commands.iter().any(|receipt| receipt.command.id == command.id) {
            return Err(std::io::Error::other(ControlError::Conflict));
        }

        if let Some(receipt) = job.deferrals.iter().find(|receipt| receipt.command.id == command.id)
        {
            return if receipt.command == *command {
                Ok(receipt.clone())
            } else {
                Err(std::io::Error::other(ControlError::Conflict))
            };
        }

        if job.revision != command.revision {
            return Err(std::io::Error::other(ControlError::Stale));
        }

        if job.uncertain {
            return Err(std::io::Error::other(ControlError::Uncertain));
        }

        validate(&command.when)?;

        if command.when.at <= at {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Choose a future time to resume the task.",
            ));
        }

        let state = job.pause().map_err(std::io::Error::other)?;
        let revision = job
            .revision
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("Task revision is exhausted."))?;
        let receipt = Receipt { command: command.clone(), revision, state };
        self.change_jobs(|jobs| {
            let job = jobs.get_mut(id).unwrap();
            job.status = state;
            job.revision = revision;
            job.restart = false;
            job.updated = at;
            job.notice = None;

            if job.deferrals.len() == 32 {
                job.deferrals.remove(0);
            }

            job.deferrals.push(receipt.clone());
            job.deferred = Some(receipt.clone());
        })?;
        Ok(receipt)
    }

    pub fn release_deferred(&mut self, at: u64) -> std::io::Result<usize> {
        let due = self
            .jobs
            .values()
            .filter(|job| {
                job.status == JobStatus::Paused
                    && !job.uncertain
                    && job.deferred.as_ref().is_some_and(|receipt| receipt.command.when.at <= at)
            })
            .map(|job| job.id.clone())
            .collect::<Vec<_>>();

        if due.is_empty() {
            return Ok(0);
        }

        self.change_jobs(|jobs| {
            for id in &due {
                let job = jobs.get_mut(id).unwrap();
                let awaiting =
                    job.approval.as_ref().is_some_and(|approval| approval.decision.is_none());

                job.deferred = None;
                job.status = if awaiting { JobStatus::Blocked } else { JobStatus::Queued };

                job.next = now();
                job.updated = at;
                job.restart = false;
                job.failures = 0;
                job.notice = None;
                job.revision = job.revision.saturating_add(1);

                if awaiting {
                    Store::approval_notice(job);
                }
            }
        })?;
        Ok(due.len())
    }
}
