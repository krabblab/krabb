//! Paces a channel's typing indicator in bursts and pauses, like a person writing.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// When the first indicator appears after a turn starts.
pub(crate) const FIRST: Duration = Duration::from_millis(500);

const REFRESH: Duration = Duration::from_secs(4);
const OPENING: Duration = Duration::from_secs(6);
const CLOSING: Duration = Duration::from_secs(4);
const BURST_SHORTEST: Duration = Duration::from_secs(4);
const BURST_LONGEST: Duration = Duration::from_secs(15);
const PAUSE_SPAN: Duration = Duration::from_secs(6);
const PAUSE_STOPPED: Duration = Duration::from_secs(2);
const PAUSE_VISIBLE: Duration = Duration::from_secs(2);
const PAUSE_LONGEST: Duration = Duration::from_secs(20);
const LIFETIME_LONGEST: u64 = 300;

/// How long a channel's typing indicator stays visible and whether the channel can clear it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Indicator {
    /// Seconds the platform keeps the indicator after one `typing` call.
    pub(crate) lifetime: u64,
    /// Whether the channel clears the indicator when `typing` carries `stop`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) stop: bool,
}

impl Indicator {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !(1..=LIFETIME_LONGEST).contains(&self.lifetime) {
            return Err(format!(
                "The typing lifetime must be between 1 and {LIFETIME_LONGEST} seconds."
            ));
        }

        Ok(())
    }

    fn refresh(&self) -> Duration {
        REFRESH.min(Duration::from_secs(self.lifetime.saturating_sub(1).max(1)))
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

/// A seeded schedule of typing bursts and pauses, measured from the start of a turn.
pub(crate) struct Rhythm {
    pauses: Option<(Duration, Duration)>,
    stop: bool,
    refresh: Duration,
    random: Random,
    phase: Phase,
    hold: Duration,
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
            hold: OPENING,
        }
    }

    /// What to do at `now` and when to wake next, both measured from the turn's start.
    pub(crate) fn beat(&mut self, now: Duration) -> (Beat, Duration) {
        let Some((shortest, longest)) = self.pauses else {
            return (Beat::Show, now + self.refresh);
        };

        match self.phase {
            Phase::Pause(end) if now < end => (Beat::Rest, end),

            Phase::Pause(_) => {
                let end = now + self.random.between(BURST_SHORTEST, BURST_LONGEST);

                self.phase = Phase::Burst(end);

                (Beat::Show, (now + self.refresh).min(end))
            }

            Phase::Burst(end) if now < end.max(self.hold) => {
                (Beat::Show, (now + self.refresh).min(end.max(self.hold)))
            }

            Phase::Burst(_) => {
                let end = now + self.random.between(shortest, longest);

                self.phase = Phase::Pause(end);

                (if self.stop { Beat::Stop } else { Beat::Rest }, end)
            }
        }
    }

    /// Reply text arrived, so typing holds until the reply is out; true when a pause ended.
    pub(crate) fn write(&mut self, now: Duration) -> bool {
        self.hold = self.hold.max(now + CLOSING);

        if !matches!(self.phase, Phase::Pause(_)) {
            return false;
        }

        self.phase = Phase::Burst(now + self.random.between(BURST_SHORTEST, BURST_LONGEST));

        true
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
        let mut now = FIRST;
        let mut beats = Vec::new();

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
            let mut now = FIRST;

            let pause = loop {
                let (beat, next) = rhythm.beat(now);

                if beat == Beat::Rest {
                    break next;
                }

                now = next;
            };

            let written = now + Duration::from_secs(1);

            assert!(written < pause);
            assert!(rhythm.write(written));
            assert!(!rhythm.write(written));

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
    fn channels_declare_their_indicator_lifetime() {
        assert_eq!(TELEGRAM.refresh(), Duration::from_secs(4));

        assert_eq!(Indicator { lifetime: 2, stop: false }.refresh(), Duration::from_secs(1));
        assert_eq!(TELEGRAM.pauses().unwrap().0, Duration::from_secs(7));
        assert_eq!(DISCORD.pauses().unwrap().0, Duration::from_secs(12));
        assert_eq!(SLACK.pauses().unwrap().0, Duration::from_secs(2));
        assert_eq!(WHATSAPP.pauses(), None);

        for lifetime in [1, 5, LIFETIME_LONGEST] {
            assert!(Indicator { lifetime, stop: false }.validate().is_ok());
        }

        for lifetime in [0, LIFETIME_LONGEST + 1] {
            assert!(Indicator { lifetime, stop: true }.validate().is_err());
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

        assert!(!rhythm.write(Duration::from_secs(1)));

        let beats = schedule(None, 9, Duration::from_secs(60));

        assert_eq!(beats.len(), 15);

        for (index, (beat, at)) in beats.iter().enumerate() {
            assert_eq!(*beat, Beat::Show);
            assert_eq!(*at, FIRST + REFRESH * u32::try_from(index).unwrap());
        }
    }
}
