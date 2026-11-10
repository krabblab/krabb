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
    for statuses in [vec![200, 429, 200], vec![429, 429, 429], vec![500]] {
        let succeeds = statuses.last() == Some(&200);

        let expected = if succeeds {
            vec!["x".repeat(2_000), "TAIL".into(), "TAIL".into()]
        } else {
            vec!["short".into(); statuses.len()]
        };

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

                assert!(line.starts_with("POST /channels/1/messages "));

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
                let body: Value = serde_json::from_slice(&body).unwrap();
                chunks.push(body["content"].as_str().unwrap().to_owned());
                let response =
                    if status == 429 { json!({"retry_after":0}) } else { json!({"id":"9"}) }
                        .to_string();
                socket.get_mut().write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
            }

            chunks
        });

        let client =
            reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(3)).build().unwrap();

        let result = call_with(
            &client,
            1,
            "send".into(),
            json!({"channel":"1", "text":source, "format":"markdown"}),
            "fixture-token",
            &base,
        )
        .await;

        assert_eq!(result.is_ok(), succeeds);

        let chunks = server.await.unwrap();

        assert_eq!(chunks, expected);
    }
}
