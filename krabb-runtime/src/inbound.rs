//! Converts inbound channel content and media into conversation messages.

use crate::*;

pub(crate) fn content(event: &serde_json::Value) -> Vec<Content> {
    if let Some(values) = event["content"].as_array() {
        let content = values
            .iter()
            .take(CONTENT_LIMIT)
            .filter_map(|value| serde_json::from_value::<Content>(value.clone()).ok())
            .collect::<Vec<_>>();

        if !content.is_empty() {
            return bound(content);
        }
    }

    event["text"]
        .as_str()
        .filter(|text| !text.is_empty())
        .map(|text| bound(vec![Content::Text { text: text.into() }]))
        .unwrap_or_default()
}

pub(crate) async fn resolve_media(
    channel_id: &str,
    channel: &Live,
    mut content: Vec<Content>,
    call: &mut u64,
) -> Vec<Content> {
    if !matches!(channel_id, "telegram" | "discord" | "whatsapp" | "signal" | "slack") {
        return content;
    }

    for item in &mut content {
        let uri = match item {
            Content::Image { uri, .. } | Content::File { uri, .. } | Content::Audio { uri, .. }
                if media_uri(uri, channel_id) =>
            {
                uri.clone()
            }

            _ => continue,
        };

        let request = Request::call(*call, "media", serde_json::json!({"uri": uri}));
        *call = (*call).saturating_add(1);
        let Ok(response) = channel.call(request).await else {
            warn!(channel = channel_id, "Media could not be resolved, retaining its reference.");
            continue;
        };

        let Some(resolved) = response
            .result
            .and_then(|value| value["uri"].as_str().map(str::to_owned))
            .filter(|value| value.starts_with("file://"))
        else {
            warn!(channel = channel_id, "Media could not be resolved, retaining its reference.");
            continue;
        };

        match item {
            Content::Image { uri, .. } | Content::File { uri, .. } | Content::Audio { uri, .. } => {
                *uri = clip(resolved, META_LIMIT);
            }

            Content::Text { .. } | Content::ToolCall { .. } | Content::Context { .. } => {}
        }
    }

    content
}

pub(crate) async fn transcribe_media(
    plugins: &Plugins,
    channel_id: &str,
    content: Vec<Content>,
    media_root: &Path,
    call: &mut u64,
) -> Vec<Content> {
    if !matches!(channel_id, "telegram" | "discord" | "whatsapp" | "signal" | "slack") {
        return content;
    }

    let speech = plugins.find(Capability::Speech).await;
    let mut output = Vec::with_capacity(content.len());

    for item in content {
        let Content::Audio { uri, .. } = &item else {
            output.push(item);
            continue;
        };

        let Some(path) = media_file(uri, media_root) else {
            warn!(
                channel = channel_id,
                "Voice attachment is unavailable or outside the media cache."
            );
            output.push(unavailable_voice());
            continue;
        };

        let Some((id, plugin)) = &speech else {
            warn!(
                channel = channel_id,
                "Voice transcription is unavailable because no speech plugin is loaded."
            );
            output.push(unavailable_voice());
            continue;
        };

        let request = if media_root.file_name().is_some_and(|name| name == "attachments")
            && media_root.parent().is_some_and(worktree::isolated)
        {
            Request::call(
                *call,
                "scoped",
                serde_json::json!({"workspace": media_root.parent().unwrap(), "method":"transcribe", "params":{"path":path}}),
            )
        } else if media_root
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "attachments")
        {
            Request::call(
                *call,
                "retained",
                serde_json::json!({
                    "workspace": media_root.parent().unwrap().parent().unwrap(),
                    "owner": sharing::principal(channel_id), "path":path
                }),
            )
        } else {
            Request::call(*call, "transcribe", serde_json::json!({"path": path}))
        };

        *call = (*call).saturating_add(1);
        let response = plugin.call(request).await;
        let transcript = match response {
            Ok(response) => response
                .result
                .and_then(|value| value["text"].as_str().map(str::to_owned))
                .map(|text| clip(text, TEXT_LIMIT))
                .filter(|text| !text.trim().is_empty()),

            Err(_) => {
                let _ = plugin.restart().await;
                None
            }
        };

        if let Some(text) = transcript {
            let pinned = std::fs::canonicalize(media_root.join("pinned"))
                .is_ok_and(|pinned| path.starts_with(pinned));

            if !pinned && std::fs::remove_file(&path).is_err() {
                warn!(
                    channel = channel_id,
                    "Transcription completed, but the raw voice file could not be removed."
                );
            }

            output.push(Content::Text { text });
        } else {
            warn!(plugin = %id, "Voice transcription failed.");
            output.push(unavailable_voice());
        }
    }

    output
}

