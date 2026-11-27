use super::*;
use krabb_core::task::{Action, Command};

pub(super) fn fixture() -> Fixture {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();

    store.tasks.clear();

    store
        .begin(
            &fixture.origin,
            Message {
                id: "foreground-user".into(),
                session: fixture.origin.clone(),
                role: Role::User,
                sender: Some("desktop".into()),
                content: vec![Content::Text {
            text:"Finish the report using the previous conversation, including the corrected \
                dataset.".into()
        }],
            },
        )
        .unwrap();

    drop(store);
    fixture
}

pub(super) async fn generate(
    context: Context,
    origin: String,
    provider: Arc<Live>,
) -> Result<Answer, PluginError> {
    generate_controlled(context, origin, provider, Arc::new(Cancellation::new())).await
}

async fn generate_controlled(
    context: Context,
    origin: String,
    provider: Arc<Live>,
    cancel: Arc<Cancellation>,
) -> Result<Answer, PluginError> {
    let history = context.sessions.lock().unwrap().sessions[&origin].context();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
    let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });

    let mut call = 1;

    let result = answer(
        &provider,
        &context.plugins,
        "model",
        history,
        &origin,
        "desktop",
        &origin,
        None,
        &context.sessions,
        Some(&context.home),
        &context.home,
        context.config.client_tools("desktop"),
        false,
        false,
        ApprovalMode::Auto,
        Arc::clone(&context.approvals),
        &cancel,
        &context.stop,
        tokio::time::Instant::now() + context.config.inference.envelope(),
        &mut call,
        &mut None,
        None,
        sender,
        None,
        None,
        None,
    )
    .await;

    drain.abort();
    result
}

#[tokio::test]
async fn queued_questions_wait_for_the_active_turn_and_use_its_completed_context() {
    let fixture = fixture();
    let provider = fixture.provider("foreground-success").await;

    let id = {
        let mut store = fixture.context.sessions.lock().unwrap();
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
        let route = routing::Route {
            kind: routing::Kind::Fast,
            message: String::new(),
            instruction: "Explain the second dataset.".into(),
        };

        let task = record(&route, &context).unwrap();
        let input = Message {
            id: "queued-user".into(),
            session: fixture.origin.clone(),
            role: Role::User,
            sender: Some("desktop".into()),
            content: vec![Content::Text { text: route.instruction }],
        };

        let mut history = store.sessions[&fixture.origin].context();
        history.push(input.clone());
        store.queue_foreground(task, history, input).unwrap().id
    };

    let worker = fixture.start();

    tokio::time::sleep(Duration::from_millis(120)).await;

    assert!(!fixture.context.home.join("generated").exists());

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store
            .push(
                &fixture.origin,
                Message {
                    id: "previous-answer".into(),
                    session: fixture.origin.clone(),
                    role: Role::Assistant,
                    sender: None,
                    content: vec![Content::Text { text: "The first dataset has 42 rows.".into() }],
                },
            )
            .unwrap();
        store.clear(&fixture.origin, "idle").unwrap();
    }

    fixture.context.plugins.tasks.wake.notify_one();

    fixture.until(|store| store.tasks[&id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    let requests = fs::read_to_string(fixture.context.home.join("requests.jsonl")).unwrap();

    assert_eq!(requests.lines().count(), 1);
    assert!(requests.contains("The first dataset has 42 rows."));
    assert!(requests.contains("Explain the second dataset."));
    assert_eq!(fixture.context.sessions.lock().unwrap().tasks.len(), 1);
}

#[tokio::test]
async fn busy_foreground_requests_pause_and_resume_under_the_same_task_identity() {
    let fixture = fixture();
    let provider = fixture.provider("foreground").await;

    let handle = tokio::spawn(generate(
        fixture.context.clone(),
        fixture.origin.clone(),
        Arc::clone(&provider),
    ));

    fixture.until(|_| fixture.context.home.join("generated").exists()).await;

    let id = {
        let mut store = fixture.context.sessions.lock().unwrap();
        let task = store.tasks.values().next().unwrap().clone();

        assert!(task.foreground);
        assert_eq!(task.status, TaskStatus::Running);
        assert!(store.resolved_context(&task.id).unwrap().iter().any(|message| {
            message.content.iter().any(|content| content.render().contains("corrected dataset"))
        }));

        control(
            &mut store,
            &json!({"action":"pause", "id":task.id}),
            &fixture.origin,
            None,
            &fixture.context.plugins.tasks,
            false,
        )
        .unwrap();
        task.id
    };

    let error =
        tokio::time::timeout(Duration::from_secs(5), handle).await.unwrap().unwrap().unwrap_err();

    assert!(error.is::<Failure>());
    assert!(!fixture.context.plugins.tasks.attached.lock().unwrap().contains(&id));

    {
        let mut store = fixture.context.sessions.lock().unwrap();

        assert_eq!(store.tasks[&id].status, TaskStatus::Paused);

        store.clear(&fixture.origin, "idle").unwrap();
        control(
            &mut store,
            &json!({"action":"resume", "id":id}),
            &fixture.origin,
            None,
            &fixture.context.plugins.tasks,
            false,
        )
        .unwrap();
    }

    let worker = fixture.start();

    fixture.until(|store| store.tasks[&id].status == TaskStatus::Completed).await;

    {
        let store = fixture.context.sessions.lock().unwrap();

        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[&id].step, 2);
        assert_eq!(store.tasks[&id].result.as_deref(), Some("The saved request is complete."));
        assert_eq!(
            store.sessions[&fixture.origin].messages.last().unwrap().content[0].render(),
            "The saved request is complete."
        );
    }

    fixture.stop(worker).await;

    provider.stop().await.unwrap();

    let requests = fs::read_to_string(fixture.context.home.join("requests.jsonl")).unwrap();

    assert_eq!(requests.lines().count(), 2);
    assert!(requests.lines().last().unwrap().contains("corrected dataset"));
}

