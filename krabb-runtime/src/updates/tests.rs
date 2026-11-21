use super::*;
use std::fs;

struct Fixture {
    root: PathBuf,
    source: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = crate::tests::test_root("replacement");

        init_at(&root).unwrap();

        let source = root.join("source");

        fs::create_dir_all(source.join("bin")).unwrap();

        let fixture = Self { root, source };

        fixture.version("0.1.0", "");

        link_at(
            Source {
                id: "memory".into(),
                source: Some(fixture.source.display().to_string()),
                revision: None,
                version: None,
                yes: true,
            },
            false,
            &fixture.root,
        )
        .unwrap();

        fixture
    }

    fn version(&self, version: &str, declarations: &str) {
        fs::write(self.source.join("krabb-plugin.toml"), format!(
            "id='memory'\nversion='{version}'\nprotocol={{major=0,minor=1}}\ncapabilities=['memory']\n{declarations}"
        )).unwrap();

        crate::tests::write_test_plugin(
            &self.source.join("bin").join(plugin_name("memory")),
            "memory",
            "memory",
            version,
        );
    }

    fn preview(&self) -> UpdatePreviewSnapshot {
        let lock = load_lock_at(&self.root).unwrap();

        inspect_update(&self.root, "memory", &lock.plugins["memory"]).unwrap();

        assert!(preview_updates_at_inner(&self.root, true, false).unwrap());

        load_update_preview_snapshot_at(&self.root).unwrap()
    }

    async fn plugins(&self) -> Plugins {
        let plugins = Plugins::default();

        load_plugin(&self.root, "memory", None, false, &Config::default(), &plugins).await.unwrap();
        plugins
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn name(snapshot: &UpdatePreviewSnapshot) -> &str {
    snapshot.root.file_name().unwrap().to_str().unwrap()
}

/// A plugin that records overlapping instances and holds `slow` calls until released.
fn exclusive(fixture: &Fixture, version: &str, declarations: &str) {
    let marker = fixture.root.join("markers");

    fs::create_dir_all(&marker).unwrap();

    fs::write(
        fixture.source.join("krabb-plugin.toml"),
        format!(
            "id='memory'\nversion='{version}'\nprotocol={{major=0,minor=1}}\ncapabilities=['memory']\n{declarations}"
        ),
    )
    .unwrap();

    let marker = marker.display();
    let path = fixture.source.join("bin").join(plugin_name("memory"));

    fs::write(
        &path,
        format!(
            "#!/bin/sh\n[ -e '{marker}/running' ] && touch '{marker}/overlap'\ntouch '{marker}/running'\nwhile IFS= read -r line; do\ncase \"$line\" in\n*hello*) printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocol\":{{\"major\":0,\"minor\":1}},\"id\":\"memory\",\"version\":\"{version}\",\"capabilities\":[\"memory\"]}}}}' ;;\n*slow*) touch '{marker}/started'; while [ ! -e '{marker}/release' ]; do sleep 0.01; done; printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{{\"version\":\"{version}\"}}}}' ;;\n*shutdown*) rm -f '{marker}/running'; printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{{\"ok\":true}}}}'; exit 0 ;;\nesac\ndone\n"
        ),
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Links the exclusive plugin and holds a `slow` request on it until `release` appears.
async fn held(fixture: &Fixture) -> (Plugins, Arc<Live>, tokio::task::JoinHandle<Response>) {
    exclusive(fixture, "0.1.0", "");

    link_at(
        Source {
            id: "memory".into(),
            source: Some(fixture.source.display().to_string()),
            revision: None,
            version: None,
            yes: true,
        },
        false,
        &fixture.root,
    )
    .unwrap();

    let plugins = fixture.plugins().await;
    let old = plugins.get("memory").await.unwrap();
    let caller = Arc::clone(&old);
    let started = fixture.root.join("markers/started");

    let request = tokio::spawn(async move {
        caller.call(Request::call(7, "slow", serde_json::json!({}))).await.unwrap()
    });

    tokio::time::timeout(Duration::from_secs(10), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    (plugins, old, request)
}

#[cfg(unix)]
#[tokio::test]
async fn switches_over_between_requests_without_pausing_other_work_or_overlapping() {
    let fixture = Fixture::new();
    let (plugins, old, request) = held(&fixture).await;
    let markers = fixture.root.join("markers");

    // Another conversation's turn no longer holds updates back.
    let _turn = plugins.lifecycle.read().await;

    exclusive(&fixture, "0.2.0", "permissions=['network']\nsecrets=['KRABB_EXAMPLE_KEY']\n");

    let snapshot = fixture.preview();
    let root = fixture.root.clone();
    let registry = plugins.clone();
    let snapshot_name = name(&snapshot).to_owned();
    let hash = snapshot.candidates["memory"].hash.clone();

    let task =
        tokio::spawn(
            async move { replace(&root, &registry, "memory", &snapshot_name, &hash).await },
        );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(!task.is_finished(), "the switch waits for the request in flight");
    assert_eq!(load_lock_at(&fixture.root).unwrap().plugins["memory"].version, "0.1.0");

    fs::write(markers.join("release"), "").unwrap();

    assert_eq!(request.await.unwrap().result.unwrap()["version"], "0.1.0");
    assert_eq!(task.await.unwrap().unwrap()["status"], "updated");
    assert!(!markers.join("overlap").exists(), "two versions must never run together");

    let current = plugins.get("memory").await.unwrap();

    assert_eq!(current.hello.version, "0.2.0");
    assert!(Arc::ptr_eq(&current.process, &old.process));

    assert_eq!(
        old.call(Request::call(1, "hello", serde_json::json!({}))).await.unwrap().result.unwrap()["version"],
        "0.2.0",
        "callers holding the previous version reach the new process"
    );

    let entry = &load_lock_at(&fixture.root).unwrap().plugins["memory"];

    assert_eq!(entry.version, "0.2.0");
    assert_eq!(entry.permissions, ["network"]);
    assert_eq!(entry.variables, ["KRABB_EXAMPLE_KEY"]);

    assert_eq!(
        fs::read_dir(fixture.root.join("run/plugin-images"))
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
            .count(),
        1
    );

    current.restart().await.unwrap();
    crate::tests::stop_registry(&plugins).await;

    assert_eq!(
        fs::read_dir(fixture.root.join("run/plugin-images"))
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
            .count(),
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn an_unload_during_the_wait_wins_without_deadlocking_the_update() {
    let fixture = Fixture::new();
    let (plugins, _, request) = held(&fixture).await;
    let markers = fixture.root.join("markers");

    exclusive(&fixture, "0.2.0", "");

    let snapshot = fixture.preview();
    let root = fixture.root.clone();
    let registry = plugins.clone();
    let snapshot_name = name(&snapshot).to_owned();
    let hash = snapshot.candidates["memory"].hash.clone();

    let update =
        tokio::spawn(
            async move { replace(&root, &registry, "memory", &snapshot_name, &hash).await },
        );

    tokio::time::sleep(Duration::from_millis(200)).await;

    let registry = plugins.clone();
    let unload = tokio::spawn(async move { unload_plugin("memory", &registry).await.unwrap() });

    tokio::time::sleep(Duration::from_millis(200)).await;
    fs::write(markers.join("release"), "").unwrap();

    request.await.unwrap();

    let (update, unloaded) = tokio::time::timeout(Duration::from_secs(10), async {
        (update.await.unwrap(), unload.await.unwrap())
    })
    .await
    .expect("an update and an unload never wait on each other");

    assert!(unloaded);
    assert!(update.unwrap_err().to_string().contains("removed"));
    assert!(plugins.get("memory").await.is_none());
    assert_eq!(load_lock_at(&fixture.root).unwrap().plugins["memory"].version, "0.1.0");
}

#[tokio::test]
async fn a_version_that_cannot_be_restored_is_unloaded_with_a_next_step() {
    let fixture = Fixture::new();
    let plugins = fixture.plugins().await;
    let old = plugins.get("memory").await.unwrap();

    let image = {
        let slot = old.process.lock().await;

        slot.as_ref().unwrap().image.as_ref().unwrap().binary.clone()
    };

    // Windows refuses to delete a running executable but lets it be renamed, though it can deny
    // the rename for a moment while the file is still being scanned or mapped.
    let moved = image.with_extension("moved");
    let mut attempts = 0;

    while let Err(error) = fs::rename(&image, &moved) {
        attempts += 1;

        assert!(error.kind() == std::io::ErrorKind::PermissionDenied && attempts < 40, "{error}");

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    fixture.version("0.2.0", "");

    crate::tests::write_test_plugin(
        &fixture.source.join("bin").join(plugin_name("memory")),
        "wrong",
        "memory",
        "0.2.0",
    );

    let snapshot = fixture.preview();

    let error = replace(
        &fixture.root,
        &plugins,
        "memory",
        name(&snapshot),
        &snapshot.candidates["memory"].hash,
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("krabb service restart"), "{error}");
    assert!(plugins.get("memory").await.is_none(), "a dead plugin is never left registered");
    assert!(!moved.exists(), "the unloaded version's image is discarded");
    assert_eq!(load_lock_at(&fixture.root).unwrap().plugins["memory"].version, "0.1.0");
}

#[tokio::test]
async fn failed_candidate_and_commit_keep_the_old_plugin_running() {
    let fixture = Fixture::new();
    let plugins = fixture.plugins().await;
    let old = plugins.get("memory").await.unwrap();

    fixture.version("0.2.0", "");

    crate::tests::write_test_plugin(
        &fixture.source.join("bin").join(plugin_name("memory")),
        "wrong",
        "memory",
        "0.2.0",
    );

    let snapshot = fixture.preview();
    let turn = plugins.lifecycle.read().await;

    assert!(
        replace(
            &fixture.root,
            &plugins,
            "memory",
            name(&snapshot),
            &snapshot.candidates["memory"].hash
        )
        .await
        .is_err()
    );

    assert!(Arc::ptr_eq(&old, &plugins.get("memory").await.unwrap()));

    assert_eq!(
        old.call(Request::call(1, "hello", serde_json::json!({}))).await.unwrap().result.unwrap()["version"],
        "0.1.0",
        "a failed replacement restores the previous version"
    );

    drop(turn);

    fixture.version("0.2.0", "");

    let snapshot = fixture.preview();
    let mut commits = 0;

    let error = replace_with_commit(
        &fixture.root,
        &plugins,
        "memory",
        name(&snapshot),
        &snapshot.candidates["memory"].hash,
        |root, lock| {
            commits += 1;

            if commits == 1 {
                return Err("injected commit failure".into());
            }

            save_lock_at(root, lock)
        },
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("injected commit failure"));
    assert!(Arc::ptr_eq(&old, &plugins.get("memory").await.unwrap()));
    assert_eq!(load_lock_at(&fixture.root).unwrap().plugins["memory"].version, "0.1.0");

    assert_eq!(
        verify_plugin_integrity(
            &fixture.root,
            "memory",
            &fixture.root.join("plugins/memory/bin").join(plugin_name("memory"))
        )
        .unwrap()
        .version,
        "0.1.0"
    );

    assert_eq!(
        old.call(Request::call(1, "hello", serde_json::json!({}))).await.unwrap().result.unwrap()["version"],
        "0.1.0"
    );

    crate::tests::stop_registry(&plugins).await;
}

#[tokio::test]
async fn review_is_bound_to_the_snapshot_and_inactive_plugins_stay_inactive() {
    let fixture = Fixture::new();

    fixture.version("0.2.0", "permissions=['network']\nsecrets=['KRABB_EXAMPLE_KEY']\n");

    let snapshot = fixture.preview();
    let candidate = &snapshot.candidates["memory"];
    let entry = &load_lock_at(&fixture.root).unwrap().plugins["memory"];
    let changes = declarations(entry, &candidate.manifest);

    assert_eq!(changes["permissions"]["after"], serde_json::json!(["network"]));
    assert!(review(&changes).iter().any(|line| line.contains("KRABB_EXAMPLE_KEY")));
    assert!(review(&serde_json::Value::Null).is_empty());

    assert!(
        declarations(entry, &read_manifest(&fixture.root.join("plugins/memory")).unwrap())
            .as_object()
            .unwrap()
            .is_empty()
    );

    let plugins = Plugins::default();

    for (id, directory, hash) in [
        ("../memory", name(&snapshot), candidate.hash.as_str()),
        ("memory", "../escape", candidate.hash.as_str()),
        ("memory", name(&snapshot), "changed"),
    ] {
        assert!(replace(&fixture.root, &plugins, id, directory, hash).await.is_err());
    }

    replace(&fixture.root, &plugins, "memory", name(&snapshot), &candidate.hash).await.unwrap();

    assert!(plugins.get("memory").await.is_none());

    assert!(
        replace(&fixture.root, &plugins, "memory", name(&snapshot), &candidate.hash)
            .await
            .unwrap_err()
            .to_string()
            .contains("stale")
    );
}

#[test]
fn image_cleanup_keeps_leased_generations_and_reclaims_abandoned_files() {
    let fixture = Fixture::new();
    let source = fixture.source.join("bin").join(plugin_name("memory"));
    let image = Image::new(&fixture.root, &source).unwrap();
    let stale = fixture.root.join("run/plugin-images/abandoned");

    fs::create_dir(&stale).unwrap();
    fs::write(stale.join("binary"), "stale").unwrap();
    Image::clean(&fixture.root).unwrap();

    assert!(image.binary.is_file());
    assert!(!stale.exists());

    let binary = image.binary.clone();

    drop(image);

    assert!(!binary.exists());
}

#[test]
fn image_cleanup_defers_an_orphaned_image_that_cannot_be_removed() {
    let fixture = Fixture::new();
    let images = fixture.root.join("run/plugin-images");
    let orphaned = images.join("orphaned");
    let abandoned = images.join("abandoned");

    for directory in [&orphaned, &abandoned] {
        fs::create_dir_all(directory).unwrap();
        fs::write(directory.join("binary"), "stale").unwrap();
    }

    Image::clean_with(&fixture.root, |path| {
        if path == orphaned {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        }

        fs::remove_dir_all(path)
    })
    .unwrap();

    assert!(orphaned.join("binary").is_file());
    assert!(!abandoned.exists());

    Image::clean(&fixture.root).unwrap();

    assert!(!orphaned.exists());
}

#[tokio::test]
async fn refuses_previews_and_snapshots_that_change_after_confirmation() {
    let fixture = Fixture::new();
    let plugins = fixture.plugins().await;
    let binary = |root: &Path| root.join("bin").join(plugin_name("memory"));

    fixture.version("0.2.0", "");

    assert!(preview_updates_at_inner(&fixture.root, true, false).unwrap());

    let preview = fixture.root.join(".plugin-update-preview/memory");
    let original = fs::read(binary(&preview)).unwrap();
    let tampered = [original.as_slice(), b"\n"].concat();

    fs::write(binary(&preview), &tampered).unwrap();

    let Err(error) = load_update_preview_snapshot_at(&fixture.root) else {
        panic!("a changed preview must not be snapshotted");
    };

    assert!(error.to_string().contains("changed"), "{error}");

    fs::write(binary(&preview), &original).unwrap();

    let snapshot = load_update_preview_snapshot_at(&fixture.root).unwrap();
    let hash = snapshot.candidates["memory"].hash.clone();

    fs::write(binary(&snapshot.root.join("memory")), &tampered).unwrap();

    let error =
        replace(&fixture.root, &plugins, "memory", name(&snapshot), &hash).await.unwrap_err();

    assert!(error.to_string().contains("changed"), "{error}");
    assert_eq!(load_lock_at(&fixture.root).unwrap().plugins["memory"].version, "0.1.0");
    assert_eq!(plugins.get("memory").await.unwrap().hello.version, "0.1.0");
}
