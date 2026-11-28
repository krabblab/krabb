//! Paces a channel's typing indicator in bursts and pauses, like a person writing.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long after a turn starts the first indicator appears.
pub(crate) const DELAY: Duration = Duration::from_millis(500);

const REFRESH: Duration = Duration::from_secs(4);
const REFRESH_LONGEST: Duration = Duration::from_secs(30);
const SHORT_LIFETIME: Duration = Duration::from_secs(6);
const OPENING: Duration = Duration::from_secs(6);
const CLOSING: Duration = Duration::from_secs(4);
const BURST_SHORTEST: Duration = Duration::from_secs(4);
const BURST_LONGEST: Duration = Duration::from_secs(15);
const PAUSE_SPAN: Duration = Duration::from_secs(6);
const PAUSE_STOPPED: Duration = Duration::from_secs(2);
const PAUSE_VISIBLE: Duration = Duration::from_secs(2);
const PAUSE_LONGEST: Duration = Duration::from_secs(20);
const RETRY: Duration = Duration::from_secs(1);
const LIFETIME_SHORTEST: u64 = 3;
const LIFETIME_LONGEST: u64 = 300;

/// How long a channel's typing indicator stays visible and whether the channel can clear it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Indicator {
    /// Seconds the platform keeps the indicator after one `typing` call.
    pub(crate) lifetime: u64,
    /// Whether the channel clears the indicator when `typing` carries `stop`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) stop: bool,
}

impl Indicator {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !(LIFETIME_SHORTEST..=LIFETIME_LONGEST).contains(&self.lifetime) {
            return Err(format!(
                "The typing lifetime must be between {LIFETIME_SHORTEST} and {LIFETIME_LONGEST} \
                 seconds."
            ));
        }

        Ok(())
    }

    /// How often typing is refreshed: a second before short indicators expire, two before others.
    fn refresh(&self) -> Duration {
        let lifetime = Duration::from_secs(self.lifetime.max(LIFETIME_SHORTEST));

        if lifetime <= SHORT_LIFETIME {
            return REFRESH.min(lifetime - Duration::from_secs(1));
        }

        REFRESH_LONGEST.min(lifetime - Duration::from_secs(2))
    }

    /// The shortest and longest pause; none when a pause outlasting the indicator would look idle.
    fn pauses(&self) -> Option<(Duration, Duration)> {
        let shortest = if self.stop {
            PAUSE_STOPPED
        } else {
            Duration::from_secs(self.lifetime) + PAUSE_VISIBLE
        };

        let longest = shortest + PAUSE_SPAN;

        (longest <= PAUSE_LONGEST).then_some((shortest, longest))
    }
}

/// What the host does with the indicator at one wake of the rhythm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Beat {
    Show,
    Stop,
    Rest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Burst(Duration),
    Pause(Duration),
}

/// Whether a pause still owes the channel a stop after one failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Retry {
    Unused,
    Due,
    Spent,
}

/// A seeded schedule of typing bursts and pauses, measured from the start of a turn.
pub(crate) struct Rhythm {
    pauses: Option<(Duration, Duration)>,
    stop: bool,
    refresh: Duration,
    random: Random,
    phase: Phase,
    hold: Option<Duration>,
    retry: Retry,
    showing: bool,
}

impl Rhythm {
    /// A rhythm for a channel's declared indicator; without one, typing refreshes steadily.
    pub(crate) fn new(indicator: Option<Indicator>, seed: u64) -> Self {
        let mut random = Random(seed);
        let burst = random.between(BURST_SHORTEST, BURST_LONGEST);

        Self {
            pauses: indicator.and_then(|indicator| indicator.pauses()),
            stop: indicator.is_some_and(|indicator| indicator.stop),
            refresh: indicator.map_or(REFRESH, |indicator| indicator.refresh()),
            random,
            phase: Phase::Burst(burst),
            hold: None,
            retry: Retry::Unused,
            showing: false,
        }
    }

