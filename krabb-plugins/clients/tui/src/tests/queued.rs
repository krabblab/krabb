use crate::*;
use serde_json::json;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};
use tokio::sync::mpsc::UnboundedReceiver;

const WAIT: std::time::Duration = std::time::Duration::from_secs(5);

fn config() -> EngineConfig {
    EngineConfig {
        home: std::env::temp_dir().display().to_string(),
        plugin: "codex".into(),
        model: "base".into(),
        model_override: None,
        session: "default".into(),
    }
}

async fn until(
    events: &mut UnboundedReceiver<EngineEvent>,
    done: impl Fn(&EngineEvent) -> bool,
) -> Vec<EngineEvent> {
    let mut seen = Vec::new();

    loop {
        let event = tokio::time::timeout(WAIT, events.recv()).await.unwrap().unwrap();
        let finished = done(&event);

        seen.push(event);

        if finished {
            return seen;
        }
    }
}

static RELEASED: AtomicBool = AtomicBool::new(false);
static QUEUED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static ANSWERS: AtomicUsize = AtomicUsize::new(0);

async fn ordered(home: String, method: String, params: Value) -> krabb_core::Result<Value> {
    let view = |queue: &[(String, String)]| {
        queue.iter().map(|(id, text)| json!({"id": id, "text": text})).collect::<Vec<_>>()
    };

    match method.as_str() {
        "session.answer" => {
            if ANSWERS.fetch_add(1, Ordering::SeqCst) == 0 {
                while !RELEASED.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            }

            Ok(json!({"text": format!("reply to {}", params["request"])}))
        }

        "session.queue" => {
            let mut queue = QUEUED.lock().unwrap();

            if queue.len() == 2 {
                return Err(krabb_core::Error::Denied(krabb_core::types::QUEUE_FULL.into()));
            }

            queue.push((
                params["request"].as_str().unwrap().into(),
                params["text"].as_str().unwrap().into(),
            ));

            Ok(json!({"position": queue.len(), "queue": view(&queue)}))
        }

        "session.next" if RELEASED.load(Ordering::SeqCst) => {
            let mut queue = QUEUED.lock().unwrap();

            if queue.is_empty() {
                return Ok(json!({"message": null, "queue": []}));
            }

            let (id, text) = queue.remove(0);

            Ok(json!({"owner": "queued-owner", "queue": view(&queue), "message": {
                "id": id, "session": "default", "role": "user", "sender": "tui",
                "content": [{"kind": "text", "text": text}]
            }}))
        }

        "session.next" => Ok(json!({"message": null})),

        "session.get" => Ok(json!({
            "id": "default", "model": "base", "status": "idle", "messages": [],
            "queue": view(&QUEUED.lock().unwrap()),
        })),

        _ => super::mock_control(home, method, params).await,
    }
}

