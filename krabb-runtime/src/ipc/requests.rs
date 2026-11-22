use super::*;
use serde::Deserialize;

pub(super) const EXPIRED: &str = concat!(
    "This request was already handled and its saved result has expired. ",
    "Send it again as a new message to run it."
);

/// Whether a client conversation belongs to `source`; an unclaimed one belongs to any client.
pub(super) fn owned(session: &crate::state::Session, source: &crate::sharing::Principal) -> bool {
    !session.is_work()
        && session.channel.is_none()
        && session.source.as_ref().is_none_or(|owner| owner == source)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lookup {
    session: String,
    client: String,
    request: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cleanup {
    id: String,
}

pub(super) fn cleanup(params: &Value, state: &State) -> io::Result<Value> {
    let request: Cleanup =
        serde_json::from_value(params.clone()).map_err(|error| invalid(&error.to_string()))?;
    let mut store = state.sessions.lock().map_err(lock)?;
    let session =
        store.sessions.get(&request.id).ok_or_else(|| not_found("Conversation not found."))?;

    if session.is_work() || session.channel.is_some() {
        return Err(invalid("Only a client conversation can be cleaned up."));
    }

    if session.reservation_active
        || session.status == "working"
        || session.inflight.is_some()
        || !session.queued.is_empty()
        || store.jobs.values().any(|job| job.origin.session == request.id && job.protected())
    {
        return Ok(json!({"id":request.id,"deleted":false}));
    }

    store.remove_deep(&request.id)?;
    drop(store);

    let worktree = match super::super::state::remove_worktree(&state.home, &request.id) {
        Ok(()) => json!({"status":"removed"}),
        Err(error) => json!({"status":"pending","error":super::super::sentence(error.to_string())}),
    };

    state.cancels.lock().map_err(lock)?.remove(&request.id);
    Ok(json!({"id":request.id,"deleted":true,"worktree":worktree}))
}

pub(super) fn receipt(params: &Value, state: &State) -> io::Result<Value> {
    let lookup: Lookup =
        serde_json::from_value(params.clone()).map_err(|error| invalid(&error.to_string()))?;

    if !super::super::valid(&lookup.session)
        || !state.known_client(&lookup.client)
        || lookup.request.is_empty()
        || lookup.request.len() > super::super::META_LIMIT
    {
        return Err(invalid("Invalid task receipt lookup."));
    }

    let store = state.sessions.lock().map_err(lock)?;
    let source = super::super::sharing::Principal::Client(lookup.client);
    let session =
        store.sessions.get(&lookup.session).ok_or_else(|| not_found("Conversation not found."))?;

    if !owned(session, &source) {
        return Err(invalid("The client does not own this conversation."));
    }

    let Some(job) = find(&store, &source, &lookup.session, &lookup.request)? else {
        return Ok(match store.retired_request(&source, &lookup.session, &lookup.request) {
            Some(retired) => json!({"found":true,"expired":true,
                "task":{"id":retired.task,"state":retired.state}}),
            None => json!({"found":false}),
        });
    };

    let text = matches!(
        job.status,
        super::super::state::JobStatus::Reporting | super::super::state::JobStatus::Completed
    )
    .then_some(job.result.as_deref())
    .flatten();

    Ok(json!({"found":true,"task":{"id":job.id,"state":job.status},
        "provider":job.provider,"text":text}))
}

pub(super) fn find<'a>(
    store: &'a Store,
    source: &crate::sharing::Principal,
    session: &str,
    request: &str,
) -> io::Result<Option<&'a crate::state::Job>> {
    let mut matches = store.jobs.values().filter(|job| {
        job.origin.session == session
            && &job.origin.source() == source
            && job.request.as_deref() == Some(request)
    });

    let first = matches.next();

    if matches.next().is_some() {
        return Err(invalid(
            "Several saved tasks share this request identity, so inspect task.list.",
        ));
    }

    Ok(first)
}

