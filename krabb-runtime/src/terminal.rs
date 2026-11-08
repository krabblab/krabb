use std::{
    io::{self, IsTerminal, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

pub(crate) fn progress(message: impl Into<String>, json: bool) -> Option<Spinner> {
    progress_to(message.into(), json, io::stderr().is_terminal(), Box::new(io::stderr()))
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
        if !io::stderr().is_terminal() {
            return None;
        }

        Some(Self::spawn(message.into(), Box::new(io::stderr())))
    }

    fn spawn(message: String, writer: Box<dyn Write + Send>) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let writer = Arc::new(Mutex::new(writer));

        if let Ok(mut writer) = writer.lock() {
            let _ = write!(writer, "\r⠋ {message}");
            let _ = writer.flush();
        }

        let worker_writer = Arc::clone(&writer);
        let worker = thread::spawn(move || {
            const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut frame = 1;

            while worker_running.load(Ordering::Relaxed) {
                if let Ok(mut writer) = worker_writer.lock() {
                    let _ = write!(writer, "\r{} {message}", FRAMES[frame]);
                    let _ = writer.flush();
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

    use super::progress_to;

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
}