    /// What to do at `now` and when to wake next, both measured from the turn's start.
    pub(crate) fn beat(&mut self, now: Duration) -> (Beat, Duration) {
        let Some((shortest, longest)) = self.pauses else {
            self.showing = true;

            return (Beat::Show, now + self.refresh);
        };

        match self.phase {
            Phase::Pause(end) if now < end && self.retry == Retry::Due => {
                self.retry = Retry::Spent;
                self.showing = false;

                (Beat::Stop, end)
            }

            Phase::Pause(end) if now < end => (Beat::Rest, end),

            Phase::Pause(_) => {
                let end = now + self.random.between(BURST_SHORTEST, BURST_LONGEST);

                self.phase = Phase::Burst(end);
                self.showing = true;

                (Beat::Show, (now + self.refresh).min(end))
            }

            Phase::Burst(end) => {
                let until = self.hold.map_or(Duration::MAX, |hold| end.max(hold));

                if now < until {
                    self.showing = true;

                    return (Beat::Show, (now + self.refresh).min(until));
                }

                let end = now + self.random.between(shortest, longest);

                self.phase = Phase::Pause(end);
                self.retry = Retry::Unused;

                if !self.stop {
                    return (Beat::Rest, end);
                }

                self.showing = false;

                (Beat::Stop, end)
            }
        }
    }

    /// Reply text arrived, so pauses may start and typing holds until the reply is out; true when
    /// a pause ended.
    pub(crate) fn reply(&mut self, now: Duration) -> bool {
        let hold = OPENING.max(now + CLOSING);

        self.hold = Some(self.hold.map_or(hold, |held| held.max(hold)));

        if !matches!(self.phase, Phase::Pause(_)) {
            return false;
        }

        self.resume(now);

        true
    }

    /// Starts a fresh burst at `now`, such as when a tool resumes after an approval.
    pub(crate) fn resume(&mut self, now: Duration) {
        self.phase = Phase::Burst(now + self.random.between(BURST_SHORTEST, BURST_LONGEST));
        self.retry = Retry::Unused;
    }

    /// A stop failed; the wake that retries it once during this pause, if any.
    pub(crate) fn missed(&mut self, now: Duration) -> Option<Duration> {
        self.showing = true;

        let Phase::Pause(end) = self.phase else {
            return None;
        };

        if self.retry != Retry::Unused {
            return None;
        }

        self.retry = Retry::Due;

        Some((now + RETRY).min(end))
    }

    /// Whether a stop would clear an indicator the channel may still show.
    pub(crate) fn clears(&self) -> bool {
        self.stop && self.showing
    }
}

