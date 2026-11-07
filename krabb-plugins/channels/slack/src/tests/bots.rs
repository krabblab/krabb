use super::*;

fn app() -> App {
    App {
        client: test_client(),
        socket: Arc::new(Mutex::new(None)),
        attachments: Arc::new(Mutex::new(BTreeMap::new())),
        inbox: Arc::new(Mutex::new(Inbox::default())),
        history: Arc::new(Mutex::new(HistoryPoll::default())),
    }
}

fn messages() -> Vec<Value> {
    vec![
        json!({"type":"message", "channel":"D1", "ts":"1.0", "user":"UBOT",
            "bot_id":"B1", "text":"pong"}),
        json!({"type":"message", "channel":"D1", "ts":"2.0", "user":"UBOT",
            "subtype":"file_share", "bot_profile":{"id":"B1"}, "text":"bot file",
            "files":[{"id":"FBOT", "name":"bot.txt", "mimetype":"text/plain",
                "url_private":"https://files.slack.com/files-pri/FBOT/download"}]}),
        json!({"type":"message", "channel":"D1", "ts":"3.0", "user":"U1",
            "text":"human message"}),
        json!({"type":"message", "channel":"D1", "ts":"4.0",
            "subtype":"bot_message", "text":"legacy bot"}),
    ]
}

#[tokio::test]
async fn socket_acknowledges_bot_messages_without_admitting_them() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();

            for event in messages() {
                let envelope = event["ts"].as_str().unwrap().to_owned();
                socket
                    .send(Message::Text(
                        json!({"envelope_id":envelope, "type":"events_api",
                            "payload":{"event":event}})
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();

                let ack = socket.next().await.unwrap().unwrap().into_text().unwrap();
                let ack: Value = serde_json::from_str(&ack).unwrap();

                assert_eq!(ack["envelope_id"], envelope);
            }
        });

        let app = app();
        let (socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
        *app.socket.lock().await = Some(socket);

        for index in 0..4 {
            let events =
                socket_poll_at(&app, "fixture", "D1", "http://127.0.0.1").await.unwrap().unwrap();

            if index == 2 {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0]["sender"], "U1");
                assert_eq!(events[0]["text"], "human message");
                app.inbox.lock().await.acknowledge(1).unwrap();
            } else {
                assert!(events.is_empty(), "bot message escaped the Socket Mode filter");
            }

            assert!(app.inbox.lock().await.pending.is_empty());
            assert!(app.attachments.lock().await.is_empty());
        }

        server.await.unwrap();
    })
    .await
    .expect("Socket Mode bot-filter fixture timed out");
}

#[tokio::test]
async fn history_skips_bots_and_advances_the_scan_cursor() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;

                if index == 1 {
                    let request = String::from_utf8(request).unwrap();
                    let body: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();

                    assert_eq!(body["oldest"], "4.0");
                }

                let messages = if index == 0 { messages() } else { Vec::new() };

                let body = json!({"ok":true, "messages":messages}).to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let app = app();
        let base = format!("http://{address}");
        let response = poll_at(&app, "fixture", "D1", &base).await.unwrap();

        assert_eq!(response["events"].as_array().unwrap().len(), 1);
        assert_eq!(response["events"][0]["sender"], "U1");
        assert!(app.attachments.lock().await.is_empty());

        let mut inbox = app.inbox.lock().await;

        assert_eq!(inbox.pending.len(), 1);
        assert_eq!(inbox.scanned["D1"], "4.0");
        inbox.acknowledge(1).unwrap();
        drop(inbox);
        app.history.lock().await.next = None;

        let response = poll_at(&app, "fixture", "D1", &base).await.unwrap();

        assert!(response["events"].as_array().unwrap().is_empty());
        server.await.unwrap();
    })
    .await
    .expect("History bot-filter fixture timed out");
}
