use super::*;
use serde_json::json;

fn profile() -> krabb_core::context::Profile {
    krabb_core::context::Profile {
        window: Some(krabb_core::context::Window { tokens: 4096, output: 2048 }),
        compact: false,
        search: false,
        execution: Some(execution::Profile {
            deployment: execution::Deployment::Local,
            identity: "fixture-v1".into(),
            streaming: true,
            cancellation: false,
            usage: true,
            recoverable: false,
            deadlines: true,
            renewal: false,
        }),
    }
}

fn learn(history: &History, profile: &krabb_core::context::Profile) {
    let reply = ModelReply {
        text: "Private prompt and reply must not be persisted.".into(),
        stop: "stop".into(),
        input: Some(1024),
        output: Some(32),
        context_usage: None,
        events: Vec::new(),
        timing: Some(execution::Timing {
            load_millis: Some(0),
            prefill_millis: Some(100),
            decode_millis: Some(3200),
        }),
    };

    for _ in 0..8 {
        history.observe("fixture", "model", profile, &reply, Duration::from_millis(3300));
    }
}

fn work() -> Work {
    Work { input: 2000, output: 512, cold: false, streaming: true }
}

#[test]
fn learned_timings_reduce_allowances_without_crossing_identity_or_configuration_boundaries() {
    let history = History::default();
    let mut profile = profile();

    let prior =
        history.plan("fixture", "model", &profile, work(), execution::MAX_MILLIS).unwrap().unwrap();

    learn(&history, &profile);

    let learned =
        history.plan("fixture", "model", &profile, work(), execution::MAX_MILLIS).unwrap().unwrap();

    assert!(learned.total_millis < prior.total_millis);

    for (provider, model) in [("other", "model"), ("fixture", "other")] {
        assert_eq!(
            history
                .plan(provider, model, &profile, work(), execution::MAX_MILLIS)
                .unwrap()
                .unwrap(),
            prior
        );
    }

    profile.execution.as_mut().unwrap().identity = "changed".into();

    assert_eq!(
        history.plan("fixture", "model", &profile, work(), execution::MAX_MILLIS).unwrap().unwrap(),
        prior
    );

    profile.execution.as_mut().unwrap().identity = "fixture-v1".into();
    profile.window.as_mut().unwrap().tokens *= 2;

    assert_eq!(
        history.plan("fixture", "model", &profile, work(), execution::MAX_MILLIS).unwrap().unwrap(),
        prior
    );
}

#[test]
fn admission_honors_hard_limits_and_does_not_reclassify_or_truncate_fast_requests() {
    let history = History::default();

    let mut request = Request::call(
        1,
        "generate",
        json!({
            "model":"model","messages":[],"stream":true,
        }),
    );

    assert!(
        history
            .prepare("fixture", "model", &profile(), true, tasks::Kind::Fast, &mut request, 1_000)
            .is_err()
    );

    let admitted = history
        .prepare(
            "fixture",
            "model",
            &profile(),
            true,
            tasks::Kind::Fast,
            &mut request,
            execution::MAX_MILLIS,
        )
        .unwrap()
        .unwrap();

    assert!(admitted.first_millis > 60_000);

    let Request::Call { params, .. } = request else { unreachable!() };

    assert_eq!(params["generation"]["output"], 2048);
    assert!(Config { max_seconds: 0, ..Config::default() }.validate().is_err());
    assert!(Config { queue_seconds: 0, ..Config::default() }.validate().is_err());
    assert!(Config { task_tokens: Some(0), ..Config::default() }.validate().is_err());
    assert!(Config { task_tokens: Some(1), ..Config::default() }.validate().is_ok());

    Config::default().validate().unwrap();

    for seconds in [0, u64::MAX] {
        assert!(Config { task_seconds: Some(seconds), ..Config::default() }.validate().is_err());
    }

    let configured = Config { task_seconds: Some(123), ..Config::default() };

    configured.validate().unwrap();

    assert_eq!(configured.task_millis(), Some(123_000));
    assert_eq!(configured.task_policy().ceiling, Some(86_400_000));

    for ceiling in [0, 122, u64::MAX] {
        let config =
            Config { task_seconds: Some(123), task_ceiling_seconds: ceiling, ..Config::default() };

        assert!(config.validate().is_err());
    }

    let bounded =
        Config { task_seconds: Some(123), task_ceiling_seconds: 123, ..Config::default() };

    bounded.validate().unwrap();

    assert_eq!(bounded.task_policy().allowance, Some(123_000));
}

