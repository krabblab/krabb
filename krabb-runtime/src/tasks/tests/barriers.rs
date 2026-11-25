use super::*;

use krabb_core::{
    operation::{Receipt, Spec, Start, State},
    task::Run,
};

const RESULT: &str = "Verified external artifact.";

#[derive(Default)]
struct Remote {
    starts: u32,
    completed: bool,
    cancelled: bool,
}

fn spec() -> Spec {
    Spec {
        name: "delegate".into(),
        description: "Produce the external report.".into(),
        input: json!({"type":"object","properties":{},"additionalProperties":false}),
        cancel: true,
    }
}

fn receipt(state: State) -> Receipt {
    Receipt {
        state,
        handle: Some("opaque-handle".into()),
        result: (state == State::Completed).then(|| ToolResult::text(RESULT)),
        detail: None,
    }
}

fn key(id: &str) -> String {
    format!("{id}-operation")
}

/// Advances the task by one host action chosen only from durable state and the backend.
fn step(store: &mut state::Store, remote: &mut Remote, id: &str) -> Option<&'static str> {
    let task = store.tasks[id].clone();

    let at = unix_seconds();

    if task.status == TaskStatus::Completed {
        return None;
    }

    if task.notice.is_some() {
        store.publish_task_notice(id).unwrap();
        return Some("delivery");
    }

    let active = task.operation.as_ref().is_some_and(|operation| operation.active());

    if task.status == TaskStatus::Cancelling && !active {
        store.finish_stop(id, task.step).unwrap();
        return Some("stop");
    }

    if task.status == TaskStatus::Cancelled && !active {
        return None;
    }

    let Some(operation) = &task.operation else {
        return match task.status {
            TaskStatus::Queued => {
                store.claim_task(id, at).unwrap();
                store.ensure_task(id, "model").unwrap();
                Some("claim")
            }

            TaskStatus::Running => {
                let run = Run { id: id.into(), generation: task.step };
                let start = Start { id: key(id), name: "delegate".into(), arguments: json!({}) };

                store.prepare_operation(&run, "runner", spec(), start).unwrap();
                Some("intent")
            }

            _ => None,
        };
    };

    let state = operation.receipt.as_ref().map(|receipt| receipt.state);

    match operation.phase {
        state::OperationPhase::Dispatching if remote.starts == 0 => {
            remote.starts += 1;
            Some("dispatch")
        }

        state::OperationPhase::Dispatching => {
            store.accept_operation(id, &key(id), receipt(State::Running), None, at).unwrap();
            Some("receipt")
        }

        state::OperationPhase::Uncertain if remote.starts > 0 => {
            store.accept_operation(id, &key(id), receipt(State::Running), None, at).unwrap();
            Some("reconcile")
        }

        state::OperationPhase::Uncertain => None,

        state::OperationPhase::Observing
            if state == Some(State::Running) && task.status == TaskStatus::Cancelling =>
        {
            store.record_operation_cancel(id, &key(id)).unwrap();
            remote.cancelled = true;
            store.accept_operation(id, &key(id), receipt(State::Cancelled), None, at).unwrap();
            Some("settle")
        }

        state::OperationPhase::Observing if state == Some(State::Running) => {
            remote.completed = true;
            store.accept_operation(id, &key(id), receipt(State::Completed), None, at).unwrap();
            Some("remote")
        }

        state::OperationPhase::Observing => {
            store.collect_operation(id, &key(id), at).unwrap();
            Some("collect")
        }

        state::OperationPhase::Collected if task.status == TaskStatus::Queued => {
            store.tasks.get_mut(id).unwrap().next = at;
            store.claim_task(id, at).unwrap();
            store.ensure_task(id, "model").unwrap();
            Some("resume")
        }

        state::OperationPhase::Collected if task.status == TaskStatus::Running => {
            store
                .checkpoint_task(id, "Collected.", TaskStatus::Reporting, at, Some(RESULT))
                .unwrap();
            Some("result")
        }

        _ => None,
    }
}

fn recover(store: &mut state::Store, id: &str) {
    let task = &store.tasks[id];

    if task.status == TaskStatus::Running
        || task.status.stopping()
        || task.budget.as_ref().is_some_and(|budget| budget.pending.is_some())
        || task
            .operation
            .as_ref()
            .is_some_and(|operation| operation.phase == state::OperationPhase::Dispatching)
    {
        store.recover_task(id).unwrap();
    }
}

fn deliveries(store: &state::Store, origin: &str) -> usize {
    store.sessions[origin]
        .messages
        .iter()
        .filter(|message| {
            message.sender.as_deref() == Some("task")
                && message.content.iter().any(|content| content.render() == RESULT)
        })
        .count()
}