pub(crate) async fn expand_media_files(
    channel_id: &str,
    content: Vec<Content>,
    media_root: &Path,
) -> Vec<Content> {
    if !matches!(channel_id, "telegram" | "discord" | "whatsapp" | "signal" | "slack") {
        return content;
    }

    let deadline = tokio::time::Instant::now() + pdf::TIME;
    let mut output = Vec::with_capacity(content.len());

    for item in content {
        let Content::File { uri, name, mime } = item else {
            output.push(item);
            continue;
        };

        let name = clip(name, META_LIMIT);

        let Some(path) = media_file(&uri, media_root) else {
            output.push(omitted(&name, missing(&uri)));
            continue;
        };

        output.push(read_file(uri, name, mime.as_deref(), &path, deadline).await);
    }

    bound(output)
}

fn missing(uri: &str) -> &'static str {
    let large = uri
        .strip_prefix("file://")
        .and_then(|path| std::fs::metadata(path).ok())
        .is_some_and(|metadata| metadata.len() > MEDIA_LIMIT);

    if large { "it is larger than 4 MiB" } else { "it is unavailable" }
}

async fn read_file(
    uri: String,
    name: String,
    mime: Option<&str>,
    path: &Path,
    deadline: tokio::time::Instant,
) -> Content {
    let mut head = Vec::with_capacity(16);

    if std::fs::File::open(path).and_then(|file| file.take(16).read_to_end(&mut head)).is_err() {
        return omitted(&name, "it is unavailable");
    }

    if image_mime(&head).is_some() {
        return Content::Image { uri, alt: Some(name) };
    }

    if head.starts_with(pdf::MAGIC) {
        return match pdf::read(path, deadline).await {
            pdf::Text::Found { text, truncated } => file_text(&name, text, truncated),
            pdf::Text::Empty => omitted(&name, "the PDF has no extractable text, such as a scan"),
            pdf::Text::Unreadable => omitted(&name, "the PDF could not be read"),
            pdf::Text::Slow => omitted(&name, "reading the PDF took too long"),
        };
    }

    if !safe_text_file(&name, mime) {
        return omitted(&name, "its file type is not supported");
    }

    let mut bytes = Vec::new();

    if std::fs::File::open(path)
        .and_then(|file| file.take(FILE_LIMIT + 1).read_to_end(&mut bytes))
        .is_err()
    {
        return omitted(&name, "it is unavailable");
    }

    let truncated = bytes.len() as u64 > FILE_LIMIT;
    bytes.truncate(FILE_LIMIT as usize);

    let text = match String::from_utf8(bytes) {
        Ok(text) => text,

        Err(error) if truncated && error.utf8_error().error_len().is_none() => {
            let valid = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();

            bytes.truncate(valid);
            String::from_utf8(bytes).unwrap_or_default()
        }

        Err(_) => return omitted(&name, "it is not valid text"),
    };

    if text
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return omitted(&name, "it is not valid text");
    }

    if text.trim().is_empty() {
        return omitted(&name, "it is empty");
    }

    file_text(&name, text, truncated)
}

fn file_text(name: &str, text: String, truncated: bool) -> Content {
    if truncated {
        return Content::Text {
            text: format!("[File: {name}, first {} KiB only]\n{text}", FILE_LIMIT / 1024),
        };
    }

    Content::Text { text: format!("[File: {name}]\n{text}") }
}

fn omitted(name: &str, reason: &str) -> Content {
    Content::Text { text: format!("[File {name} omitted because {reason}.]") }
}

pub(crate) fn media_uri(uri: &str, channel_id: &str) -> bool {
    match channel_id {
        "telegram" => uri.starts_with("telegram://file/"),
        "discord" => uri.starts_with("discord://attachment/"),
        "whatsapp" => uri.starts_with("whatsapp://media/"),
        "signal" => uri.starts_with("signal://attachment/"),
        "slack" => uri.starts_with("slack://file/"),
        _ => false,
    }
}

