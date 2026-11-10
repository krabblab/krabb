use super::*;
use krabb_core::{Content, Message, Role, reply::Reply};

#[test]
fn reply_selection_is_saved_in_model_context_and_rejects_other_sessions() {
    let state = super::tests::state_at("quoted-context");

    {
        let mut sessions = state.sessions.lock().unwrap();
        sessions.create("main", "fixture").unwrap();
        sessions.create("other", "fixture").unwrap();

        for (session, id, text) in
            [("main", "answer", "quoted answer"), ("other", "secret", "private answer")]
        {
            sessions
                .append(
                    session,
                    Message {
                        id: id.into(),
                        session: session.into(),
                        role: Role::Assistant,
                        sender: None,
                        content: vec![Content::Text { text: text.into() }],
                    },
                )
                .unwrap();
        }
    }

    let request = IpcRequest::call(
        1,
        "secret",
        "session.append",
        serde_json::json!({
            "id":"main", "reply_to":"last", "message":{
                "id":"user", "session":"main", "role":"user", "sender":"tui",
                "content":[{"kind":"text","text":"explain"}]
            }
        }),
    );

    let response = dispatch(&request, &state).unwrap();

    assert!(response.error.is_none());

    {
        let sessions = state.sessions.lock().unwrap();
        let context = sessions.sessions["main"].context();
        let user = context.last().unwrap();

        assert!(user.content[0].render().contains("quoted answer"));
        assert_eq!(user.content[1].render(), "explain");
    }

    let mut foreign = request;

    foreign.params["reply_to"] = serde_json::json!("secret");

    assert!(dispatch(&foreign, &state).is_err());
}

#[test]
fn reference_history_survives_restarts_is_bounded_and_respects_routes() {
    let root = std::env::temp_dir().join(format!("krabb-quote-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    for filename in ["legacy.json", "sessions/index.json"] {
        let path = root.join(filename);
        let mut store = crate::state::Store::load(&path).unwrap();

        for id in ["main", "other"] {
            store.create(id, "fixture").unwrap();
        }

        store.route("main", "whatsapp", "123", None, true).unwrap();
        store.route("other", "whatsapp", "456", None, true).unwrap();
        store.queue_delivery("send", "whatsapp", "123", true, None, "answer").unwrap();
        store
            .delivered(
                "send",
                &serde_json::json!({"references":[{"id":"native", "text":"sent answer"}]}),
            )
            .unwrap();

        store
            .remember_reply(
                "main",
                Reply::new("incoming".into(), Some("123".into()), Some("original".into())).unwrap(),
            )
            .unwrap();

        drop(store);

        let mut store = crate::state::Store::load(&path).unwrap();
        let reply = Reply::new("native".into(), None, None).unwrap();

        assert_eq!(
            store.sessions["main"].resolve_reply(reply.clone()).text.as_deref(),
            Some("sent answer")
        );

        assert_eq!(
            store.sessions["other"].resolve_reply(reply).text.as_deref(),
            Some("[Referenced message unavailable.]")
        );

        assert!(store.outbox.is_empty());

        for id in 0..140 {
            store
                .remember_reply(
                    "main",
                    Reply::new(id.to_string(), None, Some("🦀".repeat(4096))).unwrap(),
                )
                .unwrap();
        }

        assert_eq!(store.sessions["main"].references.len(), 128);

        assert!(
            store.sessions["main"].references.iter().all(|reply| reply
                .text
                .as_ref()
                .unwrap()
                .len()
                <= krabb_core::reply::LIMIT)
        );

        store.queue_delivery("large", "whatsapp", "123", true, None, "long answer").unwrap();

        let references: Vec<_> = (0..140)
            .map(|id| serde_json::json!({"id":format!("sent-{id}"),"text":"chunk"}))
            .collect();

        store.delivered("large", &serde_json::json!({"references":references})).unwrap();

        assert_eq!(store.sessions["main"].references.first().unwrap().id, "sent-12");
        assert_eq!(store.sessions["main"].references.last().unwrap().id, "sent-139");

        store.clear_history("main").unwrap();

        assert!(store.sessions["main"].references.is_empty());
    }

    std::fs::remove_dir_all(root).unwrap();
}
