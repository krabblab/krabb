use crate::*;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

const SESSION: &str = "rejected-fixture-session";
const TOKEN: &str = "synthetic-recovery-token";

async fn handshake(listener: &TcpListener) -> (WebSocketStream<TcpStream>, Value) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut socket = accept_async(stream).await.unwrap();

    send(&mut socket, json!({"op": 10, "d": {"heartbeat_interval": 60000}})).await;

    let payload = socket.next().await.unwrap().unwrap().into_text().unwrap();
    let payload = serde_json::from_str(&payload).unwrap();

    (socket, payload)
}

async fn send(socket: &mut WebSocketStream<TcpStream>, value: Value) {
    socket.send(Message::Text(value.to_string().into())).await.unwrap();
}

fn message() -> Value {
    json!({"op": 0, "t": "MESSAGE_CREATE", "s": 8,
        "d": {"id": "10", "channel_id": "20", "content": "recover this message",
            "author": {"id": "30", "bot": false}}})
}

async fn recovery(resumable: bool) {
    save_cursor(Some(7), Some(SESSION)).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Gateway recovery regression requires permitted loopback sockets");

    let base = format!("http://{}", listener.local_addr().unwrap());
    let recovered_session = if resumable { SESSION } else { "new-fixture-session" };

    let (acknowledged, wait) = tokio::sync::oneshot::channel();

    let server = async move {
        let (mut socket, payload) = handshake(&listener).await;

        assert_eq!(payload["op"], 6);
        assert_eq!(payload["d"]["session_id"], SESSION);
        assert_eq!(payload["d"]["seq"], 7);

        send(&mut socket, message()).await;
        send(&mut socket, json!({"op": 9, "d": resumable})).await;
        drop(socket);

        let (mut socket, payload) = handshake(&listener).await;

        assert_eq!(payload["op"], if resumable { 6 } else { 2 });

        assert_eq!(payload["d"]["token"], TOKEN);

        if resumable {
            assert_eq!(payload["d"]["session_id"], SESSION);
            assert_eq!(payload["d"]["seq"], 7);
        } else {
            assert!(payload["d"].get("session_id").is_none());

            send(
                &mut socket,
                json!({"op": 0, "t": "READY", "s": 1,
                    "d": {"session_id": recovered_session}}),
            )
            .await;
        }

        send(&mut socket, message()).await;
        wait.await.unwrap();
    };

    let client = async {
        let state = AsyncMutex::new(GatewayState::default());
        let pending = gateway(&state, TOKEN, 2, &base).await.unwrap();

        assert_eq!(pending["events"][0]["gateway_sequence"], 8);
        assert_eq!(load_cursor(), (Some(7), Some(SESSION.into())));

        let error = gateway(&state, TOKEN, 2, &base).await.unwrap_err();

        assert!(error.to_string().contains("rejected the session"));

        drop(state);

        let restarted = GatewayState::default();

        assert_eq!(restarted.sequence, Some(7));
        assert_eq!(restarted.session_id.as_deref(), resumable.then_some(SESSION));

        let state = AsyncMutex::new(restarted);
        let recovered = gateway(&state, TOKEN, 2, &base).await.unwrap();

        assert_eq!(recovered["events"].as_array().unwrap().len(), 1);
        assert_eq!(recovered["events"][0], pending["events"][0]);
        assert_eq!(load_cursor(), (Some(7), Some(recovered_session.into())));
        assert_eq!(acknowledge(&state, 8).await.unwrap()["acknowledged"], true);

        drop(state);

        let committed = GatewayState::default();

        assert_eq!(committed.sequence, Some(8));
        assert_eq!(committed.session_id.as_deref(), Some(recovered_session));

        acknowledged.send(()).unwrap();
    };

    timeout(Duration::from_secs(5), async { tokio::join!(server, client) })
        .await
        .expect("Gateway rejection and restart did not complete");
}

#[tokio::test]
async fn non_resumable_rejection_identifies_after_restart() {
    if krabb_test::inbox::isolated(
        "tests::recovery::non_resumable_rejection_identifies_after_restart",
        &[],
    )
    .await
    {
        recovery(false).await;
    }
}

#[tokio::test]
async fn resumable_rejection_preserves_committed_cursor_after_restart() {
    if krabb_test::inbox::isolated(
        "tests::recovery::resumable_rejection_preserves_committed_cursor_after_restart",
        &[],
    )
    .await
    {
        recovery(true).await;
    }
}