#[tokio::test]
async fn timing_cache_survives_reload_and_contains_only_bounded_aggregate_data() {
    let root = crate::tests::test_root("inference-cache");

    std::fs::create_dir_all(&root).unwrap();

    let history = History::open(&root);

    learn(&history, &profile());

    let path = root.join("data/inference.json");

    let bytes = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(bytes) = std::fs::read(&path) {
                break bytes;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let text = String::from_utf8(bytes).unwrap();

    assert!(!text.contains("Private prompt"));
    assert!(!text.contains("fixture-v1"));
    assert!(!text.contains("model"));

    let restored = History::open(&root);

    assert_eq!(
        history.plan("fixture", "model", &profile(), work(), execution::MAX_MILLIS).unwrap(),
        restored.plan("fixture", "model", &profile(), work(), execution::MAX_MILLIS).unwrap()
    );

    std::fs::write(&path, b"{\"version\":999,\"entries\":{}}").unwrap();

    let rejected = History::open(&root);
    let empty = History::default();

    assert_eq!(
        rejected.plan("fixture", "model", &profile(), work(), execution::MAX_MILLIS).unwrap(),
        empty.plan("fixture", "model", &profile(), work(), execution::MAX_MILLIS).unwrap()
    );

    drop((history, restored, rejected));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn invalid_reports_cannot_poison_predictions_or_expand_cache_capacity() {
    let history = History::default();
    let mut reply = tasks::reply("Not timing data.".into());

    reply.timing = Some(execution::Timing {
        load_millis: Some(50_000),
        prefill_millis: Some(50_000),
        decode_millis: Some(50_000),
    });

    history.observe("fixture", "model", &profile(), &reply, Duration::from_millis(1));

    assert!(history.data.lock().unwrap().entries.is_empty());

    reply.timing = None;

    for index in 0..100 {
        let mut profile = profile();

        profile.execution.as_mut().unwrap().identity = format!("deployment-{index}");
        history.observe("fixture", "model", &profile, &reply, Duration::from_millis(1));
    }

    assert_eq!(history.data.lock().unwrap().entries.len(), PROFILES);
}

#[test]
fn admission_preserves_explicit_output_summary_and_timing_authority() {
    let history = History::default();
    let profile = profile();

    let mut request = Request::call(
        1,
        "generate",
        json!({
            "model":"model", "messages":[], "stream":true,
            "generation":{"output":16,"summary":true,"search":false,
                "limits":{"first_millis":300_000,"idle_millis":300_000,"total_millis":360_000}},
        }),
    );

    history
        .prepare(
            "fixture",
            "model",
            &profile,
            true,
            tasks::Kind::Fast,
            &mut request,
            execution::MAX_MILLIS,
        )
        .unwrap();

    let Request::Call { params, .. } = &request else { unreachable!() };

    assert_eq!(params["generation"]["output"], 16);
    assert_eq!(params["generation"]["summary"], true);
    assert_eq!(params["generation"]["search"], false);
    assert!(params["generation"]["limits"]["total_millis"].as_u64().unwrap() <= 360_000);

    for (output, first, total) in [(0, 200_000, 240_000), (16, 1, 240_000), (16, 1, 1)] {
        let mut denied = Request::call(
            1,
            "generate",
            json!({
                "model":"model","messages":[],"stream":true,
                "generation":{
                    "output":output,
                    "limits":{"first_millis":first,"idle_millis":first,"total_millis":total}
                },
            }),
        );

        let original = serde_json::to_value(&denied).unwrap();

        assert!(
            history
                .prepare(
                    "fixture",
                    "model",
                    &profile,
                    true,
                    tasks::Kind::Fast,
                    &mut denied,
                    execution::MAX_MILLIS
                )
                .is_err()
        );

        assert_eq!(serde_json::to_value(&denied).unwrap(), original);
    }
}

#[tokio::test(start_paused = true)]
async fn traces_record_known_outcomes_and_preserve_unknown_usage_exactly_once() {
    let history = Arc::new(History::default());

    for outcome in [
        Outcome::Cancelled,
        Outcome::Interrupted,
        Outcome::Yielded,
        Outcome::Failed,
        Outcome::TimedOut,
    ] {
        let mut trace = history.trace("fixture", "model", &profile());
        tokio::time::advance(Duration::from_secs(2)).await;
        trace.outcome(outcome);
        drop(trace);
    }

    let samples = history.samples();

    assert_eq!(samples.len(), 5);

    assert_eq!(
        samples.iter().map(|sample| sample.outcome).collect::<Vec<_>>(),
        vec![
            Outcome::Cancelled,
            Outcome::Interrupted,
            Outcome::Yielded,
            Outcome::Failed,
            Outcome::TimedOut,
        ]
    );

    assert!(samples.iter().all(|sample| sample.elapsed_millis == 2_000
        && sample.input.is_none()
        && sample.output.is_none()
        && sample.timing.is_none()));

    let mut trace = history.trace("fixture", "model", &profile());

    tokio::time::advance(Duration::from_secs(1)).await;

    let reply = tasks::reply("Done".into());

    trace.complete(&reply);
    trace.complete(&reply);
    drop(trace);

    assert_eq!(history.samples().len(), 6);
    assert_eq!(history.samples().last().unwrap().outcome, Outcome::Complete);

    let mut legacy = profile();

    legacy.execution = None;
    drop(history.trace("fixture", "model", &legacy));

    assert_eq!(history.samples().len(), 6);

    let trace = history.trace("fixture", "model", &profile());

    tokio::time::advance(Duration::from_millis(execution::MAX_MILLIS + 1)).await;
    drop(trace);

    let samples = history.samples();

    assert_eq!(samples.len(), 7);
    assert_eq!(samples.last().unwrap().elapsed_millis, execution::MAX_MILLIS);
    assert_eq!(samples.last().unwrap().outcome, Outcome::Interrupted);
}

#[tokio::test(start_paused = true)]
async fn aborting_the_parent_future_retains_a_censored_interruption_without_a_reply() {
    let history = Arc::new(History::default());
    let observing = Arc::clone(&history);
    let (began, started) = tokio::sync::oneshot::channel();

    let task = tokio::spawn(async move {
        let _trace = observing.trace("fixture", "model", &profile());
        began.send(()).unwrap();
        std::future::pending::<()>().await;
    });

    started.await.unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    task.abort();

    assert!(task.await.unwrap_err().is_cancelled());

    let samples = history.samples();

    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].outcome, Outcome::Interrupted);
    assert_eq!(samples[0].elapsed_millis, 5_000);
    assert!(samples[0].input.is_none() && samples[0].output.is_none());
}