pub(crate) fn safe_text_file(name: &str, mime: Option<&str>) -> bool {
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let extension_allowed = matches!(
        extension.as_str(),
        "bash"
            | "c"
            | "conf"
            | "cpp"
            | "css"
            | "csv"
            | "go"
            | "h"
            | "hpp"
            | "html"
            | "ini"
            | "java"
            | "js"
            | "json"
            | "jsonl"
            | "kt"
            | "log"
            | "md"
            | "py"
            | "rb"
            | "rs"
            | "sh"
            | "sql"
            | "swift"
            | "toml"
            | "ts"
            | "tsx"
            | "txt"
            | "xml"
            | "yaml"
            | "yml"
    );

    match mime.map(str::trim).map(str::to_ascii_lowercase) {
        Some(mime) if mime.starts_with("text/") => true,
        Some(mime)
            if matches!(
                mime.as_str(),
                "application/json"
                    | "application/toml"
                    | "application/xml"
                    | "application/yaml"
                    | "application/x-yaml"
                    | "application/octet-stream"
            ) =>
        {
            extension_allowed
        }

        Some(_) => false,
        None => extension_allowed,
    }
}

pub(crate) fn unavailable_voice() -> Content {
    Content::Text { text: "[Voice message omitted because transcription is unavailable.]".into() }
}

pub(crate) fn media_file(uri: &str, root: &Path) -> Option<PathBuf> {
    let path = PathBuf::from(uri.strip_prefix("file://")?);
    let metadata = std::fs::symlink_metadata(&path).ok()?;

    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MEDIA_LIMIT {
        return None;
    }

    let root = std::fs::canonicalize(root).ok()?;
    let path = std::fs::canonicalize(path).ok()?;
    (path.starts_with(root) && path.is_file()).then_some(path)
}

pub(crate) fn prepare_images(messages: &mut [Message], media_root: &Path) {
    let mut total = 0_usize;

    for message in messages {
        message.content = std::mem::take(&mut message.content)
            .into_iter()
            .map(|content| match content {
                Content::Image { uri, alt } if uri.starts_with("file://") => {
                    let remaining = IMAGE_TOTAL_LIMIT.saturating_sub(total);
                    let image = media_file(&uri, media_root)
                        .and_then(|path| read_image(&path, IMAGE_LIMIT.min(remaining)));

                    match image {
                        Some((mime, bytes)) => {
                            total = total.saturating_add(bytes.len());
                            Content::Image {
                                uri: format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
                                alt,
                            }
                        }

                        None => unavailable_image(alt),
                    }
                }

                Content::Image { uri, alt } if uri.starts_with("data:image/") => {
                    match image_data_size(&uri) {
                        Some(size)
                            if size <= IMAGE_LIMIT
                                && size <= IMAGE_TOTAL_LIMIT.saturating_sub(total) =>
                        {
                            total = total.saturating_add(size);
                            Content::Image { uri, alt }
                        }

                        _ => unavailable_image(alt),
                    }
                }

                Content::Image { uri, alt }
                    if uri.starts_with("https://") && uri.len() <= META_LIMIT =>
                {
                    Content::Image { uri, alt }
                }

                Content::Image { alt, .. } => unavailable_image(alt),
                content => content,
            })
            .collect();
    }
}

pub(crate) fn read_image(path: &Path, limit: usize) -> Option<(&'static str, Vec<u8>)> {
    if limit == 0 {
        return None;
    }

    let file = std::fs::File::open(path).ok()?;

    if !file.metadata().ok()?.is_file() {
        return None;
    }

    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    file.take(limit.saturating_add(1) as u64).read_to_end(&mut bytes).ok()?;

    if bytes.len() > limit {
        return None;
    }

    let mime = image_mime(&bytes)?;
    Some((mime, bytes))
}

pub(crate) fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

