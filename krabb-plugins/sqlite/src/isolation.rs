use super::*;
use serde_json::json;

#[test]
fn scoped_data_protocol_persists_in_separate_workspace_databases() {
    let root = std::env::temp_dir().join(format!("krabb-data-isolation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(root.join("worktrees/a")).unwrap();
    std::fs::create_dir_all(root.join("worktrees/b")).unwrap();

    for id in ["a", "b", "c"] {
        std::fs::create_dir_all(root.join("worktrees").join(id)).unwrap();
        std::fs::write(root.join("worktrees").join(id).join(".git"), "fixture").unwrap();
    }

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "isolation::worker", "--ignored", "--nocapture"])
        .env("KRABB_HOME", &root)
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

#[test]
#[ignore = "isolated environment fixture"]
fn worker() {
    let root = std::path::PathBuf::from(std::env::var_os("KRABB_HOME").unwrap());
    let db = tests::db();

    for (scope, database) in [("provider:discord:1", "legacy"), ("provider:discord:2", "foreign")] {
        for params in [
            json!({"scope":scope,"operation":"create_database","database":database}),
            json!({"scope":scope,"operation":"create_table","database":database,"table":"notes","columns":[{"name":"text","type":"text"}]}),
            json!({"scope":scope,"operation":"insert","database":database,"table":"notes","record":{"text":"saved"}}),
        ] {
            store(&db, Request::call(10, "data", params)).unwrap();
        }
    }

    let request = |workspace: &std::path::Path, operation: &str| {
        Request::call(
            1,
            "scoped",
            json!({"workspace":workspace,"method":"data","params":{"scope":"provider:discord:1","operation":operation,"database":"notes"}}),
        )
    };

    assert_eq!(
        store(&db, request(&root.join("worktrees/a"), "create_database"))
            .unwrap()
            .unwrap()
            .result
            .unwrap()["created"],
        true
    );

    let a = store(&db, request(&root.join("worktrees/a"), "list_databases"))
        .unwrap()
        .unwrap()
        .result
        .unwrap();

    let b = store(&db, request(&root.join("worktrees/b"), "list_databases"))
        .unwrap()
        .unwrap()
        .result
        .unwrap();

    assert!(a.to_string().contains("notes"));
    assert!(a.to_string().contains("legacy"));
    assert!(!a.to_string().contains("foreign"));

    let imported = store(&db, Request::call(9, "scoped", json!({"workspace":root.join("worktrees/a"),"method":"data","params":{"scope":"provider:discord:1","operation":"query","database":"legacy","table":"notes"}}))).unwrap().unwrap().result.unwrap();

    assert_eq!(imported["items"].as_array().unwrap().len(), 1);
    assert!(!b.to_string().contains("notes"));

    let global = store(
        &db,
        Request::call(
            2,
            "data",
            json!({"scope":"provider:discord:1","operation":"list_databases"}),
        ),
    )
    .unwrap()
    .unwrap()
    .result
    .unwrap();

    assert!(!global.to_string().contains("notes"));
    assert!(root.join("data/workspaces/conversations/a/records/data.db").is_file());
    assert!(store(&db, request(root.parent().unwrap(), "list_databases")).is_err());

    assert!(
        store(
            &db,
            Request::call(3, "scoped", json!({"workspace":root,"method":"put","params":{}}))
        )
        .is_err()
    );

    #[cfg(unix)]
    {
        std::fs::create_dir_all(root.join("data/workspaces/conversations/c/records")).unwrap();
        std::fs::write(root.join("sentinel"), "unchanged").unwrap();
        std::os::unix::fs::symlink(
            root.join("sentinel"),
            root.join("data/workspaces/conversations/c/records/data.db"),
        )
        .unwrap();

        assert!(store(&db, request(&root.join("worktrees/c"), "list_databases")).is_err());
        assert_eq!(std::fs::read_to_string(root.join("sentinel")).unwrap(), "unchanged");
    }
}

#[test]
fn record_visibility_filters_sources_and_never_authorizes_foreign_mutation() {
    let state = tests::db();

    let mut db = state.lock().unwrap();

    for scope in ["provider:telegram:1", "provider:discord:2", "global"] {
        data(&mut db, &json!({"scope":scope,"operation":"create_database","database":"notes"}))
            .unwrap();
        data(&mut db, &json!({"scope":scope,"operation":"create_table","database":"notes","table":"items","columns":[{"name":"text","type":"text"}]})).unwrap();

        data(&mut db, &json!({"scope":scope,"operation":"insert","database":"notes","table":"items","record":{"text":scope}})).unwrap();
    }

    let mut request = json!({"scope":"provider:discord:2","operation":"list_databases","visibility":["provider:telegram:*","global"]});
    let result = data(&mut db, &request).unwrap();

    assert_eq!(result["sources"].as_array().unwrap().len(), 3);

    request["source"] = json!("provider:telegram:1");
    request["operation"] = json!("query");
    request["database"] = json!("notes");
    request["table"] = json!("items");

    assert!(data(&mut db, &request).unwrap().to_string().contains("provider:telegram:1"));

    request["operation"] = json!("drop_database");

    assert!(data(&mut db, &request).is_err());

    request["operation"] = json!("query");
    request["excluded"] = json!(["provider:telegram:*"]);

    assert!(data(&mut db, &request).is_err());

    request["source"] = json!("provider:unknown:1");

    assert!(data(&mut db, &request).is_err());

    request["source"] = json!(42);

    assert!(data(&mut db, &request).is_err());
}
