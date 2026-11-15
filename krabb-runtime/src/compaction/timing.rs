use super::*;

pub(super) fn remaining(deadline: Instant) -> u64 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .min(u128::from(krabb_core::execution::MAX_MILLIS)) as u64
}

pub(super) fn restrict(input: &mut ModelRequest, deadline: Instant) -> Result<()> {
    if let Some(generation) = &mut input.generation
        && let Some(limits) = generation.limits
    {
        generation.limits = Some(limits.restrict(remaining(deadline))?);
    }

    Ok(())
}