#[tokio::test]
async fn a_live_delivery_guard_prevents_duplicate_foreground_publication() {
    let fixture = fixture();
    let provider = fixture.provider("foreground-success").await;

    let answer = generate(fixture.context.clone(), fixture.origin.clone(), Arc::clone(&provider))
        .await
        .unwrap();

    let id = answer.foreground.as_ref().unwrap().run.id.clone();
    let worker = fixture.start();

    tokio::time::sleep(Duration::from_millis(80)).await;

    {
        let mut store = fixture.context.sessions.lock().unwrap();

        assert_eq!(store.tasks[&id].status, TaskStatus::Reporting);
        assert!(
            !store.sessions[&fixture.origin]
                .messages
                .iter()
                .any(|message| message.role == Role::Assistant)
        );

        store
            .client_foreground_reply(
                Some(&id),
                Message {
                    id: "client-result".into(),
                    session: fixture.origin.clone(),
                    role: Role::Assistant,
                    sender: None,
                    content: vec![Content::Text { text: answer.text.clone() }],
                },
            )
            .unwrap();
    }

    drop(answer);

    tokio::time::sleep(Duration::from_millis(80)).await;

    {
        let store = fixture.context.sessions.lock().unwrap();

        assert_eq!(store.tasks[&id].status, TaskStatus::Completed);
        assert_eq!(
            store.sessions[&fixture.origin]
                .messages
                .iter()
                .filter(|message| message.role == Role::Assistant)
                .count(),
            1
        );

        assert!(store.tasks[&id].notice.is_none());
    }

    fixture.stop(worker).await;

    provider.stop().await.unwrap();
}

#[tokio::test]
async fn a_result_survives_the_loss_of_its_foreground_delivery_path() {
    let mut fixture = fixture();
    let provider = fixture.provider("foreground-success").await;

    let answer = generate(fixture.context.clone(), fixture.origin.clone(), Arc::clone(&provider))
        .await
        .unwrap();

    let id = answer.foreground.as_ref().unwrap().run.id.clone();

    drop(answer);

    let path = fixture.context.home.join("data/sessions/index.json");

    fixture.context.sessions = Arc::new(Mutex::new(state::Store::load(&path).unwrap()));

    let worker = fixture.start();

    fixture.until(|store| store.tasks[&id].status == TaskStatus::Completed).await;

    {
        let store = fixture.context.sessions.lock().unwrap();

        assert_eq!(
            store.sessions[&fixture.origin]
                .messages
                .iter()
                .filter(|message| message.role == Role::Assistant)
                .count(),
            1
        );

        assert_eq!(store.tasks[&id].result.as_deref(), Some("The saved request is complete."));
    }

    fixture.stop(worker).await;

    provider.stop().await.unwrap();

    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "1");
}

