//! Turns pasted file paths and clipboard images into attachments sent with the next message.

use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

/// The host reads at most this much of one attachment.
pub(crate) const LIMIT: u64 = 4 * 1024 * 1024;

/// Attachments one message may carry.
pub(crate) const MAX: usize = 8;

const AUDIO: &[&str] = &["aac", "flac", "m4a", "mp3", "oga", "ogg", "opus", "wav"];

static CLIPBOARD: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Attachment {
    pub path: PathBuf,
    pub name: String,
    /// Removed after sending because Krabb created it from the clipboard.
    #[serde(default)]
    pub temporary: bool,
}

impl Attachment {
    fn file(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".into());

        Self { path, name, temporary: false }
    }

    pub(crate) fn audio(&self) -> bool {
        Path::new(&self.name)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| AUDIO.contains(&extension.to_ascii_lowercase().as_str()))
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Line {
    text: String,
    attachments: Vec<Attachment>,
}

/// Encodes a submitted message for the session engine.
pub(crate) fn encode(text: &str, attachments: &[Attachment]) -> String {
    if attachments.is_empty() {
        return serde_json::to_string(text).unwrap_or_default();
    }

    let line = Line { text: text.into(), attachments: attachments.to_vec() };
    serde_json::to_string(&line).unwrap_or_default()
}

/// Decodes a message line from the terminal UI, or `None` when it carries no attachments.
pub(crate) fn decode(line: &str) -> Option<(String, Vec<Attachment>)> {
    let line = serde_json::from_str::<Line>(line).ok()?;
    Some((line.text, line.attachments.into_iter().take(MAX).collect()))
}

/// Reads pasted text as attachments when every word is an absolute path or link to a file.
pub(crate) fn pasted(text: &str) -> Option<Vec<Attachment>> {
    let text = text.trim();

    if text.is_empty() || text.contains('\n') {
        return None;
    }

    let words = words(text)?;

    let paths = words.iter().map(|word| resolve(word)).collect::<Option<Vec<_>>>()?;

    Some(paths.into_iter().map(Attachment::file).collect())
}

/// Reads the path given to `/attach`, relative to the directory Krabb was started in.
pub(crate) fn named(text: &str, base: &Path) -> Result<Attachment, String> {
    let words = words(text.trim()).filter(|words| words.len() == 1);
    let word = words.and_then(|mut words| words.pop()).ok_or("Usage: /attach <path>.")?;
    let path = Path::new(&word);
    let path = if path.is_absolute() { path.to_path_buf() } else { base.join(path) };

    resolve(&path.to_string_lossy())
        .map(Attachment::file)
        .ok_or_else(|| format!("{word} is not a file Krabb can read."))
}

/// Deletes the clipboard images Krabb saved once the message carrying them is handled.
pub(crate) struct Cleanup<'a>(pub &'a [Attachment]);

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        for attachment in self.0.iter().filter(|attachment| attachment.temporary) {
            let _ = std::fs::remove_file(&attachment.path);
        }
    }
}

/// Splits pasted text the way a shell would: quotes group words and backslashes escape spaces.
fn words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut characters = text.chars();
    let mut started = false;

    while let Some(character) = characters.next() {
        match (quote, character) {
            (Some(open), character) if character == open => quote = None,
            (Some(_), character) => word.push(character),

            (None, '\'' | '"') => {
                quote = Some(character);
                started = true;
            }

            (None, '\\') if cfg!(not(windows)) => {
                word.push(characters.next()?);
                started = true;
            }

            (None, character) if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }

            (None, character) => {
                word.push(character);
                started = true;
            }
        }
    }

    if quote.is_some() {
        return None;
    }

    if started {
        words.push(word);
    }

    (!words.is_empty() && words.len() <= MAX).then_some(words)
}

fn resolve(word: &str) -> Option<PathBuf> {
    let word = match word.strip_prefix("file://") {
        Some(path) => decode_percent(path)?,
        None => word.to_owned(),
    };

    let path = PathBuf::from(word);

    if !path.is_absolute() {
        return None;
    }

    let metadata = std::fs::metadata(&path).ok()?;

    metadata.is_file().then_some(path)
}

fn decode_percent(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;

            output.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(output).ok()
}

