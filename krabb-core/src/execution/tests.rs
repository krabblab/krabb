use super::*;
use serde_json::json;

fn limits() -> Limits {
    Limits { first_millis: 100, idle_millis: 40, total_millis: 300, ceiling_millis: None }
}

#[test]
fn transport_heartbeats_do_not_extend_prefill_or_idle_allowances() {
    let mut timer = Timer::new(limits()).unwrap();

    for elapsed in [10, 20, 90, 99] {
        timer.observe(elapsed, false).unwrap();

        assert_eq!(timer.deadline(), 100);
    }

    assert_eq!(timer.observe(100, true), Err(Expiry::First));

    let mut timer = Timer::new(limits()).unwrap();

    timer.observe(90, true).unwrap();
    timer.observe(129, false).unwrap();

    assert_eq!(timer.deadline(), 130);
    assert_eq!(timer.observe(130, true), Err(Expiry::Idle));
}

#[test]
fn real_progress_and_clock_rollback_cannot_renew_the_total_allowance() {
    let mut timer = Timer::new(limits()).unwrap();

    for elapsed in [90, 120, 150, 180, 210, 240, 270, 299] {
        timer.observe(elapsed, true).unwrap();
    }

    assert_eq!(timer.deadline(), 300);

    timer.observe(1, true).unwrap();

    assert_eq!(timer.deadline(), 300);
    assert_eq!(timer.observe(300, true), Err(Expiry::Total));
}

#[test]
fn invalid_or_unbounded_deadlines_are_rejected_before_execution() {
    assert_eq!(
        limits().restrict(50).unwrap(),
        Limits { first_millis: 50, idle_millis: 40, total_millis: 50, ceiling_millis: None }
    );

    assert_eq!(limits().restrict(1_000).unwrap(), limits());
    assert!(matches!(limits().restrict(0), Err(Error::Limit(_))));

    for limits in [
        Limits { first_millis: 0, ..limits() },
        Limits { idle_millis: 0, ..limits() },
        Limits { total_millis: 0, ..limits() },
        Limits { first_millis: 301, ..limits() },
        Limits { idle_millis: 301, ..limits() },
        Limits { total_millis: MAX_MILLIS + 1, ..limits() },
    ] {
        assert!(Timer::new(limits).is_err());
    }

    assert!(
        serde_json::from_value::<Limits>(json!({
            "first_millis":1,"idle_millis":1,"total_millis":2,"renew":true
        }))
        .is_err()
    );

    let mut timer = Timer::new(Limits {
        first_millis: MAX_MILLIS,
        idle_millis: MAX_MILLIS,
        total_millis: MAX_MILLIS,
        ceiling_millis: None,
    })
    .unwrap();

    assert_eq!(timer.observe(u64::MAX, true), Err(Expiry::Total));
}

#[test]
fn execution_metadata_is_explicit_and_legacy_generation_remains_compatible() {
    let legacy: crate::context::Generation = serde_json::from_value(json!({"output":512})).unwrap();

    assert!(legacy.limits.is_none());
    assert!(serde_json::to_value(legacy).unwrap().get("limits").is_none());

    let mut profile = Profile {
        deployment: Deployment::Unknown,
        identity: "opaque-deployment-v1".into(),
        streaming: true,
        cancellation: false,
        usage: true,
        recoverable: false,
        deadlines: true,
        renewal: false,
    };

    profile.validate().unwrap();

    let encoded = serde_json::to_value(&profile).unwrap();

    assert_eq!(serde_json::from_value::<Profile>(encoded.clone()).unwrap(), profile);

    for identity in ["", "https://private-endpoint", "credential:unsafe", &"x".repeat(129)] {
        profile.identity = identity.into();

        assert!(profile.validate().is_err());
    }

    let mut malformed = encoded;

    malformed["deployment"] = json!("assumed-local");

    assert!(serde_json::from_value::<Profile>(malformed).is_err());

    let generation = crate::context::Generation {
        output: 512,
        limits: Some(limits()),
        summary: false,
        search: false,
    };

    assert_eq!(
        serde_json::from_value::<crate::context::Generation>(
            serde_json::to_value(generation).unwrap()
        )
        .unwrap(),
        generation
    );
}

