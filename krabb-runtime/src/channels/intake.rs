use super::*;

const RETRY_DELAY: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_millis(100);
const IDLE: Duration = Duration::from_secs(1);
const STOP_GRACE: Duration = Duration::from_secs(5);

pub(crate) const UNACKNOWLEDGED: &str = "The channel did not acknowledge the delivery.";

const INTERRUPTED: &str = "The turn ended before the channel acknowledged the delivery.";

pub(crate) struct Context {
    pub channel: Arc<Live>,
    pub id: String,
    pub model: String,
    pub policy: ChannelConfig,
    pub mode: ApprovalMode,
    pub sessions: Arc<Mutex<state::Store>>,
    pub approvals: Arc<AsyncMutex<approval::Gate>>,
    pub signals: Arc<jobs::Signals>,
    pub shutdown: Arc<Stop>,
    pub offset: i64,
    pub call: u64,
}

pub(crate) struct Intake {
    stop: Arc<Stop>,
    worker: Option<tokio::task::JoinHandle<Result<(), PluginError>>>,
}

impl Intake {
    pub fn start(context: Context) -> Self {
        let stop = Arc::new(Stop::new());
        let signal = Arc::clone(&stop);
        let worker = tokio::spawn(run(context, signal));

        Self { stop, worker: Some(worker) }
    }

    pub async fn finish(mut self) -> Result<(), PluginError> {
        self.stop.signal();
        self.worker.take().unwrap().await?
    }
}

/// Stops a turn's intake and returns the channel offset it committed; an intake failure is
/// logged, since the bridge polls the same events again.
pub(crate) async fn finish(
    intake: Intake,
    sessions: &Mutex<state::Store>,
    channel: &str,
) -> Result<i64, PluginError> {
    if let Err(error) = intake.finish().await {
        warn!(channel, error = %diagnostic(error.to_string()), "Channel intake paused.");
    }

    Ok(sessions.lock().map_err(|_| crate::state::POISONED)?.offset(channel))
}

impl Drop for Intake {
    fn drop(&mut self) {
        self.stop.signal();
    }
}

async fn run(mut context: Context, stop: Arc<Stop>) -> Result<(), PluginError> {
    let shutdown = Arc::clone(&context.shutdown);
    let mut wait = TICK;

    loop {
        tokio::select! {
            biased;
            _ = stop.notified() => return Ok(()),
            _ = context.shutdown.notified() => return Ok(()),

            _ = tokio::time::sleep(wait) => {}
        }

        if let Err(error) = deliver(&mut context, &stop, &shutdown).await {
            if pause(&context, &stop, "delivery", &error.to_string()).await {
                return Ok(());
            }

            continue;
        }

        let poll = context.channel.call(Request::call(
            context.call,
            "poll",
            serde_json::json!({"offset":context.offset, "wait_seconds":1}),
        ));

        context.call += 1;

        let response = tokio::select! {
            biased;
            _ = stop.notified() => return Ok(()),
            _ = context.shutdown.notified() => return Ok(()),
            response = poll => response,
        };

        let value = response.map_err(|error| error.to_string()).and_then(|response| {
            response.result.ok_or_else(|| {
                response.error.map_or_else(
                    || "The channel returned an empty poll response.".to_owned(),
                    |error| error.message,
                )
            })
        });

        let value = match value {
            Ok(value) => value,

            Err(error) => {
                if pause(&context, &stop, "poll", &error).await {
                    return Ok(());
                }

                continue;
            }
        };

        let events = value["events"].as_array().cloned().unwrap_or_default();

        wait = if events.is_empty() { IDLE } else { TICK };

        let Some(received) = bounded(receive(&mut context, events), &stop, &shutdown).await else {
            return Ok(());
        };

        if let Err(error) = received {
            if pause(&context, &stop, "intake", &error.to_string()).await {
                return Ok(());
            }

            continue;
        }

        if let Err(error) = deliver(&mut context, &stop, &shutdown).await
            && pause(&context, &stop, "delivery", &error.to_string()).await
        {
            return Ok(());
        }
    }
}

