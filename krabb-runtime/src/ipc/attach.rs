//! Accepts client attachments and reads them like channel media before the message is saved.

use base64::Engine;
use sha2::Digest;

use super::*;

/// Upper bound for one attachment's text, so it still fits a saved client message.
pub(super) const TEXT: usize = crate::FILE_LIMIT as usize + crate::META_LIMIT;

const STALE: Duration = Duration::from_secs(60 * 60);
const UPLOADS: usize = 64;

pub(super) async fn attach(
    request: &IpcRequest,
    state: &State,
    output: &mut tokio::net::tcp::OwnedWriteHalf,
) -> io::Result<()> {
    let reply = match read(request, state).await {
        Ok(content) => IpcResponse::ok(request.id, json!({"content": content})),
        Err(error) => IpcResponse::fail(request.id, -32602, error.to_string()),
    };

    write_frame(output, reply).await
}

async fn read(request: &IpcRequest, state: &State) -> io::Result<Vec<Content>> {
    let client = requester(request, state, "session.attach").map_err(|error| invalid(&error))?;
    let params = &request.params;

    let id = params["id"].as_str().ok_or_else(|| invalid("session.attach.id is required."))?;
    let name =
        params["name"].as_str().ok_or_else(|| invalid("session.attach.name is required."))?;
    let data =
        params["data"].as_str().ok_or_else(|| invalid("session.attach.data is required."))?;
    let audio = params["audio"] == true;

    if name.is_empty() || name.len() > crate::META_LIMIT || name.contains(['/', '\\']) {
        return Err(invalid("session.attach.name must be a plain file name."));
    }

    if data.len() as u64 > crate::MEDIA_LIMIT.div_ceil(3) * 4 {
        return Err(invalid("The attachment is larger than 4 MiB."));
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| invalid("session.attach.data is invalid."))?;

    if bytes.is_empty() || bytes.len() as u64 > crate::MEDIA_LIMIT {
        return Err(invalid("The attachment is empty or larger than 4 MiB."));
    }

    if !state.sessions.lock().map_err(lock)?.sessions.contains_key(id) {
        return Err(not_found("Session was not found."));
    }

    let root = media(state, id, client)?;
    let path = upload(&root, &bytes)?;
    let uri = format!("file://{}", path.display());
    let mut call = 0;

    let content = if audio {
        let item = Content::Audio { uri, mime: None };
        super::super::transcribe_media(&state.plugins, client, vec![item], &root, &mut call).await
    } else {
        let item = Content::File { uri, name: name.to_owned(), mime: None };
        super::super::expand_media_files(vec![item], &root).await
    };

    if !content.iter().any(|item| matches!(item, Content::Image { .. })) {
        let _ = std::fs::remove_file(&path);
    }

    Ok(content
        .into_iter()
        .map(|item| match item {
            Content::Text { text } => Content::Text { text: super::super::clip(text, TEXT) },
            item => item,
        })
        .collect())
}

/// Keeps upload bytes outside retained storage until the message that uses them is saved.
fn upload(root: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let directory = krabb_file::directory(root, Path::new("uploads"))?;
    let mut count = 0;

    for entry in std::fs::read_dir(&directory)? {
        let entry = entry?;
        let stale = entry.metadata()?.modified()?.elapsed().is_ok_and(|age| age > STALE);

        if stale {
            let _ = std::fs::remove_file(entry.path());
        } else {
            count += 1;
        }
    }

    if count >= UPLOADS {
        return Err(invalid("Too many attachments are waiting to be sent."));
    }

    let digest = crate::hex_digest(sha2::Sha256::digest(bytes));
    let path = directory.join(format!("{digest}.bin"));

    krabb_file::save(&path, bytes)?;
    Ok(path)
}

/// Pins uploaded images into retained storage; the caller holds the session lock until saving.
pub(super) fn admit(content: Vec<Content>, root: &Path) -> Vec<Content> {
    let uploads: Vec<PathBuf> = content
        .iter()
        .filter_map(|item| match item {
            Content::Image { uri, .. } => super::super::media_file(uri, root),
            _ => None,
        })
        .filter(|path| path.parent().is_some_and(|parent| parent.ends_with("uploads")))
        .collect();

    let content = super::super::media::pin(content, root);

    for path in uploads {
        let _ = std::fs::remove_file(path);
    }

    content
}

