use super::*;

use krabb_core::{
    execution::budget::{Policy, Reservation},
    task::Run,
};

use tokio::time::Instant;

#[derive(Debug)]
pub(crate) struct Blocked(pub String);

impl std::fmt::Display for Blocked {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Blocked {}

fn failure(error: std::io::Error) -> PluginError {
    if error.get_ref().is_some_and(|cause| cause.is::<state::TokensExhausted>()) {
        return Box::new(Blocked(error.to_string()));
    }

    if let Some(krabb_core::Error::Limit(detail)) =
        error.get_ref().and_then(|error| error.downcast_ref::<krabb_core::Error>())
    {
        Box::new(Blocked(detail.clone()))
    } else {
        error.into()
    }
}

pub(crate) fn remaining(
    store: &Arc<Mutex<state::Store>>,
    run: Option<&Run>,
    policy: Policy,
) -> Result<u64, PluginError> {
    match run {
        Some(run) => store
            .lock()
            .map_err(|_| crate::state::POISONED)?
            .budget_remaining(run, policy)
            .map_err(failure),

        None => Ok(policy.allowance.unwrap_or(u64::MAX)),
    }
}

pub(crate) struct Account {
    store: Arc<Mutex<state::Store>>,
    id: String,
    reservation: Option<Reservation>,
    pub clock: Clock,
}

impl Account {
    pub fn reserve(
        store: &Arc<Mutex<state::Store>>,
        run: Option<&Run>,
        millis: u64,
        policy: impl Into<Policy>,
        tokens: (u64, Option<u64>),
    ) -> Result<Option<Self>, PluginError> {
        let policy = policy.into();

        let Some(run) = run else { return Ok(None) };

        let reservation = store
            .lock()
            .map_err(|_| crate::state::POISONED)?
            .reserve_step(run, millis, policy, tokens)
            .map_err(failure)?;

        Ok(Some(Self {
            store: Arc::clone(store),
            id: run.id.clone(),
            reservation: Some(reservation),
            clock: Clock::default(),
        }))
    }

    /// Persists a renewal before any enforcing layer is told about it.
    pub fn extend(&mut self, millis: u64, policy: Policy) -> Result<(), PluginError> {
        let reservation = self.reservation.ok_or("The execution reservation is settled.")?;

        self.reservation =
            Some(self.store.lock().map_err(|_| crate::state::POISONED)?.extend_budget(
                &self.id,
                reservation,
                millis,
                policy,
            )?);
        Ok(())
    }

    pub fn complete(&mut self) -> Result<(), PluginError> {
        let Some(reservation) = self.reservation else { return Ok(()) };

        let used = self.clock.elapsed().map(|elapsed| elapsed.min(reservation.millis));

        self.store.lock().map_err(|_| crate::state::POISONED)?.settle_budget(
            &self.id,
            reservation,
            used,
        )?;
        self.reservation = None;
        Ok(())
    }
}

/// An abandoned step, such as a cancelled one, may still be generating, so its whole
/// reservation stays held for review instead of being settled at the time measured so far.
impl Drop for Account {
    fn drop(&mut self) {
        let Some(reservation) = self.reservation.take() else { return };

        let result = self
            .store
            .lock()
            .map_err(|_| std::io::Error::other(crate::state::POISONED))
            .and_then(|mut store| store.settle_budget(&self.id, reservation, None));

        if result.is_err() {
            warn!("Execution settlement remains pending; the saved reservation is retained.");
        }
    }
}

#[derive(Clone)]
pub(crate) struct Clock(Arc<Mutex<Meter>>);

struct Meter {
    active: Duration,
    since: Option<Instant>,
    waits: usize,
}

impl Default for Clock {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Meter {
            active: Duration::ZERO,
            since: Some(Instant::now()),
            waits: 0,
        })))
    }
}

impl Clock {
    pub fn wait(&self) -> Wait {
        if let Ok(mut meter) = self.0.lock() {
            if let Some(since) = meter.since.take() {
                meter.active = meter.active.saturating_add(since.elapsed());
            }

            meter.waits = meter.waits.saturating_add(1);
        }

        Wait(self.clone())
    }

    fn elapsed(&self) -> Option<u64> {
        let meter = self.0.lock().ok()?;

        if meter.waits != 0 {
            return None;
        }

        let duration = meter
            .active
            .saturating_add(meter.since.map_or(Duration::ZERO, |since| since.elapsed()));

        Some(duration.as_nanos().div_ceil(1_000_000).min(u128::from(u64::MAX)) as u64)
    }
}

pub(crate) struct Wait(Clock);

impl Drop for Wait {
    fn drop(&mut self) {
        if let Ok(mut meter) = self.0.0.lock() {
            meter.waits = meter.waits.saturating_sub(1);

            if meter.waits == 0 {
                meter.since = Some(Instant::now());
            }
        }
    }
}

#[cfg(test)]
mod tests;

pub(crate) fn view(budget: &krabb_core::execution::budget::Budget) -> Value {
    serde_json::json!({
        "limit_millis":budget.limit_millis,
        "used_millis":budget.spent_millis,
        "unresolved_millis":budget.held_millis,
        "reserved_millis":budget.pending.map_or(0,|pending| pending.millis),
        "earlier_usage_unknown":budget.legacy,
        "reported_tokens":budget.tokens.reported,
        "estimated_tokens":budget.tokens.estimated,
    })
}
