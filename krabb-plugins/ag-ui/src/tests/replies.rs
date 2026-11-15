use crate::*;

#[test]
fn retains_final_answers_after_a_different_streamed_prefix() {
    let run = RunInput::parse(&json!({"threadId":"thread","runId":"run","messages":[
        {"id":"u","role":"user","content":"hello"}
    ]}))
    .unwrap();

    let mut stream = RunStream::new(&run);

    stream.push(&run, json!({"kind":"text","text":"I'll check that."})).unwrap();

    let reply = ModelReply {
        timing: None,
        text: "Complete final answer".into(),
        stop: "stop".into(),
        input: None,
        output: None,
        context_usage: None,
        events: vec![],
    };

    let events = stream.fallback(&run, &reply).unwrap();

    assert_eq!(events[0]["type"], "TEXT_MESSAGE_END");
    assert_eq!(events[1]["type"], "TEXT_MESSAGE_START");
    assert_ne!(events[0]["messageId"], events[1]["messageId"]);
    assert_eq!(events[2]["delta"], "Complete final answer");
    assert!(stream.fallback(&run, &reply).unwrap().is_empty());
}

#[test]
fn a_clipped_final_answer_adds_only_its_limit_notice_after_the_stream() {
    let run = RunInput::parse(&json!({"threadId":"thread","runId":"run","messages":[
        {"id":"u","role":"user","content":"hello"}
    ]}))
    .unwrap();

    let mut stream = RunStream::new(&run);

    stream.push(&run, json!({"kind":"text","text":"A long streamed answer"})).unwrap();

    let reply = ModelReply {
        timing: None,
        text: format!("A long…{}", krabb_core::types::CLIP_NOTICE),
        stop: "stop".into(),
        input: None,
        output: None,
        context_usage: None,
        events: vec![],
    };

    let events = stream.fallback(&run, &reply).unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["delta"], krabb_core::types::CLIP_NOTICE);
}

#[test]
fn an_empty_message_with_a_reply_reference_is_not_a_user_turn() {
    let run = RunInput::parse(&json!({"threadId":"thread","runId":"run","messages":[
        {"id":"a","role":"assistant","content":"earlier answer"},
        {"id":"u","role":"user","content":"","reply":{"id":"a"}}
    ]}))
    .unwrap();

    assert_eq!(run.messages.len(), 1);

    let assistant = RunInput::parse(&json!({"threadId":"thread","runId":"run","messages":[
        {"id":"a","role":"assistant","content":"","reply":{"id":"u"}}
    ]}));

    assert!(assistant.is_err());
}

#[test]
fn resolves_quotes_only_from_the_current_run_history() {
    let input = json!({"threadId":"thread","runId":"run","messages":[
        {"id":"a","role":"assistant","content":"earlier answer"},
        {"id":"u","role":"user","content":"explain","reply":{"id":"a"}}
    ]});

    let run = RunInput::parse(&input).unwrap();

    assert!(run.messages[1].content[0].render().contains("earlier answer"));
    assert_eq!(run.messages[1].content[1].render(), "explain");

    let mut missing = input.clone();

    missing["messages"][1]["reply"]["id"] = json!("foreign");

    assert!(
        RunInput::parse(&missing).unwrap().messages[1].content[0].render().contains("unavailable")
    );

    missing["messages"][1]["reply"] = json!({"id":""});

    assert!(RunInput::parse(&missing).is_err());
}

#[test]
fn message_identifiers_are_literal_even_when_named_last() {
    let run = RunInput::parse(&json!({"threadId":"thread","runId":"run","messages":[
        {"id":"last","role":"assistant","content":"selected answer"},
        {"id":"other","role":"assistant","content":"newer answer"},
        {"id":"question","role":"user","content":"explain","reply":{"id":"last"}}
    ]}))
    .unwrap();

    assert!(run.messages[2].content[0].render().contains("selected answer"));
    assert!(!run.messages[2].content[0].render().contains("newer answer"));
}
