#[cfg(test)]
mod tests;

use super::*;
use krabb_core::execution::budget::Policy;

#[derive(Debug)]
pub(crate) struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

use krabb_core::execution::{
    self,
    policy::{self, Decision, Outcome, Series, Work},
};

const PROFILES: usize = 64;
const BYTES: u64 = 1024 * 1024;
const VERSION: u64 = 2;
/// Planned output tokens when the request and model window set no lower bound.
const FAST_OUTPUT: u64 = 512;
/// The default most `/task extend` may raise one task's inference allowance to: one day.
const TASK_CEILING_SECONDS: u64 = 86_400;
const OUTPUT: u64 = 1_024;
pub(crate) const WINDOW_OUTPUT: u64 = 2_048;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Config {
    pub max_seconds: u64,
    pub queue_seconds: u64,
    pub task_seconds: Option<u64>,
    pub task_ceiling_seconds: u64,
    pub task_tokens: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_seconds: 3_600,
            queue_seconds: 60,
            task_seconds: None,
            task_ceiling_seconds: TASK_CEILING_SECONDS,
            task_tokens: None,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=execution::MAX_MILLIS / 1_000).contains(&self.max_seconds)
            || !(1..=execution::QUEUE_MILLIS / 1_000).contains(&self.queue_seconds)
        {
            return Err(format!(
                "Inference limits require 1–{} active seconds and 1–{} queue seconds.",
                execution::MAX_MILLIS / 1_000,
                execution::QUEUE_MILLIS / 1_000
            ));
        }

        if self.task_tokens == Some(0) {
            return Err("Task token allowance must be a positive number of tokens.".into());
        }

        if self.task_seconds.is_some_and(|seconds| seconds == 0 || seconds > u64::MAX / 1_000) {
            return Err(
                "Task inference allowance must be a positive, representable number of seconds."
                    .into(),
            );
        }

        if !(1..=u64::MAX / 1_000).contains(&self.task_ceiling_seconds)
            || self.task_seconds.is_some_and(|seconds| seconds > self.task_ceiling_seconds)
        {
            return Err(
                "Task inference ceiling must be a positive number of seconds, at least the task \
                 allowance."
                    .into(),
            );
        }

        Ok(())
    }

    pub fn task_millis(&self) -> Option<u64> {
        self.task_seconds.map(|seconds| seconds.saturating_mul(1_000))
    }

    pub fn task_ceiling_millis(&self) -> u64 {
        self.task_ceiling_seconds.saturating_mul(1_000)
    }

    pub fn task_policy(&self) -> Policy {
        Policy { allowance: self.task_millis(), ceiling: Some(self.task_ceiling_millis()) }
    }

    pub fn envelope(&self) -> Duration {
        Duration::from_secs(self.max_seconds.saturating_add(self.queue_seconds))
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u64,
    entries: BTreeMap<String, Series>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self { version: VERSION, entries: BTreeMap::new() }
    }
}

pub(crate) struct History {
    data: Mutex<Snapshot>,
    writer: Option<tokio::sync::mpsc::Sender<()>>,
}

impl Default for History {
    fn default() -> Self {
        Self { data: Mutex::new(Snapshot::default()), writer: None }
    }
}

impl History {
    pub fn open(root: &Path) -> Arc<Self> {
        let path = match krabb_file::directory(root, Path::new("data")) {
            Ok(path) => path.join("inference.json"),
            Err(_) => return Arc::new(Self::default()),
        };

        let data = (|| -> Result<Snapshot, PluginError> {
            let Some(bytes) = krabb_file::load(&path, BYTES)? else {
                return Ok(Snapshot::default());
            };

            krabb_file::private(&path)?;

            let mut data: Snapshot = serde_json::from_slice(&bytes)?;

            if !matches!(data.version, 1 | 2)
                || data.entries.len() > PROFILES
                || data
                    .entries
                    .keys()
                    .any(|key| key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()))
            {
                return Err("Invalid inference timing cache.".into());
            }

            for series in data.entries.values() {
                series.validate()?;
            }

            data.version = VERSION;
            Ok(data)
        })();

        let data = data.unwrap_or_else(|_| {
            warn!("Inference timing cache is unavailable; using conservative priors.");
            Snapshot::default()
        });

