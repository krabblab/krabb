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
