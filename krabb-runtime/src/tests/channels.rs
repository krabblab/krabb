use super::*;
use serde_json::json;

const CHANNEL: &str = r#"
import json, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
polled = False
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if 'id' not in request:
        continue
    with (root / 'channel.jsonl').open('a') as log:
        log.write(json.dumps(request) + '\n')
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': 'telegram',
                  'version': '0.1.0', 'capabilities': ['channel']}
    elif method == 'poll':
        result = {'events': [] if polled else [
            {'id': n, 'chat': 7, 'private': True, 'sender': 8, 'thread': '4', 'text': 'hello'}
            for n in [1, 2]]}
        polled = True
        time.sleep(0.01)
    elif method == 'typing':
        result = True
    else:
        result = {'message_id': 99, 'acknowledged': True}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
    if method == 'shutdown':
        break
"#;

const MODEL: &str = r#"
import json, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
generated = 0
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request['method']
    response = {'jsonrpc': '2.0', 'id': request['id']}
    if method == 'hello':
        response['result'] = {'protocol': {'major': 0, 'minor': 1}, 'id': 'fixture',
                              'version': '0.1.0', 'capabilities': ['model']}
    elif method == 'generate':
        generated += 1
        (root / 'generated').write_text(str(generated))
        time.sleep(0.8)
        if generated == 1:
            response['error'] = {'code': -32000, 'message': 'Unsupported model: synthetic'}
        else:
            response['result'] = {'text': 'Recovered', 'stop': 'stop'}
    else:
        response['result'] = {}
    print(json.dumps(response), flush=True)
    if method == 'shutdown':
        break
"#;

fn requests(root: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(root.join("channel.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[tokio::test]
async fn channel_turns_type_and_do_not_replay_model_rejections() {
    let root = test_root("channel-rejection");

    fs::create_dir_all(&root).unwrap();

    let channel = Process::start_with("python3", ["-u", "-c", CHANNEL, root.to_str().unwrap()])
        .await
        .unwrap();

    let model =
        Process::start_with("python3", ["-u", "-c", MODEL, root.to_str().unwrap()]).await.unwrap();

    let plugins = registry([channel, model]).await;

    let sessions =
        Arc::new(Mutex::new(crate::state::Store::load(root.join("sessions.json")).unwrap()));

    let stop = Arc::new(Stop::new());
    let monitor_root = root.clone();
    let signal = Arc::clone(&stop);

    let monitor = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if requests(&monitor_root).iter().filter(|value| value["method"] == "send").count()
                    >= 2
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    break;
                }

                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        signal.signal();
    });

    tokio::time::timeout(
        Duration::from_secs(10),
        crate::bridge(
            &plugins,
            "telegram",
            "fixture",
            "unset",
            Arc::clone(&sessions),
            Arc::clone(&stop),
            Arc::new(Mutex::new(BTreeMap::new())),
            BTreeMap::new(),
            crate::ApprovalMode::Off,
        ),
    )
    .await
    .unwrap()
    .unwrap();

    monitor.await.unwrap();
    stop_registry(&plugins).await;

    assert_eq!(fs::read_to_string(root.join("generated")).unwrap(), "2");

    let calls = requests(&root);
    let typing: Vec<_> = calls.iter().filter(|value| value["method"] == "typing").collect();

    assert_eq!(typing.len(), 2);
    assert!(typing.iter().all(|value| value["params"] == json!({"chat":7,"thread":"4"})));

    let sent: Vec<_> = calls.iter().filter(|value| value["method"] == "send").collect();

    assert_eq!(sent.len(), 2);
    assert!(sent.iter().any(|value| value["params"]["text"] == "Recovered"));
    assert!(sent.iter().any(|value| value["params"]["text"] == "I could not complete that turn."));
    assert!(sessions.lock().unwrap().sessions["telegram-7-thread-4"].queued.is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn typing_is_optional_and_bounded() {
    for (name, handler) in [
        ("unsupported", "response = {'error': {'code': -32601, 'message': 'Unknown method'}}"),
        ("stalled", "time.sleep(10); response = {'result': True}"),
    ] {
        let script = format!(
            r#"
import json, sys, time
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    if request['method'] == 'hello':
        response = {{'result': {{'protocol': {{'major': 0, 'minor': 1}}, 'id': 'fixture', 'version': '0.1.0', 'capabilities': ['channel']}}}}
    elif request['method'] == 'typing':
        {handler}
    else:
        break
    response.update({{'jsonrpc': '2.0', 'id': request['id']}})
    print(json.dumps(response), flush=True)
"#
        );

        let channel =
            Live::new(Process::start_with("python3", ["-u", "-c", &script]).await.unwrap());
        let mut call = 7;

        assert!(!crate::indicate_typing(&channel, "telegram", "invalid", None, &mut call).await);
        assert_eq!(call, 7);
        let outcome = tokio::time::timeout(
            Duration::from_secs(3),
            crate::indicate_typing(&channel, "telegram", "7", None, &mut call),
        )
        .await;

        assert!(!outcome.unwrap(), "{name}");
        assert_eq!(call, 8);
        channel.stop().await.unwrap();
    }
}
