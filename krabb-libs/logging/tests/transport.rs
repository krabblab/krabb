use std::io::{self, Cursor, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tokio_tungstenite::tungstenite::{
    Message, client,
    protocol::{CloseFrame, Role, WebSocket, frame::coding::CloseCode},
};

const TOKEN: &str = "synthetic-websocket-credential-48291";
const CONTENT: &str = "synthetic-private-message-59382";

struct Sensitive;

impl std::fmt::Debug for Sensitive {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("Sensitive transport fields must be rejected before formatting");
    }
}

struct Transport {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}

impl Read for Transport {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.input.read(bytes)
    }
}

impl Write for Transport {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn transport() {
    // Prepare server bytes before installing the client logger to isolate its evidence.
    let mut server = WebSocket::from_raw_socket(Cursor::new(Vec::new()), Role::Server, None);

    server.send(Message::Text(CONTENT.into())).unwrap();
    server.close(Some(CloseFrame { code: CloseCode::Normal, reason: CONTENT.into() })).unwrap();

    let mut input = b"HTTP/1.1 101 Switching Protocols\r\n\
        Upgrade: websocket\r\nConnection: Upgrade\r\n\
        Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n"
        .to_vec();

    input.extend(server.into_inner().into_inner());

    let mode = if std::env::var("KRABB_TRANSPORT_MODE").unwrap() == "plugin" {
        krabb_log::Mode::Plugin
    } else {
        krabb_log::Mode::Command { verbosity: krabb_log::Verbosity::Debug, json: false }
    };

    krabb_log::initialize_with_command_log(mode);
    tracing::info!("Transport fixture started");
    tracing::debug!(target: "unrelated_transport", "Unrelated diagnostics remain available");
    tracing::debug!(target: "tungstenite_adapter", "Distinct target remains available");
    tracing::error!(target: "tokio_tungstenite::fixture", secret = TOKEN);
    tracing::error!(target: "tungstenite::fixture", secret = TOKEN);
    tracing::error!(target: "tokio_tungstenite", secret = ?Sensitive);
    tracing::error!(target: "tungstenite", secret = ?Sensitive);

    let request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(format!("ws://localhost/socket?ticket={TOKEN}"))
        .header("Host", "localhost")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(())
        .unwrap();

    let stream = Transport { input: Cursor::new(input), output: Vec::new() };
    let (mut socket, _) = client(request, stream).unwrap();

    for op in [2, 6] {
        socket
            .send(Message::Text(
                serde_json::json!({"op": op, "d": {"token": TOKEN}}).to_string().into(),
            ))
            .unwrap();
    }

    assert_eq!(socket.read().unwrap().into_text().unwrap(), CONTENT);
    assert!(matches!(socket.read().unwrap(), Message::Close(Some(_))));
    assert!(!socket.get_ref().output.is_empty());

    tracing::info!("Transport fixture completed");
}

fn assert_private(text: &str) {
    for sentinel in [TOKEN, CONTENT] {
        let hex: String = sentinel.bytes().map(|byte| format!("{byte:02x}")).collect();
        let decimal = sentinel.bytes().map(|byte| byte.to_string()).collect::<Vec<_>>().join(", ");

        assert!(!text.contains(sentinel), "Transport log disclosed plaintext");
        assert!(!text.contains(&hex), "Transport log disclosed recoverable hex");
        assert!(!text.contains(&decimal), "Transport log disclosed recoverable bytes");
    }
}

#[test]
fn websocket_logging_is_private() {
    if std::env::var_os("KRABB_TRANSPORT_CHILD").is_some() {
        transport();
        return;
    }

    for format in ["text", "json"] {
        for mode in ["plugin", "debug", "fallback"] {
            for filter in [None, Some("trace,tungstenite::protocol=trace,tokio_tungstenite=trace")]
            {
                let root = std::env::temp_dir().join(format!(
                    "krabb-transport-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&root).unwrap();

                if mode == "fallback" {
                    std::fs::write(root.join("logs"), b"Block file logging").unwrap();
                }

                let mut command = Command::new(std::env::current_exe().unwrap());
                command
                    .args(["--exact", "websocket_logging_is_private", "--nocapture"])
                    .env_clear()
                    .env("KRABB_HOME", &root)
                    .env("KRABB_LOG_FORMAT", format)
                    .env("KRABB_TRANSPORT_CHILD", "1")
                    .env("KRABB_TRANSPORT_MODE", mode)
                    .stdout(Stdio::null())
                    .stderr(Stdio::from(std::fs::File::create(root.join("stderr")).unwrap()));

                if let Some(filter) = filter {
                    command.env("KRABB_LOG", filter).env("RUST_LOG", filter);
                }

                if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
                    command.env("LLVM_PROFILE_FILE", profile);
                }

                let mut child = command.spawn().unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                let status = loop {
                    if let Some(status) = child.try_wait().unwrap() {
                        break status;
                    }

                    if Instant::now() >= deadline {
                        child.kill().unwrap();
                        child.wait().unwrap();
                        panic!("Transport fixture exceeded its deadline");
                    }

                    std::thread::sleep(Duration::from_millis(10));
                };

                assert!(status.success(), "Transport fixture failed: {root:?}");

                let mut files = vec![root.join("stderr")];

                if mode != "fallback" {
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
                }

                assert_eq!(
                    files.len(),
                    match mode {
                        "plugin" => 2,
                        "debug" => 3,
                        _ => 1,
                    }
                );

                for path in files {
                    let text = std::fs::read_to_string(&path).unwrap();
                    assert_private(&text);

                    if mode != "plugin" || path.file_name().unwrap() != "stderr" {
                        assert!(text.contains("Transport fixture completed"));
                        assert!(text.contains("Unrelated diagnostics remain available"));
                        assert!(text.contains("Distinct target remains available"));
                    }

                    if path.extension().is_some_and(|extension| extension == "jsonl") {
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
