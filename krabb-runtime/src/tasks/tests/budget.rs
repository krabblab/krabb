use super::*;
use krabb_core::task::Run;

fn claim(fixture: &Fixture) -> Run {
    let task =
        fixture.context.sessions.lock().unwrap().claim_task(&fixture.id, unix_seconds()).unwrap();

    Run { id: task.id, generation: task.step }
}

#[test]
fn reservations_precede_dispatch_and_restart_retains_the_same_remaining_allowance() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let run = claim(&fixture);
    let path = fixture.context.home.join("data/sessions/index.json");

    let first =
        fixture.context.sessions.lock().unwrap().reserve_budget(&run, 4_000, Some(5_000)).unwrap();

    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    assert_eq!(saved["jobs"][&fixture.id]["budget"]["pending"]["id"], first.id);
    assert_eq!(saved["version"], 15);

    let mut store = fixture.context.sessions.lock().unwrap();

    store.settle_budget(&run.id, first, Some(1_000)).unwrap();
    store
        .checkpoint_task(
            &run.id,
            "The first unit is saved.",
            TaskStatus::Waiting,
            unix_seconds(),
            None,
        )
        .unwrap();

    drop(store);

    let retry = claim(&fixture);

    let pending =
        fixture.context.sessions.lock().unwrap().reserve_budget(&retry, 4_000, None).unwrap();

    let mut restored = state::Store::load(&path).unwrap();
    let budget = restored.tasks[&run.id].budget.as_ref().unwrap();

    assert_eq!(budget.spent_millis, 1_000);
    assert_eq!(budget.held_millis, 4_000);
    assert!(budget.pending.is_none());
    assert_eq!(budget.remaining(None).unwrap(), 0);

    restored.settle_budget(&run.id, pending, Some(0)).unwrap();

    assert_eq!(restored.tasks[&run.id].budget.as_ref().unwrap().remaining(None).unwrap(), 0);

    for (id, action) in [
        ("budget-pause", krabb_core::task::Action::Pause),
        ("budget-resume", krabb_core::task::Action::Resume),
    ] {
        let revision = restored.tasks[&run.id].revision;
        restored
            .control_task(
                &run.id,
                &krabb_core::task::Command {
                    id: id.into(),
                    action,
                    revision,
                    instruction: String::new(),
                },
            )
            .unwrap();
    }

    assert_eq!(restored.tasks[&run.id].budget.as_ref().unwrap().remaining(None).unwrap(), 0);

    let next = restored.claim_task(&run.id, unix_seconds()).unwrap();

    assert!(restored.reserve_budget(&Run { id: next.id, generation: next.step }, 1, None).is_err());
}

#[test]
fn failed_reservation_and_settlement_writes_preserve_the_previous_authoritative_record() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let run = claim(&fixture);
    let path = fixture.context.home.join("data/sessions/index.json");
    let backup = path.with_extension("saved");

    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();

    assert!(
        fixture.context.sessions.lock().unwrap().reserve_budget(&run, 1_000, Some(2_000)).is_err()
    );

    assert!(
        fixture.context.sessions.lock().unwrap().tasks[&run.id]
            .budget
            .as_ref()
            .unwrap()
            .pending
            .is_none()
    );

    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();

    let reservation =
        fixture.context.sessions.lock().unwrap().reserve_budget(&run, 1_000, Some(2_000)).unwrap();

    let before = fs::read(&path).unwrap();

    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();

    assert!(
        fixture
            .context
            .sessions
            .lock()
            .unwrap()
            .settle_budget(&run.id, reservation, Some(100))
            .is_err()
    );

    assert_eq!(
        fixture.context.sessions.lock().unwrap().tasks[&run.id].budget.as_ref().unwrap().pending,
        Some(reservation)
    );

    assert_eq!(fs::read(&backup).unwrap(), before);

    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
    fixture.context.sessions.lock().unwrap().recover_task(&run.id).unwrap();

    let restored = state::Store::load(&path).unwrap();

    assert_eq!(restored.tasks[&run.id].budget.as_ref().unwrap().held_millis, 1_000);
}

