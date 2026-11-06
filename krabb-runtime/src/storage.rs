use super::*;

pub fn attachments(
    home: &Path,
    workspace: &Path,
    channel: &str,
    content: &[Content],
) -> Result<(), PluginError> {
    if !content.iter().any(|item| matches!(item, Content::Image { uri, .. } | Content::Audio { uri, .. } | Content::File { uri, .. } if uri.starts_with("file://"))) {
        return Ok(());
    }

    let root = std::fs::canonicalize(workspace)?;
    let mut catalog = krabb_file::Ownership::open(home, &root)?;
    let owner = String::from(sharing::principal(channel));

    for item in content {
        if let Content::Image { uri, .. } | Content::Audio { uri, .. } | Content::File { uri, .. } =
            item
            && let Some(path) = uri.strip_prefix("file://")
        {
            let path = std::fs::canonicalize(path)?;
            let relative = path.strip_prefix(&root)?;

            if catalog.owner(relative).is_some_and(|previous| previous != owner) {
                return Err("Attachment ownership collision.".into());
            }

            catalog.assign(relative, &owner)?;
        }
    }

    Ok(())
}

pub fn retain(home: &Path) -> std::io::Result<()> {
    let month = std::time::Duration::from_secs(30 * 24 * 60 * 60);

    for relative in ["logs", "logs/debug", "logs/schedules"] {
        let directory = krabb_file::directory(home, Path::new(relative))?;

        krabb_file::trim(&directory, 64 * 1024 * 1024, 1_000, month)?;
    }

    let directory = krabb_file::directory(home, Path::new("cache"))?;

    krabb_file::trim(&directory, 256 * 1024 * 1024, 1_000, month)
}

pub async fn maintain(home: PathBuf, stop: Arc<Stop>) {
    let hour = std::time::Duration::from_secs(3_600);
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + hour, hour);

    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = stop.notified() => return,

            _ = interval.tick() => {}
        }

        let home = home.clone();

        let result = tokio::task::spawn_blocking(move || retain(&home)).await;

        if !matches!(result, Ok(Ok(()))) {
            warn!("Diagnostic and cache retention could not complete; it will retry next hour.");
        }
    }
}

