//! A bounded loopback endpoint that captures one JSON request and rejects it.
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn capture() -> (String, tokio::task::JoinHandle<serde_json::Value>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(5), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();

            loop {
                let mut chunk = [0_u8; 1024];
                let size = socket.read(&mut chunk).await.unwrap();

                assert!(size > 0, "Request ended before its body");

                received.extend_from_slice(&chunk[..size]);

                assert!(received.len() <= 64 * 1024, "Fixture request exceeded limit");

                let Some(end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
                    continue;
                };

                let headers = String::from_utf8_lossy(&received[..end]);
                let size = headers.lines().filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, size)| size.trim().parse::<usize>().unwrap()).unwrap();

                if received.len() < end + 4 + size {
                    continue;
                }

                let request = serde_json::from_slice(&received[end + 4..end + 4 + size]).unwrap();
                socket.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();

                return request;
            }
        }).await.unwrap()
    });

    (format!("http://{address}"), server)
}
