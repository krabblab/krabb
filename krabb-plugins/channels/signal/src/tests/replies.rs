use crate::*;

#[test]
fn preserves_quote_text_author_and_timestamp() {
    let event = normalize(&json!({"envelope":{"sourceNumber":"123","timestamp":8,"dataMessage":{
        "message":"explain","quote":{"id":7,"author":"456","text":"quoted answer"}
    }}}))
    .unwrap();

    assert_eq!(event["message"], "8");
    assert_eq!(event["reply"]["id"], "7");
    assert_eq!(event["reply"]["sender"], "456");
    assert_eq!(event["reply"]["text"], "quoted answer");
}
