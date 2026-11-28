use super::*;
use crate::{Live, Plugins, Process, composition::Choice};

/// A plugin process that answers each method in `answers` with its JSON result.
async fn plugin(id: &str, capability: &str, answers: &[(&str, String)]) -> Live {
    let cases = answers
        .iter()
        .map(|(method, result)| {
            format!(
                "        *'\"method\":\"{method}\"'*) printf '%s\\n' \
                 '{{\"jsonrpc\":\"2.0\",\"id\":'\"$id\"',\"result\":{result}}}' ;;\n"
            )
        })
        .collect::<String>();

    let script = format!(
        r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
    case "$line" in
        *'"method":"hello"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"protocol":{{"major":0,"minor":2}},"id":"{id}","version":"0.1.0","capabilities":["{capability}"]}}}}' ;;
        *'"method":"shutdown"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"ok":true}}}}'; exit 0 ;;
{cases}    esac
done
"#
    );

    Live::new(Process::start_with("sh", ["-c", &script]).await.unwrap())
}

fn select(plugins: &Plugins, capability: &str, primary: &str, also: &[&str]) {
    let choice = Choice {
        primary: Some(primary.into()),
        also: also.iter().map(|id| (*id).to_owned()).collect(),
    };

    *plugins.composition.lock().unwrap() = Some([(capability.to_owned(), choice)].into());
}

async fn stop(plugins: &Plugins) {
    for (_, plugin) in plugins.all().await {
        plugin.stop().await.unwrap();
    }
}

fn memory(key: &str) -> String {
    json!({"items": [{"key": key, "summary": key, "scope": "global", "updated": 1}]}).to_string()
}

fn schedules(ids: &[u64]) -> String {
    let items = ids.iter().map(|id| json!({"id": id, "name": format!("job-{id}")}));
    let items = items.collect::<Vec<_>>();

    json!({"items": items, "total": ids.len(), "page": 1, "pages": 1, "from": 1, "to": ids.len()})
        .to_string()
}

async fn call(state: &State, params: Value) -> IpcResponse {
    capability_call(&IpcRequest::call(9, "secret", "capability.call", params), state).await
}

#[tokio::test]
async fn without_a_selection_the_first_plugin_serves_as_before() {
    let plugins = Plugins::default();

    plugins.insert(plugin("memory", "memory", &[("search", memory("tea"))]).await).await;
    plugins.insert(plugin("zep", "memory", &[("search", memory("coffee"))]).await).await;

    assert_eq!(plugins.find(Capability::Memory).await.unwrap().0, "memory");
    assert!(plugins.members(&Capability::Memory).await.also.is_empty());

    select(&plugins, "memory", "zep", &[]);

    assert_eq!(plugins.find(Capability::Memory).await.unwrap().0, "zep");

    select(&plugins, "memory", "gone", &["memory"]);

    assert!(plugins.find(Capability::Memory).await.is_none());

    stop(&plugins).await;
}

#[tokio::test]
async fn memory_reads_merge_with_their_source_and_writes_stay_with_the_primary() {
    let mut state = tests::state_at("composition-memory");
    let primary = [("search", memory("coffee")), ("remember", json!({"ok": "zep"}).to_string())];
    let other = [("search", memory("tea")), ("remember", json!({"ok": "memory"}).to_string())];

    state.plugins = Plugins::default();
    state.plugins.insert(plugin("memory", "memory", &other).await).await;
    state.plugins.insert(plugin("zep", "memory", &primary).await).await;
    select(&state.plugins, "memory", "zep", &["memory"]);

    let search = json!({"service": "memory", "method": "search", "params": {"query": "x"}});
    let found = call(&state, search).await.result.unwrap();
    let items = found["items"].as_array().unwrap();

    assert_eq!(items.len(), 2);
    assert_eq!((items[0]["key"].as_str(), items[0].get("source")), (Some("coffee"), None));

    assert_eq!(
        (items[1]["key"].as_str(), items[1]["source"].as_str()),
        (Some("tea"), Some("memory"))
    );

    let write = json!({"service": "memory", "method": "remember", "params": {}});

    assert_eq!(call(&state, write.clone()).await.result.unwrap()["ok"], "zep");

    let mut targeted = write;

    targeted["plugin"] = "memory".into();

    assert_eq!(call(&state, targeted).await.result.unwrap()["ok"], "memory");

    let mut missing = json!({"service": "memory", "method": "search", "params": {}});

    missing["plugin"] = "absent".into();

    assert!(call(&state, missing).await.error.is_some());

    let mut messages = Vec::new();
    let mut id = 1;

    for (id, key) in [("memory", "tea"), ("zep", "coffee")] {
        let replaced = state.plugins.insert(plugin(id, "memory", &[("index", memory(key))]).await);

        replaced.await.unwrap().stop().await.unwrap();
    }

    crate::append_memory_context_scopes(&state.plugins, &mut messages, &["global"], &mut id).await;

    let Content::Text { text } = &messages[0].content[0] else { panic!("no memory index") };

    assert!(text.contains("- [global] coffee: coffee\n"), "{text}");
    assert!(text.contains("- [global] tea: tea (from memory)\n"), "{text}");

    stop(&state.plugins).await;
}