/// Lets a step finish unless the intake stops; a step under way then gets a short grace, so a
/// stalled channel cannot hold the turn's completion.
async fn bounded<T>(step: impl Future<Output = T>, stop: &Stop, shutdown: &Stop) -> Option<T> {
    tokio::pin!(step);

    let stopped = async {
        tokio::select! {
            _ = stop.notified() => {}

            _ = shutdown.notified() => {}
        }
    };

    tokio::select! {
        biased;
        value = &mut step => Some(value),
        () = stopped => tokio::time::timeout(STOP_GRACE, step).await.ok(),
    }
}

/// Logs a failed step and waits before retrying; reports whether the intake must stop.
async fn pause(context: &Context, stop: &Stop, step: &str, error: &str) -> bool {
    warn!(
        channel = %context.id,
        step,
        error = %diagnostic(sentence(error.to_owned())),
        "Channel intake failed during a turn; retrying."
    );

    tokio::select! {
        biased;
        _ = stop.notified() => true,
        _ = context.shutdown.notified() => true,
        _ = tokio::time::sleep(RETRY_DELAY) => false,
    }
}

/// Queues messages until one must wait, then still answers later approval callbacks without
/// committing the offset past the held message, so a full queue cannot strand an approval.
async fn receive(context: &mut Context, events: Vec<Value>) -> Result<(), PluginError> {
    let mut blocked = false;

    for event in events {
        if event["kind"] == "callback" {
            handle_callback(
                &context.channel,
                &context.id,
                &event,
                &context.policy,
                context.mode,
                &context.approvals,
                &context.sessions,
                &mut context.offset,
                &mut context.call,
                !blocked,
            )
            .await?;
        } else if !blocked
            && !queue_during_turn(
                &context.channel,
                &context.id,
                &context.model,
                &event,
                &context.policy,
                &context.sessions,
                &mut context.offset,
                &mut context.call,
                Some(&context.signals),
            )
            .await?
        {
            blocked = true;
        }
    }

    Ok(())
}

async fn deliver(context: &mut Context, stop: &Stop, shutdown: &Stop) -> Result<(), PluginError> {
    let pending = {
        let mut store = context.sessions.lock().map_err(|_| crate::state::POISONED)?;
        let pending = store
            .outbox
            .iter()
            .find(|delivery| {
                delivery.channel == context.id && delivery.status == state::DeliveryStatus::Pending
            })
            .cloned();

        match pending {
            Some(delivery) => {
                store.sending(&delivery.id)?;
                request(&mut store, &context.id, context.call, &delivery)?
                    .map(|request| (delivery, request))
            }

            None => None,
        }
    };

    let Some((delivery, request)) = pending else { return Ok(()) };

    context.call += 1;

    let response = bounded(context.channel.call(request), stop, shutdown).await;
    let mut store = context.sessions.lock().map_err(|_| crate::state::POISONED)?;

    match response {
        Some(Ok(response)) => {
            settle(&mut store, &delivery.id, response)?;
        }

        Some(Err(error)) => store.uncertain(&delivery.id, sentence(error.to_string()))?,
        None => store.uncertain(&delivery.id, INTERRUPTED)?,
    }

    Ok(())
}

/// Builds a claimed delivery's request; one that cannot be encoded is left uncertain with its
/// reason, so it is reported instead of staying claimed.
pub(crate) fn request(
    store: &mut state::Store,
    channel: &str,
    call: u64,
    delivery: &state::Delivery,
) -> std::io::Result<Option<Request>> {
    match delivery_request(channel, call, delivery) {
        Ok(request) => Ok(Some(request)),

        Err(error) => {
            let message = sentence(error.to_string());

            warn!(channel, error = %diagnostic(&message), "Channel delivery is uncertain.");
            store.uncertain(&delivery.id, message)?;
            Ok(None)
        }
    }
}

