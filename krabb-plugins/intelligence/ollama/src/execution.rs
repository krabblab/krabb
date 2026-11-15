use krabb_core::{
    Error, Result,
    execution::{Deployment, Limits, Profile, Timer},
    types::ModelRequest,
};

use sha2::{Digest, Sha256};
use tokio::time::{Duration, Instant};

pub(super) fn timing(value: &serde_json::Value) -> Result<Option<krabb_core::execution::Timing>> {
    fn millis(value: Option<&serde_json::Value>) -> Result<Option<u64>> {
        value
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_u64()
                    .map(|value| value.div_ceil(1_000_000))
                    .ok_or_else(|| Error::Protocol("Invalid Ollama phase duration.".into()))
            })
            .transpose()
    }

    let timing = krabb_core::execution::Timing {
        load_millis: millis(value.get("load_duration"))?,
        prefill_millis: if value["prompt_eval_cached_count"].as_u64().is_some_and(|count| count > 0)
        {
            // Cached prompt tokens do not measure the throughput of fresh prefill.
            None
        } else {
            millis(value.get("prompt_eval_duration"))?
        },
        decode_millis: millis(value.get("eval_duration"))?,
    };

    timing.validate()?;
    Ok((timing != krabb_core::execution::Timing::default()).then_some(timing))
}

pub(super) fn profile(
    host: &str,
    model: &str,
    deployment: &str,
    revision: &str,
    thinking: &str,
    context: &str,
) -> Result<Profile> {
    let deployment = match deployment {
        "" | "unknown" => Deployment::Unknown,
        "local" => Deployment::Local,
        "remote" => Deployment::Remote,
        _ => return Err(Error::Protocol("Invalid Ollama deployment hint.".into())),
    };

    let identity = serde_json::to_vec(&(host, model, deployment, revision, thinking, context))?;

    Ok(Profile {
        deployment,
        identity: Sha256::digest(identity).iter().map(|byte| format!("{byte:02x}")).collect(),
        streaming: true,
        cancellation: false,
        usage: true,
        recoverable: false,
        deadlines: true,
        renewal: true,
    })
}

pub(super) fn limits(input: &ModelRequest) -> Result<Option<Limits>> {
    let limits = input.generation.and_then(|generation| generation.limits);

    if let Some(limits) = limits {
        limits.validate()?;
    }

    Ok(limits)
}

pub(super) fn request(
    post: reqwest::RequestBuilder,
    limits: Option<Limits>,
) -> reqwest::RequestBuilder {
    if let Some(limits) = limits {
        post.timeout(Duration::from_millis(limits.ceiling_millis.unwrap_or(limits.total_millis)))
    } else {
        post
    }
}

type Renewals = std::sync::Mutex<std::collections::BTreeMap<u64, tokio::sync::watch::Sender<u64>>>;

static RENEWALS: std::sync::LazyLock<Renewals> = std::sync::LazyLock::new(Default::default);

/// Delivers a host renewal to the live generation it names; unknown IDs are ignored.
pub(super) fn renew(params: &serde_json::Value) -> Result<()> {
    let renewal: krabb_core::execution::Renewal = serde_json::from_value(params.clone())?;
    let renewals =
        RENEWALS.lock().map_err(|_| Error::Protocol("Renewal lock is poisoned.".into()))?;

    if let Some(sender) = renewals.get(&renewal.id) {
        sender.send_replace(renewal.total_millis);
    }

    Ok(())
}

pub(super) struct Watch {
    started: Instant,
    timer: Timer,
    renewal: Option<(u64, tokio::sync::watch::Receiver<u64>)>,
}

impl Watch {
    pub fn new(id: u64, limits: Limits) -> Result<Self> {
        let renewal = limits.ceiling_millis.map(|_| {
            let (sender, receiver) = tokio::sync::watch::channel(limits.total_millis);

            if let Ok(mut renewals) = RENEWALS.lock() {
                renewals.insert(id, sender);
            }

            (id, receiver)
        });

        Ok(Self { started: Instant::now(), timer: Timer::new(limits)?, renewal })
    }

    pub fn deadline(&self) -> Instant {
        self.started + Duration::from_millis(self.timer.deadline())
    }

    pub fn observe(&mut self, progress: bool) -> Result<()> {
        if let Some((_, receiver)) = &mut self.renewal
            && receiver.has_changed().unwrap_or(false)
        {
            let total = *receiver.borrow_and_update();

            if total > self.timer.total() {
                self.timer.renew(total)?;
            }
        }

        self.timer
            .observe(self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64, progress)
            .map_err(|error| super::failure::timeout(error.to_string()))
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some((id, receiver)) = &self.renewal
            && let Ok(mut renewals) = RENEWALS.lock()
            && renewals.get(id).is_some_and(|sender| sender.subscribe().same_channel(receiver))
        {
            renewals.remove(id);
        }
    }
}

#[cfg(test)]
mod tests;
