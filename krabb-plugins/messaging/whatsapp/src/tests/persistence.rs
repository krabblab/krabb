use crate::*;
use krabb_test::inbox::{Blocked, isolated};

async fn webhook(app: &App, id: &str) -> (u16, String) {
    let body = json!({"entry": [{"changes": [{"value": {"messages": [{
        "id": id, "from": "1", "type": "text", "text": {"body": "hello"}
    }]}}]}]})
    .to_string();

    let key = hmac::Key::new(hmac::HMAC_SHA256, b"fixture-secret");
    let signature = hex(hmac::sign(&key, body.as_bytes()).as_ref());

    let request = format!(
        "POST /hook HTTP/1.1\r\nContent-Length: {}\r\nX-Hub-Signature-256: sha256={}\r\n\r\n{}",
        body.len(),
        signature,
        body
    );

    handle_http_with(&mut request.as_bytes(), app, Some("fixture-secret"), None)
        .await
        .unwrap_or_else(http_error_response)
}

#[tokio::test]
async fn durable_admission_and_acknowledgement() {
    if !isolated("tests::persistence::durable_admission_and_acknowledgement", &[]).await {
        return;
    }

    let mut inbox = load_inbox().unwrap();

    inbox
        .stage([json!({"id": "first", "text": "one"}), json!({"id": "second", "text": "two"})])
        .unwrap();

    let mut inbox = load_inbox().unwrap();

    assert_eq!(
        inbox.events(7).unwrap(),
        vec![
            json!({"id": "first", "text": "one", "gateway_sequence": 1}),
            json!({"id": "second", "text": "two", "gateway_sequence": 2}),
        ]
    );

    let previous = serde_json::to_value(&inbox).unwrap();
    let path = inbox_path().unwrap();
    let snapshot = fs::read(&path).unwrap();

    let app = App {
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        queue: Arc::new(Mutex::new(inbox)),
    };

    let blocked = Blocked::new(&path);

    assert_eq!(webhook(&app, "third").await.0, 503);

    let mut inbox = app.queue.lock().await;

    assert_eq!(serde_json::to_value(&*inbox).unwrap(), previous);
    assert!(inbox.acknowledge(1).is_err());
    assert_eq!(serde_json::to_value(&*inbox).unwrap(), previous);
    assert_eq!(blocked.snapshot(), snapshot);

    drop(blocked);

    assert_eq!(serde_json::to_value(load_inbox().unwrap()).unwrap(), previous);

    drop(inbox);

    assert_eq!(webhook(&app, "third").await.0, 200);

    let mut inbox = load_inbox().unwrap();

    assert_eq!(
        inbox
            .events(7)
            .unwrap()
            .iter()
            .map(|event| (
                event["id"].as_str().unwrap().to_owned(),
                event["gateway_sequence"].as_u64().unwrap()
            ))
            .collect::<Vec<_>>(),
        vec![("first".into(), 1), ("second".into(), 2), ("third".into(), 3)]
    );

    inbox.acknowledge(2).unwrap();
    inbox.acknowledge(1).unwrap();
    inbox.acknowledge(3).unwrap();

    let mut inbox = load_inbox().unwrap();

    assert!(inbox.events(7).unwrap().is_empty());

    inbox.stage([json!({"id": "first"}), json!({"id": "second"}), json!({"id": "third"})]).unwrap();

    assert!(inbox.events(7).unwrap().is_empty(), "acknowledged IDs must remain deduplicated");

    inbox.stage([json!({"id": "fourth"})]).unwrap();

    assert_eq!(
        load_inbox().unwrap().events(7).unwrap(),
        vec![json!({"id": "fourth", "gateway_sequence": 4})]
    );
}
