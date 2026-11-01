use crate::*;

#[tokio::test]
async fn execution_transitions_roll_back_failed_saves_and_survive_retry() {
    let root = std::env::temp_dir().join(format!("krabb-durability-{}", std::process::id()));

    std::fs::create_dir_all(&root).unwrap();

    let path = root.join("tasks.json");
    let backup = root.join("durable.json");
    let tasks = Arc::new(Mutex::new(Tasks::new()));

    call_at(
        &tasks,
        Request::call(1, "add", json!({"id": 1, "text": "fixture", "activate": true})),
        Some(&path),
    )
    .await
    .unwrap();

    let due = tasks.lock().unwrap()[&1].due;

    let operations = [
        ("claim", json!({"at": due, "lease": 42})),
        ("finish", json!({"id": 1, "run_id": 42, "success": true, "result": "done"})),
        ("report", json!({"id": 1, "run_id": 42})),
    ];

    for (method, params) in operations {
        let before = serde_json::to_value(&*tasks.lock().unwrap()).unwrap();
        let durable = std::fs::read(&path).unwrap();

        std::fs::rename(&path, &backup).unwrap();
        // A directory at the destination makes atomic replacement fail even as root.
        std::fs::create_dir(&path).unwrap();

        for _ in 0..2 {
            assert!(
                call_at(&tasks, Request::call(2, method, params.clone()), Some(&path))
                    .await
                    .is_err(),
                "{method} must not acknowledge an undurable transition"
            );

            assert_eq!(serde_json::to_value(&*tasks.lock().unwrap()).unwrap(), before);
            assert_eq!(std::fs::read(&backup).unwrap(), durable);
        }

        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&backup, &path).unwrap();

        let response = call_at(&tasks, Request::call(3, method, params.clone()), Some(&path))
            .await
            .unwrap()
            .unwrap()
            .result
            .unwrap();

        let reopened = load_at(Some(&path)).unwrap();

        assert_eq!(
            serde_json::to_value(&*reopened.lock().unwrap()).unwrap(),
            serde_json::to_value(&*tasks.lock().unwrap()).unwrap()
        );

        match method {
            "claim" => {
                assert_eq!(response["item"]["attempt"], 1);

                let task = reopened.lock().unwrap()[&1].clone();

                assert_eq!(task.status, "running");
                assert_eq!(task.run_id, Some(42));
                assert_eq!(task.attempts, 1);
            }

            "finish" => {
                assert_eq!(response["status"], "completed");
                assert!(reopened.lock().unwrap()[&1].report_pending);

                let saved = std::fs::read(&path).unwrap();

                let duplicate = call_at(&reopened, Request::call(4, method, params), Some(&path))
                    .await
                    .unwrap()
                    .unwrap();

                assert_eq!(duplicate.result.unwrap(), response);
                assert_eq!(std::fs::read(&path).unwrap(), saved);
            }

            "report" => {
                assert_eq!(response["reported"], true);
                assert!(!reopened.lock().unwrap()[&1].report_pending);
            }

            _ => unreachable!(),
        }
    }

    std::fs::remove_dir_all(root).unwrap();
}
