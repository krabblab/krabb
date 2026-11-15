use super::*;
use futures_util::StreamExt;
use krabb_core::{plugin::Emitter, types::Response};
use serde_json::json;

async fn run(
    frames: Vec<(u64, serde_json::Value)>,
    limits: Limits,
) -> krabb_core::Result<Option<Response>> {
    let stream = futures_util::stream::unfold(frames.into_iter(), |mut frames| async move {
        let (delay, frame) = frames.next()?;
        tokio::time::sleep(Duration::from_millis(delay)).await;
        Some((Ok::<_, std::io::Error>(format!("{frame}\n").into_bytes()), frames))
    })
    .boxed();

    let (output, _events) = tokio::sync::mpsc::channel(32);
    let mut emitter = Emitter::new(output);

    crate::live_stream_with_watch(1, stream, &mut emitter, Some(Watch::new(1, limits)?)).await
}

fn limits() -> Limits {
    Limits { first_millis: 100, idle_millis: 40, total_millis: 300, ceiling_millis: None }
}

#[tokio::test(start_paused = true)]
async fn silent_prefill_is_admitted_but_heartbeats_do_not_keep_it_alive() {
    let response = run(
        vec![
            (90, json!({"message":{"content":"Hello"},"done":false})),
            (30, json!({"message":{"content":"!"},"done":true})),
        ],
        limits(),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(response.result.unwrap()["text"], "Hello!");

    let error = run(
        vec![
            (40, json!({"done":false})),
            (40, json!({"done":false})),
            (40, json!({"message":{"content":"Too late"},"done":true})),
        ],
        limits(),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        &error,
        Error::Failed { failure, .. } if failure.kind == krabb_core::failure::Kind::TimedOut
    ));

    assert!(error.to_string().contains("first-output"));
}

#[tokio::test(start_paused = true)]
async fn repeated_tool_snapshots_and_stalled_output_expire() {
    let tool = json!({"message":{"tool_calls":[{
        "function":{"name":"read","arguments":{"path":"report"}}
    }]},"done":false});

    let error =
        run(vec![(10, tool.clone()), (30, tool.clone()), (30, tool)], limits()).await.unwrap_err();

    assert!(error.to_string().contains("inactivity"));

    let error = run(
        vec![
            (10, json!({"message":{"content":"First token"},"done":false})),
            (50, json!({"done":true})),
        ],
        limits(),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("inactivity"));
}

#[tokio::test(start_paused = true)]
async fn thinking_and_changed_tools_count_as_progress_but_never_extend_total_time() {
    let tool = |path: &str| {
        json!({"message":{"tool_calls":[{
        "function":{"name":"read","arguments":{"path":path}}
    }]},"done":false})
    };

    let response = run(
        vec![
            (90, json!({"message":{"thinking":"private computation"},"done":false})),
            (30, tool("first")),
            (30, tool("second")),
            (30, json!({"done":true})),
        ],
        limits(),
    )
    .await
    .unwrap()
    .unwrap();

    let result = response.result.unwrap();

    assert_eq!(result["events"][0]["args"]["path"], "second");
    assert!(!result.to_string().contains("private computation"));

    let frames = (0..11)
        .map(|_| {
            (
                30,
                json!({
                    "message":{"content":"x"},"done":false
                }),
            )
        })
        .collect();

    let error = run(frames, limits()).await.unwrap_err();

    assert!(error.to_string().contains("total"));
}

#[test]
fn deployment_is_explicit_and_observation_identity_changes_with_configuration() {
    let base = profile("https://example.test", "small", "", "1", "false", "auto").unwrap();

    assert_eq!(base.deployment, Deployment::Unknown);
    assert!(!base.cancellation);
    assert!(!base.recoverable);

    base.validate().unwrap();

    for (host, model, deployment, revision, thinking, context) in [
        ("https://elsewhere.test", "small", "", "1", "false", "auto"),
        ("https://example.test", "large", "", "1", "false", "auto"),
        ("https://example.test", "small", "local", "1", "false", "auto"),
        ("https://example.test", "small", "", "2", "false", "auto"),
        ("https://example.test", "small", "", "1", "true", "auto"),
        ("https://example.test", "small", "", "1", "false", "8192"),
    ] {
        let changed = profile(host, model, deployment, revision, thinking, context).unwrap();

        assert_ne!(base.identity, changed.identity);
        assert!(!serde_json::to_string(&changed).unwrap().contains(host));
    }

    assert_eq!(
        profile("http://127.0.0.1", "small", "remote", "1", "false", "auto").unwrap().deployment,
        Deployment::Remote
    );

    assert!(profile("local-host", "small", "guessed", "1", "false", "auto").is_err());
}

