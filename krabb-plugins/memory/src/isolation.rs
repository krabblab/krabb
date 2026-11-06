use super::*;

#[test]
fn scoped_memory_protocol_keeps_conversation_files_and_visibility_separate() {
    let root = std::env::temp_dir().join(format!("krabb-memory-isolation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(root.join("worktrees/a")).unwrap();
    std::fs::create_dir_all(root.join("worktrees/b")).unwrap();

    for id in ["a", "b", "c"] {
        std::fs::create_dir_all(root.join("worktrees").join(id)).unwrap();
        std::fs::write(root.join("worktrees").join(id).join(".git"), "fixture").unwrap();
    }

    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "isolation::worker", "--ignored", "--nocapture"])
        .env("KRABB_HOME", &root)
        .env("KRABB_MEMORY", root.join("shared/index.json"))
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "isolated environment fixture"]
fn worker() {
    let root = PathBuf::from(std::env::var_os("KRABB_HOME").unwrap());
    let items = Arc::new(Mutex::new(State::default()));
    let remember = |scope: &str, key: &str| json!({"scope":scope,"key":key,"value":"fixture fact","approved":true});

    call(&items, Request::call(1, "remember", remember("provider:telegram:1", "original")))
        .unwrap();
    call(&items, Request::call(2, "remember", remember("global", "global"))).unwrap();

    call(&items, Request::call(3, "remember", remember("tui:main", "tui"))).unwrap();

    let scoped = |workspace: &Path, method: &str, params: serde_json::Value| {
        Request::call(4, "scoped", json!({"workspace":workspace,"method":method,"params":params}))
    };

    let own = json!({"scope":"provider:telegram:1","visibility":["provider:telegram:1"]});

    let first = call(&items, scoped(&root.join("worktrees/a"), "index", own.clone()))
        .unwrap()
        .unwrap()
        .result
        .unwrap();

    assert_eq!(first["items"].as_array().unwrap().len(), 1);

    call(
        &items,
        scoped(&root.join("worktrees/a"), "remember", remember("provider:telegram:1", "local")),
    )
    .unwrap();

    let second = call(&items, scoped(&root.join("worktrees/b"), "index", own))
        .unwrap()
        .unwrap()
        .result
        .unwrap();

    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(root.join("data/workspaces/conversations/a/memory/index.json").is_file());

    let linked = call(&items, Request::call(7, "search", json!({"scope":"provider:telegram:1","query":"fixture","visibility":["provider:telegram:*","tui:*"],"excluded":["tui:private"]}))).unwrap().unwrap().result.unwrap();

    assert_eq!(linked["items"].as_array().unwrap().len(), 2);
    assert!(!linked.to_string().contains("global"));
    assert!(!linked.to_string().contains("local"));
    assert!(call(&items, scoped(root.parent().unwrap(), "index", json!({}))).is_err());
    assert!(call(&items, scoped(&root.join("worktrees/a"), "command", json!({}))).is_err());
    assert!(call(&items, Request::call(8, "index", json!({"visibility":"all"}))).is_err());
}
