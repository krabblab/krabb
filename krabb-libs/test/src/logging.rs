use std::{
    future::Future,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SENTINEL: &str = "synthetic-credential-sentinel-48291";

struct Home(PathBuf);

impl Home {
    fn new(stamp: u128) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);

        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);

        let root = std::env::temp_dir()
            .join(format!("krabb-provider-logging-{}-{stamp}-{sequence}", std::process::id()));

        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub async fn check<F, Fut>(test: &str, cases: &[&str], body: &'static str, request: F)
where
    F: Fn(String, String) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    if let Ok(case) = std::env::var("KRABB_LOG_FIXTURE_CHILD") {
        let mode = if std::env::var("KRABB_LOG_FIXTURE_MODE").unwrap() == "debug" {
            krabb_log::Mode::Command { verbosity: krabb_log::Verbosity::Debug, json: false }
        } else {
            krabb_log::Mode::Plugin
        };

        krabb_log::initialize_with_command_log(mode);

        let (base, server) = if case == "connect" {
            ("http://127.0.0.1:0".to_owned(), None)
        } else {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("Logging HTTP regression requires permitted loopback sockets");
            let base = format!("http://{}", listener.local_addr().unwrap());
            let response_case = case.clone();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut received = Vec::new();

                loop {
                    let mut chunk = [0_u8; 1024];
                    let length = stream.read(&mut chunk).await.unwrap();

                    assert!(length > 0, "Fixture request ended before headers");
                    received.extend_from_slice(&chunk[..length]);

                    assert!(received.len() <= 16 * 1024, "Fixture request exceeded limit");

                    if let Some(end) = received.windows(4).position(|value| value == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&received[..end]);
                        let length = headers
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map_or(0, |(_, value)| value.trim().parse::<usize>().unwrap());

                        if received.len() >= end + 4 + length {
                            break;
                        }
                    }
                }

                assert!(String::from_utf8_lossy(&received).contains(SENTINEL));

                if response_case == "disconnect" {
                    return;
                }

                let status = if response_case == "success" { "200 OK" } else { "403 Forbidden" };

                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            });

            (base, Some(server))
        };

        let result = krabb_log::run_plugin("logging-fixture", async {
            let response = request(base, SENTINEL.to_owned()).await;

            assert_eq!(response.is_ok(), case == "success");

            if let Err(error) = &response {
                assert!(!error.contains(SENTINEL), "Provider error disclosed the sentinel");
            }

            if let Some(server) = server {
                server.await.unwrap();
            }

            response
        })
        .await;

        assert_eq!(result == std::process::ExitCode::SUCCESS, case == "success");
        return;
    }

    for case in cases {
        for format in ["text", "json"] {
            for mode in ["plugin", "debug"] {
                let home = Home::new(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                );
                let root = &home.0;
                let mut command = crate::process::command(test);
                command
                    .env("KRABB_HOME", root)
                    .env("KRABB_LOG_FORMAT", format)
                    .env("KRABB_LOG_FIXTURE_CHILD", case)
                    .env("KRABB_LOG_FIXTURE_MODE", mode);

                let output = tokio::time::timeout(Duration::from_secs(10), command.output())
                    .await
                    .expect("Logging fixture timed out")
                    .unwrap();

                assert!(
                    output.status.success(),
                    "Logging fixture failed ({case}/{format}/{mode}): {} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );

                for captured in [&output.stdout, &output.stderr] {
                    assert!(
                        !String::from_utf8_lossy(captured).contains(SENTINEL),
                        "Captured output disclosed the sentinel"
                    );
                }

                let logs = root.join("logs");
                let mut files = Vec::new();

                for entry in std::fs::read_dir(&logs).unwrap() {
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

                    assert!(text.contains("Plugin process started"));
                    assert!(text.contains("reqwest::connect"), "Dependency logging must be active");
                    assert!(!text.contains(SENTINEL), "Fixture log disclosed the sentinel");

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    #[test]
    fn concurrent_logging_homes_survive_peer_cleanup_with_the_same_timestamp() {
        let first = Home::new(0);
        std::fs::create_dir(first.0.join("logs")).unwrap();
        std::fs::write(first.0.join("logs/first.log"), "first fixture").unwrap();
        let (ready, started) = sync_channel(0);
        let (cleaned, resume) = sync_channel(0);

        std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                let second = Home::new(0);
                std::fs::create_dir_all(second.0.join("logs")).unwrap();
                std::fs::write(second.0.join("logs/second.log"), "second fixture").unwrap();
                ready.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(5)).unwrap();

                assert_eq!(
                    std::fs::read_to_string(second.0.join("logs/second.log")).unwrap(),
                    "second fixture"
                );

                assert!(!second.0.join("logs/first.log").exists());
                let path = second.0.clone();
                drop(second);

                assert!(!path.exists(), "The fixture must remove its own directory");
            });

            started.recv_timeout(Duration::from_secs(5)).unwrap();

            assert_eq!(
                std::fs::read_to_string(first.0.join("logs/first.log")).unwrap(),
                "first fixture"
            );
            let path = first.0.clone();
            drop(first);

            assert!(!path.exists(), "The fixture must remove its own directory");
            cleaned.send(()).unwrap();
            worker.join().unwrap();
        });
    }
}