#[test]
fn interrupting_the_current_turn_preserves_independent_queued_requests() {
    let fixture = fixture();
    let mut store = fixture.context.sessions.lock().unwrap();

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

    let history = store.sessions[&fixture.origin].context();

    let running = Foreground::admit(
        &mut store,
        Arc::clone(&fixture.context.plugins.tasks),
        &context,
        history.clone(),
        &Cancellation::new(),
    )
    .unwrap();

    let route = routing::Route {
        kind: routing::Kind::Fast,
        message: String::new(),
        instruction: "Another question".into(),
    };

    let input = Message {
        id: "queued-input".into(),
        session: fixture.origin.clone(),
        role: Role::User,
        sender: Some("desktop".into()),
        content: vec![Content::Text { text: route.instruction.clone() }],
    };

    let queued = record(&route, &context).unwrap();

    let queued = store
        .queue_foreground(queued, history.into_iter().chain([input.clone()]).collect(), input)
        .unwrap();

    assert!(store.claim_task(&queued.id, u64::MAX).is_err());

    store.interrupt_foreground(&fixture.origin).unwrap();

    assert_eq!(store.tasks[&running.run.id].status, TaskStatus::Cancelling);
    assert_eq!(store.tasks[&queued.id].status, TaskStatus::Queued);
    assert!(store.claim_task(&queued.id, u64::MAX).is_err());

    store.finish_stop(&running.run.id, running.run.generation).unwrap();
    store.clear(&fixture.origin, "idle").unwrap();
    store.claim_task(&queued.id, u64::MAX).unwrap();

    assert_eq!(store.tasks[&running.run.id].status, TaskStatus::Cancelled);
    assert_eq!(store.tasks[&queued.id].status, TaskStatus::Running);
}

#[test]
fn a_foreground_journal_is_not_modified_by_a_stopped_generation() {
    let fixture = fixture();
    let mut store = fixture.context.sessions.lock().unwrap();

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

    let history = store.sessions[&fixture.origin].context();

    let guard = Foreground::admit(
        &mut store,
        Arc::clone(&fixture.context.plugins.tasks),
        &context,
        history,
        &Cancellation::new(),
    )
    .unwrap();

    let run = &guard.run;

    let message = Message {
        id: "draft".into(),
        session: fixture.origin.clone(),
        role: Role::Assistant,
        sender: None,
        content: vec![Content::Text { text: "A public partial draft.".into() }],
    };

    store.push_turn(&fixture.origin, Some(run), message.clone()).unwrap();

    let before = store.sessions[&run.id].messages.len();
    let revision = store.tasks[&run.id].revision;

    store
        .control_task(
            &run.id,
            &Command {
                id: "pause-1".into(),
                action: Action::Pause,
                revision,
                instruction: String::new(),
            },
        )
        .unwrap();

    assert!(store.push_turn(&fixture.origin, Some(run), message).is_err());
    assert_eq!(store.sessions[&run.id].messages.len(), before);
    assert_eq!(store.sessions[&run.id].messages.last().unwrap().session, run.id);
}

#[test]
fn resuming_closes_interrupted_tool_exchanges_without_inventing_results() {
    let fixture = fixture();
    let mut store = fixture.context.sessions.lock().unwrap();

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

    let mut history = store.sessions[&fixture.origin].context();

    history.push(Message {
        id: "read-request".into(),
        session: fixture.origin.clone(),
        role: Role::Assistant,
        sender: None,
        content: vec![Content::ToolCall {
            name: "read".into(),
            args: json!({"path":"report.txt"}),
            id: Some("read-1".into()),
            thought_signature: None,
        }],
    });

    let guard = Foreground::admit(
        &mut store,
        Arc::clone(&fixture.context.plugins.tasks),
        &context,
        history,
        &Cancellation::new(),
    )
    .unwrap();

    let settled = store.foreground_history(&guard.run.id).unwrap();
    let receipt = settled.last().unwrap();

    assert_eq!(receipt.role, Role::Tool);
    assert_eq!(receipt.sender.as_deref(), Some("read"));
    assert!(receipt.content[0].render().contains("Do not assume this operation succeeded"));
    assert_eq!(store.foreground_history(&guard.run.id).unwrap().len(), settled.len());
    assert!(store.tasks[&guard.run.id].result.is_none());
}

