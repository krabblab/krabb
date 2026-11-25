use super::*;

use krabb_core::task::{
    Action, Command,
    defer::{Command as Deferred, When},
};

fn deferred(fixture: &Fixture, id: &str, at: u64) -> Deferred {
    Deferred {
        id: id.into(),
        revision: fixture.context.sessions.lock().unwrap().tasks[&fixture.id].revision,
        when: When { at, timezone: "Europe/Rome".into(), expression: None },
    }
}

#[tokio::test]
async fn a_saved_task_resumes_after_a_day_offline_without_scheduler_or_another_user_message() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));

    let now = unix_seconds();
    let before;

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let task = store.claim_task(&fixture.id, now).unwrap();
        let run = krabb_core::task::Run { id: task.id, generation: task.step };
        let reservation = store.reserve_budget(&run, 3_000, Some(10_000)).unwrap();
        store.settle_budget(&fixture.id, reservation, Some(1_000)).unwrap();
        store
            .checkpoint_task(
                &fixture.id,
                "The first section is saved.",
                TaskStatus::Waiting,
                now,
                None,
            )
            .unwrap();
        before = store.tasks[&fixture.id].budget.clone();
    }

    let command = deferred(&fixture, "tomorrow-report", now + 86_400);

    fixture.context.sessions.lock().unwrap().defer_task(&fixture.id, &command, now).unwrap();
    fs::write(fixture.context.home.join("partial.txt"), "Verified first section.").unwrap();

    let path = fixture.context.home.join("data/sessions/index.json");
    let mut restored = state::Store::load(&path).unwrap();

    assert_eq!(restored.tasks[&fixture.id].status, TaskStatus::Paused);
    assert_eq!(restored.tasks[&fixture.id].budget, before);
    assert_eq!(restored.release_deferred(command.when.at - 1).unwrap(), 0);
    assert_eq!(restored.release_deferred(command.when.at + 1).unwrap(), 1);
    assert_eq!(restored.release_deferred(command.when.at + 86_400).unwrap(), 0);
    assert_eq!(restored.tasks[&fixture.id].budget, before);
    assert!(restored.tasks[&fixture.id].deferred.is_none());

    *fixture.context.sessions.lock().unwrap() = state::Store::load(&path).unwrap();
    fs::write(fixture.context.home.join("generated"), "1").unwrap();

    let provider = fixture.provider("continue").await;
    let worker = fixture.start();

    fixture.until(|store| store.tasks[&fixture.id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    let mut store = fixture.context.sessions.lock().unwrap();

    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[&fixture.id].deferrals.len(), 1);

    assert_eq!(
        store.defer_task(&fixture.id, &command, command.when.at + 86_400).unwrap().command,
        command
    );

    assert!(store.tasks[&fixture.id].deferred.is_none());
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");

    assert_eq!(
        fs::read_to_string(fixture.context.home.join("partial.txt")).unwrap(),
        "Verified first section."
    );

    assert!(
        store.sessions[&fixture.origin]
            .messages
            .iter()
            .any(|message| message.role == Role::Assistant
                && message.content[0].render() == "Verified complete report. ".repeat(250))
    );
}

#[test]
fn manual_controls_override_timers_and_steering_preserves_a_scheduled_pause() {
    for action in [Action::Pause, Action::Resume, Action::Cancel] {
        let fixture = Fixture::new(Principal::Client("desktop".into()));
        let now = unix_seconds();
        let command = deferred(&fixture, "later", now + 100);
        let mut store = fixture.context.sessions.lock().unwrap();

        store.defer_task(&fixture.id, &command, now).unwrap();

        let revision = store.tasks[&fixture.id].revision;

        store
            .control_task(
                &fixture.id,
                &Command {
                    id: "manual-override".into(),
                    action,
                    revision,
                    instruction: String::new(),
                },
            )
            .unwrap();

        assert!(store.tasks[&fixture.id].deferred.is_none());
        assert_eq!(store.release_deferred(now + 101).unwrap(), 0);
    }

    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let now = unix_seconds();
    let mut store = fixture.context.sessions.lock().unwrap();
    let task = store.claim_task(&fixture.id, now).unwrap();

    let command = Deferred {
        id: "later".into(),
        revision: task.revision,
        when: When { at: now + 100, timezone: "UTC".into(), expression: None },
    };

    assert_eq!(store.defer_task(&fixture.id, &command, now).unwrap().state, TaskStatus::Pausing);
    assert_eq!(store.release_deferred(now + 101).unwrap(), 0);

    let revision = store.tasks[&fixture.id].revision;

    store
        .control_task(
            &fixture.id,
            &Command {
                id: "clarify".into(),
                action: Action::Steer,
                revision,
                instruction: "Use the corrected dataset.".into(),
            },
        )
        .unwrap();

    assert!(!store.tasks[&fixture.id].restart);

    store.finish_task_control(&fixture.id, task.step).unwrap();

    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Paused);
    assert!(store.tasks[&fixture.id].deferred.is_some());
    assert_eq!(store.release_deferred(now + 99).unwrap(), 0);
    assert_eq!(store.release_deferred(now + 101).unwrap(), 1);
    assert!(store.tasks[&fixture.id].instruction.contains("corrected dataset"));
}

