use super::*;

use std::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::Poll,
};

use tokio::sync::{Semaphore, mpsc};

struct Active {
    id: u64,
    count: Arc<AtomicUsize>,
    retired: mpsc::UnboundedSender<u64>,
}

impl Drop for Active {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);

        let _ = self.retired.send(self.id);
    }
}

async fn drive<T>(
    server: Pin<&mut impl Future<Output = crate::Result<()>>>,
    action: impl Future<Output = T>,
) -> T {
    tokio::select! {
        biased;
        result = server => panic!("server stopped before shutdown: {result:?}"),
        result = action => result,
    }
}

#[tokio::test]
async fn saturation_preserves_controls_and_recovers_capacity() {
    timeout(Duration::from_secs(2), saturation()).await.expect("bounded admission and teardown");
}

async fn saturation() {
    let limit = super::super::CONCURRENT_HANDLERS;
    let (client, server) = duplex(16 * 1024);
    let (client_read, mut client_write) = split(client);
    let (server_read, server_write) = split(server);
    let mut input = BufReader::new(client_read);
    let (entered, mut started) = mpsc::unbounded_channel();
    let (retired, mut cleaned) = mpsc::unbounded_channel();
    let count = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let gates: Arc<Vec<_>> = Arc::new((0..limit + 2).map(|_| Semaphore::new(0)).collect());
    let handler_count = Arc::clone(&count);
    let handler_peak = Arc::clone(&peak);
    let handler_gates = Arc::clone(&gates);

    let handle = move |request: Request, _: Emitter| {
        let count = Arc::clone(&handler_count);
        let peak = Arc::clone(&handler_peak);
        let gates = Arc::clone(&handler_gates);
        let entered = entered.clone();
        let retired = retired.clone();

        async move {
            let id = request.id().unwrap();
            peak.fetch_max(count.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
            let _active = Active { id, count, retired };
            entered.send(id).unwrap();
            let gate = gates.get(id as usize).expect("rejected calls must never reach the handler");
            let _permit = gate.acquire().await.unwrap();

            Ok(Some(Response::ok(id, json!({"completed": id}))))
        }
    };

    let server =
        serve_io_events_concurrent(hello(), BufReader::new(server_read), server_write, handle);

    tokio::pin!(server);

    for id in 0..limit as u64 {
        jsonl::write(&mut client_write, &Request::call(id, "generate", json!({}))).await.unwrap();

        let started = drive(server.as_mut(), started.recv()).await.unwrap();

        assert_eq!(started, id);
    }

    assert_eq!(count.load(Ordering::SeqCst), limit);

    jsonl::write(&mut client_write, &Request::call(99, "generate", json!({}))).await.unwrap();

    let response: Response = drive(server.as_mut(), async {
        tokio::select! {
            id = started.recv() => panic!("saturated server dispatched rejected work: {id:?}"),
            response = jsonl::read(&mut input, super::super::FRAME) => response.unwrap().unwrap(),
        }
    })
    .await;

    assert_eq!(response.id, 99);
    assert!(response.result.is_none());
    assert_eq!(response.error.unwrap().code, -32000);

    for (id, method) in [(100, "ping"), (101, "hello")] {
        jsonl::write(&mut client_write, &Request::call(id, method, json!({}))).await.unwrap();

        let response: Response =
            drive(server.as_mut(), jsonl::read(&mut input, super::super::FRAME))
                .await
                .unwrap()
                .unwrap();

        assert_eq!(response.id, id);
        assert!(response.error.is_none(), "{response:?}");

        let expected = if method == "ping" { json!({"ok": true}) } else { json!(hello()) };

        assert_eq!(response.result.unwrap(), expected);
        assert_eq!(count.load(Ordering::SeqCst), limit);
        assert!(started.try_recv().is_err(), "lifecycle calls must bypass the handler");
    }

    jsonl::write(
        &mut client_write,
        &Request::Note { jsonrpc: "2.0".into(), method: "cancel".into(), params: json!({"id": 0}) },
    )
    .await
    .unwrap();

    let retired = drive(server.as_mut(), cleaned.recv()).await.unwrap();

    assert_eq!(retired, 0);
    assert_eq!(count.load(Ordering::SeqCst), limit - 1);

    for replacement in [limit, limit + 1] {
        // Poll to quiescence so completed workers are joined before testing restored capacity.
        poll_fn(|cx| {
            assert!(server.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        jsonl::write(&mut client_write, &Request::call(replacement as u64, "generate", json!({})))
            .await
            .unwrap();

        let id = drive(server.as_mut(), async {
            tokio::select! {
                id = started.recv() => id.unwrap(),

                response = jsonl::read::<Response>(&mut input, super::super::FRAME) => {
                    panic!("replacement was rejected after retirement: {response:?}");
                }
            }
        })
        .await;

        assert_eq!(id, replacement as u64);
        assert_eq!(count.load(Ordering::SeqCst), limit);

        if replacement == limit {
            gates[1].add_permits(1);

            let response: Response =
                drive(server.as_mut(), jsonl::read(&mut input, super::super::FRAME))
                    .await
                    .unwrap()
                    .unwrap();

            assert_eq!(response.id, 1);
            assert!(response.error.is_none());
            assert_eq!(response.result.unwrap(), json!({"completed": 1}));
            assert_eq!(cleaned.recv().await.unwrap(), 1);
        }
    }

    jsonl::write(&mut client_write, &Request::call(102, "shutdown", json!({}))).await.unwrap();

    let response: Response = drive(server.as_mut(), jsonl::read(&mut input, super::super::FRAME))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(response.id, 102);
    assert!(response.error.is_none());
    assert_eq!(response.result.unwrap(), json!({"ok": true}));
    assert_eq!(count.load(Ordering::SeqCst), limit);

    for gate in gates.iter().skip(2) {
        gate.add_permits(1);
    }

    server.await.unwrap();

    let mut completed = Vec::new();

    while let Some(response) =
        jsonl::read::<Response>(&mut input, super::super::FRAME).await.unwrap()
    {
        assert!(response.error.is_none());
        assert_eq!(response.result.unwrap(), json!({"completed": response.id}));
        completed.push(response.id);
    }

    completed.sort_unstable();

    assert_eq!(completed, (2..(limit + 2) as u64).collect::<Vec<_>>());
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(peak.load(Ordering::SeqCst), limit);
    assert!(started.try_recv().is_err());
}
