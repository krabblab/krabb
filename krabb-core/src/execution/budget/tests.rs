use super::*;

#[test]
fn retries_share_the_remaining_allowance_and_unresolved_work_retains_its_reservation() {
    let mut budget = Budget::default();
    let first = budget.reserve(1, 100, Some(100)).unwrap();

    assert_eq!(budget.remaining(None).unwrap(), 0);
    assert!(budget.reserve(1, 1, None).is_err());

    budget.settle(first, Some(20)).unwrap();

    assert_eq!(budget.remaining(None).unwrap(), 80);

    let retry = budget.reserve(2, 80, None).unwrap();

    assert!(budget.recover().unwrap());
    assert_eq!(budget.spent_millis, 20);
    assert_eq!(budget.held_millis, 80);
    assert_eq!(budget.remaining(None).unwrap(), 0);
    assert!(!budget.settle(retry, Some(0)).unwrap());
    assert!(budget.reserve(3, 1, Some(1000)).is_err());
}

#[test]
fn duplicate_and_stale_receipts_cannot_refund_or_charge_another_attempt() {
    let mut budget = Budget::default();
    let first = budget.reserve(1, 100, Some(300)).unwrap();

    budget.settle(first, Some(20)).unwrap();

    assert!(!budget.settle(first, Some(100)).unwrap());

    let second = budget.reserve(2, 100, None).unwrap();

    assert!(!budget.settle(first, None).unwrap());
    assert_eq!(budget.pending, Some(second));

    let before = budget.clone();

    assert!(budget.settle(Reservation { generation: 3, ..second }, Some(0)).is_err());
    assert!(budget.settle(second, Some(101)).is_err());
    assert_eq!(budget, before);

    budget.settle(second, Some(30)).unwrap();

    assert_eq!(budget.spent_millis, 50);
}

#[test]
fn persisted_reservations_and_legacy_uncertainty_cannot_become_fresh_budgets() {
    let mut budget = Budget::default();

    budget.reserve(1, 90, Some(100)).unwrap();

    let mut restored: Budget =
        serde_json::from_slice(&serde_json::to_vec(&budget).unwrap()).unwrap();

    restored.recover().unwrap();

    assert_eq!(restored.remaining(None).unwrap(), 10);

    let legacy = Budget::legacy();

    assert!(legacy.remaining(Some(100)).is_err());
    assert!(legacy.remaining(None).is_ok());

    let value = serde_json::to_value(budget).unwrap();

    for field in ["limit_millis", "spent_millis", "held_millis", "legacy", "sequence", "pending"] {
        let mut damaged = value.clone();

        damaged.as_object_mut().unwrap().remove(field);

        assert!(serde_json::from_value::<Budget>(damaged).is_err(), "{field}");
    }
}

#[test]
fn bounds_and_accounting_overflow_fail_without_partial_changes() {
    let mut budget = Budget::default();

    for (generation, millis) in [(0, 1), (1, 0), (1, MAX_MILLIS + 1)] {
        assert!(budget.reserve(generation, millis, Some(100)).is_err());
        assert_eq!(budget, Budget::default());
    }

    budget.sequence = u64::MAX;

    assert!(budget.reserve(1, 1, None).is_err());

    budget.held_millis = u64::MAX;
    budget.spent_millis = 1;

    assert!(budget.validate().is_err());
}

#[test]
fn grants_increase_only_a_settled_tracked_total_and_preserve_unresolved_exposure() {
    let mut uncapped = Budget::default();
    let spent = uncapped.reserve(1, 100, None).unwrap();

    uncapped.settle(spent, Some(100)).unwrap();

    assert_eq!(uncapped.limit_millis, None);
    assert!(uncapped.increase(100).is_err());

    uncapped.increase(150).unwrap();

    assert_eq!(uncapped.remaining(None).unwrap(), 50);

    let mut budget = Budget::default();
    let pending = budget.reserve(1, 100, Some(100)).unwrap();

    assert!(budget.increase(200).is_err());

    budget.settle(pending, None).unwrap();

    assert!(budget.increase(100).is_err());

    budget.increase(200).unwrap();

    assert_eq!(budget.held_millis, 100);
    assert_eq!(budget.remaining(None).unwrap(), 100);

    let mut legacy = Budget { held_millis: 50, ..Budget::legacy() };

    assert!(legacy.remaining(Some(100)).is_err());
    assert!(legacy.increase(50).is_err());

    legacy.increase(200).unwrap();

    assert!(!legacy.legacy);
    assert_eq!(legacy.limit_millis, Some(200));
    assert_eq!(legacy.remaining(Some(300)).unwrap(), 150);
    assert!(legacy.increase(150).is_err());

    let mut active = Budget::legacy();

    active.pending = Some(Reservation { id: 1, generation: 1, millis: 10 });
    active.sequence = 1;

    assert!(active.increase(200).is_err());

    let grant = Grant { id: "grant-1".into(), revision: 1, sequence: 1, limit_millis: 200 };

    grant.validate().unwrap();

    let wire = serde_json::to_value(&grant).unwrap();

    assert_eq!(serde_json::from_value::<Grant>(wire.clone()).unwrap(), grant);

    let mut forged = wire;

    forged["owner"] = serde_json::json!("admin");

    assert!(serde_json::from_value::<Grant>(forged).is_err());

    for id in ["", "bad id", &"a".repeat(crate::task::COMMAND_LIMIT + 1)] {
        assert!(Grant { id: id.into(), ..grant.clone() }.validate().is_err());
    }
}

