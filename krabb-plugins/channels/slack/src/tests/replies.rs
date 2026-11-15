use crate::*;

#[tokio::test]
async fn retries_only_rejected_chunks_and_records_each_sent_message() {
    let (base, requests) = krabb_test::http::responses(vec![
        (200, json!({"ok":true,"ts":"1"}).to_string()),
        (429, json!({"retry_after":0}).to_string()),
        (200, json!({"ok":false,"error":"ratelimited","retry_after":0}).to_string()),
        (200, json!({"ok":true,"ts":"2"}).to_string()),
    ])
    .await;

    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let result = send_at(
        &client,
        "fixture",
        &json!({"chat":"C1","text":format!("{}TAIL", "x".repeat(3000)),"format":"markdown"}),
        &base,
    )
    .await
    .unwrap();

    let requests = requests.await.unwrap();

    let texts: Vec<_> =
        requests.iter().map(|request| request.body["text"].as_str().unwrap()).collect();

    assert_eq!(texts, ["x".repeat(3000).as_str(), "TAIL", "TAIL", "TAIL"]);
    assert_eq!(result["references"][0]["id"], "1");
    assert_eq!(result["references"][1]["text"], "TAIL");
}

#[tokio::test]
async fn a_plain_send_records_its_message_for_later_replies() {
    let (base, _requests) =
        krabb_test::http::responses(vec![(200, json!({"ok":true,"ts":"9"}).to_string())]).await;

    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let result = send_at(&client, "fixture", &json!({"chat":"C1","text":"plain answer"}), &base)
        .await
        .unwrap();

    assert_eq!(result["references"], json!([{"id":"9","text":"plain answer"}]));
}

#[tokio::test]
async fn references_thread_parents_per_channel_and_preserves_missing_reference() {
    let (base, requests) = krabb_test::http::responses(vec![
        (200, json!({"ok":true,"messages":[{"ts":"1","user":"U1","text":"parent"}]}).to_string()),
        (200, json!({"ok":true,"messages":[]}).to_string()),
        (200, json!({"ok":true,"messages":[{"ts":"1","user":"U2","text":"late"}]}).to_string()),
        (200, json!({"ok":true,"messages":[{"ts":"1","user":"U1","text":"edited"}]}).to_string()),
    ])
    .await;

    let app = App {
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        socket: Arc::new(Mutex::new(None)),
        attachments: Arc::new(Mutex::new(BTreeMap::new())),
        inbox: Arc::new(Mutex::new(Inbox::default())),
        history: Arc::new(Mutex::new(HistoryPoll::default())),
    };

    assert!(
        thread_reply(&app, "fixture", "C1", &json!({"ts":"1","thread_ts":"1"}), &base)
            .await
            .is_none()
    );

    for ts in ["2", "3", "4"] {
        let event = json!({"ts":ts,"thread_ts":"1"});
        let reply = thread_reply(&app, "fixture", "C1", &event, &base).await.unwrap();

        assert_eq!(reply.text.as_deref(), Some("parent"));
    }

    for ts in ["5", "6"] {
        let event = json!({"ts":ts,"thread_ts":"1"});
        let missing = thread_reply(&app, "fixture", "C2", &event, &base).await.unwrap();

        assert_eq!((missing.id.as_str(), missing.text), ("1", None));
    }

    for entry in &mut app.inbox.lock().await.threads {
        entry.3 = std::time::Instant::now() - THREAD_RETRY;
    }

    let event = json!({"ts":"7","thread_ts":"1"});
    let retried = thread_reply(&app, "fixture", "C2", &event, &base).await.unwrap();

    assert_eq!(retried.text.as_deref(), Some("late"));
    assert_eq!(app.inbox.lock().await.threads.len(), 2);

    for entry in &mut app.inbox.lock().await.threads {
        entry.3 = std::time::Instant::now() - THREAD_REFRESH;
    }

    let event = json!({"ts":"8","thread_ts":"1"});
    let refreshed = thread_reply(&app, "fixture", "C1", &event, &base).await.unwrap();

    assert_eq!(refreshed.text.as_deref(), Some("edited"));

    let requests = requests.await.unwrap();
    let channels = requests.iter().map(|request| request.body["channel"].clone());

    assert_eq!(channels.collect::<Vec<_>>(), vec!["C1", "C2", "C2", "C1"]);
    assert!(requests.iter().all(|request| request.body["limit"] == 1));
}

#[tokio::test]
async fn bounds_retries_and_preserves_long_waits_and_non_rate_errors() {
    for (statuses, body) in [
        (vec![429, 429, 429], json!({"retry_after":0}).to_string()),
        (vec![429], json!({"retry_after":6}).to_string()),
        (vec![500], json!({"ok":false,"error":"failure"}).to_string()),
        (vec![429, 429, 429], "limited".into()),
    ] {
        let (base, requests) = krabb_test::http::responses(
            statuses.iter().map(|status| (*status, body.clone())).collect(),
        )
        .await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let result =
            api_at(&client, "fixture", "chat.postMessage", json!({"text":"tail"}), &base).await;

        assert!(result.is_err());
        assert_eq!(requests.await.unwrap().len(), statuses.len());
    }
}
