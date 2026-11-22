//! Terminal text styles in the brand palette from DESIGN.md, readable on dark and light themes.

use std::io::IsTerminal;
use std::sync::OnceLock;

/// Coral, for text on dark backgrounds.
pub(crate) const CORAL: (u8, u8, u8) = (255, 111, 44);
/// Coral Deep, for text on light backgrounds where Coral lacks contrast.
pub(crate) const CORAL_DEEP: (u8, u8, u8) = (184, 72, 11);

static LIGHT: OnceLock<bool> = OnceLock::new();

/// Whether output may use color: both streams are terminals that allow it. Asks the terminal
/// for its background once, so call it before entering raw mode.
pub(crate) fn colors() -> bool {
    let enabled = std::io::stdout().is_terminal() && std::io::stderr().is_terminal() && allowed();

    if enabled {
        light();
    }

    enabled
}

/// Whether the environment allows color, following `NO_COLOR` and monochrome terminals.
pub(crate) fn allowed() -> bool {
    std::env::var_os("NO_COLOR").is_none()
        && !std::env::var("TERM")
            .is_ok_and(|term| matches!(term.as_str(), "dumb" | "mono" | "vt100"))
}

/// The accent color for this terminal's background.
pub(crate) fn coral() -> (u8, u8, u8) {
    if light() { CORAL_DEEP } else { CORAL }
}

/// The accent color from the environment alone, for styles built on every run, such as
/// argument parsing, where asking the terminal would slow every command.
pub(crate) fn coral_from_environment() -> (u8, u8, u8) {
    let light = LIGHT.get().copied().or_else(environment_light).unwrap_or(false);

    if light { CORAL_DEEP } else { CORAL }
}

/// The escape that starts bold accent text.
pub(crate) fn accent_code() -> String {
    let (red, green, blue) = coral();

    format!("\x1b[1;38;2;{red};{green};{blue}m")
}

/// Headers, prompts, and markers: bold in the accent color.
pub(crate) fn accent(text: &str, color: bool) -> String {
    if color { format!("{}{text}\x1b[0m", accent_code()) } else { text.to_owned() }
}

/// Values and names that should stand out without the accent.
pub(crate) fn strong(text: &str, color: bool) -> String {
    if color { format!("\x1b[1m{text}\x1b[0m") } else { text.to_owned() }
}

/// Hints and secondary text, faint in the terminal's own foreground so it follows the theme.
pub(crate) fn dim(text: &str, color: bool) -> String {
    if color { format!("\x1b[2m{text}\x1b[0m") } else { text.to_owned() }
}

/// Examples, faint and italic.
pub(crate) fn example(text: &str, color: bool) -> String {
    if color { format!("\x1b[2;3m{text}\x1b[0m") } else { text.to_owned() }
}

fn light() -> bool {
    *LIGHT.get_or_init(|| environment_light().or_else(query).unwrap_or(false))
}

fn environment_light() -> Option<bool> {
    fgbg_light(&std::env::var("COLORFGBG").ok()?)
}

/// Reads the background index from `COLORFGBG`, such as `15;0`, where 7 and 15 are light.
fn fgbg_light(value: &str) -> Option<bool> {
    let background = value.rsplit(';').next()?.parse::<u8>().ok()?;

    Some(matches!(background, 7 | 15))
}

/// Reads a terminal's `OSC 11` reply, such as `rgb:ffff/ffff/ffff`, as light or dark.
fn reply_light(reply: &str) -> Option<bool> {
    let start = reply.find("rgb:")? + 4;
    let end = reply[start..].find(['\x07', '\x1b']).map_or(reply.len(), |end| start + end);

    let channels = reply[start..end]
        .split('/')
        .map(|channel| {
            let digits = channel.get(..channel.len().min(4))?;
            let value = u32::from_str_radix(digits, 16).ok()?;
            let max = (1u32 << (4 * digits.len() as u32)) - 1;

            (!digits.is_empty()).then(|| f64::from(value) / f64::from(max))
        })
        .collect::<Option<Vec<_>>>()?;

    let [red, green, blue] = channels[..] else {
        return None;
    };

    Some(0.2126 * red + 0.7152 * green + 0.0722 * blue > 0.5)
}

