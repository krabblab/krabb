use super::*;
use tokio::io::AsyncReadExt;

struct Cleanup {
    _sender: tokio::sync::oneshot::Sender<()>,
}

async fn input_failure_retires_handler(invalid_response: bool) {
    let (client, server) = duplex(16 * 1024);
    let (mut client_read, mut client_write) = split(client);
    let (server_read, server_write) = split(server);
    let (ready, mut started) = tokio::sync::mpsc::channel(1);
    let (dropped, cleaned) = tokio::sync::oneshot::channel();
    let dropped = Arc::new(std::sync::Mutex::new(Some(dropped)));
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::clone(&release);

    let handle = move |request: Request, mut emitter: Emitter| {
        let ready = ready.clone();
        let dropped = dropped.lock().unwrap().take().unwrap();
        let gate = Arc::clone(&gate);

        async move {
            let _cleanup = Cleanup { _sender: dropped };
            ready.send(()).await.unwrap();
            gate.notified().await;
            emitter.event(json!({"kind": "text", "text": "unexpected completion"})).await?;
            Ok(Some(Response::ok(request.id().unwrap(), json!({}))))
        }
    };

    let mut serving = Box::pin(serve_io_events_concurrent(
        hello(),
        BufReader::new(server_read),
        server_write,
        handle,
    ));

    let client = async {
        jsonl::write(&mut client_write, &Request::call(1, "generate", json!({}))).await.unwrap();
        started.recv().await.unwrap();

        if invalid_response {
            jsonl::write(&mut client_write, &json!({"jsonrpc": "1.0", "id": 2, "result": {}}))
                .await
                .unwrap();
        } else {
            client_write.shutdown().await.unwrap();
        }

        let result = cleaned.await;

        assert!(result.is_err(), "handler must be dropped without normal completion");
        // Keep reading until the server closes output; no client-side close unblocks teardown.
        let mut output = String::new();
        client_read.read_to_string(&mut output).await.unwrap();

        assert!(output.is_empty(), "aborted handler must not produce a response");
    };

    let (result, ()) =
        timeout(Duration::from_secs(2), async { tokio::join!(&mut serving, client) })
            .await
            .expect("fatal input must retire handlers without releasing their completion gate");

    let error = result.unwrap_err();

    let expected =
        if invalid_response { "host sent an invalid response" } else { "plugin input is closed" };

    assert!(matches!(error, crate::Error::Protocol(ref message) if message == expected));

    drop(release);
}

#[tokio::test]
async fn eof_retires_active_handler() {
    input_failure_retires_handler(false).await;
}

#[tokio::test]
async fn invalid_response_retires_active_handler() {
    input_failure_retires_handler(true).await;
}
