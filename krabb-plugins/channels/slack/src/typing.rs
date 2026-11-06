use super::*;

pub(super) async fn send(
    client: &reqwest::Client,
    token: &str,
    params: &Value,
    base: &str,
) -> krabb_core::Result<Value> {
    let Some(chat) = params["chat"].as_str().filter(|value| {
        value.starts_with('D')
            && value.len() <= 256
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) else {
        return Ok(json!(false));
    };

    let Some(thread) = params["thread"].as_str().filter(|value| {
        !value.is_empty()
            && value.len() <= 64
            && value.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.')
    }) else {
        return Ok(json!(false));
    };

    api_at(
        client,
        token,
        "assistant.threads.setStatus",
        json!({"channel_id": chat, "thread_ts": thread, "status": "is typing..."}),
        base,
    )
    .await?;

    Ok(json!(true))
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
    async fn typing_uses_assistant_status_only_in_direct_message_threads() {
        let client = reqwest::Client::new();

        for body in [r#"{"ok":true}"#, r#"{"ok":false,"error":"missing_scope"}"#] {
            let (base, server) = endpoint(body).await;
            let result =
                send(&client, "fixture", &json!({"chat":"D123", "thread":"123.456"}), &base).await;

            assert_eq!(result.is_ok(), body == r#"{"ok":true}"#);

            let (headers, request) = server.await.unwrap();

            assert!(headers.starts_with("POST /assistant.threads.setStatus HTTP/1.1"));
            assert!(headers.contains("authorization: Bearer fixture"));
            assert_eq!(
                request,
                json!({"channel_id":"D123", "thread_ts":"123.456", "status":"is typing..."})
            );
        }

        for params in [
            json!({}),
            json!({"chat":"C123","thread":"123.456"}),
            json!({"chat":"D123"}),
            json!({"chat":"D123","thread":"bad"}),
        ] {
            assert_eq!(send(&client, "fixture", &params, "invalid").await.unwrap(), json!(false));
        }
    }
}