#[test]
fn restart_does_not_requeue_the_conversation_request_as_a_second_task() {
    let fixture = fixture();
    let path = fixture.context.home.join("data/sessions/index.json");

    let id = {
        let mut store = fixture.context.sessions.lock().unwrap();
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
        let history = store.sessions[&fixture.origin].context();
        let guard = Foreground::admit(
            &mut store,
            Arc::clone(&fixture.context.plugins.tasks),
            &context,
            history,
            &Cancellation::new(),
        )
        .unwrap();
        guard.run.id.clone()
    };

    let recovered = state::Store::load(&path).unwrap();

    assert_eq!(recovered.tasks.len(), 1);
    assert_eq!(recovered.tasks[&id].status, TaskStatus::Queued);
    assert_eq!(recovered.sessions[&fixture.origin].status, "idle");
    assert!(recovered.sessions[&fixture.origin].inflight.is_none());
    assert!(recovered.sessions[&fixture.origin].queued.is_empty());
    assert!(
        recovered
            .resolved_context(&id)
            .unwrap()
            .iter()
            .any(|message| message.id == "foreground-user")
    );
}

#[test]
fn admission_failure_does_not_publish_a_task_or_orphan_its_journal() {
    let fixture = fixture();
    let mut store = fixture.context.sessions.lock().unwrap();

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

    let history = store.sessions[&fixture.origin].context();

    store.save().unwrap();
    store.record_limit = Some(1);

    assert!(
        Foreground::admit(
            &mut store,
            Arc::clone(&fixture.context.plugins.tasks),
            &context,
            history,
            &Cancellation::new()
        )
        .is_err()
    );

    assert!(store.tasks.is_empty());
    assert!(store.sessions.values().all(|session| !session.is_work()));
    assert!(fixture.context.plugins.tasks.attached.lock().unwrap().is_empty());
    assert!(fixture.context.plugins.tasks.cancels.lock().unwrap().is_empty());

    let recovered =
        state::Store::load(fixture.context.home.join("data/sessions/index.json")).unwrap();

    assert!(recovered.tasks.is_empty());
    assert!(recovered.sessions.values().all(|session| !session.is_work()));
}

#[tokio::test]
async fn a_tool_listing_failure_leaves_no_foreground_task_to_recover() {
    const TOOLS: &str = r#"
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request.get('method')
    if method == 'hello':
        reply = {'result':{'protocol':{'major':0,'minor':1},'id':'files','version':'0.1.0',
                 'capabilities':['tool']}}
    elif method == 'tools/list':
        reply = {'error':{'code':-32000,'message':'Tool listing failed.'}}
    else:
        reply = {'result':{}}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'], **reply}), flush=True)
    if method == 'shutdown':
        break
"#;

    let mut fixture = fixture();
    let mut config = fixture.context.config.clone();

    config.clients.get_mut("desktop").unwrap().tools = Some(true);
    fixture.context.config = config.clone();
    fixture.context.plugins.config = Arc::new(config);

    let tools = Process::start_with("python3", ["-u", "-c", TOOLS]).await.unwrap();

    fixture.context.plugins.insert(Live::new(tools)).await;

    let provider = fixture.provider("adaptive").await;

    assert!(
        generate(fixture.context.clone(), fixture.origin.clone(), Arc::clone(&provider))
            .await
            .is_err()
    );

    {
        let store = fixture.context.sessions.lock().unwrap();

        assert!(store.tasks.is_empty());
        assert!(store.sessions.values().all(|session| !session.is_work()));
    }

    provider.stop().await.unwrap();
}

