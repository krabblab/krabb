use super::*;
use crate::state::JobStatus;

#[test]
fn another_conversation_of_the_same_client_cannot_discover_or_control_the_task() {
    let (state, task) = tests::fixture("task-origin-scope");

    {
        let mut store = state.sessions.lock().unwrap();
        store.ensure("another-conversation", "model").unwrap();
        store.client("another-conversation", krabb_core::client::TUI).unwrap();
    }

    for (index, text) in [
        "/task".into(),
        format!("/task pause {task}"),
        format!("/task cancel {task}"),
        format!("/task resume {task}"),
        format!("/task steer {task} use a different dataset"),
        format!("/task defer {task} 2030-11-12T09:00 Europe/Rome"),
    ]
    .into_iter()
    .enumerate()
    {
        let receipt = message(
            &json!({"session":"another-conversation", "client":krabb_core::client::TUI,
            "id":format!("control-{index}"),"text":text}),
            &state,
        )
        .unwrap();

        assert_eq!(receipt["handled"], true);
        let text = receipt["text"].as_str().unwrap_or_default();

        assert!(!text.contains(&task));
        assert!(!text.contains("report"));
        assert!(!text.contains("Research"));
        let store = state.sessions.lock().unwrap();

        assert_eq!(store.jobs[&task].status, JobStatus::Queued);
        assert!(store.jobs[&task].commands.is_empty());
        assert!(store.jobs[&task].deferred.is_none());
    }

    let accepted = message(
        &json!({"session":"conversation", "client":krabb_core::client::TUI,
        "id":"original-chat-pause","text":format!("/task pause {task}")}),
        &state,
    )
    .unwrap();

    assert!(accepted["text"].as_str().unwrap().contains("paused"));
    assert_eq!(state.sessions.lock().unwrap().jobs[&task].status, JobStatus::Paused);
}

#[test]
fn a_local_client_cannot_control_a_channel_conversation() {
    let (state, _) = tests::fixture("task-channel-scope");

    {
        let mut store = state.sessions.lock().unwrap();
        store.ensure("channel-conversation", "model").unwrap();
        store.sessions.get_mut("channel-conversation").unwrap().channel = Some("telegram".into());
    }

    let error = message(
        &json!({"session":"channel-conversation", "client":krabb_core::client::TUI,
        "id":"channel-control","text":"/task"}),
        &state,
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "This is not a conversation for that client.");
}
