// Some fixtures here serve only Unix tests.
#![cfg_attr(not(unix), allow(unused_imports, dead_code))]

mod krabfile;

#[cfg(unix)]
mod git;

#[cfg(unix)]
mod commits;

#[cfg(unix)]
mod updates;

#[cfg(unix)]
mod schedules;

#[cfg(unix)]
mod approvals;

#[cfg(unix)]
mod channels;

#[cfg(unix)]
mod tasks;

#[cfg(unix)]
mod isolation;

use super::{
    ARCHIVE_LIMIT, BANNER, BANNER_COLUMNS, Cancellation, Cli, Command, CommandSpec,
    CompletionShell, Config, DeliveryCli, DeliveryCommand, DoctorArgs, Fork, Id, InitArgs,
    KRABFILE_VERSION, KrabbPlugin, Krabfile, KrabfileExport, KrabfileImport, KrabfileValidate,
    Live, Manifest, NAME, Name, Output, Plugins, Process, ServiceCommand, SessionCli,
    SessionCommand, SessionDelete, SessionModel, SessionNew, Sha256, Source, Stop, answer,
    append_memory_context, archive, archive_root, archive_url, assistant, banner, binary_at,
    canonical_source, changed, command_output_limited, commit_event, completion_name_from,
    daemon_lock, default_krabfile_path, delivery_at, delivery_request, download, embedded,
    ensure_home, ensure_root, env_for, export_krabfile_at, generate_completion, import_krabfile_at,
    init_at, installed_at, isolate_at, krabfile_output_path, local_session, memory_scope,
    memory_tools, model_content, plugin_binary, read_manifest, read_manifest_checked,
    reclaim_worktrees, recover, recover_plugins, redact, resolve, restart_tool, revision,
    safe_archive, send_params, send_request, service_at, service_at_with, service_environment_from,
    service_name, service_path_value, service_text, session_at, turn_messages, update_at,
    validate_archive, verify_archive,
};

#[cfg(target_os = "macos")]
use super::{MACOS_SERVICE_LABEL, service_path};

#[cfg(windows)]
use super::daemon_executable;

use base64::Engine;
use clap::{CommandFactory, Parser, error::ErrorKind};

use krabb_core::types::{Capability, Content, Event, Message, ModelReply, Protocol, Request, Role};

use sha2::Digest;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[test]
fn hex_digest_preserves_lowercase_byte_encoding() {
    assert_eq!(super::hex_digest([0x00, 0x0a, 0xab, 0xff]), "000aabff");
}

pub(super) fn test_root(label: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |value| value.as_nanos());

    std::env::temp_dir().join(format!("krabb-{label}-{}-{nonce}-{sequence}", std::process::id()))
}

fn approval_finished(sessions: &Arc<Mutex<super::state::Store>>) -> bool {
    let Ok(store) = sessions.lock() else {
        return false;
    };

    let Some(session) = store.sessions.get("telegram-7") else {
        return false;
    };

    session.status == "idle"
        && session.messages.iter().any(|message| {
            message.content.iter().any(|content| {
                    matches!(content, Content::Text { text } if text == "The note was written.")
                })
        })
}

async fn registry(processes: impl IntoIterator<Item = Process>) -> Plugins {
    let plugins = Plugins::default();

    for process in processes {
        plugins.insert(Live::new(process)).await;
    }

    plugins
}

pub(super) async fn stop_registry(plugins: &Plugins) {
    for (_, plugin) in plugins.all().await {
        plugin.stop().await.unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn maximum_data_page_survives_tool_and_model_serialization() {
    let root = test_root("data-page");
    fs::create_dir_all(&root).unwrap();
    let record_limit = 255 * 1024;
    let overhead = serde_json::to_vec(&serde_json::json!({"a": ""})).unwrap().len();
    let record = serde_json::json!({"a": "\"".repeat((record_limit - overhead) / 2)});
    let page = serde_json::json!({
        "items": [{"id": i64::MAX, "record": record}],
        "limit": 20, "truncated": true, "next_after": i64::MAX
    });

    let response = root.join("page.json");
    fs::write(
        &response,
        format!(
            "{}\n",
            serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "result": page
            })
        ),
    )
    .unwrap();
    let script = r#"while IFS= read -r line; do
            case "$line" in
                *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"sqlite","version":"0.1.0","capabilities":["store"]}}' ;;
                *shutdown*) exit 0 ;;
                *) cat "$KRABB_TEST_PAGE" ;;
            esac
        done"#;
    let process =
        Process::start_with_env("sh", ["-c", script], [("KRABB_TEST_PAGE", response.as_os_str())])
            .await
            .unwrap();
    let plugins = registry([process]).await;
    let result = super::tool_with_registry(
        &plugins,
        &super::ToolRegistry::default(),
        2,
        "data_query",
        serde_json::json!({"operation": "query", "table": "notes"}),
        false,
        None,
        "cli",
        "s",
        "turn",
        &mut None,
    )
    .await
    .unwrap();
    let text = super::tool_result_text(&result);

    assert_eq!(serde_json::from_str::<serde_json::Value>(&text).unwrap(), page);

    let messages = vec![Message {
        id: "tool-1".into(),
        session: "s".into(),
        role: Role::Tool,
        sender: Some("data".into()),
        content: super::bound(vec![Content::Text { text }]),
    }];
    let request = super::model_request(3, "test", &messages, &[], None, Protocol::CURRENT).unwrap();
    let Request::Call { params, .. } = request else { panic!("expected model request") };

    let text = params["messages"][0]["content"][0]["text"].as_str().unwrap();

    assert_eq!(serde_json::from_str::<serde_json::Value>(text).unwrap(), page);
    stop_registry(&plugins).await;
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn agent_filesystem_requests_preserve_workspace_and_approval_policy() {
    let script = r#"while IFS= read -r line; do
            id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
            case "$line" in
                *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"tools","version":"0.1.0","capabilities":["tool"]}}' ;;
                *shutdown*) exit 0 ;;
                *) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":'"$line"'}' ;;
            esac
        done"#;
    let tools = Arc::new(tokio::sync::Mutex::new(Some(
        Process::start_with("sh", ["-c", script]).await.unwrap(),
    )));
    let model = Arc::new(tokio::sync::Mutex::new(None));
    let root = test_root("agent-filesystem");
    fs::create_dir_all(root.join("project")).unwrap();
    let root = root.canonicalize().unwrap();
    let workspace = root.join("project");

    for (name, mode, allowed) in [
        ("read", super::ApprovalMode::Off, true),
        ("write", super::ApprovalMode::Off, false),
        ("write", super::ApprovalMode::Prompt, false),
        ("write", super::ApprovalMode::Auto, true),
    ] {
        let response = super::agent_request(
            Request::call(
                7,
                "host/tool",
                serde_json::json!({
                    "name": name, "workspace": workspace,
                    "args": {"path": "note.txt", "text": "value"}
                }),
            ),
            model.clone(),
            tools.clone(),
            root.clone(),
            super::Bridge { approval: mode, limit: Duration::from_secs(60) },
            true,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .await
        .unwrap();

        assert_eq!(response.error.is_none(), allowed);

        if allowed {
            let args = response.result.unwrap()["params"].clone();

            assert_eq!(args["workspace"], workspace.display().to_string());
            assert_eq!(args["approve"], name == "write");
        }
    }

    assert!(
        super::agent_request(
            Request::call(
                8,
                "host/tool",
                serde_json::json!({
                    "name": "read", "workspace": root.parent().unwrap(),
                    "args": {"path": "note.txt"}
                })
            ),
            model,
            tools.clone(),
            root.clone(),
            super::Bridge { approval: super::ApprovalMode::Auto, limit: Duration::from_secs(60) },
            true,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .await
        .is_err()
    );

    tools.lock().await.take().unwrap().stop().await.unwrap();
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn agent_model_stream_returns_normalized_provider_events() {
    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"test-model","version":"0.1.0","capabilities":["model"]}}' ;;
        *'"method":"generate"'*)
            printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"event":{"kind":"text","text":"piece"}}}'
            printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"text":"complete","stop":"stop","input":null,"output":null}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let provider = Process::start_with("sh", ["-c", script]).await.unwrap();
    let model = Arc::new(tokio::sync::Mutex::new(Some(provider)));
    let tools = Arc::new(tokio::sync::Mutex::new(None));
    let workspace = test_root("model-stream");

    fs::create_dir_all(&workspace).unwrap();

    let response = super::agent_request(
        Request::call(42, "host/model_stream", serde_json::json!({})),
        model.clone(),
        tools,
        workspace,
        super::Bridge { approval: super::ApprovalMode::Off, limit: Duration::from_secs(60) },
        true,
        tokio::sync::mpsc::unbounded_channel().0,
    )
    .await
    .unwrap();

    let reply: ModelReply = serde_json::from_value(response.result.unwrap()).unwrap();

    assert_eq!(reply.text, "complete");
    assert_eq!(reply.events, vec![Event::Text { text: "piece".into() }]);

    model.lock().await.take().unwrap().stop().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn silent_agent_generations_stop_at_the_host_allowance() {
    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"test-model","version":"0.1.0","capabilities":["model"]}}' ;;
        *'"method":"generate"'*) sleep 30 ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let workspace = test_root("model-allowance");

    fs::create_dir_all(&workspace).unwrap();

    for method in ["host/model", "host/model_stream"] {
        let provider = Process::start_with("sh", ["-c", script]).await.unwrap();
        let model = Arc::new(tokio::sync::Mutex::new(Some(provider)));
        let started = tokio::time::Instant::now();

        let error = super::agent_request(
            Request::call(42, method, serde_json::json!({})),
            model.clone(),
            Arc::new(tokio::sync::Mutex::new(None)),
            workspace.clone(),
            super::Bridge { approval: super::ApprovalMode::Off, limit: Duration::from_millis(200) },
            true,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("host inference allowance"), "{method}: {error}");
        assert!(started.elapsed() < Duration::from_secs(10), "{method}");

        let _ = model.lock().await.take().unwrap().stop().await;
    }

    let mut config = Config::default();

    config.inference.max_seconds = 600;
    config.inference.task_seconds = Some(5);

    assert_eq!(super::Bridge::new(&config).limit, Duration::from_secs(5));

    config.inference.task_seconds = None;

    assert_eq!(super::Bridge::new(&config).limit, Duration::from_secs(600));
}

#[cfg(unix)]
#[tokio::test]
async fn agent_model_stream_hands_provider_callbacks_to_the_frontend() {
    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"test-model","version":"0.1.0","capabilities":["model"]}}' ;;
        *'"method":"generate"'*)
            printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"event":{"kind":"text","text":"Choose a color."}}}'
            printf '%s\n' '{"jsonrpc":"2.0","id":7,"method":"host/tool","params":{"name":"pick","args":{"color":"blue"}}}'
            IFS= read -r reply
            touch "$KRABB_TEST_EXECUTED"
            exit 1 ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let root = test_root("model-stream-handoff");

    fs::create_dir_all(&root).unwrap();

    let executed = root.join("executed");

    for declared in [true, false] {
        let provider = Process::start_with_env(
            "sh",
            ["-c", script],
            [("KRABB_TEST_EXECUTED", executed.as_os_str())],
        )
        .await
        .unwrap();

        let model = Arc::new(tokio::sync::Mutex::new(Some(provider)));

        let tools = if declared {
            serde_json::json!([{"name":"pick","schema":{"type":"object"}}])
        } else {
            serde_json::json!([])
        };

        let response = super::agent_request(
            Request::call(42, "host/model_stream", serde_json::json!({"tools":tools})),
            model.clone(),
            Arc::new(tokio::sync::Mutex::new(None)),
            root.clone(),
            super::Bridge { approval: super::ApprovalMode::Off, limit: Duration::from_secs(60) },
            true,
            tokio::sync::mpsc::unbounded_channel().0,
        )
        .await;

        if declared {
            let reply: ModelReply =
                serde_json::from_value(response.unwrap().result.unwrap()).unwrap();

            assert_eq!(reply.text, "Choose a color.");
            assert_eq!(reply.stop, "tool_calls");

            assert_eq!(
                reply.events.last().unwrap(),
                &Event::Tool {
                    name: "pick".into(),
                    args: serde_json::json!({"color":"blue"}),
                    id: None,
                    thought_signature: None,
                }
            );
        } else {
            assert!(response.unwrap_err().to_string().contains("undeclared frontend tool"));
        }

        assert!(!executed.exists());

        model.lock().await.take().unwrap().stop().await.unwrap();
    }

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_agent_model_stream_restarts_the_provider_process() {
    let root = test_root("model-stream-cancel");

    fs::create_dir_all(&root).unwrap();

    let marker = root.join("starts");

    let script = r#"
echo started >> "$KRABB_TEST_STARTS"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"test-model","version":"0.1.0","capabilities":["model"]}}' ;;
        *'"method":"generate"'*) printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"event":{"kind":"text","text":"piece"}}}'; sleep 30 ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let provider =
        Process::start_with_env("sh", ["-c", script], [("KRABB_TEST_STARTS", marker.as_os_str())])
            .await
            .unwrap();

    let model = Arc::new(tokio::sync::Mutex::new(Some(provider)));
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();

    let stream = tokio::spawn(super::agent_request(
        Request::call(43, "host/model_stream", serde_json::json!({})),
        model.clone(),
        Arc::new(tokio::sync::Mutex::new(None)),
        root.clone(),
        super::Bridge { approval: super::ApprovalMode::Off, limit: Duration::from_secs(60) },
        true,
        events,
    ));

    assert!(tokio::time::timeout(Duration::from_secs(2), received.recv()).await.unwrap().is_some());

    stream.abort();

    assert!(stream.await.unwrap_err().is_cancelled());

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if fs::read_to_string(&marker).is_ok_and(|starts| starts.lines().count() >= 2) {
                break;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    model.lock().await.take().unwrap().stop().await.unwrap();
}

#[test]
fn scheduled_message_requires_an_explicit_bounded_destination() {
    let message = super::scheduled_message(&serde_json::json!({
        "channel": "telegram",
        "chat": "12345",
        "private": true,
        "thread": "topic-1",
        "text": "  Backup finished.  "
    }))
    .unwrap();

    assert_eq!(message.channel, "telegram");
    assert_eq!(message.chat, "12345");
    assert!(message.private);
    assert_eq!(message.thread.as_deref(), Some("topic-1"));
    assert_eq!(message.text, "Backup finished.");

    let destination = super::AuthorizedDestination {
        channel: "telegram".into(),
        chat: "12345".into(),
        private: true,
        thread: Some("topic-1".into()),
    };

    assert!(super::matches_authorized_destination(&message, &destination));

    assert!(!super::matches_authorized_destination(
        &message,
        &super::AuthorizedDestination { chat: "elsewhere".into(), ..destination.clone() }
    ));

    assert_ne!(
        super::scheduled_delivery_id("schedule-42", 11, 0),
        super::scheduled_delivery_id("schedule-42", 12, 0)
    );

    assert_ne!(
        super::scheduled_delivery_id("schedule-42", 11, 0),
        super::scheduled_delivery_id("schedule-42", 11, 1)
    );

    assert!(
        super::scheduled_message(&serde_json::json!({
            "channel": "telegram",
            "private": true,
            "text": "hello"
        }))
        .is_err()
    );

    assert!(
        super::scheduled_message(&serde_json::json!({
            "channel": "telegram",
            "chat": "12345",
            "private": true,
            "text": "hello\n\u{0000}"
        }))
        .is_err()
    );

    assert!(
        super::scheduled_message(&serde_json::json!({
            "channel": "telegram",
            "chat": "12345\n",
            "private": true,
            "text": "hello"
        }))
        .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn scheduled_message_queues_through_an_installed_channel() {
    let root = test_root("scheduled-message");

    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let process = Process::start_with("sh", ["-c", script]).await.unwrap();
    let plugins = registry([process]).await;

    let destination = super::AuthorizedDestination {
        channel: "telegram".into(),
        chat: "12345".into(),
        private: true,
        thread: None,
    };

    let message_tools = super::scheduled_message_tools(&plugins, Some(&destination)).await;

    assert_eq!(message_tools.len(), 1);
    assert_eq!(message_tools[0].name, "message_send");
    assert_eq!(message_tools[0].schema["properties"]["channel"]["const"], "telegram");
    assert!(super::scheduled_message_tools(&plugins, None).await.is_empty());

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));
    {
        let mut store = sessions.lock().unwrap();
        store.ensure_schedule("schedule-42", "test").unwrap();
        store
            .begin_with_roles(
                "schedule-42",
                Message {
                    id: "schedule-user".into(),
                    session: "schedule-42".into(),
                    role: Role::User,
                    sender: Some("scheduler".into()),
                    content: vec![Content::Text { text: "send a message".into() }],
                },
                Vec::new(),
            )
            .unwrap();
    }

    let (notices, _received) = tokio::sync::mpsc::channel(1);

    let mut failed_tool = None;

    let result = super::execute_tool(
        &plugins,
        8,
        "message_send",
        serde_json::json!({
            "channel": "telegram",
            "chat": "12345",
            "private": true,
            "thread": null,
            "text": "Backup finished."
        }),
        "codex",
        "test",
        super::ApprovalMode::Auto,
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        "scheduler",
        "42",
        None,
        "schedule-42",
        &sessions,
        None,
        false,
        false,
        false,
        notices,
        tokio::time::Instant::now() + Duration::from_secs(1),
        &mut failed_tool,
        Some(&destination),
        Some(11),
        0,
    )
    .await
    .unwrap();

    assert_eq!(result, "Message queued for delivery through telegram.");

    let mut failed_tool = None;

    super::execute_tool(
        &plugins,
        8,
        "message_send",
        serde_json::json!({
            "channel": "telegram",
            "chat": "12345",
            "private": true,
            "thread": null,
            "text": "Backup finished."
        }),
        "codex",
        "test",
        super::ApprovalMode::Auto,
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        "scheduler",
        "42",
        None,
        "schedule-42",
        &sessions,
        None,
        false,
        false,
        false,
        tokio::sync::mpsc::channel(1).0,
        tokio::time::Instant::now() + Duration::from_secs(1),
        &mut failed_tool,
        Some(&destination),
        Some(12),
        0,
    )
    .await
    .unwrap();

    {
        let store = sessions.lock().unwrap();

        assert_eq!(store.outbox.len(), 2);
        assert_ne!(store.outbox[0].id, store.outbox[1].id);
        assert_eq!(store.outbox[0].channel, "telegram");
        assert_eq!(store.outbox[0].chat, "12345");
        assert!(store.outbox[0].private);
        assert_eq!(store.outbox[0].text, "Backup finished.");
    }

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn scheduled_message_delivers_to_an_authorized_local_tui_session() {
    let root = test_root("scheduled-tui-message");

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));
    {
        let mut store = sessions.lock().unwrap();
        store.ensure_schedule("schedule-51", "test").unwrap();
        store.ensure("tui-default", "test").unwrap();
        store
            .begin_with_roles(
                "schedule-51",
                Message {
                    id: "schedule-user-1700000000000".into(),
                    session: "schedule-51".into(),
                    role: Role::User,
                    sender: Some("scheduler".into()),
                    content: vec![Content::Text { text: "send a ping".into() }],
                },
                Vec::new(),
            )
            .unwrap();
    }

    let destination = super::AuthorizedDestination {
        channel: "tui".into(),
        chat: "tui-default".into(),
        private: false,
        thread: None,
    };

    assert_eq!(
        super::scheduled_message_tools(&Plugins::default(), Some(&destination)).await.len(),
        1
    );

    let mut failed_tool = None;
    let send_started = super::unix_timestamp_millis();

    let result = super::execute_tool(
        &Plugins::default(),
        1,
        "message_send",
        serde_json::json!({
            "channel": "tui",
            "chat": "tui-default",
            "private": false,
            "thread": null,
            "text": "Ping!"
        }),
        "codex",
        "test",
        super::ApprovalMode::Auto,
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        "scheduler",
        "51",
        None,
        "schedule-51",
        &sessions,
        None,
        false,
        false,
        false,
        tokio::sync::mpsc::channel(1).0,
        tokio::time::Instant::now() + Duration::from_secs(1),
        &mut failed_tool,
        Some(&destination),
        Some(9001),
        0,
    )
    .await
    .unwrap();

    let send_finished = super::unix_timestamp_millis();

    assert_eq!(result, "Message added to TUI session tui-default.");

    let message =
        sessions.lock().unwrap().sessions["tui-default"].messages.last().cloned().unwrap();

    assert_eq!(message.role, Role::Assistant);

    let (timestamp, key) =
        message.id.strip_prefix("schedule-notice-at-").unwrap().split_once('-').unwrap();

    let timestamp = timestamp.parse::<u128>().unwrap();

    assert!((send_started..=send_finished).contains(&timestamp));
    assert_eq!(key, "schedule-51-run-9001-tool-0");
    assert!(message.id.ends_with(&super::scheduled_delivery_id("schedule-51", 9001, 0)));
    assert_eq!(message.content, vec![Content::Text { text: "Ping!".into() }]);

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn a_clipped_streamed_answer_streams_its_limit_notice() {
    let root = test_root("clipped-stream-notice");

    let provider_script = r#"
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': 'codex', 'version': '0.1.0',
                  'capabilities': ['model']}
    elif method == 'generate':
        for _ in range(5):
            event = {'kind': 'text', 'text': 'x' * 60000}
            print(json.dumps({'jsonrpc': '2.0', 'method': 'event', 'params': {'event': event}}),
                  flush=True)
        result = {'text': 'x' * 300000, 'stop': 'stop'}
    elif method == 'shutdown':
        break
    else:
        continue
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

    let provider = Process::start_with("python3", ["-u", "-c", provider_script]).await.unwrap();
    let plugins = registry([provider]).await;
    let provider = plugins.get("codex").await.unwrap();

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    let question = Message {
        id: "schedule-user".into(),
        session: "schedule-53".into(),
        role: Role::User,
        sender: Some("scheduler".into()),
        content: vec![Content::Text { text: "Write a long report.".into() }],
    };

    {
        let mut store = sessions.lock().unwrap();
        store.ensure_schedule("schedule-53", "test").unwrap();
        store.begin_with_roles("schedule-53", question.clone(), Vec::new()).unwrap();
    }

    let stop = Stop::new();

    let (notices, mut received) = tokio::sync::mpsc::channel(8);

    let drain = tokio::spawn(async move {
        let mut text = String::new();

        while let Some(notice) = received.recv().await {
            if let super::StreamNotice::Text(part) = notice {
                text.push_str(&part);
            }
        }

        text
    });

    let mut call = 1;
    let mut failed_tool = None;

    let reply = answer(
        &provider,
        &plugins,
        "test",
        vec![question],
        "schedule-53",
        "scheduler",
        "53",
        None,
        &sessions,
        Some(&root),
        &root,
        false,
        false,
        false,
        super::ApprovalMode::Auto,
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        &Cancellation::new(),
        &stop,
        tokio::time::Instant::now() + super::TURN_LIMIT,
        &mut call,
        &mut failed_tool,
        None,
        notices,
        None,
        Some(9003),
        None,
    )
    .await
    .unwrap();

    let delivered = drain.await.unwrap();

    assert!(reply.text.ends_with(krabb_core::types::CLIP_NOTICE));
    assert!(delivered.ends_with(krabb_core::types::CLIP_NOTICE));

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn scheduled_agent_uses_the_authorized_default_tui_message_tool() {
    let root = test_root("scheduled-agent-tui-message");

    let provider_script = r#"
count=0
while IFS= read -r line; do
    request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')

    case "$line" in
        *'"method":"hello"'*)
            printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"protocol":{"major":0,"minor":1},"id":"codex","version":"0.1.0","capabilities":["model"]}}'
            ;;
        *'"method":"generate"'*)
            if [ "$count" = 0 ]; then
                count=1

                case "$line" in
                    *'"name":"message_send"'*)
                        printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"event":{"kind":"text","text":"Checking…"}}}'
                        printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"text":"Checking…","stop":"tool","events":[{"kind":"tool","name":"message_send","args":{"channel":"tui","chat":"tui-default","private":false,"thread":null,"text":"Ping!"}}]}}'
                        ;;
                    *)
                        printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"text":"I do not have a tool for sending a TUI message.","stop":"stop"}}'
                        ;;
                esac
            else
                printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"event":{"kind":"text","text":"DONE: "}}}' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"text":"DONE: Ping sent to the TUI.","stop":"stop"}}'
            fi
            ;;
        *'"method":"shutdown"'*)
            printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"ok":true}}'
            exit 0
            ;;
    esac
done
"#;

    let provider = Process::start_with("sh", ["-c", provider_script]).await.unwrap();
    let plugins = registry([provider]).await;
    let provider = plugins.get("codex").await.unwrap();

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));
    {
        let mut store = sessions.lock().unwrap();
        store.ensure_schedule("schedule-52", "test").unwrap();
        store.ensure("tui-default", "test").unwrap();
        store
            .begin_with_roles(
                "schedule-52",
                Message {
                    id: "schedule-user".into(),
                    session: "schedule-52".into(),
                    role: Role::User,
                    sender: Some("scheduler".into()),
                    content: vec![Content::Text {
                        text: "Ping me on the default TUI chat.".into(),
                    }],
                },
                Vec::new(),
            )
            .unwrap();
    }

    let destination = super::AuthorizedDestination {
        channel: "tui".into(),
        chat: "tui-default".into(),
        private: false,
        thread: None,
    };

    let stop = Stop::new();
    let (notices, mut received) = tokio::sync::mpsc::channel(8);

    let drain = tokio::spawn(async move {
        let mut text = String::new();

        while let Some(notice) = received.recv().await {
            if let super::StreamNotice::Text(part) = notice {
                text.push_str(&part);
            }
        }

        text
    });

    let mut call = 1;
    let mut failed_tool = None;

    let reply = answer(
        &provider,
        &plugins,
        "test",
        vec![Message {
            id: "schedule-instruction".into(),
            session: "schedule-52".into(),
            role: Role::User,
            sender: Some("scheduler".into()),
            content: vec![Content::Text { text: "Ping me on the default TUI chat.".into() }],
        }],
        "schedule-52",
        "scheduler",
        "52",
        None,
        &sessions,
        Some(&root),
        &root,
        false,
        false,
        false,
        super::ApprovalMode::Auto,
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        &Cancellation::new(),
        &stop,
        tokio::time::Instant::now() + super::TURN_LIMIT,
        &mut call,
        &mut failed_tool,
        None,
        notices,
        Some(destination),
        Some(9002),
        None,
    )
    .await
    .unwrap();

    let delivered = drain.await.unwrap();

    assert_eq!(delivered, "Checking…DONE: Ping sent to the TUI.");

    assert_eq!(reply.text, "DONE: Ping sent to the TUI.");

    let message = {
        let store = sessions.lock().unwrap();
        store.sessions["tui-default"].messages.last().unwrap().clone()
    };

    assert_eq!(message.role, Role::Assistant);
    assert_eq!(message.content, vec![Content::Text { text: "Ping!".into() }]);

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn bridge_delivers_outbox_messages_through_other_loaded_channels() {
    let root = test_root("scheduled-channel-delivery");
    let marker = root.join("discord-delivered");

    fs::create_dir_all(&root).unwrap();

    let primary_script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}' ;;
        *'"method":"poll"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"events":[]}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let other_script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"discord","version":"0.1.0","capabilities":["channel"]}}' ;;
        *'"method":"send"'*) touch '__MARKER__'; printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"message_id":9}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let other_script = other_script.replace("__MARKER__", &marker.display().to_string());

    let provider_script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"codex","version":"0.1.0","capabilities":["model"]}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let primary = Process::start_with("sh", ["-c", primary_script]).await.unwrap();
    let other = Process::start_with("sh", ["-c", &other_script]).await.unwrap();
    let provider = Process::start_with("sh", ["-c", provider_script]).await.unwrap();
    let plugins = registry([primary, other, provider]).await;

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    sessions
        .lock()
        .unwrap()
        .queue_delivery("delivery-1", "discord", "room-9", false, None, "Backup complete.")
        .unwrap();

    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);
    let sessions_for_signal = Arc::clone(&sessions);

    let notifier = tokio::spawn(async move {
        for _ in 0..300 {
            if sessions_for_signal.lock().unwrap().outbox.is_empty() {
                break;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        signal.signal();
    });

    tokio::time::timeout(
        Duration::from_secs(4),
        super::bridge_with_media(
            &plugins,
            "telegram",
            "codex",
            "test",
            Arc::clone(&sessions),
            Arc::clone(&stop),
            Arc::new(Mutex::new(BTreeMap::new())),
            BTreeMap::new(),
            super::ApprovalMode::Off,
            false,
            root.clone(),
            root.join("workspace"),
            root.join("media"),
            Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
            10,
        ),
    )
    .await
    .unwrap()
    .unwrap();

    notifier.await.unwrap();

    assert!(marker.is_file());
    assert!(sessions.lock().unwrap().outbox.is_empty());

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_scope_is_bound_to_provider_and_exact_conversation() {
    assert_eq!(memory_scope("telegram", "dm:42"), "provider:telegram:dm:42");
    assert_eq!(memory_scope("tui", "default"), "tui:default");
    assert_eq!(memory_scope("tui", "global"), "tui:global");
    assert_ne!(memory_scope("tui", "global"), "global");
    assert_ne!(memory_scope("telegram", "dm:42"), memory_scope("discord", "dm:42"));
    assert_ne!(memory_scope("telegram", "dm:42"), memory_scope("telegram", "dm:43"));

    let tools = memory_tools();

    assert_eq!(
        tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
        ["memory_search", "memory_remember"]
    );

    assert!(tools.iter().all(|tool| tool.schema["additionalProperties"] == false));
}

#[cfg(unix)]
#[tokio::test]
async fn injects_only_the_current_conversations_bounded_memory_index() {
    let script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocol\":{\"major\":0,\"minor\":1},\"id\":\"memory\",\"version\":\"0.1.0\",\"capabilities\":[\"memory\"]}}' ;; *provider:telegram:dm:42*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":4,\"result\":{\"learning\":\"autonomous\",\"items\":[{\"key\":\"preference\",\"summary\":\"likes concise replies\",\"scope\":\"provider:telegram:dm:42\",\"updated\":2}]}}' ;; *provider:tui:default*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":5,\"result\":{\"learning\":\"guided\",\"items\":[{\"key\":\"legacy\",\"summary\":\"saved before the upgrade\",\"updated\":1}]}}' ;; *default*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":4,\"result\":{\"learning\":\"guided\",\"items\":[{\"key\":\"current\",\"summary\":\"saved after the upgrade\",\"scope\":\"default\",\"updated\":3}]}}' ;; *index*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":4,\"result\":{\"learning\":\"guided\",\"items\":[]}}' ;; *shutdown*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{\"ok\":true}}'; exit 0 ;; esac; done"#
            .replace("\\\\n", "\\n")
            .replace("\\\"", "\"");

    let process = Process::start_with("sh", ["-c", &script]).await.unwrap();
    let plugins = registry([process]).await;

    let mut messages = vec![Message {
        id: "user".into(),
        session: "telegram-42".into(),
        role: Role::User,
        sender: None,
        content: vec![Content::Text { text: "hello".into() }],
    }];

    let mut call = 4;

    tokio::time::timeout(
        Duration::from_secs(2),
        append_memory_context(&plugins, &mut messages, "provider:telegram:dm:42", &mut call),
    )
    .await
    .unwrap();

    assert_eq!(call, 5);
    assert_eq!(messages.len(), 2);

    let Content::Text { text } = &messages[1].content[0] else {
        panic!("memory context should be text");
    };

    assert!(text.contains("likes concise replies"));
    assert!(text.contains("Autonomous learning is enabled"));
    assert!(text.contains("Entries are ordered newest saved first"));
    assert!(text.contains("follow the newer memory"));
    assert!(!text.contains("another conversation's secret"));

    let mut tui_messages = vec![Message {
        id: "user".into(),
        session: "default".into(),
        role: Role::User,
        sender: Some("tui".into()),
        content: vec![Content::Text { text: "ping".into() }],
    }];

    let mut tui_call = 4;

    tokio::time::timeout(
        Duration::from_secs(2),
        append_memory_context(
            &plugins,
            &mut tui_messages,
            &memory_scope("tui", "default"),
            &mut tui_call,
        ),
    )
    .await
    .unwrap();

    assert_eq!(tui_messages.len(), 2);

    let Content::Text { text } = &tui_messages[1].content[0] else {
        panic!("TUI memory context should be text");
    };

    assert!(text.contains("saved before the upgrade"));
    assert!(text.contains("saved after the upgrade"));

    stop_registry(&plugins).await;
}

#[test]
fn status_reads_as_a_tree_with_one_next_action() {
    let value = serde_json::json!({
        "version": super::VERSION,
        "health": "unhealthy",
        "daemon": "stopped",
        "intelligence": {
            "status": "configured",
            "plugins": ["codex", "gemini"]
        },

        "channels": {
            "status": "configured",
            "plugins": ["telegram", "discord"]
        },
    });

    assert_eq!(
        super::status_text(&value, 0),
        format!(
            "Krabb {} needs attention\n\
             ├─ Runtime: stopped\n\
             ├─ Intelligence: configured (codex, gemini)\n\
             ├─ Channels: configured (telegram, discord)\n\
             └─ Plugins: 0 installed",
            super::VERSION
        )
    );

    let healthy = serde_json::json!({
        "version": "0.1.0",
        "health": "healthy",
        "daemon": "running",
        "intelligence": {"status": "configured", "plugins": ["codex"]},
        "channels": {"status": "not configured", "plugins": []},
        "recovery": {"from": "0.0.9", "accepted": true},
    });

    assert_eq!(
        super::status_text(&healthy, 1),
        "Krabb 0.1.0 is healthy\n\
         ├─ Runtime: running\n\
         ├─ Intelligence: configured (codex)\n\
         ├─ Channels: not configured\n\
         ├─ Plugins: 1 installed\n\
         └─ Recovery point: Krabb 0.0.9 (kept)"
    );

    let unfinished = serde_json::json!({"recovery": {"from": "0.0.9", "accepted": false}});

    assert!(
        super::status_text(&unfinished, 0)
            .ends_with("\n\nUndo the unfinished update with `krabb update --rollback`.")
    );

    assert_eq!(
        super::capability_text(&serde_json::json!({
            "status": "not configured",
            "plugins": [],
        })),
        "not configured"
    );
}

#[test]
fn status_places_each_plugin_on_its_path_with_a_next_action() {
    let entry = |capability: &str| super::Entry {
        source: String::new(),
        revision: String::new(),
        pinned: false,
        default: false,
        hash: String::new(),
        version: "0.1.0".into(),
        protocol: Protocol::CURRENT,
        capabilities: vec![capability.into()],
        permissions: Vec::new(),
        variables: Vec::new(),
        settings: Vec::new(),
        commands: Vec::new(),
        linked: false,
        state: None,
    };

    let model = entry("model");
    let active = vec!["openai".to_owned()];

    let state = |id, entry, configured, active: Option<&Vec<String>>, service| {
        let observed = super::status::Observed {
            active: active.map(Vec::as_slice),
            ..super::status::Observed::default()
        };

        let value = super::plugin_state(id, entry, configured, &observed, service);

        (value["state"].as_str().unwrap().to_owned(), value["next"].as_str().map(str::to_owned))
    };

    assert_eq!(state("openai", &model, false, Some(&active), true), ("ready".into(), None));

    assert_eq!(
        state("telegram", &model, false, None, true),
        ("needs setup".into(), Some("krabb plugin setup telegram".into()))
    );

    assert_eq!(
        state("gemini", &model, true, None, false),
        ("configured".into(), Some("krabb service install".into()))
    );

    assert_eq!(state("gemini", &model, true, None, true).1.unwrap(), "krabb service start");

    let failed = state("gemini", &model, true, Some(&active), true);

    assert_eq!(failed.0, "failed to start");
    assert_eq!(failed.1.unwrap(), "krabb service restart");

    let observe = |failure, missing| super::status::Observed {
        active: Some(active.as_slice()),
        failure,
        missing,
    };

    let failure = super::StartFailure::new("Gemini rejected the API key (401).");

    let rejected =
        super::plugin_state("gemini", &model, true, &observe(Some(&failure), None), true);

    assert_eq!(rejected["state"], "failed to start");
    assert_eq!(rejected["reason"], "Gemini rejected the API key (401).");
    assert_eq!(rejected["next"], "krabb plugin setup gemini");

    let help = "Install signal-cli from https://github.com/AsamK/signal-cli#installation.";
    let signal = entry("channel");

    for (configured, expected) in [(true, "missing program"), (false, "needs setup")] {
        let value =
            super::plugin_state("signal", &signal, configured, &observe(None, Some(help)), true);

        assert_eq!(value["state"], expected);
    }

    let loaded = vec!["signal".to_owned()];

    let missing = super::status::Observed {
        active: Some(loaded.as_slice()),
        failure: None,
        missing: Some(help),
    };

    assert_eq!(
        super::plugin_state("signal", &signal, false, &missing, true)["next"],
        help,
        "a loaded plugin that cannot find its program is not ready"
    );

    let next = |state: &str, next: &str| {
        let value =
            serde_json::json!({"plugin_states": [{"id": "codex", "state": state, "next": next}]});
        let text = super::status_text(&value, 1);

        text.rsplit("\n\n").next().unwrap().to_owned()
    };

    assert_eq!(
        next("configured", "krabb service start"),
        "Start codex with `krabb service start`."
    );

    assert_eq!(
        next("configured", "krabb service install"),
        "Install the service with `krabb service install`."
    );

    assert!(next("failed to start", "krabb service restart").starts_with("The log at "));

    assert_eq!(
        next("failed to start", "krabb plugin setup codex"),
        "Set up codex again with `krabb plugin setup codex`."
    );

    assert_eq!(next("missing program", help), help);

    let reasoned = super::status_text(
        &serde_json::json!({"plugin_states": [{"id": "gemini", "state": "failed to start",
            "reason": "Gemini rejected the API key (401).", "next": "krabb plugin setup gemini"}]}),
        1,
    );

    assert!(reasoned.contains("gemini: failed to start (Gemini rejected the API key (401))"));

    let unready = serde_json::json!({
        "health": "unhealthy",
        "health_details": ["selected intelligence plugin is not ready"],
        "plugin_states": [{"id": "codex", "state": "failed to start", "next": "krabb service restart"}],
    });

    assert!(
        super::status_text(&unready, 1)
            .ends_with(" shows why codex failed.\nRestart codex with `krabb service restart`.")
    );

    assert_eq!(state("tui", &entry("client"), true, Some(&active), true).0, "ready");

    let text = super::status_text(
        &serde_json::json!({"plugin_states": [
            {"id": "openai", "state": "ready", "next": null},
            {"id": "telegram", "state": "needs setup", "next": "krabb plugin setup telegram"},
        ]}),
        2,
    );

    assert!(text.ends_with(
        "└─ Plugins: 2 installed\n   ├─ openai: ready\n   └─ telegram: needs setup\n\n\
         Set up telegram with `krabb plugin setup telegram`."
    ));
}

#[test]
fn status_health_treats_channels_as_optional_and_explains_unhealthy_state() {
    assert_eq!(super::status_health(true, true, false, false, false), ("healthy", vec![]));

    assert_eq!(
        super::status_health(true, true, true, false, true),
        (
            "unhealthy",
            vec![
                "no channel is ready or the selected channel is not ready",
                "plugin commands have name conflicts",
            ],
        )
    );

    assert_eq!(
        super::status_health(false, false, false, false, false),
        (
            "unhealthy",
            vec!["configuration is invalid", "selected intelligence plugin is not ready"],
        )
    );
}

#[test]
fn status_names_the_next_command_for_an_incomplete_home() {
    let details = vec!["configuration is invalid", "selected intelligence plugin is not ready"];

    assert_eq!(
        super::explain(details.clone(), false, false),
        [
            "configuration is missing (run `krabb init`)",
            "no intelligence plugin is installed (run `krabb plugin install <provider>`)",
        ]
    );

    assert_eq!(super::explain(details.clone(), true, true), details);
}

#[test]
fn formats_status_health_details_only_when_present() {
    let mut value = serde_json::json!({
        "version": super::VERSION,
        "health": "unhealthy",
        "health_details": ["configuration is invalid", "plugin commands have name conflicts"],
        "daemon": "running",
        "intelligence": {"status": "configured", "plugins": ["codex"]},
        "channels": {"status": "not configured", "plugins": []},
    });

    let text = super::status_text(&value, 3);

    assert!(text.ends_with(
        "├─ Plugins: 3 installed\n└─ Needs attention\n   ├─ Configuration is invalid\n   \
         └─ Plugin commands have name conflicts\n\nFind the invalid setting with `krabb doctor`."
    ));

    value["health_details"] = serde_json::json!([
        "no intelligence plugin is installed (run `krabb plugin install <provider>`)"
    ]);

    assert!(super::status_text(&value, 3).ends_with(
        "   └─ No intelligence plugin is installed\n\n\
         Install a provider with `krabb plugin install <provider>`."
    ));

    value["health_details"] = serde_json::json!([]);

    assert!(!super::status_text(&value, 3).contains("Needs attention"));
}

#[tokio::test]
async fn tui_reports_a_human_readable_daemon_requirement() {
    let root = test_root("tui-daemon-required");
    let error = super::require_tui_daemon(&root).await.unwrap_err();

    assert_eq!(
        error.to_string(),
        concat!(
            "The Krabb daemon is not running.\n",
            "Start it with `krabb service start` or run `krabb-daemon`."
        )
    );
}

#[test]
fn formats_plugin_install_results_as_compact_tree_rows() {
    assert_eq!(
        super::plugin_install_output(
            &[
                serde_json::json!({
                    "id": "codex",
                    "version": "0.1.0",
                    "status": "installed",
                    "activation": "active"
                }),
                serde_json::json!({
                    "id": "telegram",
                    "version": "0.8.2",
                    "status": "failed"
                }),
            ],
            false
        ),
        concat!(
            "Installed 1 of 2 plugins\n",
            "├─ codex@v0.1.0 successfully installed\n",
            "└─ telegram@v0.8.2 installation failed"
        )
    );

    assert_eq!(
        super::plugin_install_output(
            &[serde_json::json!({
                "id": "tools",
                "version": "0.2.0",
                "status": "installed",
                "activation": "active"
            })],
            true
        ),
        "Linked 1 plugin\n└─ tools@v0.2.0 successfully linked"
    );
}

#[test]
fn failed_installs_show_each_reason_and_next_command() {
    assert_eq!(
        super::plugin_install_output(
            &[
                serde_json::json!({
                    "id": "telegram",
                    "status": "failed",
                    "error": "No stable release of plugin telegram works with this version of Krabb.",
                    "next": "Install the newest pre-release with `krabb plugin install telegram --unstable`.",
                }),
                serde_json::json!({
                    "id": "nosuch",
                    "status": "failed",
                    "error": "Plugin nosuch is not in the official plugin index.\nCheck the \
                              ID\u{1b}[2J.",
                }),
            ],
            false
        ),
        "Installed 0 of 2 plugins\n\
         ├─ telegram installation failed\n\
         │  ├─ No stable release of plugin telegram works with this version of Krabb.\n\
         │  └─ Install the newest pre-release with `krabb plugin install telegram --unstable`.\n\
         └─ nosuch installation failed\n   \
         ├─ Plugin nosuch is not in the official plugin index.\n   \
         └─ Check the ID\u{fffd}[2J."
    );
}

#[test]
fn plugin_install_failures_point_to_the_daily_application_log() {
    let log = super::logging::application_log_path().display().to_string();

    assert_eq!(super::plugin_install_error(), format!("The log at {log} has more detail."));
}

#[test]
fn plugin_install_guidance_uses_the_service_state() {
    assert_eq!(
        super::plugin_service_guidance(false),
        "The Krabb service is not installed.\nRun every plugin in the background with `krabb \
         service install`."
    );

    assert_eq!(
        super::plugin_service_guidance(true),
        "Activate the new plugins with `krabb service restart`."
    );
}

#[test]
fn plugin_install_defers_activation_when_provider_is_not_ready() {
    assert!(!super::plugin_activation_is_deferred("Model credentials are not ready."));
    assert!(super::plugin_activation_is_deferred("Channel credentials are not ready."));
    assert!(!super::plugin_activation_is_deferred("Plugin protocol is incompatible."));
}

#[test]
fn plugin_update_errors_point_to_the_daily_application_log() {
    assert_eq!(
        super::plugin_update_error(&[
            serde_json::json!({
                "id": "tools",
                "status": "failed",
                "error": "binary missing"
            }),
            serde_json::json!({"id": "tui", "status": "updated", "version": "0.1.0"}),
        ]),
        format!(
            "Could not update 1 plugin.\n\nThe tools plugin could not be updated. Binary missing.\n\nFor details about the failed update, check the application logs:\n<{}>",
            super::logging::application_log_path().display()
        )
    );
}

#[test]
fn formats_plugin_update_results_with_tree_spacing() {
    assert_eq!(
        super::plugin_update_output(&[
            serde_json::json!({
                "id": "codex",
                "version": "0.2.0",
                "status": "updated"
            }),
            serde_json::json!({
                "id": "telegram",
                "version": "0.8.2",
                "status": "failed"
            }),
        ]),
        concat!(
            "Updated 1 of 2 plugins.\n",
            "├─ codex@v0.2.0 successfully updated\n",
            "└─ telegram@v0.8.2 update failed"
        )
    );

    assert_eq!(
        super::plugin_update_output(&[
            serde_json::json!({"id": "tools", "version": "0.1.0", "status": "unchanged"}),
        ]),
        "All plugins are up to date.\n└─ tools@v0.1.0 already up to date"
    );
}

#[test]
fn plugin_uninstall_details_follow_the_summary_without_an_empty_line() {
    assert_eq!(
        super::plugin_uninstall_output(&serde_json::json!({
            "id": "tools",
            "unloaded": true,
            "purged": {"sessions": 1, "deliveries": 0}
        }),),
        concat!(
            "Uninstalled plugin tools.\n",
            "├─ Plugin unloaded from the background runtime.\n",
            "└─ Removed 1 session and 0 delivery records."
        )
    );

    assert_eq!(
        super::plugin_uninstall_output(&serde_json::json!({
            "id": "telegram",
            "revoked": {"secrets": ["KRABB_TELEGRAM_TOKEN"], "keyring": false, "grants": 2}
        })),
        concat!(
            "Uninstalled plugin telegram.\n",
            "├─ Removed 1 saved secret or setting value.\n",
            "└─ Revoked 2 sharing grants."
        )
    );

    assert_eq!(
        super::plugin_uninstall_output(&serde_json::json!({
            "id": "telegram",
            "revoked": {"secrets": [], "keyring": false, "grants": 1, "cleanup": "partial",
                "failures": [{"step": "service_credentials",
                    "error": "Could not remove telegram's values from the service credential copy \
                        /home/owner/.config/systemd/user/krabb.service.credentials.json: \
                        Permission denied (os error 13).",
                    "next": "Run `krabb service uninstall` to remove the copy."}]}
        })),
        concat!(
            "Uninstalled plugin telegram.\n",
            "├─ Revoked 1 sharing grant.\n",
            "└─ Could not remove telegram's values from the service credential copy \
                /home/owner/.config/systemd/user/krabb.service.credentials.json: \
                Permission denied (os error 13).\n",
            "   └─ Run `krabb service uninstall` to remove the copy."
        )
    );

    let partial = serde_json::json!({
        "id": "telegram", "version": "0.8.2", "status": "uninstalled", "unloaded": true,
        "purged": null,
        "revoked": {"secrets": [], "keyring": false, "grants": 0, "cleanup": "partial",
            "failures": [{"step": "service_credentials", "error": "Could not remove them.",
                "next": "Run `krabb service uninstall` to remove the copy."}]}
    });

    assert_eq!(
        super::plugin_uninstall_json(&partial),
        serde_json::json!({
            "action": "uninstall", "id": "telegram", "status": "uninstalled", "unloaded": true,
            "purged": null, "revoked": partial["revoked"]
        })
    );

    assert_eq!(
        super::plugin_uninstall_batch_output(
            &[
                ("codex@v0.1.0 successfully uninstalled".into(), Vec::new()),
                (
                    "telegram@v0.8.2 successfully uninstalled".into(),
                    vec![
                        "Could not remove telegram's values from the service credential copy."
                            .into(),
                        "Run `krabb service uninstall` to remove the copy.".into(),
                    ],
                ),
            ],
            2,
        ),
        concat!(
            "Uninstalled 2 plugins.\n",
            "├─ codex@v0.1.0 successfully uninstalled\n",
            "└─ telegram@v0.8.2 successfully uninstalled\n",
            "   ├─ Could not remove telegram's values from the service credential copy.\n",
            "   └─ Run `krabb service uninstall` to remove the copy."
        )
    );

    assert_eq!(
        super::plugin_uninstall_batch_output(
            &[
                ("tools uninstallation failed".into(), Vec::new()),
                ("scheduler uninstallation failed".into(), Vec::new()),
                ("foo uninstallation failed".into(), Vec::new()),
            ],
            3,
        ),
        concat!(
            "Uninstalled 0 of 3 plugins.\n",
            "├─ tools uninstallation failed\n",
            "├─ scheduler uninstallation failed\n",
            "└─ foo uninstallation failed\n"
        )
    );
}

#[test]
fn plugin_uninstall_ids_require_unique_valid_identifiers() {
    assert_eq!(
        super::plugin_uninstall_ids(vec!["codex".into(), "telegram".into()]).unwrap(),
        vec!["codex", "telegram"]
    );

    assert!(super::plugin_uninstall_ids(vec!["tools".into(), "tools".into()]).is_err());
    assert!(super::plugin_uninstall_ids(vec!["bad_id".into()]).is_err());
}

#[test]
fn advisory_lock_releases_after_drop() {
    let root = test_root("lock");
    let first = daemon_lock(&root).unwrap();

    drop(first);

    assert!(daemon_lock(&root).is_ok());
}

#[cfg(unix)]
#[test]
fn aborts_process_when_staging_exceeds_limit() {
    let root = test_root("staging-limit");

    fs::create_dir_all(&root).unwrap();

    let output = command_output_limited(
        std::process::Command::new("sh").args([
            "-c",
            "printf 123456 > \"$1\"; sleep 1",
            "sh",
            root.join("payload").to_str().unwrap(),
        ]),
        Some(&root),
        5,
    );

    assert_eq!(output.unwrap_err().kind(), std::io::ErrorKind::FileTooLarge);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn offline_lock_ignores_stale_ipc_files() {
    let root = test_root("offline-lock");

    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("ipc.token"), "stale").unwrap();
    fs::write(root.join("ipc.port"), "1").unwrap();

    let lock = super::offline_lock(&root).unwrap();

    drop(lock);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn default_is_safe() {
    let config = Config::default();

    assert_eq!(config.name, "Krabb");
    assert_eq!(config.update, "prompt");
    assert_eq!(config.approval, "off");
    assert!(config.validate().is_ok());
    assert!(Config { name: "\n".into(), ..Config::default() }.validate().is_err());
    assert!(Config { update: "unsafe".into(), ..Config::default() }.validate().is_err());

    for update in ["off", "check", "auto"] {
        assert!(Config { update: update.into(), ..Config::default() }.validate().is_ok());
    }

    for approval in ["off", "prompt", "auto"] {
        assert!(Config { approval: approval.into(), ..Config::default() }.validate().is_ok());
    }

    assert!(Config { approval: "unsafe".into(), ..Config::default() }.validate().is_err());

    for id in [krabb_core::client::TUI, krabb_core::client::CLI] {
        let channels = BTreeMap::from([(id.to_owned(), crate::ChannelConfig::default())]);
        let error = Config { channels, ..Config::default() }.validate().unwrap_err();

        assert!(error.contains("reserved for a built-in client"), "{error}");
    }
}

#[test]
fn krabfile_validation_reports_the_first_error() {
    let mut file = Krabfile { version: "0.2".into(), config: Config::default(), plugins: vec![] };

    assert_eq!(file.validate().unwrap_err(), "unsupported version 0.2 (expected 0.1)");

    file.version = KRABFILE_VERSION.into();
    file.config.update = "invalid".into();

    assert!(file.validate().unwrap_err().starts_with("configuration:"));

    file.config = Config::default();

    file.plugins.push(KrabbPlugin {
        id: "Bad ID".into(),
        source: "local".into(),
        revision: String::new(),
        version: "0.1.0".into(),
        capabilities: vec![],
    });

    assert!(file.validate().unwrap_err().contains("invalid ID"));
}

#[test]
fn covers_runtime_helpers() {
    assert_eq!(super::sentence("hello"), "Hello.");
    assert_eq!(super::sentence("Already!"), "Already!");
    assert_eq!(super::sentence(""), "Unknown error.");
    assert!(super::confirm_action(true, true, "Proceed?").unwrap());
    assert!(super::confirm_action(false, true, "Proceed?").is_err());

    assert_eq!(
        super::diagnostic("https://user:secret@example.com/path"),
        "https://[redacted]@example.com/path"
    );

    assert_eq!(
        super::redact_diagnostic("Authorization: Bearer secret"),
        "Authorization: [redacted]"
    );

    for capability in [
        Capability::Model,
        Capability::Vision,
        Capability::Channel,
        Capability::Store,
        Capability::Memory,
        Capability::Scheduler,
        Capability::Tool,
        Capability::Mcp,
        Capability::Speech,
        Capability::Client,
        Capability::Resource,
        Capability::Agent,
        Capability::Runner,
    ] {
        assert!(!super::capability_name(&capability).is_empty());
    }

    for name in [
        "help",
        "init",
        "hatch",
        "doctor",
        "status",
        "version",
        "completion",
        "plugin",
        "session",
        "delivery",
        "service",
        "ask",
        "export",
        "validate",
        "import",
    ] {
        assert!(super::native_command(name));
    }

    assert!(!super::native_command("custom"));

    assert!(super::valid("memory-2"));
    assert!(!super::valid("Memory"));
    assert!(!super::valid(""));

    for approval in ["off", "prompt", "auto"] {
        let config = Config { approval: approval.into(), ..Config::default() };

        assert_eq!(config.approval_mode().enabled(), approval != "off");
    }

    assert_eq!(super::ApprovalMode::Off, super::ApprovalMode::Off);
    assert!(super::plugin_name("memory").contains("memory"));

    assert_eq!(
        super::command_label(&Command::Init(InitArgs {
            steps: Vec::new(),
            provider: None,
            channel: None,
            force: false,
            yes: false,
        })),
        "init"
    );

    assert_eq!(super::command_label(&Command::Doctor(DoctorArgs { fix: false })), "doctor");
    assert_eq!(super::command_label(&Command::Status(Output { json: false })), "status");
    assert_eq!(super::command_label(&Command::Version(Output { json: false })), "version");

    assert_eq!(
        super::command_label(&Command::Completion { shell: CompletionShell::Bash }),
        "completion"
    );

    assert_eq!(
        super::command_label(&Command::Plugin {
            command: super::PluginCommand::List(super::ListOutput { page: 1, json: false })
        }),
        "plugin"
    );

    assert_eq!(
        super::command_label(&Command::Service { command: ServiceCommand::Status }),
        "service"
    );

    assert_eq!(
        super::command_label(&Command::Export(super::KrabfileExport {
            destination: None,
            path: None,
            force: false,
        })),
        "export"
    );

    assert_eq!(
        super::command_label(&Command::Validate(super::KrabfileValidate { path: None })),
        "validate"
    );

    assert_eq!(
        super::command_label(&Command::Import(super::KrabfileImport {
            path: None,
            yes: false,
            force: false
        })),
        "import"
    );

    assert_eq!(super::command_label(&Command::External(vec!["custom".into()])), "external");

    let command = super::git_command();

    assert_eq!(command.get_program(), "git");

    assert!(
        super::command_output_limited(&mut std::process::Command::new("true"), None, 0).is_ok()
    );

    assert_eq!(super::read_output(std::io::Cursor::new(b"output")).unwrap(), b"output");

    assert!(
        super::read_output(std::io::Cursor::new(vec![b'x'; super::COMMAND_OUTPUT + 1])).is_err()
    );

    let root = test_root("helpers");
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("file"), b"123").unwrap();

    assert_eq!(super::staging_size(&root).unwrap(), 3);

    assert_eq!(
        super::staging_size(&root.join("missing")).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );

    assert!(super::read_bounded(&root.join("file"), 3).is_ok());
    assert!(super::read_bounded(&root.join("file"), 2).is_err());
    assert!(super::read_bounded(&root.join("missing"), 3).is_err());
    assert!(super::read_bounded(&root, 3).is_err());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn config_deserializes_with_defaults() {
    let config: Config = toml::from_str(
        "[channels.telegram]\nallow = [\"123\"]\n[clients.tui]\ntools = true\nshell = true\n",
    )
    .unwrap();

    assert_eq!(config.update, "prompt");
    assert_eq!(config.approval, "off");
    assert_eq!(config.channels["telegram"].allow, vec!["123"]);
    assert!(!config.channels["telegram"].shell);
    assert!(!config.channels["telegram"].worktree);
    assert!(!super::ChannelConfig::default().worktree);

    let explicit: Config = toml::from_str("[channels.discord]\nworktree = true\n").unwrap();

    assert!(explicit.channels["discord"].worktree);
    assert!(config.clients["tui"].tools);
    assert_eq!(config.clients["tui"].shell, Some(true));
    assert!(config.clients["tui"].theme);
    assert!(super::ClientConfig::default().theme);

    let channel_shell: Config =
        toml::from_str("approval = \"prompt\"\n[channels.telegram]\ntools = true\nshell = true\n")
            .unwrap();

    assert!(channel_shell.channels["telegram"].tools);
    assert!(channel_shell.channels["telegram"].shell);

    let schedule: Config =
        toml::from_str("[scheduler]\ntimezone = \"Europe/Rome\"\nmissed = \"coalesce\"\n").unwrap();

    assert!(schedule.validate().is_ok());

    let invalid_schedule: Config =
        toml::from_str("[scheduler]\ntimezone = \"Mars/Olympus\"\nmissed = \"replay\"\n").unwrap();

    assert!(invalid_schedule.validate().is_err());

    let legacy: Config = toml::from_str("shell = true\n").unwrap();

    assert!(legacy.tui_shell());
    assert!(toml::to_string(&legacy).unwrap().contains("shell = true"));

    let explicit_disabled: Config =
        toml::from_str("shell = true\n[clients.tui]\nshell = false\n").unwrap();

    assert!(!explicit_disabled.tui_shell());

    let explicit_enabled: Config =
        toml::from_str("shell = false\n[clients.tui]\nshell = true\n").unwrap();

    assert!(explicit_enabled.tui_shell());

    let monochrome: Config = toml::from_str("[clients.tui]\ntheme = false\n").unwrap();

    assert!(!monochrome.clients["tui"].theme);
    assert!(!Config::default().clients.get("tui").is_some_and(|client| client.tools));
    assert_eq!(super::ClientConfig::default().shell, None);
    assert!(toml::from_str::<Config>("unknown = true\n").is_err());
}

#[test]
fn schedule_list_shows_pending_state_and_exact_utc_time() {
    let output = super::format_schedule_list(&serde_json::json!({
        "items": [{
            "id": 7,
            "name": "Backup",
            "status": "pending",
            "due": 1_700_000_000,
            "text": "create a backup",
        }]
    }));

    assert_eq!(
        output,
        "Schedules (1–1 of 1, page 1 of 1)\n└─ 7 | Backup | pending\n   ├─ Next run: 2023-11-14 22:13:20 UTC\n   └─ Automatic retries are off"
    );

    assert_eq!(
        super::format_schedule_list(&serde_json::json!({"items": []})),
        "No schedules yet.\nAdd one with `krabb schedule add`, or ask your Krabb to schedule a task."
    );

    let local = super::format_schedule_list(&serde_json::json!({
        "items": [{
            "id": 8,
            "name": "rome-report",
            "status": "active",
            "due": 1_700_000_000,
            "cron": "0 9 * * *",
            "zone": "Europe/Rome"
        }]
    }));

    assert!(
        local.contains(
            "Next run: 2023-11-14 23:13:20 +01:00 (Europe/Rome; 2023-11-14 22:13:20 UTC)"
        )
    );
}

#[test]
fn schedule_list_shows_retry_progress_and_manual_runs() {
    let output = super::format_schedule_list(&serde_json::json!({
        "items": [{
            "id": 8,
            "name": "send-report",
            "status": "active",
            "due": 1_700_000_000,
            "text": "send report",
            "attempts": 2,
            "retries": 3,
            "retry_at": 1_700_000_010,
            "run_requested": true,
            "pause_requested": true,
            "last_outcome": "retrying",
            "last_result": "RETRYABLE: Provider is temporarily unavailable."
        }]
    }));

    assert!(output.contains("A manual run is queued"));

    assert!(
        output.contains(
            "Pause requested, and the current run will finish before the schedule pauses"
        )
    );

    assert!(output.contains("Automatic retries: 1 of 3 used"));
    assert!(output.contains("Next retry: 2023-11-14 22:13:30 UTC"));
    assert!(output.contains("Last result: failed, retry queued, details in"));
    assert!(output.contains("krabb schedule history send-report"));
    assert!(!output.contains("Provider is temporarily unavailable"));
}

#[test]
fn schedule_list_shows_fixed_repeat_intervals() {
    let output = super::format_schedule_list(&serde_json::json!({
        "items": [{
            "id": 9,
            "status": "active",
            "due": 1_700_000_000,
            "repeat": 300,
            "text": "check the backup"
        }]
    }));

    assert!(output.contains("Repeats every 5 minutes"));
}

#[test]
fn schedule_list_shows_continue_after_retries_policy() {
    let output = super::format_schedule_list(&serde_json::json!({
        "items": [{
            "id": 10,
            "status": "active",
            "due": 1_700_000_000,
            "repeat": 300,
            "on_failure": "continue",
            "text": "check the backup"
        }]
    }));

    assert!(output.contains("Continues after failed occurrences"));
}

#[test]
fn scheduled_outcomes_distinguish_retryable_uncertain_and_cancelled_runs() {
    assert_eq!(
        super::scheduled_outcome("RETRYABLE: Provider is temporarily unavailable.".into()),
        (false, false, false, "Provider is temporarily unavailable.".into())
    );

    assert_eq!(
        super::scheduled_outcome("PAUSED: A message may already have been sent.".into()),
        (false, true, false, "A message may already have been sent.".into())
    );

    assert_eq!(
        super::scheduled_outcome("CANCELLED: The action was interrupted.".into()),
        (false, false, true, "The action was interrupted.".into())
    );

    assert_eq!(
        super::scheduled_outcome(" DONE: Backup completed.".into()),
        (true, false, false, "Backup completed.".into())
    );

    assert_eq!(
            super::scheduled_outcome("Backup completed.".into()),
            (
                false,
                true,
                false,
                "The agent did not confirm task completion, so review the response before retrying. Backup completed.".into()
            )
        );

    assert!(super::scheduled_outcome("DONE:".into()).1);
    assert_eq!(super::scheduled_visible_text("DONE: Finished."), "Finished.");
    assert_eq!(super::scheduled_visible_text("A normal response."), "A normal response.");
}

#[test]
fn scheduled_setup_errors_preserve_pause_and_retry_classification() {
    assert_eq!(
        super::scheduled_error_outcome(
            "PAUSED: Configured model provider codex is not available.",
            false,
            false
        ),
        (false, true, false, "Configured model provider codex is not available.".into())
    );

    assert_eq!(
        super::scheduled_error_outcome(
            "RETRYABLE: Provider is temporarily unavailable.",
            false,
            false
        ),
        (false, false, false, "Provider is temporarily unavailable.".into())
    );

    assert!(super::scheduled_error_outcome("Tool failed.", false, true).1);
    assert!(super::scheduled_error_outcome("Tool failed.", true, false).2);
}

#[test]
fn scheduled_turn_tracks_tool_errors_even_when_the_model_can_continue() {
    let failures = Arc::new(Mutex::new(Vec::new()));

    let failed = super::track_scheduled_tool_response(
        super::Response::fail(1, -32000, "The file operation failed."),
        Some(&failures),
    );

    let succeeded = super::track_scheduled_tool_response(
        super::Response::ok(2, serde_json::json!({"text": "Read completed."})),
        Some(&failures),
    );

    assert_eq!(failed.error.unwrap().message, "The file operation failed.");
    assert!(succeeded.error.is_none());
    assert_eq!(failures.lock().unwrap().as_slice(), ["The file operation failed."]);
}

#[test]
fn scheduled_tool_failures_retry_only_before_possible_mutation() {
    assert!(
        super::scheduled_tool_failure("Tool temporarily unavailable", false)
            .starts_with("RETRYABLE:")
    );

    assert!(
        super::scheduled_tool_failure("Tool temporarily unavailable", true).starts_with("PAUSED:")
    );
}

#[test]
fn schedule_notices_distinguish_continue_from_pause() {
    assert_eq!(
        super::scheduled_result_heading("continuing"),
        "Scheduled task failed, next run remains active"
    );

    assert_eq!(super::scheduled_result_heading("paused"), "Scheduled task paused");
}

#[cfg(unix)]
#[tokio::test]
async fn pending_schedule_reports_are_recovered_and_acknowledged() {
    let root = test_root("scheduled-report-recovery");
    let home = root.join("home");
    let acknowledgement = root.join("acknowledged");

    fs::create_dir_all(home.join("data/sessions")).unwrap();
    fs::create_dir_all(home.join("logs/schedules")).unwrap();

    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}' ;;
        *'"method":"reports"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"items":[{"id":7,"run_id":9,"context":{"origin":{"channel":"tui","session":"origin"}},"finish":{"status":"completed","success":true,"finished_at":1700000000,"result":"Recovered result."}},{"id":8,"run_id":10,"context":{"origin":{"channel":"tui","session":"deleted"}},"finish":{"status":"completed","success":true,"finished_at":1700000001,"result":"Destination deleted."}}]}}' ;;
        *'"method":"report"'*) printf '%s\n' "$line" >> "ACKNOWLEDGEMENT"; printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"reported":true}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let script = script.replace("ACKNOWLEDGEMENT", &acknowledgement.display().to_string());
    let scheduler = Process::start_with("sh", ["-c", &script]).await.unwrap();
    let plugins = registry([scheduler]).await;
    let mut store = super::state::Store::load(home.join("data/sessions/index.json")).unwrap();

    store.create("origin", "model").unwrap();

    let sessions = Arc::new(Mutex::new(store));
    let stop = Stop::new();
    let mut call = 1;

    super::process_pending_schedule_reports(&plugins, &sessions, &home, &stop, &mut call).await;

    let messages = sessions.lock().unwrap().sessions["origin"].messages.clone();

    assert!(messages.iter().any(|message| {
        message.id == "schedule-notice-at-1700000000000-7-9"
                && message.content.iter().any(|content| {
                    matches!(content, Content::Text { text } if text.contains("Recovered result."))
                })
    }));

    assert!(acknowledgement.is_file());
    assert_eq!(fs::read_to_string(&acknowledgement).unwrap().lines().count(), 2);

    let missing_destination_log =
        fs::read_to_string(home.join("logs/schedules/schedule-8.jsonl")).unwrap();

    assert_eq!(missing_destination_log.lines().count(), 1);

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn skipped_schedule_falls_back_when_scheduler_has_no_reports_method() {
    let root = test_root("scheduled-legacy-skip");
    let home = root.join("home");

    fs::create_dir_all(home.join("data/sessions")).unwrap();

    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}' ;;
        *'"method":"reports"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"error":{"code":-32601,"message":"method not found"}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let scheduler = Process::start_with("sh", ["-c", script]).await.unwrap();
    let plugins = registry([scheduler]).await;
    let mut store = super::state::Store::load(home.join("data/sessions/index.json")).unwrap();

    store.create("origin", "model").unwrap();

    let sessions = Arc::new(Mutex::new(store));
    let stop = Stop::new();
    let mut call = 1;

    let task = serde_json::json!({
        "id": 17,
        "name": "legacy-task",
        "skipped": true,
        "skipped_at": 1_700_000_000,
        "context": {"origin": {"channel": "tui", "session": "origin"}}
    });

    super::process_skipped_schedule(&plugins, &sessions, &home, &task, &stop, &mut call).await;

    let messages = sessions.lock().unwrap().sessions["origin"].messages.clone();

    assert!(messages.iter().any(|message| {
            message.id == "schedule-notice-at-1700000000000-17-0"
                && message.content.iter().any(|content| {
                    matches!(content, Content::Text { text } if text.contains("skipped because the daemon was offline"))
                })
        }));

    let history = fs::read_to_string(home.join("logs/schedules/legacy-task-17.jsonl")).unwrap();

    assert!(history.contains("\"status\":\"skipped\""));

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scheduled_tui_results_remain_non_system_in_later_turns() {
    let root = test_root("scheduled-tui-notice");

    fs::create_dir_all(&root).unwrap();

    let mut store = super::state::Store::load(root.join("sessions.json")).unwrap();

    store.create("origin", "model").unwrap();

    let sessions = Arc::new(Mutex::new(store));

    let task = serde_json::json!({
        "context": {
            "origin": {
                "channel": "tui",
                "session": "origin"
            }
        }
    });

    assert_eq!(
        super::report_scheduled_result(
            &root,
            &task,
            7,
            Some(9),
            Some(1_700_000_000),
            "completed",
            "DONE: Finished.",
            &sessions
        ),
        super::ScheduleReportOutcome::Delivered
    );

    assert_eq!(
        super::report_scheduled_result(
            &root,
            &task,
            7,
            Some(9),
            Some(1_700_000_000),
            "completed",
            "DONE: Finished.",
            &sessions
        ),
        super::ScheduleReportOutcome::Delivered
    );

    assert_eq!(
        super::report_scheduled_result(
            &root,
            &task,
            8,
            Some(10),
            None,
            "completed",
            "Legacy scheduler report.",
            &sessions
        ),
        super::ScheduleReportOutcome::Delivered
    );

    let history = sessions.lock().unwrap().sessions["origin"].messages.clone();

    assert_eq!(
        history
            .iter()
            .filter(|message| message.id == "schedule-notice-at-1700000000000-7-9")
            .count(),
        1
    );

    assert!(history.iter().any(|message| message.id == "schedule-notice-8-10"));

    let messages = turn_messages("origin", &root, history);
    let notice = messages.iter().find(|message| message.id.starts_with("schedule-notice-"));

    assert_eq!(notice.unwrap().role, Role::Assistant);

    drop(sessions);

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn scheduled_model_tool_errors_are_retained_after_a_normal_final_reply() {
    let provider_script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"codex","version":"0.1.0","capabilities":["model"]}}' ;;
        *'"method":"generate"'*)
            printf '%s\n' '{"jsonrpc":"2.0","id":77,"method":"host/tool","params":{"name":"read","args":{"path":"missing.txt"}}}'
            IFS= read -r tool_reply
            printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"text":"I could not read the file.","stop":"stop"}}'
            ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let tools_script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"tools","version":"0.1.0","capabilities":["tool"]}}' ;;
        *'"method":"file_read"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"error":{"code":-32000,"message":"The requested file is missing."}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let provider = Process::start_with("sh", ["-c", provider_script]).await.unwrap();
    let tools = Process::start_with("sh", ["-c", tools_script]).await.unwrap();
    let plugins = registry([provider, tools]).await;
    let provider = plugins.get("codex").await.unwrap();
    let root = test_root("scheduled-tool-error");

    fs::create_dir_all(&root).unwrap();

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    sessions.lock().unwrap().ensure_schedule("schedule-71", "test").unwrap();

    let stop = Stop::new();
    let mut call = 1;
    let mut failed_tool = None;
    let tool_failures = Arc::new(Mutex::new(Vec::new()));
    let (notices, mut events) = tokio::sync::mpsc::channel(32);
    let drain = tokio::spawn(async move { while events.recv().await.is_some() {} });

    let reply = answer(
        &provider,
        &plugins,
        "test",
        vec![Message {
            id: "scheduled-user".into(),
            session: "schedule-71".into(),
            role: Role::User,
            sender: Some("scheduler".into()),
            content: vec![Content::Text { text: "Read the file.".into() }],
        }],
        "schedule-71",
        "scheduler",
        "71",
        None,
        &sessions,
        Some(&root),
        &root,
        true,
        false,
        false,
        super::ApprovalMode::Auto,
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        &Cancellation::new(),
        &stop,
        tokio::time::Instant::now() + super::TURN_LIMIT,
        &mut call,
        &mut failed_tool,
        Some(Arc::clone(&tool_failures)),
        notices,
        None,
        Some(1),
        None,
    )
    .await
    .unwrap();

    assert_eq!(reply.text, "I could not read the file.");
    assert_eq!(tool_failures.lock().unwrap().as_slice(), ["The requested file is missing."]);

    assert!(
        super::scheduled_tool_failure(&tool_failures.lock().unwrap()[0], false)
            .starts_with("RETRYABLE:")
    );

    drain.abort();
    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[test]
fn schedule_add_parses_bounded_retries_and_missed_policy() {
    let root = test_root("schedule-add-options");

    fs::create_dir_all(&root).unwrap();

    let (params, instruction) = super::parse_schedule_add(
        &[
            "--after".into(),
            "300".into(),
            "--retries".into(),
            "2".into(),
            "--every".into(),
            "300".into(),
            "--on-failure".into(),
            "continue".into(),
            "back up the workspace".into(),
        ],
        &root,
    )
    .unwrap();

    assert_eq!(params["retries"], 2);
    assert_eq!(params["on_failure"], "continue");
    assert_eq!(instruction, "back up the workspace");
    assert!(params["context"]["provider"].is_null());
    assert!(params["context"]["model"].is_null());
    assert_eq!(params["context"]["shell"], false);
    assert!(params["context"]["message_destination"].is_null());

    let (messaging, _) = super::parse_schedule_add(
        &[
            "--after".into(),
            "300".into(),
            "--message-channel".into(),
            "telegram".into(),
            "--message-chat".into(),
            "12345".into(),
            "--message-kind".into(),
            "private".into(),
            "--message-thread".into(),
            "topic-1".into(),
            "send the report".into(),
        ],
        &root,
    )
    .unwrap();

    assert_eq!(
        messaging["context"]["message_destination"],
        serde_json::json!({
            "channel": "telegram",
            "chat": "12345",
            "private": true,
            "thread": "topic-1"
        })
    );

    let (tui_message, _) = super::parse_schedule_add(
        &["--after".into(), "5m".into(), "ping me on default tui chat".into()],
        &root,
    )
    .unwrap();

    assert_eq!(
        tui_message["context"]["message_destination"],
        serde_json::json!({
            "channel": "tui",
            "chat": "tui-default",
            "private": false,
            "thread": null
        })
    );

    let (named_tui_message, _) = super::parse_schedule_add(
        &[
            "--after".into(),
            "5m".into(),
            "--message-channel".into(),
            "tui".into(),
            "--message-chat".into(),
            "tui-default".into(),
            "--message-kind".into(),
            "group".into(),
            "say hello".into(),
        ],
        &root,
    )
    .unwrap();

    assert_eq!(named_tui_message["context"]["message_destination"]["channel"], "tui");

    assert!(!super::valid_schedule_name("my schedule"));
    assert!(super::valid_schedule_name("mighty-raccoon"));

    for args in [
        vec![
            "--after".into(),
            "300".into(),
            "--message-channel".into(),
            "telegram".into(),
            "send the report".into(),
        ],
        vec![
            "--after".into(),
            "300".into(),
            "--message-channel".into(),
            "telegram".into(),
            "--message-chat".into(),
            "12345".into(),
            "--message-kind".into(),
            "direct".into(),
            "send the report".into(),
        ],
        vec![
            "--after".into(),
            "300".into(),
            "--message-thread".into(),
            "topic-1".into(),
            "send the report".into(),
        ],
        vec![
            "--after".into(),
            "300".into(),
            "--message-channel".into(),
            "telegram".into(),
            "--message-chat".into(),
            "12345".into(),
            "--message-kind".into(),
            "private".into(),
            "--message-thread".into(),
            " ".into(),
            "send the report".into(),
        ],
    ] {
        assert!(super::parse_schedule_add(&args, &root).is_err());
    }

    fs::write(root.join("config.toml"), "[clients.tui]\nshell = true\n").unwrap();

    let (without_shell, _) = super::parse_schedule_add(
        &["--after".into(), "300".into(), "run a workspace task".into()],
        &root,
    )
    .unwrap();

    assert_eq!(without_shell["context"]["shell"], false);

    let shell_root = test_root("schedule-add-shell");

    let binary =
        shell_root.join("plugins/tools/bin").join(krabb_core::plugin::binary_name("tools"));

    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, "test plugin binary").unwrap();

    fs::write(
        shell_root.join("plugins.lock"),
        serde_json::json!({
            "plugins": {
                "tools": {
                    "source": "local",
                    "version": "0.1.0",
                    "protocol": {"major": 0, "minor": 1},
                    "capabilities": ["tool"]
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    assert!(
        super::parse_schedule_add(
            &["--after".into(), "300".into(), "--shell".into(), "run backup".into()],
            &shell_root,
        )
        .is_err()
    );

    fs::write(shell_root.join("config.toml"), "[clients.tui]\nshell = true\n").unwrap();

    let (shell, _) = super::parse_schedule_add(
        &["--after".into(), "300".into(), "--shell".into(), "run a local backup".into()],
        &shell_root,
    )
    .unwrap();

    assert_eq!(shell["context"]["tools"], true);
    assert_eq!(shell["context"]["shell"], true);

    assert!(
        super::parse_schedule_add(
            &["--after".into(), "300".into(), "--shell".into(), "run backup".into()],
            &root,
        )
        .is_err()
    );

    let _ = fs::remove_dir_all(shell_root);

    let (interval, _) = super::parse_schedule_add(
        &["--every".into(), "300".into(), "--after".into(), "60".into(), "check the backup".into()],
        &root,
    )
    .unwrap();

    assert_eq!(interval["delay"], 60);
    assert_eq!(interval["repeat"], 300);

    let (interval_start, _) = super::parse_schedule_add(
        &["--every".into(), "300".into(), "check the backup".into()],
        &root,
    )
    .unwrap();

    assert_eq!(interval_start["delay"], 300);
    assert_eq!(interval_start["repeat"], 300);

    let run_at = "2099-10-20T09:00:00+02:00".parse::<jiff::Timestamp>().unwrap();

    let (exact, _) = super::parse_schedule_add(
        &[
            "--at".into(),
            run_at.to_string(),
            "--missed".into(),
            "replay".into(),
            "send the report".into(),
        ],
        &root,
    )
    .unwrap();

    assert_eq!(exact["at"], run_at.as_second() as u64);
    assert_eq!(exact["missed"], "replay");

    let (local, _) = super::parse_schedule_add(
        &[
            "--at".into(),
            "2099-10-20 09:00".into(),
            "--timezone".into(),
            "UTC".into(),
            "--provider".into(),
            "codex".into(),
            "--model".into(),
            "gpt-6-luna".into(),
            "send the report".into(),
        ],
        &root,
    )
    .unwrap();

    assert_eq!(
        local["at"],
        "2099-10-20T09:00:00Z".parse::<jiff::Timestamp>().unwrap().as_second() as u64
    );

    assert!(
        super::parse_schedule_add(
            &[
                "--cron".into(),
                "0 9 * * *".into(),
                "--timezone".into(),
                "Europe/Rome".into(),
                "--missed".into(),
                "replay".into(),
                "--retries".into(),
                "6".into(),
                "check".into(),
            ],
            &root,
        )
        .is_err()
    );

    assert!(
        super::parse_schedule_add(
            &[
                "--after".into(),
                "300".into(),
                "--on-failure".into(),
                "continue".into(),
                "one-time task".into(),
            ],
            &root,
        )
        .is_err()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn schedule_add_accepts_named_human_durations_and_options_after_instruction() {
    let root = test_root("schedule-add-human-duration");

    fs::create_dir_all(&root).unwrap();

    let args = [
        "run a backup".into(),
        "--after".into(),
        "1m30s".into(),
        "--every".into(),
        "1mo".into(),
        "--provider".into(),
        "codex".into(),
        "--model".into(),
        "gpt-6-luna".into(),
        "--name".into(),
        "Backup".into(),
    ];

    let (params, instruction) = super::parse_schedule_add(&args, &root).unwrap();

    assert_eq!(instruction, "run a backup");
    assert_eq!(params["delay"], 90);
    assert_eq!(params["repeat"], 2_592_000);
    assert_eq!(params["context"]["provider"], "codex");
    assert_eq!(params["context"]["model"], "gpt-6-luna");
    assert_eq!(params["name"], "Backup");

    let month = super::parse_schedule_add(
        &["--after".into(), "1 month".into(), "wait one month".into()],
        &root,
    )
    .unwrap();

    assert_eq!(month.0["delay"], 2_592_000);

    let missing_model = super::parse_schedule_add(
        &["--after".into(), "5m".into(), "--provider".into(), "codex".into(), "run a check".into()],
        &root,
    )
    .unwrap();

    assert!(missing_model.0["context"].get("model").is_none());

    let missing_provider = super::parse_schedule_add(
        &[
            "--after".into(),
            "5m".into(),
            "--model".into(),
            "gpt-6-luna".into(),
            "run a check".into(),
        ],
        &root,
    )
    .unwrap();

    assert!(missing_provider.0["context"].get("provider").is_none());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn captured_model_selection_is_stored_in_schedule_context() {
    let mut context = serde_json::json!({"workspace": "/workspace"});

    super::capture_model_selection(&mut context, Some("anthropic".into()), Some("sonnet".into()));

    assert_eq!(context["provider"], "anthropic");
    assert_eq!(context["model"], "sonnet");
    assert_eq!(context["workspace"], "/workspace");

    super::capture_model_selection(&mut context, None, None);

    assert_eq!(context["provider"], "anthropic");
    assert_eq!(context["model"], "sonnet");
}

#[test]
fn session_model_uses_the_requested_or_configured_model() {
    let configured = std::env::var("KRABB_MODEL").unwrap_or_else(|_| "default".into());

    let root = test_root("session-defaults");

    assert_eq!(super::session_model(None, &root).unwrap(), configured);
    assert_eq!(super::session_model(Some("explicit".into()), &root).unwrap(), "explicit");
}

#[test]
fn scheduled_policy_explores_the_machine_only_with_enabled_shell_access() {
    let workspace = Path::new("/home/airscript/repos/personal/krabb");
    let origin = &serde_json::json!({"channel": "cli", "session": ""});
    let confined = super::scheduled_policy(workspace, origin, false);

    assert!(confined.contains("captured workspace root is /home/airscript/repos/personal/krabb"));

    assert!(confined.contains("Shell access is unavailable"));
    assert!(!confined.contains("explore the daemon machine"));

    let host_discovery = super::scheduled_policy(workspace, origin, true);

    assert!(host_discovery.contains("explore the daemon machine"));
    assert!(host_discovery.contains("folder, file, or binary"));
    assert!(host_discovery.contains("read-only discovery commands"));
    assert!(host_discovery.contains("search by name across other readable locations"));

    assert!(
        host_discovery
            .contains("unless the scheduled instruction explicitly requests that specific change")
    );

    assert!(host_discovery.contains("do not read credentials"));
    assert!(host_discovery.contains("do not append its name again"));
}

#[test]
fn scheduled_turn_starts_with_only_the_current_instruction() {
    let root = test_root("scheduled-fresh-context");

    fs::create_dir_all(&root).unwrap();

    let instruction = Message {
        id: "schedule-user-current".into(),
        session: "schedule-7".into(),
        role: Role::User,
        sender: Some("scheduler".into()),
        content: vec![Content::Text { text: "Recreate the deleted report.".into() }],
    };

    let messages = super::scheduled_messages(
        7,
        "schedule-7",
        &root,
        instruction,
        Path::new("/workspace"),
        &serde_json::json!({"channel": "cli"}),
        false,
    );

    let user_messages =
        messages.iter().filter(|message| message.role == Role::User).collect::<Vec<_>>();

    assert_eq!(user_messages.len(), 1);

    assert!(user_messages[0].content.iter().any(
            |content| matches!(content, Content::Text { text } if text == "Recreate the deleted report.")
        ));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn cli_schedule_workspace_is_canonical_current_directory() {
    let current_directory = std::env::current_dir().unwrap().canonicalize().unwrap();

    assert_eq!(super::current_schedule_workspace().unwrap(), current_directory);
}

#[test]
fn schedule_ids_remain_unique_and_below_the_integer_limit() {
    let first = super::schedule_id().unwrap();
    let second = super::schedule_id().unwrap();

    assert!(first <= i64::MAX as u64);
    assert!(second <= i64::MAX as u64);
    assert_ne!(first, second);
}

#[test]
fn schedule_logs_use_name_and_id_and_preserve_legacy_history() {
    let home = test_root("schedule-log-paths");
    let directory = home.join("logs/schedules");

    fs::create_dir_all(&directory).unwrap();

    assert_eq!(super::schedule_log_filename("mighty-raccoon", 42), "mighty-raccoon-42.jsonl");

    let legacy = directory.join("42.jsonl");

    fs::write(&legacy, "{\"status\":\"completed\"}\n").unwrap();

    assert_eq!(super::schedule_log_path(&home, 42).unwrap(), legacy);

    let task = serde_json::json!({
        "id": 42,
        "name": "mighty-raccoon",
        "zone": "Europe/Rome",
        "text": "Send the daily report.",
        "context": {
            "provider": "codex",
            "model": "gpt-6-luna",
            "workspace": "/work/reports"
        },

        "attempt": 1,
        "scheduled_for": 1_800_000_000,
        "missed": "skip",
        "manual": false,
        "retry": false
    });

    let finish = serde_json::json!({
        "success": true,
        "status": "completed",
        "attempts": 1,
        "retries": 2,
        "scheduled_for": 1_800_000_000
    });

    for run_id in 1..=105 {
        let mut run = task.clone();

        run["run_id"] = serde_json::json!(run_id);

        super::scheduler_log(
            &home,
            &run,
            "completed",
            "done\nAuthorization: Bearer secret",
            Some(&finish),
        );
    }

    let named = directory.join("mighty-raccoon-42.jsonl");
    let content = fs::read_to_string(&named).unwrap();
    let lines = content.lines().collect::<Vec<_>>();

    assert_eq!(lines.len(), 106);

    let record: serde_json::Value = serde_json::from_str(lines[1]).unwrap();

    assert_eq!(record["schema_version"], 1);
    assert_eq!(record["schedule"]["name"], "mighty-raccoon");
    assert_eq!(record["schedule"]["timezone"], "Europe/Rome");
    assert_eq!(record["run"]["id"], 1);
    assert_eq!(record["run"]["status"], "completed");
    assert_eq!(record["run"]["success"], true);
    assert_eq!(record["run"]["scheduled_for"], 1_800_000_000_u64);
    assert_eq!(record["agent"]["provider"], "codex");
    assert_eq!(record["agent"]["model"], "gpt-6-luna");

    let mut duplicate = task.clone();

    duplicate["run_id"] = serde_json::json!(1);
    super::scheduler_log(&home, &duplicate, "completed", "done", Some(&finish));

    assert_eq!(fs::read_to_string(&named).unwrap().lines().count(), 106);
    assert_eq!(record["agent"]["workspace"], "/work/reports");
    assert_eq!(record["result"], "done\nAuthorization: [redacted]");
    assert!(!legacy.exists());
    assert_eq!(super::schedule_log_path(&home, 42).unwrap(), named);

    let _ = fs::remove_dir_all(home);
}

#[test]
fn schedule_history_rotation_bounds_file_and_duplicate_scan() {
    let home = test_root("schedule-history-rotation");
    let directory = home.join("logs/schedules");

    fs::create_dir_all(&directory).unwrap();

    let path = directory.join("42.jsonl");
    let old_record = format!("{{\"payload\":\"{}\"}}\n", "x".repeat(200));
    let history = old_record.repeat(2_000);

    fs::write(&path, history).unwrap();

    let task = serde_json::json!({
        "id": 42,
        "run_id": 9_999,
        "name": "mighty-raccoon",
        "zone": "UTC",
        "context": {"provider": "codex", "model": "test", "workspace": "/work"}
    });

    super::scheduler_log(&home, &task, "completed", "done", None);

    let path = directory.join("mighty-raccoon-42.jsonl");
    let metadata = fs::metadata(&path).unwrap();
    let tail = super::schedule_history_tail(&path).unwrap();

    assert!(metadata.len() <= super::SCHEDULE_HISTORY_LIMIT);
    assert!(tail.len() as u64 <= super::SCHEDULE_HISTORY_LIMIT);
    assert!(String::from_utf8(tail).unwrap().contains("9999"));

    let _ = fs::remove_dir_all(home);
}

#[test]
fn schedule_history_without_log_directory_is_empty() {
    let home = test_root("schedule-history-missing-directory");
    let path = super::schedule_log_path(&home, 42).unwrap();

    assert_eq!(path, home.join("logs/schedules/42.jsonl"));
    assert_eq!(super::read_schedule_history_page(&path, 1).unwrap(), (Vec::new(), 0));

    let _ = fs::remove_dir_all(home);
}

#[test]
fn schedule_history_is_readable_recent_first_and_links_raw_jsonl() {
    let records = (1..=22)
        .map(|run_id| {
            serde_json::json!({
                "timestamp": "2023-11-14T22:13:20Z",
                "schedule": {
                    "id": 42,
                    "name": "mighty-raccoon",
                    "timezone": "Europe/Rome"
                },

                "run": {
                    "id": run_id,
                    "status": "completed",
                    "scheduled_for": 1_700_000_000,
                    "attempt": 1,
                    "retry_limit": 2
                },

                "agent": {
                    "provider": "codex",
                    "model": "gpt-6-luna",
                    "workspace": if run_id == 22 {
                        "/work/\u{1b}[2Jreports—folder"
                    } else {
                        "/work/reports"
                    }
                },

                "result": if run_id == 22 {
                    "result-22\ncontinued\t\u{1b}[31m—unsafe\u{1b}[0m".to_owned()
                } else {
                    format!("result-{run_id}\ncontinued")
                }
            })
        })
        .collect::<Vec<_>>();

    let jsonl = records.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
    let parsed = super::parse_schedule_history(&jsonl).unwrap();
    let home = test_root("schedule-history");
    let path = home.join("mighty-raccoon-42.jsonl");

    fs::create_dir_all(&home).unwrap();
    fs::write(&path, &jsonl).unwrap();

    let (recent, total) = super::read_schedule_history_page(&path, 1).unwrap();
    let output = super::format_schedule_history(&recent, total, &path, "42", 1, false);

    assert!(output.contains("Schedule history: mighty-raccoon"));
    assert!(output.contains("Runs | 1–5 of 22 | page 1/5 (newest first)"));
    assert!(output.contains("├─ Run 22 | completed successfully"));
    assert!(output.contains("Result: result-22 continued [31m-unsafe[0m"));
    assert!(output.contains("Recorded: 2023-11-14 22:13:20 UTC"));

    assert!(output.contains(
        "Scheduled for: 2023-11-14 23:13:20 +01:00 (Europe/Rome; 2023-11-14 22:13:20 UTC)"
    ));

    assert!(output.contains("Agent: codex (gpt-6-luna)"));
    assert!(output.contains("Workspace: /work/[2Jreports-folder"));
    assert!(output.contains(&format!("Raw JSONL: {}", path.display())));

    assert!(output.contains("--raw` to print the raw log"));
    assert!(!output.contains("result-1 continued"));
    assert!(!output.contains('—'));
    assert!(!output.contains('\t'));
    assert!(!output.contains('\u{1b}'));

    let all = super::format_schedule_history(&parsed, parsed.len(), &path, "42", 1, true);
    let (older, _) = super::read_schedule_history_page(&path, 5).unwrap();
    let older_page = super::format_schedule_history(&older, total, &path, "42", 5, false);

    assert!(all.contains("Result: result-1 continued"));
    assert!(all.find("Run 22").unwrap() < all.find("Run 1 |").unwrap());
    assert!(older_page.find("Run 2 |").unwrap() < older_page.find("Run 1 |").unwrap());
    assert_eq!(recent.len(), 5);

    let _ = fs::remove_file(path);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn schedule_history_reports_malformed_jsonl_line_numbers() {
    let error = super::parse_schedule_history("{}\nnot-json\n").unwrap_err();

    assert!(error.contains("line 2"));
}

#[test]
fn schedule_history_ignores_a_torn_final_record() {
    let home = test_root("schedule-history-torn-final-record");
    let path = home.join("history.jsonl");
    let valid = r#"{"run":{"id":1}}"#;

    fs::create_dir_all(&home).unwrap();
    fs::write(&path, format!("{valid}\n{{\"run\":")).unwrap();

    let (records, total) = super::read_schedule_history_page(&path, 1).unwrap();

    assert_eq!(total, 1);
    assert_eq!(records[0]["run"]["id"], 1);
    assert_eq!(super::parse_schedule_history("{}\n{\"run\":").unwrap().len(), 1);

    let _ = fs::remove_dir_all(home);
}

#[test]
fn streamed_schedule_history_bounds_record_size() {
    let home = test_root("schedule-history-large-record");
    let path = home.join("history.jsonl");

    fs::create_dir_all(&home).unwrap();
    fs::write(&path, vec![b'x'; super::SCHEDULE_HISTORY_LINE_LIMIT + 1]).unwrap();

    let error = super::read_schedule_history_page(&path, 1).unwrap_err();

    assert!(error.contains("64 KiB limit"));

    let _ = fs::remove_file(path);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn schedule_history_without_a_log_is_an_empty_history() {
    let path = test_root("schedule-history-not-created").join("history.jsonl");
    let (records, total) = super::read_schedule_history_page(&path, 1).unwrap();
    let output = super::format_schedule_history(&records, total, &path, "42", 1, false);

    assert!(records.is_empty());
    assert_eq!(total, 0);
    assert!(output.contains("No execution history has been recorded."));
    assert!(output.contains(&format!("Raw JSONL: {}", path.display())));
}

#[test]
fn schedule_history_handles_legacy_outcome_records() {
    let records = super::parse_schedule_history(r#"{"status":"completed"}"#).unwrap();

    let output = super::format_schedule_history(
        &records,
        records.len(),
        Path::new("history.jsonl"),
        "42",
        1,
        false,
    );

    assert!(output.contains("└─ Run | completed successfully"));
    assert!(output.contains("└─ Result: (no result text)"));
}

#[cfg(unix)]
#[tokio::test]
async fn scheduler_rpc_stops_waiting_when_daemon_shutdown_is_signaled() {
    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let process = Process::start_with("sh", ["-c", script]).await.unwrap();
    let plugins = registry([process]).await;
    let (_, scheduler) = plugins.find(Capability::Scheduler).await.unwrap();
    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);

    let notifier = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        signal.signal();
    });

    let result = super::scheduler_call(&scheduler, 1, "claim", serde_json::json!({}), &stop).await;

    assert!(result.is_none());

    notifier.await.unwrap();
    stop_registry(&plugins).await;
}

#[cfg(unix)]
#[tokio::test]
async fn scheduled_finish_retries_a_transient_save_failure_without_rerunning_work() {
    let script = r#"
finished=0
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}' ;;
        *'"method":"finish"'*)
            if [ "$finished" = 0 ]; then
                finished=1
                printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"error":{"code":-32000,"message":"temporary save failure"}}'
            else
                printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"finished":true,"status":"completed"}}'
            fi ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let process = Process::start_with("sh", ["-c", script]).await.unwrap();
    let plugins = registry([process]).await;
    let stop = Stop::new();
    let mut call = 10;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(4),
        super::retry_schedule_finish(
            &plugins,
            42,
            serde_json::json!({"id": 42, "run_id": 7, "success": true, "result": "done"}),
            &stop,
            &mut call,
        ),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(result.1.result.unwrap()["status"], "completed");
    assert_eq!(call, 12);

    stop_registry(&plugins).await;
}

#[cfg(unix)]
#[tokio::test]
async fn completed_schedule_waits_for_a_restarting_scheduler_plugin() {
    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let first = Process::start_with("sh", ["-c", script]).await.unwrap();
    let plugins = registry([first]).await;
    let stopped = plugins.remove("scheduler").await.unwrap();

    stopped.stop().await.unwrap();

    let waiting_plugins = plugins.clone();
    let waiting_stop = super::Stop::new();

    let waiting =
        tokio::spawn(
            async move { super::wait_for_scheduler(&waiting_plugins, &waiting_stop).await },
        );

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let restarted = Process::start_with("sh", ["-c", script]).await.unwrap();

    plugins.insert(super::Live::new(restarted)).await;

    let scheduler = tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(scheduler.0, "scheduler");

    stop_registry(&plugins).await;
}

#[cfg(unix)]
#[tokio::test]
async fn scheduler_recovers_again_after_the_plugin_process_restarts_in_place() {
    let root = test_root("scheduler-plugin-recovery");
    let home = root.join("home");

    fs::create_dir_all(home.join("data/sessions")).unwrap();
    fs::create_dir_all(home.join("logs/schedules")).unwrap();

    let log = root.join("recoveries.log");

    let script = format!(
        r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"protocol":{{"major":0,"minor":1}},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}}}' ;;
        *'"method":"recover"'*) printf '%s\n' recovered >> '{}'; printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"recovered":0}}}}' ;;
        *'"method":"reports"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"items":[]}}}}' ;;
        *'"method":"claim"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"item":null}}}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"ok":true}}}}'; exit 0 ;;
    esac
done
"#,
        log.display()
    );

    let process = Process::start_with("sh", ["-c", script.as_str()]).await.unwrap();
    let plugins = registry([process]).await;

    let sessions = Arc::new(Mutex::new(
        super::state::Store::load(home.join("data/sessions/index.json")).unwrap(),
    ));

    let stop = Arc::new(Stop::new());
    let scheduler_plugins = plugins.clone();
    let scheduler_stop = Arc::clone(&stop);
    let scheduler_home = home.clone();
    let scheduler_root = root.clone();
    let scheduler_sessions = Arc::clone(&sessions);

    let run = tokio::spawn(async move {
        super::run_scheduler(
            scheduler_plugins,
            scheduler_sessions,
            scheduler_stop,
            scheduler_home,
            super::Config::default(),
            scheduler_root.join("workspace"),
            scheduler_root.join("media"),
            "codex".into(),
            "test".into(),
            Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        )
        .await;
    });

    let restarted_plugins = plugins.clone();
    let replacement_stop = Arc::clone(&stop);
    let replacement_log = log.clone();

    let restart = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if fs::read_to_string(&replacement_log)
                    .is_ok_and(|contents| contents.lines().count() >= 1)
                {
                    break;
                }

                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the first scheduler process should be recovered");

        restarted_plugins.get("scheduler").await.unwrap().restart().await.unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if fs::read_to_string(&replacement_log)
                    .is_ok_and(|contents| contents.lines().count() >= 2)
                {
                    break;
                }

                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the restarted scheduler process should be recovered");

        replacement_stop.signal();
    });

    restart.await.unwrap();
    run.await.unwrap();

    assert_eq!(fs::read_to_string(log).unwrap().lines().count(), 2);

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn scheduler_retries_recovery_after_rpc_and_claim_errors() {
    let root = test_root("scheduler-recovery-rpc-errors");
    let home = root.join("home");

    fs::create_dir_all(home.join("data/sessions")).unwrap();
    fs::create_dir_all(home.join("logs/schedules")).unwrap();

    let log = root.join("scheduler.log");

    let script = format!(
        r#"
recoveries=0
claims=0
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"protocol":{{"major":0,"minor":1}},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}}}' ;;
        *'"method":"recover"'*)
            recoveries=$((recoveries + 1))
            printf '%s\n' recover >> '{}'
            if [ "$recoveries" = 1 ]; then
                printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"error":{{"code":-32000,"message":"temporary recovery failure"}}}}'
            else
                printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"recovered":0}}}}'
            fi ;;
        *'"method":"reports"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"items":[]}}}}' ;;
        *'"method":"claim"'*)
            claims=$((claims + 1))
            printf '%s\n' claim >> '{}'
            if [ "$claims" = 1 ]; then
                printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"error":{{"code":-32000,"message":"temporary claim failure"}}}}'
            else
                printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"item":null}}}}'
            fi ;;
        *'"method":"shutdown"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"ok":true}}}}'; exit 0 ;;
    esac
done
"#,
        log.display(),
        log.display()
    );

    let process = Process::start_with("sh", ["-c", script.as_str()]).await.unwrap();
    let plugins = registry([process]).await;

    let sessions = Arc::new(Mutex::new(
        super::state::Store::load(home.join("data/sessions/index.json")).unwrap(),
    ));

    let stop = Arc::new(Stop::new());
    let scheduler_plugins = plugins.clone();
    let scheduler_stop = Arc::clone(&stop);
    let scheduler_home = home.clone();
    let scheduler_root = root.clone();

    let run = tokio::spawn(async move {
        super::run_scheduler(
            scheduler_plugins,
            sessions,
            scheduler_stop,
            scheduler_home,
            super::Config::default(),
            scheduler_root.join("workspace"),
            scheduler_root.join("media"),
            "codex".into(),
            "test".into(),
            Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        )
        .await;
    });

    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if fs::read_to_string(&log).is_ok_and(|contents| contents.lines().count() >= 5) {
                break;
            }

            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("scheduler should recover after recovery and claim errors");

    stop.signal();
    run.await.unwrap();

    assert_eq!(
        fs::read_to_string(log).unwrap().lines().collect::<Vec<_>>(),
        ["recover", "recover", "claim", "recover", "claim"]
    );

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn scheduled_cancellation_reaps_event_and_host_tool_commands() {
    let script = r#"
import json, os, subprocess, sys
role, root, mode = sys.argv[1:]
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request['method']
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': role,
                  'version': '0.1.0', 'capabilities': ['model' if role == 'fixture' else 'tool']}
    elif method == 'generate':
        args = {'command': 'touch started; sleep 2; touch leaked'}
        if mode == 'host':
            print(json.dumps({'jsonrpc': '2.0', 'id': 77, 'method': 'host/tool',
                              'params': {'name': 'shell', 'args': args}}), flush=True)
            continue
        result = {'text': '', 'stop': 'tool', 'events': [
            {'kind': 'tool', 'name': 'shell', 'args': args, 'approve': True}]}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'shell', 'description': 'Run a fixture command.',
                  'schema': {'type': 'object', 'properties': {'command': {'type': 'string'}},
                             'required': ['command'], 'additionalProperties': False}}]}
    elif method == 'shell':
        assert request['params']['approve'] is True
        subprocess.run(request['params']['command'], shell=True, cwd=root, preexec_fn=os.setpgrp)
        result = {'text': 'finished'}
    elif method == 'shutdown':
        break
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

    for mode in ["event", "host"] {
        let root = test_root(&format!("schedule-cancel-{mode}"));

        fs::create_dir_all(&root).unwrap();

        let plugins = Plugins::default();

        for role in ["fixture", "tools"] {
            let process = Process::start_with(
                "python3",
                ["-u", "-c", script, role, root.to_str().unwrap(), mode],
            )
            .await
            .unwrap();

            plugins.insert(Live::new(process)).await;
        }

        let sessions =
            Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

        let cancellation = Arc::new(Cancellation::new());

        let task = serde_json::json!({
            "text": "Run the command.",
            "context": {"tools": true, "shell": true}
        });

        let stop = Stop::new();
        let config = super::Config { shell: true, ..super::Config::default() };
        let mut call = 10;

        let result = super::execute_scheduled_task(
            71,
            &task,
            &plugins,
            &sessions,
            &stop,
            &root,
            &root,
            &root,
            "fixture",
            "test",
            Arc::clone(&cancellation),
            Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
            &config,
            Some(1),
            &mut call,
        );

        tokio::pin!(result);

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    result = &mut result => panic!("schedule finished early: {result:?}"),

                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
                        if root.join("started").exists() { break; }
                    }
                }
            }
        })
        .await
        .unwrap();

        cancellation.request();

        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), result).await.unwrap().unwrap();

        assert!(result.starts_with("CANCELLED:"), "{result}");
        assert!(sessions.lock().unwrap().sessions["schedule-71"].inflight.is_none());

        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;

        assert!(!root.join("leaked").exists(), "{mode} command survived cancellation");

        stop_registry(&plugins).await;
        fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn running_schedule_observes_a_persisted_cancellation_request() {
    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}' ;;
        *'"method":"cancelled"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"cancelled":true}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let process = Process::start_with("sh", ["-c", script]).await.unwrap();
    let plugins = registry([process]).await;
    let cancellation = Arc::new(Cancellation::new());
    let stop = Arc::new(Stop::new());

    let monitor = tokio::spawn(super::monitor_schedule_cancellation(
        plugins.clone(),
        42,
        Arc::clone(&cancellation),
        stop,
        1,
    ));

    tokio::time::timeout(std::time::Duration::from_secs(3), cancellation.cancelled())
        .await
        .unwrap();

    monitor.await.unwrap();

    stop_registry(&plugins).await;
}

#[tokio::test]
async fn scheduled_workspace_locks_serialize_shared_paths_only() {
    let root = test_root("scheduled-workspace-locks");

    fs::create_dir_all(root.join("shared")).unwrap();
    fs::create_dir_all(root.join("other")).unwrap();

    let locks = Arc::new(super::AsyncMutex::new(super::HashMap::new()));
    let first = super::lock_workspace(&locks, root.join("shared")).await;

    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            super::lock_workspace(&locks, root.join("shared")),
        )
        .await
        .is_err()
    );

    let other = tokio::time::timeout(
        std::time::Duration::from_millis(20),
        super::lock_workspace(&locks, root.join("other")),
    )
    .await
    .expect("a different workspace should remain available");

    drop(first);
    drop(other);

    let second = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        super::lock_workspace(&locks, root.join("shared")),
    )
    .await
    .expect("the shared workspace should become available after release");

    drop(second);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scheduled_tasks_use_the_captured_directory_as_their_workspace_root() {
    let root = test_root("scheduled-captured-workspace");
    let configured = root.join("configured");
    let captured = root.join("repos/personal/krabb");

    fs::create_dir_all(&configured).unwrap();
    fs::create_dir_all(&captured).unwrap();

    let workspace = super::scheduled_workspace(
        &serde_json::json!({"workspace": captured.display().to_string()}),
        &configured,
    )
    .unwrap();

    assert_eq!(workspace, fs::canonicalize(&captured).unwrap());

    assert!(
        super::scheduled_workspace(
            &serde_json::json!({"workspace": root.join("missing").display().to_string()}),
            &configured,
        )
        .is_err()
    );

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn due_scheduled_tasks_run_concurrently_with_a_bounded_worker_pool() {
    let root = test_root("scheduled-concurrency");
    let home = root.join("home");
    let slow_workspace = root.join("workspace/slow");
    let fast_workspace = root.join("workspace/fast");
    let slow_started = root.join("slow-started");
    let overlap = root.join("overlap");

    fs::create_dir_all(home.join("data/sessions")).unwrap();
    fs::create_dir_all(home.join("logs/schedules")).unwrap();
    fs::create_dir_all(&slow_workspace).unwrap();
    fs::create_dir_all(&fast_workspace).unwrap();

    let scheduler_script = r#"
claimed=0
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}' ;;
        *'"method":"recover"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"recovered":0}}' ;;
        *'"method":"reports"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"items":[]}}' ;;
        *'"method":"report"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"reported":true}}' ;;
        *'"method":"claim"'*)
            if [ "$claimed" = 0 ]; then
                claimed=1
                printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"item":{"id":42,"name":"slow-task","text":"slow task","context":{"provider":"codex","model":"test","workspace":"SLOW_WORKSPACE","origin":{"channel":"tui","session":"tui-default"}}}}}'
            elif [ "$claimed" = 1 ]; then
                claimed=2
                printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"item":{"id":43,"name":"fast-task","text":"fast task","context":{"provider":"codex","model":"test","workspace":"FAST_WORKSPACE","origin":{"channel":"tui","session":"tui-default"}}}}}'
            else
                printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"item":null}}'
            fi ;;
        *'"method":"finish"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"finished":true,"status":"completed"}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#
        .replace("SLOW_WORKSPACE", &slow_workspace.display().to_string())
        .replace("FAST_WORKSPACE", &fast_workspace.display().to_string());

    let scheduler = Process::start_with("sh", ["-c", scheduler_script.as_str()]).await.unwrap();

    let provider_script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"codex","version":"0.1.0","capabilities":["model"]}}' ;;
        *'"method":"generate"'*)
            case "$line" in
                *'"text":"slow task"'*)
                    touch "SLOW_STARTED"
                    (sleep 2; printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"text":"DONE: slow task completed","stop":"stop"}}') &
                    ;;
                *'"text":"fast task"'*)
                    if [ -e "SLOW_STARTED" ]; then touch "OVERLAP"; fi
                    printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"text":"DONE: fast task completed","stop":"stop"}}'
                    ;;
            esac ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#
        .replace("SLOW_STARTED", &slow_started.display().to_string())
        .replace("OVERLAP", &overlap.display().to_string());

    let provider = Process::start_with("sh", ["-c", provider_script.as_str()]).await.unwrap();
    let plugins = registry([scheduler, provider]).await;

    let sessions = Arc::new(Mutex::new(
        super::state::Store::load(home.join("data/sessions/index.json")).unwrap(),
    ));

    sessions.lock().unwrap().create("tui-default", "test").unwrap();

    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);
    let log_dir = home.join("logs/schedules");
    let overlap_for_signal = overlap.clone();

    let notifier = tokio::spawn(async move {
        let completed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if overlap_for_signal.is_file()
                    && log_dir.join("slow-task-42.jsonl").is_file()
                    && log_dir.join("fast-task-43.jsonl").is_file()
                {
                    break;
                }

                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        signal.signal();

        assert!(completed, "both due tasks should run while the slow task is in flight");
    });

    super::run_scheduler(
        plugins.clone(),
        sessions,
        Arc::clone(&stop),
        home.clone(),
        toml::from_str("approval='auto'\n[clients.tui]\ntools=true\nshell=true\n").unwrap(),
        root.join("workspace"),
        home.join("media"),
        "codex".into(),
        "test".into(),
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
    )
    .await;

    notifier.await.unwrap();

    assert!(overlap.is_file(), "the fast task should start before the slow task finishes");

    assert!(
        fs::read_to_string(home.join("logs/schedules/slow-task-42.jsonl"))
            .unwrap()
            .contains("Slow task completed.")
    );

    assert!(
        fs::read_to_string(home.join("logs/schedules/fast-task-43.jsonl"))
            .unwrap()
            .contains("Fast task completed.")
    );

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn daemon_runs_approved_schedules_and_reports_to_their_origin() {
    let tool_script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"tools","version":"0.1.0","capabilities":["tool"]}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let scheduler_script = r#"
claimed=0
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}' ;;
        *'"method":"recover"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"recovered":0}}' ;;
        *'"method":"reports"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"items":[]}}' ;;
        *'"method":"report"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"reported":true}}' ;;
        *'"method":"claim"'*)
            if [ "$claimed" = 0 ]; then
                claimed=1
                printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"item":{"id":42,"name":"say-hello","text":"say hello","context":{"provider":"codex","model":"test","workspace":"WORKSPACE","tools":true,"shell":true,"origin":{"channel":"tui","session":"tui-default"}}}}}'
            else
                printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"item":null}}'
            fi ;;
        *'"method":"finish"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"finished":true}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let provider_script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"codex","version":"0.1.0","capabilities":["model"]}}' ;;
        *'"method":"generate"'*) case "$line" in *'"name":"shell_run"'*) case "$line" in *'originating conversation metadata is'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"text":"DONE: Shell tool available.","stop":"stop"}}' ;; *) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"text":"Origin route was not provided.","stop":"stop"}}' ;; esac ;; *) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"text":"Shell tool missing.","stop":"stop"}}' ;; esac ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let root = test_root("scheduled-agent");
    let workspace = root.join("workspace");
    let home = root.join("home");

    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(home.join("data/sessions")).unwrap();
    fs::create_dir_all(home.join("logs/schedules")).unwrap();

    let scheduler_script = scheduler_script.replace("WORKSPACE", &workspace.display().to_string());

    let scheduler = Process::start_with("sh", ["-c", &scheduler_script]).await.unwrap();
    let provider = Process::start_with("sh", ["-c", provider_script]).await.unwrap();
    let tool = Process::start_with("sh", ["-c", tool_script]).await.unwrap();
    let plugins = registry([scheduler, provider, tool]).await;

    let sessions = Arc::new(Mutex::new(
        super::state::Store::load(home.join("data/sessions/index.json")).unwrap(),
    ));

    sessions.lock().unwrap().create("tui-default", "test").unwrap();

    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);
    let observed_sessions = Arc::clone(&sessions);

    let notifier = tokio::spawn(async move {
        let completed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let reported = observed_sessions
                        .lock()
                        .unwrap()
                        .sessions
                        .get("tui-default")
                        .is_some_and(|session| {
                            session.messages.iter().any(|message| {
                                message.content.iter().any(|content| {
                                    matches!(content, Content::Text { text } if text.contains("Scheduled task completed"))
                                })
                            })
                        });

                    if reported {
                        break;
                    }

                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .is_ok();
        signal.signal();

        assert!(completed, "scheduled turn should finish and report to its origin");
    });

    super::run_scheduler(
        plugins.clone(),
        Arc::clone(&sessions),
        Arc::clone(&stop),
        home.clone(),
        toml::from_str("approval='auto'\n[clients.tui]\ntools=true\nshell=true\n").unwrap(),
        workspace,
        home.join("media"),
        "codex".into(),
        "test".into(),
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
    )
    .await;

    notifier.await.unwrap();

    let history = fs::read_to_string(home.join("logs/schedules/say-hello-42.jsonl")).unwrap();

    assert!(history.contains("Shell tool available."));

    assert!(sessions.lock().unwrap().sessions["schedule-42"].messages.iter().any(|message| {
        message.content.iter().any(
            |content| matches!(content, Content::Text { text } if text == "Shell tool available."),
        )
    }));

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn config_set_preserves_comments_validates_and_saves_the_tui_alias() {
    let root = test_root("config-set");

    std::fs::create_dir_all(&root).unwrap();

    let config_path = root.join("config.toml");

    std::fs::write(&config_path, "# keep this note\n[clients.tui]\nshell = false\n").unwrap();

    super::config_command(
        super::ConfigCommand::Set(super::ConfigSet {
            assignment: "clients.tui=true".into(),
            force: false,
        }),
        &root,
        true,
    )
    .await
    .unwrap();

    let saved = std::fs::read_to_string(&config_path).unwrap();
    let config: super::Config = toml::from_str(&saved).unwrap();

    assert!(saved.contains("# keep this note"));
    assert!(config.clients["tui"].tools);

    assert!(
        super::config_command(
            super::ConfigCommand::Set(super::ConfigSet {
                assignment: "clients.tui.shell=\"not a boolean\"".into(),
                force: false,
            }),
            &root,
            true,
        )
        .await
        .is_err()
    );

    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), saved);

    std::fs::write(&config_path, "# legacy setting\nshell = true\n").unwrap();

    super::config_command(
        super::ConfigCommand::Set(super::ConfigSet {
            assignment: "clients.tui.shell=false".into(),
            force: false,
        }),
        &root,
        true,
    )
    .await
    .unwrap();

    let migrated = std::fs::read_to_string(&config_path).unwrap();
    let config: super::Config = toml::from_str(&migrated).unwrap();

    assert!(!migrated.starts_with("shell = true"));
    assert!(!config.tui_shell());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn parses_native_config_get_and_set_commands() {
    assert!(Cli::try_parse_from(["krabb", "config", "get", "clients.tui.tools"]).is_ok());
    assert!(Cli::try_parse_from(["krabb", "config", "set", "clients.tui=true"]).is_ok());

    assert!(Cli::try_parse_from(["krabb", "config", "set", "approval=prompt", "--force"]).is_ok());
}

#[test]
fn validates_manifests_and_environment() {
    let manifest = Manifest {
        id: "tools".into(),
        version: "0.1.0".into(),
        protocol: Protocol::CURRENT,
        capabilities: vec!["tool".into()],
        permissions: vec!["filesystem".into()],
        variables: vec!["KRABB_TOOLS_KEY".into()],
        settings: Vec::new(),
        setup: BTreeMap::new(),
        check: false,
        commands: Vec::new(),
        state: None,
        requires: Vec::new(),
    };

    assert!(manifest.validate().is_ok());

    let mut invalid = manifest;

    invalid.id = "Tools".into();

    assert!(invalid.validate().is_err());

    invalid.id = "tools".into();
    invalid.version.clear();

    assert!(invalid.validate().is_err());

    invalid.version = "0.1.0".into();
    invalid.protocol = Protocol { major: 9, minor: 0 };

    assert!(invalid.validate().is_err());

    invalid.protocol = Protocol::CURRENT;
    invalid.capabilities = vec!["tool".into(), "tool".into()];

    assert!(invalid.validate().is_err());

    invalid.capabilities = vec!["unknown".into()];

    assert!(invalid.validate().is_err());

    invalid.id = "timer".into();
    invalid.capabilities = vec!["timer".into()];

    assert!(invalid.validate().is_ok());

    invalid.id = "other".into();

    assert!(invalid.validate().is_err());

    invalid.id = "tools".into();
    invalid.capabilities = vec!["tool".into()];
    invalid.permissions = vec!["unknown".into()];

    assert!(invalid.validate().is_err());

    invalid.permissions.clear();
    invalid.variables = vec!["bad-name".into()];

    assert!(invalid.validate().is_err());

    invalid.variables = vec!["OPENAI_API_KEY".into()];

    assert!(invalid.validate().is_err());

    invalid.variables = vec!["KRABB_TOOLS_KEY".into(), "KRABB_TOOLS_KEY".into()];

    assert!(invalid.validate().is_err());

    let test_home = Path::new("/tmp/krabb-home");

    let values = env_for(
        &Manifest {
            id: "tools".into(),
            version: "0.1.0".into(),
            protocol: Protocol::CURRENT,
            capabilities: vec!["tool".into()],
            permissions: Vec::new(),
            variables: Vec::new(),
            settings: Vec::new(),
            setup: BTreeMap::new(),
            check: false,
            commands: Vec::new(),
            state: None,
            requires: Vec::new(),
        },
        &Config::default(),
        test_home,
    )
    .unwrap();

    assert_eq!(
        values
            .iter()
            .find(|(key, _)| key == "KRABB_COMPUTER_ALLOW_HOST_CONTROL")
            .map(|(_, value)| value.clone()),
        std::env::var("KRABB_COMPUTER_ALLOW_HOST_CONTROL").ok()
    );

    assert!(values.iter().any(|(key, value)| key == "KRABB_SHELL" && value == "off"));

    let expected_root = std::env::var_os("KRABB_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| test_home.join("workspace"));

    assert!(
        values
            .iter()
            .any(|(key, value)| { key == "KRABB_ROOT" && Path::new(value) == expected_root })
    );

    let mut config = Config::default();

    config.channels.insert(
        "telegram".into(),
        super::ChannelConfig { tools: true, shell: true, ..super::ChannelConfig::default() },
    );

    let values = env_for(
        &Manifest {
            id: "tools".into(),
            version: "0.1.0".into(),
            protocol: Protocol::CURRENT,
            capabilities: vec!["tool".into()],
            permissions: Vec::new(),
            variables: Vec::new(),
            settings: Vec::new(),
            setup: BTreeMap::new(),
            check: false,
            commands: Vec::new(),
            state: None,
            requires: Vec::new(),
        },
        &config,
        test_home,
    )
    .unwrap();

    assert!(values.iter().any(|(key, value)| key == "KRABB_SHELL" && value == "on"));

    let values = env_for(
        &Manifest {
            id: "codex".into(),
            version: "0.1.0".into(),
            protocol: Protocol::CURRENT,
            capabilities: vec!["model".into()],
            permissions: Vec::new(),
            variables: Vec::new(),
            settings: Vec::new(),
            setup: BTreeMap::new(),
            check: false,
            commands: Vec::new(),
            state: None,
            requires: Vec::new(),
        },
        &Config::default(),
        test_home,
    )
    .unwrap();

    if std::env::var_os("KRABB_HOME").is_none() {
        assert!(values.iter().any(|(key, value)| {
            key == "KRABB_HOME" && value == &test_home.display().to_string()
        }));
    }

    assert!(!values.iter().any(|(key, _)| { matches!(key.as_str(), "HOME" | "USERPROFILE") }));
}

#[test]
fn plugins_keep_the_windows_system_directories_needed_for_networking() {
    let values = env_for(
        &Manifest {
            id: "runner".into(),
            version: "0.1.0".into(),
            protocol: Protocol::CURRENT,
            capabilities: vec!["runner".into()],
            permissions: vec!["network".into()],
            variables: Vec::new(),
            settings: Vec::new(),
            setup: BTreeMap::new(),
            check: false,
            commands: Vec::new(),
            state: None,
            requires: Vec::new(),
        },
        &Config::default(),
        Path::new("/tmp/krabb-home"),
    )
    .unwrap();

    for name in ["SystemRoot", "WINDIR"] {
        let forwarded = values.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());

        assert_eq!(forwarded, std::env::var(name).ok(), "{name} was not forwarded.");

        if cfg!(windows) {
            assert!(forwarded.is_some(), "Windows plugins need {name} to load networking.");
        }
    }
}

#[test]
fn propagates_default_home_and_tools_root_with_database_override() {
    let mut values = vec![("KRABB_DB".into(), "/tmp/krabb.db".into())];
    let test_home = Path::new("/tmp/krabb-home");

    ensure_home(&mut values, test_home);
    ensure_root(&mut values, test_home);

    let home_text = test_home.display().to_string();
    let expected_root = test_home.join("workspace");

    assert!(values.iter().any(|(key, value)| key == "KRABB_HOME" && value == &home_text));

    assert!(
        values
            .iter()
            .any(|(key, value)| { key == "KRABB_ROOT" && Path::new(value) == expected_root })
    );

    let mut configured = vec![("KRABB_ROOT".into(), "/work/project".into())];

    ensure_root(&mut configured, test_home);

    assert_eq!(configured, [("KRABB_ROOT".into(), "/work/project".into())]);
}

fn interrupted_metadata(root: &Path, name: &str, previous: &[u8], next: &[u8], phase: &str) {
    fs::create_dir_all(root).unwrap();

    let _ = fs::remove_file(root.join(name));

    fs::write(root.join(format!(".{name}.backup-test")), previous).unwrap();
    fs::write(root.join(format!(".{name}.tmp-test")), next).unwrap();

    fs::write(
        root.join(format!(".{name}.txn-test")),
        serde_json::to_vec(&serde_json::json!({
            "path": name,
            "temporary": format!(".{name}.tmp-test"),
            "backup": format!(".{name}.backup-test"),
            "phase": phase,
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn command_discovery_waits_for_metadata_recovery() {
    let root = test_root("command-metadata-contention");

    let registry = serde_json::to_vec(&serde_json::json!({
        "plugins": {
            "echo": {
                "source": "local",
                "version": "0.1.0",
                "protocol": Protocol::CURRENT,
                "capabilities": [],
                "commands": [{
                    "name": "echo",
                    "description": "Echo input.",
                    "interactive": false
                }]
            }
        }
    }))
    .unwrap();

    interrupted_metadata(&root, "plugins.lock", &registry, b"invalid", "prepared");

    let guard = super::metadata_lock(&root).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader_root = root.clone();

    let reader = std::thread::spawn(move || {
        sender.send(super::available_commands(&reader_root)).unwrap();
    });

    assert!(matches!(
        receiver.recv_timeout(std::time::Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));

    assert!(!root.join("plugins.lock").exists());

    drop(guard);

    let commands = receiver.recv_timeout(std::time::Duration::from_secs(5)).unwrap().unwrap();

    reader.join().unwrap();

    assert_eq!(commands["echo"][0].0, "echo");
    assert!(!root.join(".plugins.lock.txn-test").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_lock_wait_is_bounded() {
    let root = test_root("metadata-timeout");
    let guard = super::metadata_lock(&root).unwrap();
    let timeout = std::time::Duration::from_millis(20);
    let start = std::time::Instant::now();
    let error = super::lock_at_timeout(&root, ".metadata.lock", timeout).err().unwrap();

    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(start.elapsed() >= timeout);

    drop(guard);

    assert!(super::metadata_lock(&root).is_ok());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn command_discovery_propagates_metadata_errors() {
    let root = test_root("command-metadata-error");

    assert!(super::available_commands(&root).unwrap().is_empty());

    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("plugins.lock"), b"invalid").unwrap();

    assert!(super::available_commands(&root).is_err());
    assert!(super::help_text_with_plugins(&root).is_err());

    fs::remove_file(root.join("plugins.lock")).unwrap();
    fs::remove_file(root.join(".metadata.lock")).unwrap();
    fs::create_dir(root.join(".metadata.lock")).unwrap();

    assert!(super::available_commands(&root).is_err());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_recovers_before_reading_or_initializing() {
    let root = test_root("metadata-recovery");
    let previous = b"update = 'auto'\n";
    let next = b"update = 'off'\n";

    for phase in ["prepared", "backed_up", "installed"] {
        interrupted_metadata(&root, "config.toml", previous, next, phase);
        super::init_at(&root).unwrap();

        let expected = if phase == "backed_up" { next.as_slice() } else { previous.as_slice() };

        assert_eq!(super::load_config_text(&root).unwrap().unwrap().as_bytes(), expected);
        assert_eq!(fs::read(root.join("config.toml")).unwrap(), expected);
    }

    interrupted_metadata(&root, "config.toml", previous, next, "prepared");

    assert_eq!(super::load_config_text(&root).unwrap().unwrap().as_bytes(), previous);

    interrupted_metadata(&root, "plugins.lock", b"{\"plugins\":{}}", b"invalid", "prepared");

    assert!(super::load_lock_at(&root).unwrap().plugins.is_empty());
    assert!(!root.join(".plugins.lock.txn-test").exists());

    interrupted_metadata(&root, "plugins.lock", b"invalid", b"{\"plugins\":{}}", "backed_up");

    assert!(super::load_lock_at(&root).unwrap().plugins.is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn command_tool_progress_preserves_machine_readable_output() {
    let event = Request::Note {
        jsonrpc: "2.0".into(),
        method: "event".into(),
        params: serde_json::json!({"event": {"kind":"tool", "name":"read", "args":{}}}),
    };

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    super::write_command_event(&event, true, &mut stdout, &mut stderr);

    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn json_commands_route_streamed_instructions_to_stderr() {
    let event = Request::Note {
        jsonrpc: "2.0".into(),
        method: "event".into(),
        params: serde_json::json!({"event": {"text": "streamed"}}),
    };

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    super::write_command_event(&event, true, &mut stdout, &mut stderr);

    assert!(stdout.is_empty());
    assert_eq!(stderr, b"streamed");

    super::write_command_event(&event, false, &mut stdout, &mut stderr);

    assert_eq!(stdout, b"streamed");
    assert_eq!(stderr, b"streamed");

    assert_eq!(
        super::command_event_text(&Request::Note {
            jsonrpc: "2.0".into(),
            method: "other".into(),
            params: serde_json::json!({}),
        },),
        None
    );

    assert_eq!(super::command_event_text(&Request::call(1, "event", serde_json::json!({}))), None);
}

#[test]
fn recovers_locks_and_plugin_staging() {
    let root = std::env::temp_dir().join(format!("krabb-lock-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(root.join("plugins")).unwrap();

    let lock = daemon_lock(&root).unwrap();

    assert!(daemon_lock(&root).is_err());

    drop(lock);

    let stale = root.join("stale.lock");

    fs::write(&stale, "not-a-pid").unwrap();

    let lock = super::lock_at(&root, "stale.lock").unwrap();

    drop(lock);
    fs::create_dir_all(root.join("plugins/.echo.backup-test")).unwrap();
    fs::create_dir_all(root.join("plugins/.echo.stage-test")).unwrap();

    let active = super::lock_at(&root, ".plugins.lock").unwrap();
    let error = recover_plugins(&root).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(root.join("plugins/.echo.stage-test").exists());

    drop(active);

    let backup = root.join("plugins/.echo.backup-test");

    fs::create_dir_all(backup.join("bin")).unwrap();
    fs::write(backup.join("bin").join(super::plugin_name("echo")), "old binary").unwrap();

    fs::write(
        backup.join("krabb-plugin.toml"),
        "id = 'echo'\nversion = '0.1.0'\ncapabilities = []\n[protocol]\nmajor = 0\nminor = 1\n",
    )
    .unwrap();

    let hash = super::digest(
        &backup.join("krabb-plugin.toml"),
        &backup.join("bin").join(super::plugin_name("echo")),
    )
    .unwrap();

    let mut plugins = BTreeMap::new();

    plugins.insert(
        "echo".into(),
        super::Entry {
            source: "echo".into(),
            revision: "local".into(),
            pinned: false,
            default: false,
            hash,
            version: "0.1.0".into(),
            protocol: Protocol::CURRENT,
            capabilities: Vec::new(),
            permissions: Vec::new(),
            variables: Vec::new(),
            settings: Vec::new(),
            commands: Vec::new(),
            linked: false,
            state: None,
        },
    );

    interrupted_metadata(
        &root,
        "plugins.lock",
        &serde_json::to_vec(&super::Lock { plugins }).unwrap(),
        b"uncommitted lock",
        "prepared",
    );

    recover_plugins(&root).unwrap();

    assert!(root.join("plugins/echo").is_dir());
    assert!(!root.join("plugins/.echo.stage-test").exists());

    fs::create_dir_all(root.join("plugins/.gone.backup-remove-test")).unwrap();
    recover_plugins(&root).unwrap();

    assert!(!root.join("plugins/.gone.backup-remove-test").exists());

    let destination = root.join("plugins/echo");

    fs::rename(&destination, &backup).unwrap();
    fs::create_dir_all(destination.join("bin")).unwrap();
    fs::copy(backup.join("krabb-plugin.toml"), destination.join("krabb-plugin.toml")).unwrap();
    fs::write(destination.join("bin").join(super::plugin_name("echo")), "uncommitted binary")
        .unwrap();
    recover_plugins(&root).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("bin").join(super::plugin_name("echo"))).unwrap(),
        "old binary"
    );

    assert!(!backup.exists());

    fs::create_dir_all(&backup).unwrap();
    recover_plugins(&root).unwrap();

    assert!(!backup.exists());

    fs::rename(&destination, &backup).unwrap();
    fs::write(backup.join("bin").join(super::plugin_name("echo")), "corrupt binary").unwrap();

    assert!(recover_plugins(&root).is_err());
    assert!(backup.exists());

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn recovery_obeys_stop_signal() {
    let script = "while IFS= read -r line; do case \"$line\" in *hello*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocol\":{\"major\":0,\"minor\":1},\"id\":\"test\",\"version\":\"0.1.0\",\"capabilities\":[]}}' ;; esac; done";
    let process = Process::start_with("sh", ["-c", script]).await.unwrap();
    let process = Live::new(process);
    let stop = Stop::new();

    stop.signal();

    let mut failures = 0;

    assert!(!recover(&process, &mut failures, "test", &stop).await.unwrap());

    process.stop().await.unwrap();
}

#[tokio::test]
async fn shutdown_signal_remains_visible_to_later_waiters() {
    let stop = Stop::new();

    stop.signal();
    tokio::time::timeout(std::time::Duration::from_secs(1), stop.notified()).await.unwrap();
}

#[test]
fn covers_routing_helpers() {
    assert!(super::mentions("hello", ""));
    assert!(super::mentions("Hi @Krabb", "@krabb"));
    assert!(!super::mentions("Hi there", "krabb"));
    assert!(super::same_id(&serde_json::json!(7), "7"));
    assert!(!super::same_id(&serde_json::Value::Null, "7"));

    let cancels = Arc::new(Mutex::new(BTreeMap::new()));
    let first = super::cancellation(&cancels, "session");
    let second = super::cancellation(&cancels, "session");

    assert!(Arc::ptr_eq(&first, &second));

    assert_eq!(
        send_params("matrix", "delivery", "room", "hello", Some("topic")).unwrap(),
        serde_json::json!({"delivery": "delivery", "chat": "room", "format": "markdown", "text": "hello", "thread": "topic"})
    );

    let store = super::state::Store::default();
    let missing = std::env::temp_dir().join(format!("krabb-worktrees-{}", std::process::id()));

    reclaim_worktrees(&missing, &store);
}

#[test]
fn windows_service_uses_an_unprivileged_account_from_creation() {
    for (action, start, account) in [
        ("create", "disabled", r"NT AUTHORITY\LocalService"),
        ("config", "auto", r"NT AUTHORITY\LocalService"),
    ] {
        let command =
            super::windows_service_command(action, r#""C:\Krabb\daemon.exe" --service"#, start);
        let args = command.get_args().map(|arg| arg.to_str().unwrap()).collect::<Vec<_>>();

        assert_eq!(command.get_program(), "sc.exe");
        assert_eq!(
            args,
            [
                action,
                "Krabb",
                "binPath=",
                r#""C:\Krabb\daemon.exe" --service"#,
                "start=",
                start,
                "obj=",
                account,
                "password=",
                "",
            ]
        );
    }
}

#[test]
fn windows_service_private_state_includes_external_memory_and_its_lock() {
    let parent = Path::new("external");

    let memory = parent.join("memory.json");
    let paths = super::windows_service_private("KRABB_MEMORY", &memory);

    assert_eq!(paths, [memory.clone(), parent.join("memory.lock")]);
    assert!(!paths.contains(&parent.to_path_buf()));

    assert_eq!(
        super::windows_service_private("KRABB_MEMORY", Path::new("memory")),
        [PathBuf::from("memory"), PathBuf::from("memory.lock")]
    );

    for name in ["KRABB_HOME", "KRABB_CREDENTIALS", "KRABB_TIMER", "KRABB_CODEX_HOME"] {
        assert_eq!(super::windows_service_private(name, &memory), std::slice::from_ref(&memory));
    }

    for name in ["KRABB_ROOT", "KRABB_MEDIA", "KRABB_DB", "KRABB_MODEL"] {
        assert!(super::windows_service_private(name, &memory).is_empty());
    }
}

#[test]
fn windows_service_persistence_access_includes_missing_stores_and_sidecars() {
    let root = test_root("service-persistence-access");
    let parent = root.join("external");
    let _ = std::fs::remove_dir_all(&root);

    for name in ["KRABB_MEMORY", "KRABB_DB"] {
        let store = parent.join("store");

        assert_eq!(super::windows_service_directory(name, &store), Some(parent.clone()));

        std::fs::create_dir_all(&parent).unwrap();
        std::fs::write(&store, "existing").unwrap();
        std::fs::write(parent.join("store-wal"), "sidecar").unwrap();

        assert_eq!(super::windows_service_directory(name, &store), Some(parent.clone()));

        assert_eq!(super::windows_service_directory(name, Path::new("store")), Some(".".into()));

        std::fs::remove_dir_all(&parent).unwrap();
    }

    for name in ["KRABB_ROOT", "KRABB_MEDIA"] {
        assert_eq!(super::windows_service_directory(name, &parent), Some(parent.clone()));
    }

    assert!(super::windows_service_directory("KRABB_MODEL", &parent).is_none());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn windows_service_grants_external_directory_access_without_replacing_permissions() {
    let path = Path::new(r"C:\Users\Alice\private workspace");
    let command = super::windows_service_access(path, true);
    let args = command.get_args().map(|arg| arg.to_str().unwrap()).collect::<Vec<_>>();

    assert_eq!(command.get_program(), "icacls.exe");

    assert_eq!(args, [path.to_str().unwrap(), "/grant", r"NT SERVICE\Krabb:(OI)(CI)M", "/T", "/L"]);
}

#[test]
fn windows_service_grants_browser_dependencies_read_execute_access() {
    let root = test_root("service-browser-access");
    let modules = root.join("node_modules");
    let browsers = root.join("browsers");

    fs::create_dir_all(modules.join("playwright")).unwrap();
    fs::create_dir_all(modules.join("playwright-core")).unwrap();
    fs::create_dir_all(&browsers).unwrap();

    let node = root.join("node.exe");

    fs::write(&node, "fixture executable").unwrap();

    for (name, path, expected) in [
        ("KRABB_PLAYWRIGHT_MODULE", modules.join("playwright/index.js"), modules),
        ("KRABB_PLAYWRIGHT_BROWSERS_PATH", browsers.clone(), browsers),
        ("KRABB_NODE", node.clone(), node),
        ("KRABB_PLAYWRIGHT_MODULE", root.join("index.js"), root.clone()),
    ] {
        let dependency = super::windows_service_dependency(name, &path).unwrap();

        assert_eq!(dependency, expected);

        let command = super::windows_service_access(&dependency, false);
        let args = command.get_args().map(|arg| arg.to_str().unwrap()).collect::<Vec<_>>();
        let grant = if dependency.is_dir() {
            r"NT SERVICE\Krabb:(OI)(CI)RX"
        } else {
            r"NT SERVICE\Krabb:RX"
        };

        assert_eq!(command.get_program(), "icacls.exe");
        assert_eq!(&args[..3], [dependency.to_str().unwrap(), "/grant", grant]);
        assert_eq!(args.contains(&"/T"), dependency.is_dir());
        assert_eq!(args.last(), Some(&"/L"));
        assert!(!args.contains(&"/reset"));
    }

    assert!(super::windows_service_dependency("KRABB_BROWSER_EGRESS_PROXY", &root).is_none());

    assert!(super::windows_service_dependency("KRABB_NODE", Path::new("node")).is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn service_definition_has_a_restart_policy() {
    let text = service_text(std::path::Path::new("/tmp/krabb"), &[]);

    #[cfg(target_os = "linux")]

    assert_eq!(service_name(), "Krabb");

    #[cfg(target_os = "macos")]
    {
        assert_eq!(MACOS_SERVICE_LABEL, "it.airscript.krabb");
        assert_eq!(service_name(), "Krabb");
    }

    #[cfg(target_os = "windows")]

    assert_eq!(service_name(), "Krabb");

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]

    assert_eq!(service_name(), "Krabb");

    assert!(text.contains("/tmp/krabb"));
    assert!(!text.contains("serve"));

    #[cfg(target_os = "linux")]
    {
        assert!(text.contains("Restart=always"));
        assert!(text.contains("RestartSec=5s"));
        assert!(text.contains("StartLimitIntervalSec=0"));
        assert!(!text.contains("ExecStart=\\\""));
    }

    #[cfg(target_os = "macos")]
    {
        assert!(text.contains("<key>Label</key><string>it.airscript.krabb</string>"));
        assert_eq!(service_path().file_name().unwrap(), "it.airscript.krabb.plist");
    }

    let _ = ServiceCommand::Status;
}

#[test]
fn service_definition_preserves_paths_and_uses_credential_file() {
    let environment = vec![
        ("KRABB_HOME".into(), "/tmp/krabb-home".into()),
        ("KRABB_ROOT".into(), "/tmp/krabb-root".into()),
        ("KRABB_CREDENTIALS".into(), "/tmp/krabb-credentials.json".into()),
    ];

    let text = service_text(std::path::Path::new("/tmp/krabb"), &environment);

    assert!(text.contains("KRABB_HOME"));
    assert!(text.contains("/tmp/krabb-home"));
    assert!(text.contains("KRABB_ROOT"));
    assert!(text.contains("/tmp/krabb-root"));
    assert!(text.contains("KRABB_CREDENTIALS"));
    assert!(text.contains("/tmp/krabb-credentials.json"));
    assert!(!text.contains("secret"));
}

#[test]
#[cfg(target_os = "linux")]
fn service_definition_preserves_percent_encoded_proxy() {
    let environment = vec![(
        "KRABB_BROWSER_EGRESS_PROXY".into(),
        "http://user%40example.test:encoded%25value@proxy.example:8080".into(),
    )];

    let text = service_text(std::path::Path::new("/tmp/krabb"), &environment);

    assert!(text.contains(concat!(
        "Environment=\"KRABB_BROWSER_EGRESS_PROXY=http://user%%40example.test:",
        "encoded%%25value@proxy.example:8080\""
    )));
}

#[test]
fn service_environment_materializes_declared_credentials() {
    let root = test_root("service-env");

    fs::create_dir_all(&root).unwrap();

    let configured = root.join("credentials.json");

    super::secure(&configured, br#"{"KRABB_TELEGRAM_TOKEN":"old"}"#).unwrap();

    let mut environment = BTreeMap::new();

    environment.insert("KRABB_ROOT".into(), root.display().to_string());
    environment.insert("KRABB_MEMORY".into(), "memory.json".into());
    environment.insert("KRABB_TIMER".into(), "timer.json".into());
    environment.insert("KRABB_DB".into(), "krabb.db".into());
    environment.insert("KRABB_SANDBOX_RUNTIME".into(), "docker".into());
    environment.insert("KRABB_SANDBOX_IMAGE".into(), "local/tool:latest".into());
    environment.insert("KRABB_CREDENTIALS".into(), configured.display().to_string());
    environment.insert("KRABB_CODEX_KEY".into(), "secret".into());

    let service = root.join("krabb.service");
    let values = service_environment_from(&service, root.clone(), &environment).unwrap();

    assert!(
        values
            .iter()
            .any(|(name, value)| { name == "KRABB_HOME" && value == &root.display().to_string() })
    );

    for (name, path) in
        [("KRABB_MEMORY", "memory.json"), ("KRABB_TIMER", "timer.json"), ("KRABB_DB", "krabb.db")]
    {
        let expected = service_path_value(PathBuf::from(path)).unwrap();

        assert!(
            values.iter().any(|(value_name, value)| { value_name == name && value == &expected })
        );
    }

    let credential_path = super::service_credentials_path(&service);

    assert!(values.iter().any(|(name, value)| {
        name == "KRABB_CREDENTIALS" && value == &credential_path.display().to_string()
    }));

    let credentials =
        super::credentials::read(&credential_path, &root.join("credentials.key")).unwrap();

    assert!(!fs::read_to_string(&credential_path).unwrap().contains("secret"));

    assert_eq!(credentials["KRABB_TELEGRAM_TOKEN"], "old");
    assert_eq!(credentials["KRABB_CODEX_KEY"], "secret");

    assert!(
        values.iter().any(|(name, value)| name == "KRABB_SANDBOX_RUNTIME" && value == "docker")
    );

    assert!(
        values
            .iter()
            .any(|(name, value)| name == "KRABB_SANDBOX_IMAGE" && value == "local/tool:latest")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn services_preserve_explicit_browser_and_computer_settings() {
    let root = test_root("service-desktop");

    fs::create_dir_all(&root).unwrap();

    let environment = BTreeMap::from([
        ("KRABB_PLAYWRIGHT_MODULE".into(), "/opt/playwright/index.js".into()),
        ("KRABB_PLAYWRIGHT_BROWSERS_PATH".into(), "/opt/browsers".into()),
        ("KRABB_NODE".into(), "/opt/node".into()),
        ("KRABB_BROWSER_ALLOWED_HOSTS".into(), "example.com".into()),
        ("KRABB_BROWSER_EGRESS_PROXY".into(), "http://127.0.0.1:8080".into()),
        ("KRABB_COMPUTER_MODE".into(), "interactive".into()),
        ("KRABB_COMPUTER_ALLOW_HOST_CONTROL".into(), "1".into()),
        ("KRABB_COMPUTER_DISPLAY".into(), ":99".into()),
        ("KRABB_XVFB".into(), "/opt/Xvfb".into()),
        ("KRABB_COMPUTER_STARTUP".into(), "/opt/desktop".into()),
        ("KRABB_COMPUTER_STARTUP_ARGS".into(), "[\"--session\"]".into()),
    ]);

    let service = root.join("krabb.service");
    let values = service_environment_from(&service, root.clone(), &environment).unwrap();

    for setting in &environment {
        assert!(values.iter().any(|(name, value)| (name, value) == setting));
    }

    let defaults = service_environment_from(&service, root.clone(), &BTreeMap::new()).unwrap();

    assert!(!defaults.iter().any(|(name, _)| name == "KRABB_COMPUTER_ALLOW_HOST_CONTROL"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn service_environment_pins_installing_codex_profile() {
    let root = test_root("service-codex-profile");
    let service = root.join("krabb.service");
    let mut environment = BTreeMap::new();

    for (name, path, expected) in [
        ("USERPROFILE", "windows-user", root.join("windows-user").join(".codex")),
        ("HOME", "home-user", root.join("home-user").join(".codex")),
        ("KRABB_CODEX_HOME", "custom-profile", root.join("custom-profile")),
    ] {
        environment.insert(name.into(), root.join(path).display().to_string());
        let values = service_environment_from(&service, root.clone(), &environment).unwrap();
        let profiles =
            values.iter().filter(|(name, _)| name == "KRABB_CODEX_HOME").collect::<Vec<_>>();

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].1, expected.display().to_string());
    }
}

#[test]
fn service_environment_pins_codex_found_on_installing_path() {
    let root = test_root("service-codex-path");

    let bin = root.join("user-bin");

    fs::create_dir_all(&bin).unwrap();

    let executable = bin.join(if cfg!(windows) { "codex.exe" } else { "codex" });

    fs::write(&executable, b"codex").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut environment = BTreeMap::new();

    environment.insert("PATH".into(), bin.display().to_string());

    let home = root.join("state");
    let service = home.join("krabb.service");
    let values = service_environment_from(&service, home, &environment).unwrap();
    let expected = executable.canonicalize().unwrap().display().to_string();

    assert!(
        values.iter().any(|(name, value)| { name == "KRABB_CODEX_BINARY" && value == &expected })
    );

    let dependency =
        super::windows_service_dependency("KRABB_CODEX_BINARY", Path::new(&expected)).unwrap();

    let command = super::windows_service_access(&dependency, false);
    let args = command.get_args().map(|arg| arg.to_str().unwrap()).collect::<Vec<_>>();

    assert_eq!(dependency, executable.canonicalize().unwrap());
    assert_eq!(args, [expected.as_str(), "/grant", r"NT SERVICE\Krabb:RX", "/L"]);

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn manages_service_definition() {
    if std::env::var_os("KRABB_SERVICE_FIXTURE").is_none() {
        let root = test_root("service-environment");

        fs::create_dir_all(&root).unwrap();

        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());

        command
            .args(["--exact", "tests::manages_service_definition", "--nocapture"])
            .env_clear()
            .env("KRABB_SERVICE_FIXTURE", "1")
            .env("KRABB_HOME", &root)
            .kill_on_drop(true);

        for name in ["SystemRoot", "WINDIR", "TEMP", "TMP", "LLVM_PROFILE_FILE"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }

        let output = tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .expect("Service fixture timed out")
            .unwrap();

        assert!(
            output.status.success(),
            "Service fixture failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        fs::remove_dir_all(root).unwrap();
        return;
    }

    let path = std::env::temp_dir().join(format!("krabb-service-{}", std::process::id()));
    let _ = fs::remove_file(&path);

    super::service_at_with_status(
        &path,
        ServiceCommand::Status,
        |_, _| Ok(()),
        || panic!("status must not query the manager without an installed definition"),
        true,
    )
    .unwrap();

    assert!(
        super::service_at_with_status(
            &path,
            ServiceCommand::Restart,
            |_, _| panic!("restart must not run without an installed definition"),
            || panic!("restart must not query the manager without an installed definition"),
            true,
        )
        .is_err()
    );

    #[cfg(not(windows))]
    service_at(&path, ServiceCommand::Install(super::ServiceInstall { force: false })).unwrap();

    #[cfg(windows)]
    fs::write(&path, super::service_text(&daemon_executable().unwrap(), &[])).unwrap();

    assert!(path.is_file());

    assert!(
        super::service_at_with_status(
            &path,
            ServiceCommand::Install(super::ServiceInstall { force: false }),
            |_, _| panic!("an existing definition must not reach the service manager"),
            || panic!("an existing definition must not query the service manager"),
            false,
        )
        .is_err()
    );

    let copy = super::service_credentials_path(&path);

    fs::write(&copy, "stale copy").unwrap();

    #[cfg(not(windows))]
    {
        service_at(&path, ServiceCommand::Install(super::ServiceInstall { force: true })).unwrap();

        assert!(!copy.exists(), "a reinstall without secrets must not keep an older copy");

        fs::write(&copy, "copy").unwrap();
    }

    super::service_at_with_status(
        &path,
        ServiceCommand::Status,
        |_, _| Ok(()),
        || Ok(super::ServiceState::Inactive),
        false,
    )
    .unwrap();

    super::service_at_with_status(
        &path,
        ServiceCommand::Status,
        |_, _| Ok(()),
        || Ok(super::ServiceState::Active),
        true,
    )
    .unwrap();

    let mut restarted = false;

    super::service_at_with_status(
        &path,
        ServiceCommand::Restart,
        |_, action| {
            assert_eq!(action, super::ServiceAction::Restart);
            restarted = true;
            Ok(())
        },
        || panic!("restart delegates directly to the native service manager"),
        true,
    )
    .unwrap();

    assert!(restarted);

    {
        #[cfg(not(windows))]
        let mut stopped = false;

        #[cfg(not(windows))]
        service_at_with(
            &path,
            ServiceCommand::Remove(super::ServiceRemove { yes: true }),
            |_, action| {
                assert_eq!(action, super::ServiceAction::Stop);
                stopped = true;
                Ok(())
            },
        )
        .unwrap();

        #[cfg(not(windows))]

        assert!(stopped);

        #[cfg(windows)]
        fs::remove_file(&path).unwrap();

        assert!(!path.exists());

        if cfg!(not(windows)) {
            assert!(!copy.exists(), "uninstall must delete the service credential copy");
        }

        fs::write(&copy, "orphaned copy").unwrap();
        fs::write(copy.with_extension("json.new"), "staged copy").unwrap();
        service_at(&path, ServiceCommand::Remove(super::ServiceRemove { yes: true })).unwrap();

        assert!(!copy.exists());
        assert!(!copy.with_extension("json.new").exists());
    }
}

#[test]
fn json_uninstall_folds_its_stop_into_one_result() {
    use super::ServiceReport::{Json, Silent, Text};

    let remove = ServiceCommand::Remove(super::ServiceRemove { yes: true });

    assert_eq!(super::service_report(&remove, true), Silent);
    assert_eq!(super::service_report(&remove, false), Text);
    assert_eq!(super::service_report(&ServiceCommand::Start, true), Json);
    assert_eq!(super::service_report(&ServiceCommand::Restart, false), Text);
    assert_eq!(super::ServiceReport::nested(true), Silent);
    assert_eq!(super::ServiceReport::nested(false), Text);
}

#[test]
fn service_actions_report_when_no_transition_is_needed() {
    use super::ServiceState::{Active, Failed, Inactive, NotInstalled, Starting, Stopping};

    assert!(super::service_action_is_noop(true, Active));
    assert!(!super::service_action_is_noop(true, Inactive));
    assert!(!super::service_action_is_noop(true, Starting));
    assert!(!super::service_action_is_noop(true, Stopping));
    assert!(!super::service_action_is_noop(false, Active));
    assert!(super::service_action_is_noop(false, Inactive));
    assert!(super::service_action_is_noop(false, Failed));
    assert!(super::service_action_is_noop(false, NotInstalled));
    assert!(!super::service_action_is_noop(false, Starting));
    assert!(!super::service_action_is_noop(false, Stopping));
    assert_eq!(super::service_action_start(super::ServiceAction::Restart), None);

    assert_eq!(
        super::service_action_text(super::ServiceAction::Start, None),
        "The service has been started."
    );

    assert_eq!(
        super::service_action_text(super::ServiceAction::Stop, None),
        "The service has been stopped."
    );

    assert_eq!(
        super::service_action_text(super::ServiceAction::Restart, None),
        "The service has been restarted."
    );

    assert_eq!(
        super::service_action_text(super::ServiceAction::Start, Some(Active)),
        "The service is already running."
    );

    assert_eq!(
        super::service_action_text(super::ServiceAction::Start, Some(Starting)),
        "The service is already starting."
    );

    assert_eq!(
        super::service_action_text(super::ServiceAction::Stop, Some(Inactive)),
        "The service is already stopped."
    );

    assert_eq!(
        super::service_action_text(super::ServiceAction::Stop, Some(Stopping)),
        "The service is already stopping."
    );

    assert_eq!(
        super::service_action_text(super::ServiceAction::Stop, Some(Failed)),
        "The service has failed, so there is nothing to stop."
    );

    assert_eq!(
        super::service_action_text(super::ServiceAction::Stop, Some(NotInstalled)),
        "The service is not installed."
    );

    for (action, previous) in [
        (super::ServiceAction::Start, Some(Active)),
        (super::ServiceAction::Stop, Some(Inactive)),
        (super::ServiceAction::Stop, Some(Failed)),
        (super::ServiceAction::Stop, Some(NotInstalled)),
        (super::ServiceAction::Restart, Some(Active)),
        (super::ServiceAction::Start, None),
        (super::ServiceAction::Stop, None),
        (super::ServiceAction::Restart, None),
    ] {
        for report in
            [super::ServiceReport::Text, super::ServiceReport::Json, super::ServiceReport::Silent]
        {
            super::service_action_message(action, previous, report).unwrap();
        }
    }
}

#[test]
fn service_status_uses_clear_sentences() {
    assert_eq!(
        super::service_status_text(
            "Krabb",
            super::ServiceState::Active,
            Path::new("/home/user/krabb.service"),
            true
        ),
        "The Krabb service is running\n└─ Definition: /home/user/krabb.service"
    );

    assert_eq!(
        super::service_status_text(
            "Krabb",
            super::ServiceState::Inactive,
            Path::new("/home/user/krabb.service"),
            true
        ),
        "The Krabb service is stopped\n└─ Definition: /home/user/krabb.service\n\n\
         Start it with `krabb service start`."
    );

    assert!(
        super::service_status_text(
            "Krabb",
            super::ServiceState::Failed,
            Path::new("/home/user/krabb.service"),
            true
        )
        .ends_with("\n\nRestart it with `krabb service restart`.")
    );

    assert_eq!(
        super::service_status_text(
            "Krabb",
            super::ServiceState::NotInstalled,
            Path::new("unused"),
            false
        ),
        "The Krabb service is not installed.\nInstall it with `krabb service install`."
    );
}

#[cfg(target_os = "linux")]
#[test]
fn maps_systemd_service_states_and_errors() {
    use super::ServiceState::{Active, Failed, Inactive, Starting, Stopping};

    for (text, expected) in [
        ("active", Active),
        ("activating", Starting),
        ("deactivating", Stopping),
        ("failed", Failed),
        ("inactive", Inactive),
        ("unknown", Inactive),
        ("not-found", Inactive),
    ] {
        assert_eq!(super::linux_service_state(false, text, "").unwrap(), expected);
    }

    assert!(super::linux_service_state(false, "", "").is_err());

    let error =
        super::linux_service_state(false, "", "systemd is unavailable").unwrap_err().to_string();

    assert!(error.to_ascii_lowercase().contains("systemd is unavailable"));
}

#[cfg(unix)]
#[test]
fn rejects_a_symlink_service_definition() {
    let root = std::env::temp_dir().join(format!("krabb-service-link-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(&root).unwrap();

    let target = root.join("target");
    let path = root.join("service");

    std::os::unix::fs::symlink(&target, &path).unwrap();

    let error = service_at(&path, ServiceCommand::Install(super::ServiceInstall { force: false }))
        .unwrap_err()
        .to_string();

    assert!(error.contains("symbolic link"));
    assert!(!target.exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn plugin_discovery_supports_channel_client_and_legacy_layouts() {
    let root = test_root("plugin-layouts");
    let plugins = root.join("krabb-plugins");

    for (id, relative) in [
        ("codex", "intelligence/codex"),
        ("telegram", "channels/telegram"),
        ("tui", "clients/tui"),
        ("slack", "messaging/slack"),
        ("memory", "memory"),
    ] {
        let source = plugins.join(relative);
        let binary = root.join("target/debug").join(super::plugin_name(id));

        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, "fixture").unwrap();

        assert_eq!(super::default_plugin_source(&plugins, id).unwrap(), source);
        assert_eq!(plugin_binary(&source, id, true), Some(binary));

        let sources = [super::Source {
            id: id.into(),
            source: None,
            revision: None,
            version: None,
            yes: true,
        }];

        super::ensure_linkable_sources(true, &sources, &root).unwrap();
    }

    fs::create_dir_all(plugins.join("messaging/telegram")).unwrap();

    assert_eq!(
        super::default_plugin_source(&plugins, "telegram").unwrap(),
        plugins.join("channels/telegram")
    );

    fs::create_dir_all(plugins.join("tui")).unwrap();

    assert_eq!(super::default_plugin_source(&plugins, "tui").unwrap(), plugins.join("tui"));
    assert!(super::default_plugin_source(&plugins, "missing").is_err());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn legacy_local_plugin_sources_follow_relocations_only_when_missing() {
    let root = PathBuf::from(format!(".krabb-relocation-{}-{}", std::process::id(), super::now()));

    for (legacy, current) in [
        ("krabb-plugins/messaging/telegram", "krabb-plugins/channels/telegram"),
        ("krabb-plugins/tui", "krabb-plugins/clients/tui"),
    ] {
        let old = root.join(legacy);
        let new = root.join(current);

        fs::create_dir_all(&new).unwrap();

        let expected = fs::canonicalize(&new).unwrap();
        let absolute = std::env::current_dir().unwrap().join(&old);

        for source in [
            old.display().to_string(),
            format!("./{}", old.display()),
            absolute.display().to_string(),
            format!("file://{}", absolute.display()),
        ] {
            let resolved = resolve(&source, Some("local")).unwrap();

            assert_eq!(resolved.path, expected);
            assert!(resolved.temp.is_none());

            let canonical = if source.starts_with("file://") {
                format!("file://{}", expected.display())
            } else {
                expected.display().to_string()
            };

            assert_eq!(super::canonical_source(&source), canonical);
            assert!(resolve(&source, Some("wrong-revision")).is_err());
        }

        fs::create_dir_all(&old).unwrap();

        assert_eq!(
            resolve(&old.display().to_string(), None).unwrap().path,
            fs::canonicalize(&old).unwrap()
        );

        assert!(super::relocated_plugin_source(&old).is_none());
    }

    assert!(
        super::relocated_plugin_source(&root.join("krabb-plugins/messaging/missing")).is_none()
    );

    assert!(super::relocated_plugin_source(&root.join("krabb-plugins/missing")).is_none());
    assert!(super::relocated_plugin_source(&root.join("other/messaging/telegram")).is_none());
    assert!(super::relocated_plugin_source(Path::new("missing")).is_none());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn discovers_plugins_and_detects_source_changes() {
    let root = std::env::temp_dir().join(format!("krabb-discovery-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    init_at(&root).unwrap();

    let plugin = root.join("plugins/echo");

    fs::create_dir_all(plugin.join("bin")).unwrap();

    fs::write(
            plugin.join("krabb-plugin.toml"),
            "id = 'echo'\nversion = '0.1.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['model']\n",
        )
        .unwrap();

    let binary = plugin.join("bin").join(super::plugin_name("echo"));

    fs::write(&binary, "binary").unwrap();

    assert_eq!(installed_at(&root), vec!["echo"]);
    assert_eq!(binary_at("echo", &root), Some(binary.clone()));
    assert!(binary_at("../escape", &root).is_none());

    let default_source = root.join("repo/krabb-plugins/echo");
    let default_binary = root.join("repo/target/debug").join(super::plugin_name("echo"));

    fs::create_dir_all(&default_source).unwrap();
    fs::create_dir_all(default_binary.parent().unwrap()).unwrap();
    fs::write(&default_binary, "default binary").unwrap();

    assert_eq!(plugin_binary(&default_source, "echo", true), Some(default_binary));

    let categorized_source = root.join("repo/krabb-plugins/intelligence/codex");
    let categorized_binary = root.join("repo/target/debug").join(super::plugin_name("codex"));

    fs::create_dir_all(&categorized_source).unwrap();

    fs::write(
        categorized_source.join("krabb-plugin.toml"),
        "id = 'codex'\nversion = '0.1.0'\nprotocol = { major = 0, minor = 1 }\n",
    )
    .unwrap();

    fs::write(&categorized_binary, "categorized binary").unwrap();

    assert_eq!(
        super::default_plugin_source(&root.join("repo/krabb-plugins"), "codex").unwrap(),
        categorized_source
    );

    assert_eq!(
        plugin_binary(&root.join("repo/krabb-plugins/intelligence/codex"), "codex", true),
        Some(categorized_binary)
    );

    let entry = super::Entry {
        source: plugin.display().to_string(),
        revision: String::new(),
        pinned: false,
        default: false,
        hash: String::new(),
        version: "0.1.0".into(),
        protocol: Protocol::CURRENT,
        capabilities: vec!["model".into()],
        permissions: Vec::new(),
        variables: Vec::new(),
        settings: Vec::new(),
        commands: Vec::new(),
        linked: false,
        state: None,
    };

    assert!(changed(&root, "echo", &entry).unwrap());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn update_modes_are_explicit() {
    let root = std::env::temp_dir().join(format!("krabb-updates-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(&root).unwrap();

    assert!(super::updates(&root, "off").is_ok());
    assert!(super::updates(&root, "check").is_ok());
    assert!(super::updates(&root, "prompt").is_ok());
    assert!(super::updates(&root, "auto").is_ok());
    assert!(super::updates(&root, "invalid").is_err());

    assert!(
        super::changed(
            &root,
            "missing",
            &super::Entry {
                source: root.join("missing").display().to_string(),
                revision: String::new(),
                pinned: false,
                default: false,
                hash: String::new(),
                version: "0.1.0".into(),
                protocol: Protocol::CURRENT,
                capabilities: Vec::new(),
                permissions: Vec::new(),
                variables: Vec::new(),
                settings: Vec::new(),
                commands: Vec::new(),
                linked: false,
                state: None,
            }
        )
        .is_err()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn manifest_round_trips() {
    let value = Manifest {
        id: "echo".into(),
        version: "0.1.0".into(),
        protocol: Protocol::CURRENT,
        capabilities: vec!["model".into()],
        permissions: Vec::new(),
        variables: Vec::new(),
        settings: Vec::new(),
        setup: BTreeMap::new(),
        check: false,
        commands: Vec::new(),
        state: None,
        requires: Vec::new(),
    };

    let text = toml::to_string(&value).unwrap();
    let parsed: Manifest = toml::from_str(&text).unwrap();

    assert_eq!(parsed.id, "echo");
    assert!(value.validate().is_ok());

    let mut escaped = value.clone();

    escaped.version = "0.1.0\u{1b}[2J".into();

    assert!(escaped.validate().unwrap_err().contains("visible text"));

    let mut described = value;

    described.commands.push(CommandSpec {
        name: "echo".into(),
        description: "Echo\u{1b}]52;c;eA==\u{7}".into(),
        interactive: false,
    });

    assert!(described.validate().is_err());
}

#[test]
fn plugin_commands_reserve_native_names_and_detect_duplicates() {
    assert!(super::native_command("status"));
    assert!(super::native_command("schedule"));
    assert!(super::native_command("memory"));
    assert!(super::native_command("tui"));
    assert!(super::native_command("data"));

    let root = test_root("commands");
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(&root).unwrap();

    let mut lock = super::Lock::default();

    lock.plugins.insert(
        "first".into(),
        super::Entry {
            source: "first".into(),
            revision: String::new(),
            pinned: false,
            default: false,
            hash: String::new(),
            version: "0.1.0".into(),
            protocol: Protocol::CURRENT,
            capabilities: Vec::new(),
            permissions: Vec::new(),
            variables: Vec::new(),
            settings: Vec::new(),
            commands: vec![CommandSpec {
                name: "login".into(),
                description: "Log in.".into(),
                interactive: false,
            }],

            linked: false,
            state: None,
        },
    );

    super::save_lock_at(&root, &lock).unwrap();

    let help = super::help_text_with_plugins(&root).unwrap();

    assert!(help.contains("Plugin Commands:"));
    assert!(help.contains("login"));
    assert!(help.find("Plugin Commands:") < help.find("Options:"));

    assert!(
        super::validate_commands(
            &root,
            "second",
            &[CommandSpec {
                name: "login".into(),
                description: "Log in.".into(),
                interactive: false,
            }]
        )
        .is_err()
    );

    assert!(
        super::validate_commands(
            &root,
            "third-party",
            &[CommandSpec {
                name: "memory".into(),
                description: "Shadow Krabb memory.".into(),
                interactive: false,
            }]
        )
        .is_err()
    );

    assert!(
        super::validate_commands(
            &root,
            "sqlite",
            &[CommandSpec {
                name: "data".into(),
                description: "Manage structured SQLite data.".into(),
                interactive: false,
            }]
        )
        .is_ok()
    );

    assert!(
        super::validate_commands(
            &root,
            "memory",
            &[CommandSpec {
                name: "memory".into(),
                description: "Search and manage memories.".into(),
                interactive: false,
            }]
        )
        .is_ok()
    );

    assert!(
        super::validate_commands(
            &root,
            "tui",
            &[CommandSpec {
                name: "tui".into(),
                description: "Open the terminal interface.".into(),
                interactive: true,
            }]
        )
        .is_ok()
    );

    assert!(
        super::validate_commands(
            &root,
            "second",
            &[CommandSpec {
                name: "status".into(),
                description: "Shadow status.".into(),
                interactive: false,
            }]
        )
        .is_err()
    );

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn conditional_native_commands_follow_installed_capabilities() {
    let root = test_root("ask-command");
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(root.join("plugins/fake/bin")).unwrap();

    let mut lock = super::Lock::default();

    lock.plugins.insert(
        "fake".into(),
        super::Entry {
            source: "local".into(),
            revision: "local".into(),
            pinned: false,
            default: false,
            hash: String::new(),
            version: "0.1.0".into(),
            protocol: Protocol::CURRENT,
            capabilities: vec!["model".into()],
            permissions: Vec::new(),
            variables: Vec::new(),
            settings: Vec::new(),
            linked: false,
            commands: vec![CommandSpec {
                name: "model-console".into(),
                description: "Open the model console.".into(),
                interactive: false,
            }],
            state: None,
        },
    );

    super::save_lock_at(&root, &lock).unwrap();

    assert!(!super::available_commands(&root).unwrap().contains_key("ask"));
    assert!(!super::available_commands(&root).unwrap().contains_key("session"));
    assert!(!super::available_commands(&root).unwrap().contains_key("delivery"));
    assert!(!super::available_commands(&root).unwrap().contains_key("data"));

    let help = super::help_text_with_plugins(&root).unwrap();

    let native = help
        .split("Commands:")
        .nth(1)
        .and_then(|section| section.split("Conditional Commands:").next())
        .unwrap();

    let conditional = help
        .split("Conditional Commands:")
        .nth(1)
        .and_then(|section| section.split("Plugin Commands:").next())
        .unwrap();

    assert!(native.lines().any(|line| line.trim_start().starts_with("ask ")));
    assert!(!conditional.contains("ask"));
    assert!(!conditional.contains("session"));
    assert!(!conditional.contains("delivery"));
    assert!(!native.lines().any(|line| line.trim_start().starts_with("schedule ")));

    let binary = root.join("plugins/fake/bin").join(super::plugin_name("fake"));

    fs::write(binary, b"plugin").unwrap();

    let mut commands = super::available_commands(&root).unwrap();

    assert!(!commands.contains_key("ask"));
    assert_eq!(commands["session"][0].0, "__runtime.session");
    assert!(!commands.contains_key("delivery"));
    assert_eq!(super::model_plugin_at(&root, "fake").unwrap(), "fake");

    let session_args = ["list", "--json"].map(str::to_owned);

    super::session_command(&session_args, &root, false).await.unwrap();

    lock.plugins.insert(
        "channel".into(),
        super::Entry {
            source: "local".into(),
            revision: "local".into(),
            pinned: false,
            default: false,
            hash: String::new(),
            version: "0.1.0".into(),
            protocol: Protocol::CURRENT,
            capabilities: vec!["channel".into()],
            permissions: Vec::new(),
            variables: Vec::new(),
            settings: Vec::new(),
            linked: false,
            commands: Vec::new(),
            state: None,
        },
    );

    super::save_lock_at(&root, &lock).unwrap();

    let binary = root.join("plugins/channel/bin").join(super::plugin_name("channel"));

    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(binary, b"plugin").unwrap();
    commands = super::available_commands(&root).unwrap();

    assert_eq!(commands["delivery"][0].0, "__runtime.delivery");

    let delivery_args = ["list", "--json"].map(str::to_owned);

    super::delivery_command(&delivery_args, &root, false).await.unwrap();

    let help = super::help_text_with_plugins(&root).unwrap();

    let conditional = help
        .split("Conditional Commands:")
        .nth(1)
        .and_then(|section| section.split("Plugin Commands:").next())
        .unwrap();

    let plugin = help.split("Plugin Commands:").nth(1).unwrap();

    assert!(native.lines().any(|line| line.trim_start().starts_with("ask ")));
    assert!(!conditional.contains("ask"));
    assert!(conditional.contains("session"));
    assert!(conditional.contains("delivery"));
    assert!(!conditional.contains("model-console"));
    assert!(plugin.contains("model-console"));

    lock.plugins.insert(
        "scheduler".into(),
        super::Entry {
            source: "local".into(),
            revision: "local".into(),
            pinned: false,
            default: false,
            hash: String::new(),
            version: "0.1.0".into(),
            protocol: Protocol::CURRENT,
            capabilities: vec!["scheduler".into()],
            permissions: Vec::new(),
            variables: Vec::new(),
            settings: Vec::new(),
            linked: false,
            commands: Vec::new(),
            state: None,
        },
    );

    super::save_lock_at(&root, &lock).unwrap();

    assert!(!super::available_commands(&root).unwrap().contains_key("schedule"));

    let binary = root.join("plugins/scheduler/bin").join(super::plugin_name("scheduler"));

    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(binary, b"plugin").unwrap();

    let help = super::help_text_with_plugins(&root).unwrap();

    let native = help
        .split("Commands:")
        .nth(1)
        .and_then(|section| section.split("Conditional Commands:").next())
        .unwrap();

    let conditional = help
        .split("Conditional Commands:")
        .nth(1)
        .and_then(|section| section.split("Plugin Commands:").next())
        .unwrap();

    let plugin = help.split("Plugin Commands:").nth(1).unwrap();

    assert!(!native.lines().any(|line| line.trim_start().starts_with("schedule ")));

    assert_eq!(
        conditional.lines().filter(|line| line.trim_start().starts_with("schedule ")).count(),
        1
    );

    assert!(!plugin.contains("schedule"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn krabb_owned_plugin_commands_are_conditional_native_commands() {
    let root = test_root("first-party-commands");
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(&root).unwrap();

    let entry = |id: &str, capability: &str, name: &str, interactive: bool| super::Entry {
        source: "local".into(),
        revision: "local".into(),
        pinned: false,
        default: false,
        hash: String::new(),
        version: "0.1.0".into(),
        protocol: Protocol::CURRENT,
        capabilities: vec![capability.into()],
        permissions: Vec::new(),
        variables: Vec::new(),
        settings: Vec::new(),
        linked: false,
        commands: vec![CommandSpec {
            name: name.into(),
            description: format!("Run the {id} command."),
            interactive,
        }],
        state: None,
    };

    let mut lock = super::Lock::default();

    lock.plugins.insert("memory".into(), entry("memory", "memory", "memory", false));
    lock.plugins.insert("tui".into(), entry("tui", "client", "tui", true));
    lock.plugins.insert("sqlite".into(), entry("sqlite", "store", "data", false));
    lock.plugins.insert("codex".into(), entry("codex", "model", "codex", false));
    super::save_lock_at(&root, &lock).unwrap();

    let unavailable = super::available_commands(&root).unwrap();

    for name in ["memory", "tui", "data"] {
        assert!(!unavailable.contains_key(name));
    }

    for id in ["memory", "tui", "sqlite"] {
        let binary = root.join("plugins").join(id).join("bin").join(super::plugin_name(id));

        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(binary, b"plugin").unwrap();
    }

    let commands = super::available_commands(&root).unwrap();

    assert_eq!(commands["memory"][0].0, "__runtime.memory");
    assert_eq!(commands["tui"][0].0, "__runtime.tui");
    assert_eq!(commands["data"][0].0, "__runtime.data");
    assert_eq!(commands["codex"][0].0, "codex");

    let help = super::help_text_with_plugins(&root).unwrap();

    let native = help
        .split("Commands:")
        .nth(1)
        .and_then(|section| section.split("Conditional Commands:").next())
        .unwrap();

    let conditional = help
        .split("Conditional Commands:")
        .nth(1)
        .and_then(|section| section.split("Plugin Commands:").next())
        .unwrap();

    let plugin = help.split("Plugin Commands:").nth(1).unwrap();

    for name in ["memory", "tui", "data"] {
        assert!(!native.lines().any(|line| line.trim_start().starts_with(&format!("{name} "))));

        assert!(conditional.lines().any(|line| line.trim_start().starts_with(&format!("{name} "))));

        assert!(!plugin.lines().any(|line| line.trim_start().starts_with(&format!("{name} "))));
    }

    assert!(plugin.lines().any(|line| line.trim_start().starts_with("codex ")));

    assert!(
        super::validate_commands(
            &root,
            "third-party",
            &[CommandSpec {
                name: "memory".into(),
                description: "Shadow memory.".into(),
                interactive: false,
            }]
        )
        .is_err()
    );

    assert!(
        super::validate_commands(
            &root,
            "third-party",
            &[CommandSpec {
                name: "data".into(),
                description: "Shadow SQLite data.".into(),
                interactive: false,
            }]
        )
        .is_err()
    );

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn plugin_removal_preserves_state_owned_by_conditional_commands() {
    let root = test_root("plugin-removal-state");
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(&root).unwrap();

    let entry = |capability: &str| super::Entry {
        source: "local".into(),
        revision: "local".into(),
        pinned: false,
        default: false,
        hash: String::new(),
        version: "0.1.0".into(),
        protocol: Protocol::CURRENT,
        capabilities: vec![capability.into()],
        permissions: Vec::new(),
        variables: Vec::new(),
        settings: Vec::new(),
        linked: false,
        commands: Vec::new(),
        state: None,
    };

    let catalog = root.join("data/sessions/index.json");
    let mut store = super::state::Store::load(&catalog).unwrap();

    store.create("kept", "test").unwrap();
    store.save().unwrap();

    let mut lock = super::Lock::default();

    lock.plugins.insert("model-one".into(), entry("model"));

    assert_eq!(super::last_plugin_state(&lock, "model-one", &root), (true, false));

    assert!(
        super::ensure_no_orphaned_plugin_state(&lock, "model-one", &root)
            .unwrap_err()
            .to_string()
            .contains("krabb session delete")
    );

    let second_model = "second";

    fs::create_dir_all(root.join("plugins").join(second_model).join("bin")).unwrap();

    fs::write(
        root.join("plugins").join(second_model).join("bin").join(super::plugin_name(second_model)),
        "plugin",
    )
    .unwrap();

    lock.plugins.insert(second_model.into(), entry("model"));

    assert_eq!(super::last_plugin_state(&lock, "model-one", &root), (false, false));
    assert!(super::ensure_no_orphaned_plugin_state(&lock, "model-one", &root).is_ok());

    let mut store = super::state::Store::load(&catalog).unwrap();

    store
        .reply(
            "kept",
            Message {
                id: "delivery-user".into(),
                session: "kept".into(),
                role: Role::User,
                sender: None,
                content: vec![Content::Text { text: "pending".into() }],
            },
            "delivery-1",
            "test",
            "chat",
            None,
            "pending",
        )
        .unwrap();

    store.save().unwrap();

    lock.plugins.clear();
    lock.plugins.insert("channel-one".into(), entry("channel"));

    assert_eq!(super::last_plugin_state(&lock, "channel-one", &root), (false, true));

    super::save_lock_at(&root, &lock).unwrap();

    let error =
        super::remove_at(super::PluginRemove { id: "channel-one".into(), yes: true }, &root)
            .unwrap_err();

    assert!(error.to_string().contains("krabb delivery"), "{error}");
    assert!(super::load_lock_at(&root).unwrap().plugins.contains_key("channel-one"));

    let purged = super::purge_state_at(&root, false, true).await.unwrap();

    assert_eq!(purged["deliveries"], 1);
    assert!(super::sessions_at(&root).unwrap().outbox.is_empty());

    let purged = super::purge_state_at(&root, true, false).await.unwrap();

    assert_eq!(purged["sessions"], 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn banner_rows_have_consistent_width() {
    let widths = BANNER
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().count())
        .collect::<Vec<_>>();

    assert!(!widths.is_empty());
    assert!(widths.iter().all(|width| *width == widths[0]));
    assert!(BANNER.contains("██╗  ██╗"));
    assert!(BANNER.contains("███████║"));
    assert!(BANNER.contains("╚═════╝"));
    assert!(!BANNER.ends_with('\n'));
}

#[test]
fn narrow_terminals_get_a_banner_that_does_not_wrap() {
    let width = BANNER.lines().map(|line| line.chars().count()).max().unwrap();

    assert!(width < usize::from(BANNER_COLUMNS));
    assert_eq!(banner(BANNER_COLUMNS), BANNER);
    assert_eq!(banner(BANNER_COLUMNS - 1), "\nKrabb");
    assert_eq!(banner(0), "\nKrabb");
}

#[test]
fn cli_exposes_global_modes_and_aliases() {
    let cli = Cli::try_parse_from(["krabb", "--json", "--debug", "--verbose", "status"]).unwrap();

    assert!(cli.json);
    assert!(cli.debug);
    assert!(cli.verbose);

    let cli = Cli::try_parse_from(["krabb", "version", "--json"]).unwrap();

    assert!(matches!(cli.command, Command::Version(Output { json: true })));

    let cli = Cli::try_parse_from(["krabb", "status", "--json"]).unwrap();

    assert!(matches!(cli.command, Command::Status(Output { json: true })));

    let cli = Cli::try_parse_from(["krabb", "--json", "doctor"]).unwrap();

    assert!(cli.json);

    let cli = Cli::try_parse_from(["krabb", "doctor", "--fix"]).unwrap();

    assert!(matches!(cli.command, Command::Doctor(DoctorArgs { fix: true })));

    let cli = Cli::try_parse_from(["krabb", "service", "status", "--json"]).unwrap();

    assert!(cli.json);

    let cli = Cli::try_parse_from(["krabb", "service", "restart"]).unwrap();

    assert!(matches!(cli.command, Command::Service { command: ServiceCommand::Restart }));

    let error = Cli::try_parse_from(["krabb", "service"]).unwrap_err();
    let help = error.to_string();

    assert_eq!(error.kind(), ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand);
    assert!(help.contains("Usage: krabb service"));
    assert!(help.contains("status"));
    assert!(help.contains("start"));
    assert!(help.contains("stop"));
    assert!(help.contains("restart"));
    assert!(help.contains("--json"));
    assert!(help.contains("--debug"));
    assert!(help.contains("--verbose"));
    assert!(help.contains("alias: -H"));

    let cli = Cli::try_parse_from(["krabb", "service", "install", "--force"]).unwrap();

    assert!(matches!(
        cli.command,
        Command::Service {
            command: ServiceCommand::Install(super::ServiceInstall { force: true })
        }
    ));

    let cli = Cli::try_parse_from(["krabb", "service", "uninstall", "-y"]).unwrap();

    assert!(matches!(
        cli.command,
        Command::Service { command: ServiceCommand::Remove(super::ServiceRemove { yes: true }) }
    ));

    let cli =
        Cli::try_parse_from(["krabb", "plugin", "uninstall", "tools", "-y", "--force"]).unwrap();

    assert!(matches!(
        cli.command,
        Command::Plugin {
            command: super::PluginCommand::Remove(super::PluginUninstall {
                ids,
                yes: true,
                force: true,
            })
        } if ids == ["tools"]
    ));

    let cli =
        Cli::try_parse_from(["krabb", "plugin", "uninstall", "codex", "telegram", "-y"]).unwrap();

    assert!(matches!(
        cli.command,
        Command::Plugin {
            command: super::PluginCommand::Remove(super::PluginUninstall {
                ids,
                yes: true,
                force: false,
            })
        } if ids == ["codex", "telegram"]
    ));

    assert!(matches!(
        Cli::try_parse_from(["krabb", "plugin", "remove", "tools", "-y"]).unwrap().command,
        Command::Plugin { command: super::PluginCommand::Remove(super::PluginUninstall {
            ids, yes: true, force: false,
        }) } if ids == ["tools"]
    ));

    assert!(Cli::try_parse_from(["krabb", "service", "remove", "-y"]).is_err());

    let cli = Cli::try_parse_from([
        "krabb", "plugin", "install", "tui", "codex", "tools", "--link", "-y",
    ])
    .unwrap();

    assert!(matches!(
        cli.command,
        Command::Plugin {
            command: super::PluginCommand::Install(super::PluginInstall {
                ids,
                source: None,
                revision: None,
                link: true,
                unstable: false,
                yes: true,
                quiet: false,
            })
        } if ids == ["tui", "codex", "tools"]
    ));

    let cli = Cli::try_parse_from([
        "krabb",
        "plugin",
        "install",
        "custom",
        "--source",
        "./plugin",
        "--revision",
        "abc123",
    ])
    .unwrap();

    assert!(matches!(
        cli.command,
        Command::Plugin {
            command: super::PluginCommand::Install(super::PluginInstall {
                ids,
                source: Some(source),
                revision: Some(revision),
                link: false,
                unstable: false,
                yes: false,
                quiet: false,
            })
        } if ids == ["custom"] && source == "./plugin" && revision == "abc123"
    ));

    let cli = Cli::try_parse_from(["krabb", "plugin", "update", "-y"]).unwrap();

    assert!(matches!(
        cli.command,
        Command::Plugin {
            command: super::PluginCommand::Update(super::PluginUpdate { yes: true, json: false })
        }
    ));

    let tui_help =
        super::external_command_help("tui", "Open the TUI; the Krabb daemon must be running.");

    assert!(tui_help.contains("Usage: krabb tui [OPTIONS]"));
    assert!(tui_help.contains("daemon must be running"));
    assert!(tui_help.contains("--once <prompt>"));
    assert!(tui_help.contains("environment, config, or default"));
    assert!(super::external_command_help_requested("tui", &["tui".into(), "help".into()]));
    assert!(super::external_command_help_requested("tui", &["tui".into(), "--help".into()]));
    assert!(!super::external_command_help_requested("codex", &["codex".into(), "help".into()]));

    let codex_help =
        super::external_command_help("codex", "Manage Codex sign-in and list available models.");

    assert!(codex_help.contains("login [--device]"));
    assert!(codex_help.contains("  models            List models"));
    assert!(!codex_help.contains("models list"));

    assert!(
        super::external_command_output(&serde_json::json!({"text": "Signed in."}), false)
            .unwrap()
            .contains("Signed in.")
    );

    assert!(
        super::external_command_output(&serde_json::json!({"text": "Signed in."}), true)
            .unwrap()
            .contains("\"text\"")
    );

    assert!(
        super::external_command_help("memory", "Manage memory.")
            .contains("Usage: krabb memory [arguments...]")
    );

    assert_eq!(super::external_command_args("memory", &["--help".into()]), ["help"]);
    assert_eq!(super::external_command_args("data", &["--help".into()]), ["help"]);

    assert_eq!(
        super::external_command_args("data", &["table".into(), "create".into(), "--help".into()]),
        ["help"]
    );

    assert_eq!(
        super::external_command_args("data", &["--json".into(), "database".into(), "list".into()]),
        ["database", "list"]
    );

    assert_eq!(
        super::external_command_args("memory", &["search".into(), "tea".into()]),
        ["search", "tea"]
    );

    assert_eq!(
        super::format_data_command_output(
            &["database".into(), "list".into()],
            &serde_json::json!({"databases": []})
        ),
        "No databases found."
    );

    assert_eq!(
        super::format_data_command_output(
            &["database".into(), "list".into()],
            &serde_json::json!({"databases": ["foo", "bar"]})
        ),
        "Databases:\n├─ foo\n└─ bar"
    );

    assert_eq!(
        super::format_data_command_output(
            &["table".into(), "list".into(), "foo".into()],
            &serde_json::json!({"tables": ["bar", "baz"]})
        ),
        "Tables in foo:\n├─ bar\n└─ baz"
    );

    assert_eq!(
        super::format_data_command_output(
            &["table".into(), "create".into(), "notes".into(), "note".into(),],
            &serde_json::json!({"columns": []})
        ),
        "Created empty table notes.note.\n\nAdd a column with:\n  krabb data column add notes note NAME TYPE"
    );

    let structured = serde_json::json!({"databases": ["notes"]});

    assert_eq!(
        super::external_command_output(&structured, true).unwrap(),
        "{\n  \"databases\": [\n    \"notes\"\n  ]\n}"
    );

    let session =
        SessionCli::try_parse_from(["session", "new", "main", "--model", "test", "--json"])
            .unwrap();

    assert!(session.json);

    let session = SessionCli::try_parse_from(["session", "new", "main"]).unwrap();

    assert!(matches!(session.command, SessionCommand::New(SessionNew { model: None, .. })));

    let session = SessionCli::try_parse_from(["session", "show", "main", "--json"]).unwrap();

    assert!(session.json);

    let session =
        SessionCli::try_parse_from(["session", "delete", "main", "--yes", "--json"]).unwrap();

    assert!(session.json);

    let delivery =
        DeliveryCli::try_parse_from(["delivery", "retry", "item", "--yes", "--json"]).unwrap();

    assert!(delivery.json);

    let cli = Cli::try_parse_from(["krabb", "export", "--json"]).unwrap();

    assert!(cli.json);

    let cli = Cli::try_parse_from(["krabb", "export", "."]).unwrap();

    assert!(matches!(
        cli.command,
        Command::Export(KrabfileExport { destination: Some(path), path: None, force: false })
            if path == Path::new(".")
    ));

    let cli = Cli::try_parse_from(["krabb", "export", "--force"]).unwrap();

    assert!(matches!(cli.command, Command::Export(KrabfileExport { force: true, .. })));

    let cli = Cli::try_parse_from(["krabb", "validate", "--path", "Krabfile", "--json"]).unwrap();

    assert!(cli.json);

    assert!(
        matches!(cli.command, Command::Validate(KrabfileValidate { path: Some(path) }) if path == Path::new("Krabfile"))
    );

    assert_eq!(default_krabfile_path(), Path::new("./Krabfile"));

    assert_eq!(
        krabfile_output_path(None, Some(PathBuf::from("."))).unwrap(),
        Path::new("./Krabfile")
    );

    assert!(krabfile_output_path(Some(PathBuf::from("one")), Some(PathBuf::from("two"))).is_err());

    let help = Cli::command().render_help().to_string();

    assert!(help.contains("Initialize Krabb configuration and its global workspace."));
    assert!(help.contains("-h, --help"));
    assert!(help.contains("alias: -H"));
    assert!(help.contains("-v, --version"));
    assert!(help.contains("--verbose"));
    assert!(!help.contains("--verbose..."));
    assert!(!help.contains("Plugin Commands:"));
    assert!(!help.lines().any(|line| line.trim_start().starts_with("schedule ")));
    assert!(!help.lines().any(|line| line.trim_start().starts_with("memory ")));
    assert!(!help.lines().any(|line| line.trim_start().starts_with("tui ")));

    for argument in ["-h", "-H", "--help"] {
        let error = Cli::try_parse_from(["krabb", argument]).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
    }

    for argument in ["-v", "-V", "--version"] {
        let error = Cli::try_parse_from(["krabb", argument]).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::DisplayVersion);
    }
}

#[test]
fn plugin_install_accepts_multiple_ids_and_limits_explicit_sources() {
    let (linked, sources) = super::plugin_sources(super::PluginInstall {
        ids: vec!["tui".into(), "codex".into()],
        source: None,
        revision: None,
        link: true,
        unstable: false,
        yes: true,
        quiet: false,
    })
    .unwrap();

    assert!(linked);

    assert_eq!(
        sources.iter().map(|source| source.id.as_str()).collect::<Vec<_>>(),
        ["tui", "codex"]
    );

    assert!(sources.iter().all(|source| source.yes));

    assert!(install_sources(&["tui", "codex"], Some("./plugin"), false, false).is_err());
    assert!(install_sources(&["tui", "tui"], None, false, false).is_err());
}

#[test]
fn state_commands_check_the_home_schema_first() {
    let gated = |args: &[&str]| super::schema_gated(&Cli::try_parse_from(args).unwrap().command);

    assert!(gated(&["krabb", "status"]));
    assert!(gated(&["krabb", "init"]));
    assert!(gated(&["krabb", "plugin", "install", "tui"]));
    assert!(!gated(&["krabb", "init", "--force"]));
    assert!(!gated(&["krabb", "version"]));
    assert!(!gated(&["krabb", "completion", "bash"]));
    assert!(!gated(&["krabb", "service", "stop"]));
}

type Sources = Result<(bool, Vec<super::Source>), Box<dyn std::error::Error + Send + Sync>>;

fn install_sources(ids: &[&str], source: Option<&str>, link: bool, unstable: bool) -> Sources {
    super::plugin_sources(super::PluginInstall {
        ids: ids.iter().map(|id| (*id).into()).collect(),
        source: source.map(Into::into),
        revision: None,
        link,
        unstable,
        yes: false,
        quiet: false,
    })
}

#[test]
fn unstable_plugin_installs_resolve_from_the_index_only() {
    let install = |ids: &[&str], source: Option<&str>, link: bool| {
        install_sources(ids, source, link, true)
            .map(|(_, sources)| sources.len())
            .map_err(|error| error.to_string())
    };

    assert_eq!(install(&["tui", "codex"], None, false), Ok(2));

    assert!(install(&["tui@0.1.0"], None, false).unwrap_err().contains("plugin version"));
    assert!(install(&["tui@latest"], None, false).unwrap_err().contains("plugin version"));
    assert!(install(&["tui"], Some("./plugin"), false).unwrap_err().contains("--source"));
    assert!(install(&["tui"], None, true).unwrap_err().contains("--link"));
}

#[test]
fn plugin_install_accepts_pinned_release_versions() {
    let install =
        |ids: &[&str], source: Option<&str>, link: bool| install_sources(ids, source, link, false);

    let (_, sources) = install(&["tui@0.2.0-rc.1", "codex", "slack@latest"], None, false).unwrap();

    assert_eq!(sources[0].id, "tui");
    assert_eq!(sources[0].version.as_deref(), Some("0.2.0-rc.1"));
    assert_eq!(sources[1].version, None);
    assert_eq!(sources[2].version, None);

    assert!(matches!(
        Cli::try_parse_from(["krabb", "plugin", "versions", "tui"]).unwrap().command,
        Command::Plugin { command: crate::cli::PluginCommand::Versions(Id { id }) } if id == "tui"
    ));

    assert!(install(&["tui@main"], None, false).is_err());
    assert!(install(&["tui@0.1.0"], None, true).is_err());
    assert!(install(&["tui@0.1.0"], Some("./tui"), false).is_err());
    assert!(install(&["tui@0.1.0", "tui@0.2.0"], None, false).is_err());
    assert!(install(&["@0.1.0"], None, false).is_err());
}

#[test]
fn linked_plugin_install_requires_a_plugin_source_in_the_working_directory() {
    let root = test_root("linkable-plugin-source");

    let sources = [super::Source {
        id: "memory".into(),
        source: None,
        revision: None,
        version: None,
        yes: true,
    }];

    assert_eq!(
        super::ensure_linkable_sources(true, &sources, &root).unwrap_err().to_string(),
        concat!(
            "No linkable plugins were found in the current working directory. ",
            "Run this command from the repository root or specify a local plugin with --source PATH."
        )
    );

    let plugin = root.join("krabb-plugins/memory");

    fs::create_dir_all(&plugin).unwrap();
    super::ensure_linkable_sources(true, &sources, &root).unwrap();
    super::ensure_linkable_sources(false, &sources, &root).unwrap();

    let explicit = [super::Source {
        id: "memory".into(),
        source: Some("./memory".into()),
        revision: None,
        version: None,
        yes: true,
    }];

    super::ensure_linkable_sources(true, &explicit, &root).unwrap();

    let _ = fs::remove_dir_all(root);
}

#[test]
fn root_help_detection_leaves_subcommand_help_to_clap() {
    assert!(super::root_help_requested(&[]));
    assert!(super::root_help_requested(&["help".into()]));
    assert!(super::root_help_requested(&["--json".into(), "help".into()]));
    assert!(super::root_help_requested(&["--help".into()]));
    assert!(!super::root_help_requested(&["help".into(), "status".into()]));
    assert!(!super::root_help_requested(&["status".into(), "--help".into()]));
}

#[test]
fn cli_parses_completion_shells_and_generates_commands() {
    for (name, expected) in [
        ("bash", "complete -F"),
        ("fish", "complete -c krabb"),
        ("powershell", "Register-ArgumentCompleter"),
        ("zsh", "#compdef krabb"),
    ] {
        let cli = Cli::try_parse_from(["krabb", "completion", name]).unwrap();
        let Command::Completion { shell } = cli.command else {
            panic!("completion command was not parsed");
        };

        let mut output = Vec::new();
        let mut command = super::completion_command();
        generate_completion(shell, &mut command, NAME, &mut output);
        let output = String::from_utf8(output).unwrap();

        assert!(output.contains(expected), "completion output for {name} was unexpected");
        assert!(output.contains("schedule"), "schedule missing from {name} completion");
        assert!(output.contains("data"), "data missing from {name} completion");
        assert!(output.contains("retry"), "schedule actions missing from {name} completion");
        assert!(output.contains("setup"), "plugin setup missing from {name} completion");
        assert!(output.contains("worktree"), "worktree missing from {name} completion");
        assert!(output.contains("no-activate"), "setup options missing from {name} completion");
    }

    assert!(Cli::try_parse_from(["krabb", "completion", "nu"]).is_err());

    let schedule = Cli::try_parse_from([
        "krabb",
        "schedule",
        "add",
        "--after",
        "300",
        "--retries",
        "2",
        "check the backup",
    ])
    .unwrap();

    assert!(matches!(
        schedule.command,
        Command::Schedule(super::ScheduleArgs {
            command: Some(super::ScheduleCommand::Add(super::ScheduleValues { values }))
        }) if values == ["--after", "300", "--retries", "2", "check the backup"]
    ));

    let schedule_help = Cli::try_parse_from(["krabb", "schedule", "help"]).unwrap();

    assert!(matches!(
        schedule_help.command,
        Command::Schedule(super::ScheduleArgs { command: Some(super::ScheduleCommand::Help) })
    ));

    assert!(super::schedule_help().contains("--shell"));
    assert!(super::schedule_help().contains("--cron <EXPRESSION>"));
    assert!(super::schedule_help().contains("--missed <POLICY>"));
    assert!(super::schedule_help().contains("--retries <COUNT>"));
    assert!(super::schedule_help().contains("--on-failure <POLICY>"));
    assert!(super::schedule_help().contains("--message-channel"));
    assert!(super::schedule_help().contains("--message-thread"));
    assert!(super::schedule_help().contains("Options:"));

    assert!(
        super::schedule_help()
            .contains("  approve <ID> [-y|--yes]   Approve a pending task without prompting")
    );

    assert!(!super::schedule_help().contains("For recurring schedules, add"));

    let memory = Cli::try_parse_from(["krabb", "memory", "search", "banana"]).unwrap();

    assert!(matches!(
        memory.command,
        Command::Memory(super::NativePluginCommandArgs { args })
            if args == ["search", "banana"]
    ));

    let tui = Cli::try_parse_from(["krabb", "tui", "--once", "ping"]).unwrap();

    assert!(matches!(
        tui.command,
        Command::Tui(super::NativePluginCommandArgs { args }) if args == ["--once", "ping"]
    ));

    let data = Cli::try_parse_from(["krabb", "data", "database", "list"]).unwrap();

    assert!(matches!(
        data.command,
        Command::Data(super::NativePluginCommandArgs { args })
            if args == ["database", "list"]
    ));

    let data_help = Cli::try_parse_from(["krabb", "data", "help"]).unwrap();

    assert!(matches!(
        data_help.command,
        Command::Data(super::NativePluginCommandArgs { args }) if args == ["help"]
    ));

    let memory_help = Cli::try_parse_from(["krabb", "memory", "help"]).unwrap();

    assert!(matches!(
        memory_help.command,
        Command::Memory(super::NativePluginCommandArgs { args }) if args == ["help"]
    ));

    assert_eq!(completion_name_from(Some(std::ffi::OsStr::new("/usr/bin/krabb"))), "krabb");
    assert_eq!(completion_name_from(Some(std::ffi::OsStr::new("C:\\bin\\krabb.exe"))), "krabb");
    assert_eq!(completion_name_from(Some(std::ffi::OsStr::new("/usr/bin/krabb"))), NAME);
    assert_eq!(completion_name_from(Some(std::ffi::OsStr::new("other"))), NAME);

    let cli = Cli::try_parse_from(["krabb", "init", "--force", "--yes"]).unwrap();

    assert!(matches!(cli.command, Command::Init(InitArgs { force: true, yes: true, .. })));
    assert!(Cli::try_parse_from(["krabb", "init", "--yes"]).is_ok());
    assert!(super::validate_init_flags(false, true).is_err());
    assert!(super::validate_init_flags(true, true).is_ok());
}

#[test]
fn init_preserves_existing_state_and_force_resets_the_home() {
    let root = test_root("init-force");
    let _ = fs::remove_dir_all(&root);

    super::init_at_with_force(&root, false).unwrap();
    fs::write(root.join("config.toml"), "update = 'auto'\n").unwrap();
    fs::write(root.join("plugins/keep"), "plugin state").unwrap();
    fs::write(root.join("sessions.json"), "session state").unwrap();
    fs::write(root.join("data/system/KRAB.md"), "custom identity").unwrap();
    fs::write(root.join("data/system/KLAW.md"), "custom behavior").unwrap();

    super::init_at_with_force(&root, false).unwrap();

    assert_eq!(fs::read_to_string(root.join("config.toml")).unwrap(), "update = 'auto'\n");
    assert!(root.join("plugins/keep").exists());

    assert_eq!(fs::read_to_string(root.join("data/system/KRAB.md")).unwrap(), "custom identity");

    let active = super::lock_at(&root, "daemon.lock").unwrap();

    assert!(super::reset_home(&root).is_err());
    assert!(root.join("plugins/keep").exists());

    drop(active);

    super::init_at_with_force(&root, true).unwrap();

    assert!(fs::read_to_string(root.join("config.toml")).unwrap().contains("update = \"prompt\""));

    assert!(!root.join("plugins/keep").exists());
    assert!(!root.join("sessions.json").exists());

    let personality = fs::read_to_string(root.join("data/system/KRAB.md")).unwrap();
    let behavior = fs::read_to_string(root.join("data/system/KLAW.md")).unwrap();

    assert!(personality.contains("Personality Instructions"));
    assert!(personality.contains("You are Krabb"));
    assert!(personality.contains("Plain and direct"));
    assert!(behavior.contains("Behavioral Instructions"));
    assert!(behavior.contains("Verify important claims"));

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn init_refuses_to_reset_a_symlinked_home() {
    use std::os::unix::fs::symlink;

    let parent = test_root("init-symlink");
    let target = parent.join("target");
    let home = parent.join("home");
    let _ = fs::remove_dir_all(&parent);

    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("keep"), "user data").unwrap();
    symlink(&target, &home).unwrap();

    assert!(super::reset_home(&home).is_err());
    assert_eq!(fs::read_to_string(target.join("keep")).unwrap(), "user data");

    let _ = fs::remove_dir_all(parent);
}

#[test]
fn doctor_fix_creates_only_missing_safe_state() {
    let root = test_root("doctor-fix");
    let _ = fs::remove_dir_all(&root);

    super::doctor_at(&root, false, false).unwrap();

    assert!(!root.exists());

    super::doctor_at(&root, true, false).unwrap();

    assert!(root.join("config.toml").is_file());
    assert!(root.join("plugins").is_dir());
    assert!(root.join("data/system/KRAB.md").is_file());
    assert!(root.join("data/system/KLAW.md").is_file());

    fs::write(root.join("data/system/KRAB.md"), "custom identity").unwrap();
    fs::write(root.join("config.toml"), "update = 'auto'\n").unwrap();
    super::doctor_at(&root, true, false).unwrap();

    assert_eq!(fs::read_to_string(root.join("config.toml")).unwrap(), "update = 'auto'\n");

    assert_eq!(fs::read_to_string(root.join("data/system/KRAB.md")).unwrap(), "custom identity");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn doctor_health_requires_valid_required_state() {
    assert!(super::doctor_healthy(true, Some(true), true, true, true, true));
    assert!(!super::doctor_healthy(false, None, true, true, true, true));
    assert!(!super::doctor_healthy(true, Some(false), true, true, true, true));
    assert!(!super::doctor_healthy(true, Some(true), false, true, true, true));
    assert!(!super::doctor_healthy(true, Some(true), true, false, true, true));
    assert!(!super::doctor_healthy(true, Some(true), true, true, false, true));
    assert!(!super::doctor_healthy(true, Some(true), true, true, true, false));
}

#[test]
fn command_parser_keeps_prompt_words() {
    let cli = Cli::try_parse_from([
        "krabb",
        "ask",
        "--provider",
        "local",
        "--model",
        "local",
        "hello",
        "world",
    ])
    .unwrap();

    assert!(matches!(
        cli.command,
        Command::Ask(super::Ask { prompt, provider, .. })
            if prompt == ["hello", "world"] && provider.as_deref() == Some("local")
    ));

    assert!(Cli::try_parse_from(["krabb", "ask", "--provider", "local"]).is_err());
    assert!(Cli::try_parse_from(["krabb", "ask", "--provider", "local", "hello"]).is_ok());
    assert!(Cli::try_parse_from(["krabb", "ask", "hello"]).is_ok());
    assert!(Cli::try_parse_from(["krabb", "ask", "--plugin", "local", "hello"]).is_err());

    for args in [
        vec!["krabb", "ask", "hello", "--model", "literal"],
        vec!["krabb", "ask", "--", "--model", "literal"],
    ] {
        let cli = Cli::try_parse_from(args).unwrap();

        assert!(matches!(cli.command, Command::Ask(super::Ask { prompt, model: None, .. })
                if prompt.ends_with(&["--model".into(), "literal".into()])));
    }

    let help = Cli::command().find_subcommand("ask").unwrap().clone().render_help().to_string();

    assert!(help.contains("--provider <PROVIDER>"));
    assert!(help.contains("--model <MODEL>"));
    assert!(!help.contains("--plugin"));
    assert!(help.contains("--quote <QUOTE>"));

    let quoted =
        Cli::try_parse_from(["krabb", "ask", "--quote", "earlier answer", "explain"]).unwrap();

    assert!(
        matches!(quoted.command, Command::Ask(super::Ask { quote: Some(quote), prompt, .. }) if quote == "earlier answer" && prompt == ["explain"])
    );

    let cli =
        Cli::try_parse_from(["krabb", "ask", "--provider", "local", "--model", "local", "hello"])
            .unwrap();

    assert!(matches!(cli.command, Command::Ask(super::Ask { prompt, .. }) if prompt == ["hello"]));
}

#[test]
fn manifest_reader_rejects_wrong_files() {
    let root = std::env::temp_dir().join(format!("krabb-manifest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("krabb-plugin.toml"), "id = 'echo'\nversion = '0.1.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['model']\n").unwrap();

    assert_eq!(read_manifest(&root).unwrap().id, "echo");

    fs::write(root.join("krabb-plugin.toml"), "broken = true\n").unwrap();

    assert!(read_manifest(&root).is_none());

    let error = read_manifest_checked(&root).unwrap_err().to_string();

    assert!(error.contains("manifest parse failed at"));
    assert!(error.contains("krabb-plugin.toml"));

    fs::write(
            root.join("krabb-plugin.toml"),
            "id = 'Echo'\nversion = '0.1.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['model']\n",
        )
        .unwrap();

    let error = read_manifest_checked(&root).unwrap_err().to_string();

    assert!(error.contains("manifest validation failed at"));
    assert!(error.contains("Plugin IDs must contain lowercase letters"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn plugin_list_rejects_invalid_visible_manifests() {
    let root = test_root("plugin-list-invalid");

    fs::create_dir_all(root.join("plugins/broken")).unwrap();
    fs::write(root.join("plugins/broken/krabb-plugin.toml"), "broken = true\n").unwrap();

    assert!(super::list_at(&root, None, 1, true).is_err());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn formats_plugin_inventory_as_a_tree() {
    let output = super::format_plugin_list(
        &[serde_json::json!({
            "id": "codex",
            "version": "1.2.3",
            "health": "ready",
            "protocol": {"major": 0, "minor": 1},
            "capabilities": ["model", "vision"],
            "commands": [{"name": "codex"}],
            "permissions": ["network", "process"]
        })],
        1,
    )
    .unwrap();

    assert_eq!(
        output,
        "Plugins (1–1 of 1, page 1 of 1)\n└─ codex@v1.2.3\n   ├─ Ready\n   ├─ Capabilities include model, vision\n   ├─ Uses protocol version 0.1\n   ├─ Commands include codex\n   └─ Permissions include network, process"
    );
}

#[test]
fn plugin_inventory_describes_each_runtime_state_with_its_next_step() {
    let manifest: super::Manifest = toml::from_str(
            "id = 'telegram'\nversion = '0.1.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['channel']\nsecrets = ['KRABB_TELEGRAM_TOKEN']\n",
        )
        .unwrap();

    let item = super::plugin_summary(&manifest, "ready");
    let output = super::format_plugin_list(std::slice::from_ref(&item), 1).unwrap();

    assert_eq!(item["setup_command"], "krabb plugin setup telegram");
    assert!(output.contains("Ready"));
    assert!(output.contains("Set up credentials with `krabb plugin setup telegram`"));
    assert!(!output.contains("Health is ready"));
    assert_eq!(item["commands"], serde_json::json!([]));

    let item = super::plugin_summary(&manifest, "missing");
    let output = super::format_plugin_list(&[item], 1).unwrap();

    assert!(output.contains("Binary is missing, so reinstall the plugin from its source"));

    let line = |health: &str, reason: Option<&str>, next: Option<&str>| {
        let mut item = super::plugin_summary(&manifest, health);

        item["reason"] = reason.into();
        item["next"] = next.into();

        let output = super::format_plugin_list(&[item], 1).unwrap();

        output.lines().nth(2).unwrap().trim_start_matches("   ├─ ").to_owned()
    };

    assert_eq!(line("installed", None, None), "Installed");
    assert_eq!(line("needs setup", None, Some("krabb plugin setup telegram")), "Needs setup");

    assert_eq!(
        line("configured", None, Some("krabb service start")),
        "Configured, but the runtime is not running, so run `krabb service start`"
    );

    assert_eq!(
        line("failed to start", Some("Rejected."), Some("krabb plugin setup telegram")),
        "Failed to start: Rejected, so run `krabb plugin setup telegram`"
    );

    assert_eq!(line("failed to start", None, None), "Failed to start");
    assert_eq!(line("missing program", None, None), "Missing a program it runs");

    assert_eq!(
        line("missing program", None, Some("Install signal-cli.")),
        "Missing a program it runs: Install signal-cli"
    );
}

#[test]
fn plugin_inventory_uses_tree_rows_and_pages_of_five() {
    let items = (0..7)
        .rev()
        .map(|index| {
            serde_json::json!({
                "id": format!("plugin-{index:02}"),
                "version": "1.0.0",
                "health": "ready",
                "protocol": {"major": 0, "minor": 1},
                "capabilities": ["model"],
                "commands": [],
                "permissions": []
            })
        })
        .collect::<Vec<_>>();

    let first_page = super::format_plugin_list(&items, 1).unwrap();

    assert!(first_page.starts_with("Plugins (1–5 of 7, page 1 of 2)\n├─ plugin-00@v1.0.0"));
    assert!(first_page.contains("│  └─ No permissions\n\n├─ plugin-01"));
    assert!(!first_page.contains("plugin-05"));

    let second_page = super::format_plugin_list(&items, 2).unwrap();

    assert!(second_page.starts_with("Plugins (6–7 of 7, page 2 of 2)\n├─ plugin-05"));
    assert!(second_page.contains("└─ plugin-06@v1.0.0"));
    assert!(super::format_plugin_list(&items, 3).unwrap_err().contains("1 to 2"));
    assert_eq!(super::format_plugin_list(&[], 1).unwrap(), super::NO_PLUGINS);
    assert!(super::NO_PLUGINS.ends_with("Install one with `krabb plugin install <id>`."));
}

#[test]
fn session_inventory_prioritizes_working_sessions_and_paginates() {
    let items = (0..7)
        .map(|index| {
            serde_json::json!({
                "id": format!("session-{index:02}"),
                "model": "provider/model",
                "status": "idle",
                "created": 1735776000,
                "updated": 1735862400 + index,
                "messages": 3
            })
        })
        .collect::<Vec<_>>();

    let mut items = items;

    items[0]["status"] = serde_json::json!("working");
    items[6]["archived"] = serde_json::json!(true);

    let value = serde_json::json!({"items": items});

    let first_page = super::format_session_list(&value, 1).unwrap();

    assert!(first_page.starts_with("Sessions (1–5 of 7, page 1 of 2)\n├─ session-00 | working"));

    assert!(first_page.contains("│  ├─ Uses model provider/model"));
    assert!(first_page.contains("Created on 2025-01-02"));
    assert!(first_page.contains("Has 3 messages\n\n"));
    assert!(!first_page.contains("session-06"));

    let second_page = super::format_session_list(&value, 2).unwrap();

    assert!(second_page.starts_with("Sessions (6–7 of 7, page 2 of 2)"));
    assert!(second_page.contains("session-06 | archived"));
    assert!(super::format_session_list(&value, 3).unwrap_err().contains("sessions"));

    assert_eq!(
        super::format_session_list(&serde_json::json!({"items": []}), 1).unwrap(),
        "No sessions yet.\nStart a conversation with `krabb tui`."
    );
}

#[test]
fn delivery_inventory_groups_details_and_pages() {
    let items = (0..6)
        .map(|index| {
            serde_json::json!({
                "id": format!("delivery-{index}"),
                "channel": "telegram",
                "status": if index == 0 { "uncertain" } else { "pending" },
                "attempts": index,
                "error": if index == 0 { "network unavailable" } else { "" }
            })
        })
        .collect::<Vec<_>>();

    let value = serde_json::json!({"items": items});
    let first_page = super::format_delivery_list(&value, 1).unwrap();

    assert!(
        first_page.starts_with("Deliveries (1–5 of 6, page 1 of 2)\n├─ delivery-0 | uncertain")
    );

    assert!(first_page.contains("│  ├─ Sent through telegram"));
    assert!(first_page.contains("│  └─ Last attempt failed. Network unavailable."));
    assert!(first_page.contains("\n\n├─ delivery-1 | pending"));
    assert!(!first_page.contains("delivery-5"));
    assert!(super::format_delivery_list(&value, 3).unwrap_err().contains("deliveries"));

    assert_eq!(
        super::format_delivery_list(&serde_json::json!({"items": []}), 1).unwrap(),
        "No pending deliveries.\nEvery reply has been delivered."
    );
}

#[test]
fn cli_errors_keep_all_message_lines_flush_left() {
    assert_eq!(
        super::format_cli_error(
            "Could not update plugins.\n\nThe tools plugin could not be updated.\nBinary missing."
        ),
        "Could not update plugins.\n\nThe tools plugin could not be updated.\nBinary missing."
    );

    assert_eq!(super::format_cli_error(""), "The command could not be completed.");
}

#[test]
fn list_commands_accept_page_numbers() {
    let cli = Cli::try_parse_from(["krabb", "plugin", "list", "2", "--json"]).unwrap();

    assert!(matches!(
        cli.command,
        Command::Plugin {
            command: super::PluginCommand::List(super::ListOutput { page: 2, json: true })
        }
    ));

    let cli = SessionCli::try_parse_from(["session", "list", "3"]).unwrap();

    assert!(matches!(
        cli.command,
        SessionCommand::List(super::ListOutput { page: 3, json: false })
    ));

    let cli = DeliveryCli::try_parse_from(["delivery", "list", "4"]).unwrap();

    assert!(matches!(
        cli.command,
        DeliveryCommand::List(super::ListOutput { page: 4, json: false })
    ));
}

#[test]
fn plugin_ids_are_single_safe_path_components() {
    assert!(super::valid("telegram"));
    assert!(super::valid("plugin-2"));
    assert!(!super::valid("../escape"));
    assert!(!super::valid("Telegram"));
}

#[test]
fn encodes_unsafe_channel_routing_in_session_ids() {
    let session = super::session_id("slack", "CABC123", Some("+15551234567"));

    assert!(super::valid(&session));
    assert!(session.starts_with("slack-x"));
    assert_ne!(session, "slack-CABC123-thread-+15551234567");
}

#[test]
fn sentence_keeps_terminal_punctuation() {
    assert_eq!(super::sentence("ready"), "Ready.");
    assert_eq!(super::sentence("Ready"), "Ready.");
    assert_eq!(super::sentence("Ready!"), "Ready!");
    assert_eq!(super::sentence("Ready?"), "Ready?");
    assert_eq!(super::sentence("Ready."), "Ready.");
    assert_eq!(super::sentence("  Denied: plugin not found  "), "Plugin not found.");
    assert_eq!(super::sentence("  "), "Unknown error.");
}

#[test]
fn preserves_ordered_tool_calls_in_model_context() {
    let reply = ModelReply {
        timing: None,
        text: String::new(),
        stop: "tool".into(),
        input: None,
        output: None,
        context_usage: None,
        events: vec![
            Event::Tool {
                name: "read".into(),
                args: serde_json::json!({"path": "note.txt"}),
                id: Some("call-1".into()),
                thought_signature: Some("sig-1".into()),
            },
            Event::Tool {
                name: "patch".into(),
                args: serde_json::json!({"path": "note.txt"}),
                id: Some("call-2".into()),
                thought_signature: None,
            },
        ],
    };

    assert_eq!(
        model_content(&reply).unwrap(),
        vec![
            Content::ToolCall {
                name: "read".into(),
                args: serde_json::json!({"path": "note.txt"}),
                id: Some("call-1".into()),
                thought_signature: Some("sig-1".into()),
            },
            Content::ToolCall {
                name: "patch".into(),
                args: serde_json::json!({"path": "note.txt"}),
                id: Some("call-2".into()),
                thought_signature: None,
            },
        ]
    );
}

#[test]
fn rejects_oversized_tool_arguments_before_model_context() {
    let args = serde_json::json!({"patch": "x".repeat(super::TOOL_ARGS_LIMIT)});

    let reply = ModelReply {
        timing: None,
        text: String::new(),
        stop: "tool".into(),
        input: None,
        output: None,
        context_usage: None,
        events: vec![Event::Tool {
            name: "patch".into(),
            args: args.clone(),
            id: None,
            thought_signature: None,
        }],
    };

    assert_eq!(model_content(&reply), Err("Tool arguments exceed the size limit."));
}

#[test]
fn rejects_oversized_tool_metadata_before_model_context() {
    for (name, id, thought_signature) in [
        ("x".repeat(super::META_LIMIT + 1), None, None),
        ("patch".into(), Some("x".repeat(super::META_LIMIT + 1)), None),
        ("patch".into(), None, Some("x".repeat(super::META_LIMIT + 1))),
    ] {
        let reply = ModelReply {
            timing: None,
            text: String::new(),
            stop: "tool".into(),
            input: None,
            output: None,
            context_usage: None,
            events: vec![Event::Tool { name, args: serde_json::json!({}), id, thought_signature }],
        };

        assert_eq!(model_content(&reply), Err("Tool metadata exceeds the size limit."));
    }
}

#[test]
fn rejects_oversized_tool_content_before_model_context() {
    let args = serde_json::json!({
        "patch": "x".repeat(super::TOOL_ARGS_LIMIT.saturating_sub(32))
    });

    let reply = ModelReply {
        timing: None,
        text: String::new(),
        stop: "tool".into(),
        input: None,
        output: None,
        context_usage: None,
        events: (0..super::CONTENT_LIMIT)
            .map(|index| Event::Tool {
                name: "patch".into(),
                args: args.clone(),
                id: Some(format!("call-{index}")),
                thought_signature: None,
            })
            .collect(),
    };

    assert_eq!(model_content(&reply), Err("Tool content exceeds the size limit."));
}

#[test]
fn drops_accumulated_oversized_tool_arguments_from_bounded_content() {
    let args = serde_json::json!({"patch": "x".repeat(super::TOOL_ARGS_LIMIT)});

    let content = (0..super::CONTENT_LIMIT)
        .map(|_| Content::ToolCall {
            name: "patch".into(),
            args: args.clone(),
            id: None,
            thought_signature: None,
        })
        .collect();

    assert!(super::bound(content).is_empty());
}

#[test]
fn a_reply_reference_never_displaces_the_users_own_content() {
    let reply = krabb_core::reply::Reply::new("7".into(), None, Some("Earlier.".into())).unwrap();

    let text = |text: &str| Content::Text { text: text.into() };

    let quoted = super::quote(vec![text("Question.")], &reply);

    assert_eq!(quoted.len(), 2);
    assert_eq!(quoted[0].render(), reply.context());
    assert_eq!(quoted[1].render(), "Question.");

    let full = (0..super::CONTENT_LIMIT).map(|index| text(&index.to_string())).collect();
    let quoted = super::quote(full, &reply);

    assert_eq!(quoted.len(), super::CONTENT_LIMIT);
    assert_eq!(quoted[0].render(), format!("{}\n\n0", reply.context()));
    assert_eq!(quoted.last().unwrap().render(), (super::CONTENT_LIMIT - 1).to_string());

    let earlier = "Earlier context. ".repeat(64);
    let reply = krabb_core::reply::Reply::new("7".into(), None, Some(earlier)).unwrap();
    let long = "x".repeat(super::TEXT_LIMIT - reply.context().len() + 64);
    let quoted = super::quote(vec![text(&long)], &reply);
    let reference = quoted[0].render();

    assert_eq!(quoted[1].render(), long);
    assert!(reference.len() <= super::TEXT_LIMIT - long.len());
    assert!(reference.contains("[quoted message truncated]"), "{reference}");

    let long = "x".repeat(super::TEXT_LIMIT - 4);

    assert_eq!(super::quote(vec![text(&long)], &reply), vec![text(&long)]);
}

#[test]
fn a_thread_root_is_quoted_only_by_the_first_message_of_its_conversation() {
    let root = crate::tests::test_root("thread-root-reference");
    let mut store = crate::state::Store::load(root.join("data/sessions/index.json")).unwrap();

    store.ensure_channel("thread", "test").unwrap();

    let message = |id: &str| Message {
        id: id.into(),
        session: "thread".into(),
        role: Role::User,
        sender: Some("7".into()),
        content: vec![Content::Text { text: format!("Message {id}.") }],
    };

    let event = |id: &str, reply: &str| {
        let text = format!("Parent {reply}.");

        serde_json::json!({"id":id,"reply":{"id":reply,"text":text}})
    };

    let mut first = message("a");

    super::reference(&mut store, "thread", Some("T"), &event("a", "T"), &mut first).unwrap();

    assert_eq!(first.content.len(), 2);

    store.queue("thread", first.clone()).unwrap();

    let mut retry = message("a");

    super::reference(&mut store, "thread", Some("T"), &event("a", "T"), &mut retry).unwrap();

    assert_eq!(retry.content.len(), 2);

    let mut later = message("b");

    super::reference(&mut store, "thread", Some("T"), &event("b", "T"), &mut later).unwrap();

    assert_eq!(later.content.len(), 1);

    let mut quoting = message("c");

    super::reference(
        &mut store,
        "thread",
        Some("T"),
        &serde_json::json!({"id":"c","reply":{"id":"a"}}),
        &mut quoting,
    )
    .unwrap();

    assert_eq!(quoting.content.len(), 2);
    assert!(quoting.content[0].render().contains("Message a."));

    let references = &store.sessions["thread"].references;

    assert_eq!(
        references.iter().map(|reply| reply.id.as_str()).collect::<Vec<_>>(),
        ["a", "b", "c"]
    );

    let mut unidentified = message("d");

    super::reference(
        &mut store,
        "thread",
        None,
        &serde_json::json!({"id":99,"message":null}),
        &mut unidentified,
    )
    .unwrap();

    assert!(!store.sessions["thread"].references.iter().any(|reply| reply.id == "99"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preserves_stream_text_events_in_model_context() {
    let reply = ModelReply {
        timing: None,
        text: String::new(),
        stop: "stop".into(),
        input: None,
        output: None,
        context_usage: None,
        events: vec![
            Event::Text { text: "Hello ".into() },
            Event::Text { text: "world".into() },
            Event::Done { text: "!".into() },
        ],
    };

    assert_eq!(assistant(&reply), "Hello world!");
}

#[test]
fn normalizes_stream_notifications() {
    let note = Request::Note {
        jsonrpc: "2.0".into(),
        method: "event".into(),
        params: serde_json::json!({"event": {"kind": "text", "text": "part"}}),
    };

    assert_eq!(super::stream_event(note), Some(Event::Text { text: "part".into() }));

    assert!(
        super::stream_event(Request::Note {
            jsonrpc: "2.0".into(),
            method: "other".into(),
            params: serde_json::json!({}),
        })
        .is_none()
    );
}

#[test]
fn merges_streamed_text_without_duplication() {
    let note = |text: &str| Request::Note {
        jsonrpc: "2.0".into(),
        method: "event".into(),
        params: serde_json::json!({"event": {"kind": "text", "text": text}}),
    };

    let mut reply = ModelReply {
        timing: None,
        text: "Hello world".into(),
        stop: "stop".into(),
        input: None,
        output: None,
        context_usage: None,
        events: Vec::new(),
    };

    super::merge_stream(&mut reply, vec![note("Hello "), note("world")]);

    assert_eq!(reply.text, "Hello world");
    assert!(reply.events.is_empty());

    reply.text.clear();
    super::merge_stream(&mut reply, vec![note("Hello "), note("world")]);

    assert_eq!(reply.text, "Hello world");
}

#[test]
fn bounds_model_history_by_frame_size() {
    let mut messages = vec![
        Message {
            id: "assistant-tool".into(),
            session: "test".into(),
            role: Role::Assistant,
            sender: None,
            content: vec![Content::Text { text: "x".repeat(256 * 1024) }],
        },
        Message {
            id: "tool".into(),
            session: "test".into(),
            role: Role::Tool,
            sender: Some("read".into()),
            content: vec![Content::Text { text: "x".repeat(256 * 1024) }],
        },
    ];

    messages.extend((0..40).map(|index| Message {
        id: index.to_string(),
        session: "test".into(),
        role: Role::User,
        sender: None,
        content: vec![Content::Text { text: "x".repeat(256 * 1024) }],
    }));

    let request =
        super::model_request(1, "model", &messages, &[], None, Protocol::CURRENT).unwrap();

    let encoded = serde_json::to_vec(&request).unwrap();

    assert!(encoded.len() < krabb_core::jsonl::MAX);

    let retained =
        serde_json::to_value(request).unwrap()["params"]["messages"].as_array().unwrap().len();

    assert!(retained < messages.len());

    let request =
        super::model_request(1, "model", &messages, &[], None, Protocol::CURRENT).unwrap();

    let value = serde_json::to_value(request).unwrap();
    let retained = value["params"]["messages"].as_array().unwrap();

    assert!(retained.iter().all(|message| message["role"] != "tool"));

    let current = Message {
        id: "current".into(),
        session: "test".into(),
        role: Role::User,
        sender: None,
        content: vec![Content::Text { text: "current".into() }],
    };

    let mut history = messages;

    history.push(current.clone());

    let request = super::model_request(1, "model", &history, &[], None, Protocol::CURRENT).unwrap();

    let value = serde_json::to_value(request).unwrap();
    let retained = value["params"]["messages"].as_array().unwrap();

    assert!(retained.iter().any(|message| message["id"] == "current"));

    let current = Message {
        id: "current".into(),
        session: "test".into(),
        role: Role::User,
        sender: None,
        content: vec![Content::Text { text: "x".repeat(krabb_core::jsonl::MAX) }],
    };

    assert!(super::model_request(1, "model", &[current], &[], None, Protocol::CURRENT).is_err());
}

#[test]
fn supports_structured_tool_calls_in_protocol_01() {
    let messages = [Message {
        id: "assistant-tool".into(),
        session: "test".into(),
        role: Role::Assistant,
        sender: None,
        content: vec![Content::ToolCall {
            name: "read".into(),
            args: serde_json::json!({"path": "note.txt"}),
            id: Some("call-1".into()),
            thought_signature: Some("sig-1".into()),
        }],
    }];

    let current =
        super::model_request(1, "model", &messages, &[], None, Protocol::CURRENT).unwrap();

    assert_eq!(
        serde_json::to_value(current).unwrap()["params"]["messages"][0]["content"][0]["kind"],
        "tool_call"
    );

    let legacy =
        super::model_request(1, "model", &messages, &[], None, Protocol { major: 0, minor: 0 })
            .unwrap();

    let legacy_content =
        serde_json::to_value(legacy).unwrap()["params"]["messages"][0]["content"].clone();

    assert_eq!(
        legacy_content,
        serde_json::json!([{"kind": "text", "text": "[Tool call read]: {\"path\":\"note.txt\"}"}])
    );
}

#[test]
fn marks_mutating_tools() {
    assert!(super::mutating("file_write", &serde_json::json!({})));
    assert!(super::mutating("file_patch", &serde_json::json!({})));
    assert!(super::mutating("shell_run", &serde_json::json!({})));
    assert!(super::mutating("memory_remember", &serde_json::json!({})));
    assert!(super::mutating("data_query", &serde_json::json!({"operation":"insert"})));
    assert!(!super::mutating("data_query", &serde_json::json!({"operation":"query"})));
    assert!(super::mutating("schedule_propose", &serde_json::json!({})));
    assert!(super::mutating("schedule_run", &serde_json::json!({"id": 1})));
    assert!(super::mutating("schedule_retry", &serde_json::json!({"id": 1})));

    assert!(super::mutating(
        "browser_navigate",
        &serde_json::json!({"url":"https://example.test"})
    ));

    assert!(super::mutating("browser_click", &serde_json::json!({"approve":true})));
    assert!(super::mutating("computer_type", &serde_json::json!({"text":"submit"})));
    assert!(super::mutating("git_run", &serde_json::json!({"args": ["worktree", "add", "path"]})));

    assert!(super::mutating(
        "git_run",
        &serde_json::json!({"args": ["worktree", "remove", "path"]})
    ));

    assert!(!super::mutating("file_read", &serde_json::json!({})));
    assert!(!super::mutating("browser_screenshot", &serde_json::json!({})));
    assert!(!super::mutating("computer_screenshot", &serde_json::json!({})));
    assert!(super::mutating("git_run", &serde_json::json!({"args": ["status"]})));
    assert!(super::requires_vision_tool("browser_screenshot"));
    assert!(super::requires_vision_tool("computer_screenshot"));
    assert!(!super::requires_vision_tool("browser_snapshot"));

    for (old, new) in [("write", "file_write"), ("read", "file_read"), ("shell", "shell_run")] {
        assert_eq!(
            super::mutating(old, &serde_json::json!({})),
            super::mutating(new, &serde_json::json!({}))
        );
    }

    assert!(!super::mutating("task_list", &serde_json::json!({})));
    assert!(super::mutating("task_pause", &serde_json::json!({"id":"task-1"})));
    assert!(super::mutating("tasks", &serde_json::json!({"action":"pause"})));
    assert!(!super::mutating("job", &serde_json::json!({"action":"list"})));
}

#[cfg(unix)]
#[tokio::test]
async fn schedule_controls_bind_trusted_callers_after_auto_and_prompt_approval() {
    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
        *) printf '%s\n' "$line" >> "$1"; printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"error":{"code":-32601,"message":"method not found"}}' ;;
    esac
done
"#;

    let root = test_root("schedule-control-identity");

    fs::create_dir_all(&root).unwrap();

    let calls = root.join("calls.jsonl");

    let scheduler = Process::start_with("sh", ["-c", script, "scheduler", calls.to_str().unwrap()])
        .await
        .unwrap();

    let plugins = registry([scheduler]).await;

    for channel in ["telegram", "cli", "tui"] {
        for mode in
            [super::ApprovalMode::Off, super::ApprovalMode::Auto, super::ApprovalMode::Prompt]
        {
            for action in ["pause", "cancel", "resume", "run", "retry", "update"] {
                fs::write(&calls, "").unwrap();
                let sessions = Arc::new(Mutex::new(
                    super::state::Store::load(
                        root.join(format!("sessions-{channel}-{mode:?}-{action}.json")),
                    )
                    .unwrap(),
                ));

                sessions.lock().unwrap().ensure("owner-session", "test").unwrap();
                sessions.lock().unwrap().reserve("owner-session", "turn".into()).unwrap();
                let approvals =
                    Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap()));

                let (notices, mut events) = tokio::sync::mpsc::channel(1);
                let name = format!("schedule_{action}");
                let mut expected = serde_json::json!({
                    "id": 1, "page": null,
                    "caller": {
                        "channel": channel, "chat": "owner-chat", "thread": "topic",
                        "session": "owner-session",
                        "source": super::sharing::principal(channel),
                        "access": super::sharing::access(
                            &plugins.config,
                            &super::sharing::principal(channel),
                            "owner-session",
                            super::sharing::Data::Schedules,
                        )
                        .unwrap()
                    }
                });

                if action == "update" {
                    expected["destinations"] = serde_json::json!([
                        {"channel":"telegram","chat":"123","private":true,"thread":null},
                        {"channel":"discord","chat":"456","private":false,"thread":null}
                    ]);
                }

                let mut failed_tool = None;
                let operation = super::execute_tool(
                    &plugins,
                    1,
                    &name,
                    serde_json::json!({
                        "id": 1, "caller": {"channel": "cli"},
                        "destinations": expected["destinations"], "approved": true,
                        "context": {"origin": {"chat": "foreign"}}, "action": "approve"
                    }),
                    "codex",
                    "test",
                    mode,
                    approvals.clone(),
                    channel,
                    "owner-chat",
                    Some("topic"),
                    "owner-session",
                    &sessions,
                    None,
                    false,
                    true,
                    false,
                    notices,
                    tokio::time::Instant::now() + std::time::Duration::from_secs(5),
                    &mut failed_tool,
                    None,
                    None,
                    0,
                );

                let review = async {
                    if mode != super::ApprovalMode::Prompt {
                        return;
                    }

                    assert!(matches!(
                        events.recv().await.unwrap(),
                        super::StreamNotice::Approval { .. }
                    ));

                    assert!(fs::read_to_string(&calls).unwrap().is_empty());
                    let mut gate = approvals.lock().await;
                    let pending = gate.list().unwrap();

                    assert_eq!(pending[0].target.args, expected);
                    assert_eq!(gate.resolve_local(&pending[0].id, true), Some(true));
                };

                let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    tokio::join!(operation, review)
                })
                .await
                .unwrap();

                let recorded = fs::read_to_string(&calls).unwrap();

                if mode == super::ApprovalMode::Off {
                    assert!(result.unwrap_err().to_string().contains("blocks mutating tools"));
                    assert!(recorded.is_empty());
                    continue;
                }

                assert!(result.unwrap_err().to_string().contains("method not found"));
                let requests: Vec<serde_json::Value> =
                    recorded.lines().map(|line| serde_json::from_str(line).unwrap()).collect();

                assert_eq!(requests.len(), 1, "Never fall back to unscoped control");
                let method = if action == "cancel" { "remove" } else { action };

                expected["action"] = serde_json::json!(method);
                expected.as_object_mut().unwrap().remove("page");

                if action == "update" {
                    expected["approved"] = serde_json::json!(true);
                }

                assert_eq!(requests[0]["method"], "control");

                assert_eq!(requests[0]["params"], expected);
            }
        }
    }

    stop_registry(&plugins).await;

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn schedule_proposals_obey_approval_and_bind_execution_context() {
    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocol":{"major":0,"minor":1},"id":"scheduler","version":"0.1.0","capabilities":["scheduler"]}}' ;;
        *'"method":"add"'*'"activate":true'*'"repeat":300'*) printf '%s\n' "$line" >> "$1"; printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"id":1,"due":1790000000,"zone":"UTC","repeat":300,"retries":0,"status":"active"}}' ;;
        *'"method":"add"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"error":{"code":-32000,"message":"Activation was missing."}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    for (mode, decision, capabilities) in [
        (super::ApprovalMode::Off, false, true),
        (super::ApprovalMode::Prompt, false, true),
        (super::ApprovalMode::Prompt, true, true),
        (super::ApprovalMode::Prompt, true, false),
        (super::ApprovalMode::Auto, true, true),
    ] {
        let root = test_root("schedule-explicit-authorization");
        fs::create_dir_all(&root).unwrap();
        let calls = root.join("adds.jsonl");
        let scheduler =
            Process::start_with("sh", ["-c", script, "scheduler", calls.to_str().unwrap()])
                .await
                .unwrap();
        let plugins = registry([scheduler]).await;
        let sessions =
            Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));
        {
            let mut store = sessions.lock().unwrap();
            store.create("tui-default", "test").unwrap();
            store
                .begin(
                    "tui-default",
                    super::Message {
                        id: "user-message".into(),
                        session: "tui-default".into(),
                        role: super::Role::User,
                        sender: None,
                        content: vec![super::Content::Text {
                            text: "read the project notes".into(),
                        }],
                    },
                )
                .unwrap();
        }

        let approvals = Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap()));
        let (notices, mut events) = tokio::sync::mpsc::channel(1);
        let mut failed_tool = None;
        let arguments = serde_json::json!({
            "instruction": "write a daily report",
            "delay_seconds": 10,
            "repeat_seconds": 300,
            "context": {"shell": true, "tools": true},
            "at": 1790000000,
            "zone": "Europe/Rome",
            "repeat": 5,
            "message_destination": {
                "channel": "telegram", "chat": "12345", "private": true
            }
        });

        let mut expected = arguments.clone();

        for key in ["message_destination", "at", "zone", "repeat"] {
            expected.as_object_mut().unwrap().remove(key);
        }

        expected["context"] = serde_json::json!({
            "provider": "codex", "model": "test", "workspace": root.display().to_string(),
            "origin": {
                "channel": "tui", "chat": "tui-default", "thread": null,
                "session": "tui-default", "private": true, "sender": null, "roles": []
            },

            "message_destination": arguments["message_destination"],
            "tools": capabilities, "shell": capabilities
        });

        expected["context"]["origin"]["source"] = serde_json::json!("clients.tui");
        expected["context"]["message_destination"]["thread"] = serde_json::Value::Null;
        let operation = super::execute_tool(
            &plugins,
            1,
            "schedule_propose",
            arguments,
            "codex",
            "test",
            mode,
            approvals.clone(),
            "tui",
            "tui-default",
            None,
            "tui-default",
            &sessions,
            Some(&root),
            capabilities,
            true,
            capabilities,
            notices,
            tokio::time::Instant::now() + std::time::Duration::from_secs(5),
            &mut failed_tool,
            None,
            None,
            0,
        );
        let review = async {
            if mode != super::ApprovalMode::Prompt {
                return;
            }

            let notice = events.recv().await.unwrap();
            let super::StreamNotice::Approval { text, .. } = notice else {
                panic!("Expected schedule approval");
            };

            assert!(text.contains(&format!(
                "unattended workspace tools: {capabilities}, shell: {capabilities}"
            )));

            assert!(text.contains("chat 12345"));
            assert!(!calls.exists(), "No executable task may exist before approval");
            let mut gate = approvals.lock().await;
            let pending = gate.list().unwrap();

            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].target.tool, "schedule_propose");
            assert_eq!(pending[0].target.args, expected);
            assert_eq!(gate.resolve_local(&pending[0].id, decision), Some(decision));
        };

        let (output, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(operation, review)
        })
        .await
        .unwrap();

        if decision {
            let output = output.unwrap();

            assert!(output.contains(" is active."));
            assert!(output.contains("Frequency: every 5 minutes"));
            let added: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&calls).unwrap()).unwrap();

            assert_eq!(added["params"]["context"], expected["context"]);
            assert_eq!(added["params"]["instruction"], expected["instruction"]);
            assert_eq!(added["params"]["repeat"], 300);
            assert!(added["params"].get("at").is_none());
            assert!(added["params"].get("zone").is_none());
            assert_eq!(added["params"]["activate"], true);
            assert_eq!(sessions.lock().unwrap().sessions["tui-default"].phase, "unsafe");
        } else {
            if mode == super::ApprovalMode::Off {
                assert!(output.unwrap_err().to_string().contains("blocks mutating tools"));
            } else {
                assert!(output.unwrap().contains("denied"));
            }

            assert!(!calls.exists(), "Rejected proposals must never reach the scheduler");
            assert_ne!(sessions.lock().unwrap().sessions["tui-default"].phase, "unsafe");
        }

        assert!(events.try_recv().is_err());
        stop_registry(&plugins).await;
        let _ = fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn execute_tool_rejects_shell_when_turn_permission_is_disabled() {
    let sessions = Arc::new(Mutex::new(super::state::Store::default()));

    let mut failed_tool = None;

    let result = super::execute_tool(
        &Plugins::default(),
        1,
        "shell",
        serde_json::json!({"command": "true"}),
        "codex",
        "test",
        super::ApprovalMode::Auto,
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        "scheduler",
        "42",
        None,
        "schedule-42",
        &sessions,
        None,
        true,
        false,
        false,
        tokio::sync::mpsc::channel(1).0,
        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        &mut failed_tool,
        None,
        Some(9),
        0,
    )
    .await;

    assert_eq!(result.unwrap_err().to_string(), "Shell access is not authorized for this turn.");
}

#[tokio::test]
async fn authorized_schedule_proposals_fail_when_the_scheduler_is_missing() {
    let sessions = Arc::new(Mutex::new(super::state::Store::default()));

    sessions.lock().unwrap().ensure("tui-default", "test").unwrap();
    sessions.lock().unwrap().reserve("tui-default", "turn".into()).unwrap();

    let mut failed_tool = None;
    let notices = tokio::sync::mpsc::channel(1).0;

    let result = super::execute_tool(
        &Plugins::default(),
        1,
        "schedule_propose",
        serde_json::json!({"instruction":"say hello", "delay_seconds":10}),
        "codex",
        "test",
        super::ApprovalMode::Auto,
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        "tui",
        "tui-default",
        None,
        "tui-default",
        &sessions,
        None,
        false,
        true,
        false,
        notices,
        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        &mut failed_tool,
        None,
        None,
        0,
    )
    .await;

    let error = result.unwrap_err().to_string();

    assert!(error.contains("scheduler plugin is not installed"), "{error}");
}

#[test]
fn runs_only_one_mutating_tool_from_each_model_response() {
    let mut events = vec![
        super::Event::Tool {
            name: "read".into(),
            args: serde_json::json!({"path": "note.txt"}),
            id: None,
            thought_signature: None,
        },
        super::Event::Tool {
            name: "write".into(),
            args: serde_json::json!({"path": "note.txt", "text": "updated"}),
            id: None,
            thought_signature: None,
        },
        super::Event::Tool {
            name: "shell".into(),
            args: serde_json::json!({"command": "touch note.txt"}),
            id: None,
            thought_signature: None,
        },
    ];

    super::keep_one_mutation(&mut events);

    assert_eq!(events.len(), 2);
    assert!(matches!(events.last(), Some(super::Event::Tool { name, .. }) if name == "write"));
}

#[test]
fn output_tokens_count_payloads_rather_than_their_debug_form() {
    let text = "a".repeat(300);

    let tool = super::Event::Tool {
        name: "write".into(),
        args: serde_json::json!({"text": text}),
        id: Some("call-1".into()),
        thought_signature: Some("x".repeat(4_000)),
    };

    assert_eq!(super::output_tokens(&super::Event::Text { text: text.clone() }), 100);
    assert_eq!(super::output_tokens(&tool), 2 + 2 + 100 + 4 + krabb_core::context::OVERHEAD);
    assert_eq!(super::output_tokens(&super::Event::Error { message: text }), 0);
}

#[test]
fn prepares_approval_text_and_channel_requests() {
    assert!(
        super::approval_text(
            "task_steer",
            &serde_json::json!({"id":"task-1", "instruction":"Use v2."})
        )
        .contains("steer task-1 with instruction: Use v2.")
    );

    assert!(
        super::approval_text(
            "job",
            &serde_json::json!({"action":"extend", "id":"task-1",
                "minutes":30})
        )
        .contains("by 30 minutes")
    );

    assert!(
        super::approval_text("file_write", &serde_json::json!({"path": "note.txt"}))
            .contains("write to note.txt")
    );

    assert!(
        super::approval_text("memory_remember", &serde_json::json!({"key": "name"}))
            .contains("save memory name")
    );

    assert!(
        super::approval_text("file_patch", &serde_json::json!({"text": "diff"}))
            .contains("4 bytes")
    );

    assert!(
        super::approval_text("shell_run", &serde_json::json!({"command": "cargo test"}))
            .contains("cargo test")
    );

    assert!(
        super::approval_text("git_run", &serde_json::json!({"args": ["status"]}))
            .contains("status")
    );

    assert!(super::approval_text("file_read", &serde_json::json!({})).contains("file_read tool"));
    assert!(super::approval_text("read", &serde_json::json!({})).contains("file_read tool"));

    assert!(
        super::approval_text("task_pause", &serde_json::json!({"id":"task-1"}))
            .starts_with("Krabb requests approval to pause task-1.")
    );

    assert_eq!(
        super::approval_text("tasks", &serde_json::json!({"action":"pause", "id":"task-1"})),
        super::approval_text("task_pause", &serde_json::json!({"id":"task-1"}))
    );

    let telegram =
        super::approval_params("telegram", "7", Some("9"), "Approve?", "allow", "deny").unwrap();

    assert_eq!(telegram["chat"], 7);
    assert_eq!(telegram["thread"], "9");
    assert!(super::approval_params("telegram", "bad", None, "Approve?", "a", "d").is_err());

    let discord =
        super::approval_params("discord", "channel", Some("thread"), "Approve?", "allow", "deny")
            .unwrap();

    assert_eq!(discord["channel"], "channel");
    assert!(discord.get("thread").is_none());
    assert!(super::approval_params("other", "room", None, "Approve?", "a", "d").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_restart_backoff_keeps_retrying_with_a_cap() {
    assert_eq!(super::retry_delay(1), std::time::Duration::from_secs(2));
    assert_eq!(super::retry_delay(4), std::time::Duration::from_secs(16));
    assert_eq!(super::retry_delay(100), std::time::Duration::from_secs(30));
}

#[cfg(unix)]
#[tokio::test]
async fn keeps_retrying_after_a_plugin_restart_fails() {
    let root = test_root("restart-retry");

    fs::create_dir_all(&root).unwrap();

    let marker = root.join("fail-once");

    let script = format!(
        "if [ -e '{}' ]; then rm -f '{}'; exit 1; fi\nwhile IFS= read -r line; do case \"$line\" in *hello*) printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocol\":{{\"major\":0,\"minor\":1}},\"id\":\"test\",\"version\":\"0.1.0\",\"capabilities\":[]}}}}' ;; *shutdown*) printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{{\"ok\":true}}}}'; exit 0 ;; esac; done",
        marker.display(),
        marker.display()
    );

    let process = Process::start_with("sh", ["-c", &script]).await.unwrap();
    let process = Live::new(process);

    fs::write(&marker, "fail next startup").unwrap();

    let mut failures = 0;

    assert!(recover(&process, &mut failures, "test", &Stop::new()).await.unwrap());
    assert_eq!(failures, 2);

    process.stop().await.unwrap();

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn keeps_progress_separate_from_the_final_channel_answer() {
    for channel in ["telegram", "discord", "slack", "signal", "whatsapp", "custom"] {
        check_progress_delivery(channel).await;
    }
}

#[cfg(unix)]
async fn check_progress_delivery(channel_id: &str) {
    let root = test_root("stream-channel");

    fs::create_dir_all(&root).unwrap();

    let log = root.join("requests.log");

    let script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}' ;; *send*) echo send >> '__LOG__'; printf '%s\n' '{"jsonrpc":"2.0","id":7,"result":{"message_id":99}}' ;; *edit*) echo edit >> '__LOG__'; printf '%s\n' '{"jsonrpc":"2.0","id":8,"result":{"message_id":99}}' ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#
            .replace("__LOG__", &log.display().to_string())
            .replace("\\\\n", "\\n")
            .replace("\\\"", "\"");

    let process = Process::start_with("sh", ["-c", &script]).await.unwrap();
    let channel = Live::new(process);

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));
    {
        let mut store = sessions.lock().unwrap();
        store.create("main", "model").unwrap();
        store
            .begin(
                "main",
                Message {
                    id: "user".into(),
                    session: "main".into(),
                    role: Role::User,
                    sender: Some("sender".into()),
                    content: vec![Content::Text { text: "question".into() }],
                },
            )
            .unwrap();
    }

    let mut output = super::StreamOutput::new();

    output.apply(super::StreamNotice::Progress("I'll check the requested file.".into()));

    let mut call = 7;

    super::flush_stream(
        &mut output,
        &channel,
        channel_id,
        &sessions,
        "main",
        "telegram-1",
        "7",
        None,
        &mut call,
    )
    .await
    .unwrap();

    output.apply(super::StreamNotice::Text("answer".into()));

    super::flush_stream(
        &mut output,
        &channel,
        channel_id,
        &sessions,
        "main",
        "telegram-1",
        "7",
        None,
        &mut call,
    )
    .await
    .unwrap();

    {
        let store = sessions.lock().unwrap();

        assert!(store.outbox.is_empty());
        assert!(store.sessions["main"].stream_delivery.is_none());
        assert_eq!(fs::read_to_string(&log).unwrap().lines().collect::<Vec<_>>(), ["send"]);
    }

    let text = "long answer 🦀 ".repeat(500);

    {
        let mut store = sessions.lock().unwrap();

        store
            .reply(
                "main",
                Message {
                    id: "final".into(),
                    session: "main".into(),
                    role: Role::Assistant,
                    sender: None,
                    content: vec![Content::Text { text: text.clone() }],
                },
                "telegram-1",
                channel_id,
                "7",
                None,
                &text,
            )
            .unwrap();

        let request = super::delivery_request(channel_id, 8, &store.outbox[0]).unwrap();
        let Request::Call { method, params, .. } = request else { panic!() };

        assert_eq!(method, "send");
        assert_eq!(params["text"], text);
        assert!(params.get("draft").is_none());
        assert!(params.get("message").is_none());
    }

    channel.stop().await.unwrap();

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn progress_lost_in_transport_is_forgotten_like_other_progress() {
    let root = test_root("stream-transport-failure");

    fs::create_dir_all(&root).unwrap();

    let script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}' ;; *send*) exit 0 ;; esac; done"#;
    let channel = Live::new(Process::start_with("sh", ["-c", script]).await.unwrap());

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    sessions.lock().unwrap().create("main", "model").unwrap();

    let mut output = super::StreamOutput::new();

    output.apply(super::StreamNotice::Progress("I'll check the requested file.".into()));

    let mut call = 7;

    let flushed = super::flush_stream(
        &mut output,
        &channel,
        "telegram",
        &sessions,
        "main",
        "telegram-1",
        "7",
        None,
        &mut call,
    )
    .await;

    assert!(flushed.is_err());

    let store = sessions.lock().unwrap();

    assert!(store.outbox.is_empty());

    drop(store);

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn failed_progress_does_not_block_the_final_answer() {
    let root = test_root("stream-failure");

    fs::create_dir_all(&root).unwrap();

    let script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}' ;; *send*) printf '%s\n' '{"jsonrpc":"2.0","id":7,"error":{"code":-1,"message":"Delivery was rejected."}}' ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#
            .replace("\\\\n", "\\n")
            .replace("\\\"", "\"");

    let process = Process::start_with("sh", ["-c", &script]).await.unwrap();
    let channel = Live::new(process);

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));
    {
        let mut store = sessions.lock().unwrap();
        store.create("main", "model").unwrap();
        store
            .begin(
                "main",
                Message {
                    id: "user".into(),
                    session: "main".into(),
                    role: Role::User,
                    sender: Some("sender".into()),
                    content: vec![Content::Text { text: "question".into() }],
                },
            )
            .unwrap();
    }

    let mut output = super::StreamOutput::new();

    output.apply(super::StreamNotice::Progress("I'll check the requested file.".into()));

    let mut call = 7;

    assert!(
        super::flush_stream(
            &mut output,
            &channel,
            "telegram",
            &sessions,
            "main",
            "telegram-1",
            "7",
            None,
            &mut call,
        )
        .await
        .is_err()
    );

    {
        let mut store = sessions.lock().unwrap();
        store
            .reply(
                "main",
                Message {
                    id: "assistant".into(),
                    session: "main".into(),
                    role: Role::Assistant,
                    sender: None,
                    content: vec![Content::Text { text: "complete".into() }],
                },
                "telegram-1",
                "telegram",
                "7",
                None,
                "complete",
            )
            .unwrap();
        assert_eq!(store.outbox.len(), 1);
        assert_eq!(store.outbox[0].text, "complete");
        assert_eq!(store.outbox[0].status, super::state::DeliveryStatus::Pending);
        assert!(store.outbox[0].message_id.is_none());
        assert!(matches!(
            super::delivery_request("telegram", 8, &store.outbox[0]).unwrap(),
            Request::Call { method, .. } if method == "send"
        ));
    }

    channel.stop().await.unwrap();

    let _ = fs::remove_dir_all(root);
}

#[test]
fn prepares_channel_replies() {
    assert_eq!(
        super::send_params("discord", "delivery", "room", "hello", None).unwrap()["channel"],
        "room"
    );

    assert_eq!(super::send_params("telegram", "delivery", "7", "hello", None).unwrap()["chat"], 7);

    assert_eq!(
        super::send_params("telegram", "delivery", "7", "hello", Some("4")).unwrap()["thread"],
        "4"
    );

    assert!(super::send_params("telegram", "delivery", "room", "hello", None).is_err());

    assert_eq!(
        super::edit_params("discord", "delivery", "room", "42", "hello", None).unwrap()["message"],
        "42"
    );

    assert_eq!(
        super::edit_params("telegram", "delivery", "7", "42", "hello", Some("4")).unwrap()["thread"],
        "4"
    );

    let delivery = super::state::Delivery {
        id: "delivery".into(),
        channel: "discord".into(),
        chat: "room".into(),
        private: true,
        thread: None,
        text: "updated".into(),
        attempts: 0,
        created: 0,
        status: super::state::DeliveryStatus::Pending,
        last_error: None,
        message_id: Some("42".into()),
        updated: 0,
    };

    assert!(matches!(
        delivery_request("discord", 12, &delivery).unwrap(),
        Request::Call { method, .. } if method == "edit"
    ));

    let group_delivery = super::state::Delivery {
        id: "group-delivery".into(),
        channel: "signal".into(),
        chat: "group".into(),
        private: false,
        thread: None,
        text: "hello".into(),
        attempts: 0,
        created: 0,
        status: super::state::DeliveryStatus::Pending,
        last_error: None,
        message_id: None,
        updated: 0,
    };

    assert!(matches!(
        delivery_request("signal", 13, &group_delivery).unwrap(),
        Request::Call { params, .. } if params["private"] == false
    ));

    assert!(matches!(
        send_request("signal", 14, "stream-delivery", "group", false, "hello", None)
            .unwrap(),
        Request::Call { params, .. } if params["private"] == false
    ));

    let context_home = std::env::temp_dir().join(format!("krabb-context-{}", std::process::id()));

    let context_root = context_home.join("data/system");
    let _ = fs::remove_dir_all(&context_home);

    fs::create_dir_all(&context_root).unwrap();

    assert!(super::context_at("test", &context_root).is_empty());

    fs::write(context_root.join("AGENTS.md"), "Coding-agent instructions.").unwrap();
    fs::write(context_root.join("KRAB.md"), "A calm, helpful personality.").unwrap();
    fs::write(context_root.join("KLAW.md"), "Ask before changing files.").unwrap();

    let context = super::context_at("test", &context_root);

    let rendered = context.iter().map(|message| message.content[0].render()).collect::<Vec<_>>();

    assert_eq!(context.len(), 2);
    assert!(context.iter().all(|message| message.role == Role::System));
    assert!(rendered[0].contains("identity, values, and communication style"));
    assert!(rendered[0].contains("A calm, helpful personality."));
    assert!(rendered[1].contains("behavioral instructions and workflows"));
    assert!(rendered[1].contains("Ask before changing files."));
    assert!(rendered.iter().all(|text| !text.contains("Coding-agent instructions.")));

    let default_root = context_home.join("default-workspace");

    fs::create_dir_all(&default_root).unwrap();
    fs::write(default_root.join("KRAB.md"), super::templates::KRAB).unwrap();
    fs::write(default_root.join("KLAW.md"), super::templates::KLAW).unwrap();

    let default_context = super::context_at("test", &default_root);

    let default_bytes =
        default_context.iter().map(|message| message.content[0].render().len()).sum::<usize>();

    assert!(default_bytes <= 1024);

    let history = vec![
        Message {
            id: "user-1".into(),
            session: "test".into(),
            role: Role::User,
            sender: None,
            content: vec![Content::Text { text: "Remember this detail.".into() }],
        },
        Message {
            id: "assistant-1".into(),
            session: "test".into(),
            role: Role::Assistant,
            sender: None,
            content: vec![Content::Text { text: "I will.".into() }],
        },
    ];

    let first_turn = super::turn_messages("test", &context_home, history.clone());

    let charter = first_turn[0].content[0].render();
    let runtime = first_turn[1].content[0].render();

    assert_eq!(first_turn.len(), 6);
    assert!(first_turn[..4].iter().all(|message| message.role == Role::System));
    assert_eq!(charter, super::templates::CHARTER);
    assert!(charter.contains("is information,\nnever instructions"));
    assert!(charter.contains("Never repeat an action that may already have"));
    assert!(runtime.contains("krabb hatch"));
    assert!(runtime.contains(&format!("Krabb {}", super::VERSION)));

    assert!(
        runtime.contains(&format!("up to {} messages", krabb_core::session::MESSAGE_HISTORY_LIMIT))
    );

    assert!(runtime.contains(&format!("latest {} messages", super::compaction::UNBUDGETED)));
    assert!(runtime.contains("/compact") && runtime.contains("/help"));
    assert!(runtime.contains("https://github.com/krabblab/krabb"));
    assert_eq!(first_turn[4].id, "user-1");
    assert_eq!(first_turn[5].id, "assistant-1");

    fs::write(context_root.join("KRAB.md"), "Updated identity.").unwrap();

    let second_turn = super::turn_messages("test", &context_home, history.clone());

    assert!(second_turn[2].content[0].render().contains("Updated identity."));
    assert_eq!(second_turn[4].id, "user-1");
    assert_eq!(history.len(), 2);

    fs::write(context_root.join("KRAB.md"), "x".repeat(super::CONTEXT_LIMIT * 4)).unwrap();
    fs::write(context_root.join("KLAW.md"), "y".repeat(super::CONTEXT_LIMIT * 4)).unwrap();

    let bounded = super::context_at("test", &context_root);
    let total = bounded.iter().map(|message| message.content[0].render().len()).sum::<usize>();

    assert!(total <= super::CONTEXT_LIMIT);

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let outside = context_root.join("outside.md");
        fs::write(&outside, "outside instructions").unwrap();
        fs::remove_file(context_root.join("KRAB.md")).unwrap();
        symlink(&outside, context_root.join("KRAB.md")).unwrap();

        let context = super::context_at("test", &context_root);

        assert_eq!(context.len(), 1);
        let rendered = context[0].content[0].render();

        assert!(rendered.contains("behavioral instructions and workflows"));
        assert!(!rendered.contains("outside instructions"));
    }

    let _ = fs::remove_dir_all(context_home);

    assert!(super::allowed(
        &super::ChannelConfig::default(),
        &serde_json::json!({"private": true, "text": "hello"}),
        "7"
    ));

    let mut policy = super::ChannelConfig { tools: true, ..Default::default() };

    assert!(!super::tools_allowed(
        &policy,
        &serde_json::json!({"private": true, "sender": "7", "text": "hello"}),
        "7",
        true
    ));

    assert!(!super::schedule_allowed(
        &policy,
        &serde_json::json!({"private": true, "sender": "7", "text": "hello"}),
        "7"
    ));

    policy.shell = true;

    assert!(!super::channel_shell_allowed(
        &policy,
        &serde_json::json!({"private": true, "sender": "7", "text": "hello"}),
        "7",
        true,
        false
    ));

    policy.allow.push("7".into());

    assert!(super::tools_allowed(
        &policy,
        &serde_json::json!({"private": true, "sender": "7", "text": "hello"}),
        "7",
        true
    ));

    assert!(super::schedule_allowed(
        &policy,
        &serde_json::json!({"private": true, "sender": "7", "text": "hello"}),
        "7"
    ));

    assert!(super::channel_shell_allowed(
        &policy,
        &serde_json::json!({"private": true, "sender": "7", "text": "hello"}),
        "7",
        true,
        false
    ));

    assert!(!super::channel_shell_allowed(
        &policy,
        &serde_json::json!({"private": true, "sender": "7", "text": "hello"}),
        "7",
        false,
        false
    ));

    assert!(!super::tools_allowed(
        &policy,
        &serde_json::json!({"private": true, "sender": "7", "text": "hello"}),
        "7",
        false
    ));

    assert!(!super::allowed(
        &super::ChannelConfig::default(),
        &serde_json::json!({"private": false, "text": "hello"}),
        "7"
    ));

    assert!(!super::allowed(
        &super::ChannelConfig { allow: vec!["8".into()], mention: None, ..Default::default() },
        &serde_json::json!({"text": "hello"}),
        "7"
    ));

    assert!(!super::allowed(
        &super::ChannelConfig {
            allow: Vec::new(),
            mention: Some("@krabb".into()),
            ..Default::default()
        },
        &serde_json::json!({"text": "hello"}),
        "7"
    ));

    assert!(super::env_nonempty("PATH"));
    assert!(!super::env_nonempty("KRABB_TEST_MISSING"));
    assert!(!super::file_credential_at(None, Path::new("unused"), &["KRABB_TEST_MISSING"]));

    let credentials =
        std::env::temp_dir().join(format!("krabb-credentials-{}.json", std::process::id()));

    fs::write(
            &credentials,
            r#"{"KRABB_CODEX_KEY":"secret","KRABB_SIGNAL_ACCOUNT":"signal","KRABB_SLACK_BOT_TOKEN":"slack"}"#,
        )
        .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&credentials).unwrap().permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&credentials, permissions).unwrap();
    }

    assert!(super::credential(&credentials, Path::new("unused"), &["KRABB_CODEX_KEY"]));

    assert!(super::ready_at("signal", Some(&credentials), Path::new("unused")));
    assert!(super::ready_at("slack", Some(&credentials), Path::new("unused")));
    assert!(!super::credential(&credentials, Path::new("unused"), &["MISSING"]));

    fs::write(&credentials, "broken").unwrap();

    assert!(!super::credential(&credentials, Path::new("unused"), &["KRABB_CODEX_KEY"]));

    let _ = fs::remove_file(credentials);

    assert_eq!(super::event_id(&serde_json::json!("abc")), Some("abc".into()));
    assert_eq!(super::event_id(&serde_json::json!(7)), Some("7".into()));
    assert_eq!(super::event_id(&serde_json::Value::Null), None);
    assert_eq!(super::retry_delay(0), std::time::Duration::from_secs(1));
    assert_eq!(super::retry_delay(5), std::time::Duration::from_secs(30));
    assert_eq!(super::retry_delay(u32::MAX), std::time::Duration::from_secs(30));

    let media = serde_json::json!({
        "content": [
            {"kind": "text", "text": "caption"},
            {"kind": "image", "uri": "https://example.test/image", "alt": null},
            {"kind": "file", "uri": "https://example.test/file", "name": "file.txt", "mime": null}
        ]
    });

    assert_eq!(super::content(&media).len(), 3);

    assert_eq!(
        super::content(&serde_json::json!({"content":[{"kind":"unknown"}],"text":"fallback"}))
            .len(),
        1
    );

    let clipped = super::content(&serde_json::json!({"text": "x".repeat(300 * 1024)}));

    assert!(clipped[0].render().len() <= super::TEXT_LIMIT);
    assert!(super::content(&serde_json::json!({"text":""})).is_empty());

    assert!(super::allowed(
        &super::ChannelConfig {
            allow: vec!["7".into()],
            mention: Some("@bot".into()),
            ..Default::default()
        },
        &serde_json::json!({"private": false, "text": "hello @bot"}),
        "7"
    ));

    let event = serde_json::json!({
        "private": false,
        "text": "hello",
        "sender": "owner",
        "topic": 4,
        "thread": "thread"
    });

    assert!(super::allowed(
        &super::ChannelConfig {
            allow: vec!["7".into()],
            owner: Some("owner".into()),
            topic: vec!["4".into()],
            thread: vec!["thread".into()],
            ..Default::default()
        },
        &event,
        "7"
    ));

    assert!(!super::allowed(
        &super::ChannelConfig {
            allow: vec!["7".into()],
            admin: vec!["admin".into()],
            ..Default::default()
        },
        &event,
        "7"
    ));
}

#[tokio::test]
async fn main_reports_success_and_failure() {
    assert_eq!(
        super::main_with(Cli {
            json: false,
            debug: false,
            verbose: false,
            version_flag: None,
            help: None,
            command: Command::Version(Output { json: false }),
        })
        .await,
        std::process::ExitCode::SUCCESS
    );

    assert_eq!(
        super::main_with(Cli {
            json: false,
            debug: false,
            verbose: false,
            version_flag: None,
            help: None,
            command: Command::External(Vec::new())
        })
        .await,
        std::process::ExitCode::FAILURE
    );
}

#[cfg(unix)]
#[test]
fn secure_files_are_private() {
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::temp_dir().join(format!("krabb-secure-{}", std::process::id()));

    super::secure(&path, b"secret").unwrap();

    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn runs_non_interactive_commands() {
    for command in [Command::Version(Output { json: true }), Command::Status(Output { json: true })]
    {
        let result = super::run(Cli {
            json: false,
            debug: false,
            verbose: false,
            version_flag: None,
            help: None,
            command,
        })
        .await;
        result.unwrap();
    }

    let root = test_root("doctor-command");

    super::doctor_at(&root, false, false).unwrap();
}

#[tokio::test]
async fn manages_local_sessions_and_plugins() {
    let root = test_root("host");
    let memory_binary = super::plugin_name("memory");
    let _ = fs::remove_dir_all(&root);

    local_session(SessionCommand::List(super::ListOutput { page: 1, json: false }), &root).unwrap();

    local_session(
        SessionCommand::New(SessionNew { id: "main".into(), model: Some("test".into()) }),
        &root,
    )
    .unwrap();

    local_session(SessionCommand::List(super::ListOutput { page: 1, json: false }), &root).unwrap();

    local_session(SessionCommand::List(super::ListOutput { page: 1, json: true }), &root).unwrap();

    local_session(SessionCommand::Show(Id { id: "main".into() }), &root).unwrap();

    local_session(
        SessionCommand::Fork(Fork { source: "main".into(), target: "copy".into() }),
        &root,
    )
    .unwrap();

    local_session(
        SessionCommand::Model(SessionModel { id: "copy".into(), model: "next".into() }),
        &root,
    )
    .unwrap();

    local_session(SessionCommand::Cancel(Id { id: "copy".into() }), &root).unwrap();

    super::init_at(&root).unwrap();
    super::init_at(&root).unwrap();
    fs::create_dir_all(root.join("plugins/memory/bin")).unwrap();

    write_test_plugin(
        &root.join("plugins/memory/bin").join(&memory_binary),
        "memory",
        "memory",
        "0.1.0",
    );

    assert!(super::binary_at("memory", &root).is_some());
    assert!(super::binary_at("missing", &root).is_none());
    assert!(super::binary_at("../escape", &root).is_none());
    assert_eq!(super::installed_at(&root), vec!["memory"]);
    assert!(super::installed_at(&root.join("missing")).is_empty());

    let plugin_root = root.join("source");

    fs::create_dir_all(plugin_root.join("bin")).unwrap();

    fs::write(
            plugin_root.join("krabb-plugin.toml"),
            "id = 'memory'\nversion = '0.1.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['memory']\n",
        )
        .unwrap();

    write_test_plugin(&plugin_root.join("bin").join(&memory_binary), "memory", "memory", "0.1.0");

    let source = Source {
        id: "memory".into(),
        source: Some(plugin_root.display().to_string()),
        revision: None,
        version: None,
        yes: true,
    };

    super::link_at(source, true, &root).unwrap();

    assert_eq!(super::installed_at(&root), vec!["memory"]);

    assert!(
        super::link_at(
            Source {
                id: "memory".into(),
                source: Some(root.join("plugins/memory").display().to_string()),
                revision: None,
                version: None,
                yes: true,
            },
            true,
            &root,
        )
        .is_err()
    );

    #[cfg(unix)]

    assert!(
        fs::symlink_metadata(root.join("plugins/memory/bin").join(&memory_binary))
            .unwrap()
            .file_type()
            .is_symlink()
    );

    super::list_at(&root, None, 1, true).unwrap();
    super::list_at(&root, None, 1, false).unwrap();

    let lock = super::load_lock_at(&root).unwrap();

    assert!(lock.plugins.contains_key("memory"));
    assert_eq!(lock.plugins["memory"].hash.len(), 64);
    assert!(!lock.plugins["memory"].revision.is_empty());

    fs::write(
            plugin_root.join("krabb-plugin.toml"),
            "id = 'memory'\nversion = '0.2.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['memory']\n",
        )
        .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions =
            fs::metadata(plugin_root.join("bin").join(&memory_binary)).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(plugin_root.join("bin").join(&memory_binary), permissions).unwrap();
    }

    write_test_plugin(&plugin_root.join("bin").join(&memory_binary), "memory", "memory", "0.2.0");

    update_at(&root, true, false).unwrap();

    assert_eq!(read_manifest(&root.join("plugins/memory")).unwrap().version, "0.2.0");

    fs::write(
        plugin_root.join("krabb-plugin.toml"),
        "id = 'memory'\nversion = '0.3.0'\ncapabilities = ['memory']\npermissions = ['network']\n",
    )
    .unwrap();

    assert!(update_at(&root, true, false).is_err());
    assert_eq!(read_manifest(&root.join("plugins/memory")).unwrap().version, "0.2.0");

    fs::write(
            plugin_root.join("krabb-plugin.toml"),
            "id = 'memory'\nversion = '0.2.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['memory']\n",
        )
        .unwrap();

    assert!(
        super::link_at(
            Source {
                id: "memory".into(),
                source: Some(plugin_root.display().to_string()),
                revision: None,
                version: None,
                yes: false
            },
            true,
            &root,
        )
        .is_err()
    );

    assert!(
        super::link_at(
            Source { id: "bad_id".into(), source: None, revision: None, version: None, yes: true },
            false,
            &root,
        )
        .is_err()
    );

    fs::write(
            plugin_root.join("krabb-plugin.toml"),
            "id = 'memory'\nversion = '0.1.0'\nprotocol = { major = 1, minor = 0 }\ncapabilities = ['memory']\n",
        )
        .unwrap();

    assert!(
        super::link_at(
            Source {
                id: "memory".into(),
                source: Some(plugin_root.display().to_string()),
                revision: None,
                version: None,
                yes: true,
            },
            true,
            &root,
        )
        .is_err()
    );

    fs::remove_file(plugin_root.join("bin").join(&memory_binary)).unwrap();

    assert!(
        super::link_at(
            Source {
                id: "memory".into(),
                source: Some(plugin_root.display().to_string()),
                revision: None,
                version: None,
                yes: true,
            },
            true,
            &root,
        )
        .is_err()
    );

    assert!(update_at(&root, false, false).is_err());

    super::remove_at(super::PluginRemove { id: "memory".into(), yes: true }, &root).unwrap();

    assert!(super::installed_at(&root).is_empty());

    super::list_at(&root, None, 1, false).unwrap();

    assert!(
        super::remove_at(super::PluginRemove { id: "missing".into(), yes: false }, &root).is_err()
    );

    assert!(
        super::remove_at(super::PluginRemove { id: "missing".into(), yes: true }, &root).is_err()
    );

    assert!(
        super::plugin(
            super::PluginCommand::Install(super::PluginInstall {
                ids: vec!["bad_id".into()],
                source: None,
                revision: None,
                link: false,
                unstable: false,
                yes: true,
                quiet: false,
            }),
            false,
        )
        .await
        .is_err()
    );

    assert!(super::remove(super::PluginRemove { id: "bad_id".into(), yes: true }).is_err());

    local_session(SessionCommand::List(super::ListOutput { page: 1, json: false }), &root).unwrap();

    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_local_sources_are_not_treated_as_remote_git_urls() {
    let root = test_root("missing-local-source");

    for source in [
        root.join("missing.git").display().to_string(),
        format!("file://{}", root.join("missing").display()),
        format!("./missing-plugin-source-{}/plugin", super::now()),
        format!("../missing-plugin-source-{}/plugin", super::now()),
    ] {
        let error = resolve(&source, None).err().unwrap().to_string();

        assert!(error.contains("Local plugin source"), "{error}");
        assert!(error.contains("reinstall the plugin with --yes"), "{error}");
        assert!(!error.contains("Git URL"), "{error}");
    }

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn moved_linked_sources_can_be_reinstalled_without_resetting_configuration() {
    let root = test_root("moved-plugin-source");
    let source = root.join("source");
    let moved = root.join("moved");
    let binary = source.join("bin/krabb-plugin-memory");

    fs::create_dir_all(source.join("bin")).unwrap();

    fs::write(
            source.join("krabb-plugin.toml"),
            "id = 'memory'\nversion = '0.1.0'\ncapabilities = ['memory']\n\n[protocol]\nmajor = 0\nminor = 1\n",
        )
        .unwrap();

    write_test_plugin(&binary, "memory", "memory", "0.1.0");
    super::init_at(&root).unwrap();

    let config = super::load_config_text(&root).unwrap();

    let install = |path: &Path| {
        super::link_at(
            Source {
                id: "memory".into(),
                source: Some(path.display().to_string()),
                revision: None,
                version: None,
                yes: true,
            },
            true,
            &root,
        )
        .unwrap();
    };

    install(&source);
    fs::rename(&source, &moved).unwrap();

    assert!(super::binary_at("memory", &root).is_none());

    let lock = super::load_lock_at(&root).unwrap();

    let error =
        super::inspect_update(&root, "memory", &lock.plugins["memory"]).err().unwrap().to_string();

    assert!(error.contains("Local plugin source"));
    assert!(!super::preview_updates_at(&root, false).unwrap());

    install(&moved);

    let binary = super::binary_at("memory", &root).unwrap();
    let lock = super::load_lock_at(&root).unwrap();

    assert_eq!(lock.plugins["memory"].source, moved.canonicalize().unwrap().display().to_string());

    assert!(lock.plugins["memory"].linked);
    assert_eq!(super::load_config_text(&root).unwrap(), config);
    assert!(super::verify_plugin_integrity(&root, "memory", &binary).is_ok());
    assert!(!super::preview_updates_at(&root, false).unwrap());

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn linked_plugin_integrity_survives_source_relocation() {
    let root = test_root("linked-plugin-relocation");

    for (id, legacy, current) in
        [("telegram", "messaging/telegram", "channels/telegram"), ("tui", "tui", "clients/tui")]
    {
        let source = root.join("krabb-plugins").join(legacy);
        let relocated = root.join("krabb-plugins").join(current);
        let binary = root.join("target/debug").join(super::plugin_name(id));
        let installed = root.join("plugins").join(id);

        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::create_dir_all(installed.join("bin")).unwrap();
        fs::create_dir_all(relocated.parent().unwrap()).unwrap();
        fs::write(&binary, "binary").unwrap();
        fs::write(source.join("krabb-plugin.toml"), "manifest").unwrap();
        fs::write(installed.join("krabb-plugin.toml"), "manifest").unwrap();

        let link = installed.join("bin").join(super::plugin_name(id));
        let entry: super::Entry = serde_json::from_value(serde_json::json!({
            "source": source.display().to_string(),
            "default": true,
            "linked": true,
            "version": "0.1.0",
            "protocol": {"major": 0, "minor": 1},
            "capabilities": []
        }))
        .unwrap();

        super::link_binary(&binary, &link).unwrap();
        fs::rename(&source, &relocated).unwrap();
        super::verify_plugin_binary(id, &entry, &installed.join("krabb-plugin.toml"), &link)
            .unwrap();
        fs::write(relocated.join("krabb-plugin.toml"), "changed").unwrap();

        assert!(
            super::verify_plugin_binary(id, &entry, &installed.join("krabb-plugin.toml"), &link)
                .is_err()
        );
    }

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn linked_plugin_binary_rebuilds_keep_passing_integrity_checks() {
    let root = test_root("linked-plugin-rebuild");
    let source = root.join("source");
    let binary = source.join("bin/krabb-plugin-memory");
    let manifest = source.join("krabb-plugin.toml");

    fs::create_dir_all(source.join("bin")).unwrap();

    fs::write(
            &manifest,
            "id = 'memory'\nversion = '0.1.0'\ncapabilities = ['memory']\n\n[protocol]\nmajor = 0\nminor = 1\n",
        )
        .unwrap();

    write_test_plugin(&binary, "memory", "memory", "0.1.0");

    super::init_at(&root).unwrap();

    super::link_at(
        Source {
            id: "memory".into(),
            source: Some(source.display().to_string()),
            revision: None,
            version: None,
            yes: true,
        },
        true,
        &root,
    )
    .unwrap();

    let installed = super::binary_at("memory", &root).unwrap();

    assert!(super::verify_plugin_integrity(&root, "memory", &installed).is_ok());

    use std::io::Write;

    let mut rebuilt = fs::OpenOptions::new().append(true).open(&binary).unwrap();

    rebuilt.write_all(b"# rebuilt\n").unwrap();
    rebuilt.flush().unwrap();

    assert!(super::verify_plugin_integrity(&root, "memory", &installed).is_ok());

    fs::write(&manifest, "id = 'memory'\nversion = '0.2.0'\n").unwrap();

    assert!(super::verify_plugin_integrity(&root, "memory", &installed).is_err());

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn covers_offline_delivery_and_krabfile_commands() {
    let root = test_root("offline-delivery");
    let _ = fs::remove_dir_all(&root);

    super::init_at(&root).unwrap();

    let mut store = super::state::Store::load(root.join("data/sessions/index.json")).unwrap();

    store.create("main", "test").unwrap();

    store
        .reply(
            "main",
            Message {
                id: "message".into(),
                session: "main".into(),
                role: Role::User,
                sender: None,
                content: vec![Content::Text { text: "hello".into() }],
            },
            "delivery",
            "telegram",
            "chat",
            None,
            "reply",
        )
        .unwrap();

    store.uncertain("delivery", "interrupted").unwrap();

    delivery_at(DeliveryCommand::List(super::ListOutput { page: 1, json: false }), &root)
        .await
        .unwrap();

    delivery_at(DeliveryCommand::List(super::ListOutput { page: 1, json: true }), &root)
        .await
        .unwrap();

    assert!(
        super::delivery_at_json(
            DeliveryCommand::Retry(Name { id: "delivery".into(), yes: false }),
            &root,
            true,
        )
        .await
        .is_err()
    );

    delivery_at(DeliveryCommand::Retry(Name { id: "delivery".into(), yes: true }), &root)
        .await
        .unwrap();

    assert!(
        delivery_at(
            DeliveryCommand::Drop(super::delivery::Drop {
                id: Some("missing".into()),
                yes: true,
                ..Default::default()
            }),
            &root
        )
        .await
        .is_err()
    );

    delivery_at(
        DeliveryCommand::Drop(super::delivery::Drop {
            id: Some("delivery".into()),
            yes: true,
            ..Default::default()
        }),
        &root,
    )
    .await
    .unwrap();

    let krabfile = root.join("Krabfile");

    export_krabfile_at(
        KrabfileExport { destination: None, path: Some(krabfile.clone()), force: false },
        &root,
    )
    .unwrap();

    assert!(
        export_krabfile_at(
            KrabfileExport { destination: None, path: Some(krabfile.clone()), force: false },
            &root,
        )
        .is_err()
    );

    export_krabfile_at(
        KrabfileExport { destination: None, path: Some(krabfile.clone()), force: true },
        &root,
    )
    .unwrap();

    let export_dir = root.join("export");

    fs::create_dir_all(&export_dir).unwrap();

    export_krabfile_at(
        KrabfileExport { destination: Some(export_dir.clone()), path: None, force: false },
        &root,
    )
    .unwrap();

    assert!(export_dir.join("Krabfile").is_file());

    let error = super::import_krabfile_at_mode(
        KrabfileImport { path: Some(krabfile.clone()), yes: false, force: true },
        &root,
        true,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("Use --yes to continue without a prompt."));

    let missing = root.join("missing-krabfile");

    let error = import_krabfile_at(
        KrabfileImport { path: Some(missing.clone()), yes: true, force: false },
        &root,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("Krabfile not found"));
    assert!(error.contains(&missing.display().to_string()));

    let invalid = root.join("invalid-krabfile");

    fs::write(&invalid, "version = [\n").unwrap();

    let error = import_krabfile_at(
        KrabfileImport { path: Some(invalid.clone()), yes: true, force: false },
        &root,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("Krabfile at"));
    assert!(error.contains("is invalid at line 1, column"));

    import_krabfile_at(
        KrabfileImport { path: Some(root.join("Krabfile")), yes: true, force: true },
        &root,
    )
    .unwrap();

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn applying_preview_uses_the_candidate_captured_before_a_later_preview() {
    use std::os::unix::fs::PermissionsExt;

    let root = test_root("plugin-update-preview-snapshot");
    let _ = fs::remove_dir_all(&root);

    super::init_at(&root).unwrap();

    let source = root.join("sources/memory");

    fs::create_dir_all(source.join("bin")).unwrap();

    let manifest = source.join("krabb-plugin.toml");
    let binary = source.join("bin/krabb-plugin-memory");

    for version in ["0.1.0", "0.2.0", "0.3.0"] {
        fs::write(
                &manifest,
                format!(
                    "id = 'memory'\nversion = '{version}'\nprotocol = {{ major = 0, minor = 1 }}\ncapabilities = ['memory']\n"
                ),
            )
            .unwrap();

        write_test_plugin(&binary, "memory", "memory", version);
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();

        if version == "0.1.0" {
            super::link_at(
                Source {
                    id: "memory".into(),
                    source: Some(source.display().to_string()),
                    revision: None,
                    version: None,
                    yes: true,
                },
                true,
                &root,
            )
            .unwrap();
        } else if version == "0.2.0" {
            super::preview_updates_at(&root, false).unwrap();
        } else {
            let snapshot = super::load_update_preview_snapshot_at(&root).unwrap();

            super::preview_updates_at(&root, false).unwrap();
            super::update_at_with_previews(&root, true, &snapshot.candidates, true).unwrap();

            assert_eq!(super::load_lock_at(&root).unwrap().plugins["memory"].version, "0.2.0");
            assert!(!super::load_lock_at(&root).unwrap().plugins["memory"].linked);

            let installed = super::binary_at("memory", &root).unwrap();

            assert!(super::verify_plugin_integrity(&root, "memory", &installed).is_ok());
        }
    }

    super::preview_updates_at(&root, false).unwrap();

    let snapshot = super::load_update_preview_snapshot_at(&root).unwrap();
    let mut lock = super::load_lock_at(&root).unwrap();

    lock.plugins.get_mut("memory").unwrap().version = "0.2.1".into();
    super::save_lock_at(&root, &lock).unwrap();

    let error =
        super::update_at_with_previews(&root, true, &snapshot.candidates, true).unwrap_err();

    assert!(error.to_string().contains("preview for memory is stale"));

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn applying_an_unchanged_linked_preview_keeps_the_source_link_valid() {
    use std::os::unix::fs::PermissionsExt;

    let root = test_root("linked-plugin-preview-integrity");
    let _ = fs::remove_dir_all(&root);

    super::init_at(&root).unwrap();

    let source = root.join("sources/memory");

    fs::create_dir_all(source.join("bin")).unwrap();

    for version in ["0.1.0", "0.2.0"] {
        fs::write(
                source.join("krabb-plugin.toml"),
                format!(
                    "id = 'memory'\nversion = '{version}'\nprotocol = {{ major = 0, minor = 1 }}\ncapabilities = ['memory']\n"
                ),
            )
            .unwrap();

        let binary = source.join("bin/krabb-plugin-memory");

        write_test_plugin(&binary, "memory", "memory", version);
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();

        if version == "0.1.0" {
            super::link_at(
                Source {
                    id: "memory".into(),
                    source: Some(source.display().to_string()),
                    revision: None,
                    version: None,
                    yes: true,
                },
                true,
                &root,
            )
            .unwrap();
        }
    }

    super::preview_updates_at(&root, false).unwrap();

    let snapshot = super::load_update_preview_snapshot_at(&root).unwrap();

    super::update_at_with_previews(&root, true, &snapshot.candidates, true).unwrap();

    let lock = super::load_lock_at(&root).unwrap();

    assert!(lock.plugins["memory"].linked);

    let installed = super::binary_at("memory", &root).unwrap();

    assert!(super::verify_plugin_integrity(&root, "memory", &installed).is_ok());

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn automatic_updates_ignore_stale_saved_previews() {
    use std::os::unix::fs::PermissionsExt;

    let root = test_root("plugin-update-stale-preview");
    let _ = fs::remove_dir_all(&root);

    super::init_at(&root).unwrap();

    let source = root.join("sources/memory");

    fs::create_dir_all(source.join("bin")).unwrap();

    for version in ["0.1.0", "0.2.0"] {
        fs::write(
                source.join("krabb-plugin.toml"),
                format!(
                    "id = 'memory'\nversion = '{version}'\nprotocol = {{ major = 0, minor = 1 }}\ncapabilities = ['memory']\n"
                ),
            )
            .unwrap();

        let binary = source.join("bin/krabb-plugin-memory");

        write_test_plugin(&binary, "memory", "memory", version);
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();

        if version == "0.1.0" {
            super::link_at(
                Source {
                    id: "memory".into(),
                    source: Some(source.display().to_string()),
                    revision: None,
                    version: None,
                    yes: true,
                },
                true,
                &root,
            )
            .unwrap();
        }
    }

    super::preview_updates_at(&root, false).unwrap();
    super::update_at(&root, true, true).unwrap();

    assert_eq!(super::load_lock_at(&root).unwrap().plugins["memory"].version, "0.2.0");

    fs::write(
            source.join("krabb-plugin.toml"),
            "id = 'memory'\nversion = '0.3.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['memory']\n",
        )
        .unwrap();

    write_test_plugin(&source.join("bin/krabb-plugin-memory"), "memory", "memory", "0.3.0");

    super::update_at(&root, true, false).unwrap();

    assert_eq!(super::load_lock_at(&root).unwrap().plugins["memory"].version, "0.3.0");

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn applying_empty_preview_does_not_inspect_changed_sources() {
    use std::os::unix::fs::PermissionsExt;

    let root = test_root("plugin-update-empty-preview");
    let _ = fs::remove_dir_all(&root);

    super::init_at(&root).unwrap();

    let source = root.join("sources/memory");

    fs::create_dir_all(source.join("bin")).unwrap();

    fs::write(
            source.join("krabb-plugin.toml"),
            "id = 'memory'\nversion = '0.1.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['memory']\n",
        )
        .unwrap();

    let binary = source.join("bin/krabb-plugin-memory");

    write_test_plugin(&binary, "memory", "memory", "0.1.0");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();

    super::link_at(
        Source {
            id: "memory".into(),
            source: Some(source.display().to_string()),
            revision: None,
            version: None,
            yes: true,
        },
        true,
        &root,
    )
    .unwrap();

    super::preview_updates_at(&root, false).unwrap();

    assert!(super::load_update_previews_at(&root).unwrap().is_empty());

    fs::write(
            source.join("krabb-plugin.toml"),
            "id = 'memory'\nversion = '0.2.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['memory']\n",
        )
        .unwrap();

    write_test_plugin(&binary, "memory", "memory", "0.2.0");

    super::update_plugins_at(&root, true, false).await.unwrap();

    assert_eq!(super::load_lock_at(&root).unwrap().plugins["memory"].version, "0.1.0");

    let _ = fs::remove_dir_all(root);
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn updates_inspectable_plugins_when_another_source_is_broken() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let root = test_root("plugin-update-broken-source");
    let _ = fs::remove_dir_all(&root);

    super::init_at(&root).unwrap();

    for id in ["broken", "valid"] {
        let source = root.join("sources").join(id);

        fs::create_dir_all(source.join("bin")).unwrap();

        fs::write(
                source.join("krabb-plugin.toml"),
                format!(
                    "id = '{id}'\nversion = '0.1.0'\nprotocol = {{ major = 0, minor = 1 }}\ncapabilities = ['tool']\n"
                ),
            )
            .unwrap();

        let binary = source.join("bin").join(super::plugin_name(id));

        write_test_plugin(&binary, id, "tool", "0.1.0");

        #[cfg(unix)]
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();

        super::link_at(
            Source {
                id: id.into(),
                source: Some(source.display().to_string()),
                revision: None,
                version: None,
                yes: true,
            },
            true,
            &root,
        )
        .unwrap();
    }

    super::preview_updates_at(&root, false).unwrap();

    let broken_source = root.join("sources/broken");

    fs::remove_dir_all(&broken_source).unwrap();

    let valid_source = root.join("sources/valid");

    fs::write(
            valid_source.join("krabb-plugin.toml"),
            "id = 'valid'\nversion = '0.2.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['tool']\n",
        )
        .unwrap();

    write_test_plugin(
        &valid_source.join("bin").join(super::plugin_name("valid")),
        "valid",
        "tool",
        "0.2.0",
    );

    let error = super::updates(&root, "auto").unwrap_err();

    assert!(error.to_string().contains("broken"));
    assert_eq!(super::load_lock_at(&root).unwrap().plugins["valid"].version, "0.2.0");

    fs::write(
            valid_source.join("krabb-plugin.toml"),
            "id = 'valid'\nversion = '0.3.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['tool']\n",
        )
        .unwrap();

    write_test_plugin(
        &valid_source.join("bin").join(super::plugin_name("valid")),
        "valid",
        "tool",
        "0.3.0",
    );

    super::preview_updates_at(&root, true).unwrap();

    let previews = super::load_update_previews_at(&root).unwrap();

    assert_eq!(previews.keys().map(String::as_str).collect::<Vec<_>>(), ["valid"]);

    fs::remove_dir_all(root.join(".plugin-update-preview")).unwrap();
    super::update_plugins_at(&root, true, false).await.unwrap();

    let lock = super::load_lock_at(&root).unwrap();

    assert_eq!(lock.plugins["broken"].version, "0.1.0");
    assert_eq!(lock.plugins["valid"].version, "0.3.0");

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn updates_plugins_offline_when_the_daemon_is_absent() {
    let root = test_root("plugin-update-offline");
    let _ = fs::remove_dir_all(&root);

    super::init_at(&root).unwrap();

    assert_eq!(
        super::active_ids(serde_json::json!({"items": ["memory", "tools"]})).unwrap(),
        ["memory", "tools"]
    );

    assert!(super::active_ids(serde_json::json!({})).is_err());
    assert!(super::active_ids(serde_json::json!({"items": ["../escape"]})).is_err());

    super::update_plugins_at(&root, false, false).await.unwrap();

    let _ = fs::remove_dir_all(root);
}

#[cfg(any(unix, windows))]
pub(super) fn write_test_plugin(path: &Path, id: &str, capability: &str, version: &str) {
    #[cfg(unix)]
        fs::write(
            path,
            format!(
                "#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\n*hello*) printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocol\":{{\"major\":0,\"minor\":1}},\"id\":\"{id}\",\"version\":\"{version}\",\"capabilities\":[\"{capability}\"]}}}}' ;;\n*shutdown*) printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"ok\":true}}}}'; exit 0 ;;\nesac\ndone\n"
            ),
        )
        .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(windows)]
    {
        let source = path.with_extension("rs");
        let source_code = r##"
use std::io::{BufRead, Write};

fn main() {
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let id = line
            .split_once("\"id\":")
            .and_then(|(_, rest)| rest.split(',').next())
            .unwrap_or("1");
        let response = if line.contains("\"method\":\"hello\"") {
            format!(r#"{{"jsonrpc":"2.0","id":{},"result":{{"protocol":{{"major":0,"minor":1}},"id":"__PLUGIN_ID__","version":"__PLUGIN_VERSION__","capabilities":["__PLUGIN_CAPABILITY__"]}}}}"#, id)
        } else if line.contains("\"method\":\"shutdown\"") {
            format!(r#"{{"jsonrpc":"2.0","id":{},"result":{{"ok":true}}}}"#, id)
        } else {
            continue;
        };
        println!("{response}");
        std::io::stdout().flush().unwrap();
        if line.contains("\"method\":\"shutdown\"") {
            break;
        }
    }
}
"##
            .replace("__PLUGIN_ID__", id)
            .replace("__PLUGIN_VERSION__", version)
            .replace("__PLUGIN_CAPABILITY__", capability);

        fs::write(&source, source_code).unwrap();

        let output = std::process::Command::new("rustc")
            .args(["--edition", "2024"])
            .arg(&source)
            .arg("-o")
            .arg(path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "could not compile the Windows test plugin: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        fs::remove_file(source).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn resolves_git_sources_and_verified_archives() {
    let root = std::env::temp_dir().join(format!("krabb-sources-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let repo = root.join("repo");

    fs::create_dir_all(repo.join("bin")).unwrap();

    fs::write(
            repo.join("krabb-plugin.toml"),
            "id = 'memory'\nversion = '0.1.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['memory']\n",
        )
        .unwrap();

    fs::write(repo.join("bin/krabb-plugin-memory"), "plugin").unwrap();

    assert!(
        git::command()
            .args(["-C", repo.to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap()
            .success()
    );

    assert!(
        git::command()
            .args(["-C", repo.to_str().unwrap(), "config", "user.email", "test@example.com"])
            .status()
            .unwrap()
            .success()
    );

    assert!(
        git::command()
            .args(["-C", repo.to_str().unwrap(), "config", "user.name", "Krabb Test"])
            .status()
            .unwrap()
            .success()
    );

    assert!(
        git::command()
            .args(["-C", repo.to_str().unwrap(), "config", "commit.gpgsign", "false"])
            .status()
            .unwrap()
            .success()
    );

    assert!(
        git::command().args(["-C", repo.to_str().unwrap(), "add", "."]).status().unwrap().success()
    );

    assert!(
        git::command()
            .args(["-C", repo.to_str().unwrap(), "commit", "-qm", "initial"])
            .status()
            .unwrap()
            .success()
    );

    let head = revision(&repo);
    let cloned = resolve(&format!("git+file://{}", repo.display()), Some(&head)).unwrap();

    assert!(cloned.path.join("krabb-plugin.toml").is_file());
    assert!(resolve(&format!("git+file://{}", repo.display()), Some("wrong")).is_err());

    assert!(
        git::command()
            .args(["-C", repo.to_str().unwrap(), "commit", "--allow-empty", "-qm", "moved"])
            .status()
            .unwrap()
            .success()
    );

    let pinned = resolve(&format!("git+file://{}", repo.display()), Some(&head)).unwrap();

    assert_eq!(revision(&pinned.path), head);

    let archive = root.join("memory.tar.gz");

    assert!(
        std::process::Command::new("tar")
            .args(["-czf", archive.to_str().unwrap(), "-C", root.to_str().unwrap(), "repo"])
            .status()
            .unwrap()
            .success()
    );

    let mut hash = Sha256::new();

    hash.update(fs::read(&archive).unwrap());

    let checksum = super::hex_digest(hash.finalize());

    let extracted =
        resolve(&format!("file://{}#sha256={checksum}", archive.display()), None).unwrap();

    assert!(extracted.path.join("krabb-plugin.toml").is_file());
    assert!(resolve(&format!("file://{}", archive.display()), None).is_err());

    assert!(
        resolve(&format!("file://{}#sha256={}", archive.display(), "0".repeat(64)), None,).is_err()
    );

    assert!(resolve("https://example.com/plugin.zip", None).is_err());
    assert!(resolve(&root.join("plain.txt").display().to_string(), None).is_err());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn rejects_unsafe_archive_inputs() {
    let root = std::env::temp_dir().join(format!("krabb-archive-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("plain.txt"), "plain").unwrap();

    assert_eq!(
        canonical_source(&format!("file://{}", root.join("plain.txt").display())),
        format!("file://{}", root.join("plain.txt").canonicalize().unwrap().display())
    );

    assert!(verify_archive(&root.join("plain.txt"), None).is_err());

    let oversized = root.join("oversized.tar");

    std::fs::File::create(&oversized).unwrap().set_len(ARCHIVE_LIMIT + 1).unwrap();

    assert!(verify_archive(&oversized, Some(&"0".repeat(64))).is_err());
    assert!(download("https://127.0.0.1:1/missing.tar", &root.join("missing")).is_err());
    assert!(!archive_url("https://example.com/plugin.txt"));
    assert!(archive_url("plugin.tar"));
    assert!(archive_url("plugin.tgz"));
    assert!(archive_url("plugin.zip?download=1"));
    assert!(archive(Path::new("plugin.tar.gz")));
    assert!(archive(Path::new("/releases/PLUGIN.ZIP")));
    assert!(archive(Path::new(r"\\?\D:\releases\plugin.zip")));
    assert!(!archive(Path::new("plugin.txt")));
    assert!(!archive(Path::new("plugin.zip?download=1")));

    let redacted = redact(b"https://user:secret@example.com/path Authorization: Bearer token");

    assert!(!redacted.contains("secret"));
    assert!(!redacted.contains("Bearer token"));
    assert!(embedded("https://user@example.com/repo.git"));
    assert!(embedded("ssh://git:secret@example.com/repo.git"));
    assert!(!embedded("ssh://git@example.com/repo.git"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn validates_archive_roots_and_contents() {
    let root = std::env::temp_dir().join(format!("krabb-archive-root-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(root.join("nested")).unwrap();

    fs::write(
        root.join("krabb-plugin.toml"),
        "id = 'echo'\nversion = '0.1.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = []\n",
    )
    .unwrap();

    fs::write(root.join("nested/data"), "data").unwrap();

    assert_eq!(archive_root(&root).unwrap(), root);

    safe_archive(&root).unwrap();

    let outer = root.join("outer");

    fs::create_dir_all(outer.join("echo")).unwrap();
    fs::copy(root.join("krabb-plugin.toml"), outer.join("echo/krabb-plugin.toml")).unwrap();

    assert_eq!(archive_root(&outer).unwrap(), outer.join("echo"));

    let empty = root.join("empty");

    fs::create_dir_all(&empty).unwrap();

    assert!(archive_root(&empty).is_err());

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn rejects_archive_links_before_extraction() {
    let root = test_root("archive-links");

    fs::create_dir_all(root.join("symlink")).unwrap();
    fs::write(root.join("symlink/target"), "data").unwrap();
    std::os::unix::fs::symlink("target", root.join("symlink/link")).unwrap();

    let symlink_archive = root.join("symlink.tar");

    assert!(
        std::process::Command::new("tar")
            .args([
                "-cf",
                symlink_archive.to_str().unwrap(),
                "-C",
                root.join("symlink").to_str().unwrap(),
                "target",
                "link",
            ])
            .status()
            .unwrap()
            .success()
    );

    assert!(validate_archive(&symlink_archive).is_err());

    fs::create_dir_all(root.join("hardlink")).unwrap();
    fs::write(root.join("hardlink/source"), "data").unwrap();
    fs::hard_link(root.join("hardlink/source"), root.join("hardlink/link")).unwrap();

    let hardlink_archive = root.join("hardlink.tar");

    assert!(
        std::process::Command::new("tar")
            .args([
                "-cf",
                hardlink_archive.to_str().unwrap(),
                "-C",
                root.join("hardlink").to_str().unwrap(),
                "source",
                "link",
            ])
            .status()
            .unwrap()
            .success()
    );

    assert!(validate_archive(&hardlink_archive).is_err());

    let zip_root = root.join("zip");

    fs::create_dir_all(&zip_root).unwrap();
    fs::write(zip_root.join("target"), "data").unwrap();

    let regular_archive = root.join("regular.zip");

    assert!(
        std::process::Command::new("zip")
            .current_dir(&zip_root)
            .args(["-q", regular_archive.to_str().unwrap(), "target"])
            .status()
            .unwrap()
            .success()
    );

    assert!(validate_archive(&regular_archive).is_ok());

    std::os::unix::fs::symlink("target", zip_root.join("link")).unwrap();

    let symlink_zip = root.join("symlink.zip");

    assert!(
        std::process::Command::new("zip")
            .current_dir(&zip_root)
            .args(["-q", "-y", symlink_zip.to_str().unwrap(), "target", "link"])
            .status()
            .unwrap()
            .success()
    );

    assert!(validate_archive(&symlink_zip).is_err());

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn isolates_group_workspaces() {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("krabb-isolate-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(&root).unwrap();

    let home = root.clone();
    let root = krabb_file::directory(&home, Path::new("data/workspaces/repository")).unwrap();

    assert!(
        git::command()
            .args(["-C", root.to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap()
            .success()
    );

    fs::write(root.join("README"), "test").unwrap();
    fs::write(root.join(".gitattributes"), "README filter=test\n").unwrap();

    for args in [
        vec!["-C", root.to_str().unwrap(), "config", "user.email", "test@example.com"],
        vec!["-C", root.to_str().unwrap(), "config", "user.name", "Krabb Test"],
        vec!["-C", root.to_str().unwrap(), "config", "commit.gpgsign", "false"],
        vec!["-C", root.to_str().unwrap(), "add", "."],
        vec!["-C", root.to_str().unwrap(), "commit", "-qm", "initial"],
    ] {
        assert!(git::command().args(args).status().unwrap().success());
    }

    let hook = root.join(".git/hooks/post-checkout");

    let monitor = root.join("monitor");

    for (script, marker) in [(&hook, "hook-ran"), (&monitor, "monitor-ran")] {
        fs::write(script, format!("#!/bin/sh\ntouch '{}'\n", root.join(marker).display())).unwrap();
        fs::set_permissions(script, fs::Permissions::from_mode(0o700)).unwrap();
    }

    assert!(
        git::command()
            .arg("-C")
            .arg(&root)
            .args(["config", "core.fsmonitor"])
            .arg(&monitor)
            .status()
            .unwrap()
            .success()
    );

    assert!(isolate_at("telegram-7", &home).unwrap().is_some());
    assert!(isolate_at("telegram-7", &home).unwrap().is_some());
    assert!(isolate_at("../escape", &home).unwrap().is_none());
    assert!(!root.join("hook-ran").exists());
    assert!(!root.join("monitor-ran").exists());

    assert_eq!(fs::read_to_string(home.join("worktrees/telegram-7/README")).unwrap(), "test");

    for key in
        ["filter.test.smudge", "filter.test.process", "filter.test.clean", "remote.origin.promisor"]
    {
        assert!(
            git::command()
                .arg("-C")
                .arg(&root)
                .args(["config", key, "true"])
                .status()
                .unwrap()
                .success()
        );

        let error = isolate_at("blocked", &home).unwrap_err().to_string();

        assert!(error.contains("explicitly approved shell"), "{error}");
        assert!(!home.join("worktrees/blocked").exists());

        assert!(
            git::command()
                .arg("-C")
                .arg(&root)
                .args(["config", "--unset", key])
                .status()
                .unwrap()
                .success()
        );
    }

    assert!(
        git::command()
            .arg("-C")
            .arg(&root)
            .args(["config", "--unset", "core.fsmonitor"])
            .status()
            .unwrap()
            .success()
    );

    let filter = root.join(".git/filter-config");

    fs::write(&filter, "[filter \"test\"]\nsmudge = touch filter-ran; cat\n").unwrap();

    let conditional = "includeIf.gitdir:**/worktrees/conditional.path";

    assert!(
        git::command()
            .arg("-C")
            .arg(&root)
            .args(["config", conditional])
            .arg(&filter)
            .status()
            .unwrap()
            .success()
    );

    assert!(super::check_checkout(&root).is_ok());

    assert!(
        isolate_at("conditional", &home)
            .unwrap_err()
            .to_string()
            .contains("explicitly approved shell")
    );

    assert!(!home.join("worktrees/conditional").exists());

    assert!(
        git::command()
            .arg("-C")
            .arg(&root)
            .args(["config", "--unset", conditional])
            .status()
            .unwrap()
            .success()
    );

    assert!(isolate_at("conditional", &home).unwrap().is_some());

    fs::write(home.join("worktrees/telegram-7/dirty"), "keep me").unwrap();

    assert!(super::state::remove_worktree(&home, "telegram-7").is_err());
    assert!(home.join("worktrees/telegram-7/dirty").is_file());

    fs::remove_file(home.join("worktrees/telegram-7/dirty")).unwrap();
    super::state::remove_worktree(&home, "telegram-7").unwrap();

    assert!(!home.join("worktrees/telegram-7").exists());

    let _ = fs::remove_dir_all(home);
}

#[tokio::test]
async fn falls_back_when_daemon_is_absent() {
    let root = std::env::temp_dir().join(format!("krabb-control-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    assert!(super::control_at("status", serde_json::json!({}), &root).await.unwrap().is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn resolves_telegram_media_references() {
    let script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}' ;; *media*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"uri":"file:///tmp/media.bin"}}' ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#;
    let channel = Process::start_with("sh", ["-c", script]).await.unwrap();
    let channel = Live::new(channel);

    let content = super::resolve_media(
        "telegram",
        &channel,
        vec![Content::Image { uri: "telegram://file/id".into(), alt: None }],
        &mut 2,
    )
    .await;

    assert_eq!(content[0], Content::Image { uri: "file:///tmp/media.bin".into(), alt: None });

    channel.stop().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn transcribes_voice_with_the_speech_plugin_and_removes_raw_audio() {
    let root = test_root("voice");
    let media = root.join("media");

    fs::create_dir_all(&media).unwrap();

    let voice = media.join("voice.ogg");

    fs::write(&voice, b"voice").unwrap();

    let script = r#"while IFS= read -r line; do request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"protocol":{"major":0,"minor":1},"id":"whisper","version":"0.1.0","capabilities":["speech"]}}' ;; *transcribe*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"text":"recognized words"}}' ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"ok":true}}'; exit 0 ;; esac; done"#;
    let plugin = Process::start_with("sh", ["-c", script]).await.unwrap();
    let plugins = Plugins::default();

    plugins.insert(Live::new(plugin)).await;

    let mut call = 2;

    for (index, channel) in ["telegram", "signal", "slack"].into_iter().enumerate() {
        if index > 0 {
            fs::write(&voice, b"voice").unwrap();
        }

        let content = super::transcribe_media(
            &plugins,
            channel,
            vec![Content::Audio {
                uri: format!("file://{}", voice.display()),
                mime: Some("audio/ogg".into()),
            }],
            &media,
            &mut call,
        )
        .await;

        assert_eq!(content, vec![Content::Text { text: "recognized words".into() }]);
    }

    assert!(!voice.exists());

    plugins.all().await.into_iter().next().unwrap().1.stop().await.unwrap();

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn omits_unsafe_voice_files_without_leaking_their_paths() {
    let root = test_root("voice-unsafe");

    fs::create_dir_all(&root).unwrap();

    let outside = root.join("outside.ogg");

    fs::write(&outside, b"voice").unwrap();

    let plugins = Plugins::default();
    let mut call = 2;

    let content = super::transcribe_media(
        &plugins,
        "telegram",
        vec![Content::Audio { uri: format!("file://{}", outside.display()), mime: None }],
        &root.join("media"),
        &mut call,
    )
    .await;

    assert_eq!(content, vec![super::unavailable_voice()]);
    assert!(!content[0].render().contains(&outside.display().to_string()));
    assert_eq!(call, 2);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn bounds_cached_media_files() {
    let root = test_root("media-size");

    fs::create_dir_all(&root).unwrap();

    let large = root.join("large.bin");

    fs::File::create(&large).unwrap().set_len(super::MEDIA_LIMIT + 1).unwrap();

    assert!(super::media_file(&format!("file://{}", large.display()), &root).is_none());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn pins_image_media_outside_the_expiring_cache() {
    let root = test_root("media-pin");

    fs::create_dir_all(&root).unwrap();

    let source = root.join("photo.bin");

    fs::write(&source, b"\x89PNG\r\n\x1a\nimage").unwrap();

    let content = super::media::pin(
        vec![Content::Image {
            uri: format!("file://{}", source.display()),
            alt: Some("photo".into()),
        }],
        &root,
    );

    let Content::Image { uri, alt } = &content[0] else {
        panic!("expected pinned image");
    };

    let uri = uri.clone();
    let alt = alt.clone();

    assert_eq!(alt.as_deref(), Some("photo"));

    let pinned = PathBuf::from(uri.strip_prefix("file://").unwrap());

    assert_eq!(pinned.parent(), Some(fs::canonicalize(root.join("pinned")).unwrap().as_path()));
    assert!(pinned.is_file());

    let again = super::media::pin(content, &root);

    assert_eq!(again[0], Content::Image { uri, alt: Some("photo".into()) });

    let _ = fs::remove_dir_all(root);
}

#[test]
fn prepares_only_bounded_confined_images() {
    let root = test_root("image-media");

    fs::create_dir_all(&root).unwrap();

    let first = root.join("first.png");
    let second = root.join("second.png");
    let mut first_bytes = b"\x89PNG\r\n\x1a\n".to_vec();

    first_bytes.resize(3 * 1024 * 1024, 7);

    let second_bytes = first_bytes.clone();

    fs::write(&first, &first_bytes).unwrap();
    fs::write(&second, &second_bytes).unwrap();

    let mut messages = vec![Message {
        id: "image-message".into(),
        session: "image-session".into(),
        role: Role::User,
        sender: None,
        content: vec![
            Content::Image { uri: format!("file://{}", first.display()), alt: None },
            Content::Image { uri: format!("file://{}", second.display()), alt: None },
            Content::Image { uri: "file:///outside/photo.png".into(), alt: None },
            Content::Image { uri: "file://unsupported".into(), alt: Some("description".into()) },
        ],
    }];

    super::prepare_images(&mut messages, &root);

    let Content::Image { uri, .. } = &messages[0].content[0] else {
        panic!("The confined image should remain an image.");
    };

    let (header, encoded) = uri.split_once(',').unwrap();

    assert_eq!(header, "data:image/png;base64");
    assert_eq!(super::STANDARD.decode(encoded).unwrap(), first_bytes);
    assert!(messages[0].content[1].render().contains("omitted"));
    assert!(messages[0].content[2].render().contains("omitted"));

    assert_eq!(
        messages[0].content[3],
        Content::Text { text: "[Image attachment unavailable. Description: description]".into() }
    );

    assert!(
        !messages[0]
            .content
            .iter()
            .any(|content| content.render().contains(&root.display().to_string()))
    );

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn handles_discord_audio_without_speech_plugin() {
    let content = vec![Content::Audio { uri: "file://voice.ogg".into(), mime: None }];
    let plugins = Plugins::default();
    let mut call = 2;

    let result =
        super::transcribe_media(&plugins, "discord", content.clone(), Path::new("."), &mut call)
            .await;

    assert_eq!(result[0].render(), "[Voice message omitted because transcription is unavailable.]");

    assert_eq!(call, 2);
}

#[tokio::test]
async fn expands_readable_file_attachments() {
    let root = test_root("text-media");

    fs::create_dir_all(&root).unwrap();

    let note = root.join("note.md");
    let secret = root.join("secret.pem");
    let large = root.join("large.txt");
    let wide = root.join("wide.txt");
    let cut = root.join("cut.txt");
    let photo = root.join("photo.bin");
    let broken = root.join("broken.bin");
    let huge = root.join("huge.bin");

    let mut straddle = "a".repeat(super::FILE_LIMIT as usize - 1);
    straddle.push('é');

    fs::write(&note, "A short note.").unwrap();
    fs::write(&secret, "PRIVATE KEY").unwrap();
    fs::write(&large, "a".repeat(super::FILE_LIMIT as usize + 10)).unwrap();
    fs::write(&wide, &straddle).unwrap();
    fs::write(&cut, b"caf\xc3").unwrap();
    fs::write(&photo, b"GIF89a\x01\x00\x01\x00").unwrap();
    fs::write(&broken, b"%PDF-1.7\nbroken").unwrap();
    fs::File::create(&huge).unwrap().set_len(super::MEDIA_LIMIT + 1).unwrap();

    let file = |path: &Path, name: &str, mime: &str| Content::File {
        uri: format!("file://{}", path.display()),
        name: name.into(),
        mime: Some(mime.into()),
    };

    let content = vec![
        file(&note, "note.md", "text/markdown"),
        file(&secret, "secret.pem", "application/octet-stream"),
        file(&large, "large.txt", "text/plain"),
        file(&wide, "wide.txt", "text/plain"),
        file(&cut, "cut.txt", "text/plain"),
        file(&photo, "funny.gif", "application/octet-stream"),
        file(&broken, "broken.pdf", "application/pdf"),
        file(&huge, "huge.log", "text/plain"),
        file(&root.join("missing.txt"), "missing.txt", "text/plain"),
    ];

    let result = super::expand_media_files(content.clone(), &root).await;
    let rendered = result.iter().map(Content::render).collect::<Vec<_>>();

    assert_eq!(result.len(), 9);
    assert_eq!(rendered[0], "[File: note.md]\nA short note.");
    assert_eq!(rendered[1], "[File secret.pem omitted because its file type is not supported.]");

    assert!(rendered[2].starts_with("[File: large.txt, first 64 KiB only]\naaa"));
    assert_eq!(rendered[2].matches('a').count(), super::FILE_LIMIT as usize + 1);

    assert!(rendered[3].starts_with("[File: wide.txt, first 64 KiB only]\naaa"));
    assert!(!rendered[3].contains('é'));

    assert_eq!(rendered[4], "[File cut.txt omitted because it is not valid text.]");

    assert_eq!(
        result[5],
        Content::Image {
            uri: format!("file://{}", photo.display()),
            alt: Some("funny.gif".into())
        }
    );

    // The test binary is not a PDF reader, so this pins how unframed child output is reported.

    assert_eq!(rendered[6], "[File broken.pdf omitted because the PDF could not be read.]");
    assert_eq!(rendered[7], "[File huge.log omitted because it is larger than 4 MiB.]");
    assert_eq!(rendered[8], "[File missing.txt omitted because it is unavailable.]");

    assert!(!rendered.iter().any(|item| item.contains(&root.display().to_string())));

    let outside = test_root("text-media-outside");

    fs::create_dir_all(&outside).unwrap();
    fs::File::create(outside.join("huge.log")).unwrap().set_len(super::MEDIA_LIMIT + 1).unwrap();

    let probe = vec![file(&outside.join("huge.log"), "huge.log", "text/plain")];
    let result = super::expand_media_files(probe, &root).await;

    assert_eq!(result[0].render(), "[File huge.log omitted because it is unavailable.]");

    let clip = vec![Content::File {
        uri: "telegram://file/clip".into(),
        name: "clip.mp4".into(),
        mime: Some("video/mp4".into()),
    }];

    let result = super::expand_media_files(clip, &root).await;

    assert_eq!(result[0].render(), "[File clip.mp4 omitted because video is not read yet.]");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[tokio::test]
async fn runs_session_commands_without_daemon() {
    let root = test_root("session");
    let _ = fs::remove_dir_all(&root);

    session_at(
        SessionCommand::New(SessionNew { id: "main".into(), model: Some("first".into()) }),
        &root,
    )
    .await
    .unwrap();

    session_at(SessionCommand::List(super::ListOutput { page: 1, json: false }), &root)
        .await
        .unwrap();

    session_at(SessionCommand::List(super::ListOutput { page: 1, json: true }), &root)
        .await
        .unwrap();

    session_at(SessionCommand::Show(Id { id: "main".into() }), &root).await.unwrap();

    session_at(SessionCommand::Fork(Fork { source: "main".into(), target: "copy".into() }), &root)
        .await
        .unwrap();

    session_at(
        SessionCommand::Model(SessionModel { id: "copy".into(), model: "second".into() }),
        &root,
    )
    .await
    .unwrap();

    session_at(SessionCommand::Cancel(Id { id: "copy".into() }), &root).await.unwrap();

    assert!(session_at(SessionCommand::Show(Id { id: "missing".into() }), &root).await.is_err());

    assert!(
        super::session_at_json(
            SessionCommand::Delete(SessionDelete { id: "main".into(), yes: false }),
            &root,
            true,
        )
        .await
        .is_err()
    );

    session_at(SessionCommand::Delete(SessionDelete { id: "main".into(), yes: true }), &root)
        .await
        .unwrap();

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn asks_a_local_plugin() {
    use std::os::unix::fs::PermissionsExt;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());

    let root = std::env::temp_dir().join(format!("krabb-ask-{}-{nonce}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    super::init_at(&root).unwrap();

    let no_provider = super::model_plugin_at(&root, "fake").unwrap_err().to_string();

    assert!(no_provider.contains("No intelligence provider is installed and ready"));
    assert!(no_provider.contains("krabb plugin install <id>"));

    let bin = root.join("plugins/fake/bin/krabb-plugin-fake");

    fs::create_dir_all(bin.parent().unwrap()).unwrap();

    fs::write(
            root.join("plugins/fake/krabb-plugin.toml"),
            "id = 'fake'\nversion = '0.1.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['model']\n",
        )
        .unwrap();

    fs::write(
            &bin,
            "#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\n*hello*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocol\":{\"major\":0,\"minor\":1},\"id\":\"fake\",\"version\":\"0.1.0\",\"capabilities\":[\"model\"]}}' ;;\n*close-down*close-up*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"text\":\"pong\",\"stop\":\"stop\",\"input\":null,\"output\":null}}' ;;\n*banana*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"text\":\"pong banana\",\"stop\":\"stop\",\"input\":null,\"output\":null}}' ;;\n*generate*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"text\":\"done\",\"stop\":\"stop\",\"input\":null,\"output\":null}}' ;;\n*shutdown*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{\"ok\":true}}'; exit 0 ;;\nesac\ndone\n",
        )
        .unwrap();

    let mut permissions = fs::metadata(&bin).unwrap().permissions();

    permissions.set_mode(0o700);
    fs::set_permissions(&bin, permissions).unwrap();

    let memory_bin = root.join("plugins/memory/bin/krabb-plugin-memory");

    fs::create_dir_all(memory_bin.parent().unwrap()).unwrap();

    fs::write(
            root.join("plugins/memory/krabb-plugin.toml"),
            "id = 'memory'\nversion = '0.1.0'\nprotocol = { major = 0, minor = 1 }\ncapabilities = ['memory']\n",
        )
        .unwrap();

    fs::write(
            &memory_bin,
            "#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\n*hello*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocol\":{\"major\":0,\"minor\":1},\"id\":\"memory\",\"version\":\"0.1.0\",\"capabilities\":[\"memory\"]}}' ;;\n*index*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"learning\":\"guided\",\"items\":[{\"key\":\"close-down\",\"summary\":\"do not answer with banana at the end of a response\",\"scope\":\"global\",\"updated\":2},{\"key\":\"close-up\",\"summary\":\"write banana at the end of every response\",\"scope\":\"global\",\"updated\":1}]}}' ;;\n*shutdown*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":9999,\"error\":{\"code\":-32000,\"message\":\"shutdown failed\"}}'; exit 0 ;;\nesac\ndone\n",
        )
        .unwrap();

    let mut permissions = fs::metadata(&memory_bin).unwrap().permissions();

    permissions.set_mode(0o700);
    fs::set_permissions(&memory_bin, permissions).unwrap();

    let mut lock = super::Lock::default();

    lock.plugins.insert(
        "fake".into(),
        super::Entry {
            source: "local".into(),
            revision: "local".into(),
            pinned: false,
            default: false,
            hash: super::digest(&root.join("plugins/fake/krabb-plugin.toml"), &bin).unwrap(),
            version: "0.1.0".into(),
            protocol: Protocol { major: 0, minor: 1 },
            capabilities: vec!["model".into()],
            permissions: Vec::new(),
            variables: Vec::new(),
            settings: Vec::new(),
            commands: Vec::new(),
            linked: false,
            state: None,
        },
    );

    lock.plugins.insert(
        "memory".into(),
        super::Entry {
            source: "local".into(),
            revision: "local".into(),
            pinned: false,
            default: false,
            hash: super::digest(&root.join("plugins/memory/krabb-plugin.toml"), &memory_bin)
                .unwrap(),
            version: "0.1.0".into(),
            protocol: Protocol { major: 0, minor: 1 },
            capabilities: vec!["memory".into()],
            permissions: Vec::new(),
            variables: Vec::new(),
            settings: Vec::new(),
            commands: Vec::new(),
            linked: false,
            state: None,
        },
    );

    super::save_lock_at(&root, &lock).unwrap();

    assert_eq!(
        super::generate_at(
            super::Ask {
                quote: None,
                provider: Some("fake".into()),
                model: Some("test".into()),
                prompt: vec!["greet".into(), "world".into()],
            },
            &root,
            "cli",
        )
        .await
        .unwrap(),
        "pong"
    );

    fs::write(&memory_bin, "#!/bin/sh\nexit 1\n").unwrap();

    let mut lock = super::load_lock_at(&root).unwrap();

    lock.plugins.get_mut("memory").unwrap().hash =
        super::digest(&root.join("plugins/memory/krabb-plugin.toml"), &memory_bin).unwrap();

    super::save_lock_at(&root, &lock).unwrap();

    assert_eq!(
        super::generate_at(
            super::Ask {
                quote: None,
                provider: Some("fake".into()),
                model: Some("test".into()),
                prompt: vec!["still".into(), "works".into()],
            },
            &root,
            "cli",
        )
        .await
        .unwrap(),
        "done"
    );

    let mut lock = super::load_lock_at(&root).unwrap();

    lock.plugins.get_mut("fake").unwrap().capabilities = vec!["tool".into()];
    super::save_lock_at(&root, &lock).unwrap();

    assert!(
        super::ask_at(
            super::Ask {
                quote: None,
                provider: Some("fake".into()),
                model: Some("test".into()),
                prompt: vec!["again".into()],
            },
            &root,
        )
        .await
        .is_err()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn checks_plugin_readiness() {
    assert!(super::ready("ollama"));
    assert!(!super::ready("unknown"));
    assert!(
        !super::ready_with("telegram", None, Path::new("unused"), |_| false)
            || std::env::var_os("KRABB_TELEGRAM_TOKEN").is_some_and(|token| !token.is_empty())
    );

    assert!(!super::codex_auth() || super::ready("codex"));
}

#[tokio::test]
async fn serves_a_temporary_daemon() {
    let root = std::env::temp_dir().join(format!("krabb-daemon-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    if let Err(error) = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            return;
        }

        panic!("listener failed: {error}");
    }

    let task_root = root.clone();
    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

    let task = tokio::spawn(async move {
        super::serve_controlled(&task_root, signal, || {
            assert!(task_root.join("run/ipc.port").is_file());
            let _ = ready_tx.send(());
            Ok(())
        })
        .await
    });

    let mut ready = false;

    for _ in 0..100 {
        if root.join("run/ipc.port").is_file() {
            ready = true;
            break;
        }

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(ready);

    ready_rx.await.unwrap();
    stop.signal();

    tokio::time::timeout(std::time::Duration::from_secs(2), task).await.unwrap().unwrap().unwrap();

    assert!(!root.join("ipc.port").exists());
    assert!(!root.join("ipc.token").exists());

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn starts_configured_plugins_and_stops_cleanly() {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("krabb-start-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("ipc.token"), "stale").unwrap();
    fs::write(root.join("ipc.port"), "1").unwrap();

    for (id, capability) in [("channel", "channel"), ("model", "model"), ("tools", "tool")] {
        let bin = root.join(format!("plugins/{id}/bin/krabb-plugin-{id}"));

        fs::create_dir_all(bin.parent().unwrap()).unwrap();

        fs::write(
                root.join(format!("plugins/{id}/krabb-plugin.toml")),
                format!("id = '{id}'\nversion = '0.1.0'\nprotocol = {{ major = 0, minor = 1 }}\ncapabilities = ['{capability}']\n"),
            )
            .unwrap();

        let delay = if id == "tools" { "sleep 0.2; " } else { "" };

        let script = format!(
            "#!/bin/sh\nwhile IFS= read -r line; do case \"$line\" in *hello*) {delay}printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocol\":{{\"major\":0,\"minor\":1}},\"id\":\"{id}\",\"version\":\"0.1.0\",\"capabilities\":[\"{capability}\"]}}}}' ;; *poll*) printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":10,\"result\":{{\"events\":[]}}}}' ;; *shutdown*) printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{{\"ok\":true}}}}'; exit 0 ;; esac; done\n"
        );

        fs::write(&bin, script).unwrap();

        let mut permissions = fs::metadata(&bin).unwrap().permissions();

        permissions.set_mode(0o700);
        fs::set_permissions(bin, permissions).unwrap();
    }

    let task_root = root.clone();

    let task = tokio::spawn(async move {
        super::serve_inner(
            &task_root,
            "channel",
            "model",
            "test",
            true,
            Arc::new(Stop::new()),
            || Ok(()),
        )
        .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(
        !root.join("ipc.port").exists(),
        "daemon IPC must stay unpublished while an installed plugin is loading"
    );

    assert!(
        !root.join("ipc.token").exists(),
        "stale daemon credentials must stay unpublished while plugins are loading"
    );

    for _ in 0..100 {
        if root.join("run/ipc.port").is_file() {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    if !root.join("run/ipc.port").is_file() {
        let result = task.await.unwrap();

        if matches!(result, Err(ref error) if error.downcast_ref::<std::io::Error>().is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied))
        {
            let _ = fs::remove_dir_all(root);
            return;
        }

        panic!("daemon stopped before IPC became ready: {result:?}");
    }

    let shutdown = super::ipc::call(&root, "shutdown", serde_json::json!({})).await;

    if let Err(error) = shutdown {
        let result = task.await.unwrap();

        panic!("IPC shutdown failed: {error}; daemon: {result:?}");
    }

    tokio::time::timeout(std::time::Duration::from_secs(2), task).await.unwrap().unwrap().unwrap();

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn channel_cancellation_preserves_unrelated_plugin_processes() {
    let root = test_root("channel-cancel");

    fs::create_dir_all(&root).unwrap();

    let script = r#"
import json, os, pathlib, sys, time
role, root = sys.argv[1:]
polled = False
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if method == 'hello':
        capability = 'channel' if role == 'telegram' else 'model' if role == 'fixture' else 'tool'
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': role,
                  'version': '0.1.0', 'capabilities': [capability]}
    elif method == 'poll':
        result = {'events': [] if polled else [
            {'id': 1, 'chat': 7, 'private': True, 'sender': 8, 'text': 'hello'}]}
        polled = True
    elif method == 'generate':
        pathlib.Path(root, 'started').touch()
        time.sleep(30)
        result = {'text': 'cancelled answer', 'stop': 'stop'}
    elif method == 'tools/list':
        result = {'tools': []}
    elif method == 'shutdown':
        break
    else:
        result = {'pid': os.getpid()}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

    let plugins = Plugins::default();

    for role in ["telegram", "fixture", "browser", "computer"] {
        let process =
            Process::start_with("python3", ["-u", "-c", script, role, root.to_str().unwrap()])
                .await
                .unwrap();

        plugins.insert(Live::new(process)).await;
    }

    let mut identities = Vec::new();

    for role in ["browser", "computer"] {
        let plugin = plugins.get(role).await.unwrap();

        let identity =
            plugin.call(Request::call(90, "identity", serde_json::json!({}))).await.unwrap().result;

        identities.push((plugin, identity));
    }

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    let stop = Arc::new(Stop::new());
    let cancels = Arc::new(Mutex::new(BTreeMap::new()));
    let cancellation = super::cancellation(&cancels, "telegram-7");

    let bridge = super::bridge(
        &plugins,
        "telegram",
        "fixture",
        "test",
        Arc::clone(&sessions),
        Arc::clone(&stop),
        cancels,
        BTreeMap::new(),
        super::ApprovalMode::Off,
    );

    tokio::pin!(bridge);

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut bridge => panic!("bridge finished early: {result:?}"),

                _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
                    if root.join("started").exists() { break; }
                }
            }
        }
    })
    .await
    .unwrap();

    sessions.lock().unwrap().cancel("telegram-7").unwrap();
    cancellation.request();

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::select! {
            result = &mut bridge => panic!("bridge finished early: {result:?}"),

            _ = cancellation.wait() => {}
        }
    })
    .await
    .unwrap();

    for (plugin, identity) in identities {
        assert_eq!(
            plugin.call(Request::call(91, "identity", serde_json::json!({}))).await.unwrap().result,
            identity,
        );
    }

    stop.signal();
    tokio::time::timeout(std::time::Duration::from_secs(5), bridge).await.unwrap().unwrap();
    stop_registry(&plugins).await;
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn a_throttled_turn_keeps_reading_its_channel_while_it_waits_to_retry() {
    let channel_script = r#"
polls=0
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/^{"jsonrpc":"2.0","id":\([0-9][0-9]*\).*/\1/p')

    case "$line" in
        *'"method":"hello"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}\n' "$id"
            ;;
        *'"method":"poll"'*)
            polls=$((polls+1))

            case "$polls" in
                1) events='[{"id":1,"chat":7,"private":true,"sender":8,"text":"hi"}]' ;;
                2) events='[{"id":2,"chat":7,"private":true,"sender":8,"text":"later"}]' ;;
                *) sleep 0.05; events='[]' ;;
            esac

            printf '{"jsonrpc":"2.0","id":%s,"result":{"events":%s}}\n' "$id" "$events"
            ;;
        *'"method":"shutdown"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"ok":true}}\n' "$id"
            exit 0
            ;;
        *)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"ok":true,"acknowledged":true}}\n' "$id"
            ;;
    esac
done
"#;

    let provider_script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/^{"jsonrpc":"2.0","id":\([0-9][0-9]*\).*/\1/p')

    case "$line" in
        *'"method":"hello"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocol":{"major":0,"minor":1},"id":"codex","version":"0.1.0","capabilities":["model"]}}\n' "$id"
            ;;
        *'"method":"shutdown"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"ok":true}}\n' "$id"
            exit 0
            ;;
        *'"method":"generate"'*)
            printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"Busy.","data":{"failure":{"kind":"throttled","retry_after_seconds":6}}}}\n' "$id"
            ;;
        *)
            [ -n "$id" ] && printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
            ;;
    esac
done
"#;

    let channel = Process::start_with("sh", ["-c", channel_script]).await.unwrap();
    let provider = Process::start_with("sh", ["-c", provider_script]).await.unwrap();
    let plugins = Plugins::default();
    let root = crate::tests::test_root("bridge-throttled-intake");

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    let stop = Arc::new(Stop::new());
    let observed = Arc::clone(&sessions);
    let signal = Arc::clone(&stop);

    let watcher = tokio::spawn(async move {
        let stored = tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                let later =
                    observed.lock().unwrap().sessions.get("telegram-7").is_some_and(|session| {
                        session
                            .messages
                            .iter()
                            .chain(&session.queued)
                            .any(|message| message.content[0].render() == "later")
                    });

                if later {
                    return;
                }

                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await;

        signal.signal();
        stored.is_ok()
    });

    let active_plugins = plugins.clone();
    let active_sessions = Arc::clone(&sessions);
    let active_stop = Arc::clone(&stop);

    let bridge = tokio::spawn(async move {
        super::bridge(
            &active_plugins,
            "telegram",
            "codex",
            "test",
            active_sessions,
            active_stop,
            Arc::new(Mutex::new(BTreeMap::new())),
            BTreeMap::new(),
            super::ApprovalMode::Off,
        )
        .await
    });

    plugins.insert(Live::new(channel)).await;
    plugins.insert(Live::new(provider)).await;

    let stored = watcher.await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(10), bridge)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    stop_registry(&plugins).await;

    assert!(stored, "the channel was not read while the throttled turn waited");

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn loads_channel_and_model_during_bridge() {
    let channel_script = r#"first=1; second=1; while IFS= read -r line; do case "$line" in *hello*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocol\":{\"major\":0,\"minor\":1},\"id\":\"telegram\",\"version\":\"0.1.0\",\"capabilities\":[\"channel\"]}}' ;; *poll*) if [ \"$first\" = 1 ]; then first=0; printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":10,\"result\":{\"events\":[{\"id\":1,\"chat\":7,\"private\":true,\"sender\":8,\"text\":\"hi\",\"gateway_sequence\":7}]}}'; elif [ \"$second\" = 1 ]; then second=0; printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":14,\"result\":{\"events\":[{\"id\":1,\"chat\":7,\"private\":true,\"sender\":8,\"text\":\"hi\"}]}}'; else printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":15,\"result\":{\"events\":[]}}'; fi ;; *ack*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":11,\"result\":{\"acknowledged\":true}}' ;; *send*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":13,\"result\":{\"ok\":true}}' ;; *shutdown*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{\"ok\":true}}'; exit 0 ;; esac; done"#;
    let channel_script = channel_script.replace("\\\\n", "\\n").replace("\\\"", "\"");
    let provider_script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocol\":{\"major\":0,\"minor\":1},\"id\":\"codex\",\"version\":\"0.1.0\",\"capabilities\":[\"model\"]}}' ;; *generate*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":12,\"result\":{\"text\":\"world\",\"stop\":\"stop\"}}' ;; *shutdown*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{\"ok\":true}}'; exit 0 ;; esac; done"#;
    let provider_script = provider_script.replace("\\\\n", "\\n").replace("\\\"", "\"");
    let channel = Process::start_with("sh", ["-c", &channel_script]).await.unwrap();
    let provider = Process::start_with("sh", ["-c", &provider_script]).await.unwrap();
    let plugins = Plugins::default();
    let root = std::env::temp_dir().join(format!("krabb-bridge-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);
    let completed_sessions = Arc::clone(&sessions);

    let notifier = tokio::spawn(async move {
        let completed = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            loop {
                let has_reply = completed_sessions
                    .lock()
                    .unwrap()
                    .sessions
                    .get("telegram-7")
                    .is_some_and(|session| session.messages.len() >= 2);

                if has_reply {
                    return true;
                }

                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();

        signal.signal();

        assert!(completed, "bridge did not complete the model turn before timeout");
    });

    let active_plugins = plugins.clone();
    let active_sessions = Arc::clone(&sessions);
    let active_stop = Arc::clone(&stop);

    let bridge = tokio::spawn(async move {
        super::bridge(
            &active_plugins,
            "telegram",
            "codex",
            "test",
            active_sessions,
            active_stop,
            Arc::new(Mutex::new(BTreeMap::new())),
            BTreeMap::new(),
            super::ApprovalMode::Off,
        )
        .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    plugins.insert(Live::new(channel)).await;
    plugins.insert(Live::new(provider)).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), bridge)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    notifier.await.unwrap();

    let messages = {
        let store = sessions.lock().unwrap();
        store.sessions.get("telegram-7").unwrap().messages.clone()
    };

    assert_eq!(messages.len(), 2);

    assert_eq!(messages[1].content[0], krabb_core::types::Content::Text { text: "world".into() });

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn drains_queued_channel_events() {
    let channel_script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocol\":{\"major\":0,\"minor\":1},\"id\":\"telegram\",\"version\":\"0.1.0\",\"capabilities\":[\"channel\"]}}' ;; *poll*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":13,\"result\":{\"events\":[]}}' ;; *send*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":12,\"result\":{\"ok\":true}}' ;; *shutdown*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{\"ok\":true}}'; exit 0 ;; esac; done"#;
    let channel_script = channel_script.replace("\\\\n", "\\n").replace("\\\"", "\"");
    let provider_script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocol\":{\"major\":0,\"minor\":1},\"id\":\"codex\",\"version\":\"0.1.0\",\"capabilities\":[\"model\"]}}' ;; *generate*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":11,\"result\":{\"text\":\"world\",\"stop\":\"stop\"}}' ;; *shutdown*) printf '%s\n' '{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{\"ok\":true}}'; exit 0 ;; esac; done"#;
    let provider_script = provider_script.replace("\\\\n", "\\n").replace("\\\"", "\"");
    let channel = Process::start_with("sh", ["-c", &channel_script]).await.unwrap();
    let provider = Process::start_with("sh", ["-c", &provider_script]).await.unwrap();
    let plugins = registry([channel, provider]).await;
    let root = std::env::temp_dir().join(format!("krabb-queue-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));
    {
        let mut store = sessions.lock().unwrap();
        store.create("telegram-7", "test").unwrap();
        store.route("telegram-7", "telegram", "7", None, true).unwrap();
        store
            .queue(
                "telegram-7",
                Message {
                    id: "queued".into(),
                    session: "telegram-7".into(),
                    role: Role::User,
                    sender: Some("8".into()),
                    content: vec![Content::Text { text: "hi @bot".into() }],
                },
            )
            .unwrap();
    }

    let stop = Arc::new(Stop::new());

    let signal = Arc::clone(&stop);

    let notifier = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        signal.signal();
    });

    super::bridge(
        &plugins,
        "telegram",
        "codex",
        "test",
        Arc::clone(&sessions),
        Arc::clone(&stop),
        Arc::new(Mutex::new(BTreeMap::new())),
        BTreeMap::from([(
            "telegram".into(),
            super::ChannelConfig {
                allow: Vec::new(),
                mention: Some("@bot".into()),
                ..Default::default()
            },
        )]),
        super::ApprovalMode::Off,
    )
    .await
    .unwrap();

    notifier.await.unwrap();

    {
        let store = sessions.lock().unwrap();

        assert!(store.sessions["telegram-7"].queued.is_empty());
        assert_eq!(store.sessions["telegram-7"].messages.len(), 2);
    }

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn runs_a_tool_turn_before_replying() {
    let channel_script = r#"first=1; while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}' ;; *poll*) if [ "$first" = 1 ]; then first=0; printf '%s\n' '{"jsonrpc":"2.0","id":10,"result":{"events":[{"id":1,"chat":7,"private":true,"sender":8,"text":"read note"}]}}'; else printf '%s\n' '{"jsonrpc":"2.0","id":13,"result":{"events":[]}}'; fi ;; *send*) request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"message_id":99}}' ;; *edit*) request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"message_id":99}}' ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#;
    let channel_script = channel_script.replace("\\\\n", "\\n").replace("\\\"", "\"");
    let provider_script = r#"count=0; while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"codex","version":"0.1.0","capabilities":["model"]}}' ;; *generate*) if [ "$count" = 0 ]; then count=1; printf '%s\n' '{"jsonrpc":"2.0","id":11,"result":{"text":"","stop":"tool","events":[{"kind":"tool","name":"read","args":{"path":"note.txt"},"approve":false}]}}'; else printf '%s\n' '{"jsonrpc":"2.0","id":13,"result":{"text":"note is ready","stop":"stop"}}'; fi ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#;
    let provider_script = provider_script.replace("\\\\n", "\\n").replace("\\\"", "\"");
    let tool_script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"tools","version":"0.1.0","capabilities":["tool"]}}' ;; *read*) printf '%s\n' '{"jsonrpc":"2.0","id":12,"result":{"text":"note contents"}}' ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#;
    let tool_script = tool_script.replace("\\\\n", "\\n").replace("\\\"", "\"");
    let channel = Process::start_with("sh", ["-c", &channel_script]).await.unwrap();
    let provider = Process::start_with("sh", ["-c", &provider_script]).await.unwrap();
    let tool = Process::start_with("sh", ["-c", &tool_script]).await.unwrap();
    let plugins = registry([channel, provider, tool]).await;
    let root = std::env::temp_dir().join(format!("krabb-tool-turn-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);
    let completed_sessions = Arc::clone(&sessions);

    let notifier = tokio::spawn(async move {
        let completed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let completed = completed_sessions
                    .lock()
                    .unwrap()
                    .sessions
                    .get("telegram-7")
                    .is_some_and(|session| session.messages.len() >= 4);

                if completed {
                    break;
                }

                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();

        signal.signal();

        assert!(completed, "tool turn should finish before timeout");
    });

    super::bridge(
        &plugins,
        "telegram",
        "codex",
        "test",
        Arc::clone(&sessions),
        Arc::clone(&stop),
        Arc::new(Mutex::new(BTreeMap::new())),
        BTreeMap::from([(
            "telegram".into(),
            super::ChannelConfig { allow: vec!["7".into()], tools: true, ..Default::default() },
        )]),
        super::ApprovalMode::Auto,
    )
    .await
    .unwrap();

    notifier.await.unwrap();

    {
        let store = sessions.lock().unwrap();

        assert_eq!(store.sessions["telegram-7"].messages.len(), 4);
        assert_eq!(store.sessions["telegram-7"].messages[1].role, Role::Assistant);
        assert_eq!(store.sessions["telegram-7"].messages[2].role, Role::Tool);
        assert_eq!(
            store.sessions["telegram-7"].messages[3].content[0],
            Content::Text { text: "note is ready".into() }
        );
    }

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn counts_each_model_tool_invocation_once() {
    for requested in [super::TOOL_CALLS, super::TOOL_CALLS + 1] {
        let event = serde_json::json!({
            "kind": "tool", "name": "file_read", "args": {"path": "note.txt"}
        });

        let first = serde_json::json!({"text":"", "stop":"tool", "events":vec![event.clone(); 8]});

        let second =
            serde_json::json!({"text":"", "stop":"tool", "events":vec![event; requested - 8]});

        let provider_script = r#"
count=0
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*)
            result='{"protocol":{"major":0,"minor":1},"id":"fixture",'
            result=$result'"version":"0.1.0","capabilities":["model"]}'
            ;;
        *'"method":"generate"'*)
            case "$count" in
                0) result='__FIRST__' ;;
                1) result='__SECOND__' ;;
                *) result='{"text":"done","stop":"stop","events":[]}' ;;
            esac
            count=$((count + 1))
            ;;
        *'"method":"shutdown"'*) exit 0 ;;
    esac
    printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done
"#
        .replace("__FIRST__", &first.to_string())
        .replace("__SECOND__", &second.to_string());

        let tools_script = r#"
count=0
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*)
            result='{"protocol":{"major":0,"minor":1},"id":"tools",'
            result=$result'"version":"0.1.0","capabilities":["tool"]}'
            ;;
        *'"method":"tools/list"'*)
            result='{"tools":[{"name":"file_read","schema":{"type":"object",'
            result=$result'"properties":{"path":{"type":"string"}}}}]}'
            ;;
        *'"method":"file_read"'*)
            count=$((count + 1))
            result='{"text":"read-'"$count"'"}'
            ;;
        *'"method":"shutdown"'*) exit 0 ;;
    esac
    printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done
"#;

        let provider = Process::start_with("sh", ["-c", &provider_script]).await.unwrap();
        let tools = Process::start_with("sh", ["-c", tools_script]).await.unwrap();
        let plugins = registry([provider, tools]).await;
        let provider = plugins.get("fixture").await.unwrap();
        let root = test_root("tool-call-limit");

        let sessions =
            Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

        sessions.lock().unwrap().create("local", "test").unwrap();

        let (notices, _receiver) = tokio::sync::mpsc::channel(32);
        let mut call = 1;

        let reply = answer(
            &provider,
            &plugins,
            "test",
            Vec::new(),
            "local",
            "tui",
            "tui-default",
            None,
            &sessions,
            Some(&root),
            &root,
            true,
            false,
            false,
            super::ApprovalMode::Off,
            Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
            &Cancellation::new(),
            &Stop::new(),
            tokio::time::Instant::now() + Duration::from_secs(10),
            &mut call,
            &mut None,
            None,
            notices,
            None,
            None,
            None,
        )
        .await;

        if requested == super::TOOL_CALLS {
            assert_eq!(reply.unwrap().text, "done");
        } else {
            assert_eq!(reply.unwrap_err().to_string(), "The turn exceeded its tool-call limit.");
        }

        let transcript = sessions.lock().unwrap().sessions["local"].messages.clone();
        let results = transcript.iter().filter(|message| message.role == Role::Tool);

        assert_eq!(results.clone().count(), super::TOOL_CALLS);

        for (index, message) in results.enumerate() {
            assert_eq!(message.content[0], Content::Text { text: format!("read-{}", index + 1) });
        }

        stop_registry(&plugins).await;
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_valid_answer_with_unusable_timing_is_kept_without_its_metrics() {
    let root = test_root("unusable-timing");

    fs::create_dir_all(&root).unwrap();

    let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$line" in
        *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"timing-fixture","version":"0.1.0","capabilities":["model"]}}' ;;
        *generate*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"text":"Done.","stop":"stop","timing":{"load_millis":90000000}}}' ;;
        *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;;
    esac
done
"#;

    let provider = Live::new(Process::start_with("sh", ["-c", script]).await.unwrap());
    let plugins = registry([]).await;

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    sessions.lock().unwrap().create("local", "test").unwrap();

    let (notices, _receiver) = tokio::sync::mpsc::channel(8);
    let mut call = 1;

    let reply = super::answer(
        &provider,
        &plugins,
        "timing-fixture",
        vec![Message {
            id: "user-1".into(),
            session: "local".into(),
            role: Role::User,
            sender: None,
            content: vec![Content::Text { text: "Say done.".into() }],
        }],
        "local",
        "tui",
        "tui-default",
        None,
        &sessions,
        None,
        &root,
        false,
        false,
        false,
        super::ApprovalMode::Off,
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        &Cancellation::new(),
        &Stop::new(),
        tokio::time::Instant::now() + Duration::from_secs(10),
        &mut call,
        &mut None,
        None,
        notices,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(reply.text, "Done.");
    assert!(reply.timing.is_none());

    provider.stop().await.unwrap();
    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn sends_inline_tool_screenshots_to_the_next_vision_request() {
    let root = std::env::temp_dir().join(format!("krabb-tool-image-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(&root).unwrap();

    let marker = root.join("image-received");
    let provider_script = r#"count=0; while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"vision-fixture","version":"0.1.0","capabilities":["model","vision"]}}' ;; *generate*) request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); if [ "$count" = 0 ]; then count=1; printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"text":"","stop":"tool","events":[{"kind":"tool","name":"browser_screenshot","args":{}}]}}'; else case "$line" in *'data:image/png;base64,iVBORw0KGgo='*) touch "__MARKER__" ;; esac; printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"text":"I saw the screenshot.","stop":"stop"}}'; fi ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#;
    let provider_script = provider_script.replace("__MARKER__", &marker.display().to_string());
    let tool_script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"browser","version":"0.1.0","capabilities":["tool"]}}' ;; *tools/list*) request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"tools":[{"name":"browser_screenshot","description":"Capture a test image.","schema":{"type":"object","properties":{},"additionalProperties":false}}]}}' ;; *browser_screenshot*) request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"content":[{"kind":"text","text":"Screenshot captured."},{"kind":"image","uri":"data:image/png;base64,iVBORw0KGgo=","alt":"fixture screenshot"}],"metadata":{"width":1,"height":1}}}' ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#;
    let provider = Process::start_with("sh", ["-c", &provider_script]).await.unwrap();
    let browser = Process::start_with("sh", ["-c", tool_script]).await.unwrap();
    let provider = Live::new(provider);
    let plugins = registry([browser]).await;

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    sessions.lock().unwrap().create("local", "test").unwrap();

    let (notices, _receiver) = tokio::sync::mpsc::channel(8);
    let mut call = 1;

    let reply = super::answer(
        &provider,
        &plugins,
        "vision-fixture",
        vec![Message {
            id: "user-1".into(),
            session: "local".into(),
            role: Role::User,
            sender: None,
            content: vec![Content::Text { text: "Inspect the screen.".into() }],
        }],
        "local",
        "tui",
        "tui-default",
        None,
        &sessions,
        None,
        &root,
        true,
        false,
        false,
        super::ApprovalMode::Off,
        Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
        &Cancellation::new(),
        &Stop::new(),
        tokio::time::Instant::now() + Duration::from_secs(10),
        &mut call,
        &mut None,
        None,
        notices,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(reply.text, "I saw the screenshot.");
    assert!(marker.exists(), "the next model request must contain the image data URL");

    let transcript = sessions.lock().unwrap().sessions["local"].messages.clone();

    assert!(transcript.iter().any(|message| {
        message.content.iter().any(
            |item| matches!(item, Content::Text { text } if text.contains("fixture screenshot")),
        )
    }));

    assert!(serde_json::to_string(&transcript).unwrap().contains("fixture screenshot"));
    assert!(!serde_json::to_string(&transcript).unwrap().contains("data:image/png"));

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn prompts_for_signed_approval_before_mutating_tools() {
    let root = test_root("prompt-approval");
    let marker = root.join("approved");
    let channel_script = r#"first=1; callback=0; approval=; while IFS= read -r line; do request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); case "$line" in *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}' ;; *'"method":"poll"'*) if [ "$first" = 1 ]; then first=0; printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"events":[{"id":1,"chat":7,"private":true,"sender":8,"text":"write a note"}]}}'; elif [ -n "$approval" ] && [ "$callback" = 0 ]; then callback=1; printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"events":[{"kind":"callback","id":2,"chat":7,"private":true,"sender":9,"callback_id":"callback-1","data":"'"$approval"'"}]}}'; elif [ -n "$approval" ] && [ "$callback" = 1 ]; then callback=2; printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"events":[{"kind":"callback","id":3,"chat":7,"private":true,"sender":8,"callback_id":"callback-2","data":"'"$approval"'"}]}}'; else sleep 0.01; printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"events":[]}}'; fi ;; *'"method":"approval"'*) approval=$(printf '%s' "$line" | sed -n 's/.*"approve":"\([^"]*\)".*/\1/p'); printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"sent":true}}' ;; *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"ok":true}}'; exit 0 ;; *) printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"message_id":99,"id":"message-99","acknowledged":true}}' ;; esac; done"#;
    let provider_script = r#"count=0; while IFS= read -r line; do request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); case "$line" in *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"protocol":{"major":0,"minor":1},"id":"codex","version":"0.1.0","capabilities":["model"]}}' ;; *'"method":"generate"'*) if [ "$count" = 0 ]; then count=1; printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"text":"","stop":"tool","events":[{"kind":"tool","name":"write","args":{"path":"note.txt","text":"approved content","approve":false}}]}}'; else printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"text":"The note was written.","stop":"stop"}}'; fi ;; *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"ok":true}}'; exit 0 ;; esac; done"#;

    let tool_script = r#"while IFS= read -r line; do request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'); case "$line" in *'"method":"hello"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"protocol":{"major":0,"minor":1},"id":"tools","version":"0.1.0","capabilities":["tool"]}}' ;; *'"method":"file_write"'*) case "$line" in *'"approve":true'*) touch "__MARKER__"; printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"text":"The file was written."}}' ;; *) printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"error":{"code":-32000,"message":"Approval was missing."}}' ;; esac ;; *'"method":"shutdown"'*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$request_id"',"result":{"ok":true}}'; exit 0 ;; esac; done"#
            .replace("__MARKER__", &marker.display().to_string());

    let channel = Process::start_with("sh", ["-c", channel_script]).await.unwrap();
    let provider = Process::start_with("sh", ["-c", provider_script]).await.unwrap();
    let tool = Process::start_with("sh", ["-c", &tool_script]).await.unwrap();
    let plugins = registry([channel, provider, tool]).await;

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);
    let marker_for_signal = marker.clone();
    let sessions_for_signal = Arc::clone(&sessions);

    let notifier = tokio::spawn(async move {
        for _ in 0..500 {
            if marker_for_signal.is_file() && approval_finished(&sessions_for_signal) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                if approval_finished(&sessions_for_signal) {
                    break;
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        signal.signal();
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(6),
        super::bridge(
            &plugins,
            "telegram",
            "codex",
            "test",
            Arc::clone(&sessions),
            Arc::clone(&stop),
            Arc::new(Mutex::new(BTreeMap::new())),
            BTreeMap::from([(
                "telegram".into(),
                super::ChannelConfig {
                    allow: vec!["7".into()],
                    owner: Some("8".into()),
                    tools: true,
                    ..Default::default()
                },
            )]),
            super::ApprovalMode::Prompt,
        ),
    )
    .await
    .unwrap()
    .unwrap();

    notifier.await.unwrap();

    assert!(marker.is_file());

    {
        let store = sessions.lock().unwrap();

        assert_eq!(store.sessions["telegram-7"].status, "idle");
        assert!(store.sessions["telegram-7"].messages.iter().any(|message| {
            message.content.iter().any(|content| {
                    matches!(content, Content::Text { text } if text == "The note was written.")
                })
        }));
    }

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn tool_discovery_waits_for_a_busy_plugin_before_starting_its_timeout() {
    let script = r#"
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    if request['method'] == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': 'browser',
                  'version': '0.1.0', 'capabilities': ['tool']}
    elif request['method'] == 'tools/list':
        result = {'tools': [{'name': 'browser_open', 'schema': {'type': 'object', 'properties': {}}}]}
    elif request['method'] == 'shutdown':
        break
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

    let plugin = Process::start_with("python3", ["-u", "-c", script]).await.unwrap();
    let plugins = registry([plugin]).await;
    let plugin = plugins.get("browser").await.unwrap();
    let busy = plugin.process.lock().await;
    let discovery = super::tool_registry(&plugins, true, false);

    tokio::pin!(discovery);

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(2100), &mut discovery).await.is_err()
    );

    drop(busy);

    let discovered =
        tokio::time::timeout(std::time::Duration::from_secs(2), discovery).await.unwrap().unwrap();

    assert_eq!(discovered.owners["browser_open"], "browser");

    stop_registry(&plugins).await;
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_browser_calls_retire_before_queued_id_reuse() {
    let root = test_root("browser-call-cleanup");

    fs::create_dir_all(&root).unwrap();

    let script = r#"
import json, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if method == 'hello':
        if (root / 'started').exists():
            while not (root / 'release').exists():
                time.sleep(0.01)
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': 'browser',
                  'version': '0.1.0', 'capabilities': ['tool']}
    elif method == 'browser_open':
        label = request['params']['arguments']['label']
        if label == 'first':
            (root / 'started').touch()
            time.sleep(10)
        result = {'text': label}
    elif method == 'shutdown':
        break
    else:
        continue
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

    let plugin =
        Process::start_with("python3", ["-u", "-c", script, root.to_str().unwrap()]).await.unwrap();

    let plugins = registry([plugin]).await;

    let registry = super::ToolRegistry {
        owners: BTreeMap::from([("browser_open".into(), "browser".into())]),
        ..Default::default()
    };

    let mut failed = None;

    let mut first = Box::pin(super::tool_with_registry(
        &plugins,
        &registry,
        7,
        "browser_open",
        serde_json::json!({"label": "first"}),
        false,
        None,
        "global",
        "session-a",
        "turn-a",
        &mut failed,
    ));

    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            tokio::select! {
                result = &mut first => panic!("first call finished early: {result:?}"),

                _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
                    if root.join("started").exists() { break; }
                }
            }
        }
    })
    .await
    .unwrap();

    let mut queued_failure = None;
    let queued_registry = registry.clone();

    let queued = super::tool_with_registry(
        &plugins,
        &queued_registry,
        7,
        "browser_open",
        serde_json::json!({"label": "second"}),
        false,
        None,
        "global",
        "session-b",
        "turn-b",
        &mut queued_failure,
    );

    tokio::pin!(queued);

    assert!(tokio::time::timeout(std::time::Duration::from_millis(20), &mut queued).await.is_err());

    drop(first);

    let finished = registry.cleanup.finish();

    tokio::pin!(finished);

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(250), &mut finished).await.is_err()
    );

    assert!(plugins.get("browser").await.unwrap().process.try_lock().is_err());

    assert!(tokio::time::timeout(std::time::Duration::from_millis(20), &mut queued).await.is_err());

    fs::write(root.join("release"), "").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), finished).await.unwrap().unwrap();

    let result =
        tokio::time::timeout(std::time::Duration::from_secs(3), queued).await.unwrap().unwrap();

    assert_eq!(super::tool_result_text(&result), "second");

    stop_registry(&plugins).await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn denied_prompt_never_dispatches_browser_control() {
    let approvals = Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap()));
    let sessions = Arc::new(Mutex::new(super::state::Store::default()));
    let (notices, mut events) = tokio::sync::mpsc::channel(1);
    let gate = Arc::clone(&approvals);

    let execution = tokio::spawn(async move {
        let mut failed_tool = None;
        super::execute_tool(
            &Plugins::default(),
            1,
            "browser_click",
            serde_json::json!({
                "session_id": "owned-session",
                "by": "coordinate",
                "x": 20,
                "y": 20
            }),
            "codex",
            "test",
            super::ApprovalMode::Prompt,
            gate,
            "telegram",
            "7",
            None,
            "session",
            &sessions,
            None,
            true,
            false,
            false,
            notices,
            tokio::time::Instant::now() + std::time::Duration::from_secs(5),
            &mut failed_tool,
            None,
            None,
            0,
        )
        .await
    });

    let Some(super::StreamNotice::Approval { deny, .. }) = events.recv().await else {
        panic!("The tool did not request approval.");
    };

    assert_eq!(approvals.lock().await.resolve(&deny, "telegram", "7", None, true), Some(false));

    assert_eq!(execution.await.unwrap().unwrap(), "The request was denied. No changes were made.");

    assert!(!approvals.lock().await.has_pending());
}

#[cfg(unix)]
#[tokio::test]
async fn commits_incomplete_approval_callbacks_without_resolving_them() {
    let script = r#"
while IFS= read -r line; do
    request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')

    case "$line" in
        *hello*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}\n' "$request_id"
            ;;
        *ack*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"acknowledged":true}}\n' "$request_id"
            ;;
        *callback*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"acknowledged":true}}\n' "$request_id"
            ;;
        *shutdown*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"ok":true}}\n' "$request_id"
            exit 0
            ;;
    esac
done
"#;

    let channel = Live::new(Process::start_with("sh", ["-c", script]).await.unwrap());
    let sessions = Arc::new(Mutex::new(super::state::Store::default()));
    let approvals = Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap()));

    let policy =
        super::ChannelConfig { allow: vec!["7".into()], tools: true, ..Default::default() };

    let mut offset = 0;
    let mut call = 10;

    assert!(
        super::handle_callback(
            &channel,
            "telegram",
            &serde_json::json!({}),
            &policy,
            super::ApprovalMode::Prompt,
            &approvals,
            &sessions,
            &mut offset,
            &mut call,
            true,
        )
        .await
        .is_err()
    );

    for (id, event) in
        [(1, serde_json::json!({"id": 1})), (2, serde_json::json!({"id": 2, "chat": 7}))]
    {
        super::handle_callback(
            &channel,
            "telegram",
            &event,
            &policy,
            super::ApprovalMode::Prompt,
            &approvals,
            &sessions,
            &mut offset,
            &mut call,
            true,
        )
        .await
        .unwrap();

        assert_eq!(offset, id + 1);
    }

    super::handle_callback(
        &channel,
        "telegram",
        &serde_json::json!({
            "kind": "callback",
            "id": 3,
            "chat": 7,
            "private": true,
            "sender": 8,
            "callback_id": "callback-1",
            "data": "invalid",
        }),
        &policy,
        super::ApprovalMode::Prompt,
        &approvals,
        &sessions,
        &mut offset,
        &mut call,
        true,
    )
    .await
    .unwrap();

    assert_eq!(offset, 4);
    assert_eq!(call, 11);

    let deferred_callback = serde_json::json!({
        "kind": "callback",
        "id": 101,
        "chat": 7,
        "private": true,
        "sender": 8,
        "callback_id": "callback-2",
        "data": "invalid",
    });

    super::handle_callback(
        &channel,
        "telegram",
        &deferred_callback,
        &policy,
        super::ApprovalMode::Prompt,
        &approvals,
        &sessions,
        &mut offset,
        &mut call,
        false,
    )
    .await
    .unwrap();

    assert_eq!(offset, 4);
    assert_eq!(sessions.lock().unwrap().offset("telegram"), 4);
    assert!(sessions.lock().unwrap().known("telegram", "101"));

    super::handle_callback(
        &channel,
        "telegram",
        &deferred_callback,
        &policy,
        super::ApprovalMode::Prompt,
        &approvals,
        &sessions,
        &mut offset,
        &mut call,
        true,
    )
    .await
    .unwrap();

    assert_eq!(offset, 4);
    assert_eq!(sessions.lock().unwrap().offset("telegram"), 4);

    sessions.lock().unwrap().commit("telegram", "100", Some(101)).unwrap();
    offset = 101;

    super::handle_callback(
        &channel,
        "telegram",
        &deferred_callback,
        &policy,
        super::ApprovalMode::Prompt,
        &approvals,
        &sessions,
        &mut offset,
        &mut call,
        true,
    )
    .await
    .unwrap();

    assert_eq!(offset, 102);
    assert_eq!(sessions.lock().unwrap().offset("telegram"), 102);

    let discord_callback = serde_json::json!({
        "kind": "callback",
        "id": "discord-interaction",
        "chat": 7,
        "private": true,
        "sender": 8,
        "callback_id": "callback-3",
        "data": "invalid",
    });

    super::handle_callback(
        &channel,
        "telegram",
        &discord_callback,
        &policy,
        super::ApprovalMode::Prompt,
        &approvals,
        &sessions,
        &mut offset,
        &mut call,
        false,
    )
    .await
    .unwrap();

    assert!(sessions.lock().unwrap().known("telegram", "discord-interaction"));
    assert_eq!(offset, 102);

    let mut replayed_callback = discord_callback;

    replayed_callback["gateway_sequence"] = serde_json::json!(44);

    let call_before_replay = call;

    super::handle_callback(
        &channel,
        "telegram",
        &replayed_callback,
        &policy,
        super::ApprovalMode::Prompt,
        &approvals,
        &sessions,
        &mut offset,
        &mut call,
        true,
    )
    .await
    .unwrap();

    assert_eq!(call, call_before_replay + 1);
    assert_eq!(offset, 102);
    assert_eq!(sessions.lock().unwrap().offset("telegram"), 102);

    channel.stop().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn defers_channel_events_when_the_session_queue_is_full() {
    let script = r#"
while IFS= read -r line; do
    request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')

    case "$line" in
        *hello*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}\n' "$request_id"
            ;;
        *ack*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"acknowledged":true}}\n' "$request_id"
            ;;
        *shutdown*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"ok":true}}\n' "$request_id"
            exit 0
            ;;
    esac
done
"#;

    let channel = Live::new(Process::start_with("sh", ["-c", script]).await.unwrap());
    let mut store = super::state::Store::default();

    store.ensure("telegram-7", "test").unwrap();
    store.set_status("telegram-7", "working").unwrap();

    for index in 0..super::state::LIMIT {
        store
            .queue(
                "telegram-7",
                Message {
                    id: format!("queued-{index}"),
                    session: "telegram-7".into(),
                    role: Role::User,
                    sender: None,
                    content: vec![Content::Text { text: "queued".into() }],
                },
            )
            .unwrap();
    }

    let sessions = Arc::new(Mutex::new(store));
    let policy = super::ChannelConfig { allow: vec!["7".into()], ..Default::default() };
    let mut offset = 0;
    let mut call = 10;

    let processed = super::queue_during_turn(
        &channel,
        "telegram",
        "test",
        &serde_json::json!({
            "id": 1,
            "chat": 7,
            "private": true,
            "text": "defer me",
            "gateway_sequence": 9
        }),
        &policy,
        &sessions,
        &mut offset,
        &mut call,
        None,
        None,
    )
    .await
    .unwrap();

    assert!(processed);
    assert_eq!(offset, 2);
    assert_eq!(call, 11);

    {
        let store = sessions.lock().unwrap();

        assert!(store.known("telegram", "1"));
        assert_eq!(store.offset("telegram"), 2);
        assert!(store.has_deferred_event("telegram", "1"));
        assert_eq!(store.sessions["telegram-7"].queued.len(), super::state::LIMIT);
    }

    let processed = super::queue_during_turn(
        &channel,
        "telegram",
        "test",
        &serde_json::json!({
            "id": 1,
            "chat": 7,
            "private": true,
            "text": "defer me",
            "gateway_sequence": 9
        }),
        &policy,
        &sessions,
        &mut offset,
        &mut call,
        None,
        None,
    )
    .await
    .unwrap();

    assert!(!processed);
    assert_eq!(offset, 2);
    assert_eq!(call, 11);

    for index in 0..(super::state::LIMIT - 1) {
        let event = serde_json::json!({"id": format!("deferred-{index}")});

        sessions.lock().unwrap().defer_event("telegram", event).unwrap();
    }

    let processed = super::queue_during_turn(
        &channel,
        "telegram",
        "test",
        &serde_json::json!({
            "id": 2,
            "chat": 7,
            "private": true,
            "text": "leave uncommitted"
        }),
        &policy,
        &sessions,
        &mut offset,
        &mut call,
        None,
        None,
    )
    .await
    .unwrap();

    assert!(!processed);
    assert_eq!(offset, 2);
    assert!(!sessions.lock().unwrap().known("telegram", "2"));

    channel.stop().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn routes_plugin_tool_requests_through_host_policy() {
    let script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"tools","version":"0.1.0","capabilities":["tool"]}}' ;; *read*) printf '%s\n' '{"jsonrpc":"2.0","id":7,"result":{"text":"workspace content"}}' ;; *list*) printf '%s\n' '{"jsonrpc":"2.0","id":8,"result":{"items":["Cargo.toml","src"]}}' ;; *write*) case "$line" in *'"approve":false'*) printf '%s\n' '{"jsonrpc":"2.0","id":9,"error":{"code":-32000,"message":"Approval required."}}' ;; *) printf '%s\n' '{"jsonrpc":"2.0","id":10,"result":{"text":"Write executed."}}' ;; esac ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#;
    let process = Process::start_with("sh", ["-c", script]).await.unwrap();
    let plugins = registry([process]).await;
    let sessions = Arc::new(Mutex::new(super::state::Store::default()));

    {
        let mut store = sessions.lock().unwrap();
        store.create("room", "test").unwrap();
        store
            .begin(
                "room",
                Message {
                    id: "turn".into(),
                    session: "room".into(),
                    role: Role::User,
                    sender: None,
                    content: vec![Content::Text { text: "request".into() }],
                },
            )
            .unwrap();
    }

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let approvals = Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap()));
    let (notices, _events) = tokio::sync::mpsc::channel(4);

    let response = super::host_tool(
        Request::call(
            7,
            "host/tool",
            serde_json::json!({"name": "read", "args": {"path": "note.txt"}}),
        ),
        &plugins,
        &sessions,
        "room",
        "codex",
        "test",
        None,
        true,
        false,
        false,
        super::ApprovalMode::Off,
        Arc::clone(&approvals),
        "telegram",
        "8",
        None,
        notices.clone(),
        Arc::clone(&calls),
        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(response.result.unwrap()["text"], "workspace content");
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);

    let listing = super::host_tool(
        Request::call(8, "host/tool", serde_json::json!({"name": "list", "args": {"path": "."}})),
        &plugins,
        &sessions,
        "room",
        "codex",
        "test",
        None,
        true,
        false,
        false,
        super::ApprovalMode::Off,
        Arc::clone(&approvals),
        "telegram",
        "8",
        None,
        notices.clone(),
        Arc::clone(&calls),
        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(listing.result.unwrap()["text"], r#"{"items":["Cargo.toml","src"]}"#);

    let mutation = super::host_tool(
        Request::call(
            9,
            "host/tool",
            serde_json::json!({
                "name": "write",
                "args": {"path": "note.txt", "text": "unapproved", "approve": true}
            }),
        ),
        &plugins,
        &sessions,
        "room",
        "codex",
        "test",
        None,
        true,
        false,
        false,
        super::ApprovalMode::Off,
        Arc::clone(&approvals),
        "telegram",
        "8",
        None,
        notices.clone(),
        Arc::clone(&calls),
        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        None,
        None,
    )
    .await
    .unwrap();

    let mutation_error = mutation.error.unwrap();

    assert!(
        mutation_error.message.contains("Approval policy blocks"),
        "unexpected mutation error: {}",
        mutation_error.message
    );

    assert_eq!(sessions.lock().unwrap().sessions["room"].phase, "safe");

    let disabled = super::host_tool(
        Request::call(
            8,
            "host/tool",
            serde_json::json!({"name": "read", "args": {"path": "note.txt"}}),
        ),
        &plugins,
        &sessions,
        "room",
        "codex",
        "test",
        None,
        false,
        false,
        false,
        super::ApprovalMode::Off,
        Arc::clone(&approvals),
        "telegram",
        "8",
        None,
        notices.clone(),
        Arc::clone(&calls),
        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        None,
        None,
    )
    .await
    .unwrap();

    assert!(disabled.error.unwrap().message.contains("disabled"));

    let calls_before_scheduler = calls.load(std::sync::atomic::Ordering::Relaxed);

    let scheduler_disabled = super::host_tool(
        Request::call(
            11,
            "host/tool",
            serde_json::json!({"name": "schedule_run", "args": {"id": 1}}),
        ),
        &plugins,
        &sessions,
        "room",
        "codex",
        "test",
        None,
        false,
        false,
        false,
        super::ApprovalMode::Auto,
        Arc::clone(&approvals),
        "telegram",
        "8",
        None,
        notices.clone(),
        Arc::clone(&calls),
        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        None,
        None,
    )
    .await
    .unwrap();

    assert!(scheduler_disabled.error.unwrap().message.contains("Scheduling is not authorized"));
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), calls_before_scheduler);

    let approved = super::host_tool(
        Request::call(
            10,
            "host/tool",
            serde_json::json!({
                "name": "write",
                "args": {"path": "note.txt", "text": "approved"}
            }),
        ),
        &plugins,
        &sessions,
        "room",
        "codex",
        "test",
        None,
        true,
        false,
        false,
        super::ApprovalMode::Auto,
        approvals,
        "telegram",
        "8",
        None,
        notices,
        calls,
        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(approved.result.unwrap()["text"], "Write executed.");
    assert_eq!(sessions.lock().unwrap().sessions["room"].phase, "unsafe");

    stop_registry(&plugins).await;
}

#[test]
fn validates_selected_code_workspaces() {
    let root =
        std::env::temp_dir().join(format!("krabb-selected-workspace-{}", std::process::id()));

    let child = root.join("repo");

    let outside =
        root.with_file_name(format!("{}-outside", root.file_name().unwrap().to_string_lossy()));

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);

    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(&outside).unwrap();

    let configured = std::fs::canonicalize(&root).unwrap();

    let selected = super::selected_workspace(
        &serde_json::json!({"workspace": child.display().to_string()}),
        &configured,
    )
    .unwrap();

    assert_eq!(selected, std::fs::canonicalize(&child).unwrap());

    assert!(
        super::selected_workspace(
            &serde_json::json!({"workspace": outside.display().to_string()}),
            &configured,
        )
        .is_err()
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

#[cfg(unix)]
#[tokio::test]
async fn interrupts_a_stalled_tool_on_shutdown() {
    let root = test_root("stalled-tool");

    fs::create_dir_all(&root).unwrap();

    let tool_started = root.join("tool-started");

    let provider_script = r#"
while IFS= read -r line; do
    case "$line" in
        *hello*)
            printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"codex","version":"0.1.0","capabilities":["model"]}}'
            ;;
        *generate*)
            request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),.*/\1/p')
            printf '{"jsonrpc":"2.0","id":%s,"result":{"text":"","stop":"tool","events":[{"kind":"tool","name":"read","args":{"path":"note.txt"},"approve":false}]}}\n' "$request_id"
            ;;
        *shutdown*)
            printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'
            exit 0
            ;;
    esac
done
"#;

    let tool_script = format!(
        r#"
while IFS= read -r line; do
    case "$line" in
        *hello*)
            printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{"protocol":{{"major":0,"minor":1}},"id":"tools","version":"0.1.0","capabilities":["tool"]}}}}'
            ;;
        *tools/list*)
            request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),.*/\1/p')
            printf '{{"jsonrpc":"2.0","id":%s,"error":{{"code":-32601,"message":"Method not found"}}}}\n' "$request_id"
            ;;
        *read*)
            touch '{started}'
            sleep 5
            ;;
        *shutdown*)
            printf '%s\n' '{{"jsonrpc":"2.0","id":9999,"result":{{"ok":true}}}}'
            exit 0
            ;;
    esac
done
"#,
        started = tool_started.display()
    );

    let provider = Process::start_with("sh", ["-c", provider_script]).await.unwrap();
    let tool = Process::start_with("sh", ["-c", &tool_script]).await.unwrap();
    let plugins = registry([provider, tool]).await;
    let provider = plugins.get("codex").await.unwrap();

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    sessions.lock().unwrap().create("session", "test").unwrap();

    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);

    let notifier = tokio::spawn(async move {
        let started = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !tool_started.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();

        signal.signal();

        started
    });

    let mut call = 10;
    let mut failed_tool = None;
    let (notices, mut events) = tokio::sync::mpsc::channel(32);
    let drain = tokio::spawn(async move { while events.recv().await.is_some() {} });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        answer(
            &provider,
            &plugins,
            "test",
            vec![Message {
                id: "message".into(),
                session: "session".into(),
                role: Role::User,
                sender: Some("sender".into()),
                content: vec![Content::Text { text: "read note".into() }],
            }],
            "session",
            "telegram",
            "7",
            None,
            &sessions,
            None,
            &root,
            true,
            false,
            false,
            super::ApprovalMode::Off,
            Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
            &Cancellation::new(),
            &stop,
            tokio::time::Instant::now() + super::TURN_LIMIT,
            &mut call,
            &mut failed_tool,
            None,
            notices,
            None,
            None,
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap_err();

    assert_eq!(result.to_string(), "The turn was interrupted.");
    assert!(notifier.await.unwrap(), "the stalled tool was not called before shutdown");

    drain.await.unwrap();
    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn restarts_a_failed_tool_process() {
    let marker = std::env::temp_dir().join(format!("krabb-tool-restart-{}", std::process::id()));

    let _ = fs::remove_file(&marker);

    let script = format!(
        r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{"protocol":{{"major":0,"minor":1}},"id":"tools","version":"0.1.0","capabilities":["tool"]}}}}' ;; *read*) if [ ! -e "{marker}" ]; then touch "{marker}"; exit 0; else printf '%s\n' '{{"jsonrpc":"2.0","id":7,"result":{{"text":"recovered"}}}}'; fi ;; *shutdown*) printf '%s\n' '{{"jsonrpc":"2.0","id":9999,"result":{{"ok":true}}}}'; exit 0 ;; esac; done"#,
        marker = marker.display()
    );

    let process = Process::start_with("sh", ["-c", &script]).await.unwrap();
    let plugins = registry([process]).await;
    let mut failed = None;

    let error = super::tool(
        &plugins,
        7,
        "read",
        serde_json::json!({"path": "note.txt"}),
        false,
        None,
        "global",
        &mut failed,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("closed stdout"));
    assert_eq!(failed.as_deref(), Some("tools"));

    restart_tool(&plugins, failed.as_deref().unwrap()).await;

    let mut ignored = None;

    let output = super::tool(
        &plugins,
        7,
        "read",
        serde_json::json!({"path": "note.txt"}),
        false,
        None,
        "global",
        &mut ignored,
    )
    .await
    .unwrap();

    assert_eq!(output, "recovered");

    stop_registry(&plugins).await;

    let _ = fs::remove_file(marker);
}

#[cfg(unix)]
#[tokio::test]
async fn retries_failed_channel_delivery_and_resets_status() {
    let channel_script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocol\":{\"major\":0,\"minor\":1},\"id\":\"telegram\",\"version\":\"0.1.0\",\"capabilities\":[\"channel\"]}}' ;; *poll*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":10,\"result\":{\"events\":[{\"id\":1,\"chat\":7,\"private\":true,\"sender\":8,\"text\":\"hi\"}]}}';; *send*) exit 0 ;; esac; done"#;
    let channel_script = channel_script.replace("\\\\n", "\\n").replace("\\\"", "\"");
    let provider_script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocol\":{\"major\":0,\"minor\":1},\"id\":\"codex\",\"version\":\"0.1.0\",\"capabilities\":[\"model\"]}}' ;; *generate*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":11,\"result\":{\"text\":\"world\",\"stop\":\"stop\"}}' ;; *shutdown*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{\"ok\":true}}'; exit 0 ;; esac; done"#;
    let provider_script = provider_script.replace("\\\\n", "\\n").replace("\\\"", "\"");
    let channel = Process::start_with("sh", ["-c", &channel_script]).await.unwrap();
    let provider = Process::start_with("sh", ["-c", &provider_script]).await.unwrap();
    let plugins = registry([channel, provider]).await;
    let root = std::env::temp_dir().join(format!("krabb-retry-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    let stop = Arc::new(Stop::new());
    let sessions_for_signal = Arc::clone(&sessions);
    let signal = Arc::clone(&stop);

    let notifier = tokio::spawn(async move {
        for _ in 0..200 {
            let ready = {
                let store = sessions_for_signal.lock().unwrap();

                store.sessions.get("telegram-7").is_some_and(|session| session.status == "idle")
                    && store.outbox.first().is_some_and(|delivery| {
                        delivery.status == super::state::DeliveryStatus::Uncertain
                    })
            };

            if ready {
                signal.signal();
                return;
            }

            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        signal.signal();
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        super::bridge(
            &plugins,
            "telegram",
            "codex",
            "test",
            Arc::clone(&sessions),
            Arc::clone(&stop),
            Arc::new(Mutex::new(BTreeMap::new())),
            BTreeMap::new(),
            super::ApprovalMode::Off,
        ),
    )
    .await
    .unwrap()
    .unwrap();

    notifier.await.unwrap();

    {
        let store = sessions.lock().unwrap();

        assert_eq!(store.sessions["telegram-7"].status, "idle");
        assert_eq!(store.outbox[0].status, super::state::DeliveryStatus::Uncertain);
    }

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn retries_channel_acknowledgement_after_restarting_plugin() {
    let root = test_root("ack");
    let marker = root.join("failed");

    let script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}' ;; *ack*) if [ ! -e "__MARKER__" ]; then touch "__MARKER__"; printf '%s\n' '{"jsonrpc":"2.0","id":10,"error":{"code":-32000,"message":"retry"}}'; else printf '%s\n' '{"jsonrpc":"2.0","id":11,"result":{"acknowledged":true}}'; fi ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#
            .replace("__MARKER__", &marker.display().to_string())
            .replace("\\\\n", "\\n")
            .replace("\\\"", "\"");

    let channel = Process::start_with("sh", ["-c", &script]).await.unwrap();
    let channel = Live::new(channel);

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    let mut current = 0;
    let mut call = 10;

    commit_event(
        &channel,
        &sessions,
        "telegram",
        "event",
        Some(2),
        &mut current,
        Some(7),
        &mut call,
    )
    .await
    .unwrap();

    assert_eq!(current, 2);
    assert_eq!(call, 12);
    assert!(sessions.lock().unwrap().known("telegram", "event"));

    channel.stop().await.unwrap();

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn retries_a_deferred_event_acknowledgement_after_replay() {
    let root = test_root("deferred-ack");

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    sessions
        .lock()
        .unwrap()
        .defer_event("telegram", serde_json::json!({"id": "event", "gateway_sequence": 7}))
        .unwrap();

    let mut current = 0;
    let mut call = 10;

    assert!(!sessions.lock().unwrap().deferred_acknowledged("telegram", "event"));

    let success_script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}' ;; *ack*) printf '%s\n' '{"jsonrpc":"2.0","id":10,"result":{"acknowledged":true}}' ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#;
    let channel = Live::new(Process::start_with("sh", ["-c", success_script]).await.unwrap());

    commit_event(
        &channel,
        &sessions,
        "telegram",
        "event",
        Some(2),
        &mut current,
        Some(7),
        &mut call,
    )
    .await
    .unwrap();

    assert_eq!(current, 2);
    assert_eq!(call, 11);

    {
        let store = sessions.lock().unwrap();

        assert!(store.known("telegram", "event"));
        assert!(!store.has_deferred_event("telegram", "event"));
    }

    channel.stop().await.unwrap();

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn interrupts_working_sessions_on_shutdown() {
    let channel_script = r#"first=1; while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}' ;; *poll*) if [ "$first" = 1 ]; then first=0; printf '%s\n' '{"jsonrpc":"2.0","id":10,"result":{"events":[{"id":1,"chat":7,"private":true,"sender":8,"text":"hi"}]}}'; fi ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#;
    let channel_script = channel_script.replace("\\n", "\n").replace("\\\"", "\"");
    let provider_script = r#"while IFS= read -r line; do case "$line" in *hello*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"codex","version":"0.1.0","capabilities":["model"]}}' ;; *generate*) sleep 5 ;; *shutdown*) printf '%s\n' '{"jsonrpc":"2.0","id":9999,"result":{"ok":true}}'; exit 0 ;; esac; done"#;
    let provider_script = provider_script.replace("\\n", "\n").replace("\\\"", "\"");
    let channel = Process::start_with("sh", ["-c", &channel_script]).await.unwrap();
    let provider = Process::start_with("sh", ["-c", &provider_script]).await.unwrap();
    let plugins = registry([channel, provider]).await;
    let root = std::env::temp_dir().join(format!("krabb-interrupt-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    let sessions =
        Arc::new(Mutex::new(super::state::Store::load(root.join("sessions.json")).unwrap()));

    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);

    let notifier = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        signal.signal();
    });

    super::bridge(
        &plugins,
        "telegram",
        "codex",
        "test",
        Arc::clone(&sessions),
        Arc::clone(&stop),
        Arc::new(Mutex::new(BTreeMap::new())),
        BTreeMap::new(),
        super::ApprovalMode::Off,
    )
    .await
    .unwrap();

    notifier.await.unwrap();

    assert_eq!(sessions.lock().unwrap().sessions["telegram-7"].status, "interrupted");

    stop_registry(&plugins).await;

    let _ = fs::remove_dir_all(root);
}

#[test]
fn failures_without_their_own_next_action_point_to_debug_output() {
    assert_eq!(
        super::conclude("Session was not found.".into()),
        "Session was not found.\nRun the command again with `--debug` for details."
    );

    let own = "Plugin echo is not installed.\nInstall it with `krabb plugin install echo`.";

    assert_eq!(super::conclude(own.into()), own);

    let link = "The release is broken.\nReport it at https://github.com/krabblab/krabb/issues.";

    assert_eq!(super::conclude(link.into()), link);
}

#[tokio::test]
async fn legacy_job_calls_are_validated_against_the_current_schema() {
    let sessions = Arc::new(Mutex::new(super::state::Store::default()));

    {
        let mut store = sessions.lock().unwrap();

        store.create("room", "test").unwrap();
        store
            .begin(
                "room",
                Message {
                    id: "turn".into(),
                    session: "room".into(),
                    role: Role::User,
                    sender: None,
                    content: vec![Content::Text { text: "request".into() }],
                },
            )
            .unwrap();
    }

    let call = async |args: serde_json::Value| {
        let registry = super::ToolRegistry {
            specs: super::tasks::control_tools(),
            ..super::ToolRegistry::default()
        };

        let (notices, _events) = tokio::sync::mpsc::channel(4);

        super::host_tool_with_registry(
            Request::call(7, "host/tool", serde_json::json!({"name": "job", "args": args})),
            &Plugins::default(),
            &sessions,
            "room",
            "codex",
            "test",
            None,
            false,
            false,
            false,
            super::ApprovalMode::Off,
            Arc::new(super::AsyncMutex::new(super::approval::Gate::new().unwrap())),
            "telegram",
            "8",
            None,
            notices,
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            registry,
            "turn".into(),
            tokio::time::Instant::now() + std::time::Duration::from_secs(5),
            None,
            None,
        )
        .await
        .unwrap()
        .error
        .unwrap()
    };

    let error = call(serde_json::json!({"action": "pause", "id": "task-1", "extra": 1})).await;

    assert_eq!(error.code, -32602);
    assert!(error.message.starts_with("Tool arguments are invalid:"), "{}", error.message);

    let error = call(serde_json::json!({"action": "approve", "id": "task-1"})).await;

    assert_eq!(error.code, -32000);
    assert!(error.message.contains("Approval policy blocks"), "{}", error.message);
}

#[test]
fn plugins_cannot_register_current_or_earlier_host_tool_names() {
    let mut names = vec![super::routing::NAME, super::tasks::CHECKPOINT, super::tasks::input::NAME];

    names.extend(super::tasks::CONTROLS);
    names.extend([super::slash::TOOL, super::settings::TOOL]);
    names.extend(super::aliases::CONTROLS);
    names.extend(super::aliases::HOST.map(|(old, _)| old));

    for name in names {
        let registry = super::ToolRegistry {
            owners: [(name.to_owned(), "fixture".to_owned())].into(),
            ..super::ToolRegistry::default()
        };

        assert_eq!(super::reserved_tool(&registry), Some(name));
    }

    let registry = super::ToolRegistry {
        owners: [("read".to_owned(), "files".to_owned())].into(),
        ..super::ToolRegistry::default()
    };

    assert_eq!(super::reserved_tool(&registry), None);

    let owned = |name: &str| registry.owns(name);

    assert_eq!(super::aliases::current("read", serde_json::json!({}), owned).0, "read");
    assert_eq!(super::aliases::current("write", serde_json::json!({}), owned).0, "file_write");
}

#[test]
fn start_failures_are_one_clean_line_and_name_setup_only_for_rejected_credentials() {
    let failure = super::StartFailure::new(
        "KRABB_GEMINI_API_KEY: abc123 was rejected (401 Unauthorized).\nRun it \u{1b}[2J again.",
    );

    assert!(failure.setup);
    assert!(!failure.reason.contains('\n'));
    assert!(!failure.reason.contains('\u{1b}'));
    assert!(!failure.reason.contains("abc123"), "{}", failure.reason);

    assert!(!super::StartFailure::new("Plugin binary was not found: gemini.").setup);
    assert!(!super::StartFailure::new("Listening on port 4013 failed.").setup);
    assert!(super::StartFailure::new("Telegram answered 403.").setup);
    assert!(super::reason(&"x".repeat(1_000)).len() <= 304);
}

#[test]
fn an_older_daemon_reply_still_lists_its_plugins_and_odd_entries_are_dropped() {
    let older = super::runtime(serde_json::json!({"items": ["codex"]})).unwrap();

    assert_eq!(older.active, ["codex"]);
    assert!(older.failures.is_empty() && older.missing.is_empty());

    let current = super::runtime(serde_json::json!({
        "items": ["codex"],
        "failures": {
            "gemini": {"reason": "Rejected.\nAgain.", "setup": true},
            "../escape": {"reason": "Nope.", "setup": false},
            "telegram": "an older shape",
        },
        "missing": {
            "signal": "Install signal-cli.",
            "tools": "Install \u{1b}[2J it.",
            "slack": 7,
        },
    }))
    .unwrap();

    assert_eq!(current.failures.len(), 1);
    assert_eq!(current.failures["gemini"].reason, "Rejected. Again.");
    assert!(current.failures["gemini"].setup);
    assert_eq!(current.missing, BTreeMap::from([("signal".into(), "Install signal-cli.".into())]));
}

#[test]
fn list_items_take_the_runtime_state_unless_the_binary_is_gone() {
    let states = [serde_json::json!({"id": "gemini", "state": "failed to start",
        "reason": "Rejected.", "next": "krabb plugin setup gemini"})];

    let item = |id: &str| serde_json::json!({"id": id, "health": "installed"});

    let failed = super::status::listed(item("gemini"), &states, true);

    assert_eq!(failed["health"], "failed to start");
    assert_eq!(failed["reason"], "Rejected.");
    assert_eq!(failed["next"], "krabb plugin setup gemini");

    let gone = super::status::listed(item("gemini"), &states, false);

    assert_eq!(gone["health"], "missing");
    assert!(gone.get("reason").is_none());
    assert_eq!(super::status::listed(item("stray"), &states, true)["health"], "installed");
}