#[tokio::test]
async fn schedules_list_from_every_scheduler_and_controls_reach_the_owner() {
    let mut state = tests::state_at("composition-schedules");

    let answers = |ids: &[u64], owner: &str| {
        [
            ("list", schedules(ids)),
            ("pause", json!({"status": "paused", "owner": owner}).to_string()),
        ]
    };

    state.plugins = Plugins::default();
    state
        .plugins
        .insert(plugin("scheduler", "scheduler", &answers(&[11], "scheduler")).await)
        .await;

    state.plugins.insert(plugin("cron", "scheduler", &answers(&[22], "cron")).await).await;

    let pause = |id: u64| json!({"service": "scheduler", "method": "pause", "params": {"id": id}});

    assert_eq!(call(&state, pause(22)).await.result.unwrap()["owner"], "cron");

    let listed = json!({"service": "scheduler", "method": "list", "params": {"page": 1}});
    let single = call(&state, listed.clone()).await.result.unwrap();

    assert!(single["items"][0].get("scheduler").is_none());

    select(&state.plugins, "scheduler", "scheduler", &["cron"]);

    let merged = call(&state, listed).await.result.unwrap();

    let owners = merged["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| (item["id"].as_u64().unwrap(), item["scheduler"].as_str().unwrap()))
        .collect::<Vec<_>>();

    assert_eq!(owners, [(11, "scheduler"), (22, "cron")]);
    assert_eq!(merged["total"], 2);
    assert_eq!(call(&state, pause(22)).await.result.unwrap()["owner"], "cron");
    assert_eq!(call(&state, pause(11)).await.result.unwrap()["owner"], "scheduler");
    assert_eq!(call(&state, pause(99)).await.result.unwrap()["owner"], "scheduler");

    let owned = state.plugins.owned_by("cron");

    assert_eq!(owned.find(Capability::Scheduler).await.unwrap().0, "cron");
    assert!(owned.serves(&Capability::Scheduler, "cron").await);

    select(&state.plugins, "scheduler", "scheduler", &[]);

    assert!(!owned.serves(&Capability::Scheduler, "cron").await);
    assert_eq!(owned.find(Capability::Scheduler).await.unwrap().0, "cron");

    stop(&state.plugins).await;
}

#[test]
fn reload_applies_a_valid_selection_and_refuses_an_uninstalled_plugin() {
    let state = tests::state_at("composition-reload");
    let request = IpcRequest::call(1, "secret", "config.plugins.reload", json!({}));

    std::fs::create_dir_all(&state.home).unwrap();

    let lock = json!({"plugins": {"zep": {"source": "local", "version": "1",
        "protocol": {"major": 0, "minor": 2}, "capabilities": ["memory"]}}});

    std::fs::write(state.home.join("plugins.lock"), lock.to_string()).unwrap();
    std::fs::write(state.home.join("config.toml"), "[plugins.memory]\nprimary = \"zep\"\n")
        .unwrap();
    dispatch(&request, &state).unwrap();

    assert_eq!(state.plugins.table()["memory"].primary.as_deref(), Some("zep"));

    std::fs::write(state.home.join("config.toml"), "[plugins.memory]\nprimary = \"mem0\"\n")
        .unwrap();

    assert!(dispatch(&request, &state).is_err());

    assert_eq!(state.plugins.table()["memory"].primary.as_deref(), Some("zep"));

    let _ = std::fs::remove_dir_all(&state.home);
}