#[tokio::test]
async fn admitted_http_timeout_overrides_the_clients_shorter_default_for_both_paths() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    if !krabb_test::inbox::isolated(
        "execution::tests::admitted_http_timeout_overrides_the_clients_shorter_default_for_both_paths",
        &[("KRABB_KEYRING", "0"), ("KRABB_CREDENTIALS", ""), ("KRABB_OLLAMA_API_KEY", "")],
    ).await { return; }

    for stream in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());

        let server =
            tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = vec![0; 4096];
                let size = socket.read(&mut bytes).await.unwrap();

                assert!(size > 0);
                tokio::time::sleep(Duration::from_millis(40)).await;
                let mut body =
                    json!({"message":{"content":"Healthy slow reply"},"done":true}).to_string();

                if stream {
                    body.push('\n');
                }

                socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ).as_bytes()).await.unwrap();
            });

        let client = reqwest::Client::builder().timeout(Duration::from_millis(5)).build().unwrap();

        let mut input: ModelRequest = serde_json::from_value(json!({
            "model":"fixture", "messages":[], "stream":stream,
        }))
        .unwrap();

        input.stream = stream;

        input.generation = Some(krabb_core::context::Generation {
            output: 512,
            summary: false,
            search: false,
            limits: Some(Limits {
                first_millis: 2_000,
                idle_millis: 1_000,
                total_millis: 3_000,
                ceiling_millis: None,
            }),
        });

        let (output, _events) = tokio::sync::mpsc::channel(32);
        let mut emitter = Emitter::new(output);

        let reply =
            crate::generate_at(&client, 1, input, &host, &mut emitter).await.unwrap().unwrap();

        assert_eq!(reply.result.unwrap()["text"], "Healthy slow reply");

        server.await.unwrap();
    }
}

#[tokio::test]
async fn protocol_metadata_and_invalid_deadlines_do_not_contact_the_backend() {
    let client = reqwest::Client::new();
    let (output, _events) = tokio::sync::mpsc::channel(32);
    let mut emitter = Emitter::new(output);

    let metadata = crate::generate(
        &client,
        krabb_core::types::Request::call(
            17,
            krabb_core::execution::INFO,
            json!({"model":"fixture"}),
        ),
        emitter.clone(),
    )
    .await
    .unwrap()
    .unwrap();

    let profile: Profile = serde_json::from_value(metadata.result.unwrap()).unwrap();

    profile.validate().unwrap();

    assert!(profile.deadlines);
    assert!(!profile.cancellation);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());

    let input: ModelRequest = serde_json::from_value(json!({
        "model":"fixture", "messages":[], "stream":true,
        "generation":{"output":512,"limits":{
            "first_millis":0,"idle_millis":10,"total_millis":20
        }}
    }))
    .unwrap();

    assert!(crate::generate_at(&client, 1, input, &host, &mut emitter).await.is_err());
    assert!(tokio::time::timeout(Duration::from_millis(20), listener.accept()).await.is_err());
}

