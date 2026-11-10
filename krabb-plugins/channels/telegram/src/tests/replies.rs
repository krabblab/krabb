use crate::*;

#[test]
fn preserves_selected_quote_caption_and_message_identity() {
    let event = normalize_update(&json!({"update_id":9,"message":{
        "message_id":7,"chat":{"id":1,"type":"private"},"from":{"id":2},"text":"explain",
        "reply_to_message":{"message_id":3,"from":{"id":4},"caption":"whole caption"},
        "quote":{"text":"selected caption"}
    }}))
    .unwrap();

    assert_eq!(event["message"], 7);
    assert_eq!(event["reply"]["id"], "3");
    assert_eq!(event["reply"]["text"], "selected caption");
    assert_eq!(event["reply"]["sender"], "4");
    assert_eq!(event["content"][0]["text"], "explain");
}
