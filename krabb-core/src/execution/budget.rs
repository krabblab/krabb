//! Durable admission accounting, independent of task lifetime and timing estimates.

use super::MAX_MILLIS;
use crate::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Reservation {
    pub id: u64,
    pub generation: u64,
    pub millis: u64,
}

/// Host limits on a task's cumulative inference time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// The allowance a task captures on its first reservation.
    pub allowance: Option<u64>,
    /// The most a saved or granted allowance may reach.
    pub ceiling: Option<u64>,
}

impl From<Option<u64>> for Policy {
    fn from(limit: Option<u64>) -> Self {
        Self { allowance: limit, ceiling: limit }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    #[serde(deserialize_with = "required")]
    pub limit_millis: Option<u64>,
    pub spent_millis: u64,
    pub held_millis: u64,
    pub legacy: bool,
    pub sequence: u64,
    #[serde(deserialize_with = "required")]
    pub pending: Option<Reservation>,
    #[serde(default)]
    pub tokens: Tokens,
}

/// Cumulative token use, keeping provider reports separate from host estimates.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Tokens {
    pub reported: u64,
    pub estimated: u64,
}

impl Tokens {
    pub fn used(&self) -> u64 {
        self.reported.saturating_add(self.estimated)
    }

    pub fn remaining(&self, limit: Option<u64>) -> u64 {
        limit.map_or(u64::MAX, |limit| limit.saturating_sub(self.used()))
    }

    /// Charges a generation; any count the provider omitted uses the host estimate.
    pub fn charge(
        &mut self,
        input: Option<u64>,
        output: Option<u64>,
        estimate: (u64, u64),
    ) -> Result<()> {
        let mut reported = 0_u64;
        let mut estimated = 0_u64;

        for (value, fallback) in [(input, estimate.0), (output, estimate.1)] {
            match value {
                Some(value) => reported = reported.saturating_add(value),
                None => estimated = estimated.saturating_add(fallback),
            }
        }

        self.reported = self
            .reported
            .checked_add(reported)
            .ok_or_else(|| Error::Limit("Token accounting is full.".into()))?;

        self.estimated = self
            .estimated
            .checked_add(estimated)
            .ok_or_else(|| Error::Limit("Token accounting is full.".into()))?;

        Ok(())
    }
}

fn full() -> Error {
    Error::Limit("Execution accounting is full.".into())
}

fn required<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    input: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(input)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub id: String,
    pub revision: u64,
    pub sequence: u64,
    pub limit_millis: u64,
}