/// Records a channel's answer to a delivery and returns the reason when it stays uncertain.
pub(crate) fn settle(
    store: &mut state::Store,
    id: &str,
    response: krabb_core::types::Response,
) -> std::io::Result<Option<String>> {
    if let (None, Some(result)) = (&response.error, &response.result) {
        store.delivered(id, result)?;
        return Ok(None);
    }

    let message =
        response.error.map_or_else(|| UNACKNOWLEDGED.to_owned(), |error| sentence(error.message));

    store.uncertain(id, message.clone())?;
    Ok(Some(message))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use krabb_core::plugin::Process;
    use krabb_core::types::{Content, Message, Role};

    const CHANNEL: &str = r#"
while IFS= read -r line; do
    request_id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')

    case "$line" in
        *'"method":"hello"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocol":{"major":0,"minor":1},"id":"telegram","version":"0.1.0","capabilities":["channel"]}}\n' "$request_id"
            ;;
        *'"method":"send"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"message_id":1}}\n' "$request_id"
            ;;
        *'"method":"poll"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"events":[]}}\n' "$request_id"
            ;;
        *'"method":"callback"'*|*'"method":"ack"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"acknowledged":true}}\n' "$request_id"
            ;;
        *'"method":"shutdown"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"ok":true}}\n' "$request_id"
            exit 0
            ;;
    esac
done
"#;

    #[tokio::test]
    async fn approval_callbacks_after_a_held_message_resolve_without_advancing_the_offset() {
        let channel =
            Arc::new(Live::new(Process::start_with("sh", ["-c", CHANNEL]).await.unwrap()));

        let mut store = state::Store::default();

        store.ensure("telegram-7", "test").unwrap();
        store.set_status("telegram-7", "working").unwrap();

        for index in 0..state::LIMIT {
            let message = Message {
                id: format!("queued-{index}"),
                session: "telegram-7".into(),
                role: Role::User,
                sender: None,
                content: vec![Content::Text { text: "queued".into() }],
            };

            store.queue("telegram-7", message).unwrap();
            store
                .defer_event("telegram", serde_json::json!({"id": format!("deferred-{index}")}))
                .unwrap();
        }

        let approvals = Arc::new(AsyncMutex::new(approval::Gate::new().unwrap()));

        let target = approval::Target {
            source: krabb_core::scope::Principal::Channel("telegram".into()),
            channel: "telegram".into(),
            chat: "7".into(),
            thread: None,
            session: "telegram-7".into(),
            tool: "write".into(),
            args: serde_json::json!({}),
        };

        let challenge = approvals.lock().await.issue(target).unwrap();
        let home = std::env::temp_dir().join(format!("krabb-intake-held-{}", std::process::id()));

        let mut context = Context {
            channel: Arc::clone(&channel),
            id: "telegram".into(),
            model: "test".into(),
            policy: ChannelConfig { allow: vec!["7".into()], tools: true, ..Default::default() },
            mode: ApprovalMode::Prompt,
            sessions: Arc::new(Mutex::new(store)),
            approvals,
            signals: Arc::default(),
            shutdown: Arc::new(Stop::new()),
            offset: 0,
            call: 1,
        };

        let events = vec![
            serde_json::json!({"id":100,"chat":7,"private":true,"text":"defer me"}),
            serde_json::json!({"kind":"callback","id":101,"chat":7,"private":true,"sender":7,
                "callback_id":"callback-1","data":challenge.approve}),
        ];

        receive(&mut context, events).await.unwrap();

        let answer = tokio::time::timeout(Duration::from_secs(2), challenge.answer).await;

        assert!(answer.unwrap().unwrap());

        {
            let store = context.sessions.lock().unwrap();

            assert_eq!(context.offset, 0);
            assert!(!store.known("telegram", "100"));
            assert!(store.known("telegram", "101"));
            assert_eq!(store.offset("telegram"), 0);
        }

        channel.stop().await.ok();

        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn a_failed_save_pauses_intake_and_the_delivery_follows_once_it_recovers() {
        let channel =
            Arc::new(Live::new(Process::start_with("sh", ["-c", CHANNEL]).await.unwrap()));

        let root = crate::tests::test_root("intake-recovery");
        let path = root.join("data/sessions/index.json");
        let mut store = state::Store::load(&path).unwrap();

        store.queue_delivery("answer", "telegram", "7", true, None, "Done.").unwrap();

        let directory = path.parent().unwrap().to_path_buf();
        let backup = directory.with_extension("backup");

        std::fs::rename(&directory, &backup).unwrap();
        std::fs::write(&directory, "blocked").unwrap();

        let sessions = Arc::new(Mutex::new(store));

        let intake = Intake::start(Context {
            channel: Arc::clone(&channel),
            id: "telegram".into(),
            model: "test".into(),
            policy: ChannelConfig { allow: vec!["7".into()], ..Default::default() },
            mode: ApprovalMode::Prompt,
            sessions: Arc::clone(&sessions),
            approvals: Arc::new(AsyncMutex::new(approval::Gate::new().unwrap())),
            signals: Arc::default(),
            shutdown: Arc::new(Stop::new()),
            offset: 0,
            call: 1,
        });

        tokio::time::sleep(Duration::from_millis(300)).await;

        assert_eq!(sessions.lock().unwrap().outbox.len(), 1);

        std::fs::remove_file(&directory).unwrap();
        std::fs::rename(&backup, &directory).unwrap();

        let delivered = tokio::time::timeout(Duration::from_secs(5), async {
            while !sessions.lock().unwrap().outbox.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;

        intake.finish().await.unwrap();
        channel.stop().await.ok();

        assert!(delivered.is_ok());

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_stalled_delivery_does_not_hold_the_turn_past_its_grace() {
        let stalled = CHANNEL.replace(
            "printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"message_id\":1}}\\n' \"$request_id\"",
            "touch \"$SENT\"",
        );

        let root = crate::tests::test_root("intake-stalled-delivery");
        let sent = root.join("sent");

        std::fs::create_dir_all(&root).unwrap();

        let script = format!("SENT='{}'\n{stalled}", sent.display());

        let channel =
            Arc::new(Live::new(Process::start_with("sh", ["-c", script.as_str()]).await.unwrap()));

        let mut store = state::Store::default();

        store.queue_delivery("answer", "telegram", "7", true, None, "Done.").unwrap();

        let sessions = Arc::new(Mutex::new(store));

        let intake = Intake::start(Context {
            channel: Arc::clone(&channel),
            id: "telegram".into(),
            model: "test".into(),
            policy: ChannelConfig { allow: vec!["7".into()], ..Default::default() },
            mode: ApprovalMode::Prompt,
            sessions: Arc::clone(&sessions),
            approvals: Arc::new(AsyncMutex::new(approval::Gate::new().unwrap())),
            signals: Arc::default(),
            shutdown: Arc::new(Stop::new()),
            offset: 0,
            call: 1,
        });

        tokio::time::timeout(Duration::from_secs(5), async {
            while !sent.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        let started = std::time::Instant::now();

        intake.finish().await.unwrap();
        channel.stop().await.ok();

        assert!(started.elapsed() < STOP_GRACE + Duration::from_secs(2));

        let store = sessions.lock().unwrap();

        assert_eq!(store.outbox[0].status, state::DeliveryStatus::Uncertain);
        assert_eq!(store.outbox[0].last_error.as_deref(), Some(INTERRUPTED));

        drop(store);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_delivery_that_cannot_be_encoded_is_left_uncertain() {
        let mut store = state::Store::default();

        store.queue_delivery("d", "telegram", "not-a-chat", true, None, "Hello.").unwrap();
        store.sending("d").unwrap();

        let delivery = store.outbox[0].clone();

        assert!(request(&mut store, "telegram", 1, &delivery).unwrap().is_none());
        assert_eq!(store.outbox[0].status, state::DeliveryStatus::Uncertain);
        assert!(store.outbox[0].last_error.as_deref().unwrap().contains("chat ID"));
    }

    #[test]
    fn an_empty_channel_answer_leaves_the_delivery_uncertain() {
        let mut store = state::Store::default();

        store.queue_delivery("d", "telegram", "7", true, None, "Hello.").unwrap();
        store.sending("d").unwrap();

        let response = |result: Option<Value>| krabb_core::types::Response {
            jsonrpc: "2.0".into(),
            id: 1,
            result,
            error: None,
        };

        let reason = settle(&mut store, "d", response(None)).unwrap();

        assert_eq!(reason.as_deref(), Some(UNACKNOWLEDGED));
        assert_eq!(store.outbox[0].status, state::DeliveryStatus::Uncertain);
        assert_eq!(store.outbox[0].last_error.as_deref(), Some(UNACKNOWLEDGED));

        store.sending("d").unwrap();

        let reason = settle(&mut store, "d", response(Some(serde_json::json!({"id":1})))).unwrap();

        assert_eq!(reason, None);

        assert_ne!(
            store.outbox.first().map(|delivery| &delivery.status),
            Some(&state::DeliveryStatus::Uncertain)
        );
    }
}
