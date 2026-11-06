use crate::*;
use std::fs;

#[tokio::test]
async fn model_retry_requests_respect_successful_mutations() {
    scheduled_replies(false).await;
}

#[tokio::test]
async fn final_response_save_failures_preserve_mutation_markers() {
    scheduled_replies(true).await;
}

async fn scheduled_replies(full: bool) {
    let script = r#"
import json, pathlib, sys
role, root, mode, tool, final = sys.argv[1:]
root = pathlib.Path(root)
called = False
for line in sys.stdin:
    request = json.loads(line)
    if request.get('method') == 'visible':
        request['method'], request['params'] = request['params']['method'], request['params']['params']
    method = request.get('method')
    if method == 'hello':
        capability = {'fixture': 'model', 'tools': 'tool', 'scheduler': 'scheduler'}[role]
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': role,
                  'version': '0.1.0',
                  'capabilities': [capability]}
    elif method == 'generate':
        if not called:
            called = True
            args = {'path': 'note.txt'}
            if tool == 'write':
                args['text'] = 'one authorized write'
            if mode == 'host':
                print(json.dumps({'jsonrpc': '2.0', 'id': 77, 'method': 'host/tool',
                                  'params': {'name': tool, 'args': args}}), flush=True)
                response = json.loads(sys.stdin.readline())
                assert 'error' not in response
                result = {'text': final, 'stop': 'stop'}
            else:
                result = {'text': '', 'stop': 'tool_calls', 'events': [
                    {'kind': 'tool', 'name': tool, 'args': args}]}
        else:
            result = {'text': final, 'stop': 'stop'}
    elif method == 'tools/list':
        result = {'tools': [{'name': name, 'description': 'Local fixture', 'schema': {
            'type': 'object',
            'properties': {'path': {'type': 'string'}, 'text': {'type': 'string'}},
            'required': ['path'], 'additionalProperties': False}} for name in ['read', 'write']]}
    elif method in ('read', 'write'):
        params = request['params']
        assert pathlib.Path(params['workspace']) == root
        if method == 'write':
            assert params['approve'] is True
            with (root / 'mutations').open('a') as output:
                output.write('write\n')
            (root / params['path']).write_text(params['text'])
        result = {'text': (root / params['path']).read_text()}
    elif method == 'finish':
        params = request['params']
        (root / 'finish.json').write_text(json.dumps(params))
        result = {'status': 'completed' if params['success'] else 'paused'}
    elif method == 'shutdown':
        break
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

    let large_reply = format!("DONE: {}", "x".repeat(16 * 1024));

    let cases = if full {
        vec![("write", large_reply.as_str(), false, true), ("read", &large_reply, false, false)]
    } else {
        vec![
            ("write", "RETRYABLE: Please run again.", false, true),
            ("write", " \nRETRYABLE: Please run again.", false, true),
            ("write", "DONE: Saved the file.", true, false),
            ("read", "RETRYABLE: Please run again.", false, false),
            ("read", "DONE: Read the file.", true, false),
        ]
    };

    for mode in ["event", "host"] {
        for &(tool, final_text, success, pause) in &cases {
            let root = tests::test_root("schedule-model-retry");

            fs::create_dir(&root).unwrap();

            let root = root.canonicalize().unwrap();

            fs::write(root.join("note.txt"), "original").unwrap();

            let plugins = Plugins::default();

            for role in ["fixture", "tools", "scheduler"] {
                let process = Process::start_with(
                    "python3",
                    ["-u", "-c", script, role, root.to_str().unwrap(), mode, tool, final_text],
                )
                .await
                .unwrap();

                plugins.insert(Live::new(process)).await;
            }

            let path = root.join("state/sessions/index.json");
            let mut store = state::Store::load(&path).unwrap();

            if full {
                // Exercise the production size check without rewriting 32 MiB on every phase.
                store.record_limit = Some(32 * 1024);
                store.ensure_schedule("schedule-71", "test").unwrap();
                store
                    .append(
                        "schedule-71",
                        Message {
                            id: "history".into(),
                            session: "schedule-71".into(),
                            role: Role::Assistant,
                            sender: None,
                            content: vec![Content::Text { text: "h".repeat(24 * 1024) }],
                        },
                    )
                    .unwrap();
            }

            let sessions = Arc::new(Mutex::new(store));

            let task = serde_json::json!({
                "id": 71, "run_id": 1, "text": "Perform the local fixture action.", "retries": 3,
                "context": {"tools": true, "shell": false}
            });

            tokio::time::timeout(
                Duration::from_secs(30),
                run_scheduled_task(
                    71,
                    &task,
                    &plugins,
                    &sessions,
                    &Arc::new(Stop::new()),
                    &root,
                    &Config::default(),
                    &root,
                    &root.join("media"),
                    "fixture",
                    "test",
                    Arc::new(AsyncMutex::new(approval::Gate::new().unwrap())),
                    &ScheduledWorkspaceLocks::default(),
                    1,
                ),
            )
            .await
            .unwrap();

            let finish: Value =
                serde_json::from_str(&fs::read_to_string(root.join("finish.json")).unwrap())
                    .unwrap();

            assert_eq!(finish["success"], success, "{mode} {tool}");
            assert_eq!(finish["pause"], pause, "{mode} {tool}");
            assert_eq!(finish["cancelled"], false);
            assert_eq!(finish["run_id"], 1);

            if full {
                assert!(finish["result"].as_str().unwrap().contains("exceeds the size limit"));

                let store = sessions.lock().unwrap();
                let session = &store.sessions["schedule-71"];

                assert_eq!(session.phase, if tool == "write" { "unsafe" } else { "safe" });

                assert!(session.inflight.is_some());

                assert!(
                    !session
                        .messages
                        .iter()
                        .any(|message| message.id.starts_with("schedule-assistant-"))
                );

                drop(store);

                let recovered = state::Store::load(&path).unwrap();
                let session = &recovered.sessions["schedule-71"];

                assert_eq!(session.status, "interrupted");
                assert_eq!(session.queued.len(), usize::from(tool == "read"));
            } else {
                let store = sessions.lock().unwrap();
                let session = &store.sessions["schedule-71"];

                assert_eq!(session.phase, "safe");
                assert!(session.inflight.is_none());

                assert!(
                    session
                        .messages
                        .iter()
                        .any(|message| message.id.starts_with("schedule-assistant-"))
                );
            }

            if tool == "write" {
                assert_eq!(
                    fs::read_to_string(root.join("mutations")).unwrap(),
                    "write\n",
                    "{mode} {tool} at {}",
                    root.display()
                );

                assert_eq!(
                    fs::read_to_string(root.join("note.txt")).unwrap(),
                    "one authorized write"
                );

                if pause {
                    assert!(
                        finish["result"].as_str().unwrap().contains("review it before retrying")
                    );
                }
            } else {
                assert!(!root.join("mutations").exists());
                assert_eq!(fs::read_to_string(root.join("note.txt")).unwrap(), "original");
            }

            tests::stop_registry(&plugins).await;
            fs::remove_dir_all(root).unwrap();
        }
    }
}