pub fn assign(
    home: &Path,
    workspace: &Path,
    path: &Path,
    owner: &str,
    json: bool,
) -> Result<(), PluginError> {
    if owner != "global" {
        sharing::Principal::try_from(owner.to_owned())?;
    }

    let root = std::fs::canonicalize(workspace)?;
    let path = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };

    let path = std::fs::canonicalize(path)?;
    let relative = path.strip_prefix(&root)?;

    if !path.is_file()
        || relative.components().any(|part| part.as_os_str().to_string_lossy().starts_with(".git"))
    {
        return Err("Select a content file inside the plain workspace.".into());
    }

    krabb_file::Ownership::open(home, &root)?.assign(relative, owner)?;

    if json {
        println!("{}", serde_json::json!({"path":relative,"owner":owner}));
    } else {
        println!("{} belongs to {owner}.", relative.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_preserves_complete_legacy_instructions_and_rejects_conflicts() {
        let home = crate::tests::test_root("legacy-instructions");

        std::fs::create_dir_all(home.join("workspace")).unwrap();

        let text = "custom instruction\n".repeat(2_000);

        std::fs::write(home.join("workspace/KRAB.md"), &text).unwrap();
        init_at(&home).unwrap();

        assert_eq!(std::fs::read_to_string(home.join("data/system/KRAB.md")).unwrap(), text);
        assert!(!home.join("workspace/KRAB.md").exists());

        init_at(&home).unwrap();
        std::fs::write(home.join("workspace/KRAB.md"), "conflict").unwrap();

        assert!(init_at(&home).is_err());
        assert_eq!(std::fs::read_to_string(home.join("data/system/KRAB.md")).unwrap(), text);
        assert_eq!(std::fs::read_to_string(home.join("workspace/KRAB.md")).unwrap(), "conflict");

        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn operator_assignments_and_attachment_ownership_stay_in_the_workspace() {
        let home = crate::tests::test_root("file-assignment");

        init_at(&home).unwrap();

        let workspace = home.join("workspace");

        std::fs::write(workspace.join("note.txt"), "notes").unwrap();
        assign(&home, &workspace, Path::new("note.txt"), "global", true).unwrap();
        assign(&home, &workspace, Path::new("note.txt"), "channels.telegram", false).unwrap();

        assert!(assign(&home, &workspace, Path::new("note.txt"), "invalid", true).is_err());
        assert!(assign(&home, &workspace, &home.join("config.toml"), "global", true).is_err());

        let content = vec![Content::File {
            uri: format!("file://{}", workspace.join("note.txt").display()),
            name: "note.txt".into(),
            mime: None,
        }];

        attachments(&home, &workspace, "telegram", &content).unwrap();

        assert!(attachments(&home, &workspace, "discord", &content).is_err());
        assert!(!workspace.join(".krabb").exists());

        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn attachments_follow_worktree_transitions_with_their_owner() {
        let home = crate::tests::test_root("attachment-transitions");

        init_at(&home).unwrap();

        let media = krabb_file::directory(&home, Path::new("data/media")).unwrap();

        std::fs::write(media.join("note.txt"), "attachment").unwrap();

        let mut store = state::Store::load(home.join("data/sessions/index.json")).unwrap();

        store.ensure("telegram-1", "fixture").unwrap();
        store.route("telegram-1", "telegram", "1", None, true).unwrap();
        store
            .push(
                "telegram-1",
                Message {
                    id: "1".into(),
                    session: "telegram-1".into(),
                    role: Role::User,
                    sender: None,
                    content: vec![Content::File {
                        uri: format!("file://{}", media.join("note.txt").display()),
                        name: "note.txt".into(),
                        mime: None,
                    }],
                },
            )
            .unwrap();

        let plain = home.join("workspace");
        let private = krabb_file::directory(&home, Path::new("worktrees/telegram-1")).unwrap();

        std::fs::write(private.join(".git"), "fixture").unwrap();

        for (root, isolated) in [(&plain, false), (&private, true), (&plain, false)] {
            store.storage("telegram-1", Some(root), isolated).unwrap();

            let content = &store.sessions["telegram-1"].messages[0].content[0];

            let Content::File { uri, .. } = content else {
                panic!("attachment lost");
            };

            let path = Path::new(uri.strip_prefix("file://").unwrap());

            assert!(path.starts_with(root.join(worktree::media_path(root, "telegram"))));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "attachment");

            if !isolated {
                assert_eq!(
                    krabb_file::Ownership::open(&home, &plain)
                        .unwrap()
                        .owner(path.strip_prefix(&plain).unwrap()),
                    Some("channels.telegram")
                );
            }
        }

        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn hourly_retention_runs_until_daemon_shutdown() {
        let home = crate::tests::test_root("hourly-retention");

        init_at(&home).unwrap();

        let file = home.join("cache/expired");

        std::fs::write(&file, "expired").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH))
            .unwrap();

        let stop = Arc::new(Stop::new());

        tokio::time::pause();

        let task = tokio::spawn(maintain(home.clone(), Arc::clone(&stop)));

        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(3_600)).await;
        tokio::time::resume();

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while file.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        stop.signal();
        task.await.unwrap();
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn retention_preserves_user_files() {
        let home = crate::tests::test_root("ownership");

        init_at(&home).unwrap();

        let workspace = home.join("workspace");

        std::fs::write(workspace.join("user.txt"), "keep").unwrap();

        let log = home.join("logs/old.log");

        std::fs::write(&log, "old").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&log)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH))
            .unwrap();

        retain(&home).unwrap();

        assert!(!log.exists());
        assert!(workspace.join("user.txt").is_file());

        std::fs::remove_dir_all(home).unwrap();
    }
}
