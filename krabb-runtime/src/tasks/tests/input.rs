use super::*;
use krabb_core::task::{Action, Command, ControlError, Run};

fn admitted() -> (Fixture, Run, Message) {
    admitted_with(None)
}

fn admitted_with(text: Option<&str>) -> (Fixture, Run, Message) {
    admitted_content(text.map(|text| vec![Content::Text { text: text.into() }]))
}

fn admitted_content(content: Option<Vec<Content>>) -> (Fixture, Run, Message) {
    let fixture = super::foreground::fixture();
    let mut store = fixture.context.sessions.lock().unwrap();

    if let Some(content) = content {
        store.sessions.get_mut(&fixture.origin).unwrap().messages.last_mut().unwrap().content =
            content;
    }

    let history = store.sessions[&fixture.origin].context();
    let original = history.iter().rev().find(|message| message.role == Role::User).unwrap().clone();

    store.clear(&fixture.origin, "idle").unwrap();

    let context = capture(
        &store,
        "fixture",
        "model",
        &fixture.context.home,
        "desktop",
        &fixture.origin,
        None,
        &fixture.origin,
        false,
        false,
    )
    .unwrap();

    let guard = Foreground::admit(
        &mut store,
        Arc::clone(&fixture.context.plugins.tasks),
        &context,
        history,
        &Cancellation::new(),
    )
    .unwrap();

    let run = guard.run.clone();

    drop(guard);
    drop(store);
    (fixture, run, original)
}

#[test]
fn queued_admission_rejects_a_different_request_context_before_persisting_any_work() {
    let fixture = super::foreground::fixture();
    let mut store = fixture.context.sessions.lock().unwrap();
    let history = store.sessions[&fixture.origin].context();
    let mut input = history.last().unwrap().clone();

    input.id = "different-input".into();

    let context = capture(
        &store,
        "fixture",
        "model",
        &fixture.context.home,
        "desktop",
        &fixture.origin,
        None,
        &fixture.origin,
        false,
        false,
    )
    .unwrap();

    let task = record(
        &routing::Route {
            kind: routing::Kind::Fast,
            message: String::new(),
            instruction: "Prepare the report.".into(),
        },
        &context,
    )
    .unwrap();

    let id = task.id.clone();
    let path = fixture.context.home.join("data/sessions/index.json");
    let before = fs::read(&path).unwrap();

    assert_eq!(
        store.queue_foreground(task, history, input).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );

    assert!(store.tasks.is_empty());
    assert!(!store.sessions.contains_key(&id));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn a_retained_fingerprint_prevents_reinterpreting_a_request_after_history_is_pruned() {
    let (fixture, run, original) = admitted();
    let mut store = fixture.context.sessions.lock().unwrap();
    let identity = store.tasks[&run.id].input.clone().unwrap();

    assert!(identity.matches(&original).unwrap());

    store.sessions.get_mut(&fixture.origin).unwrap().messages.clear();
    store.sessions.get_mut(&run.id).unwrap().messages.clear();
    store.save().unwrap();

    let path = fixture.context.home.join("data/sessions/index.json");
    let mut restored = state::Store::load(&path).unwrap();

    restored.reserve(&fixture.origin, "retry".into()).unwrap();

    let before = fs::read(&path).unwrap();

    restored.append_reserved(&fixture.origin, "retry", original.clone()).unwrap();

    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(restored.sessions[&fixture.origin].messages.is_empty());

    let context = restored.sessions[&run.id].context();

    assert_eq!(context.iter().filter(|message| message.id == original.id).count(), 1);
    assert_eq!(context[0].content, original.content);
    assert_eq!(restored.tasks.len(), 1);
    assert_eq!(restored.tasks[&run.id].input.as_ref(), Some(&identity));

    let mut changed = original;

    changed.content =
        vec![Content::Text { text: "Replace the report with a different request.".into() }];

    assert_eq!(
        restored.append_reserved(&fixture.origin, "retry", changed.clone()).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );

    restored.set_status(&fixture.origin, "idle").unwrap();

    assert_eq!(
        restored.append(&fixture.origin, changed).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );

    assert_eq!(restored.tasks.len(), 1);
}

#[test]
fn a_missing_pinned_input_is_restored_only_from_hash_matching_retained_data() {
    let (fixture, run, original) = admitted();
    let path = fixture.context.home.join("data/sessions/index.json");

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.sessions.get_mut(&run.id).unwrap().original = None;
        store.save().unwrap();
    }

    let restored = state::Store::load(&path).unwrap();

    assert_eq!(restored.sessions[&run.id].original.as_ref().unwrap().content, original.content);

    let context = restored.sessions[&run.id].context();

    assert_eq!(context.iter().filter(|message| message.id == original.id).count(), 1);
}

