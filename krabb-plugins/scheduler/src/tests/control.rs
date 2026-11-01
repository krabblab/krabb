use crate::*;

#[tokio::test]
async fn controls_enforce_ownership_before_persisting_or_claiming() {
    let root = std::env::temp_dir().join(format!(
        "krabb-schedule-ownership-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));

    std::fs::create_dir_all(&root).unwrap();

    let path = root.join("tasks.json");

    let owner = json!({
        "channel": "telegram", "chat": "owner", "thread": "topic", "session": "session"
    });

    for (action, status) in [
        ("pause", "active"),
        ("pause", "running"),
        ("remove", "active"),
        ("remove", "running"),
        ("resume", "paused"),
        ("run", "completed"),
        ("retry", "paused"),
    ] {
        let tasks = Arc::new(Mutex::new(Tasks::new()));

        for (id, origin) in [
            (1, owner.clone()),
            (
                2,
                json!({
                    "channel": "telegram", "chat": "other", "thread": "topic", "session": "other"
                }),
            ),
            (
                3,
                json!({
                    "channel": "cli", "chat": "local", "thread": null, "session": "local"
                }),
            ),
            (4, json!(null)),
        ] {
            call_at(
                &tasks,
                Request::call(
                    1,
                    "add",
                    json!({
                        "id": id, "text": "local fixture", "delay": 3600,
                        "context": {"origin": origin}, "activate": true
                    }),
                ),
                Some(&path),
            )
            .await
            .unwrap();

            Arc::make_mut(tasks.lock().unwrap().get_mut(&id).unwrap()).status = status.into();
        }

        persist_at(Some(&path), &tasks).unwrap();
        let before = std::fs::read(&path).unwrap();

        for field in ["channel", "chat", "thread", "session"] {
            let mut foreign = owner.clone();
            foreign[field] = json!("foreign");
            let error = call_at(
                &tasks,
                Request::call(
                    2,
                    "control",
                    json!({
                        "action": action, "id": 1, "caller": foreign
                    }),
                ),
                Some(&path),
            )
            .await
            .unwrap_err();

            assert!(error.to_string().contains("another conversation"));
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert_eq!(serde_json::to_vec_pretty(&*tasks.lock().unwrap()).unwrap(), before);
        }

        for id in [2, 3, 4] {
            assert!(
                call_at(
                    &tasks,
                    Request::call(
                        2,
                        "control",
                        json!({
                            "action": action, "id": id, "caller": owner
                        })
                    ),
                    Some(&path),
                )
                .await
                .is_err()
            );
        }

        let claimed = call_at(&tasks, Request::call(3, "claim", json!({"at": 0})), Some(&path))
            .await
            .unwrap()
            .unwrap();

        assert!(claimed.result.unwrap()["item"].is_null());
        assert_eq!(std::fs::read(&path).unwrap(), before);

        call_at(
            &tasks,
            Request::call(4, "control", json!({"action": action, "id": 1, "caller": owner})),
            Some(&path),
        )
        .await
        .unwrap()
        .unwrap();

        assert_ne!(std::fs::read(&path).unwrap(), before);
        let changed = tasks.lock().unwrap().clone();

        match action {
            "remove" if status == "running" => assert!(changed[&1].cancel_requested),
            "remove" => assert!(!changed.contains_key(&1)),
            "pause" if status == "running" => assert!(changed[&1].pause_requested),
            "pause" => assert_eq!(changed[&1].status, "paused"),
            "run" | "retry" => assert!(changed[&1].manual_run),
            "resume" => assert_eq!(changed[&1].status, "active"),
            _ => unreachable!(),
        }

        for id in [2, 3, 4] {
            call_at(&tasks, Request::call(5, action, json!({"id": id})), Some(&path))
                .await
                .unwrap()
                .unwrap();
        }
    }

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn controls_reject_missing_identity_and_non_control_actions() {
    let tasks = Arc::new(Mutex::new(Tasks::new()));

    for params in [
        json!({"id": 1, "action": "remove"}),
        json!({"id": 1, "action": "remove", "caller": null}),
        json!({"id": 1, "action": "remove", "caller": "admin"}),
        json!({"id": 1, "action": "approve", "caller": {}}),
        json!({"id": 1, "action": "list", "caller": {}}),
        json!({"id": 1, "action": "add", "caller": {}}),
    ] {
        assert!(call_at(&tasks, Request::call(1, "control", params), None).await.is_err());
    }
}
