use super::*;
use std::fs;

#[test]
fn reload_validates_configuration_and_preserves_the_previous_selection_on_failure() {
    let state = super::tests::state_at("selection-validation");

    fs::create_dir_all(&state.home).unwrap();

    let path = state.home.join("config.toml");
    let request = IpcRequest::call(1, "secret", "config.selection.reload", json!({}));

    fs::write(&path, "provider='first'\nmodel='global'\n[channels.telegram]\nmodel='chat'")
        .unwrap();
    dispatch(&request, &state).unwrap();

    assert_eq!(
        state.plugins.channel_selection("telegram", "old", "old").unwrap(),
        ("first".into(), "chat".into())
    );

    assert_eq!(
        state.plugins.channel_selection("discord", "old", "old").unwrap(),
        ("first".into(), "global".into())
    );

    for invalid in ["model=''", "broken = ["] {
        fs::write(&path, invalid).unwrap();

        assert!(dispatch(&request, &state).is_err());

        assert_eq!(
            state.plugins.channel_selection("telegram", "old", "old").unwrap(),
            ("first".into(), "chat".into())
        );
    }

    fs::remove_dir_all(&state.home).unwrap();
}

async fn set(state: &State, assignment: &str) {
    crate::config_command(
        crate::ConfigCommand::Set(crate::ConfigSet { assignment: assignment.into(), force: false }),
        &state.home,
        true,
    )
    .await
    .unwrap();
}

async fn turn(state: &State, number: usize, expected: &str) {
    fs::write(state.home.join("next"), number.to_string()).unwrap();

    timeout(Duration::from_secs(5), async {
        loop {
            let complete =
                state.sessions.lock().unwrap().sessions.get("telegram-7").is_some_and(|session| {
                    session.status == "idle" && session.messages.len() == number * 2
                });

            if complete {
                break;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let sessions = state.sessions.lock().unwrap();
    let session = &sessions.sessions["telegram-7"];

    assert_eq!(session.messages.last().unwrap().content[0].render(), expected);
}

#[tokio::test]
async fn config_changes_reach_existing_channel_turns_without_restarting() {
    let mut state = super::tests::state_at("selection-live");

    crate::init_at(&state.home).unwrap();
    state.sessions = Arc::new(Mutex::new(Store::load(state.home.join("sessions.json")).unwrap()));

    fs::write(
        state.home.join("config.toml"),
        "[channels.telegram]\nprovider='first'\nmodel='old'\n",
    )
    .unwrap();

    let script = r#"
import json, pathlib, sys, time
role, root = sys.argv[1:]
root = pathlib.Path(root)
last = 0
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': role,
                  'version': '0.1.0',
                  'capabilities': ['channel' if role == 'telegram' else 'model']}
    elif method == 'poll':
        path = root / 'next'
        number = int(path.read_text() or '0') if path.exists() else 0
        events = []
        if number > last:
            last = number
            events = [{'id': number, 'chat': 7, 'private': True, 'sender': 8, 'text': 'ping'}]
        else:
            time.sleep(0.01)
        result = {'events': events}
    elif method == 'generate':
        result = {'text': role + ':' + request['params']['model'], 'stop': 'stop'}
    elif method == 'shutdown':
        break
    else:
        result = {'message_id': request['id'], 'acknowledged': True}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

    for role in ["telegram", "first", "second"] {
        let process = crate::process::Process::start_with(
            "python3",
            ["-u", "-c", script, role, state.home.to_str().unwrap()],
        )
        .await
        .unwrap();

        state.plugins.insert(crate::Live::new(process)).await;
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    crate::secure(state.home.join("run/ipc.token"), state.token.as_bytes()).unwrap();

    crate::secure(
        state.home.join("run/ipc.port"),
        listener.local_addr().unwrap().port().to_string().as_bytes(),
    )
    .unwrap();

    let state = Arc::new(state);
    let server = tokio::spawn(super::serve(listener, Arc::clone(&state)));
    let worker = Arc::clone(&state);

    let bridge = tokio::spawn(async move {
        crate::bridge_with_media(
            &worker.plugins,
            "telegram",
            "first",
            "old",
            Arc::clone(&worker.sessions),
            Arc::clone(&worker.stop),
            Arc::clone(&worker.cancels),
            BTreeMap::new(),
            crate::ApprovalMode::Off,
            false,
            worker.home.clone(),
            worker.home.join("workspace"),
            worker.home.join("media"),
            Arc::clone(&worker.pending),
            10,
        )
        .await
    });

    turn(&state, 1, "first:old").await;
    set(&state, "channels.telegram.model='new'").await;
    turn(&state, 2, "first:new").await;
    set(&state, "channels.telegram.provider='second'").await;
    turn(&state, 3, "second:new").await;

    super::call(&state.home, "session.model", json!({"id":"telegram-7", "model":"chosen"}))
        .await
        .unwrap();

    set(&state, "channels.telegram.model='latest'").await;
    turn(&state, 4, "second:chosen").await;
    super::call(&state.home, "session.model", json!({"id":"telegram-7", "model":"inherit"}))
        .await
        .unwrap();

    turn(&state, 5, "second:latest").await;

    state.stop.signal();
    timeout(Duration::from_secs(5), bridge).await.unwrap().unwrap().unwrap();
    timeout(Duration::from_secs(5), server).await.unwrap().unwrap().unwrap();
    crate::tests::stop_registry(&state.plugins).await;
    fs::remove_dir_all(&state.home).unwrap();
}