#[test]
fn a_changed_pinned_body_blocks_resume_until_the_accepted_input_is_restored() {
    let (fixture, run, original) = admitted();
    let path = fixture.context.home.join("data/sessions/index.json");

    let budget = {
        let mut store = fixture.context.sessions.lock().unwrap();
        let session = store.sessions.get_mut(&run.id).unwrap();
        session.original.as_mut().unwrap().content =
            vec![Content::Text { text: "A substituted request with the same identity.".into() }];
        store.save().unwrap();
        store.tasks[&run.id].budget.clone()
    };

    let mut restored = state::Store::load(&path).unwrap();

    assert_eq!(restored.tasks[&run.id].status, TaskStatus::Blocked);

    let command = Command {
        id: "resume-changed-input".into(),
        action: Action::Resume,
        revision: restored.tasks[&run.id].revision,
        instruction: String::new(),
    };

    let error = restored.control_task(&run.id, &command).unwrap_err();

    assert_eq!(error.get_ref().unwrap().downcast_ref::<ControlError>(), Some(&ControlError::Input));
    assert_eq!(restored.tasks[&run.id].budget, budget);

    restored.reserve(&fixture.origin, "repair".into()).unwrap();
    restored.append_reserved(&fixture.origin, "repair", original.clone()).unwrap();

    assert_eq!(restored.sessions[&run.id].original.as_ref().unwrap().content, original.content);
    assert_eq!(restored.tasks[&run.id].status, TaskStatus::Blocked);
    assert_eq!(restored.tasks[&run.id].budget, budget);

    let command = Command {
        id: "resume-restored-input".into(),
        revision: restored.tasks[&run.id].revision,
        ..command
    };

    restored.control_task(&run.id, &command).unwrap();

    assert_eq!(restored.tasks[&run.id].status, TaskStatus::Queued);
    assert_eq!(restored.tasks[&run.id].budget, budget);
}

