use super::*;
use krabb_core::failure::{Failure, Kind};

/// Finds the typed core error, including one a store wrapped in an I/O error.
fn core<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a krabb_core::Error> {
    error.downcast_ref::<krabb_core::Error>().or_else(|| {
        error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .and_then(|inner| inner.downcast_ref::<krabb_core::Error>())
    })
}

pub(crate) fn classify(error: &(dyn std::error::Error + 'static)) -> Failure {
    if let Some(error) = core(error) {
        return match error {
            krabb_core::Error::Failed { failure, .. } if failure.validate().is_ok() => {
                failure.clone()
            }

            krabb_core::Error::Denied(_) => Failure::new(Kind::Denied),
            krabb_core::Error::Limit(_) => Failure::new(Kind::Exhausted),

            krabb_core::Error::Protocol(_) | krabb_core::Error::Json(_) => {
                Failure::new(Kind::Invalid)
            }

            krabb_core::Error::Io(error) => classify(error),
            _ => Failure::new(Kind::Unknown),
        };
    }

    if error.is::<inference::Refused>() || error.is::<accounting::Blocked>() {
        return Failure::new(Kind::Exhausted);
    }

    if let Some(error) = error.downcast_ref::<std::io::Error>() {
        return Failure::new(match error.kind() {
            std::io::ErrorKind::TimedOut => Kind::TimedOut,
            std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof => Kind::Transient,
            std::io::ErrorKind::PermissionDenied => Kind::Denied,
            std::io::ErrorKind::NotFound => Kind::Missing,
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => Kind::Invalid,
            _ => Kind::Unknown,
        });
    }

    Failure::new(Kind::Unknown)
}

const WAIT_LIMIT: u64 = 30;

const RETRY_LIMIT: u32 = 5;

/// A turn that ran out of time is unlikely to fit on a replay, so it restarts only once.
const TIMEOUT_LIMIT: u32 = 1;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Retry {
    Never,
    After(Duration),
    Restart,
}

/// Decides how a failed turn recovers: final refusals stop, a healthy provider's retryable
/// answer waits for its hint a few times, and transport or timeout faults restart the plugin.
/// A hint beyond the wait limit ends the retries so the user can resend later.
pub(crate) fn retry(error: &(dyn std::error::Error + 'static), attempts: u32) -> Retry {
    if attempts > RETRY_LIMIT
        || error.is::<inference::Refused>()
        || error.is::<accounting::Blocked>()
    {
        return Retry::Never;
    }

    match core(error) {
        Some(
            krabb_core::Error::Denied(_) | krabb_core::Error::Limit(_) | krabb_core::Error::Json(_),
        ) => Retry::Never,

        Some(krabb_core::Error::Failed { failure, .. }) if !failure.kind.retryable() => {
            Retry::Never
        }

        Some(krabb_core::Error::Failed { failure, .. })
            if failure.kind == Kind::TimedOut && attempts > TIMEOUT_LIMIT =>
        {
            Retry::Never
        }

        Some(krabb_core::Error::Failed { failure, .. }) if failure.kind != Kind::TimedOut => {
            let backoff = super::retry_delay(attempts).as_secs();
            let wait = failure.retry_after_seconds.unwrap_or(backoff).max(backoff);

            if wait > WAIT_LIMIT {
                return Retry::Never;
            }

            Retry::After(Duration::from_secs(wait))
        }

        _ => Retry::Restart,
    }
}

/// Whether a foreground task's failure came from a provider fault that needs a restart; the
/// task itself is reported through its notice, so only the provider is recovered.
pub(crate) fn restarts(failure: &tasks::Failure) -> bool {
    let cause = std::error::Error::source(failure).unwrap_or(failure);

    retry(cause, 1) == Retry::Restart
}

/// Describes a failed answer: actionable typed causes, otherwise the generic fallback.
/// A queued automatic retry replaces the advice to retry so the user does not resend.
pub(crate) fn reply(error: &(dyn std::error::Error + 'static), retrying: bool) -> String {
    let failure = classify(error);

    let detail = match core(error) {
        Some(krabb_core::Error::Denied(reason) | krabb_core::Error::Limit(reason)) => {
            Some(reason.clone())
        }

        Some(krabb_core::Error::Failed { message, .. }) if failure.kind != Kind::Unknown => {
            Some(message.clone())
        }

        _ if error.is::<inference::Refused>() || error.is::<accounting::Blocked>() => {
            Some(error.to_string())
        }

        _ => None,
    };

    if retrying {
        let detail =
            detail.map_or_else(|| "I've encountered an error while answering.".into(), sentence);

        return format!("{detail} I will retry automatically.");
    }

    let Some(detail) = detail else { return routing::ERROR.into() };

    let next = match failure.kind {
        Kind::Denied | Kind::Missing | Kind::Invalid => {
            "Check the model connection with `krabb doctor`."
        }

        Kind::Exhausted => "Raise the limit or wait for it to reset, then retry.",
        _ => "Please retry.",
    };

    format!("{} {next}", sentence(detail))
}

pub(crate) fn blocked(message: impl Into<String>) -> PluginError {
    krabb_core::Error::Denied(message.into()).into()
}

pub(crate) fn timeout() -> PluginError {
    Failure::new(Kind::TimedOut).error("The turn exceeded its time limit.").into()
}

pub(crate) fn settle(
    store: &mut state::Store,
    id: &str,
    error: &(dyn std::error::Error + 'static),
) -> std::io::Result<()> {
    let failure = classify(error);

    if store.task(id)?.status != state::TaskStatus::Running {
        return Ok(());
    }

    let replayable = store.task(id).is_ok_and(|task| !task.unsafe_step && !task.uncertain);

    if failure.kind.retryable() && replayable {
        return store.retry_task_after(id, failure, unix_seconds());
    }

    let reason = match core(error) {
        _ if failure.kind == Kind::Unknown => routing::ERROR.into(),

        Some(krabb_core::Error::Denied(reason) | krabb_core::Error::Limit(reason)) => {
            reason.clone()
        }

        Some(krabb_core::Error::Failed { message, .. }) => message.clone(),
        _ => error.to_string(),
    };

    let checkpoint = store.task(id)?.checkpoint.clone();

    let block = |store: &mut state::Store| {
        store.checkpoint_task(
            id,
            &checkpoint,
            state::TaskStatus::Blocked,
            unix_seconds(),
            Some(&reason),
        )
    };

    if failure.kind != Kind::Uncertain {
        return block(store);
    }

    store.atomic(|store| {
        let task = store.tasks.get_mut(id).unwrap();

        task.uncertain = true;
        task.deferred = None;
        block(store)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_refusals_stop_and_healthy_providers_are_not_restarted() {
        let answered: PluginError = Failure::new(Kind::Denied).error("The key is invalid.").into();
        let unknown: PluginError = Failure::new(Kind::Unknown).error("Model failed.").into();
        let throttled: PluginError = Failure::new(Kind::Throttled).error("Slow down.").into();
        let stopped: PluginError = krabb_core::Error::Protocol("Plugin is stopped.".into()).into();
        let host: PluginError = "The provider process exited.".into();

        let hinted: PluginError = Failure { kind: Kind::Throttled, retry_after_seconds: Some(20) }
            .error("Slow down.")
            .into();

        let long: PluginError = Failure { kind: Kind::Throttled, retry_after_seconds: Some(120) }
            .error("Slow down.")
            .into();

        let expired: PluginError = timeout();
        let denied: PluginError = blocked("Tools are disabled.");
        let limit: PluginError = krabb_core::Error::Limit("Too many tokens.".into()).into();
        let refused: PluginError = Box::new(inference::Refused("Context is too large.".into()));
        let budget: PluginError = Box::new(accounting::Blocked("Budget is used.".into()));
        let json: PluginError =
            krabb_core::Error::from(serde_json::from_str::<u8>("x").unwrap_err()).into();

        for error in [&answered, &unknown, &denied, &limit, &refused, &budget, &long, &json] {
            assert_eq!(retry(error.as_ref(), 1), Retry::Never, "{error}");
        }

        assert_eq!(retry(throttled.as_ref(), 1), Retry::After(Duration::from_secs(2)));
        assert_eq!(retry(throttled.as_ref(), RETRY_LIMIT), Retry::After(Duration::from_secs(30)));
        assert_eq!(retry(throttled.as_ref(), RETRY_LIMIT + 1), Retry::Never);
        assert_eq!(retry(hinted.as_ref(), 1), Retry::After(Duration::from_secs(20)));

        for error in [&expired, &stopped, &host] {
            assert_eq!(retry(error.as_ref(), 1), Retry::Restart, "{error}");
        }

        assert_eq!(
            reply(answered.as_ref(), false),
            "The key is invalid. Check the model connection with `krabb doctor`."
        );

        assert_eq!(reply(throttled.as_ref(), false), "Slow down. Please retry.");
        assert_eq!(reply(throttled.as_ref(), true), "Slow down. I will retry automatically.");
        assert_eq!(reply(unknown.as_ref(), false), routing::ERROR);
        assert_eq!(reply(host.as_ref(), false), routing::ERROR);

        assert_eq!(
            reply(host.as_ref(), true),
            "I've encountered an error while answering. I will retry automatically."
        );

        let stored: PluginError = Box::new(std::io::Error::other(krabb_core::Error::Limit(
            "The task's remaining allowance cannot cover a renewal.".into(),
        )));

        assert_eq!(classify(stored.as_ref()).kind, Kind::Exhausted);
        assert_eq!(retry(stored.as_ref(), 1), Retry::Never);

        assert_eq!(
            reply(stored.as_ref(), false),
            "The task's remaining allowance cannot cover a renewal. Raise the limit or wait for \
             it to reset, then retry."
        );

        assert_eq!(retry(expired.as_ref(), TIMEOUT_LIMIT + 1), Retry::Never);
        assert_eq!(retry(stopped.as_ref(), TIMEOUT_LIMIT + 1), Retry::Restart);
        assert_eq!(retry(host.as_ref(), RETRY_LIMIT + 1), Retry::Never);
    }

    #[test]
    fn a_failed_foreground_task_restarts_only_a_faulty_provider() {
        let task = |source: PluginError| {
            tasks::Failure::new("task-1".into(), crate::state::TaskStatus::Blocked, source)
        };

        let crashed = std::io::Error::from(std::io::ErrorKind::BrokenPipe);

        assert!(restarts(&task(crashed.into())));
        assert!(!restarts(&task(blocked("Tools are disabled."))));
        assert!(!restarts(&task(Failure::new(Kind::Denied).error("The key is invalid.").into())));
    }
}
