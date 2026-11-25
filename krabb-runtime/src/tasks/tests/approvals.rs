use super::*;
use crate::tasks::dialog as dialogue;
use krabb_core::task::{Action, Command, ControlError};

const TOOLS: &str = r#"
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request.get('method')
    result = {}
    if method == 'hello':
        result = {'protocol':{'major':0,'minor':1},'id':'files','version':'0.1.0',
                  'capabilities':['tool']}
    elif method == 'tools/list':
        result = {'tools':[{'name':'write','description':'Write a file.','schema':{
            'type':'object','properties':{'path':{'type':'string'},'text':{'type':'string'}}}}]}
    elif method == 'write':
        with (root / 'effects.jsonl').open('a') as output:
            output.write(json.dumps(request['params'], sort_keys=True) + '\n')
        result = {'text':'Written.'}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
    if method == 'shutdown':
        break
"#;

async fn prompted() -> Fixture {
    let mut fixture = super::foreground::fixture();

    let mut config = fixture.context.config.clone();

    config.approval = "prompt".into();
    config.clients.get_mut("desktop").unwrap().tools = true;
    fixture.context.config = config.clone();
    fixture.context.plugins.config = Arc::new(config);

    let tools =
        Process::start_with("python3", ["-u", "-c", TOOLS, fixture.context.home.to_str().unwrap()])
            .await
            .unwrap();

    fixture.context.plugins.insert(Live::new(tools)).await;
    fixture
}

type Running = (
    tokio::task::JoinHandle<Result<Answer, PluginError>>,
    tokio::sync::mpsc::Receiver<StreamNotice>,
);