#[test]
fn named_crash_barriers_recover_without_repeating_external_effects_or_deliveries() {
    let mut clean = Vec::new();

    {
        let fixture = Fixture::new(Principal::Client("desktop".into()));
        let mut store = fixture.context.sessions.lock().unwrap();
        let mut remote = Remote::default();

        store.tasks.get_mut(&fixture.id).unwrap().tools = true;

        while let Some(action) = step(&mut store, &mut remote, &fixture.id) {
            clean.push(action);

            assert!(clean.len() < 32, "{clean:?}");
        }

        assert_eq!(
            clean,
            [
                "claim", "intent", "dispatch", "receipt", "remote", "collect", "resume", "result",
                "delivery"
            ]
        );
    }

    let mut blocked = Vec::new();

    for barrier in 0..=clean.len() {
        let fixture = Fixture::new(Principal::Client("desktop".into()));
        let path = fixture.context.home.join("data/sessions/index.json");
        let mut remote = Remote::default();

        {
            let mut store = fixture.context.sessions.lock().unwrap();

            store.tasks.get_mut(&fixture.id).unwrap().tools = true;
            store.save().unwrap();

            for _ in 0..barrier {
                step(&mut store, &mut remote, &fixture.id).unwrap();
            }
        }

        let after = clean.get(barrier.wrapping_sub(1)).copied().unwrap_or("start");

        let mut store = state::Store::load(&path).unwrap();
        let mut recovered = Vec::new();

        recover(&mut store, &fixture.id);

        while let Some(action) = step(&mut store, &mut remote, &fixture.id) {
            recovered.push(action);

            assert!(recovered.len() < 32, "{after}: {recovered:?}");
        }

        let task = &store.tasks[&fixture.id];
        let context = format!("crash after {after}; recovery {recovered:?}");

        assert!(remote.starts <= 1, "{context}");

        if after == "intent" {
            assert_eq!(remote.starts, 0, "{context}");
            assert_eq!(task.status, TaskStatus::Blocked, "{context}");
            assert!(task.uncertain, "{context}");
            assert_eq!(deliveries(&store, &fixture.origin), 0, "{context}");

            blocked.push(after);
            continue;
        }

        assert_eq!(task.status, TaskStatus::Completed, "{context}");
        assert_eq!(task.result.as_deref(), Some(RESULT), "{context}");
        assert_eq!(remote.starts, 1, "{context}");
        assert!(remote.completed, "{context}");
        assert_eq!(deliveries(&store, &fixture.origin), 1, "{context}");
        assert!(!recovered.contains(&"dispatch") || barrier < 2, "{context}");
        assert!(store.claim_task(&fixture.id, u64::MAX).is_err(), "{context}");
    }

    assert_eq!(blocked, ["intent"]);
}

fn retained(store: &state::Store, id: &str) -> bool {
    store.sessions.get(id).is_some_and(|journal| {
        journal.messages.iter().any(|message| {
            message.sender.as_deref() == Some("operation")
                && message.content.iter().any(|content| content.render().contains(RESULT))
        })
    })
}

#[test]
fn cancellation_at_each_phase_reports_truthfully_and_never_starts_new_effects() {
    let phases = [
        "start", "claim", "intent", "dispatch", "receipt", "remote", "collect", "resume", "result",
        "delivery",
    ];

    for (barrier, after) in phases.into_iter().enumerate() {
        let fixture = Fixture::new(Principal::Client("desktop".into()));
        let mut store = fixture.context.sessions.lock().unwrap();
        let mut remote = Remote::default();

        store.tasks.get_mut(&fixture.id).unwrap().tools = true;

        for _ in 0..barrier {
            step(&mut store, &mut remote, &fixture.id).unwrap();
        }

        let starts = remote.starts;
        let revision = store.tasks[&fixture.id].revision;

        let receipt = store
            .control_task(
                &fixture.id,
                &krabb_core::task::Command {
                    id: format!("cancel-{after}"),
                    action: krabb_core::task::Action::Cancel,
                    revision,
                    instruction: String::new(),
                },
            )
            .unwrap();

        let mut actions = Vec::new();

        while let Some(action) = step(&mut store, &mut remote, &fixture.id) {
            actions.push(action);

            assert!(actions.len() < 32, "{after}: {actions:?}");
        }

        let task = &store.tasks[&fixture.id];
        let context = format!("cancel after {after}: {:?} then {actions:?}", receipt.state);
        let finished = matches!(after, "result" | "delivery");
        let running = matches!(after, "intent" | "dispatch" | "receipt");

        assert_eq!(remote.starts, starts + u32::from(after == "intent"), "{context}");
        assert_eq!(remote.cancelled, running, "{context}");
        assert_eq!(remote.completed, !running && barrier > 4, "{context}");
        assert_eq!(retained(&store, &fixture.id), remote.completed, "{context}");

        let expected = match after {
            "start" | "collect" => TaskStatus::Cancelled,
            "remote" => TaskStatus::Cancelled,
            "result" => TaskStatus::Reporting,
            "delivery" => TaskStatus::Completed,
            _ => TaskStatus::Cancelling,
        };

        assert_eq!(receipt.state, expected, "{context}");

        if finished {
            assert_eq!(task.status, TaskStatus::Completed, "{context}");
            assert_eq!(deliveries(&store, &fixture.origin), 1, "{context}");

            continue;
        }

        assert_eq!(task.status, TaskStatus::Cancelled, "{context}");
        assert_eq!(deliveries(&store, &fixture.origin), 0, "{context}");
        assert!(task.result.is_none(), "{context}");
        assert!(store.claim_task(&fixture.id, u64::MAX).is_err(), "{context}");

        if running || after == "remote" {
            let told = store.sessions[&fixture.origin].messages.iter().any(|message| {
                message
                    .content
                    .iter()
                    .any(|content| content.render().contains("The external operation has settled"))
            });

            assert!(told, "{context}");
        }
    }
}
