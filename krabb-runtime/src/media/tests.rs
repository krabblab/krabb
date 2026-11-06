use super::*;
use krabb_core::types::Role;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let root = std::env::temp_dir().join(format!(
            "krabb-retained-media-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));

        fs::create_dir_all(root.join("media")).unwrap();
        Self(root)
    }

    fn media(&self) -> PathBuf {
        self.0.join("media")
    }

    fn image(&self, name: &str, byte: u8) -> Content {
        let path = self.media().join(name);
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();

        bytes.push(byte);
        fs::write(&path, bytes).unwrap();
        Content::Image { uri: format!("file://{}", path.display()), alt: Some("fixture".into()) }
    }

    fn catalog(&self) -> PathBuf {
        self.0.join("data/sessions/index.json")
    }

    fn files(&self) -> usize {
        fs::read_dir(self.media().join("pinned")).map_or(0, |entries| entries.count())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn path(content: &Content) -> PathBuf {
    let Content::Image { uri, .. } = content else {
        panic!("expected retained image: {content:?}");
    };

    PathBuf::from(uri.strip_prefix("file://").unwrap())
}

fn message(id: &str, content: Content) -> Message {
    Message {
        id: id.into(),
        session: "main".into(),
        role: Role::User,
        sender: None,
        content: vec![content],
    }
}

#[test]
fn retained_media_bounds_bytes_and_files_across_restarts_and_deduplicates() {
    for (bytes, files) in [(18, 100), (1000, 2)] {
        let fixture = Fixture::new();
        let mut store = Store::load(fixture.catalog()).unwrap();
        store.create("main", "local").unwrap();

        for index in 0..2 {
            let image = fixture.image(&format!("{index}.png"), index);
            let pinned = pin_bounded(vec![image], &fixture.media(), bytes, files).remove(0);
            store.push("main", message(&format!("{index}"), pinned)).unwrap();
        }

        drop(store);
        let mut store = Store::load(fixture.catalog()).unwrap();
        let duplicate = fixture.image("different-url.png", 0);
        let duplicate = pin_bounded(vec![duplicate], &fixture.media(), bytes, files).remove(0);

        assert_eq!(duplicate, store.sessions["main"].messages[0].content[0]);

        for index in 2..10 {
            let image = fixture.image("incoming.png", index);
            let rejected = pin_bounded(vec![image], &fixture.media(), bytes, files).remove(0);

            assert!(
                matches!(&rejected, Content::Text { text } if text.contains("storage is full"))
            );
            store.push("main", message(&format!("{index}"), rejected)).unwrap();

            assert_eq!(fixture.files(), 2);
        }

        assert_eq!(fs::read(path(&duplicate)).unwrap().len(), 9);
        store.queue_delivery("notice", "local", "main", true, None, "still writable").unwrap();
        let reopened = Store::load(fixture.catalog()).unwrap();

        assert_eq!(reopened.outbox[0].text, "still writable");
        assert_eq!(reopened.sessions["main"].messages.len(), 10);
    }
}

#[test]
fn retained_media_reclaims_only_after_last_durable_reference() {
    let fixture = Fixture::new();

    let mut store = Store::load(fixture.catalog()).unwrap();

    store.create("main", "local").unwrap();

    let image = pin(vec![fixture.image("image.png", 1)], &fixture.media()).remove(0);
    let pinned = path(&image);

    store.push("main", message("one", image.clone())).unwrap();
    store.fork("main", "fork").unwrap();
    store.clear_history("main").unwrap();

    assert!(pinned.is_file(), "Fork still references the image");

    store.queue("main", message("queued", image.clone())).unwrap();
    store.remove_deep("fork").unwrap();

    assert!(pinned.is_file(), "Queue still references the image");

    let queued = store.sessions["main"].queued[0].clone();

    store.begin("main", queued).unwrap();
    store.clear_history("main").unwrap();

    assert!(pinned.is_file(), "Inflight message still references the image");

    store.sessions.get_mut("main").unwrap().compacted_context =
        Some(vec![message("context", image)]);

    store.clear("main", "idle").unwrap();

    assert!(pinned.is_file(), "Compacted context still references the image");

    drop(store);

    let mut store = Store::load(fixture.catalog()).unwrap();

    assert!(pinned.is_file());

    store.clear_history("main").unwrap();

    assert!(!pinned.exists());
}

#[test]
fn retained_media_compaction_deletion_and_orphan_recovery() {
    let fixture = Fixture::new();
    let mut store = Store::load(fixture.catalog()).unwrap();

    store.create("main", "local").unwrap();

    let image = pin(vec![fixture.image("old.png", 1)], &fixture.media()).remove(0);
    let old = path(&image);

    store.push("main", message("old", image)).unwrap();

    for index in 0..krabb_core::session::MESSAGE_HISTORY_LIMIT {
        store
            .push("main", message(&index.to_string(), Content::Text { text: "local".into() }))
            .unwrap();
    }

    assert!(!old.exists());

    let image = pin(vec![fixture.image("retained.png", 2)], &fixture.media()).remove(0);
    let retained = path(&image);

    store.push("main", message("retained", image)).unwrap();

    let orphan = pin(vec![fixture.image("orphan.png", 3)], &fixture.media()).remove(0);
    let orphan = path(&orphan);

    drop(store);

    let mut store = Store::load(fixture.catalog()).unwrap();

    store.save().unwrap();

    assert!(!orphan.exists(), "Crash orphan is reclaimed on the next durable save");
    assert!(retained.is_file());

    store.remove_deep("main").unwrap();

    assert!(!retained.exists());
}

#[test]
fn retained_media_failed_save_preserves_previous_snapshot_images() {
    let fixture = Fixture::new();
    let catalog = fixture.0.join("sessions.json");
    let mut store = Store::load(&catalog).unwrap();

    store.media_root = Some(fixture.media());
    store.create("main", "local").unwrap();

    let image = pin(vec![fixture.image("image.png", 1)], &fixture.media()).remove(0);
    let pinned = path(&image);

    store.push("main", message("one", image)).unwrap();

    let saved = fixture.0.join("saved.json");

    fs::rename(&catalog, &saved).unwrap();
    fs::create_dir(&catalog).unwrap();

    assert!(store.clear_history("main").is_err());
    assert!(pinned.is_file());
    assert_eq!(store.sessions["main"].messages.len(), 1);

    fs::remove_dir(&catalog).unwrap();
    fs::rename(saved, &catalog).unwrap();

    let reopened = Store::load(catalog).unwrap();

    assert_eq!(path(&reopened.sessions["main"].messages[0].content[0]), pinned);

    store.clear_history("main").unwrap();

    assert!(!pinned.exists());
}

#[test]
fn retained_media_admission_rejects_cancelled_full_and_duplicate_messages() {
    let fixture = Fixture::new();
    let mut store = Store::load(fixture.catalog()).unwrap();
    let mut incoming = message("incoming", fixture.image("incoming.png", 1));

    assert!(!admitted(&store, &incoming, false), "No session capacity was admitted");

    store.create("main", "local").unwrap();
    store.cancel("main").unwrap();

    assert!(!admitted(&store, &incoming, false));

    let session = store.sessions.get_mut("main").unwrap();

    session.status = "working".into();
    session.queued = vec![message("queued", Content::Text { text: "queued".into() }); 100];

    assert!(!admitted(&store, &incoming, false));
    assert!(!admitted(&store, &incoming, true));
    assert_eq!(fixture.files(), 0);

    store.sessions.get_mut("main").unwrap().queued.pop();

    assert!(admitted(&store, &incoming, false));

    incoming.content = pin(incoming.content, &fixture.media());
    store.queue("main", incoming.clone()).unwrap();

    assert_eq!(fixture.files(), 1);
    assert!(!admitted(&store, &incoming, false));

    store.sessions.get_mut("main").unwrap().status = "idle".into();

    assert!(admitted(&store, &incoming, true));

    store.begin("main", incoming.clone()).unwrap();

    assert!(!admitted(&store, &incoming, true));
}

#[test]
fn retained_media_preserves_deferred_references_and_ignores_unmanaged_files() {
    let fixture = Fixture::new();
    let image = pin(vec![fixture.image("image.png", 1)], &fixture.media()).remove(0);
    let pinned = path(&image);
    let unrelated = fixture.media().join("pinned/notes.txt");

    fs::write(&unrelated, "local").unwrap();

    let deferred = serde_json::json!({"items": [image, null, 1]});
    let store = Store::default();

    reclaim(&fixture.media(), &store, std::iter::once(&deferred)).unwrap();

    assert!(pinned.is_file());

    reclaim(&fixture.media(), &store, std::iter::empty()).unwrap();

    assert!(!pinned.exists());
    assert!(unrelated.is_file());
}

#[test]
fn retained_media_counts_legacy_storage_and_preserves_non_image_content() {
    let fixture = Fixture::new();
    let image = fixture.image("image.png", 1);

    fs::create_dir(fixture.media().join("pinned")).unwrap();

    let legacy = fixture.media().join("pinned/legacy.bin");

    fs::write(&legacy, [0; 20]).unwrap();

    let rejected = pin_bounded(vec![image.clone()], &fixture.media(), 19, 10);

    assert!(matches!(&rejected[0], Content::Text { text } if text.contains("storage is full")));
    assert_eq!(fs::read(&legacy).unwrap().len(), 20);

    fs::remove_file(&legacy).unwrap();
    fs::write(&legacy, b"\x89PNG\r\n\x1a\nold").unwrap();

    let retained = Content::Image { uri: format!("file://{}", legacy.display()), alt: None };

    assert_eq!(
        pin_bounded(vec![retained.clone()], &fixture.media(), 0, 0),
        vec![retained],
        "Already retained legacy images remain usable even when the budget is full"
    );

    fs::remove_file(&legacy).unwrap();
    fs::create_dir(&legacy).unwrap();

    let rejected = pin(vec![image], &fixture.media());

    assert!(matches!(&rejected[0], Content::Text { text } if text.contains("unavailable")));

    let invalid = fixture.media().join("invalid.png");

    fs::write(&invalid, "invalid").unwrap();

    let content = vec![
        Content::Text { text: "keep text".into() },
        Content::Image { uri: "file:///unavailable/image.png".into(), alt: None },
        Content::Image { uri: format!("file://{}", invalid.display()), alt: None },
    ];

    assert_eq!(pin(content.clone(), &fixture.media()), content);
}

#[cfg(unix)]
#[test]
fn retained_media_rejects_symlinked_storage_without_touching_target() {
    let fixture = Fixture::new();
    let outside = fixture.0.join("outside");

    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, fixture.media().join("pinned")).unwrap();

    let image = fixture.image("image.png", 1);
    let result = pin(vec![image], &fixture.media());

    assert!(matches!(&result[0], Content::Text { text } if text.contains("unavailable")));
    assert!(reclaim(&fixture.media(), &Store::default(), std::iter::empty()).is_err());
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn retained_media_bridge_pins_only_admitted_messages_with_local_plugins() {
    use crate::{ApprovalMode, AsyncMutex, Live, Plugins, Stop, approval, process::Process};
    use std::{collections::BTreeMap, sync::Arc, sync::Mutex, time::Duration};

    for mode in ["accepted", "cancelled", "capacity", "queue"] {
        let fixture = Fixture::new();
        let image = fixture.image("incoming.png", 1);

        let event = serde_json::json!({
            "id": 42, "chat": 7, "sender": 8, "private": true,
            "content": [image, {"kind": "text", "text": "describe"}]
        });

        let channel_script = r#"
import json, sys
event = json.loads(sys.argv[1])
sent = False
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    result = {}
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': 'telegram',
                  'version': '0.1.0', 'capabilities': ['channel']}
    elif method == 'poll':
        result = {'events': [] if sent else [event]}
        sent = True
    elif method == 'send':
        result = {'message_id': 1}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
    if method == 'shutdown':
        break
"#;

        let model_script = r#"
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    result = {}
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': 'local',
                  'version': '0.1.0', 'capabilities': ['model', 'vision']}
    elif method == 'generate':
        with open(sys.argv[1], 'w') as output:
            json.dump(request, output)
        result = {'text': 'local image accepted', 'stop': 'stop'}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
    if method == 'shutdown':
        break
"#;

        let captured = fixture.0.join("model.json");

        let channel =
            Process::start_with("python3", ["-u", "-c", channel_script, &event.to_string()])
                .await
                .unwrap();

        let model =
            Process::start_with("python3", ["-u", "-c", model_script, captured.to_str().unwrap()])
                .await
                .unwrap();

        let plugins = Plugins::default();

        plugins.insert(Live::new(channel)).await;
        plugins.insert(Live::new(model)).await;

        let mut store = Store::load(fixture.catalog()).unwrap();

        if mode == "capacity" {
            for index in 0..crate::state::LIMIT {
                store.create(format!("session-{index}"), "local").unwrap();
            }
        } else {
            store.create("telegram-7", "local").unwrap();

            if mode == "cancelled" {
                store.cancel("telegram-7").unwrap();
            } else if mode == "queue" {
                let session = store.sessions.get_mut("telegram-7").unwrap();

                session.status = "working".into();

                session.queued =
                    vec![message("queued", Content::Text { text: "queued".into() }); 100];

                store.save().unwrap();
            }
        }

        let sessions = Arc::new(Mutex::new(store));
        let stop = Arc::new(Stop::new());

        let monitor = async {
            let completed = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let done = {
                        let store = sessions.lock().unwrap();

                        if mode == "accepted" {
                            store.sessions["telegram-7"]
                                .messages
                                .iter()
                                .any(|message| message.role == Role::Assistant)
                        } else {
                            store.known("telegram", "42")
                        }
                    };

                    if done {
                        break;
                    }

                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            stop.signal();
            completed
        };

        let bridge = crate::bridge_with_media(
            &plugins,
            "telegram",
            "local",
            "local",
            Arc::clone(&sessions),
            Arc::clone(&stop),
            Arc::new(Mutex::new(BTreeMap::new())),
            BTreeMap::new(),
            ApprovalMode::Off,
            false,
            fixture.0.clone(),
            fixture.0.join("workspace"),
            fixture.media(),
            Arc::new(AsyncMutex::new(approval::Gate::new().unwrap())),
            10,
        );

        let (result, completed) = tokio::join!(bridge, monitor);

        for (_, plugin) in plugins.all().await {
            plugin.stop().await.unwrap();
        }

        result.unwrap();
        completed.unwrap();

        if mode == "accepted" {
            assert_eq!(
                fs::read_dir(fixture.0.join("workspace/attachments/channels.telegram/pinned"))
                    .map_or(0, |entries| entries.count()),
                1
            );

            let request = fs::read_to_string(&captured).unwrap();

            assert!(request.contains("data:image/png;base64,"));
            assert!(request.contains("describe"));
            assert!(!request.contains("file://"));
        } else {
            assert_eq!(
                fs::read_dir(fixture.0.join("workspace/attachments/channels.telegram/pinned"))
                    .map_or(0, |entries| entries.count()),
                0,
                "Rejected {mode} message must not pin images"
            );

            assert!(!captured.exists());
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn retained_voice_uses_owned_storage_without_deleting_the_attachment() {
    use crate::{Live, Plugins, Process};
    let fixture = Fixture::new();
    let workspace = fixture.0.join("workspace");
    let media = workspace.join("attachments/channels.telegram");

    fs::create_dir_all(media.join("pinned")).unwrap();

    let voice = media.join("pinned/voice.ogg");

    fs::write(&voice, "fixture").unwrap();

    let script = r#"
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if method == 'hello':
        result = {'protocol': {'major': 0, 'minor': 1}, 'id': 'whisper', 'version': '0.1.0', 'capabilities': ['speech']}
    elif method == 'shutdown':
        break
    else:
        assert method == 'retained'
        assert request['params']['owner'] == 'channels.telegram'
        assert request['params']['workspace'] == sys.argv[1]
        result = {'text': 'retained words'}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

    let process = Process::start_with("python3", ["-u", "-c", script, workspace.to_str().unwrap()])
        .await
        .unwrap();

    let plugins = Plugins::default();

    plugins.insert(Live::new(process)).await;

    let content = crate::transcribe_media(
        &plugins,
        "telegram",
        vec![Content::Audio { uri: format!("file://{}", voice.display()), mime: None }],
        &media,
        &mut 2,
    )
    .await;

    assert_eq!(content, vec![Content::Text { text: "retained words".into() }]);
    assert!(voice.is_file());

    plugins.all().await.into_iter().next().unwrap().1.stop().await.unwrap();
}
