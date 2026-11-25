use super::*;
use krabb_core::task::{Action, Command};

fn command(id: &str, action: Action, revision: u64, instruction: &str) -> Command {
    Command { id: id.into(), action, revision, instruction: instruction.into() }
}

#[tokio::test]
async fn pause_and_resume_preserve_work_while_inference_is_busy() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.tasks.get_mut(&fixture.id).unwrap().checkpoint =
            "Source A verified; write the report.".into();
        store.save().unwrap();
    }

    let provider = fixture.provider("pause").await;

    let worker = fixture.start();

    fixture.until(|_| fixture.context.home.join("generated").exists()).await;

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let revision = store.tasks[&fixture.id].revision;
        let receipt = store
            .control_task(&fixture.id, &command("pause-1", Action::Pause, revision, ""))
            .unwrap();

        assert_eq!(receipt.state, TaskStatus::Pausing);
        assert!(store.check_task_step(&fixture.id, 1).is_err());
        assert!(
            store
                .control_task(
                    &fixture.id,
                    &command("resume-early", Action::Resume, receipt.revision, "")
                )
                .is_err()
        );
    }

    fixture.context.plugins.tasks.cancel(&fixture.id);

    fixture.until(|store| store.tasks[&fixture.id].status == TaskStatus::Paused).await;

    {
        let path = fixture.context.home.join("data/sessions/index.json");
        let recovered = state::Store::load(&path).unwrap();

        assert_eq!(recovered.tasks[&fixture.id].status, TaskStatus::Paused);
        assert!(recovered.tasks[&fixture.id].checkpoint.contains("Source A verified"));
        assert!(recovered.tasks[&fixture.id].instruction.contains("verified sources"));
        assert!(recovered.outbox.is_empty());
    }

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        control(
            &mut store,
            &json!({"action":"resume", "id":fixture.id}),
            &fixture.origin,
            None,
            &fixture.context.plugins.tasks,
            false,
        )
        .unwrap();
    }

    fixture.until(|store| store.tasks[&fixture.id].status == TaskStatus::Completed).await;

    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    let requests = fs::read_to_string(fixture.context.home.join("requests.jsonl")).unwrap();

    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");
    assert!(requests.lines().last().unwrap().contains("Source A verified"));
}

#[test]
fn duplicate_commands_survive_restart_and_stale_commands_cannot_resume_work() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let pause = command("pause-1", Action::Pause, 0, "");
    let first = fixture.context.sessions.lock().unwrap().control_task(&fixture.id, &pause).unwrap();

    let mut recovered =
        state::Store::load(fixture.context.home.join("data/sessions/index.json")).unwrap();

    assert_eq!(recovered.control_task(&fixture.id, &pause).unwrap(), first);
    assert_eq!(recovered.tasks[&fixture.id].revision, first.revision);
    assert_eq!(recovered.tasks[&fixture.id].status, TaskStatus::Paused);

    assert!(
        recovered.control_task(&fixture.id, &command("pause-1", Action::Cancel, 0, "")).is_err()
    );

    assert!(
        recovered
            .control_task(&fixture.id, &command("resume-stale", Action::Resume, 0, ""))
            .is_err()
    );

    let receipt = recovered
        .control_task(&fixture.id, &command("resume-1", Action::Resume, first.revision, ""))
        .unwrap();

    assert_eq!(receipt.state, TaskStatus::Queued);
    assert_eq!(recovered.control_task(&fixture.id, &pause).unwrap(), first);
    assert_eq!(recovered.tasks[&fixture.id].status, TaskStatus::Queued);
}