#[tokio::test]
async fn http_header_waits_and_body_stalls_use_the_admitted_deadlines() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    if !krabb_test::inbox::isolated(
        "execution::tests::http_header_waits_and_body_stalls_use_the_admitted_deadlines",
        &[("KRABB_KEYRING", "0"), ("KRABB_CREDENTIALS", ""), ("KRABB_OLLAMA_API_KEY", "")],
    )
    .await
    {
        return;
    }

    for (stream, headers) in [(true, true), (false, true), (true, false)] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();

            loop {
                let mut chunk = [0; 1024];
                let size = socket.read(&mut chunk).await.unwrap();

                assert!(size > 0);
                received.extend_from_slice(&chunk[..size]);

                assert!(received.len() < 4096);
                let Some(end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
                    continue;
                };

                let text = String::from_utf8_lossy(&received[..end]);
                let size: usize = text
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .unwrap()
                    .1
                    .trim()
                    .parse()
                    .unwrap();

                if received.len() < end + 4 + size {
                    continue;
                }

                let body: serde_json::Value =
                    serde_json::from_slice(&received[end + 4..end + 4 + size]).unwrap();
                assert!(body.get("generation").is_none());
                assert!(body.get("limits").is_none());
                break;
            }

            let first = format!("{}\n", json!({"message":{"content":"First"},"done":false}));
            let last = format!("{}\n", json!({"message":{"content":"Late"},"done":true}));
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                first.len() + last.len(),
            );

            if headers {
                tokio::time::sleep(Duration::from_millis(1_500)).await;
            }

            let _ = socket.write_all(format!("{head}{first}").as_bytes()).await;

            if !headers {
                tokio::time::sleep(Duration::from_millis(1_500)).await;
            }

            let _ = socket.write_all(last.as_bytes()).await;
        });

        let input: ModelRequest = serde_json::from_value(json!({
            "model":"fixture", "messages":[], "stream":stream,
            "generation":{"output":512,"limits":{
                "first_millis":1_000,"idle_millis":500,"total_millis":5_000
            }}
        }))
        .unwrap();

        let client = reqwest::Client::new();
        let (output, _events) = tokio::sync::mpsc::channel(32);
        let mut emitter = Emitter::new(output);
        let error = crate::generate_at(&client, 1, input, &host, &mut emitter).await.unwrap_err();

        assert!(matches!(
            &error,
            Error::Failed { failure, .. } if failure.kind == krabb_core::failure::Kind::TimedOut
        ));

        let reason = if headers { "first-output" } else { "inactivity" };

        assert!(error.to_string().contains(reason));

        tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn final_stream_and_plain_responses_normalize_phase_timings_without_prompt_contents() {
    let frame = json!({
        "done":true, "message":{"content":"Hello!"},
        "load_duration":1_000_001, "prompt_eval_duration":20_000_000,
        "eval_duration":30_000_000, "prompt_eval_count":100, "eval_count":2,
    });

    let plain = crate::response_body(7, frame.clone()).unwrap().unwrap().result.unwrap();

    let streamed = run(vec![(0, frame)], limits()).await.unwrap().unwrap().result.unwrap();

    for value in [plain, streamed] {
        let reply: krabb_core::types::ModelReply = serde_json::from_value(value).unwrap();

        assert_eq!(reply.text, "Hello!");
        assert_eq!(reply.input, Some(100));
        assert_eq!(reply.output, Some(2));

        assert_eq!(
            reply.timing,
            Some(krabb_core::execution::Timing {
                load_millis: Some(2),
                prefill_millis: Some(20),
                decode_millis: Some(30),
            })
        );
    }
}

#[test]
fn missing_timings_and_cached_prefill_are_not_measurements_of_zero_or_fresh_prompt_speed() {
    assert_eq!(timing(&json!({})).unwrap(), None);
    assert_eq!(timing(&json!({"load_duration":0})).unwrap().unwrap().load_millis, Some(0));

    let cached = timing(&json!({
        "prompt_eval_duration":1_000_000, "prompt_eval_cached_count":100,
        "eval_duration":2_000_000,
    }))
    .unwrap()
    .unwrap();

    assert_eq!(cached.prefill_millis, None);
    assert_eq!(cached.decode_millis, Some(2));

    for value in [
        json!({"eval_duration":-1}),
        json!({"load_duration":"100"}),
        json!({"eval_duration":u64::MAX}),
    ] {
        assert!(timing(&value).is_err());
    }
}