#[tokio::test]
async fn verified_input_repair_survives_write_failure_and_resumes_with_details_beyond_the_preview()
{
    let tail = "The final requirement is to include the orchid dataset.";
    let full = format!("{}\n{tail}", "Context for the report. ".repeat(4_000));
    let (mut fixture, run, original) = admitted_with(Some(&full));
    let path = fixture.context.home.join("data/sessions/index.json");
    {
        let mut store = fixture.context.sessions.lock().unwrap();

        assert!(!store.tasks[&run.id].instruction.contains(tail));
        store.tasks.get_mut(&run.id).unwrap().checkpoint = "The first section is verified.".into();
        store.sessions.get_mut(&fixture.origin).unwrap().messages.clear();
        let session = store.sessions.get_mut(&run.id).unwrap();
        session.messages.clear();
        session.original = None;
        store.save().unwrap();
    }

    fixture.context.sessions = Arc::new(Mutex::new(state::Store::load(&path).unwrap()));
    let provider = fixture.provider("context-success").await;
    let worker = fixture.start();
    fixture
        .until(|store| {
            store.tasks[&run.id].status == TaskStatus::Blocked
                && store.tasks[&run.id].notice.is_none()
        })
        .await;
    assert!(!fixture.context.home.join("generated").exists());
    fs::write(fixture.context.home.join("partial.txt"), "Verified section one.").unwrap();
    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let budget = store.tasks[&run.id].budget.clone();
        let revision = store.tasks[&run.id].revision;

        assert!(
            store
                .control_task(
                    &run.id,
                    &Command {
                        id: "premature-resume".into(),
                        action: Action::Resume,
                        revision,
                        instruction: String::new()
                    }
                )
                .is_err()
        );
        store.reserve(&fixture.origin, "repair".into()).unwrap();
        let backup = path.with_extension("before-repair");
        fs::rename(&path, &backup).unwrap();
        fs::create_dir(&path).unwrap();

        assert!(store.append_reserved(&fixture.origin, "repair", original.clone()).is_err());
        assert!(store.sessions[&run.id].original.is_none());
        assert_eq!(store.tasks[&run.id].revision, revision);
        assert_eq!(store.tasks[&run.id].budget, budget);
        fs::remove_dir(&path).unwrap();
        fs::rename(&backup, &path).unwrap();

        store.append_reserved(&fixture.origin, "repair", original).unwrap();

        assert_eq!(store.tasks[&run.id].status, TaskStatus::Blocked);
        assert_eq!(store.tasks[&run.id].budget, budget);
        assert!(
            store.sessions[&fixture.origin]
                .messages
                .iter()
                .all(|message| message.role != Role::User)
        );

        assert!(store.sessions[&run.id].context()[0].content[0].render().contains(tail));
        let revision = store.tasks[&run.id].revision;
        store
            .control_task(
                &run.id,
                &Command {
                    id: "resume-repaired".into(),
                    action: Action::Resume,
                    revision,
                    instruction: String::new(),
                },
            )
            .unwrap();
        store.set_status(&fixture.origin, "idle").unwrap();
    }

    fixture.context.plugins.tasks.wake.notify_one();
    fixture.until(|store| store.tasks[&run.id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "1");
    assert!(
        !fs::read_to_string(fixture.context.home.join("requests.jsonl")).unwrap().contains(tail)
    );

    assert!(fs::read_to_string(fixture.context.home.join("paged")).unwrap().contains(tail));

    assert_eq!(
        fs::read_to_string(fixture.context.home.join("partial.txt")).unwrap(),
        "Verified section one."
    );
    let store = fixture.context.sessions.lock().unwrap();

    assert_eq!(store.tasks.len(), 1);
    assert_eq!(
        store.tasks[&run.id].result.as_deref(),
        Some("Verified report includes the orchid dataset.")
    );
}

