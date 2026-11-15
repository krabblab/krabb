use krabb_core::{
    Error,
    failure::{Failure, Kind},
};

pub(super) fn timeout(message: impl Into<String>) -> Error {
    Failure::new(Kind::TimedOut).error(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn a_reset_after_sending_is_transient_and_an_unsendable_request_is_invalid() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let reset = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", reset.local_addr().unwrap());

        tokio::spawn(async move {
            let (mut socket, _) = reset.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await;
        });

        let kind = |error| match krabb_http::transport(&error, "Ollama") {
            Error::Failed { failure, .. } => failure.kind,
            error => panic!("Missing typed transport failure: {error}"),
        };

        let error = client.get(&address).send().await.unwrap_err();

        assert!(!error.is_connect(), "{error:?}");
        assert_eq!(kind(error), Kind::Transient);
        assert_eq!(kind(client.get("https://[invalid").send().await.unwrap_err()), Kind::Invalid);
    }

    #[tokio::test]
    async fn http_failures_classify_before_body_parsing_in_every_generation_route() {
        for (status, retry, kind, delay) in [
            (429, Some("2"), Kind::Throttled, Some(2)),
            (503, Some("Sun, 06 Nov 1994 08:49:37 GMT"), Kind::Transient, Some(0)),
            (401, Some("999999999999999999999"), Kind::Denied, None),
            (429, Some("-1"), Kind::Invalid, None),
            (503, Some("604801"), Kind::Invalid, None),
            (404, None, Kind::Missing, None),
        ] {
            for route in 0..3 {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let host = format!("http://{}", listener.local_addr().unwrap());
                let server = tokio::spawn(async move {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut reader = BufReader::new(socket);
                    let mut headers = String::new();
                    let mut length = 0;

                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).await.unwrap();

                        if line == "\r\n" {
                            break;
                        }

                        if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                            length = value.trim().parse::<usize>().unwrap();
                        }

                        headers.push_str(&line);
                    }

                    assert!(headers.starts_with("POST /api/chat HTTP/1.1"));
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).await.unwrap();
                    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();

                    assert_eq!(body["stream"], route != 0);
                    let hint =
                        retry.map(|value| format!("Retry-After: {value}\r\n")).unwrap_or_default();
                    let body = "private upstream body must not be echoed";
                    let response = format!(
                        "HTTP/1.1 {status} Fixture\r\n{hint}Content-Length: {}\r\nConnection: \
                            close\r\n\r\n{body}",
                        body.len()
                    );
                    reader.get_mut().write_all(response.as_bytes()).await.unwrap();
                });

                let input = serde_json::from_value(serde_json::json!({
                    "model":"fixture", "messages":[], "stream":route != 0,
                }))
                .unwrap();
                let client = reqwest::Client::builder().no_proxy().build().unwrap();
                let error = if route == 2 {
                    let (sender, _receiver) = tokio::sync::mpsc::channel(8);
                    let mut emitter = krabb_core::plugin::Emitter::new(sender);
                    crate::stream_request(&client, 1, input, &host, &mut emitter, None)
                        .await
                        .unwrap_err()
                } else {
                    crate::request(&client, 1, input, &host, route != 0).await.unwrap_err()
                };

                assert!(!error.to_string().contains("private upstream"));

                match error {
                    Error::Failed { failure, .. } => {
                        assert_eq!(failure.kind, kind);
                        assert_eq!(failure.retry_after_seconds, delay);
                    }

                    error => panic!("Missing typed HTTP failure: {error}"),
                }

                server.await.unwrap();
            }
        }
    }
}
