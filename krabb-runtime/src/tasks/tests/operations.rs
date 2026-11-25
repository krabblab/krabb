use super::*;

use krabb_core::{
    operation::{Receipt, Spec, Start, State},
    task::{Action, Command, Run},
};

fn fixture() -> Fixture {
    let mut fixture = Fixture::new(Principal::Client("desktop".into()));

    fixture.context.config.clients.get_mut("desktop").unwrap().tools = true;
    fixture.context.plugins.config = Arc::new(fixture.context.config.clone());

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.tasks.get_mut(&fixture.id).unwrap().tools = true;
        store.save().unwrap();
    }

    fixture
}

fn spec(cancel: bool) -> Spec {
    Spec {
        name: "delegate".into(),
        description: "Produce the external report.".into(),
        input: json!({"type":"object","properties":{},"additionalProperties":false}),
        cancel,
    }
}

fn receipt(state: State) -> Receipt {
    Receipt {
        state,
        handle: Some("opaque-handle".into()),
        result: (state == State::Completed)
            .then(|| ToolResult::text("Verified external artifact.")),
        detail: (state == State::Waiting).then(|| "Which source should I use?".into()),
    }
}

fn prepare(fixture: &Fixture, cancel: bool) -> (Run, String) {
    let mut store = fixture.context.sessions.lock().unwrap();

    store.ensure_task(&fixture.id, "model").unwrap();

    let task = store.claim_task(&fixture.id, unix_seconds()).unwrap();
    let run = Run { id: task.id, generation: task.step };
    let key = format!("{}-operation", run.id);

    store
        .prepare_operation(
            &run,
            "runner",
            spec(cancel),
            Start { id: key.clone(), name: "delegate".into(), arguments: json!({}) },
        )
        .unwrap();

    (run, key)
}

fn control(fixture: &Fixture, action: Action) -> TaskStatus {
    let mut store = fixture.context.sessions.lock().unwrap();
    let revision = store.tasks[&fixture.id].revision;

    store
        .control_task(
            &fixture.id,
            &Command {
                id: format!("control-{revision}"),
                action,
                revision,
                instruction: String::new(),
            },
        )
        .unwrap()
        .state
}

fn due(fixture: &Fixture) -> Task {
    let mut store = fixture.context.sessions.lock().unwrap();

    store.tasks.get_mut(&fixture.id).unwrap().next = unix_seconds();
    store.claim_task(&fixture.id, unix_seconds()).unwrap()
}

async fn backend(fixture: &Fixture, cancel: bool) -> Arc<Live> {
    backend_as(fixture, cancel, "runner", "runner").await
}

async fn backend_as(fixture: &Fixture, cancel: bool, id: &str, capability: &str) -> Arc<Live> {
    let script = r#"
import json, pathlib, sys
root, declaration = pathlib.Path(sys.argv[1]), json.loads(sys.argv[2])
identity, capability = sys.argv[3], sys.argv[4]
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request['method']
    result = {}
    if method == 'hello':
        result = {'protocol':{'major':0,'minor':1},'id':identity,
                  'version':'0.1.0','capabilities':[capability]}
    elif method == 'operation.list':
        with (root / 'effects.jsonl').open('a') as output:
            output.write(json.dumps(request) + '\n')
        result = json.loads((root / 'declarations').read_text()) if (root / 'declarations').exists() else {'operations':[declaration]}
    elif method == 'operation.start' and (root / 'refuse').exists():
        print(json.dumps({'jsonrpc':'2.0','id':request['id'],'error':{
            'code':-32602,'message':'The delegate arguments are invalid.'
                + (root / 'refuse').read_text()}}), flush=True)
        continue
    elif method in ['operation.start','operation.inspect','operation.cancel']:
        with (root / 'effects.jsonl').open('a') as output:
            output.write(json.dumps(request) + '\n')
        if method == 'operation.cancel' and (root / 'crash-cancel').exists():
            (root / 'crash-cancel').unlink()
            sys.exit(1)
        if method == 'operation.cancel' and (root / 'refuse-cancel').exists():
            (root / 'refuse-cancel').unlink()
            print(json.dumps({'jsonrpc':'2.0','id':request['id'],'error':{
                'code':-32000,'message':'The backend is temporarily unavailable.'}}), flush=True)
            continue
        state = (root / 'remote').read_text() if (root / 'remote').exists() else 'running'
        result = {'state':state,'handle':'opaque-handle','result':None,'detail':None}
        if state == 'completed':
            result['result'] = {'content':[{'kind':'text','text':'Verified external artifact.'}]}
        elif state == 'waiting':
            result['detail'] = 'Which source should I use?'
        elif state == 'malformed':
            result = {}
        elif state == 'changed':
            result.update(state='running', handle='different-handle')
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
    if method == 'shutdown':
        break
"#;

    let declaration = serde_json::to_string(&spec(cancel)).unwrap();

    let process = Process::start_with(
        "python3",
        ["-u", "-c", script, fixture.context.home.to_str().unwrap(), &declaration, id, capability],
    )
    .await
    .unwrap();

    fixture.context.plugins.insert(Live::new(process)).await;
    fixture.context.plugins.get(id).await.unwrap()
}