#[test]
fn old_tasks_preserve_unknown_usage_and_missing_current_accounts_fail_closed() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let path = fixture.context.home.join("data/sessions/index.json");
    let flat = fixture.context.home.join("sessions.json");
    let mut document = serde_json::to_value(&*fixture.context.sessions.lock().unwrap()).unwrap();

    document["jobs"][&fixture.id].as_object_mut().unwrap().remove("budget");
    krabb_file::save(&flat, serde_json::to_vec(&document).unwrap()).unwrap();

    assert!(state::Store::load(&flat).unwrap().tasks[&fixture.id].budget.as_ref().unwrap().legacy);

    let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    saved["version"] = json!(7);
    saved["jobs"][&fixture.id].as_object_mut().unwrap().remove("budget");
    krabb_file::save(&path, serde_json::to_vec(&saved).unwrap()).unwrap();

    let restored = state::Store::load(&path).unwrap();
    let budget = restored.tasks[&fixture.id].budget.as_ref().unwrap();

    assert!(budget.legacy);
    assert!(budget.remaining(None).is_ok());
    assert!(budget.remaining(Some(10_000)).is_err());

    for version in [8, 9, 10, 11] {
        saved["version"] = json!(version);

        let damaged = serde_json::to_vec(&saved).unwrap();

        krabb_file::save(&path, &damaged).unwrap();

        assert!(state::Store::load(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), damaged);
    }
}

#[tokio::test]
async fn exhausted_worker_budget_blocks_without_another_model_call_and_preserves_partial_work() {
    let mut fixture = Fixture::new(Principal::Client("desktop".into()));

    fixture.context.config.inference.task_seconds = Some(1);
    Arc::make_mut(&mut fixture.context.plugins.config).inference.task_seconds = Some(1);

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.tasks.get_mut(&fixture.id).unwrap().checkpoint =
            "The first section is saved in partial.txt.".into();
        store.save().unwrap();
    }

    fs::write(fixture.context.home.join("partial.txt"), "Verified first section.").unwrap();

    let provider = fixture.provider("slow").await;
    let worker = fixture.start();

    fixture.until(|_| fixture.context.home.join("generated").exists()).await;

    {
        let store = fixture.context.sessions.lock().unwrap();

        assert_eq!(
            store.tasks[&fixture.id].budget.as_ref().unwrap().pending.unwrap().millis,
            1_000
        );
    }

    super::advance(2).await;

    fixture.until(|store| store.tasks[&fixture.id].failures == 1).await;

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.tasks.get_mut(&fixture.id).unwrap().next = unix_seconds();
        store.save().unwrap();
    }

    fixture.context.plugins.tasks.wake.notify_one();

    fixture.until(|store| store.tasks[&fixture.id].status == TaskStatus::Blocked).await;

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let revision = store.tasks[&fixture.id].revision;
        store
            .control_task(
                &fixture.id,
                &krabb_core::task::Command {
                    id: "budget-retry".into(),
                    action: krabb_core::task::Action::Resume,
                    revision,
                    instruction: String::new(),
                },
            )
            .unwrap();
    }

    fixture.context.plugins.tasks.wake.notify_one();

    fixture.until(|store| store.tasks[&fixture.id].status == TaskStatus::Blocked).await;
    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    let store = state::Store::load(fixture.context.home.join("data/sessions/index.json")).unwrap();
    let budget = store.tasks[&fixture.id].budget.as_ref().unwrap();

    assert_eq!(budget.spent_millis, 0);
    assert_eq!(budget.held_millis, 1_000);
    assert_eq!(budget.remaining(None).unwrap(), 0);
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "1");

    assert_eq!(
        fs::read_to_string(fixture.context.home.join("partial.txt")).unwrap(),
        "Verified first section."
    );

    assert_eq!(store.tasks[&fixture.id].checkpoint, "The first section is saved in partial.txt.");
}

