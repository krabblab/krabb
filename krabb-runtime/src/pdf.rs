//! Extracts bounded text from inbound PDF attachments in a disposable child process.

use std::{
    io::{Read, Write},
    path::Path,
    process::{ExitCode, Stdio},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Semaphore,
    time::Instant,
};

pub(crate) const MAGIC: &[u8] = b"%PDF-";
pub(crate) const TIME: Duration = Duration::from_secs(30);

const ARGUMENT: &str = "__pdf";
const WHOLE: &[u8] = b"krabb-pdf whole\n";
const PART: &[u8] = b"krabb-pdf part\n";

// Parsing is unbounded inside the PDF library, so a child holds the memory and is killed on time.
#[cfg(target_os = "linux")]
const MEMORY: u64 = 1024 * 1024 * 1024;

static SLOT: Semaphore = Semaphore::const_new(1);

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Text {
    Found { text: String, truncated: bool },
    Empty,
    Unreadable,
    Slow,
}

pub(crate) async fn read(path: &Path, deadline: Instant) -> Text {
    let Ok(bytes) = tokio::fs::read(path).await else {
        return Text::Unreadable;
    };

    let Ok(program) = std::env::current_exe() else {
        return Text::Unreadable;
    };

    let mut command = Command::new(program);
    command.arg(ARGUMENT);

    run(command, bytes, deadline).await
}

async fn run(mut command: Command, bytes: Vec<u8>, deadline: Instant) -> Text {
    let Ok(_slot) = tokio::time::timeout_at(deadline, SLOT.acquire()).await else {
        return Text::Slow;
    };

    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);

    let Ok(mut child) = command.spawn() else {
        tracing::warn!("PDF reader could not start.");
        return Text::Unreadable;
    };

    let (Some(mut input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
        return Text::Unreadable;
    };

    let exchange = async {
        let _ = input.write_all(&bytes).await;
        drop(input);

        let mut reply = Vec::new();
        let limit = (PART.len().max(WHOLE.len()) + crate::FILE_LIMIT as usize) as u64;
        output.take(limit).read_to_end(&mut reply).await?;

        Ok::<_, std::io::Error>((child.wait().await?, reply))
    };

    match tokio::time::timeout_at(deadline, exchange).await {
        Ok(Ok((status, reply))) if status.success() => parse(reply),

        Ok(_) => {
            tracing::warn!("PDF text extraction failed.");
            Text::Unreadable
        }

        Err(_) => {
            tracing::warn!("PDF text extraction timed out.");
            Text::Slow
        }
    }
}

fn parse(reply: Vec<u8>) -> Text {
    let (truncated, body) = if let Some(body) = reply.strip_prefix(WHOLE) {
        (false, body)
    } else if let Some(body) = reply.strip_prefix(PART) {
        (true, body)
    } else {
        return Text::Unreadable;
    };

    match std::str::from_utf8(body) {
        Ok("") => Text::Empty,
        Ok(text) => Text::Found { text: text.to_owned(), truncated },
        Err(_) => Text::Unreadable,
    }
}

/// Serves one extraction when the executable was started as a PDF reader.
pub fn child() -> Option<ExitCode> {
    if std::env::args_os().nth(1).is_none_or(|argument| argument != ARGUMENT) {
        return None;
    }

    limit();

    let mut bytes = Vec::new();
    let read = std::io::stdin().take(crate::MEDIA_LIMIT + 1).read_to_end(&mut bytes);

    if read.is_err() || bytes.len() as u64 > crate::MEDIA_LIMIT {
        return Some(ExitCode::FAILURE);
    }

    let Some(reply) = extract(&bytes) else {
        return Some(ExitCode::FAILURE);
    };

    let mut output = std::io::stdout().lock();

    match output.write_all(&reply).and_then(|()| output.flush()) {
        Ok(()) => Some(ExitCode::SUCCESS),
        Err(_) => Some(ExitCode::FAILURE),
    }
}