pub(crate) fn image_data_size(uri: &str) -> Option<usize> {
    let (mime, encoded) = uri.strip_prefix("data:image/")?.split_once(";base64,")?;

    if !matches!(mime, "png" | "jpeg" | "gif" | "webp")
        || encoded.len() > IMAGE_LIMIT.saturating_add(2).saturating_div(3) * 4
    {
        return None;
    }

    let bytes = STANDARD.decode(encoded).ok()?;
    let expected = match mime {
        "png" => "image/png",
        "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };

    (image_mime(&bytes)? == expected).then_some(bytes.len())
}

pub(crate) fn unavailable_image(alt: Option<String>) -> Content {
    Content::Text {
        text: alt.map_or_else(
            || "[Image attachment omitted because it is unavailable or unsupported.]".into(),
            |alt| format!("[Image attachment unavailable. Description: {}]", clip(alt, META_LIMIT)),
        ),
    }
}

pub(crate) fn bound(mut content: Vec<Content>) -> Vec<Content> {
    content.truncate(CONTENT_LIMIT);
    content.retain(
        |item| !matches!(item, Content::ToolCall { args, .. } if validate_tool_args(args).is_err()),
    );
    content.retain(|item| !matches!(item, Content::Context { state } if !state.valid()));
    let mut text = TEXT_LIMIT;

    for item in &mut content {
        match item {
            Content::Context { .. } => {}

            Content::Text { text: value } => {
                *value = clip(std::mem::take(value), text);
                text = text.saturating_sub(value.len());
            }

            Content::Image { uri, alt } => {
                *uri = clip(std::mem::take(uri), META_LIMIT);
                *alt = alt.take().map(|value| clip(value, META_LIMIT));
            }

            Content::File { uri, name, mime } => {
                *uri = clip(std::mem::take(uri), META_LIMIT);
                *name = clip(std::mem::take(name), META_LIMIT);
                *mime = mime.take().map(|value| clip(value, META_LIMIT));
            }

            Content::Audio { uri, mime } => {
                *uri = clip(std::mem::take(uri), META_LIMIT);
                *mime = mime.take().map(|value| clip(value, META_LIMIT));
            }

            Content::ToolCall { name, id, thought_signature, .. } => {
                *name = clip(std::mem::take(name), META_LIMIT);
                *id = id.take().map(|value| clip(value, META_LIMIT));
                *thought_signature = thought_signature.take().map(|value| clip(value, META_LIMIT));
            }
        }
    }

    content
}

/// Quotes a message's reply reference and remembers the message so later replies can cite it.
/// A thread root is quoted only by the first message of its thread's conversation.
pub(crate) fn reference(
    store: &mut state::Store,
    session: &str,
    thread: Option<&str>,
    event: &serde_json::Value,
    message: &mut Message,
) -> std::io::Result<()> {
    let Some(saved) = store.sessions.get(session) else { return Ok(()) };

    let mut earlier = saved.messages.iter().chain(&saved.queued).map(|item| &item.id);
    let duplicate = earlier.clone().any(|id| *id == message.id);
    let started = earlier.any(|id| *id != message.id);

    let text = message.content.iter().map(Content::render).collect::<Vec<_>>().join("\n");

    if let Some(reply) = serde_json::from_value::<krabb_core::reply::Reply>(event["reply"].clone())
        .ok()
        .and_then(krabb_core::reply::Reply::normalized)
        .filter(|reply| !(started && thread == Some(reply.id.as_str())))
    {
        let reply = saved.resolve_reply(reply);
        message.content = quote(std::mem::take(&mut message.content), &reply);
    }

    let native = match event.get("message") {
        Some(message) => event_id(message),
        None => event_id(&event["id"]),
    };

    match native
        .and_then(|id| krabb_core::reply::Reply::new(id, message.sender.clone(), Some(text)))
    {
        Some(reply) if !duplicate => store.remember_reply(session, reply),
        _ => Ok(()),
    }
}

pub(crate) fn quote(content: Vec<Content>, reply: &krabb_core::reply::Reply) -> Vec<Content> {
    reply.quote(bound(content))
}

pub(crate) fn clip(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }

    let mut end = limit.saturating_sub(3).min(value.len());

    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }

    value.truncate(end);
    value.push('…');
    value
}

pub(crate) fn event_id(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_owned).or_else(|| value.as_i64().map(|value| value.to_string()))
}

pub(crate) fn session_id(channel: &str, chat: &str, thread: Option<&str>) -> String {
    let raw = thread.map_or_else(
        || format!("{channel}-{chat}"),
        |thread| format!("{channel}-{chat}-thread-{thread}"),
    );

    if valid(&raw) {
        return raw;
    }

    let mut key = Sha256::new();

    key.update(channel.as_bytes());
    key.update([0]);
    key.update(chat.as_bytes());
    key.update([0]);
    key.update(thread.unwrap_or_default().as_bytes());
    format!("{channel}-x{}", hex_digest(key.finalize()))
}