#[tokio::test]
async fn messages_typed_during_a_turn_queue_in_order_and_start_after_it() {
    let (mut input, reader) = duplex(4096);

    let (writer, _output) = duplex(64 * 1024);
    let (_interrupt, interrupts) = tokio::sync::mpsc::unbounded_channel();
    let (events, mut notifications) = tokio::sync::mpsc::unbounded_channel();

    let engine = tokio::spawn(run_with(
        reader,
        writer,
        config(),
        ordered,
        EngineEvents { engine: Some(events), ..Default::default() },
        interrupts,
    ));

    input.write_all(b"first\n").await.unwrap();
    until(&mut notifications, |event| *event == EngineEvent::GenerationStarted).await;

    for text in ["second", "third", "fourth"] {
        input.write_all(format!("{text}\n").as_bytes()).await.unwrap();
    }

    let receipts = until(
        &mut notifications,
        |event| matches!(event, EngineEvent::InputRejected { input, .. } if input == "fourth"),
    )
    .await;

    let accepted = receipts
        .iter()
        .filter_map(|event| match event {
            EngineEvent::InputQueued { input, receipt } => Some((input.as_str(), receipt.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[0].0, "second");
    assert!(accepted[0].1.starts_with("Queued at position 1."));
    assert!(accepted[1].1.contains("/unqueue 2"));

    assert!(receipts.iter().any(|event| matches!(event, EngineEvent::Queue(queue)
        if queue.len() == 2)));

    assert_eq!(
        receipts.last().unwrap(),
        &EngineEvent::InputRejected { input: "fourth".into(), reason: queue::FULL.into() }
    );

    RELEASED.store(true, Ordering::SeqCst);

    let rest = until(
        &mut notifications,
        |event| matches!(event, EngineEvent::QueueStarted(text) if text.contains("third")),
    )
    .await;

    let started = rest
        .iter()
        .filter_map(|event| match event {
            EngineEvent::QueueStarted(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(started, ["second", "third"]);

    let finished = rest
        .iter()
        .position(|event| matches!(event, EngineEvent::GenerationFinished { .. }))
        .unwrap();

    let second = rest.iter().position(|event| matches!(event, EngineEvent::QueueStarted(_)));

    assert!(finished < second.unwrap());

    until(&mut notifications, |event| matches!(event, EngineEvent::GenerationFinished { .. }))
        .await;

    assert_eq!(ANSWERS.load(Ordering::SeqCst), 3);

    input.write_all(b"/quit\n").await.unwrap();
    tokio::time::timeout(WAIT, engine).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn quitting_during_a_turn_leaves_it_running_and_lists_the_queue() {
    async fn host(home: String, method: String, params: Value) -> krabb_core::Result<Value> {
        match method.as_str() {
            "session.answer" => std::future::pending().await,
            "session.cancel" => panic!("closing the window cancelled the turn"),

            "session.get" => Ok(json!({
                "id": "default", "model": "base", "status": "working", "messages": [],
                "queue": [{"id": "q1", "text": "later"}],
            })),

            _ => super::mock_control(home, method, params).await,
        }
    }

    let (mut input, reader) = duplex(4096);

    let (writer, mut output) = duplex(64 * 1024);
    let (_interrupt, interrupts) = tokio::sync::mpsc::unbounded_channel();
    let (events, mut notifications) = tokio::sync::mpsc::unbounded_channel();

    let engine = tokio::spawn(run_with(
        reader,
        writer,
        config(),
        host,
        EngineEvents { engine: Some(events), ..Default::default() },
        interrupts,
    ));

    input.write_all(b"first\n").await.unwrap();
    until(&mut notifications, |event| *event == EngineEvent::GenerationStarted).await;
    input.write_all(b"/queue\n").await.unwrap();

    let listed =
        until(&mut notifications, |event| matches!(event, EngineEvent::ControlNotice(_))).await;

    assert_eq!(
        listed.last().unwrap(),
        &EngineEvent::ControlNotice(
            "Queue:\n1. later\nUse /unqueue <position> to cancel one.".into()
        )
    );

    input.write_all(b"/quit\n").await.unwrap();
    tokio::time::timeout(WAIT, engine).await.unwrap().unwrap().unwrap();

    let mut text = String::new();

    output.read_to_string(&mut text).await.unwrap();

    assert!(text.contains(DETACHED));
}

#[tokio::test]
async fn the_interface_wakes_an_idle_engine_to_start_another_windows_queue() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);

    async fn host(home: String, method: String, params: Value) -> krabb_core::Result<Value> {
        match method.as_str() {
            "session.next" if CALLS.fetch_add(1, Ordering::SeqCst) != 1 => {
                Ok(json!({"message": null}))
            }

            "session.next" => Ok(json!({"owner": "owner", "message": {
                "id": "other-window", "session": "default", "role": "user", "sender": "tui",
                "content": [{"kind": "text", "text": "from the other window"}]
            }})),

            _ => super::mock_control(home, method, params).await,
        }
    }

    let (mut input, reader) = duplex(4096);

    let (writer, _output) = duplex(64 * 1024);
    let (_interrupt, interrupts) = tokio::sync::mpsc::unbounded_channel();
    let (events, mut notifications) = tokio::sync::mpsc::unbounded_channel();
    let (wake, wakes) = tokio::sync::mpsc::unbounded_channel();

    let engine = tokio::spawn(run_with(
        reader,
        writer,
        config(),
        host,
        EngineEvents { engine: Some(events), wake: Some(wakes), ..Default::default() },
        interrupts,
    ));

    for _ in 0..50 {
        wake.send(()).unwrap();
    }

    let seen =
        until(&mut notifications, |event| matches!(event, EngineEvent::GenerationFinished { .. }))
            .await;

    assert!(seen.contains(&EngineEvent::QueueStarted("from the other window".into())));

    drop(wake);
    input.write_all(b"/quit\n").await.unwrap();
    tokio::time::timeout(WAIT, engine).await.unwrap().unwrap().unwrap();
}
