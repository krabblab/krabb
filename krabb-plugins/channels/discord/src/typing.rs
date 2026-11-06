use super::*;

pub(super) async fn send(
    client: &reqwest::Client,
    channel: &str,
    token: &str,
    base: &str,
) -> krabb_core::Result<Value> {
    let response = client
        .post(format!("{base}/channels/{channel}/typing"))
        .header(AUTHORIZATION, format!("Bot {token}"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map_err(|_| krabb_core::Error::Denied("Discord typing request failed.".into()))?;

    if response.status() != reqwest::StatusCode::NO_CONTENT {
        return Err(krabb_core::Error::Denied("Discord rejected the typing request.".into()));
    }

    Ok(json!(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn typing_routes_to_the_channel_and_accepts_empty_success() {
        for status in [204, 403, 429] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..count]);

                assert!(request.starts_with("POST /channels/123/typing HTTP/1.1"));
                assert!(request.contains("authorization: Bot fixture"));

                socket
                    .write_all(
                        format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\n\r\n").as_bytes(),
                    )
                    .await
                    .unwrap();
            });

            let result = call_with(
                &reqwest::Client::new(),
                7,
                "typing".into(),
                json!({"channel": "123", "message": "456", "private": false}),
                "fixture",
                &base,
            )
            .await;

            if status == 204 {
                assert_eq!(result.unwrap().unwrap().result, Some(json!(true)));
            } else {
                assert!(result.is_err());
            }

            server.await.unwrap();
        }

        for channel in [Value::Null, json!(""), json!("../other"), json!(123)] {
            assert!(operation("typing", &json!({"channel": channel})).is_err());
        }
    }
}