pub(super) fn replay(
    request: &IpcRequest,
    state: &State,
    session: &str,
    client: &str,
    plugin: &str,
) -> io::Result<Option<Value>> {
    let identity = match request.params.get("request") {
        Some(identity) => identity.clone(),

        None => {
            let store = state.sessions.lock().map_err(lock)?;
            let last = store.sessions.get(session).and_then(|value| {
                value.conversation().next_back().filter(|message| {
                    message.role == Role::User && message.sender.as_deref() == Some(client)
                })
            });

            match last {
                Some(message) => json!(message.id),
                None => return Ok(None),
            }
        }
    };

    let saved = receipt(&json!({"session":session,"client":client,"request":identity}), state)?;

    if saved["found"] != true {
        return Ok(None);
    }

    if saved["expired"] == true {
        return Err(invalid(EXPIRED));
    }

    if saved["provider"] != plugin {
        return Err(invalid("This request was accepted with a different provider."));
    }

    Ok(Some(
        json!({"done":saved["text"].is_string(),"text":saved["text"].as_str().unwrap_or_default(),
        "task":saved["task"],"replayed":true}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use krabb_core::task::Run;

    fn fixture() -> (State, Run) {
        fixture_for(krabb_core::client::TUI)
    }

    fn fixture_for(client: &str) -> (State, Run) {
        let mut state = super::super::tests::state_at("request-recovery");
        state.config.clients.insert(client.into(), Default::default());
        let _ = std::fs::remove_dir_all(&state.home);

        std::fs::create_dir_all(state.home.join("plugins/fixture")).unwrap();
        state.sessions =
            Arc::new(Mutex::new(Store::load(state.home.join("data/sessions/index.json")).unwrap()));
        let mut store = state.sessions.lock().unwrap();
        store.ensure("conversation", "model").unwrap();
        store.client("conversation", client).unwrap();
        store
            .append(
                "conversation",
                Message {
                    id: "request-1".into(),
                    session: "conversation".into(),
                    role: Role::User,
                    sender: Some(client.into()),
                    content: vec![Content::Text { text: "Prepare a verified report.".into() }],
                },
            )
            .unwrap();
        store.reserve("conversation", "owner".into()).unwrap();
        let context = crate::jobs::capture(
            &store,
            "fixture",
            "model",
            &state.root,
            client,
            "conversation",
            None,
            "conversation",
            false,
            false,
        )
        .unwrap();
        let task = crate::tasks::Task {
            kind: crate::tasks::Kind::Fast,
            message: String::new(),
            instruction: "Prepare a verified report.".into(),
        };

        let job = crate::jobs::record(&task, &context).unwrap();
        let history = store.sessions["conversation"].context();
        let run = store.admit_foreground(job, history).unwrap();
        drop(store);
        (state, run)
    }

    fn query() -> Value {
        json!({"session":"conversation","client":krabb_core::client::TUI,
            "request":"request-1"})
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_reader_cannot_hold_final_or_streamed_delivery_open_indefinitely() {
        for streamed in [false, true] {
            let (mut writer, mut reader) = tokio::io::duplex(8);
            let started = tokio::time::Instant::now();
            let error = if streamed {
                super::super::write_stream_notice(
                    &mut writer,
                    7,
                    crate::StreamNotice::Text("answer".repeat(100)),
                )
                .await
                .unwrap_err()
            } else {
                super::super::write_frame(
                    &mut writer,
                    IpcResponse::ok(7, json!({"text":"answer".repeat(100)})),
                )
                .await
                .unwrap_err()
            };

            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert_eq!(started.elapsed(), WRITE_LIMIT);
            drop(writer);
            let mut partial = Vec::new();
            reader.read_to_end(&mut partial).await.unwrap();

            assert!(partial.len() <= 8);
            assert!(serde_json::from_slice::<IpcResponse>(&partial).is_err());
        }
    }

    #[test]
    fn receipt_recovery_is_read_only_private_and_available_without_a_provider() {
        let (mut state, run) = fixture();
        let path = state.home.join("data/sessions/index.json");
        let before = std::fs::read(&path).unwrap();
        let saved = receipt(&query(), &state).unwrap();

        assert_eq!(saved["task"]["id"], run.id);
        assert_eq!(saved["task"]["state"], "running");
        assert!(saved["text"].is_null());
        assert_eq!(receipt(&query(), &state).unwrap(), saved);
        assert_eq!(std::fs::read(&path).unwrap(), before);

        let mut unknown = query();
        unknown["request"] = json!("unknown-request");

        assert_eq!(receipt(&unknown, &state).unwrap(), json!({"found":false}));
        state.config.clients.insert("other".into(), Default::default());
        let mut foreign = query();
        foreign["client"] = json!("other");

        assert!(receipt(&foreign, &state).is_err());
        let mut forged = query();
        forged["owner"] = json!("admin");

        assert!(receipt(&forged, &state).is_err());
        assert_eq!(state.sessions.lock().unwrap().jobs.len(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_replayed_request_is_done_only_once_its_answer_is_final() {
        let (state, run) = fixture();

        let request = IpcRequest::call(
            1,
            "token",
            "session.answer",
            json!({"session":"conversation","client":krabb_core::client::TUI,"plugin":"fixture"}),
        );

        let running = replay(&request, &state, "conversation", krabb_core::client::TUI, "fixture")
            .unwrap()
            .unwrap();

        assert_eq!(running["done"], false);
        assert_eq!(running["replayed"], true);

        {
            let mut store = state.sessions.lock().unwrap();
            let job = store.jobs.get_mut(&run.id).unwrap();

            job.status = crate::state::JobStatus::Completed;
            job.result = Some("The verified report.".into());
        }

        let finished = replay(&request, &state, "conversation", krabb_core::client::TUI, "fixture")
            .unwrap()
            .unwrap();

        assert_eq!(finished["done"], true);
        assert_eq!(finished["text"], "The verified report.");
    }

    #[test]
    fn a_legacy_answer_without_a_request_id_replays_its_saved_message_task() {
        let (state, run) = fixture();

        {
            let mut store = state.sessions.lock().unwrap();
            let job = store.jobs.get_mut(&run.id).unwrap();
            job.status = crate::state::JobStatus::Completed;
            job.result = Some("The verified report.".into());
        }

        let legacy = IpcRequest::call(
            1,
            "token",
            "session.answer",
            json!({"session":"conversation","client":krabb_core::client::TUI,"plugin":"fixture"}),
        );
        let saved = replay(&legacy, &state, "conversation", krabb_core::client::TUI, "fixture")
            .unwrap()
            .unwrap();

        assert_eq!(saved["text"], "The verified report.");
        assert_eq!(saved["replayed"], true);
        assert_eq!(saved["task"]["id"], run.id);

        assert!(replay(&legacy, &state, "conversation", krabb_core::client::TUI, "other").is_err());

        assert!(replay(&legacy, &state, "conversation", "desktop", "fixture").unwrap().is_none());

        state
            .sessions
            .lock()
            .unwrap()
            .push(
                "conversation",
                Message {
                    id: "reply".into(),
                    session: "conversation".into(),
                    role: Role::Assistant,
                    sender: None,
                    content: vec![Content::Text { text: "The verified report.".into() }],
                },
            )
            .unwrap();

        assert!(
            replay(&legacy, &state, "conversation", krabb_core::client::TUI, "fixture")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn an_expired_receipt_is_reported_and_never_admits_the_request_again() {
        let (state, run) = fixture();
        let path = state.home.join("data/sessions/index.json");

        {
            let mut store = state.sessions.lock().unwrap();
            let job = store.jobs.get_mut(&run.id).unwrap();
            job.status = crate::state::JobStatus::Completed;
            job.result = Some("The verified report.".into());
            store.retire_job(&run.id);
            store.sessions.get_mut("conversation").unwrap().messages.clear();
            store.save().unwrap();
        }

        state.sessions.lock().unwrap().clone_from(&Store::load(&path).unwrap());

        let saved = receipt(&query(), &state).unwrap();

        assert_eq!(
            saved,
            json!({"found":true,"expired":true,
            "task":{"id":run.id,"state":"completed"}})
        );

        let answer = IpcRequest::call(1, "token", "session.answer", query());
        let refused = replay(&answer, &state, "conversation", krabb_core::client::TUI, "fixture")
            .unwrap_err();

        assert!(refused.to_string().contains("saved result has expired"));

        let params = json!({"session":"conversation","client":krabb_core::client::TUI,
            "plugin":"fixture","id":"request-1","text":"Prepare a verified report."});
        let queued = super::super::tasks::enqueue(&params, &state).unwrap_err();

        assert!(queued.to_string().contains("saved result has expired"));

        {
            let store = state.sessions.lock().unwrap();

            assert!(store.jobs.is_empty());
            assert_eq!(store.retired.len(), 1);
        }

        let mut other = query();
        other["request"] = json!("request-2");

        assert_eq!(receipt(&other, &state).unwrap(), json!({"found":false}));
    }

    #[test]
    fn a_configured_client_recovers_its_own_task_without_tui_aliases() {
        let (state, run) = fixture_for("desktop");
        let mut query = query();
        query["client"] = json!("desktop");

        assert_eq!(receipt(&query, &state).unwrap()["task"]["id"], run.id);
    }

    #[test]
    fn repeating_a_saved_user_message_is_idempotent_and_changed_content_is_rejected() {
        let (state, _) = fixture();
        let mut store = state.sessions.lock().unwrap();
        let message = store.sessions["conversation"].messages[0].clone();
        let path = state.home.join("data/sessions/index.json");
        let before = std::fs::read(&path).unwrap();
        store.append_reserved("conversation", "owner", message.clone()).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), before);

        let mut changed = message.clone();
        changed.content = vec![Content::Text { text: "A different request.".into() }];

        assert_eq!(
            store.append_reserved("conversation", "owner", changed).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );

        assert_eq!(store.sessions["conversation"].messages, vec![message]);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(store.jobs.len(), 1);
    }

    #[test]
    fn queued_request_replay_uses_its_fingerprint_after_both_histories_have_been_trimmed() {
        let (state, run) = fixture();
        {
            let mut store = state.sessions.lock().unwrap();
            store.sessions.get_mut("conversation").unwrap().messages.clear();
            store.sessions.get_mut(&run.id).unwrap().messages.clear();
            store.save().unwrap();
        }

        let params = json!({"session":"conversation","client":krabb_core::client::TUI,
            "plugin":"fixture","id":"request-1","text":"Prepare a verified report."});
        let recovered = super::super::tasks::enqueue(&params, &state).unwrap();

        assert_eq!(recovered["task"], run.id);
        assert_eq!(recovered["replayed"], true);
        let mut changed = params;
        changed["text"] = json!("Replace it with a different task.");

        assert!(super::super::tasks::enqueue(&changed, &state).is_err());
        assert_eq!(state.sessions.lock().unwrap().jobs.len(), 1);
        assert!(state.sessions.lock().unwrap().sessions["conversation"].messages.is_empty());
    }

    #[tokio::test]
    async fn a_repeated_answer_recovers_the_saved_result_after_restart_with_the_provider_absent() {
        let (mut state, run) = fixture();
        state
            .sessions
            .lock()
            .unwrap()
            .foreground_result(&run, "Verified report, exactly once.")
            .unwrap();
        state.sessions =
            Arc::new(Mutex::new(Store::load(state.home.join("data/sessions/index.json")).unwrap()));
        let request = IpcRequest::call(
            7,
            "secret",
            "session.answer",
            json!({
            "id":"conversation","client":krabb_core::client::TUI,
            "plugin":"fixture","request":"request-1"}),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        let (socket, _) = listener.accept().await.unwrap();
        let (_, mut output) = socket.into_split();

        super::super::session_answer(&request, &state, &mut output).await.unwrap();
        let response: IpcResponse =
            jsonl::read(&mut BufReader::new(client), FRAME).await.unwrap().unwrap();
        let response = response.result.unwrap();

        assert_eq!(response["replayed"], true);
        assert_eq!(response["task"]["id"], run.id);
        assert_eq!(response["text"], "Verified report, exactly once.");
        assert_eq!(state.sessions.lock().unwrap().jobs.len(), 1);

        let mut changed = request;
        changed.params["plugin"] = json!("another-provider");

        assert!(
            replay(&changed, &state, "conversation", krabb_core::client::TUI, "another-provider")
                .is_err()
        );
    }

    #[test]
    fn an_already_admitted_request_cannot_create_a_second_task_with_a_fresh_job_id() {
        let (state, run) = fixture();
        let mut store = state.sessions.lock().unwrap();
        let mut another = store.jobs[&run.id].clone();
        another.id = "job-999999".into();
        let history = store.sessions["conversation"].context();

        let error = store.admit_foreground(another, history).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(store.jobs.len(), 1);
        assert!(!store.sessions.contains_key("job-999999"));
    }

    #[test]
    fn legacy_duplicate_identities_require_review_instead_of_selecting_an_arbitrary_result() {
        let (state, run) = fixture();
        {
            let mut store = state.sessions.lock().unwrap();
            let mut duplicate = store.jobs[&run.id].clone();
            duplicate.id = "job-999999".into();
            store.jobs.insert(duplicate.id.clone(), duplicate);
        }

        assert!(receipt(&query(), &state).is_err());
        assert!(
            super::super::tasks::enqueue(
                &json!({"session":"conversation",
            "client":krabb_core::client::TUI,"plugin":"fixture","id":"request-1",
            "text":"Prepare a verified report."}),
                &state
            )
            .is_err()
        );

        assert_eq!(state.sessions.lock().unwrap().jobs.len(), 2);
    }

    #[test]
    fn temporary_cleanup_retains_unfinished_and_uncertain_work_then_removes_idle_conversations() {
        let (state, run) = fixture();
        let params = json!({"id":"conversation"});

        assert_eq!(cleanup(&params, &state).unwrap()["deleted"], false);
        {
            let mut store = state.sessions.lock().unwrap();
            store.set_status("conversation", "idle").unwrap();
            let job = store.jobs.get_mut(&run.id).unwrap();
            job.status = crate::state::JobStatus::Cancelled;
            job.uncertain = true;

            assert!(store.remove("conversation").is_err());
        }

        assert_eq!(cleanup(&params, &state).unwrap()["deleted"], false);
        assert!(state.sessions.lock().unwrap().sessions.contains_key("conversation"));
        {
            let mut store = state.sessions.lock().unwrap();
            store.jobs.get_mut(&run.id).unwrap().uncertain = false;
            store
                .queue(
                    "conversation",
                    Message {
                        id: "unadmitted-input".into(),
                        session: "conversation".into(),
                        role: Role::User,
                        sender: Some("tui".into()),
                        content: vec![Content::Text { text: "A second request.".into() }],
                    },
                )
                .unwrap();
        }

        assert_eq!(cleanup(&params, &state).unwrap()["deleted"], false);
        state.sessions.lock().unwrap().sessions.get_mut("conversation").unwrap().queued.clear();

        assert_eq!(cleanup(&params, &state).unwrap()["deleted"], true);
        assert!(!state.sessions.lock().unwrap().sessions.contains_key("conversation"));
    }

    #[tokio::test]
    async fn receipt_lookup_is_authenticated_and_bypasses_busy_work_and_update_locks() {
        let (mut state, run) = fixture();
        state.slots = Arc::new(Semaphore::new(1));
        let busy = Arc::clone(&state.slots).acquire_owned().await.unwrap();
        let updating = Arc::clone(&state.plugins.lifecycle).write_owned().await;
        let state = Arc::new(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(serve(listener, Arc::clone(&state)));

        for token in ["invalid", "secret"] {
            let mut socket = TcpStream::connect(address).await.unwrap();
            jsonl::write(&mut socket, &IpcRequest::call(7, token, "task.receipt", query()))
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
            } else {
                assert_eq!(response.result.unwrap()["task"]["id"], run.id);
            }
        }

        assert_eq!(state.slots.available_permits(), 0);
        state.stop.signal();
        timeout(Duration::from_secs(1), server).await.unwrap().unwrap().unwrap();
        drop(updating);
        drop(busy);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_lost_foreground_reply_recovers_one_accepted_task_and_exact_result() {
        let state = super::super::tests::pending_tui_provider("stream").await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        let (socket, _) = listener.accept().await.unwrap();
        let (_, mut output) = socket.into_split();
        let answering = Arc::clone(&state);
        let request = IpcRequest::call(
            7,
            "secret",
            "session.answer",
            json!({
            "id":"turn","plugin":"fixture","request":"user-0"}),
        );
        let original = request.clone();
        let generation = tokio::spawn(async move {
            super::super::session_answer(&original, &answering, &mut output).await
        });

        let mut reader = BufReader::new(client);
        let _: IpcResponse = jsonl::read(&mut reader, FRAME).await.unwrap().unwrap();
        super::super::tests::wait_for_tui_request(&state, 0).await;
        let lookup = json!({"session":"turn","client":krabb_core::client::TUI,"request":"user-0"});
        let running = receipt(&lookup, &state).unwrap();

        assert_eq!(running["found"], true);
        assert_eq!(running["task"]["state"], "running");
        let duplicate =
            replay(&request, &state, "turn", krabb_core::client::TUI, "fixture").unwrap().unwrap();
        assert_eq!(duplicate["task"], running["task"]);
        drop(reader);
        std::fs::write(state.home.join("release-0"), "").unwrap();
        let _ = timeout(Duration::from_secs(5), generation).await.unwrap().unwrap();

        let saved = receipt(&lookup, &state).unwrap();

        assert_eq!(saved["task"]["id"], running["task"]["id"]);
        assert_eq!(saved["task"]["state"], "completed");
        assert_eq!(saved["text"], "fresh answer");
        {
            let store = state.sessions.lock().unwrap();

            assert_eq!(store.jobs.len(), 1);
            assert_eq!(store.sessions["turn"].status, "idle");
            assert!(!store.sessions["turn"].reservation_active);
            assert_eq!(
                store.sessions["turn"]
                    .messages
                    .iter()
                    .filter(|message| {
                        message.role == Role::Assistant
                            && message.content[0].render() == "fresh answer"
                    })
                    .count(),
                1
            );
        }

        assert!(!state.home.join("started-1").exists());
        state.plugins.get("fixture").await.unwrap().stop().await.unwrap();
        let duplicate =
            replay(&request, &state, "turn", krabb_core::client::TUI, "fixture").unwrap().unwrap();
        assert_eq!(duplicate["text"], "fresh answer");
        assert_eq!(duplicate["task"], saved["task"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_aborted_connection_releases_its_lease_without_releasing_a_later_owner() {
        let state = super::super::tests::pending_tui_provider("stream").await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        let (socket, _) = listener.accept().await.unwrap();
        let (_, mut output) = socket.into_split();
        let answering = Arc::clone(&state);
        let generation = tokio::spawn(async move {
            super::super::session_answer(
                &IpcRequest::call(
                    7,
                    "secret",
                    "session.answer",
                    json!({"id":"turn","plugin":"fixture","request":"user-0"}),
                ),
                &answering,
                &mut output,
            )
            .await
        });

        super::super::tests::wait_for_tui_request(&state, 0).await;
        generation.abort();

        assert!(generation.await.unwrap_err().is_cancelled());
        {
            let mut store = state.sessions.lock().unwrap();

            assert_eq!(store.sessions["turn"].status, "idle");
            assert!(!store.sessions["turn"].reservation_active);
            assert!(store.sessions["turn"].reservation_owner.is_none());
            assert_eq!(store.jobs.len(), 1);
            store.reserve("turn", "later-owner".into()).unwrap();
        }

        let stale = super::super::Reservation {
            sessions: &state.sessions,
            id: "turn",
            owner: "owner".into(),
            wake: &state.plugins.jobs.wake,
            release: true,
        };

        drop(stale);

        assert_eq!(
            state.sessions.lock().unwrap().sessions["turn"].reservation_owner.as_deref(),
            Some("later-owner")
        );
        std::fs::write(state.home.join("release-0"), "").unwrap();
        state.plugins.get("fixture").await.unwrap().stop().await.unwrap();

        assert!(!state.home.join("started-1").exists());
    }
}