#[test]
fn deferrals_are_idempotent_replaceable_and_preserved_when_writes_fail() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let now = unix_seconds();
    let first = deferred(&fixture, "first", now + 100);
    let mut store = fixture.context.sessions.lock().unwrap();
    let receipt = store.defer_task(&fixture.id, &first, now).unwrap();

    assert_eq!(store.defer_task(&fixture.id, &first, now + 200).unwrap(), receipt);

    let second = Deferred {
        id: "second".into(),
        revision: store.tasks[&fixture.id].revision,
        when: When { at: now + 300, timezone: "UTC".into(), expression: None },
    };

    store.defer_task(&fixture.id, &second, now).unwrap();

    assert_eq!(store.defer_task(&fixture.id, &first, now + 200).unwrap(), receipt);
    assert_eq!(store.tasks[&fixture.id].deferred.as_ref().unwrap().command, second);
    assert_eq!(store.release_deferred(now + 101).unwrap(), 0);

    let path = fixture.context.home.join("data/sessions/index.json");
    let backup = path.with_extension("before-release");

    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();

    assert!(store.release_deferred(now + 301).is_err());
    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Paused);
    assert!(store.tasks[&fixture.id].deferred.is_some());

    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();

    assert_eq!(store.release_deferred(now + 301).unwrap(), 1);
    assert_eq!(store.release_deferred(now + 400).unwrap(), 0);
}

#[test]
fn ambiguous_times_and_uncertain_actions_cannot_create_an_automatic_resume() {
    for input in [
        "2030-11-12T09:00",
        "2026-03-29T02:30 Europe/Rome",
        "2026-10-25T02:30 Europe/Rome",
        "2030-11-12T09:00:00.001Z",
        "2030-11-12T09:00 Not/AZone",
    ] {
        assert!(crate::tasks::defer::parse(input).is_err(), "{input}");
    }

    assert_eq!(crate::tasks::defer::parse("2030-11-12T09:00:00+01:00").unwrap().timezone, "+01:00");

    let annotated = crate::tasks::defer::parse("2030-11-12T09:00:00+01:00[Europe/Rome]").unwrap();

    assert_eq!(annotated.timezone, "Europe/Rome");
    assert_eq!(annotated.at, crate::tasks::defer::parse("2030-11-12T09:00:00+01:00").unwrap().at);

    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let now = unix_seconds();
    let mut store = fixture.context.sessions.lock().unwrap();
    let task = store.claim_task(&fixture.id, now).unwrap();

    let command = Deferred {
        id: "later".into(),
        revision: task.revision,
        when: When { at: now + 100, timezone: "UTC".into(), expression: None },
    };

    store.defer_task(&fixture.id, &command, now).unwrap();
    store.tasks.get_mut(&fixture.id).unwrap().unsafe_step = true;
    store.finish_task_control(&fixture.id, task.step).unwrap();

    assert!(store.tasks[&fixture.id].uncertain);
    assert!(store.tasks[&fixture.id].deferred.is_none());
    assert_eq!(store.release_deferred(now + 101).unwrap(), 0);
    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Blocked);
}

