use crate::*;
use krabb_test::inbox::{Blocked, isolated};

async fn request(
    inbox: &Arc<Mutex<Inbox>>,
    method: &str,
    params: Value,
) -> krabb_core::Result<Value> {
    Ok(call(inbox, Request::call(7, method, params)).await?.unwrap().result.unwrap())
}

#[tokio::test]
async fn durable_spool_handoff_and_acknowledgement() {
    if !isolated(
        "tests::persistence::durable_spool_handoff_and_acknowledgement",
        &[
            ("KRABB_SIGNAL_ACCOUNT", "+1000"),
            ("KRABB_SIGNAL_COMMAND", "/nonexistent/krabb-fixture-signal-cli"),
        ],
    )
    .await
    {
        return;
    }

    save_inbox(&Inbox::default()).unwrap();

    let path = inbox_path().unwrap();
    let spool = receive_spool_path().unwrap();

    let values = [1, 2].map(|timestamp| {
        json!({"envelope": {
            "sourceNumber": "+1001", "timestamp": timestamp,
            "dataMessage": {"message": format!("message {timestamp}")}
        }})
    });

    let received = values.iter().map(|value| format!("{value}\n")).collect::<String>();

    krabb_file::save(&spool, received.as_bytes()).unwrap();

    let inbox = Arc::new(Mutex::new(load_inbox().unwrap()));
    let previous = serde_json::to_value(&*inbox.lock().await).unwrap();
    let snapshot = fs::read(&path).unwrap();
    let blocked = Blocked::new(&path);

    assert!(request(&inbox, "poll", json!({})).await.is_err());
    assert_eq!(serde_json::to_value(&*inbox.lock().await).unwrap(), previous);
    assert_eq!(fs::read(&spool).unwrap(), received.as_bytes());
    assert_eq!(blocked.snapshot(), snapshot);

    drop(blocked);

    assert_eq!(serde_json::to_value(load_inbox().unwrap()).unwrap(), previous);

    let events = request(&inbox, "poll", json!({})).await.unwrap()["events"].clone();

    assert_eq!(events.as_array().unwrap().len(), 2);

    for (index, event) in events.as_array().unwrap().iter().enumerate() {
        assert_eq!(event["id"], format!("+1001:+1001:{}", index + 1));
        assert_eq!(event["gateway_sequence"], index + 1);
        assert_eq!(event["text"], format!("message {}", index + 1));
    }

    assert!(!spool.exists());
    assert_eq!(json!(load_inbox().unwrap().events(7).unwrap()), events);

    // Reproduce a crash after saving the inbox but before deleting the receive spool.
    krabb_file::save(&spool, received.as_bytes()).unwrap();

    let inbox = Arc::new(Mutex::new(load_inbox().unwrap()));

    assert_eq!(request(&inbox, "poll", json!({})).await.unwrap()["events"], events);
    assert!(!spool.exists());
    assert_eq!(inbox.lock().await.next_sequence, 3);

    let snapshot = fs::read(&path).unwrap();
    let blocked = Blocked::new(&path);

    assert!(request(&inbox, "ack", json!({"sequence": 1})).await.is_err());
    assert_eq!(json!(inbox.lock().await.events(7).unwrap()), events);
    assert_eq!(inbox.lock().await.next_sequence, 3);
    assert_eq!(blocked.snapshot(), snapshot);

    drop(blocked);

    assert_eq!(json!(load_inbox().unwrap().events(7).unwrap()), events);

    request(&inbox, "ack", json!({"sequence": 2})).await.unwrap();

    assert_eq!(load_inbox().unwrap().events(7).unwrap(), vec![events[0].clone()]);

    request(&inbox, "ack", json!({"sequence": 1})).await.unwrap();

    let reopened = load_inbox().unwrap();

    assert!(reopened.events(7).unwrap().is_empty());
    assert_eq!(reopened.next_sequence, 3);
    assert!(!spool.exists());
}