#[cfg(target_os = "linux")]
fn limit() {
    let memory = rustix::process::Rlimit { current: Some(MEMORY), maximum: Some(MEMORY) };
    let _ = rustix::process::setrlimit(rustix::process::Resource::Data, memory);
}

#[cfg(not(target_os = "linux"))]
fn limit() {}

fn extract(bytes: &[u8]) -> Option<Vec<u8>> {
    let text = pdf_extract::extract_text_from_mem(bytes).ok()?;
    let (text, truncated) = clean(&text, crate::FILE_LIMIT as usize);
    let mut reply = if truncated { PART.to_vec() } else { WHOLE.to_vec() };

    reply.extend(text.bytes());
    Some(reply)
}

fn clean(text: &str, limit: usize) -> (String, bool) {
    let mut output = String::new();

    for character in text.trim().chars() {
        let character = match character {
            '\n' | '\t' => character,
            '\u{c}' => '\n',
            character if character.is_control() => ' ',
            character => character,
        };

        if output.len() + character.len_utf8() > limit {
            return (output, true);
        }

        output.push(character);
    }

    (output, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: Option<&str>) -> Vec<u8> {
        let stream =
            text.map_or_else(String::new, |text| format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET"));

        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_owned(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        ];

        let mut output = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();

        for (index, object) in objects.iter().enumerate() {
            offsets.push(output.len());
            output.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).bytes());
        }

        let start = output.len();
        let size = objects.len() + 1;

        output.extend(format!("xref\n0 {size}\n0000000000 65535 f \n").bytes());

        for offset in offsets {
            output.extend(format!("{offset:010} 00000 n \n").bytes());
        }

        output.extend(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n").bytes(),
        );

        output
    }

    #[test]
    fn extracts_text_and_marks_empty_pages() {
        let reply = extract(&document(Some("Quarterly revenue grew."))).unwrap();

        assert!(reply.starts_with(WHOLE));
        assert!(String::from_utf8(reply).unwrap().contains("Quarterly revenue grew."));

        assert_eq!(parse(extract(&document(None)).unwrap()), Text::Empty);
        assert!(extract(b"%PDF-1.7\nbroken").is_none());
    }

    #[test]
    fn cleans_and_bounds_text() {
        assert_eq!(
            clean("Page one\u{c}Page\u{0}two\r\n", 64),
            ("Page one\nPage two".into(), false)
        );

        assert_eq!(clean(" \n\u{c} ", 64), (String::new(), false));
        assert_eq!(clean("abcé", 4), ("abc".into(), true));
    }

    #[test]
    fn parses_only_framed_replies() {
        assert_eq!(
            parse(b"krabb-pdf part\nabc".to_vec()),
            Text::Found { text: "abc".into(), truncated: true }
        );

        assert_eq!(parse(b"krabb-pdf whole\n".to_vec()), Text::Empty);
        assert_eq!(parse(b"running 0 tests\n".to_vec()), Text::Unreadable);
        assert_eq!(parse(b"krabb-pdf whole\n\xff".to_vec()), Text::Unreadable);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounds_the_reader_process() {
        let shell = |script: &str| {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            command
        };

        let soon = || Instant::now() + Duration::from_secs(10);

        assert_eq!(
            run(shell("cat >/dev/null; printf 'krabb-pdf whole\\nhello'"), vec![1; 1024], soon())
                .await,
            Text::Found { text: "hello".into(), truncated: false }
        );

        assert_eq!(run(shell("exit 1"), Vec::new(), soon()).await, Text::Unreadable);
        assert_eq!(
            run(Command::new("/missing/reader"), Vec::new(), soon()).await,
            Text::Unreadable
        );

        let started = std::time::Instant::now();
        let deadline = Instant::now() + Duration::from_millis(200);

        assert_eq!(run(shell("sleep 30"), Vec::new(), deadline).await, Text::Slow);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
