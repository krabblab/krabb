//! Optional execution metadata and host-admitted inference deadlines.

pub mod budget;
pub mod policy;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const INFO: &str = "model.execution";
pub const MAX_MILLIS: u64 = 86_400_000;
pub const QUEUE_MILLIS: u64 = 300_000;
pub const ENVELOPE: &str = "execution";

/// Finite request allowance, including queue time, advertised to an observing client.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub total_millis: u64,
}

impl Envelope {
    pub fn validate(self) -> Result<()> {
        if self.total_millis == 0 || self.total_millis > MAX_MILLIS + QUEUE_MILLIS {
            return Err(Error::Protocol("Invalid execution envelope.".into()));
        }

        Ok(())
    }
}

/// Reported elapsed phases, not a monetary charge or a substitute for host accounting.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    pub load_millis: Option<u64>,
    pub prefill_millis: Option<u64>,
    pub decode_millis: Option<u64>,
}

impl Timing {
    pub fn validate(self) -> Result<()> {
        let total = [self.load_millis, self.prefill_millis, self.decode_millis]
            .into_iter()
            .flatten()
            .try_fold(0_u64, u64::checked_add);

        if total.is_none_or(|total| total > MAX_MILLIS) {
            return Err(Error::Protocol("Invalid inference phase timing.".into()));
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Deployment {
    #[default]
    Unknown,
    Local,
    Remote,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub deployment: Deployment,
    /// Opaque deployment/model/configuration identity; never a credential or endpoint URL.
    pub identity: String,
    pub streaming: bool,
    /// Whether stopping the request also confirms that provider computation stopped.
    pub cancellation: bool,
    pub usage: bool,
    pub recoverable: bool,
    pub deadlines: bool,
    /// Whether a live generation accepts `generation.renew` up to its admitted ceiling.
    #[serde(default)]
    pub renewal: bool,
}

pub const RENEW: &str = "generation.renew";

/// Raises a live generation's total allowance within its admitted ceiling.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Renewal {
    pub id: u64,
    pub total_millis: u64,
}

impl Profile {
    pub fn validate(&self) -> Result<()> {
        if self.identity.is_empty()
            || self.identity.len() > 128
            || !self
                .identity
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(Error::Protocol("Invalid execution identity.".into()));
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub first_millis: u64,
    pub idle_millis: u64,
    pub total_millis: u64,
    /// The highest total a renewal may reach; sent only to renewal-capable adapters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling_millis: Option<u64>,
}

impl Limits {
    pub fn restrict(self, remaining_millis: u64) -> Result<Self> {
        self.validate()?;

        let total_millis = self.total_millis.min(remaining_millis);

        if total_millis == 0 {
            return Err(Error::Limit(Expiry::Total.to_string()));
        }

        Ok(Self {
            first_millis: self.first_millis.min(total_millis),
            idle_millis: self.idle_millis.min(total_millis),
            total_millis,
            ceiling_millis: self
                .ceiling_millis
                .map(|ceiling| ceiling.min(remaining_millis).max(total_millis)),
        })
    }

    pub fn validate(self) -> Result<()> {
        if self.total_millis == 0
            || self.total_millis > MAX_MILLIS
            || self.first_millis == 0
            || self.first_millis > self.total_millis
            || self.idle_millis == 0
            || self.idle_millis > self.total_millis
            || self
                .ceiling_millis
                .is_some_and(|ceiling| ceiling < self.total_millis || ceiling > MAX_MILLIS)
        {
            return Err(Error::Protocol("Invalid inference deadlines.".into()));
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Expiry {
    #[error("Inference first-output allowance exhausted.")]
    First,
    #[error("Inference inactivity allowance exhausted.")]
    Idle,
    #[error("Inference total allowance exhausted.")]
    Total,
}

/// Uses elapsed active milliseconds supplied by a monotonic caller, not wall-clock time.
#[derive(Clone, Copy, Debug)]
pub struct Timer {
    limits: Limits,
    elapsed: u64,
    output: Option<u64>,
}

impl Timer {
    pub fn new(limits: Limits) -> Result<Self> {
        limits.validate()?;

        Ok(Self { limits, elapsed: 0, output: None })
    }

    pub fn deadline(&self) -> u64 {
        self.output
            .map_or(self.limits.first_millis, |last| last.saturating_add(self.limits.idle_millis))
            .min(self.limits.total_millis)
    }

    /// Raises the total allowance; it never shrinks and never passes the admitted ceiling.
    pub fn renew(&mut self, total_millis: u64) -> Result<()> {
        let ceiling = self.limits.ceiling_millis.ok_or_else(|| {
            Error::Protocol("This generation was not admitted for renewal.".into())
        })?;

        self.limits.total_millis = self.limits.total_millis.max(total_millis.min(ceiling));

        Ok(())
    }

    pub fn total(&self) -> u64 {
        self.limits.total_millis
    }

    /// Only actual model output or changed tool content counts as progress.
    pub fn observe(&mut self, elapsed: u64, progress: bool) -> std::result::Result<(), Expiry> {
        self.elapsed = self.elapsed.max(elapsed);

        if self.elapsed >= self.limits.total_millis {
            return Err(Expiry::Total);
        }

        if self.elapsed >= self.deadline() {
            let expiry = if self.output.is_some() { Expiry::Idle } else { Expiry::First };

            return Err(expiry);
        }

        if progress {
            self.output = Some(self.elapsed);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;
