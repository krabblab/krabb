use std::{
    collections::BTreeSet,
    fs, io,
    io::Read,
    path::{Path, PathBuf},
};

use krabb_core::types::{Content, Message};
use sha2::{Digest, Sha256};

use crate::state::Store;

const BYTE_LIMIT: u64 = 64 * 1024 * 1024;
const FILE_LIMIT: usize = 1024;

// The caller holds the session store lock through pinning and durable admission.
pub(super) fn admitted(store: &Store, message: &Message, queued: bool) -> bool {
    store.sessions.get(&message.session).is_some_and(|session| {
        session.status != "cancelled"
            && (session.status == "working" || session.inflight.is_none())
            && !session.messages.iter().any(|item| item.id == message.id)
            && (queued || !session.queued.iter().any(|item| item.id == message.id))
            && (session.status != "working"
                || (!queued
                    && session.queued.len() + usize::from(session.inflight.is_some())
                        < crate::state::LIMIT))
    })
}

pub(super) fn pin(content: Vec<Content>, root: &Path) -> Vec<Content> {
    pin_bounded(content, root, BYTE_LIMIT, FILE_LIMIT)
}

fn pin_bounded(
    content: Vec<Content>,
    root: &Path,
    byte_limit: u64,
    file_limit: usize,
) -> Vec<Content> {
    content
        .into_iter()
        .map(|item| {
            let Content::Image { uri, alt } = &item else {
                return item;
            };

            let Some(source) = crate::media_file(uri, root) else {
                return item;
            };

            if directory(root).is_ok_and(|pinned| source.parent() == Some(pinned.as_path())) {
                return item;
            }

            let Some((_, bytes)) = crate::read_image(&source, crate::IMAGE_LIMIT) else {
                return item;
            };

            match retain(root, &bytes, byte_limit, file_limit) {
                Ok(path) => {
                    Content::Image { uri: format!("file://{}", path.display()), alt: alt.clone() }
                }

                Err(error) => {
                    tracing::warn!(kind = ?error.kind(), "Image retention rejected.");
                    Content::Text {
                        text:
                            "[Image omitted because retained media storage is full or unavailable.]"
                                .into(),
                    }
                }
            }
        })
        .collect()
}

fn directory(root: &Path) -> io::Result<PathBuf> {
    let root = fs::canonicalize(root)?;
    let pinned = root.join("pinned");

    match fs::symlink_metadata(&pinned) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(pinned),
        Ok(_) => Err(io::Error::new(io::ErrorKind::PermissionDenied, "Invalid media directory.")),
        Err(error) => Err(error),
    }
}

fn retain(root: &Path, bytes: &[u8], byte_limit: u64, file_limit: usize) -> io::Result<PathBuf> {
    fs::create_dir_all(root.join("pinned"))?;
    let pinned = directory(root)?;
    let destination = pinned.join(format!("{}.bin", crate::hex_digest(Sha256::digest(bytes))));
    let mut total = 0_u64;
    let mut count = 0_usize;
    let mut exists = false;

    // Recount disk state on every admission, including after restart or a failed write.
    for entry in fs::read_dir(&pinned)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;

        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Invalid retained media."));
        }

        total = total.saturating_add(metadata.len());
        count = count.saturating_add(1);
        exists |= entry.path() == destination;

        if total > byte_limit || count > file_limit {
            return Err(full());
        }
    }

    if exists {
        return Ok(destination);
    }

    if bytes.len() as u64 > byte_limit.saturating_sub(total) || count >= file_limit {
        return Err(full());
    }

    krabb_file::save(&destination, bytes)?;
    Ok(destination)
}

fn full() -> io::Error {
    io::Error::new(io::ErrorKind::StorageFull, "Retained media storage limit reached.")
}

pub(super) fn reclaim<'a>(
    root: &Path,
    store: &Store,
    deferred: impl Iterator<Item = &'a serde_json::Value>,
) -> io::Result<()> {
    let pinned = match directory(root) {
        Ok(pinned) => pinned,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    let mut retained = BTreeSet::new();

    for session in store.sessions.values() {
        for message in session
            .messages
            .iter()
            .chain(session.queued.iter())
            .chain(session.inflight.iter())
            .chain(session.compacted_context.iter().flatten())
        {
            for content in &message.content {
                if let Content::Image { uri, .. }
                | Content::File { uri, .. }
                | Content::Audio { uri, .. } = content
                {
                    reference(uri, &mut retained);
                }
            }
        }
    }

    for event in deferred {
        references(event, &mut retained);
    }

    for entry in fs::read_dir(&pinned)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let managed = name.strip_suffix(".bin").is_some_and(|hash| {
            hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        });

        if managed && entry.file_type()?.is_file() && !retained.contains(&entry.path()) {
            fs::remove_file(entry.path())?;
        }
    }

    Ok(())
}

fn reference(uri: &str, retained: &mut BTreeSet<PathBuf>) {
    if let Some(path) = uri.strip_prefix("file://")
        && let Ok(path) = fs::canonicalize(path)
    {
        retained.insert(path);
    }
}

fn references(value: &serde_json::Value, retained: &mut BTreeSet<PathBuf>) {
    match value {
        serde_json::Value::String(uri) => reference(uri, retained),

        serde_json::Value::Array(items) => {
            for item in items {
                references(item, retained);
            }
        }

        serde_json::Value::Object(items) => {
            for item in items.values() {
                references(item, retained);
            }
        }

        _ => {}
    }
}

#[cfg(test)]
mod tests;

/// Retain conversation attachments in their selected storage before durable admission.
pub(super) fn archive(
    content: Vec<Content>,
    source: &Path,
    destination: &Path,
) -> io::Result<Vec<Content>> {
    if source == destination {
        return Ok(content);
    }

    let mut output = Vec::with_capacity(content.len());

    for mut item in content {
        let uri = match &mut item {
            Content::Image { uri, .. } | Content::File { uri, .. } | Content::Audio { uri, .. } => {
                uri
            }

            _ => {
                output.push(item);
                continue;
            }
        };

        if crate::media_file(uri, destination).is_none() {
            if let Some(path) = crate::media_file(uri, source) {
                let mut bytes = Vec::new();
                fs::File::open(&path)?.take(crate::MEDIA_LIMIT + 1).read_to_end(&mut bytes)?;

                if bytes.len() as u64 > crate::MEDIA_LIMIT {
                    return Err(io::Error::new(
                        io::ErrorKind::FileTooLarge,
                        "Attachment exceeds the media limit.",
                    ));
                }

                let path = retain(destination, &bytes, BYTE_LIMIT, FILE_LIMIT)?;
                *uri = format!("file://{}", path.display());
            } else if uri.starts_with("file://") {
                item =
                    Content::Text { text: "[Attachment unavailable in this conversation.]".into() };
            }
        }

        output.push(item);
    }

    Ok(output)
}

pub(super) fn release(content: &[Content], source: &Path, destination: &Path) {
    if source == destination {
        return;
    }

    for item in content {
        let uri = match item {
            Content::Image { uri, .. } | Content::File { uri, .. } | Content::Audio { uri, .. } => {
                uri
            }

            _ => continue,
        };

        if let Some(path) = crate::media_file(uri, source)
            && path.parent() != Some(source.join("pinned").as_path())
        {
            let _ = fs::remove_file(path);
        }
    }
}