#[test]
fn metadata_only_controls_preserve_existing_transcript_records() {
    let fixture = fixture();
    let mut store = fixture.context.sessions.lock().unwrap();

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

    let history = store.sessions[&fixture.origin].context();

    let guard = Foreground::admit(
        &mut store,
        Arc::clone(&fixture.context.plugins.tasks),
        &context,
        history,
        &Cancellation::new(),
    )
    .unwrap();

    let id = guard.run.id.clone();
    let path = fixture.context.home.join("data/sessions/index.json");
    let before: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let revision = store.tasks[&id].revision;

    store.record_limit = Some(1);
    store
        .control_task(
            &id,
            &Command {
                id: "pause-1".into(),
                action: Action::Pause,
                revision,
                instruction: String::new(),
            },
        )
        .unwrap();

    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    assert_eq!(before["sessions"], after["sessions"]);
    assert_eq!(before["journals"], after["journals"]);
    assert_eq!(after["jobs"][&id]["status"], "pausing");
}

#[tokio::test]
async fn a_healthy_local_model_can_answer_a_fast_request_beyond_the_old_limit() {
    let fixture = fixture();
    let provider = fixture.provider("adaptive").await;
    let answering = fixture.context.clone();
    let origin = fixture.origin.clone();
    let model = Arc::clone(&provider);
    let (notices, mut receiver) = tokio::sync::mpsc::channel(32);
    let history = answering.sessions.lock().unwrap().sessions[&origin].context();

    let task = tokio::spawn(async move {
        answer(
            &model,
            &answering.plugins,
            "model",
            history,
            &origin,
            "desktop",
            &origin,
            None,
            &answering.sessions,
            Some(&answering.home),
            &answering.home,
            false,
            false,
            false,
            ApprovalMode::Auto,
            Arc::clone(&answering.approvals),
            &Cancellation::new(),
            &answering.stop,
            tokio::time::Instant::now() + answering.config.inference.envelope(),
            &mut 1,
            &mut None,
            None,
            notices,
            None,
            None,
            None,
        )
        .await
    });

    fixture.until(|_| fixture.context.home.join("pending").exists()).await;

    let requests = fs::read_to_string(fixture.context.home.join("requests.jsonl")).unwrap();
    let request: serde_json::Value = serde_json::from_str(requests.trim()).unwrap();

    assert!(request["params"]["generation"]["limits"]["first_millis"].as_u64().unwrap() > 80_000);

    super::advance(80).await;

    assert!(!task.is_finished());
    assert!(receiver.try_recv().is_err());

    fs::write(fixture.context.home.join("finish"), "").unwrap();

    let answer =
        tokio::time::timeout(Duration::from_secs(8), task).await.unwrap().unwrap().unwrap();

    assert_eq!(answer.model.text, "Hello!");

    {
        let store = fixture.context.sessions.lock().unwrap();
        let budget = store.tasks.values().next().unwrap().budget.as_ref().unwrap();

        assert!(budget.spent_millis >= 80_000);
        assert_eq!(budget.held_millis, 0);
        assert!(budget.pending.is_none());
    }

    let observed = fixture.context.plugins.inference.samples();

    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].outcome, krabb_core::execution::policy::Outcome::Complete);
    assert!(observed[0].elapsed_millis >= 80_000);
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "1");

    while let Ok(notice) = receiver.try_recv() {
        assert!(!matches!(notice, StreamNotice::Progress(_)));
    }

    provider.stop().await.unwrap();
}

#[tokio::test]
async fn interrupted_model_attempts_are_censored_without_duplicate_generation_or_fabricated_usage()
{
    use krabb_core::execution::policy::Outcome;

    for (mode, expected) in [
        ("cancel", Outcome::Cancelled),
        ("deadline", Outcome::TimedOut),
        ("failed", Outcome::Failed),
        ("drop", Outcome::Interrupted),
    ] {
        let fixture = fixture();
        let provider =
            fixture.provider(if mode == "failed" { "adaptive-failure" } else { "adaptive" }).await;
        let cancel = Arc::new(Cancellation::new());
        let task = tokio::spawn(generate_controlled(
            fixture.context.clone(),
            fixture.origin.clone(),
            Arc::clone(&provider),
            Arc::clone(&cancel),
        ));
        fixture.until(|_| fixture.context.home.join("pending").exists()).await;

        match mode {
            "cancel" => cancel.request(),
            "drop" => task.abort(),
            "failed" => fs::write(fixture.context.home.join("finish"), "").unwrap(),

            "deadline" => {
                let requests =
                    fs::read_to_string(fixture.context.home.join("requests.jsonl")).unwrap();
                let request: serde_json::Value = serde_json::from_str(requests.trim()).unwrap();
                let total =
                    request["params"]["generation"]["limits"]["total_millis"].as_u64().unwrap();
                super::advance(total.div_ceil(1_000) + 1).await;
            }

            _ => unreachable!(),
        }

        let result = tokio::time::timeout(Duration::from_secs(8), task).await.unwrap();

        if mode == "drop" {
            assert!(result.unwrap_err().is_cancelled());
        } else {
            assert!(result.unwrap().is_err());
        }

        let samples = fixture.context.plugins.inference.samples();

        assert_eq!(samples.len(), 1, "{mode}");
        assert_eq!(samples[0].outcome, expected, "{mode}");
        {
            let store = fixture.context.sessions.lock().unwrap();
            let budget = store.tasks.values().next().unwrap().budget.as_ref().unwrap();

            assert!(budget.held_millis > 0, "{mode}");
            assert_eq!(budget.spent_millis, 0, "{mode}");
            assert!(budget.pending.is_none(), "{mode}");
        }

        assert!(
            samples[0].input.is_none()
                && samples[0].output.is_none()
                && samples[0].timing.is_none()
        );

        assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "1");
        provider.stop().await.unwrap();
    }
}

