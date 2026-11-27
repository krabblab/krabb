use std::time::Duration;

pub(super) const RESTORED: &str = "Krabb restarted, and this window reconnected.\nPending \
    approvals were cleared, so ask again if a reply is missing.";

pub(super) const LOST: &str =
    "Krabb restarted, and this window could not reconnect.\nReopen it with `krabb tui`.";

pub(super) const WAITING: &str = "Krabb is reconnecting, so send this again in a moment.";

pub(super) const DISCONNECTED: &str = "This window is disconnected, so reopen it with `krabb tui`.";

const ATTEMPTS: u32 = 6;

const LONGEST: Duration = Duration::from_secs(8);

/// Tracks the TUI's link to its session engine across daemon restarts.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) enum Reconnect {
    #[default]
    Connected,
    Waiting {
        attempt: u32,
    },
    Lost,
}

impl Reconnect {
    /// Records a failed engine and returns how long to wait before starting another one.
    pub(super) fn fail(&mut self) -> Option<Duration> {
        let attempt = match self {
            Self::Connected => 1,
            Self::Waiting { attempt } => attempt.saturating_add(1),
            Self::Lost => return None,
        };

        if attempt > ATTEMPTS {
            *self = Self::Lost;
            return None;
        }

        *self = Self::Waiting { attempt };

        Some(Duration::from_secs(1 << (attempt - 1)).min(LONGEST))
    }

    /// Marks the engine as answering, and returns whether that ended a reconnection.
    pub(super) fn ready(&mut self) -> bool {
        let restored = matches!(self, Self::Waiting { .. });

        if restored {
            *self = Self::Connected;
        }

        restored
    }

    /// Explains why input cannot reach the engine right now.
    pub(super) fn status(&self) -> &'static str {
        if matches!(self, Self::Waiting { .. }) { WAITING } else { DISCONNECTED }
    }
}

#[cfg(test)]
mod tests {
    use super::{Duration, Reconnect};

    #[test]
    fn backs_off_then_gives_up_after_bounded_attempts() {
        let mut link = Reconnect::default();
        let mut delays = Vec::new();

        while let Some(delay) = link.fail() {
            delays.push(delay.as_secs());
        }

        assert_eq!(delays, [1, 2, 4, 8, 8, 8]);
        assert_eq!(link, Reconnect::Lost);
        assert_eq!(link.fail(), None);
        assert!(!link.ready());
        assert_eq!(link.status(), super::DISCONNECTED);
    }

    #[test]
    fn a_ready_engine_resets_the_attempts() {
        let mut link = Reconnect::default();

        assert!(!link.ready());
        assert_eq!(link.fail(), Some(Duration::from_secs(1)));
        assert_eq!(link.fail(), Some(Duration::from_secs(2)));
        assert_eq!(link.status(), super::WAITING);

        assert!(link.ready());
        assert_eq!(link, Reconnect::Connected);
        assert_eq!(link.fail(), Some(Duration::from_secs(1)));
    }
}
