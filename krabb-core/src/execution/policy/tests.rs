use super::*;

fn calibration() -> Series {
    let mut series = Series::default();

    for (index, (prefill, decode)) in [
        (900, 1_100),
        (1_000, 1_200),
        (1_050, 1_250),
        (1_100, 1_300),
        (1_150, 1_400),
        (1_200, 1_500),
        (1_250, 1_600),
        (1_300, 1_700),
    ]
    .into_iter()
    .enumerate()
    {
        series
            .record(Sample {
                outcome: Outcome::Complete,
                at: 1_000 + index as u64,
                input: Some(1_024),
                output: Some(256),
                elapsed_millis: prefill + decode + 1_020,
                timing: Some(Timing {
                    load_millis: Some(1_000),
                    prefill_millis: Some(prefill),
                    decode_millis: Some(decode),
                }),
            })
            .unwrap();
    }

    series
}

fn admitted(decision: Decision) -> Estimate {
    match decision {
        Decision::Admit(estimate) => estimate,
        other => panic!("Unexpected admission decision: {other:?}"),
    }
}

#[test]
fn withheld_latency_traces_fit_without_reusing_them_as_observations() {
    let series = calibration();

    for (input, output, prefill, decode, load) in [
        (1_500, 320, 2_000, 2_700_u64, 1_200),
        (2_500, 640, 3_800, 5_700, 1_100),
        (4_000, 900, 5_900, 7_500, 1_500),
    ] {
        let estimate = admitted(
            predict(
                Deployment::Remote,
                Work { input, output, cold: false, streaming: true },
                &series,
                1_010,
                MAX_MILLIS,
            )
            .unwrap(),
        );

        assert_eq!(estimate.confidence, Confidence::Measured);
        assert_eq!(estimate.samples, 8);
        assert!(estimate.limits.first_millis >= load + prefill + decode.div_ceil(output));
        assert!(estimate.limits.total_millis >= load + prefill + decode);
    }
}

#[test]
fn slow_local_priors_and_nonstreaming_work_do_not_inherit_a_one_minute_cutoff() {
    for deployment in [Deployment::Local, Deployment::Unknown] {
        let estimate = admitted(
            predict(
                deployment,
                Work { input: 2_000, output: 512, cold: true, streaming: true },
                &Series::default(),
                1_000,
                MAX_MILLIS,
            )
            .unwrap(),
        );

        assert_eq!(estimate.confidence, Confidence::Prior);
        assert!(estimate.limits.first_millis >= 180_000);
        assert!(estimate.limits.total_millis >= 180_000 + 512_000);
        let plain = admitted(
            predict(
                deployment,
                Work { input: 2_000, output: 512, cold: true, streaming: false },
                &calibration(),
                1_010,
                MAX_MILLIS,
            )
            .unwrap(),
        );

        assert_eq!(plain.limits.first_millis, plain.limits.total_millis);
        assert_eq!(plain.limits.idle_millis, plain.limits.total_millis);
    }
}

#[test]
fn hard_limits_reject_before_spending_and_slowdown_changes_the_prediction() {
    let work = Work { input: 2_000, output: 512, cold: false, streaming: true };

    let mut series = calibration();
    let original = admitted(predict(Deployment::Remote, work, &series, 1_010, MAX_MILLIS).unwrap());

    assert_eq!(
        predict(Deployment::Remote, work, &series, 1_010, original.limits.total_millis - 1)
            .unwrap(),
        Decision::NeedsBudget {
            required_millis: original.limits.total_millis,
            available_millis: original.limits.total_millis - 1,
        }
    );

    for at in 1_011..1_019 {
        series
            .record(Sample {
                outcome: Outcome::Complete,
                at,
                input: Some(1_024),
                output: Some(256),
                elapsed_millis: 90_000,
                timing: Some(Timing {
                    load_millis: Some(1_000),
                    prefill_millis: Some(8_000),
                    decode_millis: Some(80_000),
                }),
            })
            .unwrap();
    }

    let slower = admitted(predict(Deployment::Remote, work, &series, 1_020, MAX_MILLIS).unwrap());

    assert!(slower.limits.total_millis > original.limits.total_millis * 5);
    assert!(slower.limits.first_millis > original.limits.first_millis);
}

