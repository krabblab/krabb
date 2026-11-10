use crate::*;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

#[tokio::test]
async fn terminal_reply_preserves_context_and_treats_bang_as_text() {
    let root = std::env::temp_dir().join(format!("krabb-tui-quoted-{}", std::process::id()));
    let home = root.to_str().unwrap();
    let _ = std::fs::remove_dir_all(&root);

    offline::control(home, "session.ensure", json!({"id":"default","model":"test"})).await.unwrap();

    offline::control(
        home,
        "session.append",
        json!({"id":"default","message":{
            "id":"earlier", "session":"default", "role":"assistant", "sender":null,
            "content":[{"kind":"text","text":"earlier answer"}]
        }}),
    )
    .await
    .unwrap();

    let (mut input, reader) = duplex(4096);
    let (writer, mut output) = duplex(16 * 1024);

    input.write_all(b"/reply missing explain\n/reply last !explain\n/quit\n").await.unwrap();
    drop(input);

    let (interrupts, receiver) = tokio::sync::mpsc::unbounded_channel();

    drop(interrupts);

    run_with(
        reader,
        writer,
        EngineConfig {
            home: home.into(),
            plugin: "codex".into(),
            model: "test".into(),
            model_override: None,
            session: "default".into(),
        },
        local_aware,
        EngineEvents::default(),
        receiver,
    )
    .await
    .unwrap();

    let mut text = String::new();

    output.read_to_string(&mut text).await.unwrap();

    assert!(text.contains("unavailable in this session"));
    assert!(text.contains("Reply"));
    assert!(!text.contains("Request failed"));

    let saved = offline::control(home, "session.get", json!({"id":"default"})).await.unwrap();
    let messages: Vec<Message> = serde_json::from_value(saved["messages"].clone()).unwrap();
    let question = messages.iter().find(|message| message.role == Role::User).unwrap();

    assert!(question.content[0].render().contains("earlier answer"));
    assert_eq!(question.content[1].render(), "!explain");

    std::fs::remove_dir_all(root).unwrap();
}