#[test]
fn stop_completion_races_preserve_the_winning_durable_transition() {
    for completes_first in [true, false] {
        let fixture = Fixture::new(Principal::Channel("telegram".into()));
        let mut store = fixture.context.sessions.lock().unwrap();
        let execution = store.claim_task(&fixture.id, unix_seconds()).unwrap();

        if completes_first {
            apply(
                &mut store,
                &fixture.id,
                json!({"state":"complete", "summary":"Finished",
                "message":"The complete result."}),
            )
            .unwrap();
        }

        let revision = store.tasks[&fixture.id].revision;

        let receipt = store
            .control_task(&fixture.id, &command("cancel-1", Action::Cancel, revision, ""))
            .unwrap();

        if completes_first {
            assert_eq!(receipt.state, TaskStatus::Reporting);

            store.publish_task_notice(&fixture.id).unwrap();

            assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Completed);
            assert_eq!(store.outbox[0].text, "The complete result.");
        } else {
            assert_eq!(receipt.state, TaskStatus::Cancelling);

            assert!(
                apply(
                    &mut store,
                    &fixture.id,
                    json!({"state":"complete", "summary":"Late",
                "message":"The late result."})
                )
                .is_err()
            );

            store.finish_stop(&fixture.id, execution.step).unwrap();

            assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Cancelled);
            assert!(store.outbox.is_empty());
            assert!(store.tasks[&fixture.id].result.is_none());
        }
    }
}

#[test]
fn stopping_after_an_uncertain_effect_never_becomes_a_resumable_pause() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();
    let execution = store.claim_task(&fixture.id, unix_seconds()).unwrap();

    store.tasks.get_mut(&fixture.id).unwrap().unsafe_step = true;

    let revision = store.tasks[&fixture.id].revision;

    store.control_task(&fixture.id, &command("pause-1", Action::Pause, revision, "")).unwrap();
    store.finish_stop(&fixture.id, execution.step).unwrap();

    let revision = store.tasks[&fixture.id].revision;

    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Blocked);
    assert!(store.tasks[&fixture.id].uncertain);

    assert!(
        store
            .control_task(&fixture.id, &command("resume-1", Action::Resume, revision, ""))
            .is_err()
    );

    assert!(
        store
            .control_task(&fixture.id, &command("steer-1", Action::Steer, revision, "Try again."))
            .is_err()
    );
}

#[test]
fn paused_inference_cannot_publish_through_a_new_worker_generation() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();
    let old = store.claim_task(&fixture.id, unix_seconds()).unwrap();
    let revision = store.tasks[&fixture.id].revision;

    let receipt =
        store.control_task(&fixture.id, &command("pause-1", Action::Pause, revision, "")).unwrap();

    store.finish_stop(&fixture.id, old.step).unwrap();

    let revision = store.tasks[&fixture.id].revision;

    store.control_task(&fixture.id, &command("resume-1", Action::Resume, revision, "")).unwrap();

    let new = store.claim_task(&fixture.id, unix_seconds()).unwrap();

    assert_eq!(receipt.state, TaskStatus::Pausing);
    assert!(new.step > old.step);
    assert!(store.check_task_step(&fixture.id, old.step).is_err());
    assert!(store.finish_stop(&fixture.id, old.step).is_err());
    assert!(store.check_task_step(&fixture.id, new.step).is_ok());
    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Running);
}

#[test]
fn restart_finishes_pending_stop_requests_without_restarting_inference() {
    for action in [Action::Pause, Action::Cancel, Action::Steer] {
        let fixture = Fixture::new(Principal::Client("desktop".into()));

        {
            let mut store = fixture.context.sessions.lock().unwrap();
            let execution = store.claim_task(&fixture.id, unix_seconds()).unwrap();
            let instruction =
                if action == Action::Steer { "Use the corrected dataset." } else { "" };

            store
                .control_task(
                    &fixture.id,
                    &command("control-1", action, execution.revision, instruction),
                )
                .unwrap();
        }

        let recovered =
            state::Store::load(fixture.context.home.join("data/sessions/index.json")).unwrap();

        let expected = match action {
            Action::Pause => TaskStatus::Paused,
            Action::Cancel => TaskStatus::Cancelled,
            _ => TaskStatus::Queued,
        };

        assert_eq!(recovered.tasks[&fixture.id].status, expected);
        assert_eq!(recovered.tasks[&fixture.id].step, 1);
        assert!(!recovered.tasks[&fixture.id].restart);
    }
}