#[test]
fn pending_accounts_are_protected_when_a_terminal_task_is_selected_for_retention() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let run = claim(&fixture);
    let mut store = fixture.context.sessions.lock().unwrap();
    let receipt = store.reserve_budget(&run, 1_000, None).unwrap();
    let mut template = store.tasks[&run.id].clone();

    template.budget = Some(krabb_core::execution::budget::Budget::default());
    template.status = TaskStatus::Cancelled;
    template.notice = None;
    template.updated = 1;
    store.tasks.get_mut(&run.id).unwrap().status = TaskStatus::Cancelled;
    store.tasks.get_mut(&run.id).unwrap().notice = None;

    assert!(store.tasks[&run.id].protected());

    for index in 0..63 {
        let mut task = template.clone();

        task.id = format!("task-{}", index + 1);
        store.tasks.insert(task.id.clone(), task);
    }

    store.save().unwrap();

    let mut incoming = template;

    incoming.id = "task-1000".into();
    incoming.status = TaskStatus::Queued;
    store.enqueue_task(incoming).unwrap();

    assert_eq!(store.tasks.len(), 64);
    assert!(store.tasks.contains_key(&run.id));
    assert_eq!(store.tasks[&run.id].budget.as_ref().unwrap().pending, Some(receipt));
}

#[test]
fn budget_grants_are_durable_scoped_idempotent_and_do_not_clear_unknown_exposure() {
    use krabb_core::execution::budget::Grant;
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let run = claim(&fixture);
    let mut store = fixture.context.sessions.lock().unwrap();
    let reserved = store.reserve_budget(&run, 1_000, Some(1_000)).unwrap();

    let initial = Grant {
        id: "grant-1".into(),
        revision: store.tasks[&run.id].revision,
        sequence: reserved.id,
        limit_millis: 2_000,
    };

    assert!(store.grant_budget(&run.id, &initial, Some(2_000)).is_err());

    store.settle_budget(&run.id, reserved, None).unwrap();
    store
        .checkpoint_task(
            &run.id,
            "Review the inference allowance.",
            TaskStatus::Blocked,
            unix_seconds(),
            None,
        )
        .unwrap();

    let initial = Grant { revision: store.tasks[&run.id].revision, ..initial };

    assert!(store.grant_budget(&run.id, &initial, Some(1_000)).is_err());

    let receipt = store.grant_budget(&run.id, &initial, Some(2_000)).unwrap();

    assert_eq!(store.grant_budget(&run.id, &initial, Some(2_000)).unwrap(), receipt);
    assert_eq!(store.tasks[&run.id].budget.as_ref().unwrap().held_millis, 1_000);
    assert_eq!(store.tasks[&run.id].budget.as_ref().unwrap().remaining(None).unwrap(), 1_000);

    assert!(
        store
            .grant_budget(&run.id, &Grant { id: "stale".into(), ..initial.clone() }, Some(3_000))
            .is_err()
    );

    assert!(
        store
            .grant_budget(&run.id, &Grant { limit_millis: 3_000, ..initial.clone() }, Some(3_000))
            .is_err()
    );

    let path = fixture.context.home.join("data/sessions/index.json");

    let next = Grant {
        id: "grant-2".into(),
        revision: store.tasks[&run.id].revision,
        sequence: initial.sequence,
        limit_millis: 3_000,
    };

    let backup = path.with_extension("grant-backup");

    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();

    assert!(store.grant_budget(&run.id, &next, Some(3_000)).is_err());
    assert_eq!(store.tasks[&run.id].budget.as_ref().unwrap().limit_millis, Some(2_000));
    assert_eq!(store.tasks[&run.id].grants.len(), 1);

    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
    store.grant_budget(&run.id, &next, Some(3_000)).unwrap();
    drop(store);

    let mut restored = state::Store::load(path).unwrap();

    assert_eq!(restored.grant_budget(&run.id, &initial, Some(3_000)).unwrap(), receipt);
    assert_eq!(restored.tasks[&run.id].budget.as_ref().unwrap().limit_millis, Some(3_000));
    assert_eq!(restored.tasks[&run.id].budget.as_ref().unwrap().held_millis, 1_000);
}

