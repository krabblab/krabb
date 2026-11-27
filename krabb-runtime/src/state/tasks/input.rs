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

        for task in
            self.tasks.values_mut().filter(|task| task.request.is_some() && task.input.is_none())
        {
            task.input = Some(Input::unknown());
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

        let mut tasks = self.tasks.values().filter(|task| {
            task.origin.session == session && task.request.as_deref() == Some(message.id.as_str())
        });

        let Some(task) = tasks.next() else { return Ok(false) };

        if tasks.next().is_some() {
            return Err(std::io::Error::other(ControlError::Identity));
        }

        if task.input.as_ref().is_none_or(|input| input.fingerprint.is_none()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "The original request fingerprint is unknown. Recover its saved task by receipt.",
            ));
        }

        if !task.input.as_ref().unwrap().matches(message)? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "This request identity is already saved with different content.",
            ));
        }

        if self.missing_input(task) {
            let id = task.id.clone();

            if !self.sessions.contains_key(&id) {
                return Err(std::io::Error::other(ControlError::Input));
            }

            let mut original = message.clone();
            original.session = id.clone();
            self.change(|store| {
                store.sessions.get_mut(&id).unwrap().original = Some(original);
                let task = store.tasks.get_mut(&id).unwrap();
                task.revision = task.revision.saturating_add(1);

                if task.notice.as_ref().is_some_and(|notice| notice.text == MISSING) {
                    task.notice = None;
                }
            })?;
        }

        Ok(true)
    }

    pub(super) fn missing_input(&self, task: &Task) -> bool {
        !task.status.terminal()
            && task.status != TaskStatus::Reporting
            && task.input.as_ref().is_some_and(|input| input.fingerprint.is_some())
            && !self
                .sessions
                .get(&task.id)
                .and_then(|session| session.original.as_ref())
                .is_some_and(|message| {
                    message.role == Role::User
                        && Some(message.id.as_str()) == task.request.as_deref()
                        && message.session == task.id
                        && task.input.as_ref().unwrap().matches(message).unwrap_or(false)
                })
    }

    pub(super) fn restore_inputs(&mut self) -> std::io::Result<bool> {
        let mut restored = Vec::new();

        for task in self.tasks.values().filter(|task| self.missing_input(task)) {
            let Some(session) = self.sessions.get(&task.id) else { continue };

            if session.original.is_some() {
                continue;
            }

            let input = task.input.as_ref().unwrap();

            for message in
                session.messages.iter().chain(session.compacted_context.iter().flatten()).chain(
                    self.sessions
                        .get(&task.origin.session)
                        .into_iter()
                        .flat_map(|session| &session.messages),
                )
            {
                if message.role == Role::User
                    && Some(&message.id) == task.request.as_ref()
                    && input.matches(message)?
                {
                    let mut original = message.clone();
                    original.session = task.id.clone();
                    restored.push((task.id.clone(), original));
                    break;
                }
            }
        }

        let changed = !restored.is_empty();

        for (id, original) in restored {
            self.sessions.get_mut(&id).unwrap().original = Some(original);
            let task = self.tasks.get_mut(&id).unwrap();
            task.revision = task.revision.saturating_add(1);

            if task.notice.as_ref().is_some_and(|notice| notice.text == MISSING) {
                task.notice = None;
            }
        }

        Ok(changed)
    }

    pub(super) fn conflicting_input(&self, task: &Task) -> bool {
        task.request.as_ref().is_some_and(|request| {
            self.tasks.values().any(|other| {
                other.id != task.id
                    && other.origin.session == task.origin.session
                    && other.request.as_ref() == Some(request)
                    && (other.status != TaskStatus::Cancelled || other.uncertain)
            })
        })
    }

    pub(super) fn fence_inputs(&mut self) -> bool {
        let conflicts = self
            .tasks
            .values()
            .filter(|task| self.conflicting_input(task) || self.missing_input(task))
            .map(|task| {
                let conflict = self.conflicting_input(task);
                let announce = !conflict
                    || !self.tasks.values().any(|other| {
                        other.id < task.id
                            && other.origin.session == task.origin.session
                            && other.request == task.request
                            && !other.status.terminal()
                            && other.status != TaskStatus::Reporting
                    });

                (task.id.clone(), announce, if conflict { CONFLICT } else { MISSING })
            })
            .collect::<Vec<_>>();
        let mut changed = false;

        for (id, announce, reason) in conflicts {
            let task = self.tasks.get_mut(&id).unwrap();

            if task.status.terminal() || task.status == TaskStatus::Reporting {
                continue;
            }

            let mut modified = false;

            if matches!(task.status, TaskStatus::Queued | TaskStatus::Running | TaskStatus::Waiting)
            {
                task.status = TaskStatus::Blocked;
                modified = true;
            }

            modified |= task.deferred.take().is_some();
            modified |= task.restart;
            task.restart = false;
            let announce = announce
                && task.last_notice != reason
                && task.notice.as_ref().is_none_or(|notice| notice.text != reason);

            if modified || announce {
                task.revision = task.revision.saturating_add(1);

                if announce {
                    task.notice = Some(Notice {
                        id: format!("{}-event-{}", task.id, task.revision),
                        text: reason.into(),
                        model: false,
                    });
                }

                changed = true;
            }
        }

        changed
    }
}