#[test]
fn failed_control_persistence_does_not_acknowledge_or_change_task_state() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let directory = fixture.context.home.join("data/sessions");
    let backup = fixture.context.home.join("data/sessions-backup");

    fs::rename(&directory, &backup).unwrap();
    fs::write(&directory, "blocked").unwrap();

    let mut store = fixture.context.sessions.lock().unwrap();

    assert!(store.control_task(&fixture.id, &command("pause-1", Action::Pause, 0, "")).is_err());
    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Queued);
    assert_eq!(store.tasks[&fixture.id].revision, 0);
    assert!(store.tasks[&fixture.id].commands.is_empty());

    fs::remove_file(&directory).unwrap();
    fs::rename(&backup, &directory).unwrap();
    store.control_task(&fixture.id, &command("pause-1", Action::Pause, 0, "")).unwrap();

    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Paused);
}

#[test]
fn old_command_ids_cannot_reapply_after_bounded_receipt_retention() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();
    let old = command("pause-old", Action::Pause, 0, "");

    store.control_task(&fixture.id, &old).unwrap();

    for index in 0..40 {
        let revision = store.tasks[&fixture.id].revision;

        store
            .control_task(
                &fixture.id,
                &command(&format!("pause-{index}"), Action::Pause, revision, ""),
            )
            .unwrap();
    }

    assert_eq!(store.tasks[&fixture.id].commands.len(), 32);
    assert!(store.control_task(&fixture.id, &old).is_err());
    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Paused);
}

#[test]
fn legacy_blockers_migrate_without_becoming_manual_pauses() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let path = fixture.context.home.join("data/sessions/index.json");
    let mut index: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    index["version"] = json!(4);
    index["jobs"][&fixture.id]["status"] = json!("paused");
    index["jobs"][&fixture.id].as_object_mut().unwrap().remove("commands");
    index["jobs"][&fixture.id].as_object_mut().unwrap().remove("restart");
    fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();

    let recovered = state::Store::load(&path).unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    assert_eq!(recovered.tasks[&fixture.id].status, TaskStatus::Blocked);
    assert_eq!(saved["version"], 16);
    assert!(recovered.tasks[&fixture.id].commands.is_empty());

    let mut future = saved;

    future["version"] = json!(u8::MAX);
    fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();

    assert!(state::Store::load(&path).is_err());
}

#[test]
fn a_task_saved_by_an_earlier_release_as_a_job_loads_and_stays_in_that_format() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let legacy = fixture.id.replacen("task-", "job-", 1);
    let data = fixture.context.home.join("data");

    for entry in walk(&data) {
        let text = fs::read_to_string(&entry).unwrap();

        fs::write(&entry, text.replace(&fixture.id, &legacy)).unwrap();
    }

    let journal = data.join("tasks").join(&fixture.id);

    if journal.exists() {
        fs::rename(&journal, data.join("tasks").join(&legacy)).unwrap();
    }

    let path = data.join("sessions/index.json");
    let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    saved["version"] = json!(15);
    fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();

    assert_eq!(saved["jobs"][&legacy]["id"], legacy.as_str());

    let mut store = state::Store::load(&path).unwrap();
    let revision = store.tasks[&legacy].revision;

    store.ensure_task(&legacy, "model").unwrap();
    store.control_task(&legacy, &command("pause-1", Action::Pause, revision, "")).unwrap();

    let reloaded = state::Store::load(&path).unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    assert_eq!(reloaded.tasks[&legacy].status, TaskStatus::Paused);
    assert_eq!(reloaded.sessions[&legacy].kind, state::SessionKind::Task);
    assert_eq!(saved["version"], 16);
    assert_eq!(saved["jobs"][&legacy]["status"], "paused");
    assert!(saved.get("tasks").is_none());

    assert!(
        walk(&data)
            .iter()
            .any(|file| { fs::read_to_string(file).unwrap().contains(r#""kind":"job""#) })
    );
}

fn walk(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();

    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();

        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }

    files
}
