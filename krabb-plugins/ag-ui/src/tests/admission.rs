use super::*;
use std::{collections::BTreeSet, time::Duration};
use tokio::{io::AsyncBufReadExt, task::JoinSet};

async fn request(address: SocketAddr, run: &str) -> TcpStream {
    let mut client = TcpStream::connect(address).await.unwrap();

    let body = json!({
        "threadId": "admission",
        "runId": run,
        "messages": [{"id": run, "role": "user", "content": "hello"}]
    })
    .to_string();

    let request = format!(
        "POST / HTTP/1.1\r\nAuthorization: Bearer test-token\r\n\
         Host: localhost\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );

    client.write_all(request.as_bytes()).await.unwrap();

    client
}

async fn admitted(output: &mut mpsc::Receiver<Value>, run: &str) -> u64 {
    let call = output.recv().await.unwrap();

    assert_eq!(call["method"], "host/model_stream");
    assert_eq!(call["params"]["messages"][0]["id"], run);

    call["id"].as_u64().unwrap()
}

async fn rejected(address: SocketAddr, output: &mut mpsc::Receiver<Value>) {
    let client = request(address, "rejected").await;
    let mut reader = tokio::io::BufReader::new(client);
    let mut status = String::new();

    reader.read_line(&mut status).await.unwrap();

    assert_eq!(status, "HTTP/1.1 503 Service Unavailable\r\n");
    assert!(matches!(output.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn limits_pending_runs_and_recovers_admission_after_disconnect() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("admission regression requires permitted loopback sockets");

    let address = listener.local_addr().unwrap();
    let (sender, mut output) = mpsc::channel(16);
    let mut tasks = JoinSet::new();

    tasks.spawn(serve(listener, Emitter::new(sender), test_security()));

    tokio::time::timeout(Duration::from_secs(5), async {
        let mut active = Vec::new();

        for index in 0..4 {
            let run = format!("pending-{index}");
            let client = request(address, &run).await;
            let id = admitted(&mut output, &run).await;
            active.push((id, client));
        }

        rejected(address, &mut output).await;

        let (id, mut disconnected) = active.remove(0);
        disconnected.shutdown().await.unwrap();
        let mut response = String::new();
        disconnected.read_to_string(&mut response).await.unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("RUN_STARTED"));

        let cancelled = output.recv().await.unwrap();

        assert_eq!(cancelled["method"], "cancel");
        assert_eq!(cancelled["params"]["id"], id);

        let replacement = request(address, "replacement").await;
        let replacement_id = admitted(&mut output, "replacement").await;

        assert_ne!(replacement_id, id);

        active.push((replacement_id, replacement));
        rejected(address, &mut output).await;

        let mut expected: BTreeSet<_> = active.iter().map(|(id, _)| *id).collect();

        assert_eq!(expected.len(), 4);

        for (_, mut client) in active {
            client.shutdown().await.unwrap();
        }

        for _ in 0..4 {
            let cancelled = output.recv().await.unwrap();

            assert_eq!(cancelled["method"], "cancel");
            assert!(expected.remove(&cancelled["params"]["id"].as_u64().unwrap()));
        }

        assert!(expected.is_empty());
        assert!(matches!(output.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
    })
    .await
    .expect("listener admission or cancellation did not complete");

    tasks.abort_all();

    while let Some(result) = tasks.join_next().await {
        assert!(result.unwrap_err().is_cancelled());
    }
}