#[tokio::test]
async fn granting_more_allowance_resumes_the_original_goal_without_resetting_prior_exposure() {
    let mut fixture = Fixture::new(Principal::Client("desktop".into()));

    {
        let run = claim(&fixture);
        let mut store = fixture.context.sessions.lock().unwrap();
        let reserved = store.reserve_budget(&run, 10_000, Some(10_000)).unwrap();
        store.settle_budget(&run.id, reserved, None).unwrap();
        store
            .checkpoint_task(
                &run.id,
                "The remaining report still needs verification.",
                TaskStatus::Blocked,
                unix_seconds(),
                Some("More inference allowance is required."),
            )
            .unwrap();
        store.publish_task_notice(&run.id).unwrap();
        let task = &store.tasks[&run.id];
        let grant = krabb_core::execution::budget::Grant {
            id: "approved-budget".into(),
            revision: task.revision,
            sequence: task.budget.as_ref().unwrap().sequence,
            limit_millis: 20_000,
        };

        store.grant_budget(&run.id, &grant, Some(20_000)).unwrap();

        assert_eq!(store.tasks[&run.id].status, TaskStatus::Blocked);
        let revision = store.tasks[&run.id].revision;
        store
            .control_task(
                &run.id,
                &krabb_core::task::Command {
                    id: "resume-budget".into(),
                    action: krabb_core::task::Action::Resume,
                    revision,
                    instruction: String::new(),
                },
            )
            .unwrap();
    }

    fixture.context.config.inference.task_seconds = Some(20);

    Arc::make_mut(&mut fixture.context.plugins.config).inference.task_seconds = Some(20);
    fs::write(fixture.context.home.join("generated"), "1").unwrap();

    let provider = fixture.provider("continue").await;
    let worker = fixture.start();

    fixture.until(|store| store.tasks[&fixture.id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    let store = fixture.context.sessions.lock().unwrap();
    let budget = store.tasks[&fixture.id].budget.as_ref().unwrap();

    assert_eq!(budget.held_millis, 10_000);
    assert!(budget.spent_millis <= 10_000);
    assert_eq!(budget.limit_millis, Some(20_000));
    assert_eq!(store.tasks[&fixture.id].grants.len(), 1);
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");

    assert!(store.sessions[&fixture.origin].messages.iter().any(|message| {
        message.role == Role::Assistant
            && message
                .content
                .iter()
                .any(|content| content.render() == "Verified complete report. ".repeat(250))
    }));
}

#[tokio::test]
async fn the_live_host_reconciles_orphaned_terminal_reservations_without_restarting_work() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let run = claim(&fixture);

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.reserve_budget(&run, 1_000, Some(2_000)).unwrap();
        let task = store.tasks.get_mut(&run.id).unwrap();
        task.status = TaskStatus::Cancelled;
        task.notice = None;
        store.save().unwrap();
    }

    let worker = fixture.start();

    fixture.until(|store| store.tasks[&run.id].budget.as_ref().unwrap().pending.is_none()).await;
    fixture.stop(worker).await;

    let restored =
        state::Store::load(fixture.context.home.join("data/sessions/index.json")).unwrap();

    assert_eq!(restored.tasks[&run.id].status, TaskStatus::Cancelled);

    let budget = restored.tasks[&run.id].budget.as_ref().unwrap();

    assert_eq!(budget.held_millis, 1_000);
    assert_eq!(budget.spent_millis, 0);
    assert_eq!(budget.remaining(None).unwrap(), 1_000);
    assert!(!fixture.context.home.join("generated").exists());
}

