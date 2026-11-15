use super::*;
use crate::jobs::dialog as dialogue;

fn input(session: &str, id: &str, text: &str) -> Message {
    Message {
        id: id.into(),
        session: session.into(),
        role: Role::User,
        sender: Some("8".into()),
        content: vec![Content::Text { text: text.into() }],
    }
}

fn send(fixture: &Fixture, store: &mut state::Store, message: &Message) -> Option<String> {
    dialogue::message(store, message, &fixture.context.plugins.jobs).unwrap()
}

#[test]
fn task_commands_are_durable_idempotent_and_do_not_change_the_active_context() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));

    let mut store = fixture.context.sessions.lock().unwrap();
    let history = store.sessions[&fixture.origin].context();
    let message = input(&fixture.origin, "pause-message", &format!("/task pause {}", fixture.id));

    let reply = send(&fixture, &mut store, &message).unwrap();
    let revision = store.jobs[&fixture.id].revision;

    assert!(reply.contains("paused"));
    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Paused);
    assert_eq!(store.sessions[&fixture.origin].context(), history);
    assert_eq!(store.sessions[&fixture.origin].messages.len(), history.len());
    assert_eq!(store.sessions[&fixture.origin].commands.len(), 1);

    let path = fixture.context.home.join("data/sessions/index.json");
    let mut recovered = state::Store::load(&path).unwrap();

    assert_eq!(send(&fixture, &mut recovered, &message), Some(reply));
    assert_eq!(recovered.jobs[&fixture.id].revision, revision);

    let conflict = input(&fixture.origin, "pause-message", &format!("/task cancel {}", fixture.id));

    assert!(dialogue::message(&mut recovered, &conflict, &fixture.context.plugins.jobs).is_err());
    assert_eq!(recovered.jobs[&fixture.id].status, JobStatus::Paused);
}

#[test]
fn a_lost_transcript_commit_does_not_repeat_the_saved_control() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();
    let message = input(&fixture.origin, "pause-message", &format!("/task pause {}", fixture.id));
    let count = store.sessions[&fixture.origin].messages.len();

    store.record_limit = Some(1);

    assert!(dialogue::message(&mut store, &message, &fixture.context.plugins.jobs).is_err());
    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Paused);
    assert_eq!(store.jobs[&fixture.id].commands.len(), 1);
    assert_eq!(store.sessions[&fixture.origin].messages.len(), count);

    let path = fixture.context.home.join("data/sessions/index.json");
    let mut recovered = state::Store::load(&path).unwrap();

    let reply = send(&fixture, &mut recovered, &message).unwrap();

    assert!(reply.contains("paused"));
    assert_eq!(recovered.jobs[&fixture.id].commands.len(), 1);
    assert_eq!(recovered.jobs[&fixture.id].revision, 1);
    assert_eq!(recovered.sessions[&fixture.origin].messages.len(), count);
    assert_eq!(recovered.sessions[&fixture.origin].commands.len(), 1);
}

#[test]
fn a_timed_resume_survives_a_lost_reply_without_replacing_the_goal() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();
    let history = store.sessions[&fixture.origin].context();
    let goal = store.jobs[&fixture.id].instruction.clone();
    let origin = store.jobs[&fixture.id].origin.clone();

    let message = input(
        &fixture.origin,
        "timed-resume",
        &format!("/task defer {} 2030-11-12T09:00 Europe/Rome", fixture.id),
    );

    store.record_limit = Some(1);

    assert!(dialogue::message(&mut store, &message, &fixture.context.plugins.jobs).is_err());

    let saved = store.jobs[&fixture.id].deferred.clone().unwrap();

    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Paused);
    assert_eq!(store.sessions[&fixture.origin].context(), history);

    let path = fixture.context.home.join("data/sessions/index.json");
    let mut recovered = state::Store::load(&path).unwrap();

    let reply = send(&fixture, &mut recovered, &message).unwrap();

    assert!(reply.contains("2030-11-12"));
    assert!(reply.contains("Europe/Rome"));
    assert_eq!(recovered.jobs[&fixture.id].deferred.as_ref(), Some(&saved));
    assert_eq!(recovered.jobs[&fixture.id].deferrals.len(), 1);
    assert_eq!(recovered.jobs[&fixture.id].instruction, goal);
    assert_eq!(recovered.jobs[&fixture.id].origin, origin);
    assert_eq!(recovered.sessions[&fixture.origin].context(), history);

    let status =
        send(&fixture, &mut recovered, &input(&fixture.origin, "status", "/task")).unwrap();

    assert!(status.contains(&fixture.id));
    assert!(status.contains("resumes no earlier than"));
    assert!(status.contains("Europe/Rome"));
}

#[test]
fn inexact_times_and_malformed_commands_leave_the_task_unchanged() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();
    let id = fixture.id.clone();

    for (index, text) in [
        format!("/task defer {id} tomorrow 09:00 Europe/Rome"),
        format!("/task defer {id} 2030-11-12T09:00"),
        format!("/task defer {id} 2026-10-25T02:30 Europe/Rome"),
        format!("/task defer {id} 2000-01-01T09:00:00Z"),
        format!("/task extend {id} thirty"),
        format!("/task steer {id}"),
        format!("/task pause {id} now"),
        "/task pause".into(),
        "/task archive job-1".into(),
    ]
    .into_iter()
    .enumerate()
    {
        let reply =
            send(&fixture, &mut store, &input(&fixture.origin, &format!("bad-{index}"), &text));

        assert!(reply.is_some_and(|reply| !reply.is_empty()), "{text}");
        assert_eq!(store.jobs[&fixture.id].status, JobStatus::Queued, "{text}");
        assert_eq!(store.jobs[&fixture.id].revision, 0, "{text}");
        assert!(store.jobs[&fixture.id].deferrals.is_empty(), "{text}");
    }
}

