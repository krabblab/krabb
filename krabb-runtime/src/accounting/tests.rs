use super::*;

#[tokio::test(start_paused = true)]
async fn overlapping_tool_and_approval_waits_do_not_consume_active_inference_time() {
    let clock = Clock::default();

    tokio::time::advance(Duration::from_secs(2)).await;

    let first = clock.wait();

    tokio::time::advance(Duration::from_secs(30)).await;

    let second = clock.wait();

    drop(first);
    tokio::time::advance(Duration::from_secs(30)).await;

    assert_eq!(clock.elapsed(), None);

    drop(second);
    tokio::time::advance(Duration::from_secs(3)).await;

    assert_eq!(clock.elapsed(), Some(5_000));
}

#[tokio::test(start_paused = true)]
async fn submillisecond_work_is_rounded_up_and_requests_without_a_task_use_the_ceiling() {
    let clock = Clock::default();

    tokio::time::advance(Duration::from_nanos(1)).await;

    assert_eq!(clock.elapsed(), Some(1));

    let store = Arc::new(Mutex::new(state::Store::default()));

    assert_eq!(remaining(&store, None, None.into()).unwrap(), u64::MAX);
    assert_eq!(remaining(&store, None, Some(100).into()).unwrap(), 100);
    assert!(Account::reserve(&store, None, 100, None, (0, None)).unwrap().is_none());
    assert!(Account::reserve(&store, None, 100, Some(100), (0, None)).unwrap().is_none());
}
