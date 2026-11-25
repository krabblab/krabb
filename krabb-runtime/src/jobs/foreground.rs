use super::*;
use krabb_core::task::Run;

pub(crate) struct Foreground {
    pub run: Run,
    signals: Arc<Signals>,
    cancel: Arc<Cancellation>,
}

impl Foreground {
    pub fn admit(
        store: &mut state::Store,
        signals: Arc<Signals>,
        context: &Value,
        history: Vec<Message>,
        cancel: &Cancellation,
    ) -> Result<Self, PluginError> {
        let instruction = history
            .iter()
            .rev()
            .find(|message| message.role == Role::User)
            .map(krabb_core::reply::unquoted)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| {
                "Answer the user's current request using the saved conversation.".into()
            });

        let instruction = goal(instruction);
        let task = routing::Route { kind: routing::Kind::Fast, message: String::new(), instruction };
        let job = record(&task, context)?;
        let id = job.id.clone();
        let cancel = Arc::new(cancel.clone());
        let guard = Self { run: Run { id: id.clone(), generation: 1 }, signals, cancel };

        guard
            .signals
            .attached
            .lock()
            .map_err(|_| "Task attachment lock is poisoned.")?
            .insert(id.clone());

        guard
            .signals
            .cancels
            .lock()
            .map_err(|_| "Task cancellation lock is poisoned.")?
            .insert(id, Arc::clone(&guard.cancel));

        store.admit_foreground(job, history)?;
        Ok(guard)
    }

    pub fn settle(
        &self,
        store: &mut state::Store,
        result: &Result<ModelReply, PluginError>,
        interrupted: bool,
    ) -> Result<(), PluginError> {
        let job = store.job(&self.run.id)?;

        if job.step != self.run.generation {
            return Err("The task worker was superseded.".into());
        }

        if job.status.stopping() {
            store.finish_job_control(&self.run.id, self.run.generation)?;
        }

        let job = &store.jobs[&self.run.id];

        if job.status != JobStatus::Running {
            return Ok(());
        }

        match result {
            Ok(reply) => store.foreground_result(&self.run, &reply.text)?,

            Err(_) if job.unsafe_step => {
                let checkpoint = job.checkpoint.clone();

                store.uncertain_job(&self.run.id)?;
                store.checkpoint_job(
                    &self.run.id,
                    &checkpoint,
                    JobStatus::Blocked,
                    unix_seconds(),
                    Some(
                        "An action may have completed before this request stopped. Review its \
                         effects before resuming.",
                    ),
                )?;
            }

            Err(_) if interrupted => store.recover_job(&self.run.id)?,
            Err(error) => crate::failure::settle(store, &self.run.id, error.as_ref())?,
        }

        Ok(())
    }
}

impl Drop for Foreground {
    fn drop(&mut self) {
        if let Ok(mut cancels) = self.signals.cancels.lock()
            && cancels.get(&self.run.id).is_some_and(|token| Arc::ptr_eq(token, &self.cancel))
        {
            cancels.remove(&self.run.id);
        }

        if let Ok(mut attached) = self.signals.attached.lock() {
            attached.remove(&self.run.id);
        }

        self.signals.wake.notify_one();
    }
}

pub(crate) struct Answer {
    pub model: ModelReply,
    pub foreground: Option<Foreground>,
    pub restart: bool,
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Answer")
            .field("model", &self.model)
            .field("task", &self.foreground.as_ref().map(|task| &task.run.id))
            .finish()
    }
}

impl std::ops::Deref for Answer {
    type Target = ModelReply;

    fn deref(&self) -> &Self::Target {
        &self.model
    }
}

impl Serialize for Answer {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.model.serialize(serializer)
    }
}

#[derive(Debug)]
pub(crate) struct Failure {
    pub id: String,
    pub state: JobStatus,
    source: PluginError,
}

impl Failure {
    pub fn new(id: String, state: JobStatus, source: PluginError) -> Self {
        Self { id, state, source }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Task {} is {:?}: {}", self.id, self.state, self.source)
    }
}

impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
