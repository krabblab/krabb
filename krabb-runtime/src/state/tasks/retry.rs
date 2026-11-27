use super::*;
use krabb_core::failure::{Failure, Kind};

const ATTEMPTS: u32 = 3;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Retry {
    pub failure: Failure,
    pub at: u64,
}

impl Task {
    pub fn ready_at(&self) -> u64 {
        self.retry.as_ref().map_or(self.next, |retry| self.next.max(retry.at))
    }
}

impl Store {
    pub(in crate::state) fn migrate_retries(&mut self) -> bool {
        let mut changed = false;

        for task in self.tasks.values_mut().filter(|task| task.failures > 0 && task.retry.is_none())
        {
            task.retry = Some(Retry { failure: Failure::new(Kind::Unknown), at: task.next });

            if !task.observing() && matches!(task.status, TaskStatus::Queued | TaskStatus::Waiting)
            {
                task.status = TaskStatus::Blocked;
                task.revision = task.revision.saturating_add(1);
                task.notice = Some(Notice {
                    id: format!("{}-event-{}", task.id, task.revision),
                    text: "The saved retry needs review after upgrading. Its goal, prior use, and \
                        earliest retry time are preserved."
                        .into(),
                    model: false,
                });
            }

            changed = true;
        }

        changed
    }

    pub fn retry_task(&mut self, id: &str) -> std::io::Result<()> {
        self.retry_task_after(id, Failure::new(Kind::Transient), now())
    }

    pub fn retry_task_after(&mut self, id: &str, failure: Failure, at: u64) -> std::io::Result<()> {
        failure.validate().map_err(std::io::Error::other)?;
        let task = self.task(id)?;

        if task.unsafe_step || task.uncertain {
            return Err(std::io::Error::other("An interrupted mutation cannot be replayed."));
        }

        if task.status != TaskStatus::Running || !failure.kind.retryable() {
            return Err(std::io::Error::other("The task cannot admit this retry."));
        }

        self.change(|store| {
            let task = store.tasks.get_mut(id).unwrap();
            task.failures = task.failures.saturating_add(1);
            let base = 2_u64.saturating_pow(task.failures.min(12)).min(3600);
            let spread =
                task.id.bytes().fold(0_u64, |value, byte| {
                    value.wrapping_mul(31).wrapping_add(u64::from(byte))
                }) % (base / 4 + 1);
            let delay = base
                .saturating_add(spread)
                .min(3600)
                .max(failure.retry_after_seconds.unwrap_or(0))
                .saturating_add(1);
            let deadline = at.saturating_add(delay);
            task.retry = Some(Retry { failure, at: deadline });
            task.next = deadline;
            task.status =
                if task.failures >= ATTEMPTS { TaskStatus::Blocked } else { TaskStatus::Waiting };

            task.updated = at;
            task.revision = task.revision.saturating_add(1);

            if task.failures >= ATTEMPTS {
                task.notice = Some(Notice {
                    id: format!("{id}-event-{}", task.revision),
                    text: format!(
                        "The task stopped retrying after {ATTEMPTS} consecutive failures. Its \
                        goal and partial work are saved. Review its provider or limits before \
                        resuming."
                    ),
                    model: false,
                });
            } else if task.failures == 1 {
                task.notice = Some(Notice {
                    id: format!("{id}-event-{}", task.revision),
                    text: format!(
                        "The request failed. Its saved task will retry no earlier than \
                        {}.",
                        krabb_date::format_datetime(deadline)
                    ),
                    model: false,
                });
            }
        })
    }
}