fn effects(fixture: &Fixture, method: &str) -> usize {
    fs::read_to_string(fixture.context.home.join("effects.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|request| request["method"] == method)
        .count()
}

#[test]
fn a_lost_start_receipt_blocks_replay_and_accepts_a_late_fact_without_reviving_cancelled_work() {
    let fixture = fixture();

    let (run, key) = prepare(&fixture, true);
    let path = fixture.context.home.join("data/sessions/index.json");
    let mut recovered = state::Store::load(path).unwrap();

    assert_eq!(recovered.tasks[&run.id].status, TaskStatus::Blocked);
    assert!(recovered.tasks[&run.id].uncertain);
    assert!(recovered.claim_task(&run.id, u64::MAX).is_err());
    assert!(recovered.check_task_step(&run.id, run.generation).is_err());

    let revision = recovered.tasks[&run.id].revision;

    recovered
        .control_task(
            &run.id,
            &Command {
                id: "cancel-after-restart".into(),
                action: Action::Cancel,
                revision,
                instruction: String::new(),
            },
        )
        .unwrap();

    recovered.accept_operation(&run.id, &key, receipt(State::Running), None, 1).unwrap();

    assert_eq!(recovered.tasks[&run.id].status, TaskStatus::Cancelled);
    assert!(recovered.tasks[&run.id].observing());
    assert!(!recovered.tasks[&run.id].uncertain);

    recovered.accept_operation(&run.id, &key, receipt(State::Completed), None, 2).unwrap();
    recovered.collect_operation(&run.id, &key, 2).unwrap();
    recovered.collect_operation(&run.id, &key, 3).unwrap();
    recovered.accept_operation(&run.id, &key, receipt(State::Running), None, 4).unwrap();

    let operation = recovered.tasks[&run.id].operation.as_ref().unwrap();

    assert_eq!(recovered.tasks[&run.id].status, TaskStatus::Cancelled);
    assert_eq!(operation.phase, state::OperationPhase::Collected);
    assert_eq!(operation.receipt.as_ref().unwrap().state, State::Completed);
}

#[test]
fn catalog_migration_and_failed_intent_writes_preserve_existing_tasks() {
    let fixture = fixture();
    let path = fixture.context.home.join("data/sessions/index.json");
    let mut index: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    index["version"] = json!(6);
    index["jobs"][&fixture.id].as_object_mut().unwrap().remove("operation");
    fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();

    let mut store = state::Store::load(&path).unwrap();

    assert!(store.tasks[&fixture.id].operation.is_none());
    assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Queued);

    let task = store.claim_task(&fixture.id, unix_seconds()).unwrap();
    let run = Run { id: task.id, generation: task.step };
    let directory = fixture.context.home.join("data/sessions");
    let backup = fixture.context.home.join("data/sessions-backup");

    fs::rename(&directory, &backup).unwrap();
    fs::write(&directory, "blocked").unwrap();

    assert!(
        store
            .prepare_operation(
                &run,
                "runner",
                spec(true),
                Start {
                    id: "failed-admission".into(),
                    name: "delegate".into(),
                    arguments: json!({}),
                }
            )
            .is_err()
    );

    assert!(store.tasks[&run.id].operation.is_none());
    assert_eq!(store.tasks[&run.id].status, TaskStatus::Running);

    fs::remove_file(&directory).unwrap();
    fs::rename(&backup, &directory).unwrap();

    assert_eq!(state::Store::load(path).unwrap().tasks[&run.id].status, TaskStatus::Queued);
}

#[test]
fn terminal_receipts_win_over_late_running_responses_and_collection_is_atomic() {
    let fixture = fixture();
    let (run, key) = prepare(&fixture, true);
    let mut store = fixture.context.sessions.lock().unwrap();

    store.ensure_task(&run.id, "model").unwrap();
    store.accept_operation(&run.id, &key, receipt(State::Completed), None, 1).unwrap();
    store.accept_operation(&run.id, &key, receipt(State::Running), None, 2).unwrap();

    let directory = fixture.context.home.join("data/sessions");
    let backup = fixture.context.home.join("data/sessions-backup");

    fs::rename(&directory, &backup).unwrap();
    fs::write(&directory, "blocked").unwrap();

    assert!(store.collect_operation(&run.id, &key, 3).is_err());

    assert_eq!(
        store.tasks[&run.id].operation.as_ref().unwrap().phase,
        state::OperationPhase::Observing
    );

    assert!(
        !store.sessions[&run.id]
            .messages
            .iter()
            .any(|message| message.sender.as_deref() == Some("operation"))
    );

    fs::remove_file(&directory).unwrap();
    fs::rename(&backup, &directory).unwrap();
    store.collect_operation(&run.id, &key, 3).unwrap();
    store.collect_operation(&run.id, &key, 4).unwrap();

    assert_eq!(
        store.sessions[&run.id]
            .messages
            .iter()
            .filter(|message| message.sender.as_deref() == Some("operation"))
            .count(),
        1
    );

    assert_eq!(store.tasks[&run.id].status, TaskStatus::Queued);
}

#[tokio::test]
async fn an_invalid_start_receipt_never_causes_an_automatic_second_start() {
    let fixture = fixture();

    fs::write(fixture.context.home.join("remote"), "malformed").unwrap();

    let adapter = backend(&fixture, true).await;
    let provider = fixture.provider("operation").await;
    let worker = fixture.start();

    fixture.until(|store| store.tasks[&fixture.id].status == TaskStatus::Blocked).await;
    fixture.stop(worker).await;

    assert_eq!(effects(&fixture, "operation.start"), 1);
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "1");

    let path = fixture.context.home.join("data/sessions/index.json");
    let recovered = state::Store::load(path).unwrap();

    assert!(recovered.tasks[&fixture.id].uncertain);
    assert!(recovered.tasks[&fixture.id].operation.as_ref().unwrap().receipt.is_none());

    provider.stop().await.unwrap();
    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn an_explicitly_refused_start_lets_the_task_continue_without_review() {
    let fixture = fixture();

    fs::write(fixture.context.home.join("refuse"), " detail".repeat(10_000)).unwrap();

    let adapter = backend(&fixture, true).await;
    let provider = fixture.provider("operation").await;
    let worker = fixture.start();

    fixture.until(|store| store.tasks[&fixture.id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;

    assert_eq!(effects(&fixture, "operation.start"), 0);
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");

    let requests = fs::read_to_string(fixture.context.home.join("requests.jsonl")).unwrap();
    let refusal = requests.lines().last().unwrap();

    assert!(refusal.contains("The runner refused the operation"));
    assert!(refusal.len() < 32 * 1024, "{}", refusal.len());

    {
        let store = fixture.context.sessions.lock().unwrap();
        let task = &store.tasks[&fixture.id];

        assert!(!task.uncertain);
        assert!(task.operation.is_none());
    }

    provider.stop().await.unwrap();

    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_refused_cancellation_is_sent_again() {
    let fixture = fixture();
    let adapter = backend(&fixture, true).await;
    let (run, key) = prepare(&fixture, true);

    fixture
        .context
        .sessions
        .lock()
        .unwrap()
        .accept_operation(&run.id, &key, receipt(State::Running), None, unix_seconds())
        .unwrap();

    assert_eq!(control(&fixture, Action::Cancel), TaskStatus::Cancelling);

    fs::write(fixture.context.home.join("refuse-cancel"), "").unwrap();

    for response in ["running", "cancelled"] {
        fs::write(fixture.context.home.join("remote"), response).unwrap();

        let task = due(&fixture);

        crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();
    }

    assert_eq!(effects(&fixture, "operation.cancel"), 2);

    assert_eq!(
        fixture.context.sessions.lock().unwrap().tasks[&run.id].status,
        TaskStatus::Cancelled
    );

    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_cancellation_that_got_no_response_is_sent_again() {
    let fixture = fixture();
    let adapter = backend(&fixture, true).await;
    let (run, key) = prepare(&fixture, true);

    fixture
        .context
        .sessions
        .lock()
        .unwrap()
        .accept_operation(&run.id, &key, receipt(State::Running), None, unix_seconds())
        .unwrap();

    assert_eq!(control(&fixture, Action::Cancel), TaskStatus::Cancelling);

    fs::write(fixture.context.home.join("crash-cancel"), "").unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    assert!(
        !fixture.context.sessions.lock().unwrap().tasks[&run.id]
            .operation
            .as_ref()
            .unwrap()
            .cancel_sent
    );

    let restarted = backend(&fixture, true).await;

    fs::write(fixture.context.home.join("remote"), "cancelled").unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    assert_eq!(effects(&fixture, "operation.cancel"), 2);
    assert_eq!(effects(&fixture, "operation.inspect"), 0);

    assert_eq!(
        fixture.context.sessions.lock().unwrap().tasks[&run.id].status,
        TaskStatus::Cancelled
    );

    let _ = adapter.stop().await;

    restarted.stop().await.unwrap();
}

#[tokio::test]
async fn operations_outlive_ten_minutes_and_restart_without_inference_or_a_second_dispatch() {
    let mut fixture = fixture();
    let adapter = backend(&fixture, true).await;
    let provider = fixture.provider("operation").await;
    let worker = fixture.start();

    fixture
        .until(|store| {
            store.tasks[&fixture.id]
                .operation
                .as_ref()
                .is_some_and(|operation| operation.phase == state::OperationPhase::Observing)
        })
        .await;

    fixture.stop(worker).await;
    fixture.context.stop = Arc::new(Stop::new());

    assert_eq!(effects(&fixture, "operation.start"), 1);
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "1");

    let start = unix_seconds();

    for offset in [601, 3_600, 86_400] {
        let task = {
            let mut store = fixture.context.sessions.lock().unwrap();
            let due = store.tasks[&fixture.id].next;
            let task =
                store.claim_task(&fixture.id, start.saturating_add(offset).max(due)).unwrap();

            assert!(store.check_task_step(&task.id, task.step).is_err());
            task
        };

        crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();
    }

    assert_eq!(effects(&fixture, "operation.inspect"), 3);
    assert_eq!(effects(&fixture, "operation.start"), 1);
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "1");

    let path = fixture.context.home.join("data/sessions/index.json");

    fixture.context.sessions = Arc::new(Mutex::new(state::Store::load(path).unwrap()));
    fixture.context.stop = Arc::new(Stop::new());
    fs::write(fixture.context.home.join("remote"), "completed").unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    let worker = fixture.start();

    fixture.until(|store| store.tasks[&fixture.id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;

    assert_eq!(effects(&fixture, "operation.start"), 1);
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");

    let requests = fs::read_to_string(fixture.context.home.join("requests.jsonl")).unwrap();

    assert!(requests.lines().last().unwrap().contains("Verified external artifact."));

    assert!(
        fixture.context.sessions.lock().unwrap().sessions[&fixture.origin]
            .messages
            .iter()
            .any(|message| message.content[0].render().contains("Verified complete report."))
    );

    provider.stop().await.unwrap();
    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn pause_preserves_external_result_and_cancel_waits_for_actual_backend_settlement() {
    for action in [Action::Pause, Action::Cancel] {
        for cancellable in [false, true] {
            let fixture = fixture();
            let adapter = backend(&fixture, cancellable).await;
            let (run, key) = prepare(&fixture, cancellable);

            fixture
                .context
                .sessions
                .lock()
                .unwrap()
                .accept_operation(&run.id, &key, receipt(State::Running), None, unix_seconds())
                .unwrap();

            let requested = control(&fixture, action);

            let expected =
                if action == Action::Pause { TaskStatus::Pausing } else { TaskStatus::Cancelling };

            assert_eq!(requested, expected);

            for _ in 0..2 {
                let task = due(&fixture);

                crate::operations::monitor(&fixture.context, &task, &Cancellation::new())
                    .await
                    .unwrap();

                assert_eq!(
                    fixture.context.sessions.lock().unwrap().tasks[&run.id].status,
                    expected
                );
            }

            let cancellations = usize::from(action == Action::Cancel && cancellable);

            assert_eq!(effects(&fixture, "operation.cancel"), cancellations);

            fs::write(fixture.context.home.join("remote"), "completed").unwrap();

            let task = due(&fixture);

            crate::operations::monitor(&fixture.context, &task, &Cancellation::new())
                .await
                .unwrap();

            let expected =
                if action == Action::Pause { TaskStatus::Paused } else { TaskStatus::Cancelled };

            {
                let store = fixture.context.sessions.lock().unwrap();

                assert_eq!(store.tasks[&run.id].status, expected);
                assert_eq!(
                    store.tasks[&run.id]
                        .operation
                        .as_ref()
                        .unwrap()
                        .receipt
                        .as_ref()
                        .unwrap()
                        .result,
                    Some(ToolResult::text("Verified external artifact."))
                );

                assert!(store.check_task_step(&run.id, run.generation).is_err());
            }

            if action == Action::Pause {
                assert_eq!(control(&fixture, Action::Resume), TaskStatus::Queued);
            }

            assert_eq!(effects(&fixture, "operation.start"), 0);

            adapter.stop().await.unwrap();
        }
    }
}

#[tokio::test]
async fn an_overdue_resume_waits_for_external_settlement_and_keeps_its_verified_result() {
    let fixture = fixture();
    let adapter = backend(&fixture, false).await;
    let (run, key) = prepare(&fixture, false);
    let now = unix_seconds();

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.accept_operation(&run.id, &key, receipt(State::Running), None, now).unwrap();
        let command = krabb_core::task::defer::Command {
            id: "resume-after-settlement".into(),
            revision: store.tasks[&run.id].revision,
            when: krabb_core::task::defer::When {
                at: now + 100,
                timezone: "UTC".into(),
                expression: None,
            },
        };

        assert_eq!(store.defer_task(&run.id, &command, now).unwrap().state, TaskStatus::Pausing);
        assert_eq!(store.release_deferred(now + 101).unwrap(), 0);
    }

    let path = fixture.context.home.join("data/sessions/index.json");

    *fixture.context.sessions.lock().unwrap() = state::Store::load(path).unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    {
        let mut store = fixture.context.sessions.lock().unwrap();

        assert_eq!(store.tasks[&run.id].status, TaskStatus::Pausing);
        assert_eq!(store.release_deferred(now + 101).unwrap(), 0);
    }

    fs::write(fixture.context.home.join("remote"), "completed").unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    {
        let mut store = fixture.context.sessions.lock().unwrap();

        assert_eq!(store.tasks[&run.id].status, TaskStatus::Paused);
        assert!(store.tasks[&run.id].notice.is_some());

        assert_eq!(
            store.tasks[&run.id].operation.as_ref().unwrap().receipt.as_ref().unwrap().result,
            Some(ToolResult::text("Verified external artifact."))
        );

        assert_eq!(store.release_deferred(now + 101).unwrap(), 1);
        assert_eq!(store.tasks[&run.id].status, TaskStatus::Queued);
        assert!(store.tasks[&run.id].deferred.is_none());
        assert!(store.tasks[&run.id].notice.is_none());
        assert!(store.check_task_step(&run.id, run.generation).is_err());
    }

    assert_eq!(effects(&fixture, "operation.start"), 0);

    assert_eq!(effects(&fixture, "operation.cancel"), 0);

    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_duplicate_input_fence_preserves_external_reconciliation_without_resuming_inference() {
    let mut fixture = fixture();
    let adapter = backend(&fixture, false).await;
    let (run, key) = prepare(&fixture, false);
    let other = "task-999999999999";

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store
            .accept_operation(&run.id, &key, receipt(State::Running), None, unix_seconds())
            .unwrap();
        let task = store.tasks.get_mut(&run.id).unwrap();
        task.request = Some("shared-input".into());
        task.input = Some(state::Input::unknown());
        let mut duplicate = task.clone();
        duplicate.id = other.into();
        duplicate.operation = None;
        duplicate.status = TaskStatus::Queued;
        store.tasks.insert(other.into(), duplicate);
        store.ensure_task(other, "model").unwrap();
        store.save().unwrap();
    }

    fixture.context.sessions = Arc::new(Mutex::new(
        state::Store::load(fixture.context.home.join("data/sessions/index.json")).unwrap(),
    ));

    assert_eq!(fixture.context.sessions.lock().unwrap().tasks[&run.id].status, TaskStatus::Blocked);

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();
    std::fs::write(fixture.context.home.join("remote"), "completed").unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    {
        let mut store = fixture.context.sessions.lock().unwrap();

        assert!(store.claim_task(&run.id, unix_seconds()).is_err());
        assert_eq!(store.tasks[&run.id].status, TaskStatus::Blocked);
        assert_eq!(
            store.tasks[&run.id].operation.as_ref().unwrap().receipt.as_ref().unwrap().result,
            Some(ToolResult::text("Verified external artifact."))
        );
    }

    assert!(effects(&fixture, "operation.inspect") >= 2);

    assert_eq!(effects(&fixture, "operation.start"), 0);
    assert_eq!(effects(&fixture, "operation.cancel"), 0);
    assert!(!fixture.context.home.join("generated").exists());

    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn input_required_and_adapter_outages_do_not_lose_handles_or_create_poll_storms() {
    let fixture = fixture();
    let (run, key) = prepare(&fixture, false);

    fixture
        .context
        .sessions
        .lock()
        .unwrap()
        .accept_operation(&run.id, &key, receipt(State::Running), None, unix_seconds())
        .unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    {
        let store = fixture.context.sessions.lock().unwrap();

        assert!(store.tasks[&run.id].next > unix_seconds());
        assert_eq!(store.tasks[&run.id].operation.as_ref().unwrap().failures, 1);
    }

    let adapter = backend(&fixture, false).await;

    for response in ["malformed", "changed", "waiting", "waiting", "completed"] {
        fs::write(fixture.context.home.join("remote"), response).unwrap();

        let task = due(&fixture);

        crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

        let store = fixture.context.sessions.lock().unwrap();
        let operation = store.tasks[&run.id].operation.as_ref().unwrap();

        assert_eq!(operation.receipt.as_ref().unwrap().handle.as_deref(), Some("opaque-handle"));

        if response == "waiting" {
            assert_eq!(store.tasks[&run.id].status, TaskStatus::Blocked);
            assert!(store.tasks[&run.id].next > unix_seconds());
        }
    }

    assert_eq!(fixture.context.sessions.lock().unwrap().tasks[&run.id].status, TaskStatus::Queued);
    assert_eq!(effects(&fixture, "operation.start"), 0);

    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_lost_cancel_receipt_is_reconciled_without_replaying_the_cancellation() {
    let mut fixture = fixture();
    let adapter = backend(&fixture, true).await;
    let (run, key) = prepare(&fixture, true);

    fixture
        .context
        .sessions
        .lock()
        .unwrap()
        .accept_operation(&run.id, &key, receipt(State::Running), None, unix_seconds())
        .unwrap();

    assert_eq!(control(&fixture, Action::Cancel), TaskStatus::Cancelling);

    fs::write(fixture.context.home.join("remote"), "malformed").unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    let path = fixture.context.home.join("data/sessions/index.json");

    fixture.context.sessions = Arc::new(Mutex::new(state::Store::load(path).unwrap()));

    for response in ["running", "cancelled"] {
        fs::write(fixture.context.home.join("remote"), response).unwrap();

        let task = due(&fixture);

        crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();
    }

    assert_eq!(effects(&fixture, "operation.cancel"), 1);
    assert_eq!(effects(&fixture, "operation.inspect"), 2);

    assert_eq!(
        fixture.context.sessions.lock().unwrap().tasks[&run.id].status,
        TaskStatus::Cancelled
    );

    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn missing_operation_context_blocks_resume_without_generating_the_original_goal_again() {
    let fixture = fixture();
    let (run, key) = prepare(&fixture, true);

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.sessions.remove(&run.id);
        store.accept_operation(&run.id, &key, receipt(State::Completed), None, 1).unwrap();
        store.collect_operation(&run.id, &key, 1).unwrap();

        assert_eq!(store.tasks[&run.id].status, TaskStatus::Blocked);
        assert_eq!(
            store.tasks[&run.id].operation.as_ref().unwrap().receipt.as_ref().unwrap().result,
            Some(ToolResult::text("Verified external artifact."))
        );
    }

    let provider = fixture.provider("foreground-success").await;

    assert_eq!(control(&fixture, Action::Resume), TaskStatus::Queued);

    let worker = fixture.start();

    fixture.until(|store| store.tasks[&run.id].status == TaskStatus::Blocked).await;
    fixture.stop(worker).await;

    assert!(!fixture.context.home.join("generated").exists());

    provider.stop().await.unwrap();
}

#[test]
fn retention_protects_cancelled_operation_journals_and_uncertain_effects() {
    for uncertain in [false, true] {
        let fixture = fixture();
        let (run, key) = prepare(&fixture, true);
        let mut store = fixture.context.sessions.lock().unwrap();

        if uncertain {
            store.abandon_operation(&run.id, &key).unwrap();
        } else {
            store.accept_operation(&run.id, &key, receipt(State::Running), None, 1).unwrap();
        }

        store.tasks.get_mut(&run.id).unwrap().status = TaskStatus::Cancelled;
        store.tasks.get_mut(&run.id).unwrap().notice = None;
        store.sessions.get_mut(&run.id).unwrap().updated = 0;

        let template = store.sessions[&run.id].clone();

        for index in 0..999 {
            let mut journal = template.clone();

            journal.id = format!("schedule-{index}");
            journal.kind = state::SessionKind::Schedule;
            journal.updated = 1;
            store.sessions.insert(journal.id.clone(), journal);
        }

        store.ensure_schedule("schedule-1000", "model").unwrap();

        assert!(store.sessions.contains_key(&run.id));
        assert!(!store.sessions.contains_key("schedule-0"));
        assert!(store.tasks[&run.id].protected());
    }
}

#[tokio::test]
async fn foreground_operation_handoff_is_announced_once_and_delivers_the_collected_result() {
    let mut fixture = fixture();

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.tasks.clear();
        store
            .begin(
                &fixture.origin,
                Message {
                    id: "foreground-operation".into(),
                    session: fixture.origin.clone(),
                    role: Role::User,
                    sender: Some("desktop".into()),
                    content: vec![Content::Text { text: "Complete the external report.".into() }],
                },
            )
            .unwrap();
    }

    let adapter = backend(&fixture, true).await;

    let provider = fixture.provider("operation-foreground").await;

    let answer = super::foreground::generate(
        fixture.context.clone(),
        fixture.origin.clone(),
        Arc::clone(&provider),
    )
    .await
    .unwrap();

    fixture.id = answer.foreground.as_ref().unwrap().run.id.clone();

    {
        let mut store = fixture.context.sessions.lock().unwrap();

        assert_eq!(store.tasks[&fixture.id].status, TaskStatus::Waiting);
        assert!(store.tasks[&fixture.id].notice.is_none());
        store
            .push(
                &fixture.origin,
                Message {
                    id: "handoff-answer".into(),
                    session: fixture.origin.clone(),
                    role: Role::Assistant,
                    sender: None,
                    content: vec![Content::Text { text: answer.text.clone() }],
                },
            )
            .unwrap();
        store.clear(&fixture.origin, "idle").unwrap();
    }

    drop(answer);

    fs::write(fixture.context.home.join("remote"), "completed").unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    let worker = fixture.start();

    fixture.until(|store| store.tasks[&fixture.id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;

    {
        let store = fixture.context.sessions.lock().unwrap();

        assert_eq!(
            store.sessions[&fixture.origin]
                .messages
                .iter()
                .filter(|message| message.content[0].render()
                    == "I'll collect the external report when it is ready.")
                .count(),
            1
        );

        assert!(
            store.sessions[&fixture.origin]
                .messages
                .iter()
                .any(|message| message.content[0].render()
                    == "The external report is verified and complete.")
        );
    }

    assert_eq!(effects(&fixture, "operation.start"), 1);

    let requests = fs::read_to_string(fixture.context.home.join("requests.jsonl")).unwrap();

    assert!(requests.lines().last().unwrap().contains("Verified external artifact."));

    provider.stop().await.unwrap();
    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn extended_discovery_requires_runner_identity_and_negotiated_capability() {
    let fixture = fixture();

    assert!(crate::operations::discover(&fixture.context.plugins).await.unwrap().is_empty());

    let legacy = backend_as(&fixture, true, "backend", "agent").await;
    let impostor = backend_as(&fixture, true, "other", "runner").await;
    let unsupported = backend_as(&fixture, true, "runner", "agent").await;

    assert!(crate::operations::discover(&fixture.context.plugins).await.unwrap().is_empty());

    fixture.context.plugins.remove("runner").await;
    unsupported.stop().await.unwrap();

    let runner = backend(&fixture, true).await;
    let offers = crate::operations::discover(&fixture.context.plugins).await.unwrap();
    let tools = crate::operations::definitions(&offers);

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "operation_runner_delegate");
    assert_eq!(effects(&fixture, "operation.start"), 0);

    legacy.stop().await.unwrap();
    impostor.stop().await.unwrap();
    runner.stop().await.unwrap();
}

#[tokio::test]
async fn runner_replacement_invalidates_offers_before_any_external_start() {
    let fixture = fixture();
    let runner = backend(&fixture, true).await;
    let offers = crate::operations::discover(&fixture.context.plugins).await.unwrap();
    let replacement = backend(&fixture, false).await;

    let run = {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.ensure_task(&fixture.id, "model").unwrap();
        let task = store.claim_task(&fixture.id, unix_seconds()).unwrap();
        Run { id: task.id, generation: task.step }
    };

    let registry = ToolRegistry { run: Some(run), ..ToolRegistry::default() };

    let error = crate::operations::start(
        &offers["operation_runner_delegate"],
        &json!({"arguments":{}, "message":"I'll collect the report."}),
        42,
        &registry,
        &fixture.context.plugins,
        &fixture.context.sessions,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("changed after discovery"));
    assert_eq!(effects(&fixture, "operation.start"), 0);
    assert!(fixture.context.sessions.lock().unwrap().tasks[&fixture.id].operation.is_none());

    runner.stop().await.unwrap();
    replacement.stop().await.unwrap();
}

#[tokio::test]
async fn runner_offers_are_listed_once_per_runner_process() {
    let fixture = fixture();
    let adapter = backend(&fixture, true).await;

    for _ in 0..2 {
        assert_eq!(crate::operations::discover(&fixture.context.plugins).await.unwrap().len(), 1);
    }

    assert_eq!(effects(&fixture, "operation.list"), 1);

    let replacement = backend(&fixture, true).await;

    assert_eq!(crate::operations::discover(&fixture.context.plugins).await.unwrap().len(), 1);
    assert_eq!(effects(&fixture, "operation.list"), 2);

    let _ = adapter.stop().await;

    replacement.stop().await.unwrap();
}

#[tokio::test]
async fn runner_restart_invalidates_offers_before_any_external_start() {
    let fixture = fixture();
    let runner = backend(&fixture, true).await;
    let offers = crate::operations::discover(&fixture.context.plugins).await.unwrap();

    runner.restart().await.unwrap();

    let run = {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.ensure_task(&fixture.id, "model").unwrap();
        let task = store.claim_task(&fixture.id, unix_seconds()).unwrap();
        Run { id: task.id, generation: task.step }
    };

    let registry = ToolRegistry { run: Some(run), ..ToolRegistry::default() };

    let error = crate::operations::start(
        &offers["operation_runner_delegate"],
        &json!({"arguments":{}, "message":"I'll collect the report."}),
        42,
        &registry,
        &fixture.context.plugins,
        &fixture.context.sessions,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("changed after discovery"));
    assert_eq!(effects(&fixture, "operation.start"), 0);
    assert!(fixture.context.sessions.lock().unwrap().tasks[&fixture.id].operation.is_none());

    runner.stop().await.unwrap();
}

#[tokio::test]
async fn absent_runner_preserves_controls_and_legacy_handles_without_claiming_effective_stop() {
    let fixture = fixture();
    let (run, key) = prepare(&fixture, true);

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store
            .accept_operation(&run.id, &key, receipt(State::Running), None, unix_seconds())
            .unwrap();
        store.tasks.get_mut(&run.id).unwrap().operation.as_mut().unwrap().plugin = "backend".into();
        store.save().unwrap();
    }

    let interact = |id: &str, text: &str| {
        let input = Message {
            id: id.into(),
            session: fixture.origin.clone(),
            role: Role::User,
            sender: Some("desktop".into()),
            content: vec![Content::Text { text: text.into() }],
        };

        crate::tasks::dialog::message(
            &mut fixture.context.sessions.lock().unwrap(),
            &input,
            &fixture.context.plugins.tasks,
        )
        .unwrap()
        .unwrap()
    };

    assert!(interact("status-without-runner", "/task").contains(&run.id));

    assert!(
        interact("pause-without-runner", &format!("/task pause {}", run.id))
            .contains("stopping safely")
    );

    assert_eq!(fixture.context.sessions.lock().unwrap().tasks[&run.id].status, TaskStatus::Pausing);

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    assert_eq!(fixture.context.sessions.lock().unwrap().tasks[&run.id].status, TaskStatus::Pausing);

    assert!(
        interact("cancel-without-runner", &format!("/task cancel {}", run.id))
            .contains("stopping safely")
    );

    assert_eq!(
        fixture.context.sessions.lock().unwrap().tasks[&run.id].status,
        TaskStatus::Cancelling
    );

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    let legacy = backend_as(&fixture, true, "backend", "agent").await;

    assert!(crate::operations::discover(&fixture.context.plugins).await.unwrap().is_empty());

    fs::write(fixture.context.home.join("remote"), "cancelled").unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    assert_eq!(
        fixture.context.sessions.lock().unwrap().tasks[&run.id].status,
        TaskStatus::Cancelled
    );

    assert_eq!(effects(&fixture, "operation.start"), 0);
    assert_eq!(effects(&fixture, "operation.cancel"), 1);
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap_or_default(), "");

    legacy.stop().await.unwrap();
}

#[tokio::test]
async fn malformed_runner_declarations_disable_extended_tools_without_disabling_minimal_work() {
    let fixture = fixture();
    let runner = backend(&fixture, true).await;

    for declarations in [
        json!({"operations":[{}]}),
        json!({"operations":[spec(true), spec(true)]}),
        json!({"operations":"invalid"}),
    ] {
        fs::write(fixture.context.home.join("declarations"), declarations.to_string()).unwrap();

        assert!(crate::operations::discover(&fixture.context.plugins).await.unwrap().is_empty());
    }

    let provider = fixture.provider("success").await;

    let worker = fixture.start();

    fixture.until(|store| store.tasks[&fixture.id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;

    assert_eq!(effects(&fixture, "operation.start"), 0);
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");
    assert!(fixture.context.sessions.lock().unwrap().tasks[&fixture.id].operation.is_none());

    provider.stop().await.unwrap();
    runner.stop().await.unwrap();
}

#[tokio::test]
async fn an_operation_beyond_its_allowance_settles_once_and_blocks_for_a_decision() {
    for cancellable in [false, true] {
        let mut fixture = fixture();

        fixture.context.config.operations.max_seconds = Some(600);

        let adapter = backend(&fixture, cancellable).await;
        let (run, key) = prepare(&fixture, cancellable);
        let started = unix_seconds() - 601;

        fixture
            .context
            .sessions
            .lock()
            .unwrap()
            .accept_operation(&run.id, &key, receipt(State::Running), None, started)
            .unwrap();

        for _ in 0..2 {
            let task = due(&fixture);

            crate::operations::monitor(&fixture.context, &task, &Cancellation::new())
                .await
                .unwrap();

            let store = fixture.context.sessions.lock().unwrap();
            let task = &store.tasks[&run.id];

            assert!(task.operation.as_ref().unwrap().exceeded);
            assert_eq!(task.operation.as_ref().unwrap().accepted, Some(started));
            assert!(!task.status.terminal());
        }

        assert_eq!(effects(&fixture, "operation.cancel"), usize::from(cancellable));
        assert_eq!(effects(&fixture, "operation.inspect"), 2 - usize::from(cancellable));

        fs::write(fixture.context.home.join("remote"), "completed").unwrap();

        let task = due(&fixture);

        crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

        {
            let store = fixture.context.sessions.lock().unwrap();
            let task = &store.tasks[&run.id];

            assert_eq!(task.status, TaskStatus::Blocked);
            assert_eq!(
                task.operation.as_ref().unwrap().receipt.as_ref().unwrap().result,
                Some(ToolResult::text("Verified external artifact."))
            );
        }

        assert_eq!(control(&fixture, Action::Resume), TaskStatus::Queued);

        assert_eq!(effects(&fixture, "operation.start"), 0);
        assert_eq!(effects(&fixture, "operation.cancel"), usize::from(cancellable));

        adapter.stop().await.unwrap();
    }
}

#[tokio::test]
async fn an_updated_runner_reconciles_a_day_old_handle_without_another_start() {
    let fixture = fixture();
    let original = backend(&fixture, true).await;
    let (run, key) = prepare(&fixture, true);

    fixture
        .context
        .sessions
        .lock()
        .unwrap()
        .accept_operation(&run.id, &key, receipt(State::Running), None, unix_seconds() - 86_400)
        .unwrap();

    original.stop().await.unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    {
        let store = fixture.context.sessions.lock().unwrap();
        let operation = store.tasks[&run.id].operation.as_ref().unwrap();

        assert_eq!(operation.failures, 1);
        assert!(operation.active());
        assert!(!store.tasks[&run.id].status.terminal());
    }

    let updated = backend(&fixture, true).await;

    fs::write(fixture.context.home.join("remote"), "completed").unwrap();

    let task = due(&fixture);

    crate::operations::monitor(&fixture.context, &task, &Cancellation::new()).await.unwrap();

    {
        let store = fixture.context.sessions.lock().unwrap();
        let task = &store.tasks[&run.id];
        let operation = task.operation.as_ref().unwrap();

        assert_eq!(operation.phase, state::OperationPhase::Collected);
        assert_eq!(
            operation.receipt.as_ref().unwrap().result,
            Some(ToolResult::text("Verified external artifact."))
        );

        assert_eq!(task.status, TaskStatus::Queued);
    }

    assert_eq!(effects(&fixture, "operation.start"), 0);

    assert_eq!(effects(&fixture, "operation.inspect"), 1);

    updated.stop().await.unwrap();
}

#[test]
fn monitoring_polls_keep_the_task_revision_until_something_changes() {
    let fixture = fixture();
    let (run, key) = prepare(&fixture, true);

    let revision = {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.accept_operation(&run.id, &key, receipt(State::Running), None, 1).unwrap();
        store.tasks[&run.id].revision
    };

    let polled = due(&fixture);

    assert_eq!(polled.status, TaskStatus::Waiting);
    assert_eq!(polled.revision, revision);
    assert!(polled.step > run.generation);

    let mut store = fixture.context.sessions.lock().unwrap();

    store.accept_operation(&run.id, &key, receipt(State::Running), None, 2).unwrap();

    assert_eq!(store.tasks[&run.id].revision, revision);

    store.fail_operation_poll(&run.id, &key, 3).unwrap();

    let failed = store.tasks[&run.id].revision;

    store.fail_operation_poll(&run.id, &key, 4).unwrap();

    assert_eq!(failed, revision + 1);
    assert_eq!(store.tasks[&run.id].revision, failed);

    store
        .control_task(
            &run.id,
            &Command {
                id: "pause-after-polls".into(),
                action: Action::Pause,
                revision: failed,
                instruction: String::new(),
            },
        )
        .unwrap();

    let paused = store.tasks[&run.id].revision;

    store.accept_operation(&run.id, &key, receipt(State::Completed), None, 5).unwrap();

    assert!(store.tasks[&run.id].revision > paused);
}
