use krabb_core::{Error, Result, execution::Envelope};
use serde_json::Value;
use tokio::time::{Duration, Instant};

/// Extra time for the final frames after the daemon's advertised allowance ends.
const GRACE: Duration = Duration::from_secs(10);

pub(super) struct Deadline {
    started: Instant,
    expires: Instant,
    envelope: Option<Envelope>,
}

impl Deadline {
    pub fn new(fallback: Duration) -> Self {
        let started = Instant::now();

        Self { started, expires: started + fallback, envelope: None }
    }

    pub fn expires(&self) -> Instant {
        self.expires
    }

    pub fn check(&self) -> Result<()> {
        if Instant::now() >= self.expires {
            return Err(expired());
        }

        Ok(())
    }

    pub fn accept(&mut self, value: &Value) -> Result<()> {
        self.check()?;

        let envelope: Envelope = serde_json::from_value(value.clone())
            .map_err(|_| Error::Protocol("Invalid execution envelope.".into()))?;

        envelope.validate()?;

        if let Some(previous) = self.envelope {
            if previous != envelope {
                return Err(Error::Protocol("The execution envelope changed.".into()));
            }

            return Ok(());
        }

        // Delivery grace does not renew when progress or repeated metadata arrives.
        self.expires = self.started + Duration::from_millis(envelope.total_millis) + GRACE;
        self.envelope = Some(envelope);

        Ok(())
    }
}

pub(super) fn expired() -> Error {
    Error::Denied("The daemon did not respond in time.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test(start_paused = true)]
    async fn a_slow_admitted_answer_outlives_the_legacy_client_timeout() {
        let mut deadline = Deadline::new(super::super::STREAMING_TURN_TIMEOUT);
        deadline.accept(&json!({"total_millis": 900_000})).unwrap();

        let waiting = tokio::spawn(async move {
            tokio::time::timeout_at(deadline.expires(), async {
                tokio::time::sleep(Duration::from_secs(600)).await;
                "exact answer"
            })
            .await
        });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(311)).await;

        assert!(!waiting.is_finished());
        assert_eq!(waiting.await.unwrap().unwrap(), "exact answer");
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_metadata_and_progress_cannot_renew_an_absolute_allowance() {
        let mut deadline = Deadline::new(super::super::STREAMING_TURN_TIMEOUT);
        let budget = json!({"total_millis": 600_000});
        deadline.accept(&budget).unwrap();
        let original = deadline.expires();

        for _ in 0..6 {
            tokio::time::advance(Duration::from_secs(100)).await;
            deadline.accept(&budget).unwrap();

            assert_eq!(deadline.expires(), original);
        }

        assert!(deadline.accept(&json!({"total_millis": 900_000})).is_err());
        tokio::time::advance(Duration::from_secs(11)).await;

        assert!(Instant::now() > deadline.expires());
        assert!(deadline.accept(&budget).is_err());
        assert!(deadline.check().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn legacy_daemons_keep_the_fallback_and_control_requests_keep_their_short_limit() {
        for wait in [super::super::STREAMING_TURN_TIMEOUT, super::super::CONTROL_TIMEOUT] {
            let deadline = Deadline::new(wait);
            let result =
                tokio::time::timeout_at(deadline.expires(), std::future::pending::<()>()).await;

            assert!(result.is_err());
        }
    }

    #[test]
    fn malformed_or_unbounded_envelopes_do_not_change_the_deadline() {
        let mut deadline = Deadline::new(super::super::STREAMING_TURN_TIMEOUT);
        let original = deadline.expires();

        for value in [
            json!(null),
            json!({"total_millis": 0}),
            json!({"total_millis": -1}),
            json!({"total_millis": u64::MAX}),
            json!({"total_millis": 10, "renew": true}),
        ] {
            assert!(deadline.accept(&value).is_err());
            assert_eq!(deadline.expires(), original);
        }
    }
}

#[cfg(test)]
mod wire {
    use super::*;
    use krabb_core::{
        jsonl,
        types::{IpcRequest, IpcResponse},
    };

    use serde_json::json;
    use tokio::{io::BufReader, net::TcpListener};

    #[tokio::test]
    async fn the_real_client_honors_negotiated_and_legacy_deadlines_without_displaying_metadata() {
        for (budget, elapsed, succeeds) in
            [(Some(900_000), 600, true), (None, 311, false), (Some(600_000), 611, false)]
        {
            let root = std::env::temp_dir().join(format!(
                "krabb-envelope-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            std::fs::create_dir_all(root.join("run")).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            std::fs::write(root.join("run/ipc.token"), "fixture").unwrap();
            std::fs::write(
                root.join("run/ipc.port"),
                listener.local_addr().unwrap().port().to_string(),
            )
            .unwrap();
            let (release, resume) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let (input, mut output) = socket.into_split();
                let request: IpcRequest =
                    jsonl::read(&mut BufReader::new(input), jsonl::MAX).await.unwrap().unwrap();
                assert_eq!(request.params[krabb_core::execution::ENVELOPE], true);

                if let Some(total_millis) = budget {
                    jsonl::write(
                        &mut output,
                        &IpcResponse::ok(
                            request.id,
                            json!({
                                "event": krabb_core::execution::ENVELOPE,
                                "budget": {"total_millis": total_millis},
                            }),
                        ),
                    )
                    .await
                    .unwrap();
                }

                jsonl::write(
                    &mut output,
                    &IpcResponse::ok(request.id, json!({"event":"text", "text":"Hello"})),
                )
                .await
                .unwrap();
                let _ = resume.await;
                let _ = jsonl::write(
                    &mut output,
                    &IpcResponse::ok(request.id, json!({"done":true,"text":"Hello!"})),
                )
                .await;
            });

            let home = root.to_str().unwrap().to_owned();
            let (events, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let client = tokio::spawn(async move {
                super::super::control_with_stream(
                    &home,
                    "session.answer",
                    json!({"id":"main"}),
                    Some(events),
                )
                .await
            });

            let first =
                tokio::time::timeout(Duration::from_secs(5), receiver.recv()).await.unwrap();
            assert!(matches!(
                first,
                Some(super::super::EngineEvent::AssistantText(ref text)) if text == "Hello"
            ));

            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(elapsed)).await;
            tokio::task::yield_now().await;
            tokio::time::resume();

            if succeeds {
                assert!(!client.is_finished());
            }

            release.send(()).unwrap();
            let result =
                tokio::time::timeout(Duration::from_secs(5), client).await.unwrap().unwrap();
            assert_eq!(result.is_ok(), succeeds);

            if succeeds {
                assert!(matches!(
                    receiver.recv().await,
                    Some(super::super::EngineEvent::AssistantText(ref text)) if text == "!"
                ));
            } else {
                assert!(matches!(result, Err(super::super::ControlError::Failed(_))));
            }

            assert!(receiver.try_recv().is_err());
            server.await.unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