#[tokio::test]
async fn malformed_terminal_phase_reports_are_rejected_by_both_response_paths() {
    let frame = json!({"done":true,"message":{"content":"Hello"},"eval_duration":"invalid"});

    assert!(crate::response_body(1, frame.clone()).is_err());
    assert!(run(vec![(0, frame)], limits()).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn a_renewal_extends_a_progressing_stream_only_within_its_ceiling() {
    let frames = (0..15)
        .map(|index| (30, json!({"message":{"content":"x"},"done":index == 14})))
        .collect::<Vec<_>>();

    let renewable = Limits {
        first_millis: 100,
        idle_millis: 100,
        total_millis: 300,
        ceiling_millis: Some(600),
    };

    for renewed in [false, true] {
        let stream =
            futures_util::stream::unfold(frames.clone().into_iter(), |mut frames| async move {
                let (delay, frame) = frames.next()?;
                tokio::time::sleep(Duration::from_millis(delay)).await;
                Some((Ok::<_, std::io::Error>(format!("{frame}\n").into_bytes()), frames))
            })
            .boxed();

        let (output, _events) = tokio::sync::mpsc::channel(32);
        let mut emitter = Emitter::new(output);
        let watch = Watch::new(4242, renewable).unwrap();
        let generation = crate::live_stream_with_watch(4242, stream, &mut emitter, Some(watch));

        tokio::pin!(generation);

        let result = tokio::select! {
            result = &mut generation => result,

            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if renewed {
                    renew(&json!({"id":4242,"total_millis":600})).unwrap();
                }

                generation.await
            }
        };

        if renewed {
            assert_eq!(result.unwrap().unwrap().result.unwrap()["text"], "x".repeat(15));
        } else {
            assert!(result.unwrap_err().to_string().contains("total allowance"));
        }
    }

    assert!(renew(&json!({"id":4242,"total_millis":600})).is_ok());
    assert!(renew(&json!({"id":4242})).is_err());
}

#[tokio::test]
async fn renewed_streams_and_slow_plain_replies_finish_within_their_admitted_bounds() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    if !krabb_test::inbox::isolated(
        "execution::tests::renewed_streams_and_slow_plain_replies_finish_within_their_admitted_bounds",
        &[("KRABB_KEYRING", "0"), ("KRABB_CREDENTIALS", ""), ("KRABB_OLLAMA_API_KEY", "")],
    ).await { return; }

    for stream in [true, false] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 4096];

            assert!(socket.read(&mut bytes).await.unwrap() > 0);

            if !stream {
                tokio::time::sleep(Duration::from_millis(250)).await;

                let body =
                    json!({"message":{"content":"Slow plain reply"},"done":true}).to_string();

                socket.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ).as_bytes()).await.unwrap();
                return;
            }

            socket.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n").await.unwrap();

            for index in 0..15 {
                tokio::time::sleep(Duration::from_millis(30)).await;

                let frame = json!({"message":{"content":"x"},"done":index == 14});

                if socket.write_all(format!("{frame}\n").as_bytes()).await.is_err() {
                    return;
                }
            }
        });

        let mut input: ModelRequest = serde_json::from_value(json!({
            "model":"fixture", "messages":[], "stream":stream,
        }))
        .unwrap();

        input.stream = stream;

        input.generation = Some(krabb_core::context::Generation {
            output: 512,
            summary: false,
            search: false,
            limits: Some(if stream {
                Limits {
                    first_millis: 200,
                    idle_millis: 200,
                    total_millis: 300,
                    ceiling_millis: Some(900),
                }
            } else {
                Limits {
                    first_millis: 1_000,
                    idle_millis: 1_000,
                    total_millis: 1_000,
                    ceiling_millis: None,
                }
            }),
        });

        let client = reqwest::Client::new();
        let (output, _events) = tokio::sync::mpsc::channel(64);
        let mut emitter = Emitter::new(output);
        let generation = crate::generate_at(&client, 7, input, &host, &mut emitter);

        tokio::pin!(generation);

        let reply = tokio::select! {
            reply = &mut generation => reply,

            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if stream {
                    renew(&json!({"id":7,"total_millis":900})).unwrap();
                }

                generation.await
            }
        };

        let expected = if stream { "x".repeat(15) } else { "Slow plain reply".into() };

        assert_eq!(reply.unwrap().unwrap().result.unwrap()["text"], expected);

        server.await.unwrap();
    }
}

#[tokio::test]
async fn a_finished_watch_leaves_a_newer_renewal_with_the_same_id() {
    let limits = Limits { ceiling_millis: Some(1_000), ..limits() };
    let older = Watch::new(41, limits).unwrap();
    let mut newer = Watch::new(41, limits).unwrap();

    drop(older);

    renew(&json!({"id":41,"total_millis":900})).unwrap();
    newer.observe(true).unwrap();

    assert_eq!(newer.timer.total(), 900);
}

#[tokio::test]
async fn a_renewal_past_the_admitted_ceiling_is_clamped_instead_of_failing() {
    let limits = Limits { ceiling_millis: Some(1_000), ..limits() };
    let mut watch = Watch::new(43, limits).unwrap();

    renew(&json!({"id":43,"total_millis":1_005})).unwrap();
    watch.observe(true).unwrap();

    assert_eq!(watch.timer.total(), 1_000);
}