        let (writer, mut receiver) = tokio::sync::mpsc::channel(1);
        let history = Arc::new(Self { data: Mutex::new(data), writer: Some(writer) });
        let weak = Arc::downgrade(&history);
        let root = root.to_owned();

        tokio::spawn(async move {
            let mut failed = false;

            while receiver.recv().await.is_some() {
                let Some(history) = weak.upgrade() else { break };

                let snapshot = history.data.lock().ok().map(|data| data.clone());

                drop(history);

                let Some(snapshot) = snapshot else { continue };

                let root = root.clone();
                let path = path.clone();
                let saved = tokio::task::spawn_blocking(move || -> Result<(), PluginError> {
                    krabb_file::directory(&root, Path::new("data"))?;
                    let bytes = serde_json::to_vec(&snapshot)?;

                    if bytes.len() as u64 > BYTES {
                        return Err("Timing cache is too large.".into());
                    }

                    krabb_file::save(&path, bytes)?;
                    Ok(())
                })
                .await;

                let failure = !matches!(saved, Ok(Ok(())));

                if failure && !failed {
                    warn!("Inference timing cache could not be saved.");
                }

                failed = failure;
            }
        });

        history
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &self,
        provider: &str,
        model: &str,
        profile: &krabb_core::context::Profile,
        cold: bool,
        kind: tasks::Kind,
        request: &mut Request,
        available_millis: u64,
    ) -> Result<Option<execution::Limits>, PluginError> {
        if !profile.execution.as_ref().is_some_and(|profile| profile.deadlines) {
            return Ok(None);
        }

        let Request::Call { params, .. } = request else {
            return Err("Inference call required.".into());
        };

        let mut input: ModelRequest = serde_json::from_value(params.clone())?;
        let requested = input.generation;
        let bound = profile
            .window
            .map_or(WINDOW_OUTPUT, |window| window.output)
            .min(requested.map_or(u64::MAX, |generation| generation.output));

        if bound == 0 {
            return Err(
                krabb_core::Error::Protocol("Invalid generation output allowance.".into()).into()
            );
        }

        let explicit = requested.and_then(|generation| generation.limits);

        if let Some(limits) = explicit {
            limits.validate()?;
        }

        let available_millis =
            available_millis.min(explicit.map_or(u64::MAX, |limits| limits.total_millis));
        let output = bound.min(if kind == tasks::Kind::Fast { FAST_OUTPUT } else { OUTPUT });

        let mut planned = output;
        let tokens = krabb_core::context::estimate(&input.messages, &input.tools);

        if let Some(window) = profile.window {
            let capacity = window.input()?;

            if tokens > capacity {
                let batches = tokens.div_ceil((capacity / 2).max(1)).min(16);
                planned =
                    planned.saturating_add(batches.saturating_mul((capacity / 4).clamp(1, 512)));
            }
        }

        let streaming =
            input.stream && profile.execution.as_ref().is_some_and(|profile| profile.streaming);
        let mut limits = self
            .plan(
                provider,
                model,
                profile,
                Work { input: tokens, output: planned, cold, streaming },
                available_millis,
            )?
            .ok_or("Inference deadlines were not negotiated.")?;

        if streaming
            && explicit.is_none()
            && profile.execution.as_ref().is_some_and(|profile| profile.renewal)
            && available_millis > limits.total_millis
        {
            limits.ceiling_millis = Some(available_millis);
        }

        if explicit.is_some_and(|explicit| {
            limits.first_millis > explicit.first_millis || limits.idle_millis > explicit.idle_millis
        }) {
            return Err(krabb_core::Error::Limit(
                "Predicted inference exceeds the requested timing limits.".into(),
            )
            .into());
        }

        let summary = requested.is_some_and(|generation| generation.summary);
        input.stream = streaming;
        input.generation = Some(krabb_core::context::Generation {
            output: bound,
            limits: Some(limits),
            summary,
            search: !summary
                && requested
                    .map_or(profile.search, |generation| generation.search && profile.search),
        });

        *params = serde_json::to_value(input)?;
        Ok(Some(limits))
    }

    fn key(provider: &str, model: &str, profile: &krabb_core::context::Profile) -> Option<String> {
        let execution = profile.execution.as_ref()?;
        let data =
            serde_json::to_vec(&(provider, model, &execution.identity, profile.window)).ok()?;
        Some(crate::digest::sha256(data))
    }

    pub fn plan(
        &self,
        provider: &str,
        model: &str,
        profile: &krabb_core::context::Profile,
        work: Work,
        available_millis: u64,
    ) -> Result<Option<execution::Limits>, PluginError> {
        let Some(execution) = profile.execution.as_ref().filter(|profile| profile.deadlines) else {
            return Ok(None);
        };

        let key = Self::key(provider, model, profile).ok_or("Inference identity is missing.")?;
        let data = self.data.lock().map_err(|_| "Inference history lock is poisoned.")?;
        let empty = Series::default();
        let series = data.entries.get(&key).unwrap_or(&empty);

        match policy::predict(execution.deployment, work, series, unix_seconds(), available_millis)?
        {
            Decision::Admit(estimate) => Ok(Some(estimate.limits)),

            Decision::NeedsBudget { required_millis, available_millis } => {
                Err(krabb_core::Error::Limit(format!(
                    "The predicted inference requires {} seconds; {} seconds remain. Authorize a \
                     larger allowance or split the work.",
                    required_millis.div_ceil(1_000),
                    available_millis / 1_000
                ))
                .into())
            }
        }
    }

    pub fn trace(
        self: &Arc<Self>,
        provider: &str,
        model: &str,
        profile: &krabb_core::context::Profile,
    ) -> Trace {
        Trace {
            history: Arc::clone(self),
            key: Self::key(provider, model, profile),
            started: tokio::time::Instant::now(),
            outcome: Outcome::Interrupted,
        }
    }

    #[cfg(test)]
    pub(crate) fn samples(&self) -> Vec<policy::Sample> {
        self.data
            .lock()
            .unwrap()
            .entries
            .values()
            .flat_map(|series| series.samples().iter().copied())
            .collect()
    }

    #[cfg(test)]
    pub fn observe(
        &self,
        provider: &str,
        model: &str,
        profile: &krabb_core::context::Profile,
        reply: &ModelReply,
        elapsed: Duration,
    ) {
        let Some(key) = Self::key(provider, model, profile) else { return };

        let sample = policy::Sample {
            outcome: Outcome::Complete,
            at: unix_seconds(),
            input: reply.input,
            output: reply.output,
            elapsed_millis: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
            timing: reply.timing,
        };

        self.record(key, sample);
    }

    fn record(&self, key: String, sample: policy::Sample) {
        if sample.validate().is_err() {
            return;
        }

        let Ok(mut data) = self.data.lock() else { return };

        if !data.entries.contains_key(&key)
            && data.entries.len() >= PROFILES
            && let Some(oldest) = data
                .entries
                .iter()
                .min_by_key(|(_, series)| series.last())
                .map(|(key, _)| key.clone())
        {
            data.entries.remove(&oldest);
        }

        if data.entries.entry(key).or_default().record(sample).is_err() {
            return;
        }

        drop(data);

        if let Some(writer) = &self.writer {
            let _ = writer.try_send(());
        }
    }
}

/// Tracks an admitted attempt, including a future dropped before a reply arrives.
pub(crate) struct Trace {
    history: Arc<History>,
    key: Option<String>,
    started: tokio::time::Instant,
    outcome: Outcome,
}

impl Trace {
    pub fn outcome(&mut self, outcome: Outcome) {
        self.outcome = outcome;
    }

    pub fn complete(&mut self, reply: &ModelReply) {
        let Some(key) = self.key.take() else { return };

        let mut sample = self.sample(Outcome::Complete);
        sample.input = reply.input;
        sample.output = reply.output;
        sample.timing = reply.timing;

        if sample.validate().is_err() {
            // A valid answer with unusable metrics is still a completion, not zero-cost work.
            sample.input = None;
            sample.output = None;
            sample.timing = None;
        }

        self.history.record(key, sample);
    }

    fn sample(&self, outcome: Outcome) -> policy::Sample {
        policy::Sample {
            outcome,
            at: unix_seconds(),
            input: None,
            output: None,
            elapsed_millis: self
                .started
                .elapsed()
                .as_millis()
                .min(u128::from(execution::MAX_MILLIS)) as u64,
            timing: None,
        }
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.history.record(key, self.sample(self.outcome));
        }
    }
}