async fn live(fixture: &Fixture, provider: Arc<Live>) -> Running {
    let context = fixture.context.clone();
    let origin = fixture.origin.clone();
    let (sender, receiver) = tokio::sync::mpsc::channel(32);

    let task = tokio::spawn(async move {
        let history = context.sessions.lock().unwrap().sessions[&origin].context();
        let mut call = 1;

        answer(
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
            true,
            false,
            false,
            ApprovalMode::Prompt,
            Arc::clone(&context.approvals),
            &Cancellation::new(),
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
        .await
    });

    (task, receiver)
}

async fn pending() -> (Fixture, Arc<Live>, String) {
    pending_in("approval").await
}

async fn pending_in(mode: &str) -> (Fixture, Arc<Live>, String) {
    let mut fixture = prompted().await;
    let provider = fixture.provider(mode).await;

    let task = tokio::spawn(super::foreground::generate(
        fixture.context.clone(),
        fixture.origin.clone(),
        Arc::clone(&provider),
    ));

    fixture.until(|_| fixture.context.home.join("generated").exists()).await;

    let id = {
        let mut store = fixture.context.sessions.lock().unwrap();
        let id = store.tasks.keys().next().unwrap().clone();

        control(
            &mut store,
            &json!({"action":"pause", "id":id}),
            &fixture.origin,
            None,
            &fixture.context.plugins.tasks,
            false,
        )
        .unwrap();
        id
    };

    tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap().unwrap_err();

    {
        let mut store = fixture.context.sessions.lock().unwrap();

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

    fixture.id = id.clone();

    let worker = fixture.start();

    fixture
        .until(|store| {
            store.tasks[&id].approval.is_some()
                && store.tasks[&id].notice.is_none()
                && store.tasks[&id].status == TaskStatus::Blocked
        })
        .await;

    fixture.stop(worker).await;
    fixture.context.stop = Arc::new(Stop::new());

    (fixture, provider, id)
}

fn effects(fixture: &Fixture) -> Vec<String> {
    fs::read_to_string(fixture.context.home.join("effects.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn generated(fixture: &Fixture) -> u64 {
    fs::read_to_string(fixture.context.home.join("generated")).unwrap().trim().parse().unwrap()
}

fn reply(fixture: &Fixture, id: &str, action: &str) -> String {
    let mut store = fixture.context.sessions.lock().unwrap();

    let task = store.tasks.keys().next().unwrap().clone();

    let input = Message {
        id: id.into(),
        session: fixture.origin.clone(),
        role: Role::User,
        sender: Some("desktop".into()),
        content: vec![Content::Text {
            text: if action.is_empty() { "/task".into() } else { format!("/task {action} {task}") },
        }],
    };

    dialogue::message(&mut store, &input, &fixture.context.plugins.tasks).unwrap().unwrap()
}

fn notices_matching(fixture: &Fixture, text: &str) -> usize {
    fixture.context.sessions.lock().unwrap().sessions[&fixture.origin]
        .messages
        .iter()
        .filter(|message| message.content.iter().any(|content| content.render().contains(text)))
        .count()
}

#[tokio::test]
async fn a_pending_approval_survives_restart_and_runs_the_exact_action_once() {
    let (mut fixture, provider, id) = pending().await;

    {
        let store = fixture.context.sessions.lock().unwrap();
        let approval = store.tasks[&id].approval.clone().unwrap();

        assert_eq!(approval.tool, "write");
        assert_eq!(approval.args, json!({"path":"note.txt","text":"orchid-17"}));
        assert_eq!(approval.decision, None);
        assert!(!store.tasks[&id].unsafe_step);
    }

    assert!(effects(&fixture).is_empty());

    assert_eq!(generated(&fixture), 2);
    assert_eq!(notices_matching(&fixture, "Approval needed before running write"), 1);

    let path = fixture.context.home.join("data/sessions/index.json");
    let restored = state::Store::load(path).unwrap();

    assert_eq!(restored.tasks[&id].status, TaskStatus::Blocked);

    assert_eq!(
        restored.tasks[&id].approval,
        fixture.context.sessions.lock().unwrap().tasks[&id].approval
    );

    fixture.context.sessions = Arc::new(Mutex::new(restored));

    let worker = fixture.start();

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(generated(&fixture), 2);
    assert!(effects(&fixture).is_empty());

    assert_eq!(
        reply(&fixture, "resume-while-waiting", "resume"),
        format!(
            "This task is waiting for your approval. Send `/task approve {}` or `/task deny {}`.",
            id, id
        )
    );

    let approved = reply(&fixture, "approve-1", "approve");

    assert_eq!(
        approved,
        "Approved. The task will run the saved action once and continue in this conversation."
    );

    assert_eq!(reply(&fixture, "approve-1", "approve"), approved);

    fixture.until(|store| store.tasks[&id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;

    let effects = effects(&fixture);

    assert_eq!(effects.len(), 1);

    assert_eq!(
        serde_json::from_str::<Value>(&effects[0]).unwrap()["arguments"],
        json!({"path":"note.txt","text":"orchid-17"})
    );

    assert_eq!(generated(&fixture), 3);

    {
        let store = fixture.context.sessions.lock().unwrap();
        let task = &store.tasks[&id];

        assert!(task.approval.is_none());
        assert_eq!(task.result.as_deref(), Some("The note is written."));
        assert!(store.sessions[&id].messages.iter().any(|message| {
            message.sender.as_deref() == Some("approval")
                && message.content[0].render().contains("has run once")
                && message.content[0].render().contains("untrusted data")
        }));

        assert_eq!(
            store.sessions[&fixture.origin].messages.last().unwrap().content[0].render(),
            "The note is written."
        );
    }

    assert_eq!(notices_matching(&fixture, "Approval needed before running write"), 1);

    provider.stop().await.unwrap();
}

#[tokio::test]
async fn denial_continues_without_the_action_and_other_conversations_cannot_decide() {
    let (fixture, provider, id) = pending().await;

    {
        let mut store = fixture.context.sessions.lock().unwrap();

        store.ensure("other", "model").unwrap();
        store.client("other", "desktop").unwrap();
    }

    let foreign = Message {
        id: "foreign-approve".into(),
        session: "other".into(),
        role: Role::User,
        sender: Some("desktop".into()),
        content: vec![Content::Text { text: format!("/task approve {id}") }],
    };

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let revision = store.tasks[&id].revision;

        let refused =
            dialogue::message(&mut store, &foreign, &fixture.context.plugins.tasks).unwrap();

        assert!(refused.unwrap().contains("not applied"));

        assert_eq!(store.tasks[&id].revision, revision);
        assert_eq!(store.tasks[&id].approval.as_ref().unwrap().decision, None);

        let stale = store.control_task(
            &id,
            &Command {
                id: "stale-approve".into(),
                action: Action::Approve,
                revision: revision - 1,
                instruction: String::new(),
            },
        );

        assert!(stale.unwrap_err().to_string().contains(&ControlError::Stale.to_string()));
    }

    assert_eq!(
        reply(&fixture, "deny-1", "deny"),
        "Denied, so the saved action will not run, and the task continues without it."
    );

    let worker = fixture.start();

    fixture.until(|store| store.tasks[&id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;

    assert!(effects(&fixture).is_empty());
    assert_eq!(generated(&fixture), 3);

    {
        let store = fixture.context.sessions.lock().unwrap();

        assert_eq!(store.tasks[&id].result.as_deref(), Some("The note was skipped."));
        assert!(store.tasks[&id].approval.is_none());
        assert!(store.sessions[&id].messages.iter().any(|message| {
            message.sender.as_deref() == Some("approval")
                && message.content[0].render().contains("did not run")
        }));
    }

    provider.stop().await.unwrap();
}

#[tokio::test]
async fn cancellation_discards_the_proposal_and_a_late_approval_cannot_revive_it() {
    let (fixture, provider, id) = pending().await;

    let revision = {
        let mut store = fixture.context.sessions.lock().unwrap();
        let revision = store.tasks[&id].revision;

        store
            .control_task(
                &id,
                &Command {
                    id: "pause-pending".into(),
                    action: Action::Pause,
                    revision,
                    instruction: String::new(),
                },
            )
            .unwrap();

        assert_eq!(store.tasks[&id].status, TaskStatus::Paused);
        assert!(store.tasks[&id].approval.is_some());

        let early = store.control_task(
            &id,
            &Command {
                id: "approve-paused".into(),
                action: Action::Approve,
                revision: revision + 1,
                instruction: String::new(),
            },
        );

        assert!(early.unwrap_err().to_string().contains(&ControlError::Approval.to_string()));

        store
            .control_task(
                &id,
                &Command {
                    id: "cancel-pending".into(),
                    action: Action::Cancel,
                    revision: revision + 1,
                    instruction: String::new(),
                },
            )
            .unwrap();

        assert_eq!(store.tasks[&id].status, TaskStatus::Cancelled);
        assert!(store.tasks[&id].approval.is_none());
        revision + 2
    };

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let late = store.control_task(
            &id,
            &Command {
                id: "approve-late".into(),
                action: Action::Approve,
                revision,
                instruction: String::new(),
            },
        );

        assert!(late.unwrap_err().to_string().contains(&ControlError::Terminal.to_string()));
    }

    let worker = fixture.start();

    tokio::time::sleep(Duration::from_millis(200)).await;
    fixture.stop(worker).await;

    assert!(effects(&fixture).is_empty());
    assert_eq!(generated(&fixture), 2);

    provider.stop().await.unwrap();
}

#[tokio::test]
async fn interrupted_approved_dispatch_retries_before_the_fence_and_never_replays_after_it() {
    let (fixture, provider, id) = pending().await;

    assert_eq!(
        reply(&fixture, "approve-crash", "approve"),
        "Approved. The task will run the saved action once and continue in this conversation."
    );

    let path = fixture.context.home.join("data/sessions/index.json");

    {
        let mut store = fixture.context.sessions.lock().unwrap();

        store.claim_task(&id, unix_seconds()).unwrap();
        store.recover_task(&id).unwrap();

        assert_eq!(store.tasks[&id].status, TaskStatus::Queued);
        assert_eq!(store.tasks[&id].approval.as_ref().unwrap().decision, Some(true));
        assert!(!store.tasks[&id].uncertain);

        store.claim_task(&id, unix_seconds()).unwrap();
        store.tasks.get_mut(&id).unwrap().unsafe_step = true;
        store.recover_task(&id).unwrap();
    }

    let restored = state::Store::load(path).unwrap();

    let task = &restored.tasks[&id];

    assert_eq!(task.status, TaskStatus::Blocked);
    assert!(task.uncertain);
    assert_eq!(task.approval.as_ref().unwrap().decision, Some(true));

    let mut store = restored;

    for action in [Action::Resume, Action::Approve] {
        let revision = store.tasks[&id].revision;

        let error = store
            .control_task(
                &id,
                &Command {
                    id: format!(
                        "after-crash-{}",
                        serde_json::to_value(action).unwrap().as_str().unwrap()
                    ),
                    action,
                    revision,
                    instruction: String::new(),
                },
            )
            .unwrap_err();

        assert!(error.to_string().contains(&ControlError::Uncertain.to_string()));
    }

    assert!(effects(&fixture).is_empty());
    assert_eq!(generated(&fixture), 2);

    provider.stop().await.unwrap();
}

#[test]
fn saved_approvals_are_bounded_exact_actions() {
    let approval = state::Approval {
        id: "task-1-approval-1-1".into(),
        tool: "write".into(),
        args: json!({"path":"note.txt"}),
        text: "Write note.txt".into(),
        generation: 1,
        decision: None,
        started: false,
    };

    assert!(approval.validate().is_ok());

    for invalid in [
        state::Approval { args: json!("not an object"), ..approval.clone() },
        state::Approval { text: " ".into(), ..approval.clone() },
        state::Approval { id: "../escape".into(), ..approval.clone() },
        state::Approval { tool: String::new(), ..approval.clone() },
        state::Approval { args: json!({"text":"x".repeat(70 * 1024)}), ..approval.clone() },
    ] {
        assert!(invalid.validate().is_err());
    }

    let encoded = serde_json::to_value(&approval).unwrap();

    assert_eq!(serde_json::from_value::<state::Approval>(encoded).unwrap(), approval);

    assert!(
        serde_json::from_value::<state::Approval>(json!({
            "id":"task-1-approval-1-1", "tool":"write", "args":{}, "text":"Write",
            "generation":1, "scope":"everything"
        }))
        .is_err()
    );
}

#[tokio::test]
async fn a_live_approval_is_withdrawn_when_answered_inline() {
    let mut fixture = prompted().await;
    let provider = fixture.provider("live-approval").await;
    let (handle, mut notices) = live(&fixture, Arc::clone(&provider)).await;

    loop {
        let notice = tokio::time::timeout(Duration::from_secs(8), notices.recv()).await.unwrap();

        if matches!(notice, Some(StreamNotice::Approval { .. })) {
            break;
        }
    }

    fixture.id = {
        let store = fixture.context.sessions.lock().unwrap();
        let task = store.tasks.values().next().unwrap();

        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.approval.as_ref().unwrap().args["text"], "orchid-17");
        task.id.clone()
    };

    let pending = fixture.context.approvals.lock().await.list().unwrap();

    assert_eq!(pending.len(), 1);

    assert_eq!(
        fixture.context.approvals.lock().await.resolve_local(&pending[0].id, true),
        Some(true)
    );

    let answer =
        tokio::time::timeout(Duration::from_secs(8), handle).await.unwrap().unwrap().unwrap();

    assert_eq!(answer.model.text, "The note is written.");
    assert_eq!(effects(&fixture).len(), 1);
    assert_eq!(generated(&fixture), 2);
    assert!(fixture.context.sessions.lock().unwrap().tasks[&fixture.id].approval.is_none());

    drop(answer);
    provider.stop().await.unwrap();
}

#[tokio::test]
async fn a_live_approval_lost_with_its_turn_resumes_from_the_saved_proposal() {
    let mut fixture = prompted().await;
    let provider = fixture.provider("live-approval").await;
    let (handle, notices) = live(&fixture, Arc::clone(&provider)).await;

    fixture.until(|store| store.tasks.values().any(|task| task.approval.is_some())).await;

    handle.abort();

    let _ = handle.await;

    drop(notices);

    fixture.id = fixture.context.sessions.lock().unwrap().tasks.keys().next().unwrap().clone();

    let path = fixture.context.home.join("data/sessions/index.json");
    let mut restored = state::Store::load(path).unwrap();
    let id = fixture.id.clone();

    restored.clear(&fixture.origin, "idle").unwrap();

    assert_eq!(restored.tasks[&id].approval.as_ref().unwrap().decision, None);

    fixture.context.sessions = Arc::new(Mutex::new(restored));

    let worker = fixture.start();

    fixture
        .until(|store| {
            store.tasks[&id].status == TaskStatus::Blocked && store.tasks[&id].notice.is_none()
        })
        .await;

    assert!(effects(&fixture).is_empty());
    assert_eq!(generated(&fixture), 1);
    assert_eq!(notices_matching(&fixture, "Approval needed before running write"), 1);

    reply(&fixture, "approve-live", "approve");

    fixture.until(|store| store.tasks[&id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;

    assert_eq!(effects(&fixture).len(), 1);
    assert_eq!(generated(&fixture), 2);

    assert_eq!(
        fixture.context.sessions.lock().unwrap().tasks[&id].result.as_deref(),
        Some("The note is written.")
    );

    provider.stop().await.unwrap();
}

#[tokio::test]
async fn status_questions_and_controls_during_an_approval_wait_keep_the_proposal() {
    let (fixture, provider, id) = pending().await;
    let status = reply(&fixture, "status-1", "");

    assert!(status.to_lowercase().contains("approval"), "{status}");

    let question = Message {
        id: "question-1".into(),
        session: fixture.origin.clone(),
        role: Role::User,
        sender: Some("desktop".into()),
        content: vec![Content::Text { text: "What is the capital of Peru?".into() }],
    };

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let routed =
            dialogue::message(&mut store, &question, &fixture.context.plugins.tasks).unwrap();

        assert!(routed.is_none(), "{routed:?}");
        assert_eq!(store.tasks[&id].status, TaskStatus::Blocked);
    }

    let paused = reply(&fixture, "pause-1", "pause");

    assert!(paused.contains("paused"), "{paused}");

    let early = reply(&fixture, "approve-paused", "approve");

    assert!(early.contains(&format!("/task resume {id}")), "{early}");

    {
        let store = fixture.context.sessions.lock().unwrap();

        assert_eq!(store.tasks[&id].status, TaskStatus::Paused);
        assert_eq!(store.tasks[&id].approval.as_ref().unwrap().decision, None);
    }

    assert_eq!(
        reply(&fixture, "resume-1", "resume"),
        format!(
            "This task is waiting for your approval. Send `/task approve {id}` or `/task deny \
                {id}`."
        )
    );

    {
        let store = fixture.context.sessions.lock().unwrap();

        assert_eq!(store.tasks[&id].status, TaskStatus::Blocked);
        assert_eq!(store.tasks[&id].approval.as_ref().unwrap().tool, "write");
    }

    assert!(effects(&fixture).is_empty());

    assert_eq!(generated(&fixture), 2);

    provider.stop().await.unwrap();
}

#[tokio::test]
async fn an_approved_handoff_runs_once_and_is_not_replayed_by_later_steps() {
    let (mut fixture, provider, id) = pending_in("approval-handoff").await;

    assert_eq!(
        fixture.context.sessions.lock().unwrap().tasks[&id].approval.as_ref().unwrap().tool,
        "task_classify"
    );

    reply(&fixture, "approve-handoff", "approve");

    let worker = fixture.start();

    fixture.until(|store| store.tasks[&id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;
    fixture.context.stop = Arc::new(Stop::new());

    {
        let store = fixture.context.sessions.lock().unwrap();
        let task = &store.tasks[&id];

        assert!(task.approval.is_none());
        assert!(!task.foreground);
        assert_eq!(task.instruction, "Finish the verified report in the background.");
    }

    assert_eq!(generated(&fixture), 3);

    provider.stop().await.unwrap();
}

#[tokio::test]
async fn a_started_approved_action_is_not_run_again_after_its_step_fails() {
    let (fixture, provider, id) = pending().await;

    reply(&fixture, "approve-fenced", "approve");

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let approval = store.tasks[&id].approval.clone().unwrap();
        let task = store.claim_task(&id, unix_seconds()).unwrap();
        let run = krabb_core::task::Run { id: id.clone(), generation: task.step };

        store.start_approval(&run, &approval.id).unwrap();
        store.recover_task(&id).unwrap();

        assert_eq!(store.tasks[&id].status, TaskStatus::Queued);
        assert!(store.tasks[&id].approval.as_ref().unwrap().started);
        assert!(store.start_approval(&run, &approval.id).is_err());
    }

    let worker = fixture.start();

    fixture.until(|store| store.tasks[&id].status == TaskStatus::Completed).await;
    fixture.stop(worker).await;

    assert!(effects(&fixture).is_empty());

    {
        let store = fixture.context.sessions.lock().unwrap();

        assert!(store.tasks[&id].approval.is_none());
        assert!(store.sessions[&id].messages.iter().any(|message| {
            message.sender.as_deref() == Some("approval")
                && message.content[0].render().contains("outcome is unknown")
        }));
    }

    provider.stop().await.unwrap();
}

#[tokio::test]
async fn a_deferred_resume_keeps_an_undecided_approval_blocked() {
    let (fixture, provider, id) = pending().await;

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let input = Message {
            id: "defer-approval".into(),
            session: fixture.origin.clone(),
            role: Role::User,
            sender: Some("desktop".into()),
            content: vec![Content::Text { text: format!("/task defer {id} 2030-11-12T09:00Z") }],
        };

        dialogue::message(&mut store, &input, &fixture.context.plugins.tasks).unwrap();

        let due = store.tasks[&id].deferred.as_ref().unwrap().command.when.at;

        assert_eq!(store.release_deferred(due).unwrap(), 1);
        assert_eq!(store.tasks[&id].status, TaskStatus::Blocked);
        assert_eq!(store.tasks[&id].approval.as_ref().unwrap().decision, None);
    }

    provider.stop().await.unwrap();
}

#[test]
fn approval_steps_only_change_the_current_saved_proposal() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();

    store.tasks.get_mut(&fixture.id).unwrap().tools = true;

    let task = store.claim_task(&fixture.id, unix_seconds()).unwrap();
    let run = krabb_core::task::Run { id: task.id, generation: task.step };
    let id = format!("{}-approval-{}-1", run.id, run.generation);
    let args = json!({"path":"note.txt"});

    let result = |text: &str| Message {
        id: format!("approval-result-{text}"),
        session: run.id.clone(),
        role: Role::Tool,
        sender: Some("write".into()),
        content: vec![Content::Text { text: text.into() }],
    };

    store.propose_approval(&run, 1, "write", &args, "Write note.txt").unwrap();

    let revision = store.tasks[&run.id].revision;

    store.withdraw_approval(&run, 2).unwrap();

    assert_eq!(store.tasks[&run.id].approval.as_ref().unwrap().id, id);
    assert_eq!(store.tasks[&run.id].revision, revision);

    let error = store.settle_approval(&run, "another-approval", result("stale")).unwrap_err();

    assert!(error.to_string().contains("superseded"));
    assert_eq!(store.tasks[&run.id].approval.as_ref().unwrap().id, id);

    store.tasks.get_mut(&run.id).unwrap().approval.as_mut().unwrap().decision = Some(true);

    let error = store.escalate_approval(&run).unwrap_err();

    assert!(error.to_string().contains("superseded"));
    assert_eq!(store.tasks[&run.id].status, TaskStatus::Running);

    store.settle_approval(&run, &id, result("Written.")).unwrap();

    assert!(store.tasks[&run.id].approval.is_none());
    assert!(store.tasks[&run.id].revision > revision);

    let revision = store.tasks[&run.id].revision;

    store.withdraw_approval(&run, 1).unwrap();

    assert_eq!(store.tasks[&run.id].revision, revision);
    assert!(store.escalate_approval(&run).unwrap_err().to_string().contains("superseded"));
    assert!(store.settle_approval(&run, &id, result("again")).is_err());

    store.propose_approval(&run, 3, "write", &args, "Write note.txt").unwrap();
    store.escalate_approval(&run).unwrap();

    assert_eq!(store.tasks[&run.id].status, TaskStatus::Blocked);
    assert!(store.tasks[&run.id].notice.is_some());

    store.withdraw_approval(&run, 3).unwrap();

    assert!(store.tasks[&run.id].approval.is_none());
}

#[test]
fn refused_approvals_say_why() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();

    let task = store.claim_task(&fixture.id, unix_seconds()).unwrap();
    let run = krabb_core::task::Run { id: task.id, generation: task.step };
    let args = json!({"path":"note.txt"});

    let refusal = |store: &mut crate::state::Store, call| {
        store
            .propose_approval(&run, call, "write", &args, "Write note.txt")
            .unwrap_err()
            .to_string()
    };

    store.tasks.get_mut(&run.id).unwrap().tools = false;

    assert!(refusal(&mut store, 1).contains("`tools = true`"));

    store.tasks.get_mut(&run.id).unwrap().tools = true;
    store.propose_approval(&run, 2, "write", &args, "Write note.txt").unwrap();

    assert!(refusal(&mut store, 3).contains("already waiting"));

    store.withdraw_approval(&run, 2).unwrap();
    store.tasks.get_mut(&run.id).unwrap().uncertain = true;

    assert!(refusal(&mut store, 4).contains("uncertain step"));
}