#[test]
fn ordinary_text_in_any_language_reaches_the_model() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();

    for text in ["pause the report", "metti in pausa il report", "continue", "/tasks"] {
        let messages = store.sessions[&fixture.origin].messages.len();

        assert_eq!(send(&fixture, &mut store, &input(&fixture.origin, text, text)), None, "{text}");
        assert_eq!(store.sessions[&fixture.origin].messages.len(), messages, "{text}");
        assert_eq!(store.jobs[&fixture.id].revision, 0, "{text}");
    }
}

#[test]
fn another_conversation_or_sender_cannot_see_or_control_the_task() {
    let fixture = Fixture::new(Principal::Channel("telegram".into()));
    let mut store = fixture.context.sessions.lock().unwrap();

    store.ensure("another-chat", "model").unwrap();

    let mut foreign = input(&fixture.origin, "foreign", &format!("/task pause {}", fixture.id));

    foreign.sender = Some("someone-else".into());

    for message in [
        foreign.clone(),
        Message {
            id: "foreign-status".into(),
            content: vec![Content::Text { text: "/task".into() }],
            ..foreign
        },
        input("another-chat", "chat-status", "/task"),
        input("another-chat", "chat-pause", &format!("/task pause {}", fixture.id)),
    ] {
        let reply = send(&fixture, &mut store, &message).unwrap();

        assert!(!reply.contains("report"), "{reply}");
        assert_eq!(store.jobs[&fixture.id].status, JobStatus::Queued);
        assert!(store.jobs[&fixture.id].commands.is_empty());
    }

    let owner = input(&fixture.origin, "owner-pause", &format!("/task pause {}", fixture.id));

    assert!(send(&fixture, &mut store, &owner).unwrap().contains("paused"));
    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Paused);
}

#[test]
fn an_uncertain_effect_is_not_resumed_by_a_command() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();
    let due = store.jobs[&fixture.id].next;

    store.claim_job(&fixture.id, due).unwrap();
    store.ensure_job(&fixture.id, "model").unwrap();
    store.begin(&fixture.id, input(&fixture.id, "work", "Complete the report")).unwrap();
    store.phase(&fixture.id, "unsafe").unwrap();
    store.recover_job(&fixture.id).unwrap();

    let reply = send(
        &fixture,
        &mut store,
        &input(&fixture.origin, "resume", &format!("/task resume {}", fixture.id)),
    )
    .unwrap();

    assert!(reply.contains("not applied"), "{reply}");
    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Blocked);
    assert!(store.jobs[&fixture.id].commands.is_empty());
}

#[test]
fn context_visibility_does_not_grant_another_chat_task_authority() {
    let mut fixture = Fixture::new(Principal::Client("desktop".into()));
    let viewer = Principal::Client("viewer".into());

    fixture.context.config.clients.insert("viewer".into(), ClientConfig::default());
    fixture.context.config.clients.get_mut("desktop").unwrap().visibility = Some(vec![
        serde_json::from_value(json!({"data":["context"],"resources":["clients.viewer"]})).unwrap(),
    ]);

    let scope = sharing::sources(&fixture.context.config, &viewer, sharing::Data::Context);

    assert!(scope.contains(&Principal::Client("desktop".into())));

    let mut store = fixture.context.sessions.lock().unwrap();

    store.ensure("viewer-chat", "model").unwrap();
    store.client("viewer-chat", "viewer").unwrap();

    let reply = send(
        &fixture,
        &mut store,
        &input("viewer-chat", "visible", &format!("/task pause {}", fixture.id)),
    )
    .unwrap();

    assert!(reply.contains("not applied"));
    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Queued);
    assert!(store.jobs[&fixture.id].commands.is_empty());
}

#[test]
fn the_model_cannot_approve_a_saved_action() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();
    let args = json!({"action":"approve", "id":fixture.id});

    let refused =
        control(&mut store, &args, &fixture.origin, None, &fixture.context.plugins.jobs, false);

    assert!(refused.unwrap_err().to_string().contains("Only the user"));
    assert!(store.jobs[&fixture.id].commands.is_empty());
}

#[test]
fn a_command_from_an_unknown_chat_creates_no_conversation() {
    let fixture = Fixture::new(Principal::Channel("telegram".into()));
    let mut store = fixture.context.sessions.lock().unwrap();
    let sessions = store.sessions.len();
    let event = json!({"id":1, "chat":99, "private":true, "sender":5, "text":"/task"});

    assert!(
        dialogue::channel(&mut store, &fixture.context.plugins.jobs, "telegram", &event,).unwrap()
    );

    assert_eq!(store.sessions.len(), sessions);
    assert!(store.outbox.iter().any(|delivery| delivery.text.contains("no unfinished tasks")));
}

#[test]
fn repeated_commands_never_displace_conversation_history() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();
    let history = store.sessions[&fixture.origin].messages.clone();

    for index in 0..100 {
        send(&fixture, &mut store, &input(&fixture.origin, &format!("status-{index}"), "/task"));
    }

    assert_eq!(store.sessions[&fixture.origin].messages, history);
    assert_eq!(store.sessions[&fixture.origin].commands.len(), 64);
}
