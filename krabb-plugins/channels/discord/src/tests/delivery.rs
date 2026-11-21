use crate::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[test]
fn bounds_rate_limit_waits_and_requires_an_explicit_rejection() {
    let limited = reqwest::StatusCode::TOO_MANY_REQUESTS;

    assert_eq!(
        retry_delay(limited, &json!({"retry_after": 0.25}), None),
        Some(Duration::from_millis(250))
    );

    assert_eq!(retry_delay(limited, &json!({}), Some(1.0)), Some(Duration::from_secs(1)));

    for delay in [-1.0, 5.1, f64::INFINITY, f64::NAN] {
        assert!(retry_delay(limited, &json!({}), Some(delay)).is_none());
    }

    assert!(retry_delay(limited, &json!({}), None).is_none());

    assert!(
        retry_delay(reqwest::StatusCode::INTERNAL_SERVER_ERROR, &json!({"retry_after":0}), None)
            .is_none()
    );
}

#[tokio::test]
async fn retries_the_rejected_chunk_without_replaying_delivered_chunks() {
    for statuses in [vec![200, 429, 200], vec![200, 1429, 200], vec![429, 429, 429], vec![500]] {
        let succeeds = statuses.last() == Some(&200);

        let expected = if succeeds {
            vec!["x".repeat(2_000), "TAIL".into(), "TAIL".into()]
        } else {
            vec!["short".into(); statuses.len()]
        };

        let label = format!("{statuses:?}");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let source = if succeeds { format!("{}TAIL", "x".repeat(2_000)) } else { "short".into() };

        let server = tokio::spawn(async move {
            let mut chunks = Vec::new();

            for status in statuses {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                let mut line = String::new();
                socket.read_line(&mut line).await.unwrap();

                assert!(
                    line.starts_with("POST /channels/1/messages "),
                    "unexpected request {line:?}"
                );

                let mut length = 0;

                loop {
                    line.clear();

                    let size = socket.read_line(&mut line).await.unwrap();

                    assert_ne!(size, 0, "request ended before its headers");

                    if line == "\r\n" {
                        break;
                    }

                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }

                let mut body = vec![0; length];
                socket.read_exact(&mut body).await.unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap();
                chunks.push(body["content"].as_str().unwrap().to_owned());
                let (status, header, response) = match status {
                    1429 => (429, "Retry-After: 0\r\n", "<html>Slow down</html>".to_owned()),
                    429 => (429, "", json!({"retry_after":0}).to_string()),
                    status => (status, "", json!({"id":"9"}).to_string()),
                };

                socket.get_mut().write_all(format!("HTTP/1.1 {status} Fixture\r\n{header}Content-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
            }

            chunks
        });

        let client =
            reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap();

        let result = call_with(
            &client,
            1,
            "send".into(),
            json!({"channel":"1", "text":source, "format":"markdown"}),
            "fixture-token",
            &base,
        )
        .await;

        let chunks = tokio::time::timeout(Duration::from_secs(15), server)
            .await
            .unwrap_or_else(|_| panic!("{label}: fixture awaited a request after {result:?}"))
            .unwrap();

        assert_eq!(result.is_ok(), succeeds, "{label}: {result:?}");
        assert_eq!(chunks, expected, "{label}");
    }
}

#[tokio::test]
async fn a_plain_send_records_its_message_for_later_replies() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());

    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = BufReader::new(socket);
        let mut line = String::new();
        let mut length = 0;

        loop {
            line.clear();
            socket.read_line(&mut line).await.unwrap();

            if line == "\r\n" {
                break;
            }

            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<usize>().unwrap();
            }
        }

        let mut body = vec![0; length];
        socket.read_exact(&mut body).await.unwrap();

        let response = json!({"id":"9"}).to_string();
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
            response.len()
        );

        socket.get_mut().write_all(reply.as_bytes()).await.unwrap();
    });

    let client =
        reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(3)).build().unwrap();

    let response = call_with(
        &client,
        1,
        "send".into(),
        json!({"channel":"1", "text":"plain answer"}),
        "fixture-token",
        &base,
    )
    .await
    .unwrap()
    .unwrap();

    server.await.unwrap();

    let references = &response.result.unwrap()["references"];

    assert_eq!(references[0]["id"], "9");
    assert_eq!(references[0]["text"], "plain answer");
}

#[tokio::test]
async fn an_unreachable_discord_reports_its_transport_cause() {
    let closed = tokio::net::TcpSocket::new_v4().unwrap();

    closed.bind("127.0.0.1:0".parse().unwrap()).unwrap();

    let base = format!("http://{}", closed.local_addr().unwrap());
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let error =
        message(&client, reqwest::Method::GET, format!("{base}/gateway"), "fixture", Value::Null)
            .await
            .unwrap_err();

    assert!(error.to_string().contains("could not be reached"), "{error}");

    let error = message(
        &client,
        reqwest::Method::POST,
        format!("{base}/channels/1/messages"),
        "fixture",
        json!({"content":"hello"}),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(
            &error,
            krabb_core::Error::Failed { failure, .. }
                if failure.kind == krabb_core::failure::Kind::Transient
        ),
        "{error}"
    );

    drop(closed);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 1024];
        let _ = socket.read(&mut request).await;
    });

    let error = message(
        &client,
        reqwest::Method::POST,
        format!("{base}/channels/1/messages"),
        "fixture",
        json!({"content":"hello"}),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(
            &error,
            krabb_core::Error::Failed { failure, .. }
                if failure.kind == krabb_core::failure::Kind::Uncertain
        ),
        "{error}"
    );
}
