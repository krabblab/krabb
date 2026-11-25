use super::control as control_task;
use super::*;

use krabb_core::{
    failure::{Failure, Kind},
    task::{Action, Command},
};

fn control(store: &mut state::Store, id: &str, action: Action) {
    store
        .control_task(
            id,
            &Command {
                id: format!("command-{}-{action:?}", store.tasks[id].revision),
                revision: store.tasks[id].revision,
                action,
                instruction: String::new(),
            },
        )
        .unwrap();
}

#[test]
fn retry_waits_survive_pause_resume_restart_and_failed_publication_without_refilling_budget() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let path = fixture.context.home.join("data/sessions/index.json");
    let mut store = fixture.context.sessions.lock().unwrap();

    store.ensure_task(&fixture.id, "model").unwrap();

    let at = unix_seconds();
    let task = store.claim_task(&fixture.id, at).unwrap();
    let budget = task.budget.clone();
    let revision = task.revision;
    let backup = path.with_extension("before-retry");

    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();

    let failure = Failure { kind: Kind::Throttled, retry_after_seconds: Some(3600) };

    assert!(store.retry_task_after(&fixture.id, failure.clone(), at).is_err());
    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Running);
    assert_eq!(store.tasks[&fixture.id].revision, revision);
    assert_eq!(store.tasks[&fixture.id].failures, 0);
    assert!(store.tasks[&fixture.id].retry.is_none());

    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
    store.retry_task_after(&fixture.id, failure, at).unwrap();

    let due = store.tasks[&fixture.id].retry.as_ref().unwrap().at;

    assert!(due > at + 3600);

    control(&mut store, &fixture.id, Action::Pause);
    control(&mut store, &fixture.id, Action::Resume);
    store.publish_task_notice(&fixture.id).unwrap();

    assert!(store.claim_task(&fixture.id, due - 1).is_err());
    assert_eq!(store.tasks[&fixture.id].budget, budget);

    drop(store);

    let mut restored = state::Store::load(&path).unwrap();

    assert_eq!(restored.tasks[&fixture.id].retry.as_ref().unwrap().at, due);
    assert_eq!(restored.tasks[&fixture.id].budget, budget);
    assert!(restored.claim_task(&fixture.id, due - 1).is_err());

    restored.claim_task(&fixture.id, due).unwrap();
    restored
        .checkpoint_task(&fixture.id, "Verified section one.", TaskStatus::Waiting, due + 5, None)
        .unwrap();

    assert!(restored.tasks[&fixture.id].retry.is_none());
    assert_eq!(restored.tasks[&fixture.id].failures, 0);
    assert_eq!(restored.tasks[&fixture.id].budget, budget);
}

#[test]
fn consecutive_failures_block_once_and_restart_cannot_grant_another_automatic_attempt() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let path = fixture.context.home.join("data/sessions/index.json");
    let mut store = fixture.context.sessions.lock().unwrap();

    store.ensure_task(&fixture.id, "model").unwrap();

    let budget = store.tasks[&fixture.id].budget.clone();
    let mut at = unix_seconds();

    for attempt in 1..=3 {
        store.claim_task(&fixture.id, at).unwrap();
        store.retry_task_after(&fixture.id, Failure::new(Kind::TimedOut), at).unwrap();

        assert_eq!(store.tasks[&fixture.id].failures, attempt);
        assert_eq!(store.tasks[&fixture.id].budget, budget);

        at = store.tasks[&fixture.id].ready_at();
        store.publish_task_notice(&fixture.id).unwrap();

        let expected = if attempt == 3 { TaskStatus::Blocked } else { TaskStatus::Waiting };

        assert_eq!(store.tasks[&fixture.id].status, expected);
    }

    assert!(store.claim_task(&fixture.id, at + 86400).is_err());

    let revision = store.tasks[&fixture.id].revision;

    drop(store);

    let mut restored = state::Store::load(&path).unwrap();

    assert_eq!(restored.tasks[&fixture.id].status, TaskStatus::Blocked);
    assert_eq!(restored.tasks[&fixture.id].revision, revision);
    assert!(restored.tasks[&fixture.id].notice.is_none());
    assert_eq!(restored.tasks[&fixture.id].failures, 3);
    assert!(restored.claim_task(&fixture.id, at + 86400).is_err());
}

#[test]
fn an_upgraded_legacy_retry_keeps_its_deadline_and_needs_review_without_losing_context() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let path = fixture.context.home.join("data/sessions/index.json");
    let mut store = fixture.context.sessions.lock().unwrap();

    store.ensure_task(&fixture.id, "model").unwrap();

    let task = store.tasks.get_mut(&fixture.id).unwrap();

    task.status = TaskStatus::Waiting;
    task.next = unix_seconds() + 7200;
    task.checkpoint = "Retained completed section.".into();

    let next = task.next;
    let budget = task.budget.clone();

    store.save().unwrap();

    let mut index: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    index["version"] = json!(10);
    index["jobs"][&fixture.id]["failures"] = json!(2);
    index["jobs"][&fixture.id].as_object_mut().unwrap().remove("retry");
    fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
    drop(store);

    let restored = state::Store::load(&path).unwrap();
    let task = &restored.tasks[&fixture.id];

    assert_eq!(task.status, TaskStatus::Blocked);
    assert_eq!(task.retry.as_ref().unwrap().at, next);
    assert_eq!(task.retry.as_ref().unwrap().failure.kind, Kind::Unknown);
    assert_eq!(task.budget, budget);
    assert_eq!(task.checkpoint, "Retained completed section.");
}

