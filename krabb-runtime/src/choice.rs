//! A vertical list picker for the terminal: arrow keys move, a digit jumps, Enter chooses.

use std::io::Write;

use crate::style::{accent, colors, dim};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

const HINT: &str = "↑/↓ to move, Enter to choose";

/// What a key press did to the selection.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Moved,
    Chosen(usize),
    Cancelled,
    Ignored,
}

/// Which option is selected out of LEN.
struct Picker {
    selected: usize,
    len: usize,
}

impl Picker {
    fn key(&mut self, key: KeyEvent) -> Outcome {
        if key.kind == KeyEventKind::Release {
            return Outcome::Ignored;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.checked_sub(1).unwrap_or(self.len - 1);
                Outcome::Moved
            }

            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1) % self.len;
                Outcome::Moved
            }

            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Outcome::Cancelled
            }

            KeyCode::Char(digit @ '1'..='9') => {
                let index = usize::from(digit as u8 - b'1');

                if index < self.len {
                    self.selected = index;
                    Outcome::Moved
                } else {
                    Outcome::Ignored
                }
            }

            KeyCode::Enter => Outcome::Chosen(self.selected),
            KeyCode::Esc => Outcome::Cancelled,
            _ => Outcome::Ignored,
        }
    }

    /// The option lines, then the hint after a blank line.
    fn lines(&self, options: &[&str], color: bool) -> Vec<String> {
        let mut lines = options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                if index == self.selected {
                    accent(&format!("› {option}"), color)
                } else {
                    format!("  {option}")
                }
            })
            .collect::<Vec<_>>();

        lines.push(String::new());
        lines.push(dim(HINT, color));
        lines
    }
}

/// Restores the terminal however the picker ends.
struct Raw;

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stderr(), crossterm::cursor::Show);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Shows OPTIONS under the current line and returns the chosen index, or `None` when the
/// person cancels. The list is replaced by the chosen option once picked.
pub(crate) fn pick(options: &[&str], selected: usize) -> std::io::Result<Option<usize>> {
    if options.is_empty() {
        return Ok(None);
    }

    let mut picker = Picker { selected: selected.min(options.len() - 1), len: options.len() };
    let mut output = std::io::stderr();
    let color = colors();

    crossterm::terminal::enable_raw_mode()?;

    let _raw = Raw;

    crossterm::execute!(output, crossterm::cursor::Hide)?;
    draw(&mut output, &picker.lines(options, color), false)?;

    loop {
        let Event::Key(key) = crossterm::event::read()? else {
            continue;
        };

        match picker.key(key) {
            Outcome::Moved => draw(&mut output, &picker.lines(options, color), true)?,

            Outcome::Ignored => {}

            Outcome::Chosen(index) => {
                clear(&mut output, options.len() + 2)?;
                write!(output, "{}\r\n", accent(&format!("› {}", options[index]), color))?;
                output.flush()?;
                return Ok(Some(index));
            }

            Outcome::Cancelled => {
                clear(&mut output, options.len() + 2)?;
                output.flush()?;
                return Ok(None);
            }
        }
    }
}

fn draw(output: &mut impl Write, lines: &[String], redraw: bool) -> std::io::Result<()> {
    if redraw {
        clear(output, lines.len())?;
    }

    for line in lines {
        write!(output, "{line}\r\n")?;
    }

    output.flush()
}

fn clear(output: &mut impl Write, rows: usize) -> std::io::Result<()> {
    let rows = u16::try_from(rows).unwrap_or(u16::MAX);

    crossterm::queue!(
        output,
        crossterm::cursor::MoveUp(rows),
        crossterm::cursor::MoveToColumn(0),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn keys_move_wrap_jump_and_choose() {
        let mut picker = Picker { selected: 0, len: 3 };

        assert_eq!(picker.key(press(KeyCode::Up)), Outcome::Moved);
        assert_eq!(picker.selected, 2);
        assert_eq!(picker.key(press(KeyCode::Down)), Outcome::Moved);
        assert_eq!(picker.selected, 0);
        assert_eq!(picker.key(press(KeyCode::Char('j'))), Outcome::Moved);
        assert_eq!(picker.selected, 1);

        assert_eq!(picker.key(press(KeyCode::Char('3'))), Outcome::Moved);
        assert_eq!(picker.selected, 2);
        assert_eq!(picker.key(press(KeyCode::Char('9'))), Outcome::Ignored);
        assert_eq!(picker.key(press(KeyCode::Char('x'))), Outcome::Ignored);

        assert_eq!(picker.key(press(KeyCode::Enter)), Outcome::Chosen(2));
        assert_eq!(picker.key(press(KeyCode::Esc)), Outcome::Cancelled);

        let interrupt = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        assert_eq!(picker.key(interrupt), Outcome::Cancelled);

        let mut release = press(KeyCode::Down);

        release.kind = KeyEventKind::Release;

        assert_eq!(picker.key(release), Outcome::Ignored);
    }

    #[test]
    fn the_list_marks_the_selection_and_ends_with_the_hint() {
        let picker = Picker { selected: 1, len: 2 };
        let lines = picker.lines(&["OpenAI", "Skip"], false);

        assert_eq!(lines, ["  OpenAI", "› Skip", "", "↑/↓ to move, Enter to choose"]);

        let colored = picker.lines(&["OpenAI", "Skip"], true);

        assert_eq!(colored[1], format!("{}› Skip\x1b[0m", crate::style::accent_code()));
        assert_eq!(colored[3], format!("\x1b[2m{HINT}\x1b[0m"));
    }

    #[test]
    fn drawing_replaces_the_previous_list() {
        let mut output = Vec::new();
        let lines = vec!["a".to_owned(), "b".to_owned()];

        draw(&mut output, &lines, false).unwrap();

        assert_eq!(String::from_utf8_lossy(&output), "a\r\nb\r\n");

        output.clear();
        draw(&mut output, &lines, true).unwrap();

        assert!(String::from_utf8_lossy(&output).ends_with("a\r\nb\r\n"));
        assert!(output.len() > "a\r\nb\r\n".len());
    }
}
