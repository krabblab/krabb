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

#[test]
fn a_forum_topic_message_is_not_a_reply_to_the_topic_creation() {
    let event = normalize_update(&json!({"update_id":9,"message":{
        "message_id":8,"message_thread_id":5,"is_topic_message":true,
        "chat":{"id":-1,"type":"supergroup","is_forum":true},"from":{"id":2},"text":"status?",
        "reply_to_message":{"message_id":5,"from":{"id":2},
            "forum_topic_created":{"name":"Reports","icon_color":0}}
    }}))
    .unwrap();

    assert!(event["reply"].is_null());
    assert_eq!(event["thread"], 5);

    let root = normalize_update(&json!({"update_id":10,"message":{
        "message_id":9,"message_thread_id":5,"is_topic_message":true,
        "chat":{"id":-1,"type":"supergroup","is_forum":true},"from":{"id":2},"text":"again?",
        "reply_to_message":{"message_id":5,"from":{"id":2},"text":"Reports"}
    }}))
    .unwrap();

    assert!(root["reply"].is_null());
}