#[test]
fn token_use_keeps_reports_and_estimates_apart_and_never_treats_missing_usage_as_free() {
    let mut tokens = Tokens::default();

    tokens.charge(Some(100), Some(20), (90, 50)).unwrap();
    tokens.charge(None, Some(30), (80, 50)).unwrap();
    tokens.charge(Some(10), None, (5, 40)).unwrap();
    tokens.charge(None, None, (7, 3)).unwrap();

    assert_eq!(tokens, Tokens { reported: 160, estimated: 130 });
    assert_eq!(tokens.used(), 290);
    assert_eq!(tokens.remaining(Some(300)), 10);
    assert_eq!(tokens.remaining(Some(100)), 0);
    assert_eq!(tokens.remaining(None), u64::MAX);

    let mut full = Tokens { reported: u64::MAX, estimated: 0 };

    assert!(full.charge(Some(1), None, (0, 0)).is_err());

    let saved = serde_json::to_value(Budget::default()).unwrap();
    let mut legacy = saved.clone();

    legacy.as_object_mut().unwrap().remove("tokens");

    assert_eq!(serde_json::from_value::<Budget>(legacy).unwrap(), Budget::default());
    assert_eq!(saved["tokens"], serde_json::json!({"reported":0,"estimated":0}));
}

#[test]
fn renewals_grow_only_the_active_reservation_within_the_remaining_allowance() {
    let mut budget = Budget::default();
    let reservation = budget.reserve(1, 100, Some(300)).unwrap();

    assert!(budget.extend(Reservation { id: 9, ..reservation }, 50, Some(300)).is_err());
    assert!(budget.extend(reservation, 0, Some(300)).is_err());
    assert!(budget.extend(reservation, 201, Some(300)).is_err());

    let extended = budget.extend(reservation, 150, Some(300)).unwrap();

    assert_eq!(extended.millis, 250);
    assert_eq!(budget.pending, Some(extended));
    assert_eq!(budget.remaining(Some(300)).unwrap(), 50);
    assert!(budget.extend(reservation, 10, Some(300)).is_err());

    budget.settle(extended, Some(240)).unwrap();

    assert_eq!(budget.spent_millis, 240);

    let mut unbounded = Budget::default();
    let reservation = unbounded.reserve(1, 100, None).unwrap();

    assert_eq!(unbounded.extend(reservation, 500, None).unwrap().millis, 600);
}

#[test]
fn a_lower_configured_ceiling_tightens_admission_without_erasing_the_saved_grant() {
    let mut budget = Budget { limit_millis: Some(10_000), ..Budget::default() };

    assert_eq!(budget.remaining(Some(4_000)).unwrap(), 4_000);
    assert_eq!(budget.remaining(Some(20_000)).unwrap(), 10_000);
    assert_eq!(budget.remaining(None).unwrap(), 10_000);

    let reservation = budget.reserve(1, 1_000, Some(4_000)).unwrap();

    assert_eq!(budget.limit_millis, Some(10_000));
    assert_eq!(budget.remaining(Some(4_000)).unwrap(), 3_000);
    assert!(budget.extend(reservation, 3_001, Some(4_000)).is_err());

    budget.settle(reservation, Some(1_000)).unwrap();

    assert_eq!(budget.remaining(Some(20_000)).unwrap(), 9_000);
}

#[test]
fn the_allowance_is_captured_once_and_the_ceiling_bounds_every_saved_limit() {
    let policy = |allowance, ceiling| Policy { allowance, ceiling };
    let mut budget = Budget::default();

    budget.reserve(1, 1_000, policy(Some(5_000), Some(20_000))).unwrap();

    assert_eq!(budget.limit_millis, Some(5_000));
    assert_eq!(budget.remaining(policy(Some(1_000), Some(20_000))).unwrap(), 4_000);

    budget.limit_millis = Some(10_000);

    assert_eq!(budget.remaining(policy(Some(1_000), Some(20_000))).unwrap(), 9_000);
    assert_eq!(budget.remaining(policy(None, Some(4_000))).unwrap(), 3_000);
    assert_eq!(Budget::default().remaining(policy(None, Some(4_000))).unwrap(), u64::MAX);
}
