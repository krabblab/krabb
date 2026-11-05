use crate::*;
use tokio_tungstenite::{accept_async, tungstenite::protocol::CloseFrame};

use krabb_test::websocket as fixture;

#[tokio::test]
async fn gateway_logging_is_private() {
    fixture::check(
        "tests::logging::gateway_logging_is_private",
        |listener| async move {
            for (op, sequence) in [(2, 2), (6, 3)] {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(stream).await.unwrap();
                socket
                    .send(Message::Text(
                        json!({"op": 10, "d": {"heartbeat_interval": 60000}}).to_string().into(),
                    ))
                    .await
                    .unwrap();
                let payload = socket.next().await.unwrap().unwrap().into_text().unwrap();
                let payload: Value = serde_json::from_str(&payload).unwrap();

                assert_eq!(payload["op"], op);
                assert_eq!(payload["d"]["token"], fixture::TOKEN);

                if op == 6 {
                    assert_eq!(payload["d"]["session_id"], "fixture-session");
                    assert_eq!(payload["d"]["seq"], 2);
                } else {
                    socket
                        .send(Message::Text(
                            json!({"op": 0, "t": "READY", "s": 1,
                                "d": {"session_id": "fixture-session"}})
                            .to_string()
                            .into(),
                        ))
                        .await
                        .unwrap();
                }

                socket
                    .send(Message::Text(
                        json!({"op": 0, "t": "MESSAGE_CREATE", "s": sequence,
                            "d": {"id": "10", "channel_id": "20", "content": fixture::CONTENT,
                                "author": {"id": "30", "bot": false}}})
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
                socket
                    .close(Some(CloseFrame {
                        code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                        reason: fixture::CONTENT.into(),
                    }))
                    .await
                    .unwrap();

                assert!(matches!(socket.next().await, Some(Ok(Message::Close(_)))));
            }
        },

        |base| async move {
            let state = AsyncMutex::new(GatewayState::default());

            for sequence in [2, 3] {
                let result = gateway(&state, fixture::TOKEN, 2, &base).await.unwrap();

                assert_eq!(result["events"][0]["text"], fixture::CONTENT);
                assert_eq!(result["events"][0]["gateway_sequence"], sequence);
                assert_eq!(acknowledge(&state, sequence).await.unwrap()["acknowledged"], true);

                let result = gateway(&state, fixture::TOKEN, 2, &base).await.unwrap();

                assert_eq!(result["events"], json!([]));
                assert!(state.lock().await.socket.is_none());
            }
        },
    )
    .await;
}
