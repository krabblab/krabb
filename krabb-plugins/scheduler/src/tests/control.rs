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

#[tokio::test]
async fn shared_access_filters_pages_and_authorizes_each_mutation() {
    let tasks = Arc::new(Mutex::new(Tasks::new()));

    let origin = json!({"channel":"telegram","chat":"room","session":"remote","thread":null});

    for id in 1..=25 {
        call_at(&tasks, Request::call(id, "add", json!({
            "id":id,"text":"private fixture","delay":3600,"activate":true,
            "context":{"origin":{"channel":"discord","chat":"secret","session":"hidden","thread":null}}
        })), None).await.unwrap();
    }

    call_at(&tasks, Request::call(26, "add", json!({
        "id":26,"text":"shared fixture","delay":3600,"activate":true,"context":{"origin":origin}
    })), None).await.unwrap();

    let mut caller = json!({
        "channel":"tui","chat":"local","session":"local","thread":null,
        "access":{"owner":{"source":"clients.tui","conversation":"local"},"sources":[],"excluded":[]}
    });

    let request = |action: &str, caller: &serde_json::Value| {
        Request::call(
            30,
            "control",
            json!({
                "action":action,"id":26,"page":1,"caller":caller
            }),
        )
    };

    let hidden =
        call_at(&tasks, request("list", &caller), None).await.unwrap().unwrap().result.unwrap();

    assert!(hidden["items"].as_array().unwrap().is_empty());
    assert!(call_at(&tasks, request("pause", &caller), None).await.is_err());

    caller["access"]["sources"] = json!(["channels.telegram"]);

    let visible =
        call_at(&tasks, request("list", &caller), None).await.unwrap().unwrap().result.unwrap();

    assert_eq!(visible["items"].as_array().unwrap().len(), 1);
    assert_eq!(visible["items"][0]["id"], 26);
    assert!(!visible.to_string().contains("private fixture"));

    call_at(&tasks, request("pause", &caller), None).await.unwrap();

    assert_eq!(tasks.lock().unwrap()[&26].status, "paused");

    caller["access"]["excluded"] = json!(["channels.telegram"]);

    assert!(call_at(&tasks, request("resume", &caller), None).await.is_err());

    caller["access"]["owner"]["conversation"] = json!("forged");

    assert!(call_at(&tasks, request("list", &caller), None).await.is_err());
}

#[tokio::test]
async fn destination_updates_require_approval_access_and_an_idle_run() {
    let tasks = Arc::new(Mutex::new(Tasks::new()));
    let owner = json!({"channel":"telegram","chat":"1","session":"telegram-1"});
    let context = json!({"origin":owner,"workspace":"/workspace","tools":true});

    call_at(
        &tasks,
        Request::call(
            1,
            "add",
            json!({
                "id":1,"delay":60,"text":"report","context":context,"activate":true
            }),
        ),
        None,
    )
    .await
    .unwrap();

    let destinations = json!([
        {"channel":"telegram","chat":"1","private":true,"thread":null},
        {"channel":"discord","chat":"2","private":false,"thread":null}
    ]);

    let mut params = json!({"action":"update","id":1,"caller":owner,"destinations":destinations});

    assert!(call_at(&tasks, Request::call(2, "control", params.clone()), None).await.is_err());

    params["approved"] = json!(true);
    params["caller"]["session"] = json!("another");

    assert!(call_at(&tasks, Request::call(2, "control", params.clone()), None).await.is_err());

    params["caller"] = owner;
    Arc::make_mut(tasks.lock().unwrap().get_mut(&1).unwrap()).status = "running".into();

    assert!(call_at(&tasks, Request::call(2, "control", params.clone()), None).await.is_err());

    {
        let mut guard = tasks.lock().unwrap();
        let task = Arc::make_mut(guard.get_mut(&1).unwrap());
        task.status = "active".into();
        task.report_pending = true;
    }

    assert!(call_at(&tasks, Request::call(2, "control", params.clone()), None).await.is_err());

    Arc::make_mut(tasks.lock().unwrap().get_mut(&1).unwrap()).report_pending = false;
    call_at(&tasks, Request::call(2, "control", params.clone()), None).await.unwrap();

    let saved = tasks.lock().unwrap()[&1].context.clone();

    assert_eq!(saved["destinations"], destinations);
    assert_eq!(saved["origin"], context["origin"]);
    assert_eq!(saved["workspace"], context["workspace"]);

    params["destinations"] = json!([]);
    call_at(&tasks, Request::call(3, "control", params), None).await.unwrap();

    assert_eq!(tasks.lock().unwrap()[&1].context["destinations"], json!([]));
}

#[tokio::test]
async fn receipts_only_confirm_finished_acknowledged_or_removed_runs() {
    let tasks = Arc::new(Mutex::new(Tasks::new()));

    call_at(
        &tasks,
        Request::call(
            1,
            "add",
            json!({
                "id":1,"delay":60,"text":"report","activate":true
            }),
        ),
        None,
    )
    .await
    .unwrap();

    for (finished, pending, expected) in [
        (None, false, false),
        (Some(7), true, false),
        (Some(7), false, true),
        (Some(8), true, true),
    ] {
        {
            let mut guard = tasks.lock().unwrap();
            let task = Arc::make_mut(guard.get_mut(&1).unwrap());
            task.finished_run_id = finished;
            task.report_pending = pending;
        }

        let reply = call_at(&tasks, Request::call(2, "receipt", json!({"id":1,"run_id":7})), None)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(reply.result.unwrap()["reported"], expected);
    }

    let reply = call_at(&tasks, Request::call(2, "receipt", json!({"id":2,"run_id":7})), None)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(reply.result.unwrap()["reported"], true);
}