#[tokio::test]
async fn background_handoff_and_checkpointing_preserve_the_complete_original_input() {
    let tail = "The final requirement is to include the orchid dataset.";
    let full = format!("{}\n{tail}", "Context for the report. ".repeat(4_000));
    let (fixture, run, _) = admitted_with(Some(&full));
    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.promote_foreground(&run, "Prepare the concise report.").unwrap();
        store.sessions.get_mut(&fixture.origin).unwrap().messages.clear();
        store.sessions.get_mut(&run.id).unwrap().messages.clear();
        store.save().unwrap();
    }

    let provider = fixture.provider("context-report").await;
    let worker = fixture.start();
    fixture.until(|store| store.tasks[&run.id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    let requests = fs::read_to_string(fixture.context.home.join("requests.jsonl")).unwrap();

    let paged = fs::read_to_string(fixture.context.home.join("paged")).unwrap();

    assert_eq!(requests.lines().count(), 2);
    assert!(requests.lines().all(|request| request.len() < full.len()));
    assert!(requests.lines().all(|request| !request.contains(tail)));
    assert!(requests.lines().all(|request| request.contains("characters are shown")));
    assert_eq!(paged.lines().count(), 2);
    assert!(paged.lines().all(|page| page.contains(tail)));
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");
    let store = fixture.context.sessions.lock().unwrap();

    assert_eq!(store.tasks.len(), 1);
    assert_eq!(
        store.tasks[&run.id].result.as_deref(),
        Some("Verified report includes the orchid dataset.")
    );

    assert!(
        store.sessions[&fixture.origin]
            .messages
            .iter()
            .any(|message| message.role == Role::Assistant
                && message.content[0].render() == "Verified report includes the orchid dataset.")
    );
}

#[test]
fn legacy_unknown_input_is_preserved_without_fabricating_a_fingerprint_or_resetting_budget() {
    let (fixture, run, original) = admitted();
    let path = fixture.context.home.join("data/sessions/index.json");
    let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let budget = saved["jobs"][&run.id]["budget"].clone();
    saved["version"] = json!(9);
    saved["jobs"][&run.id].as_object_mut().unwrap().remove("input");
    krabb_file::save(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let mut restored = state::Store::load(&path).unwrap();

    assert!(restored.tasks[&run.id].input.as_ref().unwrap().fingerprint.is_none());
    let migrated: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    assert_eq!(migrated["version"], 15);
    assert_eq!(migrated["jobs"][&run.id]["budget"], budget);
    restored.sessions.get_mut(&fixture.origin).unwrap().messages.clear();
    restored.reserve(&fixture.origin, "retry".into()).unwrap();

    assert_eq!(
        restored.append_reserved(&fixture.origin, "retry", original).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );

    assert_eq!(restored.tasks.len(), 1);
}

#[tokio::test]
async fn an_unfit_inference_admission_blocks_once_without_generation_budget_use_or_retry_growth() {
    let full = "Context for the report. ".repeat(4_000);
    let (fixture, run, _) = admitted_with(Some(&full));
    let budget = fixture.context.sessions.lock().unwrap().tasks[&run.id].budget.clone();
    let provider = fixture.provider("context-local").await;
    let worker = fixture.start();
    fixture
        .until(|store| {
            store.tasks[&run.id].status == TaskStatus::Blocked
                && store.tasks[&run.id].notice.is_none()
        })
        .await;
    fixture.stop(worker).await;
    provider.stop().await.unwrap();
    let store = fixture.context.sessions.lock().unwrap();

    assert_eq!(store.tasks[&run.id].failures, 0);
    assert_eq!(store.tasks[&run.id].budget, budget);
    assert!(store.tasks[&run.id].last_notice.contains("predicted inference"));
    assert!(store.tasks[&run.id].last_notice.contains("split the work"));
    assert!(!fixture.context.home.join("generated").exists());
    assert!(fixture.context.plugins.inference.samples().is_empty());
}

#[test]
fn missing_current_identity_and_malformed_fingerprints_fail_closed_without_rewriting_data() {
    let (fixture, run, _) = admitted();
    let path = fixture.context.home.join("data/sessions/index.json");
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    for damage in [
        Value::Null,
        json!({}),
        json!({"fingerprint":"not-a-digest"}),
        json!({"fingerprint":"A".repeat(64)}),
        json!({"fingerprint":"a".repeat(65)}),
    ] {
        let mut document = saved.clone();
        document["jobs"][&run.id]["input"] = damage;
        let bytes = serde_json::to_vec(&document).unwrap();
        krabb_file::save(&path, &bytes).unwrap();

        assert!(state::Store::load(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    let mut absent = saved;
    absent["jobs"][&run.id].as_object_mut().unwrap().remove("input");
    let bytes = serde_json::to_vec(&absent).unwrap();
    krabb_file::save(&path, &bytes).unwrap();

    assert!(state::Store::load(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[tokio::test]
async fn legacy_duplicate_requests_are_fenced_on_restart_without_a_generation_or_lost_checkpoint() {
    let (mut fixture, run, _) = admitted();
    let other = "task-999999999999";
    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.tasks.get_mut(&run.id).unwrap().checkpoint = "Verified section one.".into();
        let mut duplicate = store.tasks[&run.id].clone();
        duplicate.id = other.into();
        duplicate.status = TaskStatus::Queued;
        store.tasks.insert(other.into(), duplicate);
        store.ensure_task(other, "model").unwrap();
        store.save().unwrap();
    }

    let path = fixture.context.home.join("data/sessions/index.json");
    fixture.context.sessions = Arc::new(Mutex::new(state::Store::load(&path).unwrap()));
    {
        let mut store = fixture.context.sessions.lock().unwrap();

        for id in [&run.id, &other.to_owned()] {
            assert_eq!(store.tasks[id].status, TaskStatus::Blocked);
            assert_eq!(store.tasks[id].checkpoint, "Verified section one.");
            let revision = store.tasks[id].revision;
            let error = store
                .control_task(
                    id,
                    &Command {
                        id: "resume".into(),
                        action: Action::Resume,
                        revision,
                        instruction: String::new(),
                    },
                )
                .unwrap_err();
            assert!(
                error
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<ControlError>()
                    .is_some_and(|error| *error == ControlError::Identity)
            );

            assert!(store.claim_task(id, unix_seconds()).is_err());
            assert!(store.check_task_step(id, store.tasks[id].step).is_err());
        }
    }

    let provider = fixture.provider("foreground-success").await;
    let worker = fixture.start();
    fixture.until(|store| store.tasks.values().all(|task| task.notice.is_none())).await;
    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    assert!(!fixture.context.home.join("generated").exists());
    let restored = state::Store::load(path).unwrap();

    assert!(
        restored
            .tasks
            .values()
            .all(|task| task.status == TaskStatus::Blocked && task.notice.is_none())
    );

    assert_eq!(
        restored.sessions[&fixture.origin]
            .messages
            .iter()
            .filter(|message| message.role == Role::Assistant
                && message.content[0].render().contains("request identity"))
            .count(),
        1
    );
}

#[test]
fn cancelling_a_duplicate_lets_the_kept_task_resume_but_a_completed_one_still_fences() {
    let (fixture, run, _) = admitted();
    let path = fixture.context.home.join("data/sessions/index.json");
    let control = |store: &mut state::Store, id: &str, command: &str, action: Action| {
        let revision = store.tasks[id].revision;

        store.control_task(
            id,
            &Command { id: command.into(), action, revision, instruction: String::new() },
        )
    };

    {
        let mut store = fixture.context.sessions.lock().unwrap();

        for (other, status) in
            [("task-999999999998", TaskStatus::Queued), ("task-999999999999", TaskStatus::Queued)]
        {
            let mut duplicate = store.tasks[&run.id].clone();
            duplicate.id = other.into();
            duplicate.status = status;
            store.tasks.insert(other.into(), duplicate);
            store.ensure_task(other, "model").unwrap();
        }

        store.save().unwrap();
    }

    let mut store = state::Store::load(&path).unwrap();

    control(&mut store, "task-999999999998", "cancel-copy", Action::Cancel).unwrap();

    assert!(control(&mut store, &run.id, "resume-early", Action::Resume).is_err());

    store.tasks.get_mut("task-999999999999").unwrap().status = TaskStatus::Completed;
    store.save().unwrap();

    let mut store = state::Store::load(&path).unwrap();
    let fenced = control(&mut store, &run.id, "resume-completed", Action::Resume).unwrap_err();

    assert!(fenced.to_string().contains(&ControlError::Identity.to_string()));

    store.tasks.get_mut("task-999999999999").unwrap().status = TaskStatus::Cancelled;
    store.tasks.get_mut("task-999999999998").unwrap().uncertain = true;

    assert!(control(&mut store, &run.id, "resume-uncertain", Action::Resume).is_err());

    store.tasks.get_mut("task-999999999998").unwrap().uncertain = false;
    store.save().unwrap();

    let mut store = state::Store::load(&path).unwrap();
    let receipt = control(&mut store, &run.id, "resume-kept", Action::Resume).unwrap();

    assert_eq!(receipt.state, TaskStatus::Queued);
    assert!(store.claim_task(&run.id, unix_seconds()).is_ok());
}

#[test]
fn a_reply_reference_stays_out_of_the_saved_task_goal() {
    let reply = krabb_core::reply::Reply::quoted("A long earlier message.".into()).unwrap();

    let (fixture, run, _) = admitted_content(Some(vec![
        Content::Text { text: reply.context() },
        Content::Text { text: "Summarize this.".into() },
    ]));

    let store = fixture.context.sessions.lock().unwrap();

    assert_eq!(store.tasks[&run.id].instruction, "Summarize this.");
}
