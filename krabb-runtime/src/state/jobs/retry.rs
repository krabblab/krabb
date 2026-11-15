use super::*;
use krabb_core::failure::{Failure, Kind};

const ATTEMPTS: u32 = 3;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Retry {
    pub failure: Failure,
    pub at: u64,
}

impl Job {
    pub fn ready_at(&self) -> u64 {
        self.retry.as_ref().map_or(self.next, |retry| self.next.max(retry.at))
    }
}

impl Store {
    pub(in crate::state) fn migrate_retries(&mut self) -> bool {
        let mut changed = false;

        for job in self.jobs.values_mut().filter(|job| job.failures > 0 && job.retry.is_none()) {
            job.retry = Some(Retry { failure: Failure::new(Kind::Unknown), at: job.next });

            if !job.observing() && matches!(job.status, JobStatus::Queued | JobStatus::Waiting) {
                job.status = JobStatus::Blocked;
                job.revision = job.revision.saturating_add(1);
                job.notice = Some(Notice {
                    id: format!("{}-event-{}", job.id, job.revision),
                    text: "The saved retry needs review after upgrading. Its goal, prior use, and \
                        earliest retry time are preserved."
                        .into(),
                });
            }

            changed = true;
        }

        changed
    }

    pub fn retry_job(&mut self, id: &str) -> std::io::Result<()> {
        self.retry_job_after(id, Failure::new(Kind::Transient), now())
    }

    pub fn retry_job_after(&mut self, id: &str, failure: Failure, at: u64) -> std::io::Result<()> {
        failure.validate().map_err(std::io::Error::other)?;
        let job = self.job(id)?;

        if job.unsafe_step || job.uncertain {
            return Err(std::io::Error::other("An interrupted mutation cannot be replayed."));
        }

        if job.status != JobStatus::Running || !failure.kind.retryable() {
            return Err(std::io::Error::other("The task cannot admit this retry."));
        }

        self.change(|store| {
            let job = store.jobs.get_mut(id).unwrap();
            job.failures = job.failures.saturating_add(1);
            let base = 2_u64.saturating_pow(job.failures.min(12)).min(3600);
            let spread =
                job.id.bytes().fold(0_u64, |value, byte| {
                    value.wrapping_mul(31).wrapping_add(u64::from(byte))
                }) % (base / 4 + 1);
            let delay = base
                .saturating_add(spread)
                .min(3600)
                .max(failure.retry_after_seconds.unwrap_or(0))
                .saturating_add(1);
            let deadline = at.saturating_add(delay);
            job.retry = Some(Retry { failure, at: deadline });
            job.next = deadline;
            job.status =
                if job.failures >= ATTEMPTS { JobStatus::Blocked } else { JobStatus::Waiting };

            job.updated = at;
            job.revision = job.revision.saturating_add(1);

            if job.failures >= ATTEMPTS {
                job.notice = Some(Notice {
                    id: format!("{id}-event-{}", job.revision),
                    text: format!(
                        "The task stopped retrying after {ATTEMPTS} consecutive failures. Its \
                        goal and partial work are saved. Review its provider or limits before \
                        resuming."
                    ),
                });
            } else if job.failures == 1 {
                job.notice = Some(Notice {
                    id: format!("{id}-event-{}", job.revision),
                    text: format!(
                        "The request failed. Its saved task will retry no earlier than \
                        {}.",
                        krabb_date::format_datetime(deadline)
                    ),
                });
            }
        })
    }
}