/// Asks the terminal for its background color, followed by a device-attributes query that
/// every terminal answers, so an unsupported query ends promptly instead of timing out. A
/// terminal that starts an escape reply gets up to a second to finish, and anything left unread
/// is discarded so it never reaches a later prompt.
#[cfg(unix)]
fn query() -> Option<bool> {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    if cfg!(test)
        || !std::io::stdin().is_terminal()
        || crossterm::terminal::is_raw_mode_enabled().ok()?
    {
        return None;
    }

    let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok()?;

    crossterm::terminal::enable_raw_mode().ok()?;

    let tty = Cooked(tty);
    let mut tty = &tty.0;

    tty.write_all(b"\x1b]11;?\x1b\\\x1b[c").ok()?;
    tty.flush().ok()?;

    let start = Instant::now();
    let mut deadline = start + Duration::from_millis(200);
    let mut reply = Vec::new();
    let mut buffer = [0; 64];

    while reply.len() < 256 {
        let left = deadline.saturating_duration_since(Instant::now());

        if left.is_zero() {
            break;
        }

        let timeout =
            rustix::event::Timespec { tv_sec: 0, tv_nsec: i64::from(left.subsec_nanos()) };

        let mut fds = [rustix::event::PollFd::new(&tty, rustix::event::PollFlags::IN)];

        if rustix::event::poll(&mut fds, Some(&timeout)).ok()? == 0 {
            break;
        }

        let read = tty.read(&mut buffer).ok()?;

        reply.extend_from_slice(&buffer[..read]);

        if reply.contains(&0x1b) {
            deadline = start + Duration::from_secs(1);
        }

        if read == 0 || answered(&reply) {
            break;
        }
    }

    reply_light(&String::from_utf8_lossy(&reply))
}

#[cfg(not(unix))]
fn query() -> Option<bool> {
    None
}

/// Whether the device-attributes reply, `ESC [ ? … c`, has arrived.
#[cfg(unix)]
fn answered(reply: &[u8]) -> bool {
    reply
        .windows(3)
        .position(|window| window == b"\x1b[?")
        .is_some_and(|start| reply[start..].contains(&b'c'))
}

/// Discards unread reply bytes and leaves raw mode however the query ends.
#[cfg(unix)]
struct Cooked(std::fs::File);

#[cfg(unix)]
impl Drop for Cooked {
    fn drop(&mut self) {
        let _ = rustix::termios::tcflush(&self.0, rustix::termios::QueueSelector::IFlush);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backgrounds_read_as_light_or_dark() {
        assert_eq!(fgbg_light("0;15"), Some(true));
        assert_eq!(fgbg_light("15;default;0"), Some(false));
        assert_eq!(fgbg_light("default"), None);

        assert_eq!(reply_light("\x1b]11;rgb:ffff/ffff/ffff\x1b\\"), Some(true));
        assert_eq!(reply_light("\x1b]11;rgb:1e1e/2323/2727\x07"), Some(false));
        assert_eq!(reply_light("\x1b]11;rgb:f4/f1/ec\x07"), Some(true));
        assert_eq!(reply_light("\x1b[?62;22c"), None);
        assert_eq!(reply_light("rgb:zz/00/00"), None);
    }

    #[test]
    fn plain_text_stays_plain_without_color() {
        for style in [accent, strong, dim, example] {
            assert_eq!(style("Pinch", false), "Pinch");
        }

        assert!(dim("hint", true).starts_with("\x1b[2m"));
        assert!(example("Pinch", true).starts_with("\x1b[2;3m"));
    }
}