#[test]
fn a_conversational_extension_grants_bounded_allowance_once_within_the_host_ceiling() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let run = claim(&fixture);

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let reserved = store.reserve_budget(&run, 60_000, Some(60_000)).unwrap();

        store.settle_budget(&run.id, reserved, Some(60_000)).unwrap();
        store
            .checkpoint_task(&run.id, "Allowance used.", TaskStatus::Blocked, unix_seconds(), None)
            .unwrap();
        store.ensure("other", "model").unwrap();
        store.client("other", "desktop").unwrap();
    }

    let signals = Signals { ceiling: Some(900_000), ..Signals::default() };

    let say = |session: &str, message: &str, minutes: &str| {
        let mut store = fixture.context.sessions.lock().unwrap();
        let input = Message {
            id: message.into(),
            session: session.into(),
            role: Role::User,
            sender: Some("desktop".into()),
            content: vec![Content::Text { text: format!("/task extend {} {minutes}", run.id) }],
        };

        crate::tasks::dialog::message(&mut store, &input, &signals).unwrap()
    };

    let limit = || {
        fixture.context.sessions.lock().unwrap().tasks[&run.id]
            .budget
            .as_ref()
            .unwrap()
            .limit_millis
    };

    assert!(say("other", "foreign", "10").unwrap().contains("not applied"));
    assert_eq!(limit(), Some(60_000));

    let granted = say(&fixture.origin, "extend-1", "10").unwrap();

    assert!(granted.contains("now 11 minutes"), "{granted}");
    assert_eq!(limit(), Some(660_000));
    assert_eq!(say(&fixture.origin, "extend-1", "10").unwrap(), granted);
    assert_eq!(limit(), Some(660_000));

    let stale = crate::tasks::control(
        &mut fixture.context.sessions.lock().unwrap(),
        &json!({"action":"extend", "id":run.id, "minutes":1, "revision":0}),
        &fixture.origin,
        None,
        &signals,
        false,
    );

    assert!(stale.unwrap_err().to_string().contains("The task changed"));
    assert_eq!(limit(), Some(660_000));

    let ceiling = say(&fixture.origin, "extend-2", "5").unwrap();

    assert!(ceiling.contains("host inference ceiling"), "{ceiling}");

    let excessive = say(&fixture.origin, "extend-3", "2880").unwrap();

    assert!(excessive.contains("1440 minutes"), "{excessive}");
    assert_eq!(limit(), Some(660_000));

    let store = fixture.context.sessions.lock().unwrap();

    assert_eq!(store.tasks[&run.id].grants.len(), 1);
    assert_eq!(store.tasks[&run.id].status, TaskStatus::Blocked);
    assert_eq!(store.tasks[&run.id].budget.as_ref().unwrap().spent_millis, 60_000);
}

#[test]
fn a_conversational_extension_reviews_legacy_usage_without_forgetting_held_exposure() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let task = store.tasks.get_mut(&fixture.id).unwrap();
        task.budget = Some(krabb_core::execution::budget::Budget {
            held_millis: 30_000,
            ..krabb_core::execution::budget::Budget::legacy()
        });

        task.status = TaskStatus::Blocked;
        store.save().unwrap();
    }

    let signals = Signals { ceiling: Some(3_600_000), ..Signals::default() };

    let reply = {
        let mut store = fixture.context.sessions.lock().unwrap();

        let input = Message {
            id: "review-1".into(),
            session: fixture.origin.clone(),
            role: Role::User,
            sender: Some("desktop".into()),
            content: vec![Content::Text { text: format!("/task extend {} 10", fixture.id) }],
        };

        crate::tasks::dialog::message(&mut store, &input, &signals).unwrap().unwrap()
    };

    assert!(reply.contains("now 10 minutes and 30 seconds"), "{reply}");

    let path = fixture.context.home.join("data/sessions/index.json");
    let restored = state::Store::load(path).unwrap();
    let budget = restored.tasks[&fixture.id].budget.clone().unwrap();

    assert!(!budget.legacy);
    assert_eq!(budget.limit_millis, Some(630_000));
    assert_eq!(budget.held_millis, 30_000);
    assert_eq!(budget.remaining(Some(3_600_000)).unwrap(), 600_000);
    assert_eq!(restored.tasks[&fixture.id].status, TaskStatus::Blocked);
}

