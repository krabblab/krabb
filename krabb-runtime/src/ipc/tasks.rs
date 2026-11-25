use super::*;
use serde::Deserialize;

#[cfg(test)]
mod scope;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    id: String,
    command: krabb_core::task::Command,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeferredControl {
    id: String,
    command: krabb_core::task::defer::Command,
}

pub(super) fn defer(params: &Value, state: &State) -> io::Result<Value> {
    let request: DeferredControl =
        serde_json::from_value(params.clone()).map_err(io::Error::other)?;

    apply(state, &request.id, |store| {
        store.defer_task(&request.id, &request.command, crate::unix_seconds())
    })
}

/// Applies a saved control, then stops an active step and wakes the worker as needed.
fn apply<R: serde::Serialize>(
    state: &State,
    id: &str,
    control: impl FnOnce(&mut Store) -> io::Result<R>,
) -> io::Result<Value> {
    let (receipt, current) = {
        let mut store = state.sessions.lock().map_err(lock)?;
        let receipt = control(&mut store)?;

        (receipt, store.tasks[id].status)
    };

    if current.stopping() {
        state.plugins.tasks.cancel(id);
    }

    state.plugins.tasks.wake.notify_one();

    Ok(json!({"id":id, "receipt":receipt, "current":current}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetControl {
    id: String,
    grant: krabb_core::execution::budget::Grant,
}

pub(super) fn grant(params: &Value, state: &State) -> io::Result<Value> {
    let request: BudgetControl =
        serde_json::from_value(params.clone()).map_err(io::Error::other)?;
    let mut store = state.sessions.lock().map_err(lock)?;
    let receipt = store.grant_budget(
        &request.id,
        &request.grant,
        Some(state.config.inference.task_ceiling_millis()),
    )?;
    let task = &store.tasks[&request.id];

    Ok(json!({"id":request.id,"receipt":receipt,"current":task.status,
        "inference":task.budget.as_ref().map(crate::accounting::view)}))
}

pub(super) fn list(state: &State) -> io::Result<Value> {
    let store = state.sessions.lock().map_err(lock)?;
    Ok(json!({"items":store.tasks.values().map(|task| json!({
        "id":task.id, "status":task.status, "revision":task.revision,
        "goal":super::super::clip(task.instruction.clone(), SUMMARY_LIMIT),
        "checkpoint":super::super::clip(task.checkpoint.clone(), SUMMARY_LIMIT),
        "steps":task.step, "uncertain":task.uncertain,
        "retry":task.retry,
        "resume":task.deferred.as_ref().map(|receipt| &receipt.command.when),
        "sequence":task.budget.as_ref().map(|budget| budget.sequence),
        "inference":task.budget.as_ref().map(crate::accounting::view),
    })).collect::<Vec<_>>()}))
}

pub(super) fn control(params: &Value, state: &State) -> io::Result<Value> {
    let request: Control = serde_json::from_value(params.clone()).map_err(io::Error::other)?;

    apply(state, &request.id, |store| store.control_task(&request.id, &request.command))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageControl {
    session: String,
    client: String,
    id: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Submission {
    session: String,
    client: String,
    plugin: String,
    id: String,
    text: String,
}

pub(super) fn enqueue(params: &Value, state: &State) -> io::Result<Value> {
    let request: Submission = serde_json::from_value(params.clone()).map_err(io::Error::other)?;
    let mut store = state.sessions.lock().map_err(lock)?;

    enqueue_in(request, state, &mut store)
}

fn enqueue_in(request: Submission, state: &State, store: &mut Store) -> io::Result<Value> {
    use super::super::*;
    use std::sync::atomic::Ordering;

    if request.id.is_empty()
        || request.id.len() > META_LIMIT
        || request.text.trim().is_empty()
        || request.text.len() > TEXT_LIMIT
        || !valid(&request.plugin)
        || !state.known_client(&request.client)
    {
        return Err(io::Error::other("Invalid queued request."));
    }

    if !state.home.join("plugins").join(&request.plugin).is_dir() {
        return Err(io::Error::other("The selected model plugin is not installed."));
    }

    let principal = sharing::Principal::Client(request.client.clone());

    let source = store.session(&request.session)?;

    if !super::requests::owned(source, &principal) {
        return Err(io::Error::other("The client does not own this conversation."));
    }

    let input = Message {
        id: request.id.clone(),
        session: request.session.clone(),
        role: Role::User,
        sender: Some(request.client.clone()),
        content: vec![Content::Text { text: request.text.clone() }],
    };

    if let Some(task) = super::requests::find(store, &principal, &request.session, &request.id)? {
        let saved = store.sessions.get(&task.id).and_then(|journal| {
            journal
                .messages
                .iter()
                .chain(&journal.original)
                .find(|message| message.id == request.id && message.role == Role::User)
        });

        let matches = if let Some(input_identity) =
            task.input.as_ref().filter(|input| input.fingerprint.is_some())
        {
            input_identity.matches(&input)?
        } else {
            saved.is_some_and(|message| {
                message.content.iter().map(Content::render).collect::<Vec<_>>().join("\n")
                    == request.text
            })
        };

        if !matches || task.provider != request.plugin {
            return Err(io::Error::other("This request identity is already in use."));
        }

        return Ok(json!({"task":task.id,"status":task.status,"replayed":true}));
    }

    if store.retired_request(&principal, &request.session, &request.id).is_some() {
        return Err(io::Error::other(super::requests::EXPIRED));
    }

    let policy = state.config.clients.get(&request.client).cloned().unwrap_or_default();
    let workspace =
        source.workspace.as_ref().map(PathBuf::from).unwrap_or_else(|| state.root.clone());

    if policy.worktree && source.isolated.is_none() {
        return Err(io::Error::other("Initialize this conversation's isolated workspace first."));
    }

    let live = state.client(&request.client)?;
    let attach = source.source.is_none();
    let model = source.model.clone();
    let mut history = source.context();

    history.push(input.clone());

    let route = routing::Route {
        kind: routing::Kind::Fast,
        message: String::new(),
        instruction: tasks::goal(request.text),
    };

    let run = store.atomic(|store| {
        if attach {
            store.client(&request.session, &request.client)?;
        }

        let context = tasks::capture(
            store,
            &request.plugin,
            &model,
            &workspace,
            &request.client,
            &request.session,
            None,
            &request.session,
            live.tools.load(Ordering::Acquire),
            live.shell.load(Ordering::Acquire),
        )
        .map_err(io::Error::other)?;

        let task = tasks::record(&route, &context).map_err(io::Error::other)?;

        store.queue_foreground(task, history, input)
    })?;

    state.plugins.tasks.wake.notify_one();
    Ok(json!({"task":run.id,"status":"queued","replayed":false}))
}

pub(super) fn message(params: &Value, state: &State) -> io::Result<Value> {
    let request: MessageControl =
        serde_json::from_value(params.clone()).map_err(io::Error::other)?;

    if request.id.is_empty()
        || request.id.len() > super::super::META_LIMIT
        || request.text.len() > krabb_core::task::INSTRUCTION_LIMIT
        || !state.known_client(&request.client)
    {
        return Err(io::Error::other("Invalid task-control message."));
    }

    let mut store = state.sessions.lock().map_err(lock)?;
    let session = store.session(&request.session)?;

    if !super::requests::owned(
        session,
        &super::super::sharing::Principal::Client(request.client.clone()),
    ) {
        return Err(io::Error::other("This is not a conversation for that client."));
    }

    let input = super::super::Message {
        id: request.id,
        session: request.session,
        role: super::super::Role::User,
        sender: Some(request.client),
        content: vec![super::super::Content::Text { text: request.text }],
    };

    let key = super::super::tasks::dialog::key(&input.session, &input.id);
    let replayed = session.commands.iter().any(|control| control.id == key);

    let Some(reply) =
        super::super::tasks::dialog::message(&mut store, &input, &state.plugins.tasks)
            .map_err(io::Error::other)?
    else {
        return Ok(json!({"handled":false}));
    };

    Ok(json!({"handled":true, "text":reply, "replayed":replayed}))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn fixture(label: &str) -> (State, String) {
        let mut state = super::super::tests::state_at(label);
        let _ = std::fs::remove_dir_all(&state.home);

        for plugin in ["fixture", "missing-provider"] {
            std::fs::create_dir_all(state.home.join("plugins").join(plugin)).unwrap();
        }

        state.sessions = Arc::new(Mutex::new(
            crate::state::Store::load(state.home.join("data/sessions/index.json")).unwrap(),
        ));
        let mut store = state.sessions.lock().unwrap();
        store.ensure("conversation", "model").unwrap();
        store.client("conversation", krabb_core::client::TUI).unwrap();
        let context = crate::tasks::capture(
            &store,
            "fixture",
            "model",
            &state.root,
            krabb_core::client::TUI,
            "conversation",
            None,
            "conversation",
            false,
            false,
        )
        .unwrap();
        let route = crate::routing::Route::parse(
            json!({"kind":"background", "message":"I will prepare the report.",
            "instruction":"Research and complete the report."}),
        )
        .unwrap();
        let id = crate::tasks::enqueue(&mut store, &route, &context).unwrap();
        drop(store);
        (state, id)
    }

    #[test]
    fn a_request_for_an_uninstalled_provider_is_refused_before_it_is_saved() {
        let (state, _) = fixture("queued-uninstalled-provider");
        let tasks = state.sessions.lock().unwrap().tasks.len();

        let request = json!({
            "session":"conversation", "client":krabb_core::client::TUI, "plugin":"absent",
            "id":"new-question", "text":"Explain the dataset."
        });

        let error = enqueue(&request, &state).unwrap_err();

        assert!(error.to_string().contains("not installed"), "{error}");
        assert_eq!(state.sessions.lock().unwrap().tasks.len(), tasks);
    }

    #[test]
    fn a_refused_request_leaves_a_new_conversation_unattached() {
        let (mut state, _) = fixture("queued-refused-attachment");

        state.config.clients.entry(krabb_core::client::TUI.into()).or_default().worktree = true;
        state.sessions.lock().unwrap().ensure("fresh", "model").unwrap();

        let request = json!({
            "session":"fresh", "client":krabb_core::client::TUI, "plugin":"fixture",
            "id":"new-question", "text":"Explain the dataset."
        });

        let error = enqueue(&request, &state).unwrap_err();

        assert!(error.to_string().contains("isolated workspace"), "{error}");
        assert_eq!(state.sessions.lock().unwrap().sessions["fresh"].source, None);
    }

    #[test]
    fn queued_client_requests_preserve_the_active_lease_and_survive_restart() {
        let (state, previous) = fixture("queued-client-input");
        state.sessions.lock().unwrap().reserve("conversation", "active-owner".into()).unwrap();
        let request = json!({
            "session":"conversation", "client":krabb_core::client::TUI, "plugin":"missing-provider",
            "id":"new-question", "text":"Explain the second dataset."
        });

        let first = enqueue(&request, &state).unwrap();
        let task = first["task"].as_str().unwrap();

        assert_eq!(first["status"], "queued");
        assert_eq!(enqueue(&request, &state).unwrap()["replayed"], true);
        let store = state.sessions.lock().unwrap();

        assert_eq!(store.tasks.len(), 2);
        assert_eq!(
            store.sessions["conversation"].reservation_owner.as_deref(),
            Some("active-owner")
        );

        assert_eq!(store.tasks[&previous].status, crate::state::TaskStatus::Queued);
        assert_eq!(store.tasks[task].request.as_deref(), Some("new-question"));
        assert_eq!(
            store.sessions[task].context().last().unwrap().content[0].render(),
            "Explain the second dataset."
        );
        drop(store);

        let recovered =
            crate::state::Store::load(state.home.join("data/sessions/index.json")).unwrap();
        assert_eq!(recovered.tasks[task].status, crate::state::TaskStatus::Queued);
        assert!(recovered.sessions["conversation"].queued.is_empty());
        let mut changed = request;
        changed["text"] = json!("A different question");

        assert!(enqueue(&changed, &state).is_err());
    }

    #[test]
    fn queued_requests_keep_the_full_text_when_the_goal_preview_is_bounded() {
        let (state, _) = fixture("queued-large-input");
        let text = "é".repeat(crate::TOOL_ARGS_LIMIT);
        let request = json!({"session":"conversation", "client":krabb_core::client::TUI,
            "plugin":"fixture", "id":"large-question", "text":text});
        let receipt = enqueue(&request, &state).unwrap();
        let store = state.sessions.lock().unwrap();
        let task = &store.tasks[receipt["task"].as_str().unwrap()];

        assert!(task.instruction.len() <= crate::TOOL_ARGS_LIMIT);
        assert_eq!(
            store.resolved_context(&task.id).unwrap().last().unwrap().content[0].render(),
            text
        );
        drop(store);

        assert_eq!(enqueue(&request, &state).unwrap()["replayed"], true);
    }

    #[test]
    fn failed_queued_admission_preserves_the_conversation_and_existing_goal() {
        let (state, previous) = fixture("queued-input-failure");
        let mut store = state.sessions.lock().unwrap();
        let messages = store.sessions["conversation"].messages.clone();
        store.record_limit = Some(1);
        drop(store);
        let request = json!({"session":"conversation", "client":krabb_core::client::TUI,
            "plugin":"fixture", "id":"new-question", "text":"Explain this calculation."});

        assert!(enqueue(&request, &state).is_err());
        let store = state.sessions.lock().unwrap();

        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[&previous].status, crate::state::TaskStatus::Queued);
        assert_eq!(store.sessions["conversation"].messages, messages);
    }

    #[test]
    fn task_commands_work_without_a_provider_and_reject_message_id_reuse() {
        let (state, id) = fixture("task-commands");
        let request = json!({"session":"conversation", "client":krabb_core::client::TUI,
            "id":"human-message", "text":format!("/task pause {id}")});
        let first = message(&request, &state).unwrap();

        assert_eq!(first["handled"], true);
        assert_eq!(first["replayed"], false);
        assert!(first["text"].as_str().unwrap().contains("paused"));
        assert_eq!(message(&request, &state).unwrap()["replayed"], true);
        assert_eq!(state.sessions.lock().unwrap().tasks[&id].revision, 1);

        let mut changed = request;
        changed["text"] = json!(format!("/task cancel {id}"));

        assert!(message(&changed, &state).is_err());
        changed["client"] = json!("unregistered");

        assert!(message(&changed, &state).is_err());
        assert_eq!(
            state.sessions.lock().unwrap().tasks[&id].status,
            crate::state::TaskStatus::Paused
        );

        assert_eq!(
            message(
                &json!({"session":"conversation", "client":krabb_core::client::TUI,
            "id":"ordinary", "text":"pause the report"}),
                &state
            )
            .unwrap()["handled"],
            false
        );
    }

    #[test]
    fn local_controls_require_revisions_and_preserve_idempotent_receipts() {
        let (state, id) = fixture("task-controls");
        let args = json!({"id":id, "command":{"id":"pause-1", "action":"pause", "revision":0}});
        let first = control(&args, &state).unwrap();

        assert_eq!(first["current"], "paused");
        assert_eq!(control(&args, &state).unwrap(), first);
        assert_eq!(list(&state).unwrap()["items"][0]["status"], "paused");
        assert!(
            control(&json!({"id":id, "command":{"id":"resume-1", "action":"resume"}}), &state)
                .is_err()
        );

        let forged = json!({
            "id":id, "owner":"forged",
            "command":{"id":"resume-1", "action":"resume", "revision":1}
        });

        assert!(control(&forged, &state).is_err());
        assert_eq!(
            state.sessions.lock().unwrap().tasks[&id].status,
            crate::state::TaskStatus::Paused
        );
    }

    #[tokio::test]
    async fn authenticated_controls_bypass_busy_work_slots_and_plugin_update_locks() {
        let (mut state, id) = fixture("task-control-admission");
        state.slots = Arc::new(Semaphore::new(1));
        let busy = Arc::clone(&state.slots).acquire_owned().await.unwrap();
        let updating = Arc::clone(&state.plugins.lifecycle).write_owned().await;
        let state = Arc::new(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = Arc::clone(&state);
        let server = tokio::spawn(serve(listener, server_state));
        let args = json!({"id":id, "command":{"id":"pause-1", "action":"pause", "revision":0}});

        for (token, expected) in [("invalid", false), ("secret", true)] {
            let mut stream = TcpStream::connect(address).await.unwrap();
            jsonl::write(&mut stream, &IpcRequest::call(7, token, "task.control", args.clone()))
                .await
                .unwrap();
            let mut input = BufReader::new(stream);
            let response =
                timeout(Duration::from_secs(1), jsonl::read::<IpcResponse>(&mut input, FRAME))
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();

            if expected {
                assert_eq!(response.result.unwrap()["current"], "paused");
            } else {
                assert_eq!(response.error.unwrap().code, 401);
                assert_eq!(
                    state.sessions.lock().unwrap().tasks[&id].status,
                    crate::state::TaskStatus::Queued
                );
            }
        }

        assert_eq!(state.slots.available_permits(), 0);
        state.stop.signal();
        timeout(Duration::from_secs(1), server).await.unwrap().unwrap().unwrap();
        drop(updating);
        drop(busy);
    }

    #[tokio::test]
    async fn timed_resume_is_authenticated_and_remains_available_during_busy_execution() {
        let (mut state, id) = fixture("deferred-control-admission");
        state.slots = Arc::new(Semaphore::new(1));
        let busy = Arc::clone(&state.slots).acquire_owned().await.unwrap();
        let updating = Arc::clone(&state.plugins.lifecycle).write_owned().await;
        let state = Arc::new(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(serve(listener, Arc::clone(&state)));
        let at = crate::unix_seconds() + 86_400;
        let args = json!({"id":id,"command":{"id":"timed-1","revision":0,
            "when":{"at":at,"timezone":"Europe/Rome"}}});
        let mut receipt = None;

        for token in ["invalid", "secret", "secret"] {
            let mut socket = TcpStream::connect(address).await.unwrap();
            jsonl::write(&mut socket, &IpcRequest::call(7, token, "task.defer", args.clone()))
                .await
                .unwrap();
            let response: IpcResponse =
                timeout(Duration::from_secs(1), jsonl::read(&mut BufReader::new(socket), FRAME))
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();

            if token == "invalid" {
                assert_eq!(response.error.unwrap().code, 401);
                assert!(state.sessions.lock().unwrap().tasks[&id].deferred.is_none());
            } else {
                let result = response.result.unwrap();

                assert_eq!(result["current"], "paused");
                assert_eq!(result["receipt"]["command"]["when"]["at"], at);

                if let Some(previous) = &receipt {
                    assert_eq!(&result, previous);
                }

                receipt = Some(result);
            }
        }

        assert_eq!(state.sessions.lock().unwrap().tasks[&id].deferrals.len(), 1);
        assert_eq!(state.slots.available_permits(), 0);
        state.stop.signal();
        timeout(Duration::from_secs(1), server).await.unwrap().unwrap().unwrap();
        drop(updating);
        drop(busy);
    }

    #[tokio::test]
    async fn budget_grants_require_local_authentication_and_bypass_busy_execution() {
        let (mut state, id) = fixture("budget-grant-admission");
        state.config.inference.task_seconds = Some(1);
        state.config.inference.task_ceiling_seconds = 2;
        let grant = {
            let mut store = state.sessions.lock().unwrap();
            let task = store.claim_task(&id, crate::unix_seconds()).unwrap();
            let run = krabb_core::task::Run { id: id.clone(), generation: task.step };
            let reserved = store.reserve_budget(&run, 1_000, Some(1_000)).unwrap();
            store.settle_budget(&id, reserved, None).unwrap();
            let revision = store.tasks[&id].revision;
            store
                .control_task(
                    &id,
                    &krabb_core::task::Command {
                        id: "pause-before-grant".into(),
                        action: krabb_core::task::Action::Pause,
                        revision,
                        instruction: String::new(),
                    },
                )
                .unwrap();
            store.finish_task_control(&id, run.generation).unwrap();
            krabb_core::execution::budget::Grant {
                id: "grant-1".into(),
                revision: store.tasks[&id].revision,
                sequence: reserved.id,
                limit_millis: 2_000,
            }
        };

        state.slots = Arc::new(Semaphore::new(1));
        let busy = Arc::clone(&state.slots).acquire_owned().await.unwrap();
        let updating = Arc::clone(&state.plugins.lifecycle).write_owned().await;
        let state = Arc::new(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(serve(listener, Arc::clone(&state)));
        let args = json!({"id":id,"grant":grant});

        for token in ["invalid", "secret", "secret"] {
            let mut socket = TcpStream::connect(address).await.unwrap();
            jsonl::write(&mut socket, &IpcRequest::call(7, token, "task.budget", args.clone()))
                .await
                .unwrap();
            let response: IpcResponse =
                timeout(Duration::from_secs(1), jsonl::read(&mut BufReader::new(socket), FRAME))
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();

            if token == "invalid" {
                assert_eq!(response.error.unwrap().code, 401);
                assert_eq!(
                    state.sessions.lock().unwrap().tasks[&id].budget.as_ref().unwrap().limit_millis,
                    Some(1_000)
                );
            } else {
                let result = response.result.unwrap();

                assert_eq!(result["current"], "paused");
                assert_eq!(result["inference"]["limit_millis"], 2_000);
                assert_eq!(result["inference"]["unresolved_millis"], 1_000);
            }
        }
        {
            let mut store = state.sessions.lock().unwrap();
            let before = store.tasks[&id].budget.clone();

            assert!(
                crate::tasks::control(
                    &mut store,
                    &json!({"action":"budget","id":id,"limit_millis":3_000}),
                    "conversation",
                    None,
                    &state.plugins.tasks,
                    false
                )
                .is_err()
            );

            assert_eq!(store.tasks[&id].budget, before);
        }

        assert_eq!(state.slots.available_permits(), 0);
        state.stop.signal();
        timeout(Duration::from_secs(1), server).await.unwrap().unwrap().unwrap();
        drop(updating);
        drop(busy);
    }
}