#[tokio::test]
async fn rejected_inference_has_no_execution_observation_or_generation_dispatch() {
    let mut fixture = fixture();
    Arc::make_mut(&mut fixture.context.plugins.config).inference.max_seconds = 1;
    let provider = fixture.provider("adaptive").await;

    assert!(
        generate(fixture.context.clone(), fixture.origin.clone(), Arc::clone(&provider))
            .await
            .is_err()
    );

    assert!(fixture.context.plugins.inference.samples().is_empty());
    assert!(!fixture.context.home.join("generated").exists());
    provider.stop().await.unwrap();
}

#[tokio::test]
async fn progressing_generations_renew_within_their_ceiling_and_silent_ones_expire() {
    for mode in ["adaptive-renew", "adaptive-stall"] {
        let fixture = fixture();
        let provider = fixture.provider(mode).await;
        let answering = fixture.context.clone();
        let origin = fixture.origin.clone();
        let model = Arc::clone(&provider);
        let (notices, _receiver) = tokio::sync::mpsc::channel(32);
        let history = answering.sessions.lock().unwrap().sessions[&origin].context();

        let task = tokio::spawn(async move {
            answer(
                &model,
                &answering.plugins,
                "model",
                history,
                &origin,
                "desktop",
                &origin,
                None,
                &answering.sessions,
                Some(&answering.home),
                &answering.home,
                false,
                false,
                false,
                ApprovalMode::Auto,
                Arc::clone(&answering.approvals),
                &Cancellation::new(),
                &answering.stop,
                tokio::time::Instant::now() + answering.config.inference.envelope(),
                &mut 1,
                &mut None,
                None,
                notices,
                None,
                None,
                None,
            )
            .await
        });

        fixture.until(|_| fixture.context.home.join("pending").exists()).await;

        let requests = fs::read_to_string(fixture.context.home.join("requests.jsonl")).unwrap();
        let request: serde_json::Value = serde_json::from_str(requests.trim()).unwrap();
        let limits = &request["params"]["generation"]["limits"];
        let total = limits["total_millis"].as_u64().unwrap();
        let ceiling = limits["ceiling_millis"].as_u64().unwrap();

        assert!(ceiling > total);

        super::advance(total * 85 / 100_000 + 1).await;

        let renewed = fixture.context.home.join("renewed");

        if mode == "adaptive-renew" {
            let parse = || {
                fs::read_to_string(&renewed)
                    .ok()
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(text.trim()).ok())
            };

            fixture.until(|_| parse().is_some()).await;

            let note = parse().unwrap();
            let extended = (total + total / 2).min(ceiling);

            assert_eq!(note["params"]["total_millis"], extended);

            {
                let path = fixture.context.home.join("data/sessions/index.json");
                let saved = state::Store::load(path).unwrap();
                let budget = saved.tasks.values().next().unwrap().budget.clone().unwrap();
                let exposure =
                    budget.pending.map_or(0, |pending| pending.millis) + budget.held_millis;

                assert_eq!(exposure, extended, "{budget:?}");
            }

            super::advance(total / 2_000).await;
            tokio::time::sleep(Duration::from_millis(100)).await;

            assert!(!task.is_finished());

            fs::write(fixture.context.home.join("finish"), "").unwrap();

            let answer =
                tokio::time::timeout(Duration::from_secs(8), task).await.unwrap().unwrap().unwrap();

            assert_eq!(answer.model.text, "Working, verified.");

            let store = fixture.context.sessions.lock().unwrap();
            let budget = store.tasks.values().next().unwrap().budget.as_ref().unwrap();

            assert!(budget.spent_millis > total);
            assert!(budget.pending.is_none());
            assert_eq!(budget.sequence, 1);
        } else {
            super::advance(total / 1_000).await;

            let failed = tokio::time::timeout(Duration::from_secs(8), task).await.unwrap().unwrap();

            assert!(failed.is_err());
            assert!(!renewed.exists());
        }

        provider.stop().await.unwrap();
    }
}

