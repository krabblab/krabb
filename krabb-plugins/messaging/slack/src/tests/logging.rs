use crate::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

use krabb_test::websocket as fixture;

#[expect(clippy::result_large_err, reason = "Tungstenite requires this callback error type")]
fn handshake(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    assert_eq!(request.uri().query(), Some(format!("ticket={}", fixture::TOKEN).as_str()));

    Ok(response)
}

#[tokio::test]
async fn socket_logging_is_private() {
    fixture::check(
        "tests::logging::socket_logging_is_private",
        |listener| async move {
            let address = listener.local_addr().unwrap();
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();

            loop {
                let mut buffer = [0; 1024];
                let read = stream.read(&mut buffer).await.unwrap();

                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);

                assert!(request.len() <= 16 * 1024);

                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map_or(0, |(_, value)| value.trim().parse::<usize>().unwrap());

                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }

            let request = String::from_utf8(request).unwrap();

            assert!(request.starts_with("POST /apps.connections.open "));
            assert!(request.contains(fixture::TOKEN));

            let body = json!({"ok": true,
                "url": format!("ws://{address}/socket?ticket={}", fixture::TOKEN)})
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            drop(stream);

            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_hdr_async(stream, handshake).await.unwrap();
            socket
                .send(Message::Text(
                    json!({"envelope_id": "fixture-envelope", "type": "events_api",
                        "payload": {"event": {"type": "message", "channel": "D1", "ts": "1.0",
                            "text": fixture::CONTENT, "user": "U1"}}})
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let ack = socket.next().await.unwrap().unwrap().into_text().unwrap();
            let ack: Value = serde_json::from_str(&ack).unwrap();

            assert_eq!(ack["envelope_id"], "fixture-envelope");

            socket
                .send(Message::Text(json!({"type": "disconnect"}).to_string().into()))
                .await
                .unwrap();
        },
        |base| async move {
            let app = App {
                client: reqwest::Client::builder().no_proxy().build().unwrap(),
                socket: Arc::new(Mutex::new(None)),
                attachments: Arc::new(Mutex::new(BTreeMap::new())),
                inbox: Arc::new(Mutex::new(Inbox::default())),
                history: Arc::new(Mutex::new(HistoryPoll::default())),
            };

            let events = socket_poll_at(&app, fixture::TOKEN, "D1", &base).await.unwrap().unwrap();

            assert_eq!(events[0]["text"], fixture::CONTENT);
            assert_eq!(events[0]["private"], true);
            assert_eq!(events[0]["gateway_sequence"], 1);
            app.inbox.lock().await.acknowledge(1).unwrap();

            assert!(socket_poll_at(&app, fixture::TOKEN, "D1", &base).await.unwrap().is_none());
            assert!(app.socket.lock().await.is_none());
        },
    )
    .await;
}
