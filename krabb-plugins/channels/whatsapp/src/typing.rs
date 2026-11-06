use super::*;

pub(super) async fn send(
    app: &App,
    params: &Value,
    token: &str,
    base: &str,
    phone: &str,
) -> krabb_core::Result<Value> {
    let message = params["message"]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 1024)
        .ok_or_else(|| krabb_core::Error::Denied("typing.message is required.".into()))?;

    let result = graph(
        app,
        &format!("{base}/{phone}/messages"),
        token,
        json!({
            "messaging_product": "whatsapp",
            "status": "read",
            "message_id": message,
            "typing_indicator": {"type": "text"}
        }),
    )
    .await?;

    Ok(json!(result["success"] == true))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn endpoint(body: &'static str) -> (String, tokio::task::JoinHandle<(String, Value)>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(5), async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut received = Vec::new();

                loop {
                    let mut chunk = [0; 1024];
                    let count = socket.read(&mut chunk).await.unwrap();

                    assert!(count > 0);
                    received.extend_from_slice(&chunk[..count]);

                    assert!(received.len() <= 8192);

                    let Some(end) = received.windows(4).position(|part| part == b"\r\n\r\n") else {
                        continue;
                    };

                    let headers = String::from_utf8_lossy(&received[..end]).into_owned();
                    let size: usize = headers.lines().filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .unwrap().1.trim().parse().unwrap();

                    if received.len() < end + 4 + size {
                        continue;
                    }

                    let request = serde_json::from_slice(&received[end + 4..end + 4 + size]).unwrap();
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();

                    return (headers, request);
                }
            }).await.unwrap()
        });

        (base, server)
    }

    #[tokio::test]
    async fn typing_marks_the_inbound_message_read_and_checks_api_success() {
        let app =
            App { client: reqwest::Client::new(), queue: Arc::new(Mutex::new(Inbox::default())) };

        for (body, expected) in [
            (r#"{"success":true}"#, Some(true)),
            (r#"{"success":false}"#, Some(false)),
            (r#"{"error":{"message":"rejected"}}"#, None),
        ] {
            let (base, server) = endpoint(body).await;
            let result =
                send(&app, &json!({"message":"wamid.fixture"}), "fixture", &base, "123").await;

            if let Some(expected) = expected {
                assert_eq!(result.unwrap(), json!(expected));
            } else {
                assert!(result.is_err());
            }

            let (headers, request) = server.await.unwrap();

            assert!(headers.starts_with("POST /123/messages HTTP/1.1"));
            assert!(headers.contains("authorization: Bearer fixture"));
            assert_eq!(
                request,
                json!({"messaging_product":"whatsapp", "status":"read",
                "message_id":"wamid.fixture", "typing_indicator":{"type":"text"}})
            );
        }

        for message in [Value::Null, json!(""), json!("x".repeat(1025))] {
            assert!(
                send(&app, &json!({"message":message}), "fixture", "invalid", "123").await.is_err()
            );
        }
    }
}
