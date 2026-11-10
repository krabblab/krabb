use crate::*;

#[test]
fn normalizes_id_only_quotes_without_fabricating_text() {
    let events = normalize(&json!({"entry":[{"changes":[{"value":{"messages":[{
        "id":"new","from":"123","type":"text","text":{"body":"what about this?"},
        "context":{"id":"old","from":"456"}
    }]}}]}]}));

    assert_eq!(events[0]["reply"]["id"], "old");
    assert_eq!(events[0]["reply"]["sender"], "456");
    assert!(events[0]["reply"].get("text").is_none());
    assert_eq!(events[0]["text"], "what about this?");
}

#[tokio::test]
async fn retries_explicit_rate_rejections_and_bounds_attempts() {
    for statuses in [vec![429, 200], vec![429, 429, 429], vec![500], vec![200]] {
        let replies = statuses
            .iter()
            .map(|status| {
                (
                    *status,
                    if *status == 200 {
                        json!({"messages":[{"id":"sent"}]})
                    } else {
                        json!({"error":{"message":"rejected"}})
                    }
                    .to_string(),
                )
            })
            .collect();

        let (base, requests) = krabb_test::http::responses(replies).await;

        let app = App {
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            queue: Arc::new(Mutex::new(Inbox::default())),
        };

        let result = graph(&app, &base, "fixture", json!({"text":"tail"})).await;

        assert_eq!(result.is_ok(), statuses.last() == Some(&200));

        let requests = requests.await.unwrap();

        assert_eq!(requests.len(), statuses.len());
        assert!(requests.iter().all(|request| request.body["text"] == "tail"));
    }
}

#[test]
fn recognizes_native_throttling_without_replaying_accepted_messages() {
    let bad = reqwest::StatusCode::BAD_REQUEST;

    assert_eq!(
        retry_delay(bad, &json!({"error":{"code":130429}}), None),
        Some(Duration::from_secs(1))
    );

    assert_eq!(
        retry_delay(bad, &json!({"error":{"code":131056}}), None),
        Some(Duration::from_secs(6))
    );

    assert!(retry_delay(bad, &json!({"error":{"code":131048}}), None).is_none());
    assert!(retry_delay(bad, &json!({"error":{"code":130429}}), Some(7)).is_none());

    assert!(
        retry_delay(bad, &json!({"error":{"code":130429},"messages":[{"id":"accepted"}]}), None)
            .is_none()
    );
}

#[tokio::test]
async fn recovers_from_native_throughput_rejection() {
    let (base, requests) = krabb_test::http::responses(vec![
        (400, json!({"error":{"code":130429}}).to_string()),
        (200, json!({"messages":[{"id":"accepted"}]}).to_string()),
    ])
    .await;

    let app = App {
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        queue: Arc::new(Mutex::new(Inbox::default())),
    };

    let result = graph(&app, &base, "fixture", json!({"text":"tail"})).await.unwrap();

    assert_eq!(result["messages"][0]["id"], "accepted");
    assert_eq!(requests.await.unwrap().len(), 2);
}
