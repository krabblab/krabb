use super::*;

use krabb_core::{
    execution::budget::{Budget, Policy, Reservation},
    task::Run,
};

#[derive(Debug)]
pub(crate) struct TokensExhausted;

impl std::fmt::Display for TokensExhausted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(concat!(
            "The task's token allowance is exhausted. Its saved goal and artifacts remain; ",
            "raise inference.task_tokens to continue."
        ))
    }
}

impl std::error::Error for TokensExhausted {}

impl Store {
    fn save_budget(&mut self, id: &str, budget: Budget) -> std::io::Result<()> {
        self.change_jobs(|jobs| jobs.get_mut(id).unwrap().budget = Some(budget))
    }

    pub fn recover_budget(&mut self, id: &str) -> std::io::Result<bool> {
        let job = self.job(id)?;

        let mut budget = job.account()?;

        if !budget.recover().map_err(std::io::Error::other)? {
            return Ok(false);
        }

        self.save_budget(id, budget)?;
        Ok(true)
    }

    pub fn grant_budget(
        &mut self,
        id: &str,
        grant: &krabb_core::execution::budget::Grant,
        host_limit: Option<u64>,
    ) -> std::io::Result<krabb_core::execution::budget::GrantReceipt> {
        grant.validate().map_err(std::io::Error::other)?;

        let job = self.job(id)?;

        if let Some(receipt) = job.grants.iter().find(|receipt| receipt.grant.id == grant.id) {
            return if receipt.grant == *grant {
                Ok(receipt.clone())
            } else {
                Err(std::io::Error::other("This grant identity is already in use."))
            };
        }

        if job.status.terminal()
            || job.status == JobStatus::Running
            || job.status == JobStatus::Reporting
            || job.uncertain
            || job.status.stopping()
            || job.revision != grant.revision
        {
            return Err(std::io::Error::other("The task changed or cannot accept a budget grant."));
        }

        if host_limit.is_some_and(|limit| grant.limit_millis > limit) {
            return Err(std::io::Error::other(
                "The grant exceeds the configured host inference ceiling.",
            ));
        }

        let mut budget = job.account()?;

        if budget.sequence != grant.sequence {
            return Err(std::io::Error::other(
                "The execution account changed; review it before granting more allowance.",
            ));
        }

        budget.increase(grant.limit_millis).map_err(std::io::Error::other)?;

        let revision = job
            .revision
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("Task revision is exhausted."))?;

        let receipt =
            krabb_core::execution::budget::GrantReceipt { grant: grant.clone(), revision };

        self.change_jobs(|jobs| {
            let job = jobs.get_mut(id).unwrap();
            job.budget = Some(budget);
            job.revision = revision;

            if job.grants.len() == 32 {
                job.grants.remove(0);
            }

            job.grants.push(receipt.clone());
        })?;

        Ok(receipt)
    }

    pub fn budget_remaining(&self, run: &Run, policy: impl Into<Policy>) -> std::io::Result<u64> {
        self.check_job_step(&run.id, run.generation)?;
        self.jobs[&run.id]
            .budget
            .as_ref()
            .ok_or_else(|| std::io::Error::other("Task accounting is missing."))?
            .remaining(policy)
            .map_err(std::io::Error::other)
    }

    pub fn release_tokens(&mut self, id: &str, estimate: u64) -> std::io::Result<()> {
        let Some(mut budget) = self.jobs.get(id).and_then(|job| job.budget.clone()) else {
            return Ok(());
        };

        budget.tokens.estimated = budget.tokens.estimated.saturating_sub(estimate);
        self.save_budget(id, budget)
    }

    pub fn settle_tokens(
        &mut self,
        id: &str,
        estimate: (u64, u64),
        input: Option<u64>,
        output: Option<u64>,
    ) -> std::io::Result<()> {
        let Some(mut budget) = self.jobs.get(id).and_then(|job| job.budget.clone()) else {
            return Ok(());
        };

        budget.tokens.estimated = budget.tokens.estimated.saturating_sub(estimate.0);
        budget.tokens.charge(input, output, estimate).map_err(std::io::Error::other)?;
        self.save_budget(id, budget)
    }

    #[cfg(test)]
    pub fn reserve_budget(
        &mut self,
        run: &Run,
        millis: u64,
        policy: impl Into<Policy>,
    ) -> std::io::Result<Reservation> {
        self.reserve_step(run, millis, policy, (0, None))
    }

    /// Reserves a step's inference time and estimated input tokens in one durable write.
    pub fn reserve_step(
        &mut self,
        run: &Run,
        millis: u64,
        policy: impl Into<Policy>,
        (estimate, tokens): (u64, Option<u64>),
    ) -> std::io::Result<Reservation> {
        self.check_job_step(&run.id, run.generation)?;

        let mut budget = self.jobs[&run.id].account()?;

        // A step must leave room for its output, so an exact fit is refused.
        if budget.tokens.remaining(tokens) <= estimate {
            return Err(std::io::Error::other(TokensExhausted));
        }

        budget.tokens.estimated = budget.tokens.estimated.saturating_add(estimate);

        let reservation =
            budget.reserve(run.generation, millis, policy).map_err(std::io::Error::other)?;

        self.save_budget(&run.id, budget)?;
        Ok(reservation)
    }

    pub fn extend_budget(
        &mut self,
        id: &str,
        reservation: Reservation,
        millis: u64,
        policy: impl Into<Policy>,
    ) -> std::io::Result<Reservation> {
        let mut budget = self
            .jobs
            .get(id)
            .ok_or_else(|| std::io::Error::other("Task accounting is missing."))?
            .account()?;

        let extended = budget.extend(reservation, millis, policy).map_err(std::io::Error::other)?;

        self.save_budget(id, budget)?;
        Ok(extended)
    }

    pub fn settle_budget(
        &mut self,
        id: &str,
        reservation: Reservation,
        used: Option<u64>,
    ) -> std::io::Result<()> {
        let job = self.job(id)?;

        let mut budget = job.account()?;

        if budget.settle(reservation, used).map_err(std::io::Error::other)? {
            self.save_budget(id, budget)?;
        }

        Ok(())
    }
}
