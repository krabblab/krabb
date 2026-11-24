use std::{
    io::{self, IsTerminal, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use unicode_width::UnicodeWidthChar;

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

fn draw(writer: &mut dyn Write, frame: &str, message: &str, columns: u16) {
    let mut remaining = usize::from(columns.saturating_sub(1));
    let mut line = String::from("\r\x1b[2K");

    for character in frame.chars().chain(" ".chars()).chain(message.chars()) {
        if character.is_control() {
            continue;
        }

        let width = character.width().unwrap_or(0);

        if width > remaining {
            break;
        }

        line.push(character);
        remaining -= width;
    }

    let _ = writer.write_all(line.as_bytes());
    let _ = writer.flush();
}

/// TEXT with control characters other than newlines and tabs shown as `�`, so text from
/// plugins or models cannot move the cursor, rewrite the screen, or set the clipboard.
pub(crate) fn printable(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\n' | '\t' => character,
            character if character.is_control() => '\u{fffd}',
            character => character,
        })
        .collect()
}

pub(crate) fn columns() -> u16 {
    crossterm::terminal::size().map(|(columns, _)| columns).unwrap_or(80)
}

pub(crate) fn progress(message: impl Into<String>, json: bool) -> Option<Spinner> {
    progress_to(message.into(), json, redraws(), Box::new(io::stderr()))
}

/// Whether progress may redraw its line: stderr is a terminal that understands line controls.
fn redraws() -> bool {
    io::stderr().is_terminal() && !std::env::var("TERM").is_ok_and(|term| term == "dumb")
}

fn progress_to(
    message: String,
    json: bool,
    interactive: bool,
    mut writer: Box<dyn Write + Send>,
) -> Option<Spinner> {
    if json {
        return None;
    }

    if interactive {
        return Some(Spinner::spawn(message, writer));
    }

    let _ = writeln!(writer, "{message}");
    let _ = writer.flush();

    None
}

pub(crate) struct Spinner {
    running: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    writer: SharedWriter,
}

impl Spinner {
    pub(crate) fn start(message: impl Into<String>) -> Option<Self> {
        if !redraws() {
            return None;
        }

        Some(Self::spawn(message.into(), Box::new(io::stderr())))
    }

    fn spawn(message: String, writer: Box<dyn Write + Send>) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let writer = Arc::new(Mutex::new(writer));

        if let Ok(mut writer) = writer.lock() {
            draw(&mut **writer, "⠋", &message, columns());
        }

        let worker_writer = Arc::clone(&writer);
        let worker = thread::spawn(move || {
            const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut frame = 1;

            while worker_running.load(Ordering::Relaxed) {
                if let Ok(mut writer) = worker_writer.lock() {
                    draw(&mut **writer, FRAMES[frame], &message, columns());
                }

                frame = (frame + 1) % FRAMES.len();
                thread::sleep(Duration::from_millis(90));
            }
        });

        Self { running, worker: Some(worker), writer }
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);

        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }

        if let Ok(mut writer) = self.writer.lock() {
            let _ = write!(writer, "\r\x1b[2K");
            let _ = writer.flush();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
    };

    use unicode_width::UnicodeWidthStr;

    use super::{draw, printable, progress_to};

    #[test]
    fn frames_fit_narrow_terminals_and_preserve_unicode() {
        for columns in [0, 1, 2, 3, 4, 8, 40, 80, 120] {
            let mut output = Vec::new();

            draw(&mut output, "⠋", "Updating 界e\u{301}🦀 plugin...", columns);

            let output = String::from_utf8(output).unwrap();
            let line = output.strip_prefix("\r\x1b[2K").unwrap();

            assert!(line.width() <= usize::from(columns.saturating_sub(1)));
            assert!(!line.contains(['\r', '\n', '\x1b']));
        }

        let mut output = Vec::new();

        draw(&mut output, "⠋", "界e\u{301}", 7);

        assert_eq!(String::from_utf8(output).unwrap(), "\r\x1b[2K⠋ 界e\u{301}");
    }

    #[test]
    fn shorter_frames_clear_previous_text() {
        let mut output = Vec::new();

        draw(&mut output, "⠋", "Updating plugins...", 80);
        draw(&mut output, "⠙", "Done", 80);

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\r\x1b[2K⠋ Updating plugins...\r\x1b[2K⠙ Done"
        );
    }

    #[test]
    fn frames_do_not_emit_message_control_characters() {
        let mut output = Vec::new();

        draw(&mut output, "⠋", "Updating\nplugin\r\t\x1b\x07", 80);

        assert_eq!(String::from_utf8(output).unwrap(), "\r\x1b[2K⠋ Updatingplugin");
    }

    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);

            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn spinner_starts_immediately_and_clears_when_dropped() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let spinner = progress_to(
            "Installing plugins...".into(),
            false,
            true,
            Box::new(Buffer(Arc::clone(&output))),
        );

        assert!(String::from_utf8(output.lock().unwrap().clone()).unwrap().contains("Installing"));

        drop(spinner);

        let output = String::from_utf8(output.lock().unwrap().clone()).unwrap();

        assert!(output.contains("⠋ Installing plugins..."));
        assert!(output.ends_with("\r\x1b[2K"));
        assert!(!output.contains('\n'));
    }

    #[test]
    fn redirected_progress_is_one_plain_line() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let spinner = progress_to(
            "Updating plugins...".into(),
            false,
            false,
            Box::new(Buffer(Arc::clone(&output))),
        );

        assert!(spinner.is_none());
        assert_eq!(*output.lock().unwrap(), b"Updating plugins...\n");
    }

    #[test]
    fn json_progress_is_silent_in_terminals_and_pipes() {
        for interactive in [true, false] {
            let output = Arc::new(Mutex::new(Vec::new()));
            let spinner = progress_to(
                "Uninstalling plugins...".into(),
                true,
                interactive,
                Box::new(Buffer(Arc::clone(&output))),
            );

            assert!(spinner.is_none());
            assert!(output.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn printable_text_keeps_lines_and_neutralizes_controls() {
        assert_eq!(printable("one\n\ttwo"), "one\n\ttwo");
        assert_eq!(printable("a\u{1b}]52;c;eA==\u{7}b\r"), "a\u{fffd}]52;c;eA==\u{fffd}b\u{fffd}");
    }
}
