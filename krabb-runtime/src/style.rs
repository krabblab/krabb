//! Terminal text styles in the brand palette from DESIGN.md. Plain text uses the terminal's
//! own foreground, so it stays readable on dark and light themes alike.

use std::io::IsTerminal;

/// Coral, the brand accent.
pub(crate) const CORAL: (u8, u8, u8) = (255, 111, 44);

const ACCENT: &str = "\x1b[1;38;2;255;111;44m";

/// Whether output may use color: both streams are terminals that allow it.
pub(crate) fn colors() -> bool {
    std::io::stdout().is_terminal() && std::io::stderr().is_terminal() && allowed()
}

/// Whether the environment allows color, following `NO_COLOR` and monochrome terminals.
pub(crate) fn allowed() -> bool {
    std::env::var_os("NO_COLOR").is_none()
        && !std::env::var("TERM")
            .is_ok_and(|term| matches!(term.as_str(), "dumb" | "mono" | "vt100"))
}

/// The escape that starts bold accent text.
pub(crate) fn accent_code() -> &'static str {
    ACCENT
}

/// Headers, prompts, and markers: bold Coral.
pub(crate) fn accent(text: &str, color: bool) -> String {
    if color { format!("{ACCENT}{text}\x1b[0m") } else { text.to_owned() }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_wrap_text_only_with_color() {
        for style in [accent, strong, dim, example] {
            assert_eq!(style("Pinch", false), "Pinch");
        }

        assert_eq!(accent("Pinch", true), "\x1b[1;38;2;255;111;44mPinch\x1b[0m");
        assert!(dim("hint", true).starts_with("\x1b[2m"));
        assert!(example("Pinch", true).starts_with("\x1b[2;3m"));
    }
}