/// Resolves the media directory a client's turn reads attachments from.
pub(super) fn media(state: &State, id: &str, client: &str) -> io::Result<PathBuf> {
    let saved = {
        let sessions = state.sessions.lock().map_err(lock)?;
        sessions.sessions.get(id).and_then(|session| session.workspace.clone())
    };

    let policy = state.config.clients.get(client).cloned().unwrap_or_default();

    match workspace(state, id, policy.worktree, saved)? {
        Some(workspace) => super::super::worktree::media(&workspace, client),
        None => Ok(super::super::media_root_at(&state.home)),
    }
}

/// Selects the workspace a client's turn runs in.
pub(super) fn workspace(
    state: &State,
    id: &str,
    worktree: bool,
    saved: Option<String>,
) -> io::Result<Option<PathBuf>> {
    if worktree {
        return super::super::worktree::conversation(id, &state.home)
            .map(Some)
            .map_err(io::Error::other);
    }

    match saved {
        Some(saved) => Ok(Some(PathBuf::from(saved))),
        None => super::super::sharing::workspace(&state.root).map_err(io::Error::other),
    }
}

/// Accepts a saved client message part when it is text or media already inside the client's root.
pub(super) fn accepted(content: &Content, root: Option<&Path>) -> bool {
    match content {
        Content::Text { text } => text.len() <= TEXT,

        Content::Image { uri, .. } => {
            root.is_some_and(|root| super::super::media_file(uri, root).is_some())
        }

        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD;

    use super::*;

    fn request(params: Value) -> IpcRequest {
        IpcRequest::call(1, "secret", "session.attach", params)
    }

    fn upload(state: &State, name: &str, bytes: &[u8]) -> io::Result<Vec<Content>> {
        let params = json!({"id": "s", "name": name, "data": STANDARD.encode(bytes)});

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(read(&request(params), state))
    }

    #[test]
    fn reads_uploads_like_channel_media() {
        let state = super::super::tests::state_at("attach-read");

        state.sessions.lock().unwrap().create("s", "model").unwrap();

        let root = media(&state, "s", krabb_core::client::TUI).unwrap();
        let note = upload(&state, "note.txt", b"Remember the milk.").unwrap();

        assert_eq!(
            note,
            vec![Content::Text { text: "[File: note.txt]\nRemember the milk.".into() }]
        );

        let image = upload(&state, "shot.png", b"\x89PNG\r\n\x1a\n0000").unwrap();

        let [Content::Image { uri, alt }] = image.as_slice() else {
            panic!("expected an image: {image:?}");
        };

        let path = crate::media_file(uri, &root).unwrap();

        assert_eq!(alt.as_deref(), Some("shot.png"));
        assert!(path.parent().unwrap().ends_with("uploads"));
        assert!(accepted(&image[0], Some(&root)));
        assert!(!accepted(&image[0], None));

        let saved = admit(image.clone(), &root);

        assert_ne!(saved, image);
        assert!(!path.exists());

        let uploads = std::fs::read_dir(root.join("uploads")).unwrap().count();

        assert_eq!(uploads, 0);

        let _ = std::fs::remove_dir_all(&state.home);
    }

    #[test]
    fn rejects_unsafe_uploads() {
        let state = super::super::tests::state_at("attach-reject");

        state.sessions.lock().unwrap().create("s", "model").unwrap();

        assert!(upload(&state, "../escape.txt", b"x").is_err());
        assert!(upload(&state, "", b"x").is_err());
        assert!(upload(&state, "empty.txt", b"").is_err());

        let large = vec![b'a'; crate::MEDIA_LIMIT as usize + 1];

        assert!(upload(&state, "large.txt", &large).is_err());

        let missing = json!({"id": "missing", "name": "a.txt", "data": STANDARD.encode(b"a")});
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

        assert!(runtime.block_on(read(&request(missing), &state)).is_err());

        let long = Content::Text { text: "a".repeat(TEXT + 1) };

        assert!(!accepted(&long, None));
        assert!(accepted(&Content::Text { text: "a".repeat(TEXT) }, None));
        assert!(!accepted(
            &Content::File { uri: "file:///x".into(), name: "x".into(), mime: None },
            None
        ));

        let _ = std::fs::remove_dir_all(&state.home);
    }
}