#[tokio::test]
async fn a_typed_transient_foreground_failure_retries_once_and_delivers_the_exact_answer() {
    let fixture = super::foreground::fixture();
    let provider = fixture.provider("retry-transient").await;

    let result = super::foreground::generate(
        fixture.context.clone(),
        fixture.origin.clone(),
        Arc::clone(&provider),
    )
    .await;

    assert!(result.is_err());

    let task = {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.clear(&fixture.origin, "idle").unwrap();
        let task = store.tasks.values().next().unwrap();

        assert_eq!(task.status, TaskStatus::Waiting);
        assert_eq!(task.failures, 1);
        task.id.clone()
    };

    let worker = fixture.start();

    fixture.until(|store| store.tasks[&task].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");

    let store = fixture.context.sessions.lock().unwrap();

    assert_eq!(store.tasks[&task].result.as_deref(), Some("The retried report is verified."));

    assert_eq!(
        store.sessions[&fixture.origin]
            .messages
            .iter()
            .filter(|message| {
                message.role == Role::Assistant
                    && message
                        .content
                        .iter()
                        .any(|part| part.render() == "The retried report is verified.")
            })
            .count(),
        1
    );
}

#[tokio::test]
async fn typed_permanent_and_uncertain_failures_do_not_retry_or_revive_mutating_work() {
    for kind in ["denied", "invalid", "exhausted", "missing", "uncertain"] {
        let fixture = super::foreground::fixture();
        let provider = fixture.provider(&format!("retry-{kind}")).await;

        assert!(
            super::foreground::generate(
                fixture.context.clone(),
                fixture.origin.clone(),
                Arc::clone(&provider)
            )
            .await
            .is_err()
        );

        let worker = fixture.start();

        fixture.until(|store| store.tasks.values().all(|task| task.notice.is_none())).await;
        fixture.stop(worker).await;
        provider.stop().await.unwrap();

        assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "1");

        let mut store = fixture.context.sessions.lock().unwrap();
        let task = store.tasks.values().next().unwrap();

        assert_eq!(task.status, TaskStatus::Blocked);
        assert_eq!(task.failures, 0);
        assert_eq!(task.uncertain, kind == "uncertain");

        if kind == "uncertain" {
            let id = task.id.clone();
            let revision = task.revision;

            assert!(
                store
                    .control_task(
                        &id,
                        &Command {
                            id: "unsafe-resume".into(),
                            revision,
                            action: Action::Resume,
                            instruction: String::new()
                        }
                    )
                    .is_err()
            );
        }
    }
}

#[test]
fn missing_current_retry_metadata_cannot_remove_a_persisted_wait() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let path = fixture.context.home.join("data/sessions/index.json");
    let mut store = fixture.context.sessions.lock().unwrap();

    store.ensure_task(&fixture.id, "model").unwrap();

    let at = unix_seconds();

    store.claim_task(&fixture.id, at).unwrap();
    store
        .retry_task_after(
            &fixture.id,
            Failure { kind: Kind::Throttled, retry_after_seconds: Some(3600) },
            at,
        )
        .unwrap();

    let mut index: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    index["jobs"][&fixture.id].as_object_mut().unwrap().remove("retry");
    fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
    drop(store);

    let error = match state::Store::load(&path) {
        Ok(_) => panic!("Missing retry evidence was accepted"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("retry reason and deadline are missing"));
}

#[test]
fn a_resume_reply_states_the_saved_retry_wait() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();

    store.ensure_task(&fixture.id, "model").unwrap();

    let at = unix_seconds();
    let failure = Failure { kind: Kind::Throttled, retry_after_seconds: Some(3600) };

    store.claim_task(&fixture.id, at).unwrap();
    store.retry_task_after(&fixture.id, failure, at).unwrap();
    control(&mut store, &fixture.id, Action::Pause);

    let due = store.tasks[&fixture.id].retry.as_ref().unwrap().at;
    let args = json!({"action":"resume", "id":fixture.id});

    let result = control_task(
        &mut store,
        &args,
        &fixture.origin,
        None,
        &fixture.context.plugins.tasks,
        true,
    );

    let Some(ToolContent::Text { text }) = result.unwrap().content.into_iter().next() else {
        panic!("Missing resume reply");
    };

    assert!(text.contains("Task resumed"), "{text}");
    assert!(text.contains(&krabb_date::format_datetime(due)), "{text}");
}

#[test]
fn a_retryable_failure_after_an_uncertain_step_blocks_instead_of_replaying() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();

    store.ensure_task(&fixture.id, "model").unwrap();
    store.claim_task(&fixture.id, unix_seconds()).unwrap();
    store.tasks.get_mut(&fixture.id).unwrap().uncertain = true;

    let error: PluginError = Failure::new(Kind::Transient).error("Connection reset.").into();

    crate::failure::settle(&mut store, &fixture.id, error.as_ref()).unwrap();

    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Blocked);
    assert!(store.tasks[&fixture.id].retry.is_none());
}

#[test]
fn a_failure_after_a_concurrent_pause_keeps_the_paused_task() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();

    store.ensure_task(&fixture.id, "model").unwrap();
    store.claim_task(&fixture.id, unix_seconds()).unwrap();
    store.tasks.get_mut(&fixture.id).unwrap().status = TaskStatus::Paused;

    let error: PluginError = Failure::new(Kind::Throttled).error("Slow down.").into();

    crate::failure::settle(&mut store, &fixture.id, error.as_ref()).unwrap();

    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Paused);
    assert!(store.tasks[&fixture.id].retry.is_none());
}