#[tokio::test]
async fn token_allowance_accumulates_estimates_for_missing_usage_and_blocks_before_dispatch() {
    for limit in [1_000_000, 1] {
        let mut fixture = Fixture::new(Principal::Channel("telegram".into()));

        fixture.context.config.inference.task_tokens = Some(limit);
        Arc::make_mut(&mut fixture.context.plugins.config).inference.task_tokens = Some(limit);

        let provider = fixture.provider("continue").await;
        let worker = fixture.start();
        let expected = if limit == 1 { TaskStatus::Blocked } else { TaskStatus::Completed };

        fixture
            .until(|store| {
                store.tasks[&fixture.id].status == expected
                    && store.tasks[&fixture.id].notice.is_none()
            })
            .await;

        fixture.stop(worker).await;
        provider.stop().await.unwrap();

        let store = fixture.context.sessions.lock().unwrap();
        let tokens = store.tasks[&fixture.id].budget.as_ref().unwrap().tokens;

        if limit == 1 {
            assert!(!fixture.context.home.join("generated").exists());
            assert_eq!(tokens, krabb_core::execution::budget::Tokens::default());
            assert!(store.outbox.iter().any(|delivery| delivery.text.contains("token allowance")));
        } else {
            assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");
            assert_eq!(tokens.reported, 0);
            assert!(tokens.estimated > 0);

            let view = crate::accounting::view(store.tasks[&fixture.id].budget.as_ref().unwrap());

            assert_eq!(view["estimated_tokens"], tokens.estimated);
        }
    }
}

#[tokio::test]
async fn a_refused_attempt_releases_its_reservations_and_an_ambiguous_one_keeps_them() {
    let mut estimated = Vec::new();
    let mut held = Vec::new();

    for kind in ["throttled", "transient"] {
        let mut fixture = super::foreground::fixture();

        fixture.context.config.inference.task_tokens = Some(1_000_000);
        Arc::make_mut(&mut fixture.context.plugins.config).inference.task_tokens = Some(1_000_000);

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

        let task = {
            let mut store = fixture.context.sessions.lock().unwrap();

            store.clear(&fixture.origin, "idle").unwrap();
            store.tasks.keys().next().unwrap().clone()
        };

        let worker = fixture.start();

        fixture.until(|store| store.tasks[&task].status == TaskStatus::Completed).await;
        fixture.stop(worker).await;
        provider.stop().await.unwrap();

        let store = fixture.context.sessions.lock().unwrap();

        assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");

        estimated.push(store.tasks[&task].budget.as_ref().unwrap().tokens.estimated);
        held.push(store.tasks[&task].budget.as_ref().unwrap().held_millis);
    }

    assert!(estimated[0] > 0);
    assert!(estimated[0] < estimated[1], "{estimated:?}");
    assert_eq!(held[0], 0);
    assert!(held[1] > 0, "{held:?}");
}

#[test]
fn a_step_reserves_tokens_and_time_together_or_not_at_all() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let run = claim(&fixture);
    let mut store = fixture.context.sessions.lock().unwrap();

    let error = store.reserve_step(&run, 1_000, None, (100, Some(100))).unwrap_err();
    let budget = store.tasks[&run.id].budget.clone().unwrap();

    assert!(error.get_ref().unwrap().is::<state::TokensExhausted>());
    assert!(budget.pending.is_none());
    assert_eq!(budget.tokens.estimated, 0);

    let reserved = store.reserve_step(&run, 1_000, None, (40, Some(100))).unwrap();
    let budget = store.tasks[&run.id].budget.clone().unwrap();

    assert_eq!(budget.pending, Some(reserved));
    assert_eq!(budget.tokens.estimated, 40);
}

#[tokio::test(start_paused = true)]
async fn a_measured_overrun_is_spent_up_to_its_reservation_rather_than_held() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let run = claim(&fixture);

    let mut account = crate::accounting::Account::reserve(
        &fixture.context.sessions,
        Some(&run),
        1_000,
        None,
        (0, None),
    )
    .unwrap()
    .unwrap();

    tokio::time::advance(Duration::from_millis(1_050)).await;
    account.complete().unwrap();

    let store = fixture.context.sessions.lock().unwrap();
    let budget = store.tasks[&run.id].budget.as_ref().unwrap();

    assert_eq!(budget.spent_millis, 1_000);
    assert_eq!(budget.held_millis, 0);
}
