//! Bounded loopback endpoints for deterministic provider transport tests.
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn capture() -> (String, tokio::task::JoinHandle<serde_json::Value>) {
    let (base, server) = responses(vec![(403, "{}".into())]).await;
    let capture = tokio::spawn(async move { server.await.unwrap().remove(0).body });

    (base, capture)
}

pub struct Captured {
    pub headers: String,
    pub body: serde_json::Value,
}

pub async fn responses(
    replies: Vec<(u16, String)>,
) -> (String, tokio::task::JoinHandle<Vec<Captured>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(5), async move {
            let mut requests = Vec::new();

            for (status, reply) in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut received = Vec::new();

            loop {
                let mut chunk = [0_u8; 1024];
                let size = socket.read(&mut chunk).await.unwrap();

                assert!(size > 0, "Request ended before its body");

                received.extend_from_slice(&chunk[..size]);

                assert!(received.len() <= 2 * 1024 * 1024, "Fixture request exceeded limit");

                let Some(end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
                    continue;
                };

                let headers = String::from_utf8_lossy(&received[..end]);
                let size = headers.lines().filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, size)| size.trim().parse::<usize>().unwrap()).unwrap_or(0);

                if received.len() < end + 4 + size {
                    continue;
                }

                let body = if size == 0 { serde_json::Value::Null } else {
                    serde_json::from_slice(&received[end + 4..end + 4 + size]).unwrap()
                };

                requests.push(Captured { headers: headers.into_owned(), body });
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).as_bytes()).await.unwrap();

                break;
            }
            }

            requests
        }).await.unwrap()
    });

    (format!("http://{address}"), server)
}
