use crate::{State, call_at, call_inner_at, load_at, lock, record_ref, storage};
use krabb_core::types::Request;
use serde_json::{Value, json};

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

struct Store {
    root: PathBuf,
    index: PathBuf,
    state: Arc<Mutex<State>>,
}

impl Store {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "krabb-memory-rollback-{}-{}",
            std::process::id(),
            crate::nonce()
        ));

        std::fs::create_dir(&root).unwrap();

        let index = root.join("index.json");
        let state = Arc::new(Mutex::new(State::default()));
        let store = Self { root, index, state };

        for (key, value) in [("favorite", "old value"), ("unrelated", "keep this")] {
            store.call("remember", json!({"key": key, "value": value, "approved": true}));
        }

        store
    }

    fn call(&self, method: &str, params: Value) -> Value {
        let response = call_at(&self.state, Request::call(1, method, params), Some(&self.index))
            .unwrap()
            .unwrap();

        assert!(response.error.is_none());

        response.result.unwrap()
    }

    fn snapshot(&self) -> Value {
        serde_json::to_value(&*self.state.lock().unwrap()).unwrap()
    }

    fn records(&self) -> BTreeMap<String, Vec<u8>> {
        self.state
            .lock()
            .unwrap()
            .items
            .values()
            .map(|entry| {
                (entry.record.clone(), std::fs::read(self.root.join(&entry.record)).unwrap())
            })
            .collect()
    }

    fn assert_reopened(&self) {
        let reopened = load_at(Some(&self.index)).unwrap();

        assert_eq!(serde_json::to_value(&*reopened.lock().unwrap()).unwrap(), self.snapshot());

        for entry in reopened.lock().unwrap().items.values() {
            assert_eq!(
                std::fs::read_to_string(self.root.join(&entry.record)).unwrap(),
                format!("# {}\n\n{}\n", entry.key, entry.value)
            );
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct IndexFault {
    index: PathBuf,
    saved: PathBuf,
}

impl IndexFault {
    fn new(store: &Store) -> Self {
        let saved = store.root.join("committed.json");

        std::fs::rename(&store.index, &saved).unwrap();
        std::fs::create_dir(&store.index).unwrap();
        Self { index: store.index.clone(), saved }
    }
}

impl Drop for IndexFault {
    fn drop(&mut self) {
        std::fs::remove_dir(&self.index).unwrap();
        std::fs::rename(&self.saved, &self.index).unwrap();
    }
}

fn index_failure(store: Store, method: &str, params: Value) {
    let before = store.snapshot();
    let index = std::fs::read(&store.index).unwrap();
    let records = store.records();

    // Setup and reload have completed; fault only the final index publication under the lock.
    let lease = lock(&store.index).unwrap();
    let fault = IndexFault::new(&store);

    let result =
        call_inner_at(&store.state, Request::call(2, method, params.clone()), Some(&store.index));

    assert!(result.is_err(), "index publication must fail");
    assert_eq!(store.snapshot(), before, "values, audit and learning must roll back");
    assert_eq!(std::fs::read(&fault.saved).unwrap(), index);

    for (record, contents) in &records {
        assert_eq!(
            std::fs::read(store.root.join(record)).unwrap(),
            *contents,
            "a failed mutation must preserve every committed record before reload"
        );
    }

    drop(fault);
    drop(lease);

    assert_eq!(std::fs::read(&store.index).unwrap(), index);

    store.assert_reopened();
    store.call(method, params);
    store.assert_reopened();

    let after = store.snapshot();

    assert_eq!(
        after["items"][storage("global", "unrelated")],
        before["items"][storage("global", "unrelated")]
    );

    match method {
        "remember" => {
            assert_eq!(after["items"][storage("global", "favorite")]["value"], "new value");
            assert_eq!(after["audit"].as_array().unwrap().len(), 3);
            assert_eq!(after["audit"][2]["action"], "remember");
        }

        "forget" => {
            assert!(after["items"].get(storage("global", "favorite")).is_none());
            assert_eq!(after["audit"].as_array().unwrap().len(), 3);
            assert_eq!(after["audit"][2]["action"], "forget");
        }

        "learning" => {
            assert_eq!(after["learning"], "autonomous");
            assert_eq!(after["audit"], before["audit"]);
        }

        _ => unreachable!(),
    }

    let current = store.records();

    for record in records.keys().filter(|record| !current.contains_key(*record)) {
        assert!(!store.root.join(record).exists(), "retire old records after commit");
    }
}

#[test]
fn remember_index_failure_preserves_committed_state_and_records() {
    index_failure(
        Store::new(),
        "remember",
        json!({"key": "favorite", "value": "new value", "approved": true}),
    );
}

#[test]
fn forget_index_failure_preserves_committed_state_and_records() {
    index_failure(Store::new(), "forget", json!({"key": "favorite"}));
}

#[test]
fn learning_index_failure_preserves_committed_state_and_records() {
    index_failure(Store::new(), "learning", json!({"mode": "autonomous"}));
}

#[test]
fn legacy_record_survives_failed_update_and_migrates_on_retry() {
    let store = Store::new();

    {
        let mut state = store.state.lock().unwrap();
        let entry = state.items.get_mut(&storage("global", "favorite")).unwrap();
        entry.record = record_ref(&entry.scope, &entry.key);
        crate::persist_at(Some(&store.index), &state).unwrap();
    }

    index_failure(
        store,
        "remember",
        json!({"key": "favorite", "value": "new value", "approved": true}),
    );
}

#[test]
fn repeated_updates_and_reverts_keep_committed_records_consistent() {
    let store = Store::new();
    let before = store.snapshot();
    let unrelated = storage("global", "unrelated");

    for value in ["new value", "old value", "old value"] {
        let records = store.records();

        store.call("remember", json!({"key": "favorite", "value": value, "approved": true}));
        store.assert_reopened();

        let after = store.snapshot();

        assert_eq!(after["items"][storage("global", "favorite")]["value"], value);
        assert_eq!(after["items"][&unrelated], before["items"][&unrelated]);

        let current = store.records();

        for record in records.keys().filter(|record| !current.contains_key(*record)) {
            assert!(!store.root.join(record).exists());
        }
    }

    assert_eq!(store.snapshot()["audit"].as_array().unwrap().len(), 5);
}

#[test]
fn record_write_failure_rolls_back_and_allows_retry() {
    let store = Store::new();
    let before = store.snapshot();
    let index = std::fs::read(&store.index).unwrap();
    let records = store.records();
    let record = store.root.join(record_ref("conversation", "new"));
    let blocker = record.parent().unwrap();

    std::fs::write(blocker, "block record directory creation").unwrap();

    let params = json!({
        "key": "new", "value": "new value", "scope": "conversation", "approved": true
    });

    let result =
        call_at(&store.state, Request::call(2, "remember", params.clone()), Some(&store.index));

    assert!(result.is_err());
    assert_eq!(store.snapshot(), before);
    assert_eq!(std::fs::read(&store.index).unwrap(), index);
    assert_eq!(store.records(), records);

    std::fs::remove_file(blocker).unwrap();
    store.assert_reopened();
    store.call("remember", params);
    store.assert_reopened();

    let after = store.snapshot();

    assert_eq!(after["items"][storage("conversation", "new")]["value"], "new value");
    assert_eq!(after["audit"].as_array().unwrap().len(), 3);
    assert_eq!(after["audit"][2]["scope"], "conversation");
}
