use crate::*;
use std::fs;

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn acknowledgements(channel: &Live, call: &mut u64) -> Vec<Value> {
    // The response follows every preceding request on the same JSONL stream.
    let response = channel.call(Request::call(*call, "inspect", Value::Null)).await.unwrap();

    *call += 1;

    assert!(response.error.is_none());

    serde_json::from_value(response.result.unwrap()).unwrap()
}

async fn publication_failure(retained: bool, acknowledged: bool, offset: Option<i64>) {
    let fixture = Fixture(tests::test_root("event-publication"));
    let path = fixture.0.join("data/sessions/index.json");
    let saved = path.with_extension("saved");
    let mut store = state::Store::load(&path).unwrap();
    let event = serde_json::json!({"id": 7, "gateway_sequence": 17, "text": "retry me"});
    let following = serde_json::json!({"id": "next", "text": "keep queued"});

    store.commit("telegram", "previous", Some(4)).unwrap();
    store.commit("other", "untouched", Some(31)).unwrap();
    store.defer_event("telegram", event.clone()).unwrap();
    store.defer_event("telegram", following.clone()).unwrap();

    if acknowledged {
        store.commit_retained("telegram", "7", Some(4)).unwrap();
        store.mark_deferred_acknowledged("telegram", "7").unwrap();
    }

    let before = serde_json::to_value(&store).unwrap();
    let committed = fs::read(&path).unwrap();
    let sessions = Arc::new(Mutex::new(store));

    let script = r#"
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
acks = []
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': 'telegram',
                  'version': '0.1.0', 'capabilities': ['channel']}
    elif method == 'ack':
        try:
            clients = json.loads(path.read_text())['clients']
        except OSError:
            clients = None
        acks.append({'sequence': request['params']['sequence'], 'clients': clients})
        result = {'acknowledged': True}
    elif method == 'inspect':
        result = acks
    elif method == 'shutdown':
        break
    else:
        raise AssertionError(method)
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

    let channel = Live::new(
        Process::start_with("python3", ["-u", "-c", script, path.to_str().unwrap()]).await.unwrap(),
    );

    let mut current = 4;
    let mut call = 10;

    // Block publication after staging, without relying on permissions or disk exhaustion.
    fs::rename(&path, &saved).unwrap();
    fs::create_dir(&path).unwrap();

    let error = commit_event_inner(
        &channel,
        &sessions,
        "telegram",
        "7",
        offset,
        &mut current,
        Some(17),
        &mut call,
        retained,
    )
    .await
    .unwrap_err();

    assert!(error.downcast_ref::<std::io::Error>().is_some(), "{error}");
    assert!(acknowledgements(&channel, &mut call).await.is_empty());
    assert_eq!(current, 4);

    {
        let store = sessions.lock().unwrap();

        assert_eq!(store.offset("telegram"), 4);
        assert_eq!(store.known("telegram", "7"), acknowledged);
        assert_eq!(store.deferred_event("telegram"), Some(event.clone()));
        assert_eq!(store.deferred_acknowledged("telegram", "7"), acknowledged);
        assert_eq!(serde_json::to_value(&*store).unwrap(), before);
    }

    assert_eq!(fs::read(&saved).unwrap(), committed);

    fs::remove_dir(&path).unwrap();
    fs::rename(&saved, &path).unwrap();

    let recovered = state::Store::load(&path).unwrap();

    assert_eq!(serde_json::to_value(&recovered).unwrap(), before);
    assert_eq!(fs::read(&path).unwrap(), committed);

    commit_event_inner(
        &channel,
        &sessions,
        "telegram",
        "7",
        offset,
        &mut current,
        Some(17),
        &mut call,
        retained,
    )
    .await
    .unwrap();

    let expected_offset = offset.unwrap_or(4).max(4);

    let after = {
        let store = sessions.lock().unwrap();

        assert_eq!(current, expected_offset);
        assert_eq!(store.offset("telegram"), expected_offset);
        assert!(store.known("telegram", "7"));
        assert!(store.known("telegram", "previous"));
        assert_eq!(store.offset("other"), 31);
        assert!(store.known("other", "untouched"));
        assert_eq!(store.has_deferred_event("telegram", "7"), retained);
        assert_eq!(
            store.deferred_event("telegram"),
            Some(if retained { event } else { following })
        );

        assert!(store.has_deferred_event("telegram", "next"));
        assert_eq!(store.deferred_acknowledged("telegram", "7"), retained);
        serde_json::to_value(&*store).unwrap()
    };

    let acks = acknowledgements(&channel, &mut call).await;

    assert_eq!(acks.len(), usize::from(!acknowledged));

    if !acknowledged {
        assert_eq!(acks[0]["sequence"], 17);

        for field in ["offsets", "seen", "deferred"] {
            assert_eq!(acks[0]["clients"][field], after[field], "persist {field} before ack");
        }
    }

    let recovered = state::Store::load(&path).unwrap();

    assert_eq!(serde_json::to_value(&recovered).unwrap(), after);

    if retained {
        // Replaying an acknowledged retained event consumes it without acknowledging twice.
        let recovered = Arc::new(Mutex::new(recovered));

        commit_event(
            &channel,
            &recovered,
            "telegram",
            "7",
            offset,
            &mut current,
            Some(17),
            &mut call,
        )
        .await
        .unwrap();

        assert_eq!(acknowledgements(&channel, &mut call).await, acks);

        let store = recovered.lock().unwrap();

        assert!(!store.has_deferred_event("telegram", "7"));
        assert!(!store.deferred_acknowledged("telegram", "7"));

        assert_eq!(
            serde_json::to_value(state::Store::load(&path).unwrap()).unwrap(),
            serde_json::to_value(&*store).unwrap()
        );
    }

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn ordinary_commit_rolls_back_publication_failure_before_acknowledging() {
    tokio::time::timeout(Duration::from_secs(15), publication_failure(false, false, Some(8)))
        .await
        .unwrap();
}

#[tokio::test]
async fn retained_commit_rolls_back_publication_failure_before_acknowledging() {
    tokio::time::timeout(Duration::from_secs(15), publication_failure(true, false, Some(8)))
        .await
        .unwrap();
}

#[tokio::test]
async fn acknowledged_replay_preserves_durable_state_when_publication_fails() {
    for offset in [None, Some(2)] {
        tokio::time::timeout(Duration::from_secs(15), publication_failure(false, true, offset))
            .await
            .unwrap();
    }
}