/// Saves the clipboard image as a temporary PNG to attach.
pub(crate) fn clipboard() -> Result<Attachment, String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|_| "The clipboard is unavailable in this terminal session.".to_owned())?;

    let image = clipboard.get_image().map_err(|_| {
        "The clipboard has no image. Paste a file path to attach a file.".to_owned()
    })?;

    let width = u32::try_from(image.width).map_err(|_| "The clipboard image is too large.")?;
    let height = u32::try_from(image.height).map_err(|_| "The clipboard image is too large.")?;

    let pixels = image::RgbaImage::from_raw(width, height, image.bytes.into_owned())
        .ok_or_else(|| "The clipboard image could not be read.".to_owned())?;

    let mut png = Cursor::new(Vec::new());

    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|_| "The clipboard image could not be saved.".to_owned())?;

    let png = png.into_inner();

    if png.len() as u64 > LIMIT {
        return Err("The clipboard image is larger than 4 MiB.".into());
    }

    let sequence = CLIPBOARD.fetch_add(1, Ordering::Relaxed) + 1;
    let name = format!("clipboard-{sequence}.png");
    let mut nonce = [0_u8; 8];

    getrandom::fill(&mut nonce).map_err(|_| "The clipboard image could not be saved.")?;

    let nonce = nonce.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let directory = std::env::var_os("KRABB_HOME")
        .map(|home| PathBuf::from(home).join("run"))
        .unwrap_or_else(std::env::temp_dir);
    let path = directory.join(format!("clipboard-{nonce}.png"));

    krabb_file::save(&path, png).map_err(|_| "The clipboard image could not be saved.")?;

    Ok(Attachment { path, name, temporary: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pasted_paths_like_a_shell() {
        let root = std::env::temp_dir().join(format!("krabb-tui-paste-{}", std::process::id()));
        let spaced = root.join("my shot.png");
        let note = root.join("note.txt");

        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&spaced, b"png").unwrap();
        std::fs::write(&note, b"note").unwrap();

        let escaped = format!("{}/my\\ shot.png", root.display());
        let quoted = format!("'{}' \"{}\"", spaced.display(), note.display());
        let link = format!("file://{}/my%20shot.png", root.display());

        assert_eq!(pasted(&escaped).unwrap()[0].path, spaced);
        assert_eq!(pasted(&link).unwrap()[0].name, "my shot.png");
        assert!(pasted("note.txt").is_none());

        let both = pasted(&quoted).unwrap();

        assert_eq!(
            both.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(),
            ["my shot.png", "note.txt"]
        );

        assert!(pasted(&format!("{} and more", note.display())).is_none());
        assert!(pasted("'/unterminated").is_none());
        assert!(pasted(&root.display().to_string()).is_none());
        assert!(pasted(&format!("{0}\n{0}", note.display())).is_none());
        assert!(pasted("   ").is_none());

        assert_eq!(named("note.txt", &root).unwrap().path, note);
        assert_eq!(named(&escaped, &root).unwrap().path, spaced);
        assert_eq!(
            named("missing.txt", &root).unwrap_err(),
            "missing.txt is not a file Krabb can read."
        );

        assert_eq!(named("", &root).unwrap_err(), "Usage: /attach <path>.");
        assert!(named("a b", &root).is_err());

        let temporary = root.join("clip.png");

        std::fs::write(&temporary, b"png").unwrap();

        let attachment =
            Attachment { path: temporary.clone(), name: "clip.png".into(), temporary: true };

        drop(Cleanup(&[attachment, Attachment::file(note.clone())]));

        assert!(!temporary.exists());
        assert!(note.exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn encodes_messages_for_the_engine() {
        let attachment =
            Attachment { path: "/tmp/a.ogg".into(), name: "a.ogg".into(), temporary: true };

        assert_eq!(encode("hello", &[]), "\"hello\"");
        assert!(decode(&encode("hello", &[])).is_none());

        let line = encode("look", std::slice::from_ref(&attachment));

        assert_eq!(decode(&line), Some(("look".into(), vec![attachment.clone()])));
        assert!(attachment.audio());
        assert!(!Attachment::file("/tmp/a.pdf".into()).audio());
        assert_eq!(decode_percent("a%2Fb").as_deref(), Some("a/b"));
        assert!(decode_percent("a%2").is_none());
    }
}
