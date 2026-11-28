use std::{
    io::Write,
    time::{Duration, Instant},
};

use base64::Engine;

/// How long a copy notice stays, and how long it shows faint before it goes.
const NOTICE_LIFE: Duration = Duration::from_secs(3);
const NOTICE_FADE: Duration = Duration::from_secs(1);

/// The most text sent through OSC 52, below what common terminals accept.
const TERMINAL_LIMIT: usize = 74 * 1024;

const TOO_LARGE: &str = "Could not copy this much text, so select less and try again.";

const UNREACHABLE: &str =
    "Could not reach a clipboard, so hold Shift and select with the terminal.";

/// A short message at the top right of the input area about the last copy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Notice {
    pub(super) text: String,
    pub(super) failed: bool,
    shown: Instant,
}

/// How a notice shows at a given moment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Shade {
    Bright,
    Faint,
}

impl Notice {
    fn new(text: impl Into<String>, failed: bool, shown: Instant) -> Self {
        Self { text: text.into(), failed, shown }
    }

    /// How the notice shows at NOW, or nothing once it has gone.
    pub(super) fn shade(&self, now: Instant) -> Option<Shade> {
        let age = now.saturating_duration_since(self.shown);

        if age >= NOTICE_LIFE {
            None
        } else if age >= NOTICE_LIFE - NOTICE_FADE {
            Some(Shade::Faint)
        } else {
            Some(Shade::Bright)
        }
    }
}

/// The OSC 52 sequence that asks the terminal to put TEXT on the clipboard.
pub(super) fn sequence(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);

    format!("\x1b]52;c;{encoded}\x07")
}

/// Puts TEXT on the system clipboard, keeping it open in CLIPBOARD because some platforms
/// drop copied text when it closes.
pub(super) fn system(clipboard: &mut Option<arboard::Clipboard>, text: &str) -> bool {
    if clipboard.is_none() {
        *clipboard = arboard::Clipboard::new().ok();
    }

    clipboard.as_mut().is_some_and(|clipboard| clipboard.set_text(text).is_ok())
}

/// Copies TEXT through the terminal's OSC 52 sequence and through SYSTEM.
pub(super) fn copy(
    text: &str,
    terminal: &mut impl Write,
    system: impl FnOnce(&str) -> bool,
    now: Instant,
) -> Notice {
    let fits = text.len() <= TERMINAL_LIMIT;

    let through_terminal = fits
        && terminal.write_all(sequence(text).as_bytes()).and_then(|()| terminal.flush()).is_ok();

    let through_system = system(text);

    if through_terminal || through_system {
        notice(text, now)
    } else if fits {
        Notice::new(UNREACHABLE, true, now)
    } else {
        Notice::new(TOO_LARGE, true, now)
    }
}

fn notice(text: &str, now: Instant) -> Notice {
    let count = text.chars().count();
    let noun = if count == 1 { "character" } else { "characters" };

    Notice::new(format!("Copied {count} {noun}"), false, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_the_exact_osc_52_clipboard_sequence() {
        assert_eq!(sequence("hi\nthere 界"), "\x1b]52;c;aGkKdGhlcmUg55WM\x07");
        assert_eq!(sequence(""), "\x1b]52;c;\x07");
    }

    #[test]
    fn counts_characters_and_fades_before_it_goes() {
        let now = Instant::now();
        let copied = notice("a界\n", now);

        assert_eq!(copied.text, "Copied 3 characters");
        assert!(!copied.failed);
        assert_eq!(notice("x", now).text, "Copied 1 character");

        assert_eq!(copied.shade(now), Some(Shade::Bright));
        assert_eq!(copied.shade(now + Duration::from_millis(2500)), Some(Shade::Faint));
        assert_eq!(copied.shade(now + NOTICE_LIFE), None);
    }

    struct Closed;

    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writes_the_sequence_and_explains_a_failed_copy() {
        let now = Instant::now();
        let mut written = Vec::new();

        let copied = copy("hello", &mut written, |_| false, now);

        assert_eq!(written, sequence("hello").into_bytes());
        assert_eq!(copied.text, "Copied 5 characters");

        let large = "x".repeat(TERMINAL_LIMIT + 1);
        let mut written = Vec::new();

        let refused = copy(&large, &mut written, |_| false, now);

        assert!(written.is_empty());
        assert!(refused.failed);
        assert_eq!(refused.text, TOO_LARGE);

        let refused = copy("hello", &mut Closed, |_| false, now);

        assert_eq!(refused.text, UNREACHABLE);
        assert_eq!(copy("hello", &mut Closed, |_| true, now).text, "Copied 5 characters");
    }
}
