use std::{future::Future, time::Duration};
use tokio::net::TcpListener;

pub const TOKEN: &str = "synthetic-websocket-credential-48291";
pub const CONTENT: &str = "synthetic-private-message-59382";

pub async fn check<S, SF, C, CF>(test: &str, server: S, client: C)
where
    S: Fn(TcpListener) -> SF,
    SF: Future<Output = ()> + Send + 'static,
    C: Fn(String) -> CF,
    CF: Future<Output = ()>,
{
    if let Ok(base) = std::env::var("KRABB_WEBSOCKET_FIXTURE") {
        let mode = if std::env::var("KRABB_WEBSOCKET_MODE").unwrap() == "debug" {
            krabb_log::Mode::Command { verbosity: krabb_log::Verbosity::Debug, json: false }
        } else {
            krabb_log::Mode::Plugin
        };

        krabb_log::initialize_with_command_log(mode);
        let status = krabb_log::run_plugin("websocket-fixture", async {
            client(base).await;
            Ok::<(), String>(())
        })
        .await;

        assert_eq!(status, std::process::ExitCode::SUCCESS);
        return;
    }

    for format in ["text", "json"] {
        for mode in ["plugin", "debug"] {
            for filter in [None, Some("trace,tungstenite::protocol=trace,tokio_tungstenite=trace")]
            {
                let listener = TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("WebSocket logging regression requires permitted loopback sockets");
                let base = format!("http://{}", listener.local_addr().unwrap());
                let mut server = tokio::spawn(server(listener));
                let root = std::env::temp_dir().join(format!(
                    "krabb-websocket-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&root).unwrap();

                let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
                command
                    .args(["--exact", test, "--nocapture"])
                    .env_clear()
                    .env("KRABB_HOME", &root)
                    .env("KRABB_LOG_FORMAT", format)
                    .env("KRABB_WEBSOCKET_FIXTURE", base)
                    .env("KRABB_WEBSOCKET_MODE", mode)
                    .kill_on_drop(true);

                if let Some(filter) = filter {
                    command.env("KRABB_LOG", filter).env("RUST_LOG", filter);
                }

                if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
                    command.env("LLVM_PROFILE_FILE", profile);
                }

                let result = tokio::time::timeout(Duration::from_secs(15), command.output()).await;

                if result.is_err() {
                    server.abort();
                }

                let output = result.expect("WebSocket client fixture timed out").unwrap();

                if !output.status.success() {
                    server.abort();
                }

                assert!(
                    output.status.success(),
                    "WebSocket fixture failed: {} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                let result = tokio::time::timeout(Duration::from_secs(5), &mut server).await;

                if result.is_err() {
                    server.abort();
                }

                result.expect("WebSocket server fixture timed out").unwrap();

                for captured in [&output.stdout, &output.stderr] {
                    assert_private(&String::from_utf8_lossy(captured));
                }

                let mut files = Vec::new();

                for entry in std::fs::read_dir(root.join("logs")).unwrap() {
                    let path = entry.unwrap().path();

                    if path.is_dir() {
                        files.extend(
                            std::fs::read_dir(path).unwrap().map(|entry| entry.unwrap().path()),
                        );
                    } else {
                        files.push(path);
                    }
                }

                assert_eq!(files.len(), if mode == "debug" { 2 } else { 1 });

                for path in files {
                    let text = std::fs::read_to_string(path).unwrap();
                    assert_private(&text);

                    assert!(text.contains("Plugin process started"));
                    assert!(text.contains("Plugin process stopped"));

                    if format == "json" {
                        for line in text.lines() {
                            serde_json::from_str::<serde_json::Value>(line).unwrap();
                        }
                    }
                }

                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }
}

fn assert_private(text: &str) {
    for sentinel in [TOKEN, CONTENT] {
        let hex: String = sentinel.bytes().map(|byte| format!("{byte:02x}")).collect();
        let decimal = sentinel.bytes().map(|byte| byte.to_string()).collect::<Vec<_>>().join(", ");

        assert!(!text.contains(sentinel), "WebSocket diagnostic disclosed plaintext");
        assert!(!text.contains(&hex), "WebSocket diagnostic disclosed recoverable hex");
        assert!(!text.contains(&decimal), "WebSocket diagnostic disclosed recoverable bytes");
    }
}
