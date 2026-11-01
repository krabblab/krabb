use crate::*;
use krabb_test::inbox::{Blocked, isolated};

fn attachment(id: &str) -> Attachment {
    Attachment {
        url: format!("https://files.slack.com/{id}"),
        name: format!("{id}.txt"),
        mime: Some("text/plain".into()),
        size: Some(12),
    }
}

fn event(id: &str, timestamp: &str) -> StagedEvent {
    (
        json!({"id": id, "text": "hello", "content": [
            {"kind": "text", "text": "hello"},
            {"kind": "text", "uri": format!("slack://file/{id}"), "name": format!("{id}.txt")}
        ]}),
        Some("C1".into()),
        Some(timestamp.into()),
        None,
    )
}

#[tokio::test]
async fn durable_cursor_attachments_and_acknowledgement() {
    if !isolated("tests::persistence::durable_cursor_attachments_and_acknowledgement", &[]).await {
        return;
    }

    let mut inbox = load_inbox().unwrap();

    let attachments =
        BTreeMap::from([("F1".into(), attachment("F1")), ("F2".into(), attachment("F2"))]);

    inbox
        .stage(
            [event("F1", "10.0"), event("F2", "20.0")],
            &attachments,
            "C1",
            Some("20.0"),
            Some("next-page"),
        )
        .unwrap();

    let mut inbox = load_inbox().unwrap();
    let mut expected = event("F1", "10.0").0;

    expected["gateway_sequence"] = json!(1);

    assert_eq!(inbox.pending_event().unwrap(), expected);

    assert_eq!(
        inbox
            .pending
            .iter()
            .map(|pending| (pending.event["id"].as_str().unwrap(), pending.sequence))
            .collect::<Vec<_>>(),
        vec![("F1", 1), ("F2", 2)]
    );

    assert_eq!(inbox.cursors["C1"], "next-page");
    assert_eq!(inbox.scanned["C1"], "20.0");
    assert!(inbox.channels.is_empty());
    assert_eq!(inbox.attachments["F1"].url, "https://files.slack.com/F1");
    assert_eq!(inbox.attachments["F2"].name, "F2.txt");
    assert_eq!(inbox.attachments["F2"].mime.as_deref(), Some("text/plain"));
    assert_eq!(inbox.attachments["F2"].size, Some(12));

    let path = inbox_path().unwrap();
    let previous = serde_json::to_value(&inbox).unwrap();
    let snapshot = std::fs::read(&path).unwrap();
    let blocked = Blocked::new(&path);
    let mut more = attachments.clone();

    more.insert("F3".into(), attachment("F3"));

    assert!(inbox.stage([event("F3", "30.0")], &more, "C1", Some("30.0"), None).is_err());
    assert_eq!(serde_json::to_value(&inbox).unwrap(), previous);
    assert!(inbox.acknowledge(1).is_err());
    assert_eq!(serde_json::to_value(&inbox).unwrap(), previous);
    assert_eq!(blocked.snapshot(), snapshot);

    drop(blocked);

    assert_eq!(serde_json::to_value(load_inbox().unwrap()).unwrap(), previous);

    inbox.acknowledge(1).unwrap();

    let mut inbox = load_inbox().unwrap();

    assert_eq!(inbox.pending_event().unwrap()["id"], "F2");
    assert!(!inbox.attachments.contains_key("F1"));
    assert!(inbox.attachments.contains_key("F2"));
    assert!(inbox.channels.is_empty(), "partial history must not advance acknowledged watermark");
    assert_eq!(inbox.cursors["C1"], "next-page");

    inbox.stage([event("F3", "30.0")], &more, "C1", Some("30.0"), None).unwrap();

    let mut inbox = load_inbox().unwrap();
    let previous = serde_json::to_value(&inbox).unwrap();
    let blocked = Blocked::new(&path);

    assert!(inbox.acknowledge(2).is_err());

    assert_eq!(
        serde_json::to_value(&inbox).unwrap(),
        previous,
        "failed final acknowledgement must roll back watermark and attachment removal"
    );

    drop(blocked);
    inbox.acknowledge(2).unwrap();

    let mut inbox = load_inbox().unwrap();

    assert_eq!(inbox.channels["C1"], "20.0");
    assert_eq!(inbox.pending_event().unwrap()["id"], "F3");
    assert_eq!(inbox.pending_event().unwrap()["gateway_sequence"], 3);
    assert_eq!(inbox.attachments.keys().map(String::as_str).collect::<Vec<_>>(), vec!["F3"]);

    inbox.acknowledge(3).unwrap();

    let inbox = load_inbox().unwrap();

    assert!(inbox.pending_event().is_none());
    assert!(inbox.attachments.is_empty());
    assert!(inbox.cursors.is_empty());
    assert_eq!(inbox.channels["C1"], "30.0");
    assert_eq!(inbox.scanned["C1"], "30.0");
    assert_eq!(inbox.next_sequence, 4);
}