#[test]
fn missing_usage_expiry_clock_jumps_and_bounded_history_keep_conservative_fallbacks() {
    let work = Work { input: 1_000, output: 512, cold: false, streaming: true };
    let series = calibration();

    for now in [999, 1_008 + AGE_SECONDS + 1] {
        let estimate =
            admitted(predict(Deployment::Remote, work, &series, now, MAX_MILLIS).unwrap());

        assert_eq!(estimate.samples, 0);
        assert_eq!(estimate.confidence, Confidence::Prior);
    }

    let mut series = Series::default();

    for at in 0..100 {
        series
            .record(Sample {
                outcome: Outcome::Complete,
                at,
                input: None,
                output: None,
                elapsed_millis: 10,
                timing: None,
            })
            .unwrap();
    }

    let estimate = admitted(predict(Deployment::Unknown, work, &series, 100, MAX_MILLIS).unwrap());

    assert_eq!(estimate.samples, SAMPLES);
    assert_eq!(estimate.confidence, Confidence::Prior);
    assert_eq!(series.last(), Some(99));

    series
        .record(Sample {
            outcome: Outcome::Complete,
            at: 1,
            input: None,
            output: None,
            elapsed_millis: 10,
            timing: None,
        })
        .unwrap();

    assert_eq!(series.last(), Some(1));

    assert_eq!(
        admitted(predict(Deployment::Unknown, work, &series, 1, MAX_MILLIS).unwrap()).samples,
        1
    );
}

#[test]
fn impossible_timings_and_oversized_restored_histories_are_rejected() {
    let mut sample = Sample {
        outcome: Outcome::Complete,
        at: 1,
        input: Some(100),
        output: Some(100),
        elapsed_millis: 100,
        timing: Some(Timing {
            load_millis: None,
            prefill_millis: Some(50),
            decode_millis: Some(1_000),
        }),
    };

    assert!(sample.validate().is_err());

    sample.timing = None;
    sample.input = Some(u64::MAX);

    assert!(sample.validate().is_err());

    sample.input = None;
    sample.elapsed_millis = MAX_MILLIS + 1;

    assert!(sample.validate().is_err());

    let sample = Sample {
        outcome: Outcome::Complete,
        at: 1,
        input: None,
        output: None,
        elapsed_millis: 1,
        timing: None,
    };

    let series: Series = serde_json::from_value(serde_json::json!({
        "samples": vec![sample; SAMPLES + 1]
    }))
    .unwrap();

    assert!(series.validate().is_err());

    assert!(
        predict(
            Deployment::Unknown,
            Work { input: 0, output: 0, cold: true, streaming: true },
            &Series::default(),
            1,
            MAX_MILLIS
        )
        .is_err()
    );
}

fn censored(at: u64, outcome: Outcome) -> Sample {
    Sample { outcome, at, input: None, output: None, elapsed_millis: 90_000, timing: None }
}

#[test]
fn failures_lower_confidence_without_shortening_slow_estimates_or_granting_more_authority() {
    let work = Work { input: 2_000, output: 512, cold: false, streaming: true };

    for slow in [false, true] {
        let mut series = calibration();

        if slow {
            for sample in &mut series.samples {
                sample.timing.as_mut().unwrap().prefill_millis = Some(100_000);
                sample.timing.as_mut().unwrap().decode_millis = Some(500_000);
                sample.elapsed_millis = 601_000;
            }
        }

        let before =
            admitted(predict(Deployment::Remote, work, &series, 1_010, MAX_MILLIS).unwrap());

        series.record(censored(1_011, Outcome::TimedOut)).unwrap();

        let after =
            admitted(predict(Deployment::Remote, work, &series, 1_012, MAX_MILLIS).unwrap());

        assert_eq!(before.confidence, Confidence::Measured);
        assert_eq!(after.confidence, Confidence::Prior);
        assert!(after.limits.first_millis >= before.limits.first_millis);
        assert!(after.limits.total_millis >= before.limits.total_millis);

        assert!(matches!(
            predict(Deployment::Remote, work, &series, 1_012, before.limits.total_millis - 1)
                .unwrap(),
            Decision::NeedsBudget { .. }
        ));
    }
}