#[tokio::test(start_paused = true)]
async fn unusable_metrics_do_not_turn_a_valid_completion_into_free_or_successful_rate_evidence() {
    let history = Arc::new(History::default());
    let mut trace = history.trace("fixture", "model", &profile());
    let mut reply = tasks::reply("Done".into());

    reply.input = Some(100);

    reply.timing =
        Some(execution::Timing { load_millis: Some(500_000), ..execution::Timing::default() });

    tokio::time::advance(Duration::from_secs(1)).await;
    trace.complete(&reply);
    drop(trace);

    let samples = history.samples();

    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].outcome, Outcome::Complete);
    assert!(samples[0].input.is_none() && samples[0].timing.is_none());
}

#[tokio::test]
async fn legacy_success_history_migrates_and_failed_attempts_survive_reload() {
    let root = crate::tests::test_root("inference-migration");

    std::fs::create_dir_all(root.join("data")).unwrap();

    let initial = History::default();

    learn(&initial, &profile());

    let mut legacy = serde_json::to_value(initial.data.lock().unwrap().clone()).unwrap();

    legacy["version"] = json!(1);

    for series in legacy["entries"].as_object_mut().unwrap().values_mut() {
        for sample in series["samples"].as_array_mut().unwrap() {
            sample.as_object_mut().unwrap().remove("outcome");
        }
    }

    let path = root.join("data/inference.json");

    krabb_file::save(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

    let restored = History::open(&root);

    assert_eq!(restored.data.lock().unwrap().version, 2);
    assert_eq!(restored.samples().len(), 8);
    assert!(restored.samples().iter().all(|sample| sample.outcome == Outcome::Complete));

    assert_eq!(
        initial.plan("fixture", "model", &profile(), work(), execution::MAX_MILLIS).unwrap(),
        restored.plan("fixture", "model", &profile(), work(), execution::MAX_MILLIS).unwrap()
    );

    let mut trace = restored.trace("fixture", "model", &profile());

    trace.outcome(Outcome::Failed);
    drop(trace);

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(bytes) = std::fs::read(&path) {
                let data: Snapshot = serde_json::from_slice(&bytes).unwrap();

                if data.version == 2
                    && data.entries.values().any(|series| {
                        series.samples().iter().any(|sample| sample.outcome == Outcome::Failed)
                    })
                {
                    break;
                }
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let again = History::open(&root);

    assert_eq!(again.samples().last().unwrap().outcome, Outcome::Failed);

    assert!(
        again
            .plan("fixture", "model", &profile(), work(), execution::MAX_MILLIS)
            .unwrap()
            .unwrap()
            .total_millis
            >= initial
                .plan("fixture", "model", &profile(), work(), execution::MAX_MILLIS)
                .unwrap()
                .unwrap()
                .total_millis
    );

    drop((restored, again));
    std::fs::remove_dir_all(root).unwrap();
}
