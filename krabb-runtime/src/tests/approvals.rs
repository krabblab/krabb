use crate::*;
use std::fs;

#[tokio::test]
async fn command_approvals_require_complete_arguments() {
    let root = std::env::temp_dir().join(format!(
        "krabb-approval-display-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));

    fs::create_dir_all(&root).unwrap();

    let script = r#"
import json, pathlib, subprocess, sys
root = pathlib.Path(sys.argv[1])
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': 'tools',
                  'version': '0.1.0', 'capabilities': ['tool']}
    elif method in ('shell', 'git'):
        params = request['params']
        assert params['approve'] is True
        (root / 'dispatch.json').write_text(json.dumps(params))
        if method == 'shell':
            subprocess.run(params['command'], shell=True, cwd=root, timeout=1, check=True)
        result = {'text': 'executed'}
    elif method == 'shutdown':
        result = {'ok': True}
    else:
        raise AssertionError(method)
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
    if method == 'shutdown':
        break
"#;

    let process =
        Process::start_with("python3", ["-u", "-c", script, root.to_str().unwrap()]).await.unwrap();

    let plugins = Plugins::default();

    plugins.insert(Live::new(process)).await;

    let sessions = Arc::new(Mutex::new(state::Store::default()));

    sessions.lock().unwrap().ensure("session", "test").unwrap();
    sessions.lock().unwrap().reserve("session", "test-turn".into()).unwrap();

    let approvals = Arc::new(AsyncMutex::new(approval::Gate::new().unwrap()));
    let suffix = "; printf approved > sentinel";
    let long = serde_json::json!({"command": format!(": {}{suffix}", " ".repeat(1100))});

    let boundary = |size: usize| {
        let padding =
            size - serde_json::json!({"command": format!(": {suffix}")}).to_string().len();
        serde_json::json!({"command": format!(": {}{suffix}", " ".repeat(padding))})
    };

    let short = serde_json::json!({"command": format!(": {suffix}")});
    let escaped = serde_json::json!({"command": format!(": '{}' {suffix}", "\\".repeat(260))});
    let unicode = serde_json::json!({"command": format!(": '{}' {suffix}", "é".repeat(260))});
    let git = serde_json::json!({"args": ["status", " ".repeat(1100)]});

    let git_boundary = |size: usize| {
        let overhead = serde_json::json!({"args": ["status", ""]}).to_string().len();
        serde_json::json!({"args": ["status", "x".repeat(size - overhead)]})
    };

    for channel in ["tui", "telegram", "discord"] {
        for (name, args, mode, enabled, decision, rejected) in [
            ("shell", long.clone(), ApprovalMode::Prompt, true, true, true),
            ("shell", boundary(513), ApprovalMode::Prompt, true, true, true),
            ("shell", escaped.clone(), ApprovalMode::Prompt, true, true, true),
            ("shell", unicode.clone(), ApprovalMode::Prompt, true, true, true),
            ("git", git.clone(), ApprovalMode::Prompt, true, true, true),
            ("git", git_boundary(513), ApprovalMode::Prompt, true, true, true),
            ("shell", boundary(512), ApprovalMode::Prompt, true, true, false),
            ("git", git_boundary(512), ApprovalMode::Prompt, true, true, false),
            ("shell", short.clone(), ApprovalMode::Prompt, true, false, false),
            ("shell", short.clone(), ApprovalMode::Prompt, false, true, true),
            ("shell", short.clone(), ApprovalMode::Off, true, true, true),
            ("shell", long.clone(), ApprovalMode::Auto, true, true, false),
        ] {
            let (notices, mut events) = tokio::sync::mpsc::channel(1);
            let mut failed = None;
            let operation = execute_tool(
                &plugins,
                1,
                name,
                args.clone(),
                "fixture",
                "test",
                mode,
                approvals.clone(),
                channel,
                "7",
                None,
                "session",
                &sessions,
                Some(&root),
                true,
                false,
                enabled,
                notices,
                tokio::time::Instant::now() + Duration::from_secs(5),
                &mut failed,
                None,
                None,
                0,
            );
            let review = async {
                let Some(StreamNotice::Approval {
                    arguments, command, text, approve, deny, ..
                }) = events.recv().await
                else {
                    return false;
                };

                assert!(!root.join("dispatch.json").exists());
                let mut gate = approvals.lock().await;
                let pending = gate.list().unwrap();

                assert_eq!(pending[0].target.args, args);
                assert_eq!(
                    gate.resolve(if decision { &approve } else { &deny }, channel, "7", None, true),
                    Some(decision)
                );

                if !rejected {
                    assert_eq!(arguments, args.to_string());

                    if name == "shell" {
                        assert_eq!(command.as_deref(), args["command"].as_str());
                        assert!(text.contains(args["command"].as_str().unwrap()));
                    } else {
                        assert!(text.contains(&args["args"].to_string()));
                    }
                }

                true
            };

            let (result, prompted) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(operation, review)
            })
            .await
            .unwrap();
            let dispatched = root.join("dispatch.json").exists();
            let sentinel = root.join("sentinel").exists();

            if dispatched {
                let mut received: Value =
                    serde_json::from_str(&fs::read_to_string(root.join("dispatch.json")).unwrap())
                        .unwrap();
                received.as_object_mut().unwrap().remove("approve");
                received.as_object_mut().unwrap().remove("workspace");

                assert_eq!(received, args);
                fs::remove_file(root.join("dispatch.json")).unwrap();
            }

            if sentinel {
                fs::remove_file(root.join("sentinel")).unwrap();
            }

            assert_eq!(dispatched, !rejected && decision, "{channel}: {name}: {result:?}");
            assert_eq!(sentinel, !rejected && decision && name == "shell");
            assert_eq!(prompted, !rejected && mode == ApprovalMode::Prompt);
            assert!(!approvals.lock().await.has_pending());

            if rejected {
                let error = result.unwrap_err().to_string();

                if enabled && mode == ApprovalMode::Prompt {
                    assert!(error.contains("approval display limit"), "{error}");
                } else if !enabled {
                    assert!(error.contains("not authorized"), "{error}");
                } else {
                    assert!(error.contains("blocks mutating tools"), "{error}");
                }
            } else if decision {
                assert_eq!(result.unwrap(), "executed");
            } else {
                assert!(result.unwrap().contains("denied"));
            }
        }
    }

    for (_, plugin) in plugins.all().await {
        plugin.stop().await.unwrap();
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn approval_text_discloses_complete_mutation_arguments() {
    for (name, args) in [
        (
            "browser_navigate",
            serde_json::json!({"session_id": "owned", "url": "https://example.invalid/first"}),
        ),
        (
            "browser_click",
            serde_json::json!({"session_id": "owned", "by": "css", "target": "#confirm"}),
        ),
        (
            "browser_fill",
            serde_json::json!({"session_id": "owned", "by": "css", "target": "#name", "value": "new value"}),
        ),
        ("computer_type", serde_json::json!({"session_id": "owned", "text": "desktop value"})),
        ("write", serde_json::json!({"path": "note.txt", "text": "replacement"})),
        ("patch", serde_json::json!({"text": "fixture patch"})),
        ("memory_remember", serde_json::json!({"key": "note", "value": "remembered"})),
        ("data", serde_json::json!({"operation": "create_database", "database": "fixture"})),
        ("schedule_cancel", serde_json::json!({"id": 42})),
        ("custom_mutation", serde_json::json!({"value": "first"})),
        (
            "schedule_propose",
            serde_json::json!({
                "instruction": format!("{} then write a local report", "Read notes. ".repeat(55)),
                "delay_seconds": 10,
                "timezone": "UTC",
                "context": {"tools": true, "shell": false}
            }),
        ),
    ] {
        let text = approval_text(name, &args);

        assert!(text.contains(&args.to_string()), "Incomplete {name} approval: {text}");

        let mut other = args.clone();

        other["different"] = serde_json::json!("second");

        assert_ne!(text, approval_text(name, &other));
    }
}

#[tokio::test]
async fn mutation_approvals_reject_hidden_arguments_before_dispatch() {
    let root = std::env::temp_dir().join(format!(
        "krabb-mutation-review-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));

    fs::create_dir_all(&root).unwrap();

    let script = r#"
import json, pathlib, sys
root, plugin = pathlib.Path(sys.argv[1]), sys.argv[2]
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': plugin,
                  'version': '0.1.0', 'capabilities': [{'scheduler': 'scheduler', 'sqlite': 'store'}.get(plugin, 'tool')]}
    elif method == 'shutdown':
        result = {'ok': True}
    else:
        (root / 'dispatch.json').write_text(json.dumps(request))
        result = {'text': 'executed', 'id': 1, 'due': 1790000000, 'zone': 'UTC', 'status': 'active'}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
    if method == 'shutdown':
        break
"#;

    let plugins = Plugins::default();

    for plugin in ["scheduler", "fixture", "sqlite"] {
        let process =
            Process::start_with("python3", ["-u", "-c", script, root.to_str().unwrap(), plugin])
                .await
                .unwrap();

        plugins.insert(Live::new(process)).await;
    }

    let sessions = Arc::new(Mutex::new(state::Store::default()));

    sessions.lock().unwrap().ensure("session", "test").unwrap();
    sessions.lock().unwrap().reserve("session", "test-turn".into()).unwrap();

    let approvals = Arc::new(AsyncMutex::new(approval::Gate::new().unwrap()));
    let prefix = "Read the notes. ".repeat(38)[..600].to_owned();

    assert_eq!(prefix.len(), 600);

    let mut cases = vec![];

    for suffix in [" Summarize them.", " Write a local report."] {
        cases.push((
            "schedule_propose",
            serde_json::json!({"instruction": format!("{prefix}{}{suffix}", " ".repeat(600)), "delay_seconds": 10}),
            true,
        ));
    }

    cases.push((
        "schedule_propose",
        serde_json::json!({
            "instruction": "Read the notes and write a report", "delay_seconds": 10,
            "message_destination": {"channel": "telegram", "chat": "7", "private": true},
            "context": {"shell": true}
        }),
        false,
    ));

    for (name, args, field) in [
        (
            "browser_navigate",
            serde_json::json!({"session_id": "owned", "url": "https://example.invalid/first"}),
            "url",
        ),
        (
            "browser_click",
            serde_json::json!({"session_id": "owned", "by": "css", "target": "#first"}),
            "target",
        ),
        (
            "browser_fill",
            serde_json::json!({"session_id": "owned", "by": "css", "target": "#name", "value": "first"}),
            "value",
        ),
        ("computer_type", serde_json::json!({"session_id": "owned", "text": "first"}), "text"),
        ("write", serde_json::json!({"path": "note.txt", "text": "first"}), "text"),
        (
            "data",
            serde_json::json!({"operation": "create_database", "database": "first"}),
            "database",
        ),
        ("custom_mutation", serde_json::json!({"value": "first"}), "value"),
    ] {
        cases.push((name, args.clone(), false));

        let mut other = args.clone();

        other[field] = serde_json::json!(format!("{}second", args[field].as_str().unwrap()));
        cases.push((name, other, false));

        for size in [512, 513] {
            let mut boundary = args.clone();
            let overhead = boundary.to_string().len();

            boundary[field] = serde_json::json!(format!(
                "{}{}",
                args[field].as_str().unwrap(),
                "x".repeat(size - overhead)
            ));

            cases.push((name, boundary, size > 512));
        }

        for content in ["é".repeat(260), "\\".repeat(260)] {
            let mut other = args.clone();

            other[field] = serde_json::json!(content);
            cases.push((name, other, true));
        }
    }

    for channel in ["tui", "telegram", "discord"] {
        let mut channel_cases = cases.clone();

        for size in [1_024, 1_025] {
            let mut args = serde_json::json!({
                "instruction": "", "delay_seconds": 10,
                "context": {
                    "provider": "fixture", "model": "test", "workspace": root,
                    "origin": {"channel": channel, "chat": "7", "thread": null,
                               "session": "session", "private": true},
                    "message_destination": null, "tools": true, "shell": false
                }
            });

            args["instruction"] = serde_json::json!("x".repeat(size - args.to_string().len()));

            assert_eq!(args.to_string().len(), size);

            channel_cases.push(("schedule_propose", args, size > 1_024));
        }

        for (name, args, oversized) in &channel_cases {
            for (mode, decision) in [
                (ApprovalMode::Prompt, true),
                (ApprovalMode::Prompt, false),
                (ApprovalMode::Off, true),
                (ApprovalMode::Auto, true),
            ] {
                sessions.lock().unwrap().phase("session", "safe").unwrap();
                let mut registry = legacy_tool_registry(Some(name));
                registry.owners.insert((*name).into(), "fixture".into());
                let (notices, mut events) = tokio::sync::mpsc::channel(1);
                let mut failed = None;

                let operation = execute_tool_with_registry(
                    &plugins,
                    &registry,
                    "test-turn",
                    1,
                    name,
                    args.clone(),
                    "fixture",
                    "test",
                    mode,
                    approvals.clone(),
                    channel,
                    "7",
                    None,
                    "session",
                    &sessions,
                    Some(&root),
                    true,
                    true,
                    false,
                    notices,
                    tokio::time::Instant::now() + Duration::from_secs(5),
                    &mut failed,
                    None,
                    None,
                    0,
                );

                let review = async {
                    let Some(StreamNotice::Approval { arguments, text, approve, deny, .. }) =
                        events.recv().await
                    else {
                        return false;
                    };

                    assert!(!root.join("dispatch.json").exists());
                    let mut gate = approvals.lock().await;
                    let pending = gate.list().unwrap();
                    let target = &pending[0].target.args;

                    assert_eq!(arguments, target.to_string());
                    assert!(text.contains(&target.to_string()), "{name}: {text}");
                    assert!(text.len() <= 2_000);

                    if *name == "schedule_propose" {
                        assert_eq!(target["instruction"], args["instruction"]);
                        assert_eq!(target["context"]["shell"], false);
                        assert_eq!(target["context"]["tools"], true);
                    }

                    if channel != "tui" {
                        let params =
                            approval_params(channel, "7", None, &text, &approve, &deny).unwrap();
                        assert_eq!(params["text"], text);
                    }

                    assert_eq!(
                        gate.resolve(
                            if decision { &approve } else { &deny },
                            channel,
                            "7",
                            None,
                            true
                        ),
                        Some(decision)
                    );
                    true
                };

                let (result, prompted) = tokio::time::timeout(Duration::from_secs(5), async {
                    tokio::join!(operation, review)
                })
                .await
                .unwrap();
                let rejected =
                    mode == ApprovalMode::Off || (mode == ApprovalMode::Prompt && *oversized);
                let dispatched = root.join("dispatch.json").exists();

                assert_eq!(prompted, mode == ApprovalMode::Prompt && !oversized);
                assert_eq!(
                    dispatched,
                    !rejected && (mode == ApprovalMode::Auto || decision),
                    "{channel}: {name}: {mode:?}: {result:?}"
                );

                assert!(!approvals.lock().await.has_pending());
                assert_eq!(
                    sessions.lock().unwrap().sessions["session"].phase == "unsafe",
                    dispatched
                );

                if dispatched {
                    let request: Value = serde_json::from_str(
                        &fs::read_to_string(root.join("dispatch.json")).unwrap(),
                    )
                    .unwrap();

                    if *name == "schedule_propose" {
                        assert_eq!(request["method"], "add");
                        assert_eq!(request["params"]["activate"], true);
                        assert_eq!(request["params"]["instruction"], args["instruction"]);
                        assert_eq!(request["params"]["context"]["shell"], false);
                    } else if *name == "data" {
                        let mut received = request["params"].clone();

                        assert_eq!(received["scope"], memory_scope(channel, "7"));
                        received.as_object_mut().unwrap().remove("scope");

                        assert_eq!(received, *args);
                    } else {
                        assert_eq!(request["params"]["arguments"], *args);
                    }

                    assert!(result.is_ok(), "{result:?}");
                    fs::remove_file(root.join("dispatch.json")).unwrap();
                } else if rejected {
                    let error = result.unwrap_err().to_string();

                    assert!(
                        error.contains(if mode == ApprovalMode::Off {
                            "blocks mutating tools"
                        } else {
                            "approval display limit"
                        }),
                        "{error}"
                    );
                } else {
                    assert!(tool_result_text(&result.unwrap()).contains("denied"));
                }
            }
        }
    }

    for (_, plugin) in plugins.all().await {
        plugin.stop().await.unwrap();
    }

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn agent_write_approvals_display_exact_bounded_requests_before_dispatch() {
    let root = tests::test_root("agent-approval");

    fs::create_dir_all(&root).unwrap();

    let root = root.canonicalize().unwrap();

    let script = r#"
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': 'tools',
                  'version': '0.1.0', 'capabilities': ['tool']}
    elif method == 'write':
        params = request['params']
        assert params['approve'] is True
        assert pathlib.Path(params['workspace']) == root
        (root / params['path']).write_text(params['text'])
        result = params
    elif method == 'shutdown':
        break
    else:
        raise AssertionError(method)
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

    let process =
        Process::start_with("python3", ["-u", "-c", script, root.to_str().unwrap()]).await.unwrap();

    let tools = AsyncMutex::new(Some(process));

    let boundary = |size: usize| {
        let overhead = serde_json::json!({"path": "note.txt", "text": "", "workspace": root})
            .to_string()
            .len();
        "x".repeat(size - overhead)
    };

    let paired = |suffix: &str| format!("{}{suffix}", "x".repeat(400));

    let pair_rejected =
        serde_json::json!({"path": "note.txt", "text": paired("suffix-one"), "workspace": root})
            .to_string()
            .len()
            > APPROVAL_ARGUMENT_LIMIT;

    for (text, decision, rejected) in [
        (format!("{}suffix-one", "x".repeat(600)), true, true),
        (format!("{}suffix-two", "x".repeat(600)), true, true),
        (boundary(513), true, true),
        ("é".repeat(256), true, true),
        ("\\".repeat(256), true, true),
        (boundary(512), true, false),
        (paired("suffix-one"), true, pair_rejected),
        (paired("suffix-two"), true, pair_rejected),
        ("short approved write".into(), true, false),
        ("short denied write".into(), false, false),
    ] {
        let params = serde_json::json!({
            "name": "write", "workspace": root,
            "args": {"path": "note.txt", "text": text, "workspace": "untrusted", "approve": true}
        });

        let reviewed = serde_json::json!({"path": "note.txt", "text": text, "workspace": root});
        let mut prompted = false;
        let response =
            agent_tool_request(7, &params, &tools, &root, ApprovalMode::Prompt, |question| {
                prompted = true;

                assert!(!rejected);
                assert!(!root.join("note.txt").exists());
                assert!(question.contains(&reviewed.to_string()));
                assert!(question.len() <= APPROVAL_TEXT_LIMIT);

                decision
            })
            .await
            .unwrap();

        assert_eq!(prompted, !rejected);
        assert_eq!(response.error.is_some(), rejected || !decision);

        if rejected {
            assert!(response.error.unwrap().message.contains("approval display limit"));
        }

        if !rejected && decision {
            let mut dispatched = response.result.unwrap();

            assert_eq!(
                dispatched.as_object_mut().unwrap().remove("approve"),
                Some(Value::Bool(true))
            );

            assert_eq!(dispatched, reviewed);
            assert_eq!(fs::read_to_string(root.join("note.txt")).unwrap(), text);
            fs::remove_file(root.join("note.txt")).unwrap();
        } else {
            assert!(!root.join("note.txt").exists());
        }
    }

    let response = agent_tool_request(
        8,
        &serde_json::json!({"name": "x".repeat(APPROVAL_TEXT_LIMIT), "args": {}}),
        &tools,
        &root,
        ApprovalMode::Prompt,
        |_| panic!("Oversized approval text must not prompt."),
    )
    .await
    .unwrap();

    assert!(response.error.unwrap().message.contains("approval display limit"));
    assert!(!root.join("note.txt").exists());

    tools.lock().await.take().unwrap().stop().await.unwrap();
    fs::remove_dir_all(root).unwrap();
}