#[tokio::test]
async fn due_work_revalidates_revoked_permissions_before_any_provider_execution() {
    let mut fixture = Fixture::new(Principal::Client("desktop".into()));

    fixture.context.config.clients.get_mut("desktop").unwrap().tools = true;

    let live = fixture.context.plugins.clients.get("desktop", &fixture.context.config).unwrap();
    let now = unix_seconds();
    let command = deferred(&fixture, "after-revocation", now + 100);

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.tasks.get_mut(&fixture.id).unwrap().tools = true;
        store.defer_task(&fixture.id, &command, now).unwrap();

        assert_eq!(store.release_deferred(now + 101).unwrap(), 1);
    }

    live.tools.store(false, std::sync::atomic::Ordering::Release);

    let provider = fixture.provider("continue").await;
    let worker = fixture.start();

    fixture
        .until(|store| {
            store.tasks[&fixture.id].status == TaskStatus::Blocked
                && store.tasks[&fixture.id].last_notice.contains("authorized")
        })
        .await;

    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    assert!(!fixture.context.home.join("generated").exists());

    let store = fixture.context.sessions.lock().unwrap();

    assert!(store.tasks[&fixture.id].last_notice.contains("authorized"));
    assert!(store.tasks[&fixture.id].deferred.is_none());
}

#[test]
fn failed_admission_and_reused_command_ids_cannot_change_the_saved_resume() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let now = unix_seconds();
    let first = deferred(&fixture, "shared-id", now + 100);
    let mut store = fixture.context.sessions.lock().unwrap();
    let path = fixture.context.home.join("data/sessions/index.json");
    let backup = path.with_extension("before-deferral");

    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();

    assert!(store.defer_task(&fixture.id, &first, now).is_err());
    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Queued);
    assert_eq!(store.tasks[&fixture.id].revision, 0);
    assert!(store.tasks[&fixture.id].deferred.is_none());

    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
    store.defer_task(&fixture.id, &first, now).unwrap();

    let revision = store.tasks[&fixture.id].revision;

    assert!(
        store
            .control_task(
                &fixture.id,
                &Command {
                    id: first.id.clone(),
                    action: Action::Cancel,
                    revision,
                    instruction: String::new(),
                }
            )
            .is_err()
    );

    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Paused);

    assert!(
        store
            .defer_task(
                &fixture.id,
                &Deferred {
                    revision,
                    when: When { at: now + 200, timezone: "UTC".into(), expression: None },
                    ..first.clone()
                },
                now
            )
            .is_err()
    );

    store
        .control_task(
            &fixture.id,
            &Command {
                id: "manual-pause".into(),
                action: Action::Pause,
                revision,
                instruction: String::new(),
            },
        )
        .unwrap();

    let revision = store.tasks[&fixture.id].revision;

    assert!(
        store
            .defer_task(
                &fixture.id,
                &Deferred { id: "manual-pause".into(), revision, ..first },
                now
            )
            .is_err()
    );

    assert!(store.tasks[&fixture.id].deferred.is_none());
}

#[test]
fn cancelling_a_conversation_removes_its_deferred_resume_before_restart() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let now = unix_seconds();
    let command = deferred(&fixture, "conversation-timer", now + 100);
    let mut store = fixture.context.sessions.lock().unwrap();

    store.defer_task(&fixture.id, &command, now).unwrap();

    assert_eq!(store.cancel_tasks(&fixture.origin).unwrap(), vec![fixture.id.clone()]);
    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Cancelled);
    assert!(store.tasks[&fixture.id].deferred.is_none());

    let path = fixture.context.home.join("data/sessions/index.json");
    let mut recovered = state::Store::load(path).unwrap();

    assert_eq!(recovered.release_deferred(now + 101).unwrap(), 0);
    assert_eq!(recovered.tasks[&fixture.id].status, TaskStatus::Cancelled);
}

#[test]
fn corrupted_deferred_records_fail_closed_without_rewriting_the_catalog() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let now = unix_seconds();
    let command = deferred(&fixture, "saved-timer", now + 100);

    fixture.context.sessions.lock().unwrap().defer_task(&fixture.id, &command, now).unwrap();

    let path = fixture.context.home.join("data/sessions/index.json");
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    for damage in ["missing-receipt", "unknown-zone", "running", "uncertain"] {
        let mut damaged = saved.clone();
        let task = &mut damaged["jobs"][&fixture.id];

        match damage {
            "missing-receipt" => task["deferrals"] = json!([]),

            "unknown-zone" => {
                task["deferred"]["command"]["when"]["timezone"] = json!("Not/AZone");
                task["deferrals"][0]["command"]["when"]["timezone"] = json!("Not/AZone");
            }

            "running" => task["status"] = json!("running"),
            "uncertain" => task["uncertain"] = json!(true),
            _ => unreachable!(),
        }

        let bytes = serde_json::to_vec(&damaged).unwrap();

        krabb_file::save(&path, &bytes).unwrap();

        assert!(state::Store::load(&path).is_err(), "{damage}");
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
