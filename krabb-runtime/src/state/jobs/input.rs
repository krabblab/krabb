use super::*;
use krabb_core::task::ControlError;
use sha2::Digest;

const CONFLICT: &str = "Several saved tasks share one request identity. Review their results and \
    effects, cancel the duplicates you do not want, then resume the one to keep.";

const MISSING: &str = "The saved task input is missing. Restore it before continuing.";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
    #[serde(deserialize_with = "fingerprint")]
    pub fingerprint: Option<String>,
}

fn fingerprint<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Option<String>, D::Error> {
    Option::deserialize(value)
}

impl Input {
    pub fn unknown() -> Self {
        Self { fingerprint: None }
    }

    pub fn capture(message: &Message) -> std::io::Result<Self> {
        let mut digest = Sha256::new();
        digest.update(b"krabb-input-v1\0");
        digest.update(
            serde_json::to_vec(&(&message.sender, &message.content))
                .map_err(std::io::Error::other)?,
        );
        Ok(Self { fingerprint: Some(crate::hex_digest(digest.finalize())) })
    }

    pub fn valid(&self) -> bool {
        self.fingerprint.as_ref().is_none_or(|value| {
            value.len() == 64
                && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    }

    pub fn matches(&self, message: &Message) -> std::io::Result<bool> {
        Ok(self.fingerprint.is_some() && *self == Self::capture(message)?)
    }
}

impl Store {
    pub(in crate::state) fn migrate_inputs(&mut self) -> bool {
        let mut changed = false;

        for job in self.jobs.values_mut().filter(|job| job.request.is_some() && job.input.is_none())
        {
            job.input = Some(Input::unknown());
            changed = true;
        }

        changed
    }

    pub(in crate::state) fn check_input(
        &mut self,
        session: &str,
        message: &Message,
    ) -> std::io::Result<bool> {
        if message.role != Role::User {
            return Ok(false);
        }

        let mut jobs = self.jobs.values().filter(|job| {
            job.origin.session == session && job.request.as_deref() == Some(message.id.as_str())
        });

        let Some(job) = jobs.next() else { return Ok(false) };

        if jobs.next().is_some() {
            return Err(std::io::Error::other(ControlError::Identity));
        }

        if job.input.as_ref().is_none_or(|input| input.fingerprint.is_none()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "The original request fingerprint is unknown. Recover its saved task by receipt.",
            ));
        }

        if !job.input.as_ref().unwrap().matches(message)? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "This request identity is already saved with different content.",
            ));
        }

        if self.missing_input(job) {
            let id = job.id.clone();

            if !self.sessions.contains_key(&id) {
                return Err(std::io::Error::other(ControlError::Input));
            }

            let mut original = message.clone();
            original.session = id.clone();
            self.change(|store| {
                store.sessions.get_mut(&id).unwrap().original = Some(original);
                let job = store.jobs.get_mut(&id).unwrap();
                job.revision = job.revision.saturating_add(1);

                if job.notice.as_ref().is_some_and(|notice| notice.text == MISSING) {
                    job.notice = None;
                }
            })?;
        }

        Ok(true)
    }

    pub(super) fn missing_input(&self, job: &Job) -> bool {
        !job.status.terminal()
            && job.status != JobStatus::Reporting
            && job.input.as_ref().is_some_and(|input| input.fingerprint.is_some())
            && !self
                .sessions
                .get(&job.id)
                .and_then(|session| session.original.as_ref())
                .is_some_and(|message| {
                    message.role == Role::User
                        && Some(message.id.as_str()) == job.request.as_deref()
                        && message.session == job.id
                        && job.input.as_ref().unwrap().matches(message).unwrap_or(false)
                })
    }

    pub(super) fn restore_inputs(&mut self) -> std::io::Result<bool> {
        let mut restored = Vec::new();

        for job in self.jobs.values().filter(|job| self.missing_input(job)) {
            let Some(session) = self.sessions.get(&job.id) else { continue };

            if session.original.is_some() {
                continue;
            }

            let input = job.input.as_ref().unwrap();

            for message in
                session.messages.iter().chain(session.compacted_context.iter().flatten()).chain(
                    self.sessions
                        .get(&job.origin.session)
                        .into_iter()
                        .flat_map(|session| &session.messages),
                )
            {
                if message.role == Role::User
                    && Some(&message.id) == job.request.as_ref()
                    && input.matches(message)?
                {
                    let mut original = message.clone();
                    original.session = job.id.clone();
                    restored.push((job.id.clone(), original));
                    break;
                }
            }
        }

        let changed = !restored.is_empty();

        for (id, original) in restored {
            self.sessions.get_mut(&id).unwrap().original = Some(original);
            let job = self.jobs.get_mut(&id).unwrap();
            job.revision = job.revision.saturating_add(1);

            if job.notice.as_ref().is_some_and(|notice| notice.text == MISSING) {
                job.notice = None;
            }
        }

        Ok(changed)
    }

    pub(super) fn conflicting_input(&self, job: &Job) -> bool {
        job.request.as_ref().is_some_and(|request| {
            self.jobs.values().any(|other| {
                other.id != job.id
                    && other.origin.session == job.origin.session
                    && other.request.as_ref() == Some(request)
                    && (other.status != JobStatus::Cancelled || other.uncertain)
            })
        })
    }

    pub(super) fn fence_inputs(&mut self) -> bool {
        let conflicts = self
            .jobs
            .values()
            .filter(|job| self.conflicting_input(job) || self.missing_input(job))
            .map(|job| {
                let conflict = self.conflicting_input(job);
                let announce = !conflict
                    || !self.jobs.values().any(|other| {
                        other.id < job.id
                            && other.origin.session == job.origin.session
                            && other.request == job.request
                            && !other.status.terminal()
                            && other.status != JobStatus::Reporting
                    });

                (job.id.clone(), announce, if conflict { CONFLICT } else { MISSING })
            })
            .collect::<Vec<_>>();
        let mut changed = false;

        for (id, announce, reason) in conflicts {
            let job = self.jobs.get_mut(&id).unwrap();

            if job.status.terminal() || job.status == JobStatus::Reporting {
                continue;
            }

            let mut modified = false;

            if matches!(job.status, JobStatus::Queued | JobStatus::Running | JobStatus::Waiting) {
                job.status = JobStatus::Blocked;
                modified = true;
            }

            modified |= job.deferred.take().is_some();
            modified |= job.restart;
            job.restart = false;
            let announce = announce
                && job.last_notice != reason
                && job.notice.as_ref().is_none_or(|notice| notice.text != reason);

            if modified || announce {
                job.revision = job.revision.saturating_add(1);

                if announce {
                    job.notice = Some(Notice {
                        id: format!("{}-event-{}", job.id, job.revision),
                        text: reason.into(),
                    });
                }

                changed = true;
            }
        }

        changed
    }
}