fn admit_with(fixture: &Fixture, extras: &[(&str, Role)]) -> String {
    let mut store = fixture.context.sessions.lock().unwrap();
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

    let mut history = store.sessions[&fixture.origin].context();

    history.extend(extras.iter().map(|(id, role)| Message {
        id: (*id).into(),
        session: fixture.origin.clone(),
        role: role.clone(),
        sender: None,
        content: vec![Content::Text { text: format!("{id} prompt") }],
    }));

    let guard = Foreground::admit(
        &mut store,
        Arc::clone(&fixture.context.plugins.tasks),
        &context,
        history,
        &Cancellation::new(),
    )
    .unwrap();

    guard.run.id.clone()
}

fn later(fixture: &Fixture) {
    fixture
        .context
        .sessions
        .lock()
        .unwrap()
        .push(
            &fixture.origin,
            Message {
                id: "later-question".into(),
                session: fixture.origin.clone(),
                role: Role::User,
                sender: Some("desktop".into()),
                content: vec![Content::Text { text: "An unrelated later question.".into() }],
            },
        )
        .unwrap();
}

#[test]
fn a_foreground_journal_references_its_conversation_instead_of_copying_it() {
    let fixture = fixture();
    let path = fixture.context.home.join("data/sessions/index.json");
    let id =
        admit_with(&fixture, &[("memory", Role::System), ("krabb-context-KRAB.md", Role::System)]);

    later(&fixture);

    let recovered = state::Store::load(&path).unwrap();
    let journal = &recovered.sessions[&id];
    let resolved = recovered.resolved_context(&id).unwrap();
    let ids = resolved.iter().map(|message| message.id.as_str()).collect::<Vec<_>>();

    assert_eq!(journal.messages.iter().map(|message| &message.id).collect::<Vec<_>>(), ["memory"]);
    assert_eq!(ids.first(), Some(&"memory"));
    assert!(ids.contains(&"foreground-user"));
    assert!(!ids.contains(&"later-question"));
    assert!(!ids.contains(&"krabb-context-KRAB.md"));
    assert!(resolved.iter().all(|message| message.session == id));
}

#[test]
fn saved_journal_copies_migrate_to_conversation_references() {
    let fixture = fixture();
    let path = fixture.context.home.join("data/sessions/index.json");
    let id = admit_with(&fixture, &[("memory", Role::System)]);

    let expected = {
        let mut store = fixture.context.sessions.lock().unwrap();
        let mut copy = store.resolved_context(&id).unwrap();

        copy.insert(
            0,
            Message {
                id: "krabb-context-KRAB.md".into(),
                session: id.clone(),
                role: Role::System,
                sender: None,
                content: vec![Content::Text { text: "Identity.".into() }],
            },
        );

        let journal = store.sessions.get_mut(&id).unwrap();
        journal.messages = copy.clone();
        journal.reference = None;
        store.save().unwrap();
        copy.remove(0);
        copy
    };

    later(&fixture);

    let mut index: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    index["version"] = json!(14);
    fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();

    let recovered = state::Store::load(&path).unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    assert_eq!(saved["version"], 16);
    assert!(recovered.sessions[&id].reference.is_some());
    assert_eq!(
        recovered.sessions[&id].messages.iter().map(|message| &message.id).collect::<Vec<_>>(),
        ["memory"]
    );

    assert_eq!(recovered.resolved_context(&id).unwrap(), expected);
}