/// SplitMix64, which is enough to vary a rhythm and needs no dependency.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);

        let mut value = self.0;

        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);

        value ^ (value >> 31)
    }

    fn between(&mut self, shortest: Duration, longest: Duration) -> Duration {
        let span = u64::try_from(longest.saturating_sub(shortest).as_millis()).unwrap_or(u64::MAX);

        shortest + Duration::from_millis(self.next() % span.saturating_add(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TELEGRAM: Indicator = Indicator { lifetime: 5, stop: false };
    const DISCORD: Indicator = Indicator { lifetime: 10, stop: false };
    const SLACK: Indicator = Indicator { lifetime: 120, stop: true };
    const SIGNAL: Indicator = Indicator { lifetime: 15, stop: true };
    const WHATSAPP: Indicator = Indicator { lifetime: 25, stop: false };

    #[derive(Debug, PartialEq)]
    enum Span {
        Burst(Duration, Duration),
        Pause(Duration, Duration),
    }

    fn schedule(indicator: Option<Indicator>, seed: u64, turn: Duration) -> Vec<(Beat, Duration)> {
        let mut rhythm = Rhythm::new(indicator, seed);
        let mut now = DELAY;
        let mut beats = Vec::new();

        rhythm.reply(DELAY);

        while now < turn {
            let (beat, next) = rhythm.beat(now);

            assert!(next > now);

            beats.push((beat, now));
            now = next;
        }

        beats
    }

    fn phases(beats: &[(Beat, Duration)]) -> Vec<Span> {
        let mut phases = Vec::new();
        let mut start = beats[0].1;
        let mut showing = true;

        for &(beat, at) in &beats[1..] {
            let shown = beat == Beat::Show;

            if shown != showing {
                phases.push(if showing { Span::Burst(start, at) } else { Span::Pause(start, at) });

                start = at;
                showing = shown;
            }
        }

        phases
    }

    #[test]
    fn seeded_rhythms_repeat_and_vary() {
        let turn = Duration::from_secs(180);

        assert_eq!(schedule(Some(TELEGRAM), 7, turn), schedule(Some(TELEGRAM), 7, turn));
        assert_ne!(schedule(Some(TELEGRAM), 7, turn), schedule(Some(TELEGRAM), 8, turn));
    }

    #[test]
    fn bursts_and_pauses_stay_within_human_bounds() {
        for (indicator, shortest) in [
            (TELEGRAM, Duration::from_secs(7)),
            (DISCORD, Duration::from_secs(12)),
            (SLACK, Duration::from_secs(2)),
            (SIGNAL, Duration::from_secs(2)),
        ] {
            for seed in 0..64 {
                let beats = schedule(Some(indicator), seed, Duration::from_secs(300));
                let phases = phases(&beats);
                let pauses = phases.iter().filter(|span| matches!(span, Span::Pause(..))).count();

                assert!(pauses >= 5, "{indicator:?} {seed}");

                for span in &phases[1..] {
                    match *span {
                        Span::Burst(start, end) => {
                            assert!(end - start >= BURST_SHORTEST);
                            assert!(end - start <= BURST_LONGEST);
                        }

                        Span::Pause(start, end) => {
                            assert!(end - start >= shortest, "{indicator:?} {seed}");
                            assert!(end - start <= shortest + PAUSE_SPAN);
                        }
                    }
                }

                for pair in beats.windows(2) {
                    if pair[0].0 == Beat::Show && pair[1].0 == Beat::Show {
                        assert!(pair[1].1 - pair[0].1 <= indicator.refresh());
                    }
                }
            }
        }
    }

    #[test]
    fn pauses_outlast_indicators_that_cannot_be_stopped() {
        for indicator in [TELEGRAM, DISCORD] {
            for seed in 0..64 {
                let beats = schedule(Some(indicator), seed, Duration::from_secs(300));

                assert!(beats.iter().all(|(beat, _)| *beat != Beat::Stop));

                let lifetime = Duration::from_secs(indicator.lifetime);

                for beat in beats.windows(3) {
                    if let [(Beat::Show, shown), (Beat::Rest, _), (Beat::Show, resumed)] = *beat {
                        assert!(resumed - shown >= lifetime + PAUSE_VISIBLE, "{seed}");
                    }
                }
            }
        }
    }

    #[test]
    fn stoppable_indicators_are_cleared_when_a_pause_starts() {
        for indicator in [SLACK, SIGNAL] {
            let beats = schedule(Some(indicator), 3, Duration::from_secs(300));

            assert!(beats.iter().any(|(beat, _)| *beat == Beat::Stop));
            assert!(
                beats.windows(2).all(|pair| pair[1].0 != Beat::Stop || pair[0].0 == Beat::Show)
            );
        }
    }

    #[test]
    fn turns_open_with_typing() {
        for seed in 0..256 {
            for indicator in [TELEGRAM, DISCORD, SLACK, SIGNAL] {
                let beats = schedule(Some(indicator), seed, OPENING);

                assert!(beats.iter().all(|(beat, _)| *beat == Beat::Show), "{seed}");
            }
        }
    }

    #[test]
    fn reply_text_ends_a_pause_and_holds_typing() {
        for seed in 0..64 {
            let mut rhythm = Rhythm::new(Some(TELEGRAM), seed);
            let mut now = DELAY;

            rhythm.reply(DELAY);

            let pause = loop {
                let (beat, next) = rhythm.beat(now);

                if beat == Beat::Rest {
                    break next;
                }

                now = next;
            };

            let written = now + Duration::from_secs(1);

            assert!(written < pause);
            assert!(rhythm.reply(written));
            assert!(!rhythm.reply(written));

            let mut now = written;

            while now < written + CLOSING {
                let (beat, next) = rhythm.beat(now);

                assert_eq!(beat, Beat::Show);
                assert!(next - now <= TELEGRAM.refresh());

                now = next;
            }
        }
    }

    #[test]
    fn lifetimes_set_the_refresh_and_pause_bounds() {
        for (indicator, refresh) in [
            (Indicator { lifetime: 3, stop: false }, 2),
            (TELEGRAM, 4),
            (Indicator { lifetime: 6, stop: false }, 4),
            (DISCORD, 8),
            (SIGNAL, 13),
            (WHATSAPP, 23),
            (SLACK, 30),
        ] {
            assert_eq!(indicator.refresh(), Duration::from_secs(refresh), "{indicator:?}");
        }

        assert_eq!(TELEGRAM.pauses().unwrap().0, Duration::from_secs(7));

        assert_eq!(DISCORD.pauses().unwrap().0, Duration::from_secs(12));
        assert_eq!(SLACK.pauses().unwrap().0, Duration::from_secs(2));
        assert_eq!(WHATSAPP.pauses(), None);

        for lifetime in [LIFETIME_SHORTEST, 5, LIFETIME_LONGEST] {
            assert!(Indicator { lifetime, stop: false }.validate().is_ok());
        }

        for lifetime in [0, LIFETIME_SHORTEST - 1, LIFETIME_LONGEST + 1] {
            assert!(Indicator { lifetime, stop: true }.validate().is_err());
        }
    }

    #[test]
    fn turns_without_reply_text_never_pause() {
        for indicator in [TELEGRAM, DISCORD, SLACK, SIGNAL] {
            let mut rhythm = Rhythm::new(Some(indicator), 5);
            let mut now = DELAY;

            while now < Duration::from_secs(300) {
                let (beat, next) = rhythm.beat(now);

                assert_eq!(beat, Beat::Show, "{indicator:?}");
                assert!(next - now <= indicator.refresh());

                now = next;
            }
        }
    }

    #[test]
    fn resuming_starts_a_full_burst() {
        for seed in 0..64 {
            let mut rhythm = Rhythm::new(Some(SIGNAL), seed);
            let mut now = DELAY;

            rhythm.reply(DELAY);

            loop {
                let (beat, next) = rhythm.beat(now);

                if beat == Beat::Stop {
                    break;
                }

                now = next;
            }

            assert!(!rhythm.clears());

            rhythm.resume(now);

            let (beat, _) = rhythm.beat(now);

            assert_eq!(beat, Beat::Show);
            assert!(rhythm.clears());

            let mut later = now;

            while later < now + BURST_SHORTEST {
                let (beat, next) = rhythm.beat(later);

                assert_eq!(beat, Beat::Show, "{seed}");

                later = next;
            }
        }
    }

    #[test]
    fn a_failed_stop_is_retried_once() {
        let mut rhythm = Rhythm::new(Some(SLACK), 2);
        let mut now = DELAY;

        assert_eq!(rhythm.missed(now), None);

        rhythm.reply(DELAY);

        let end = loop {
            let (beat, next) = rhythm.beat(now);

            if beat == Beat::Stop {
                break next;
            }

            now = next;
        };

        let retry = rhythm.missed(now).unwrap();

        assert_eq!(retry, now + RETRY);
        assert!(rhythm.clears());
        assert_eq!(rhythm.beat(retry), (Beat::Stop, end));
        assert!(!rhythm.clears());
        assert_eq!(rhythm.missed(retry), None);
        assert_eq!(rhythm.beat(retry + RETRY / 2), (Beat::Rest, end));
        assert_eq!(rhythm.beat(end).0, Beat::Show);
    }

    #[test]
    fn only_stoppable_indicators_are_cleared() {
        for (indicator, clears) in [(Some(SLACK), true), (Some(TELEGRAM), false), (None, false)] {
            let mut rhythm = Rhythm::new(indicator, 4);

            assert!(!rhythm.clears());

            rhythm.beat(DELAY);

            assert_eq!(rhythm.clears(), clears, "{indicator:?}");
        }
    }

    #[test]
    fn long_lived_indicators_refresh_without_pauses() {
        let beats = schedule(Some(WHATSAPP), 1, Duration::from_secs(120));

        assert!(beats.iter().all(|(beat, _)| *beat == Beat::Show));
    }

    #[test]
    fn channels_without_a_declaration_keep_the_steady_refresh() {
        let mut rhythm = Rhythm::new(None, 9);

        assert!(!rhythm.reply(Duration::from_secs(1)));

        let beats = schedule(None, 9, Duration::from_secs(60));

        assert_eq!(beats.len(), 15);

        for (index, (beat, at)) in beats.iter().enumerate() {
            assert_eq!(*beat, Beat::Show);
            assert_eq!(*at, DELAY + REFRESH * u32::try_from(index).unwrap());
        }
    }
}
