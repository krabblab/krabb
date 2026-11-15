use crate::*;

#[test]
fn preserves_bot_quotes_and_excludes_foreign_channels() {
    let mut message = json!({"id":"2","channel_id":"1","content":"explain","author":{"id":"3"},
        "message_reference":{"message_id":"4","channel_id":"1"},
        "referenced_message":{"id":"4","channel_id":"1","author":{"id":"5","bot":true},"content":"quoted answer"}});

    let event = normalize(&message).unwrap();

    assert_eq!(event["reply"]["text"], "quoted answer");
    assert_eq!(event["reply"]["sender"], "5");
    assert_eq!(event["content"][0]["text"], "explain");

    message["message_reference"]["channel_id"] = json!("999");

    assert_eq!(
        normalize(&message).unwrap()["reply"]["text"],
        "[Referenced message is outside this conversation.]"
    );

    message["message_reference"]["channel_id"] = json!("1");
    message["referenced_message"] = Value::Null;

    assert_eq!(normalize(&message).unwrap()["reply"]["id"], "4");
}

#[test]
fn a_forwarded_message_is_input_and_not_a_reply() {
    let message = json!({"id":"2","channel_id":"1","content":"","author":{"id":"3"},
        "message_reference":{"type":1,"message_id":"4","channel_id":"999"},
        "message_snapshots":[{"message":{"content":"forwarded report"}}]});

    let event = normalize(&message).unwrap();

    assert!(event["reply"].is_null());
    assert_eq!(event["text"], "forwarded report");
    assert_eq!(event["content"][0]["text"], "forwarded report");
}

#[tokio::test]
async fn messages_sent_as_content_can_be_replied_to() {
    let (base, _) = krabb_test::http::responses(vec![(200, json!({"id":"5"}).to_string())]).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let item = Content::ToolCall {
        name: "read".into(),
        args: json!({"path":"notes.md"}),
        id: None,
        thought_signature: None,
    };

    let result = send_content(&client, "1", "", std::slice::from_ref(&item), "fixture", &base)
        .await
        .unwrap();

    assert_eq!(result["references"][0]["id"], "5");
    assert_eq!(result["references"][0]["text"], item.render());
}
