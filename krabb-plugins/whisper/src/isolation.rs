use super::*;

#[test]
fn scoped_speech_cannot_read_other_workspaces() {
    let root = std::env::temp_dir().join(format!("krabb-speech-isolation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(root.join("worktrees/a/attachments")).unwrap();
    std::fs::create_dir_all(root.join("worktrees/b/attachments")).unwrap();
    std::fs::write(root.join("worktrees/a/attachments/voice"), "fixture").unwrap();
    std::fs::write(root.join("worktrees/b/attachments/voice"), "private").unwrap();

    for id in ["a", "b", "c"] {
        std::fs::create_dir_all(root.join("worktrees").join(id)).unwrap();
        std::fs::write(root.join("worktrees").join(id).join(".git"), "fixture").unwrap();
    }

    let binary = root.join(if cfg!(windows) { "transcriber.exe" } else { "transcriber" });

    tests::write_transcriber(&binary, "fixture transcript", "", 0);

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "isolation::worker", "--ignored", "--nocapture"])
        .env("KRABB_HOME", &root)
        .env("KRABB_WHISPER_COMMAND", &binary)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "isolated environment fixture"]
async fn worker() {
    let root = std::path::PathBuf::from(std::env::var_os("KRABB_HOME").unwrap());

    let request = |workspace: &std::path::Path, path: &std::path::Path| {
        Request::call(
            1,
            "scoped",
            json!({"workspace":workspace,"method":"transcribe","params":{"path":path}}),
        )
    };

    let workspace = krabb_file::directory(&root, std::path::Path::new("workspace")).unwrap();

    let media =
        krabb_file::directory(&workspace, std::path::Path::new("attachments/channels.telegram"))
            .unwrap();

    let voice = media.join("voice");

    std::fs::write(&voice, "fixture").unwrap();

    let retained = Request::call(
        10,
        "retained",
        json!({"workspace":workspace,"owner":"channels.telegram","path":voice}),
    );

    assert!(call(&Policy::default(), retained.clone()).await.is_err());

    krabb_file::Ownership::open(&root, &workspace)
        .unwrap()
        .assign(std::path::Path::new("attachments/channels.telegram/voice"), "channels.telegram")
        .unwrap();

    assert_eq!(
        call(&Policy::default(), retained).await.unwrap().unwrap().result.unwrap()["text"],
        "fixture transcript"
    );

    assert!(
        call(
            &Policy::default(),
            Request::call(
                11,
                "retained",
                json!({"workspace":workspace,"owner":"channels.discord","path":voice})
            )
        )
        .await
        .is_err()
    );

    assert!(
        call(
            &Policy::default(),
            Request::call(
                12,
                "retained",
                json!({"workspace":workspace,"owner":"../escape","path":voice})
            )
        )
        .await
        .is_err()
    );

    let response = call(
        &Policy::default(),
        request(&root.join("worktrees/a"), &root.join("worktrees/a/attachments/voice")),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(response.result.unwrap()["text"], "fixture transcript");

    assert!(
        call(
            &Policy::default(),
            request(&root.join("worktrees/a"), &root.join("worktrees/b/attachments/voice"))
        )
        .await
        .is_err()
    );

    assert!(
        call(
            &Policy::default(),
            request(root.parent().unwrap(), &root.join("worktrees/a/attachments/voice"))
        )
        .await
        .is_err()
    );

    assert!(
        call(
            &Policy::default(),
            Request::call(2, "scoped", json!({"workspace":root,"method":"shell"}))
        )
        .await
        .is_err()
    );
}