#[test]
fn optional_timing_metadata_preserves_the_previous_wire_readers() {
    #[derive(Deserialize)]
    struct LegacyGeneration {
        output: u64,
        summary: bool,
        search: bool,
    }

    #[derive(Deserialize)]
    struct LegacyProfile {
        window: Option<crate::context::Window>,
        compact: bool,
        search: bool,
    }

    let generation = crate::context::Generation {
        output: 512,
        limits: Some(limits()),
        summary: false,
        search: true,
    };

    let old: LegacyGeneration =
        serde_json::from_value(serde_json::to_value(generation).unwrap()).unwrap();

    assert_eq!(old.output, 512);
    assert!(!old.summary);
    assert!(old.search);

    let profile = crate::context::Profile {
        window: Some(crate::context::Window { tokens: 4096, output: 512 }),
        compact: false,
        search: true,
        execution: Some(Profile {
            deployment: Deployment::Local,
            identity: "fixture-v1".into(),
            streaming: true,
            cancellation: false,
            usage: true,
            recoverable: false,
            deadlines: true,
            renewal: false,
        }),
    };

    let old: LegacyProfile =
        serde_json::from_value(serde_json::to_value(&profile).unwrap()).unwrap();

    assert_eq!(old.window, profile.window);
    assert_eq!(old.compact, profile.compact);
    assert_eq!(old.search, profile.search);
}

#[test]
fn execution_envelopes_round_trip_and_reject_unbounded_waits() {
    for total_millis in [1, MAX_MILLIS + 300_000] {
        let budget = Envelope { total_millis };

        budget.validate().unwrap();

        let frame = crate::types::IpcResponse::ok(
            7,
            json!({
                "event": ENVELOPE, "budget": budget,
            }),
        );

        let wire = serde_json::to_vec(&frame).unwrap();
        let decoded: crate::types::IpcResponse = serde_json::from_slice(&wire).unwrap();

        assert!(decoded.valid());
        assert_eq!(decoded.id, 7);

        let value: Envelope =
            serde_json::from_value(decoded.result.unwrap()["budget"].clone()).unwrap();

        assert_eq!(value, budget);
    }

    assert!(Envelope { total_millis: 0 }.validate().is_err());
    assert!(Envelope { total_millis: MAX_MILLIS + 300_001 }.validate().is_err());

    assert!(
        Timing { load_millis: Some(u64::MAX), prefill_millis: Some(1), decode_millis: None }
            .validate()
            .is_err()
    );
}

#[test]
fn optional_phase_reports_preserve_legacy_reply_readers_and_reject_malformed_measurements() {
    #[derive(Deserialize)]
    struct Legacy {
        text: String,
        stop: String,
    }

    let reply: crate::types::ModelReply = serde_json::from_value(json!({
        "text":"Hello", "stop":"stop", "timing":{
            "load_millis":0,"prefill_millis":100,"decode_millis":20,
        },
    }))
    .unwrap();

    reply.timing.unwrap().validate().unwrap();

    let wire = serde_json::to_value(reply).unwrap();
    let legacy: Legacy = serde_json::from_value(wire.clone()).unwrap();

    assert_eq!((legacy.text.as_str(), legacy.stop.as_str()), ("Hello", "stop"));

    for timing in
        [json!({"decode_millis":-1}), json!({"decode_millis":"20"}), json!({"unknown":100})]
    {
        let mut malformed = wire.clone();
        malformed["timing"] = timing;

        assert!(serde_json::from_value::<crate::types::ModelReply>(malformed).is_err());
    }
}

#[test]
fn renewal_raises_only_the_total_and_never_passes_the_admitted_ceiling() {
    let mut timer = Timer::new(limits()).unwrap();

    assert!(timer.renew(400).is_err());

    let renewable = Limits {
        first_millis: 100,
        idle_millis: 300,
        total_millis: 300,
        ceiling_millis: Some(600),
    };

    let mut timer = Timer::new(renewable).unwrap();

    assert!(timer.observe(50, true).is_ok());
    assert!(timer.observe(250, true).is_ok());

    timer.renew(300).unwrap();

    assert_eq!(timer.total(), 300);

    timer.renew(450).unwrap();

    assert_eq!(timer.total(), 450);

    timer.renew(400).unwrap();

    assert_eq!(timer.total(), 450);

    let mut clamped = Timer::new(renewable).unwrap();

    clamped.renew(601).unwrap();

    assert_eq!(clamped.total(), 600);
    assert!(timer.observe(280, true).is_ok());
    assert!(timer.observe(310, true).is_ok());
    assert_eq!(timer.observe(450, true), Err(Expiry::Total));
    assert_eq!(Timer::new(renewable).unwrap().observe(140, false), Err(Expiry::First));

    for ceiling in [299, MAX_MILLIS + 1] {
        assert!(Limits { ceiling_millis: Some(ceiling), ..renewable }.validate().is_err());
    }

    assert_eq!(renewable.restrict(500).unwrap().ceiling_millis, Some(500));
    assert_eq!(renewable.restrict(200).unwrap().ceiling_millis, Some(200));

    let wire = serde_json::to_value(limits()).unwrap();

    assert!(wire.get("ceiling_millis").is_none());

    assert_eq!(
        serde_json::from_value::<Renewal>(serde_json::json!({"id":7,"total_millis":450})).unwrap(),
        Renewal { id: 7, total_millis: 450 }
    );
}