impl Grant {
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || self.id.len() > crate::task::COMMAND_LIMIT
            || !self.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || self.revision == u64::MAX
            || self.limit_millis == 0
        {
            return Err(Error::Protocol("Invalid task budget grant.".into()));
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GrantReceipt {
    pub grant: Grant,
    pub revision: u64,
}

impl Budget {
    pub fn legacy() -> Self {
        Self { legacy: true, ..Self::default() }
    }

    pub fn validate(&self) -> Result<()> {
        if self.limit_millis == Some(0)
            || self.pending.is_some_and(|pending| {
                pending.id == 0
                    || pending.id != self.sequence
                    || pending.generation == 0
                    || pending.millis == 0
                    || pending.millis > MAX_MILLIS
            })
        {
            return Err(Error::Protocol("Invalid execution budget.".into()));
        }

        self.consumed()?;

        Ok(())
    }

    /// The saved allowance, or the configured one, never above the host ceiling.
    fn limit(&self, policy: Policy) -> Option<u64> {
        match (self.limit_millis.or(policy.allowance), policy.ceiling) {
            (Some(limit), Some(ceiling)) => Some(limit.min(ceiling)),
            (limit, _) => limit,
        }
    }

    fn consumed(&self) -> Result<u64> {
        self.spent_millis
            .checked_add(self.held_millis)
            .and_then(|total| total.checked_add(self.pending.map_or(0, |pending| pending.millis)))
            .ok_or_else(full)
    }

    pub fn remaining(&self, policy: impl Into<Policy>) -> Result<u64> {
        self.validate()?;

        let limit = self.limit(policy.into());

        if self.legacy && limit.is_some() {
            return Err(Error::Limit(
                "Earlier execution usage is unknown. Review it by granting this task a new \
                 allowance, such as \"extend it by 30 minutes\"."
                    .into(),
            ));
        }

        let consumed = self.consumed()?;

        Ok(limit.unwrap_or(u64::MAX).saturating_sub(consumed))
    }

    pub fn reserve(
        &mut self,
        generation: u64,
        millis: u64,
        policy: impl Into<Policy>,
    ) -> Result<Reservation> {
        let policy = policy.into();

        if self.pending.is_some() {
            return Err(Error::Limit("An execution reservation is unresolved.".into()));
        }

        if generation == 0 || millis == 0 || millis > MAX_MILLIS {
            return Err(Error::Protocol("Invalid execution reservation.".into()));
        }

        if millis > self.remaining(policy)? {
            return Err(Error::Limit("The task's execution allowance is exhausted.".into()));
        }

        let id = self.sequence.checked_add(1).ok_or_else(full)?;

        let reservation = Reservation { id, generation, millis };

        self.limit_millis = self.limit_millis.or(policy.allowance);
        self.sequence = id;
        self.pending = Some(reservation);
        Ok(reservation)
    }

    /// Grows the pending reservation for a renewal, never beyond the remaining allowance.
    pub fn extend(
        &mut self,
        reservation: Reservation,
        millis: u64,
        policy: impl Into<Policy>,
    ) -> Result<Reservation> {
        if self.pending != Some(reservation) || millis == 0 {
            return Err(Error::Protocol(
                "The renewal does not match the active reservation.".into(),
            ));
        }

        let total = reservation
            .millis
            .checked_add(millis)
            .filter(|total| *total <= MAX_MILLIS)
            .ok_or_else(|| Error::Limit("The renewal exceeds the execution limit.".into()))?;

        if millis > self.remaining(policy)? {
            return Err(Error::Limit(
                "The task's remaining allowance cannot cover a renewal.".into(),
            ));
        }

        let extended = Reservation { millis: total, ..reservation };

        self.pending = Some(extended);
        Ok(extended)
    }

    pub fn settle(&mut self, reservation: Reservation, used_millis: Option<u64>) -> Result<bool> {
        self.validate()?;

        let Some(pending) = self.pending else {
            if reservation.id <= self.sequence {
                return Ok(false);
            }

            return Err(Error::Protocol("Unknown execution reservation.".into()));
        };

        if reservation.id < pending.id {
            return Ok(false);
        }

        if reservation != pending || used_millis.is_some_and(|used| used > pending.millis) {
            return Err(Error::Protocol(
                "Execution receipt does not match its reservation.".into(),
            ));
        }

        if let Some(used) = used_millis {
            self.spent_millis = self.spent_millis.checked_add(used).ok_or_else(full)?;
        } else {
            self.held_millis = self.held_millis.checked_add(pending.millis).ok_or_else(full)?;
        }

        self.pending = None;
        Ok(true)
    }

    /// Raises a tracked ceiling, captures a first one above known use, or records a
    /// human-reviewed ceiling for legacy use.
    ///
    /// A legacy account has unknown earlier use; an explicit grant becomes its first
    /// known total ceiling and must cover all use the host can still observe.
    pub fn increase(&mut self, limit: u64) -> Result<()> {
        self.validate()?;

        if self.legacy && self.pending.is_none() {
            if limit == 0 || limit <= self.consumed()? {
                return Err(Error::Denied(
                    "A reviewed grant must exceed the task's known execution use.".into(),
                ));
            }

            self.legacy = false;
            self.limit_millis = Some(limit);
            return Ok(());
        }

        if self.legacy || self.pending.is_some() {
            return Err(Error::Denied(
                "Reconcile the task's earlier or active execution before increasing its budget."
                    .into(),
            ));
        }

        let consumed = self.consumed()?;

        let raised = match self.limit_millis {
            Some(previous) => limit > previous && limit >= consumed,
            None => limit > consumed,
        };

        if !raised {
            return Err(Error::Denied(
                "A grant must increase the task's total inference ceiling.".into(),
            ));
        }

        self.limit_millis = Some(limit);
        Ok(())
    }

    pub fn recover(&mut self) -> Result<bool> {
        self.validate()?;

        if let Some(pending) = self.pending { self.settle(pending, None) } else { Ok(false) }
    }
}

#[cfg(test)]
mod tests;
