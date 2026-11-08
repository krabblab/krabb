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

#[tokio::test]
async fn replaces_at_a_turn_boundary_without_a_missing_plugin_or_restarting_active_work() {
    let fixture = Fixture::new();
    let plugins = fixture.plugins().await;
    let old = plugins.get("memory").await.unwrap();
    let turn = plugins.lifecycle.read().await;

    fixture.version("0.2.0", "permissions=['network']\nsecrets=['KRABB_EXAMPLE_KEY']\n");

    let snapshot = fixture.preview();
    let root = fixture.root.clone();
    let registry = plugins.clone();
    let snapshot_name = name(&snapshot).to_owned();
    let hash = snapshot.candidates["memory"].hash.clone();

    let task =
        tokio::spawn(
            async move { replace(&root, &registry, "memory", &snapshot_name, &hash).await },
        );

    tokio::time::timeout(Duration::from_secs(10), async {
        while plugins.lifecycle.try_read().is_ok() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    assert!(Arc::ptr_eq(&old, &plugins.get("memory").await.unwrap()));

    assert_eq!(
        old.call(Request::call(1, "hello", serde_json::json!({}))).await.unwrap().result.unwrap()["version"],
        "0.1.0"
    );

    assert_eq!(load_lock_at(&fixture.root).unwrap().plugins["memory"].version, "0.1.0");

    let waiting = plugins.clone();

    let next_turn = tokio::spawn(async move {
        let _turn = waiting.lifecycle.read().await;
        waiting.get("memory").await.unwrap().hello.version.clone()
    });

    drop(turn);

    assert_eq!(next_turn.await.unwrap(), "0.2.0");
    assert_eq!(task.await.unwrap().unwrap()["status"], "updated");
    assert!(old.process.lock().await.is_none());

    let entry = &load_lock_at(&fixture.root).unwrap().plugins["memory"];

    assert_eq!(entry.permissions, ["network"]);
    assert_eq!(entry.variables, ["KRABB_EXAMPLE_KEY"]);

    assert_eq!(
        fs::read_dir(fixture.root.join("run/plugin-images"))
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
            .count(),
        1
    );

    plugins.get("memory").await.unwrap().restart().await.unwrap();
    crate::tests::stop_registry(&plugins).await;

    assert_eq!(
        fs::read_dir(fixture.root.join("run/plugin-images"))
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
            .count(),
        0
    );
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