#[test]
fn only_valid_post_failure_measurements_restore_confidence_and_controls_do_not_poison_learning() {
    let work = Work { input: 2_000, output: 512, cold: false, streaming: true };
    let mut series = calibration();
    let before = admitted(predict(Deployment::Remote, work, &series, 1_010, MAX_MILLIS).unwrap());

    for (offset, outcome) in
        [Outcome::Cancelled, Outcome::Interrupted, Outcome::Yielded].into_iter().enumerate()
    {
        series.record(censored(1_011 + offset as u64, outcome)).unwrap();
    }

    let controls = admitted(predict(Deployment::Remote, work, &series, 1_014, MAX_MILLIS).unwrap());

    assert_eq!(controls.limits, before.limits);
    assert_eq!(controls.confidence, Confidence::Measured);

    series.record(censored(1_015, Outcome::Failed)).unwrap();

    for at in 1_016..1_024 {
        series
            .record(Sample {
                outcome: Outcome::Complete,
                at,
                input: None,
                output: None,
                elapsed_millis: 10,
                timing: None,
            })
            .unwrap();
    }

    assert_eq!(
        admitted(predict(Deployment::Remote, work, &series, 1_024, MAX_MILLIS).unwrap()).confidence,
        Confidence::Prior
    );

    let fresh = calibration();

    for (index, sample) in fresh.samples.iter().enumerate() {
        series.record(Sample { at: 1_025 + index as u64, ..*sample }).unwrap();
    }

    assert_eq!(
        admitted(predict(Deployment::Remote, work, &series, 1_033, MAX_MILLIS).unwrap()).confidence,
        Confidence::Measured
    );
}

#[test]
fn censored_records_preserve_unknown_usage_and_old_success_samples_remain_readable() {
    let legacy: Sample = serde_json::from_value(serde_json::json!({
        "at":1,"input":10,"output":2,"elapsed_millis":100,"timing":null,
    }))
    .unwrap();

    assert_eq!(legacy.outcome, Outcome::Complete);

    legacy.validate().unwrap();

    let mut sample = censored(1, Outcome::Failed);

    sample.validate().unwrap();
    sample.input = Some(0);

    assert!(sample.validate().is_err());

    sample.input = None;
    sample.timing = Some(Timing::default());

    assert!(sample.validate().is_err());

    let mut unknown = serde_json::to_value(censored(1, Outcome::Failed)).unwrap();

    unknown["outcome"] = serde_json::json!("guessed");

    assert!(serde_json::from_value::<Sample>(unknown).is_err());

    let mut series = Series::default();

    for at in 1..100 {
        series.record(censored(at, Outcome::TimedOut)).unwrap();
    }

    assert_eq!(series.samples().len(), SAMPLES);

    assert!(
        series
            .samples()
            .iter()
            .all(|sample| sample.outcome == Outcome::TimedOut && sample.input.is_none())
    );
}

#[test]
fn idle_floors_follow_the_deployment_and_whether_rates_are_measured() {
    let work = Work { input: 1_024, output: 256, cold: false, streaming: true };

    let idle = |deployment, series: &Series| {
        admitted(predict(deployment, work, series, 1_010, MAX_MILLIS).unwrap()).limits.idle_millis
    };

    assert_eq!(idle(Deployment::Local, &Series::default()), 120_000);
    assert_eq!(idle(Deployment::Local, &calibration()), 30_000);
    assert_eq!(idle(Deployment::Remote, &Series::default()), 15_000);
}

#[test]
fn a_recent_failure_never_trusts_rates_faster_than_the_prior() {
    let work = Work { input: 2_000, output: 512, cold: false, streaming: true };
    let mut series = calibration();

    series.record(censored(1_011, Outcome::TimedOut)).unwrap();

    let recovering =
        admitted(predict(Deployment::Remote, work, &series, 1_012, MAX_MILLIS).unwrap());

    let prior =
        admitted(predict(Deployment::Remote, work, &Series::default(), 1_012, MAX_MILLIS).unwrap());

    assert_eq!(recovering.limits.total_millis, prior.limits.total_millis);
    assert_eq!(recovering.limits.first_millis, prior.limits.first_millis);
}
