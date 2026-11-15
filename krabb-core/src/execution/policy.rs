//! Deterministic predictions from bounded, prompt-free observations.

use super::{Deployment, Limits, MAX_MILLIS, Timing};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

pub const SAMPLES: usize = 32;
pub const AGE_SECONDS: u64 = 7 * 86_400;
const MIN_SAMPLES: usize = 8;

/// Conservative per-deployment assumptions used until enough recent samples exist.
struct Prior {
    cold_millis: u64,
    warm_millis: u64,
    prefill_micros: u64,
    decode_micros: u64,
    idle_millis: u64,
    unmeasured_idle_millis: u64,
}

const REMOTE: Prior = Prior {
    cold_millis: 5_000,
    warm_millis: 2_000,
    prefill_micros: 2_000,
    decode_micros: 50_000,
    idle_millis: 15_000,
    unmeasured_idle_millis: 15_000,
};

const LOCAL: Prior = Prior {
    cold_millis: 120_000,
    warm_millis: 30_000,
    prefill_micros: 100_000,
    decode_micros: 1_000_000,
    idle_millis: 30_000,
    unmeasured_idle_millis: 120_000,
};

/// Local attempt outcome; cancellation does not confirm that remote computation stopped.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    #[default]
    Complete,
    Failed,
    TimedOut,
    Cancelled,
    Interrupted,
    Yielded,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    #[serde(default)]
    pub outcome: Outcome,
    pub at: u64,
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub elapsed_millis: u64,
    pub timing: Option<Timing>,
}

impl Sample {
    pub fn validate(self) -> Result<()> {
        if self.outcome != Outcome::Complete
            && (self.input.is_some() || self.output.is_some() || self.timing.is_some())
        {
            return Err(Error::Protocol(
                "Censored observations cannot invent usage or phase timings.".into(),
            ));
        }

        if self.elapsed_millis > MAX_MILLIS
            || self.input.is_some_and(|count| count > u64::from(u32::MAX))
            || self.output.is_some_and(|count| count > u64::from(u32::MAX))
        {
            return Err(Error::Protocol("Invalid inference observation.".into()));
        }

        if let Some(timing) = self.timing {
            timing.validate()?;
            let reported = [timing.load_millis, timing.prefill_millis, timing.decode_millis]
                .into_iter()
                .flatten()
                .sum::<u64>();

            if reported > self.elapsed_millis.saturating_add(3) {
                return Err(Error::Protocol("Inference phase timings exceed elapsed time.".into()));
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Series {
    samples: Vec<Sample>,
}

impl Series {
    pub fn validate(&self) -> Result<()> {
        if self.samples.len() > SAMPLES {
            return Err(Error::Protocol("Inference observation capacity exceeded.".into()));
        }

        for sample in &self.samples {
            sample.validate()?;
        }

        Ok(())
    }

    pub fn record(&mut self, sample: Sample) -> Result<()> {
        self.validate()?;

        sample.validate()?;
        self.samples.retain(|old| recent(old.at, sample.at));
        self.samples.push(sample);

        if self.samples.len() > SAMPLES {
            self.samples.remove(0);
        }

        Ok(())
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    pub fn last(&self) -> Option<u64> {
        self.samples.last().map(|sample| sample.at)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Work {
    pub input: u64,
    pub output: u64,
    pub cold: bool,
    pub streaming: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    Prior,
    Measured,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Estimate {
    pub limits: Limits,
    pub confidence: Confidence,
    pub samples: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Admit(Estimate),
    NeedsBudget { required_millis: u64, available_millis: u64 },
}

fn recent(at: u64, now: u64) -> bool {
    at <= now && now - at <= AGE_SECONDS
}

fn quantile(mut values: Vec<u64>) -> Option<u64> {
    if values.is_empty() {
        return None;
    }

    values.sort_unstable();
    Some(values[(values.len() * 9).div_ceil(10).saturating_sub(1)])
}

fn rate(samples: &[&Sample], prefill: bool) -> Option<u64> {
    let mut tokens = 0_u64;
    let mut values = Vec::new();

    for sample in samples {
        let Some(timing) = sample.timing else { continue };

        let (count, millis) = if prefill {
            (sample.input, timing.prefill_millis)
        } else {
            (sample.output, timing.decode_millis)
        };

        if let (Some(count), Some(millis)) = (count, millis)
            && count > 0
            && millis > 0
        {
            tokens = tokens.saturating_add(count);
            values.push(millis.saturating_mul(1_000).div_ceil(count));
        }
    }

    let minimum = if prefill { 512 } else { 128 };

    if values.len() < MIN_SAMPLES || tokens < minimum {
        return None;
    }

    quantile(values)
}

fn margin(millis: u64) -> u64 {
    millis.saturating_add(millis.div_ceil(2)).saturating_add(2_000)
}

pub fn predict(
    deployment: Deployment,
    work: Work,
    series: &Series,
    now: u64,
    available_millis: u64,
) -> Result<Decision> {
    series.validate()?;

    if work.input > u64::from(u32::MAX)
        || work.output == 0
        || work.output > u64::from(u32::MAX)
        || available_millis > MAX_MILLIS
    {
        return Err(Error::Protocol("Invalid inference admission inputs.".into()));
    }

    let samples =
        series.samples().iter().filter(|sample| recent(sample.at, now)).collect::<Vec<_>>();

    let prior = if deployment == Deployment::Remote { REMOTE } else { LOCAL };

    let recovering = samples
        .iter()
        .rposition(|sample| matches!(sample.outcome, Outcome::Failed | Outcome::TimedOut))
        .is_some_and(|last| {
            rate(&samples[last + 1..], true).is_none()
                || rate(&samples[last + 1..], false).is_none()
        });

    let complete = samples
        .iter()
        .copied()
        .filter(|sample| sample.outcome == Outcome::Complete)
        .collect::<Vec<_>>();

    let prefill = rate(&complete, true);
    let decode = rate(&complete, false);

    let confidence = if !recovering && prefill.is_some() && decode.is_some() {
        Confidence::Measured
    } else {
        Confidence::Prior
    };

    let load = quantile(samples.iter().filter_map(|sample| sample.timing?.load_millis).collect())
        .unwrap_or(0)
        .max(if work.cold { prior.cold_millis } else { prior.warm_millis });

    let rate_or = |measured: Option<u64>, prior: u64| {
        let rate = measured.unwrap_or(prior);

        if recovering { rate.max(prior) } else { rate }
    };

    let prefill_micros = rate_or(prefill, prior.prefill_micros);
    let decode_micros = rate_or(decode, prior.decode_micros);

    let ready = load.saturating_add(work.input.saturating_mul(prefill_micros).div_ceil(1_000));
    let first = margin(ready.saturating_add(decode_micros.div_ceil(1_000)));

    let total =
        margin(ready.saturating_add(work.output.saturating_mul(decode_micros).div_ceil(1_000)));

    if total > available_millis {
        return Ok(Decision::NeedsBudget { required_millis: total, available_millis });
    }

    let idle_floor = if confidence == Confidence::Prior {
        prior.unmeasured_idle_millis
    } else {
        prior.idle_millis
    };

    let idle = margin(decode_micros.saturating_mul(16).div_ceil(1_000)).max(idle_floor).min(total);

    let limits = Limits {
        first_millis: if work.streaming { first.min(total) } else { total },
        idle_millis: if work.streaming { idle } else { total },
        total_millis: total,
        ceiling_millis: None,
    };

    limits.validate()?;
    Ok(Decision::Admit(Estimate { limits, confidence, samples: samples.len() }))
}

#[cfg(test)]
mod tests;
