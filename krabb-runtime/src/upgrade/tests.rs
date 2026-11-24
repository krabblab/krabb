use std::fs;

use super::*;

fn host_archive(version: &str) -> String {
    let (target, extension) =
        releases::target(std::env::consts::OS, std::env::consts::ARCH).unwrap();

    releases::archive("core", version, target, extension)
}

fn temp_root(name: &str) -> PathBuf {
    let root = crate::tests::test_root(&format!("upgrade-{name}"));

    fs::create_dir_all(&root).unwrap();

    root
}

fn sha(path: &Path) -> String {
    crate::digest::sha256(fs::read(path).unwrap())
}

/// Packs `files` into the release archive `name`, in the format this host's releases use.
fn package(release: &Path, name: &str, files: &[(&str, &str)]) -> String {
    let zip = name.ends_with(".zip");
    let directory = name.trim_end_matches(".tar.gz").trim_end_matches(".zip");
    let work = release.join(".work").join(directory);

    for (path, content) in files {
        let file = work.join(path);

        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, content).unwrap();
    }

    let archive = release.join(name);
    let format: &[&str] = if zip { &["--format=zip", "-cf"] } else { &["-czf"] };

    let status = std::process::Command::new("tar")
        .args(format)
        .arg(&archive)
        .arg("-C")
        .arg(release.join(".work"))
        .arg(directory)
        .status()
        .unwrap();

    assert!(status.success());

    format!("{}  {name}\n", sha(&archive))
}

const NEXT: Protocol = Protocol { major: 1, minor: 0 };

const ECHO: &str = "id = 'echo'\nversion = '0.2.0'\ncapabilities = ['tool']\n\
                    permissions = ['network']\nsecrets = ['KRABB_ECHO_KEY']\n\n\
                    [protocol]\nmajor = 1\nminor = 0\n";

/// A host release directory with its core archive and checksum list.
fn release(root: &Path, version: &str) -> PathBuf {
    let release = root.join("release");
    let exe = std::env::consts::EXE_SUFFIX;
    let core = host_archive(version);

    fs::create_dir_all(&release).unwrap();

    let sums = package(
        &release,
        &core,
        &[
            (&format!("bin/krabb{exe}"), "new krabb"),
            (&format!("bin/krabb-daemon{exe}"), "new daemon"),
        ],
    );

    fs::write(release.join("SHA256SUMS"), sums).unwrap();

    release
}

/// Publishes echo 0.1.0 (protocol 0.1) and `version` (`manifest`) to a local index and
/// registry, returning the index location.
fn publish(root: &Path, manifest: &str, version: &str) -> String {
    let registry = root.join("registry");
    let work = root.join("plugins");

    let (target, extension) =
        releases::target(std::env::consts::OS, std::env::consts::ARCH).unwrap();

    let name = format!("krabb-echo-v{version}-{target}.{extension}");
    let exe = std::env::consts::EXE_SUFFIX;

    fs::create_dir_all(&work).unwrap();

    package(
        &work,
        &name,
        &[("krabb-plugin.toml", manifest), (&format!("bin/krabb-plugin-echo{exe}"), "echo")],
    );

    let archive = work.join(&name);
    let digest = sha(&archive);
    let size = fs::metadata(&archive).unwrap().len();
    let blobs = registry.join("blobs/sha256");

    fs::create_dir_all(&blobs).unwrap();
    fs::copy(&archive, blobs.join(&digest)).unwrap();

    let index = serde_json::json!({
        "schema": 1,
        "registry": format!("file://{}", registry.display()),
        "plugins": {"echo": [
            {
                "version": "0.1.0",
                "protocol": {"major": 0, "minor": 1},
                "released": "2026-11-01",
                "artifacts": {},
            },
            {
                "version": version,
                "protocol": NEXT,
                "released": "2026-11-20",
                "permissions": ["network"],
                "artifacts": {target: {"sha256": digest, "size": size}},
            },
        ]},
    });

    fs::write(root.join("index.json"), index.to_string()).unwrap();

    format!("file://{}", root.join("index.json").display())
}

fn inspection(protocol: Option<Protocol>) -> stage::Inspection {
    stage::Inspection {
        version: "0.2.0".into(),
        schema: stage::Schema { from: 1, to: 1, steps: Vec::new() },
        protocol,
    }
}

fn entry(version: &str, source: &str, linked: bool) -> Entry {
    serde_json::from_value(serde_json::json!({
        "source": source,
        "version": version,
        "protocol": {"major": 0, "minor": 1},
        "capabilities": ["tool"],
        "permissions": ["filesystem"],
        "secrets": [],
        "linked": linked,
    }))
    .unwrap()
}

#[test]
fn changes_list_additions_and_removals() {
    let before = ["network".to_owned(), "filesystem".to_owned()];

    let after = ["network".to_owned(), "memory".to_owned()];

    assert_eq!(
        change("permissions", &before, &after).as_deref(),
        Some("permissions: adds memory, removes filesystem")
    );

    assert_eq!(change("secrets", &before, &before), None);
}

#[test]
fn swaps_replace_binaries_and_sweeps_remove_leftovers() {
    let root = temp_root("swap");
    let live = root.join("krabb");
    let replacement = root.join("new");

    fs::write(&live, "old").unwrap();
    fs::write(&replacement, "new").unwrap();
    stage::swap(&live, &replacement).unwrap();

    assert_eq!(fs::read_to_string(&live).unwrap(), "new");

    fs::write(root.join(".krabb.old-1"), "running").unwrap();
    stage::sweep(&live);

    let names = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(names.len(), 2, "{names:?}");
    assert!(stage::swap(&live, &root.join("missing")).is_err());
    assert_eq!(fs::read_to_string(&live).unwrap(), "new");

    stage::swap(&root.join("fresh"), &replacement).unwrap();

    assert!(stage::writable(&live).is_ok());

    let error = stage::writable(&root.join("missing/krabb")).unwrap_err();

    assert!(error.to_string().contains("cannot replace files"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn staging_verifies_the_host_and_moves_plugins_it_cannot_load() {
    let root = temp_root("stage");
    let home = root.join("home");
    let release = release(&root, "0.2.0");
    let base = format!("file://{}", release.display());
    let location = publish(&root, ECHO, "0.2.0");
    let load = || index::load_from(&home, &location);

    fs::create_dir_all(&home).unwrap();

    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();
    let dir = staged.dir.clone();

    assert_eq!(fs::read_to_string(&staged.krabb).unwrap(), "new krabb");
    assert_eq!(fs::read_to_string(&staged.daemon).unwrap(), "new daemon");

    let mut lock = Lock::default();
    let mut tui = entry("0.1.0", "/src/tui", true);

    tui.protocol = NEXT;
    lock.plugins.insert("echo".into(), entry("0.1.0", "index:echo@0.1.0", false));
    lock.plugins.insert("tui".into(), tui);
    lock.plugins.insert("codex".into(), entry("0.1.0", "index:codex", false));
    lock.plugins.get_mut("codex").unwrap().protocol = NEXT;

    stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &load).unwrap();
    stage::compatible(&staged, &inspection(Some(NEXT))).unwrap();

    assert_eq!(staged.manifests["echo"].permissions, ["network"]);

    let plan = stage::plan(&dir).unwrap();

    assert_eq!(plan.version, "0.2.0");
    assert_eq!(plan.plugins[0].id, "echo");
    assert_eq!(plan.plugins[0].sha256, sha(&dir.join(&plan.plugins[0].file)));

    let schema = stage::Schema { from: 1, to: 2, steps: vec!["move".into()] };

    let preview = preview("0.2.0", &lock, &staged, &schema, &root.join("none.service"), false);

    let update = format!("Update Krabb {VERSION} to 0.2.0");
    let rollback = format!("`krabb update --rollback` restores {VERSION}");

    for row in [
        update.as_str(),
        "echo: 0.1.0 → 0.2.0 (pin to 0.1.0 removed because it cannot run on 0.2.0)",
        "echo permissions: adds network, removes filesystem",
        "echo secrets: adds KRABB_ECHO_KEY",
        "tui: refreshes from its linked development build after the update",
        "codex: stays at 0.1.0 (no longer in the plugin index)",
        "home schema: 1 → 2 (1 migration)",
        "service: not installed",
        rollback.as_str(),
    ] {
        assert!(preview.text.contains(row), "{row}\n{}", preview.text);
    }

    assert_eq!(preview.value["kept"], serde_json::json!(["codex"]));

    assert_eq!(preview.value["refresh"], serde_json::json!(["tui"]));
    assert_eq!(preview.value["supported"], serde_json::json!([]));

    drop(staged);

    assert!(!dir.exists());

    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();

    lock.plugins.remove("tui");
    lock.plugins.remove("codex");
    stage::plugins(&mut staged, &lock, &inspection(Some(Protocol::CURRENT)), &load).unwrap();

    assert!(stage::plan(&staged.dir).unwrap().plugins.is_empty());

    drop(staged);

    fs::write(
        release.join("SHA256SUMS"),
        fs::read_to_string(release.join("SHA256SUMS")).unwrap().replacen(
            &sha(&release.join(host_archive("0.2.0"))),
            &"0".repeat(64),
            1,
        ),
    )
    .unwrap();

    let error = stage::prepare(&home, "0.2.0", Some(&base)).err().unwrap();

    assert!(error.to_string().contains("failed verification"), "{error}");
    assert!(stage::prepare(&home, "0.3.0", Some(&base)).is_err());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn staging_updates_every_official_plugin_except_pinned_ones() {
    let root = temp_root("stage-all");
    let home = root.join("home");
    let release = release(&root, "0.2.0");
    let base = format!("file://{}", release.display());
    let location = publish(&root, ECHO, "0.2.0");
    let load = || index::load_from(&home, &location);

    fs::create_dir_all(&home).unwrap();

    let current = |version: &str, source: &str| {
        let mut entry = entry(version, source, false);

        entry.protocol = NEXT;
        entry
    };

    let mut lock = Lock::default();

    lock.plugins.insert("echo".into(), current("0.1.0", "index:echo"));

    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();

    stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &load).unwrap();

    let plan = stage::plan(&staged.dir).unwrap();

    assert_eq!(plan.plugins.len(), 1);
    assert_eq!(plan.plugins[0].id, "echo");
    assert_eq!(plan.plugins[0].pin, None);

    let schema = stage::Schema { from: 1, to: 1, steps: Vec::new() };
    let shown = preview("0.2.0", &lock, &staged, &schema, &root.join("none.service"), false);

    assert!(shown.text.contains("echo: 0.1.0 → 0.2.0\n"), "{}", shown.text);

    drop(staged);

    for (source, version, reason) in
        [("index:echo@0.1.0", "0.1.0", "pinned to 0.1.0"), ("index:echo", "0.2.0", "current")]
    {
        lock.plugins.insert("echo".into(), current(version, source));

        let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();

        stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &load).unwrap();

        let shown = preview("0.2.0", &lock, &staged, &schema, &root.join("none.service"), false);
        let row = format!("echo: stays at {version} ({reason})");

        assert!(stage::plan(&staged.dir).unwrap().plugins.is_empty());
        assert!(shown.text.contains(&row), "{row}\n{}", shown.text);
    }

    lock.plugins.insert("echo".into(), current("0.1.0", "index:echo@0.3.0-rc.1"));

    lock.plugins.insert("codex".into(), current("0.1.0", "index:codex"));

    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();

    stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &load).unwrap();

    let shown = preview("0.2.0", &lock, &staged, &schema, &root.join("none.service"), false);

    assert!(stage::plan(&staged.dir).unwrap().plugins.is_empty());
    assert!(shown.text.contains("echo: stays at 0.1.0 (pinned to 0.3.0-rc.1)"), "{}", shown.text);
    assert!(shown.text.contains("codex: stays at 0.1.0 (no longer in the plugin index)"));

    drop(staged);

    let loads = std::cell::Cell::new(0);

    let offline = || -> Result<std::sync::Arc<index::Index>, PluginError> {
        loads.set(loads.get() + 1);
        Err("The plugin index is unreachable.".into())
    };

    lock.plugins.insert("echo".into(), current("0.1.0", "index:echo"));

    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();

    stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &offline).unwrap();

    let shown = preview("0.2.0", &lock, &staged, &schema, &root.join("none.service"), false);

    assert!(
        shown.text.contains("echo: stays at 0.1.0 (plugin index unavailable)"),
        "{}",
        shown.text
    );

    assert!(shown.text.contains("codex: stays at 0.1.0 (plugin index unavailable)"));
    assert_eq!(loads.get(), 1);

    drop(staged);

    lock.plugins.get_mut("codex").unwrap().protocol = Protocol { major: 0, minor: 1 };

    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();

    assert!(stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &offline).is_err());

    drop(staged);
    lock.plugins.remove("codex");

    let mut revision = current("0.1.0", "index:echo");

    revision.pinned = true;
    lock.plugins.insert("echo".into(), revision);

    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();

    stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &load).unwrap();

    let shown = preview("0.2.0", &lock, &staged, &schema, &root.join("none.service"), false);

    assert!(stage::plan(&staged.dir).unwrap().plugins.is_empty());
    assert!(shown.text.contains("echo: stays at 0.1.0 (pinned to a revision)"));

    drop(staged);

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn refresh_lets_the_new_host_review_plugin_updates_and_reports_failures() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_root("refresh");
    let script = root.join("krabb");
    let calls = root.join("calls");

    let write = |body: &str| {
        fs::write(
            &script,
            format!("#!/bin/sh\necho \"$KRABB_HOME $*\" >> '{}'\n{body}\n", calls.display()),
        )
        .unwrap();

        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    };

    write("exit 0");

    assert!(matches!(refresh(&root, &script).await, Refresh::Done));

    assert_eq!(
        fs::read_to_string(&calls).unwrap().trim(),
        format!("{} plugin update", root.display())
    );

    write("exit 3");

    let Refresh::Failed(error) = refresh(&root, &script).await else {
        panic!("a failing plugin update must be reported");
    };

    assert!(error.contains("`krabb plugin update` exited"), "{error}");

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn recovery_points_restore_binaries_and_state_but_not_credentials() {
    let root = temp_root("snapshot");
    let home = root.join("home");
    let bin = root.join("bin");
    let service = root.join("krabb.service");
    let binaries = [bin.join("krabb"), bin.join("krabb-daemon")];

    for (path, content) in [
        (home.join("config.toml"), "model = 'old'"),
        (home.join("plugins.lock"), "{}"),
        (home.join("plugins/echo/krabb-plugin.toml"), "old"),
        (home.join("data/sessions/index.json"), "old"),
        (home.join("data/media/photo.jpg"), "photo"),
        (home.join("credentials.json"), "old secret"),
        (binaries[0].clone(), "old krabb"),
        (binaries[1].clone(), "old daemon"),
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    let mut point = snapshot::create(&home, "0.1.0", "0.2.0", &binaries, false).unwrap();

    assert!(!snapshot::load(&home).unwrap().unwrap().accepted);
    assert!(point.home.iter().all(|entry| !entry.path.starts_with("data/media")));
    assert!(point.home.iter().all(|entry| entry.path != "credentials.json"));

    snapshot::accept(&home, &mut point).unwrap();

    assert_eq!(recovery(&home).unwrap()["accepted"], true);

    for (path, content) in [
        (home.join("config.toml"), "model = 'new'"),
        (home.join("data/sessions/index.json"), "new"),
        (home.join("data/sessions/added.json"), "new"),
        (home.join(krabb_migrations::MARKER), "{\"schema\":2}"),
        (home.join("credentials.json"), "rotated secret"),
        (binaries[0].clone(), "new krabb"),
        (binaries[1].clone(), "new daemon"),
    ] {
        fs::write(path, content).unwrap();
    }

    fs::remove_dir_all(home.join("plugins/echo")).unwrap();

    snapshot::rollback(&home, &service, true, true).await.unwrap();

    assert_eq!(fs::read_to_string(home.join("config.toml")).unwrap(), "model = 'old'");
    assert_eq!(fs::read_to_string(home.join("data/sessions/index.json")).unwrap(), "old");
    assert_eq!(fs::read_to_string(home.join("plugins/echo/krabb-plugin.toml")).unwrap(), "old");
    assert_eq!(fs::read_to_string(home.join("credentials.json")).unwrap(), "rotated secret");
    assert_eq!(fs::read_to_string(home.join("data/media/photo.jpg")).unwrap(), "photo");
    assert_eq!(fs::read_to_string(&binaries[0]).unwrap(), "old krabb");
    assert_eq!(fs::read_to_string(&binaries[1]).unwrap(), "old daemon");
    assert!(!home.join("data/sessions/added.json").exists());
    assert!(!home.join(krabb_migrations::MARKER).exists());
    assert!(snapshot::load(&home).unwrap().is_none());
    assert!(recovery(&home).is_none());

    let error = snapshot::rollback(&home, &service, true, true).await.unwrap_err();

    assert!(error.to_string().contains("nothing to roll back"));

    let missing = snapshot::create(&home, "0.1.0", "0.2.0", &[bin.join("gone/krabb")], false);

    assert!(missing.is_ok());

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn rollback_keeps_grants_of_plugins_uninstalled_after_the_update() {
    let root = temp_root("revoked");
    let home = root.join("home");
    let service = root.join("krabb.service");

    let config = r#"
[[visibility]]
data = ["context"]
resources = ["global", "channels.echo"]

[channels.telegram]
worktree = "older schema"
visibility = [{ data = ["files"], resources = ["channels.echo"] }]
"#;

    fs::create_dir_all(home.join("plugins/echo")).unwrap();
    fs::write(home.join("config.toml"), config).unwrap();
    fs::write(home.join("plugins.lock"), "{}").unwrap();

    snapshot::uninstalled(&home, "echo").unwrap();

    assert!(!snapshot::store(&home).join("revoked.json").exists());

    let mut point = snapshot::create(&home, "0.1.0", "0.2.0", &[], false).unwrap();

    snapshot::accept(&home, &mut point).unwrap();
    snapshot::uninstalled(&home, "echo").unwrap();
    snapshot::uninstalled(&home, "echo").unwrap();

    assert_eq!(
        fs::read_to_string(snapshot::store(&home).join("revoked.json")).unwrap(),
        "[\n  \"echo\"\n]"
    );

    crate::revoke::restored(&home, "echo").unwrap();
    fs::remove_dir_all(home.join("plugins/echo")).unwrap();

    snapshot::rollback(&home, &service, true, true).await.unwrap();

    let restored = fs::read_to_string(home.join("config.toml")).unwrap();

    assert!(home.join("plugins/echo").is_dir());
    assert!(!restored.contains("channels.echo"), "{restored}");
    assert!(restored.contains("global"), "{restored}");
    assert!(restored.contains("older schema"), "{restored}");
    assert!(!snapshot::store(&home).exists());

    snapshot::create(&home, "0.1.0", "0.2.0", &[], false).unwrap();
    snapshot::uninstalled(&home, "echo").unwrap();
    snapshot::create(&home, "0.2.0", "0.3.0", &[], false).unwrap();

    assert!(!snapshot::store(&home).join("revoked.json").exists());

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn unreadable_uninstall_records_block_rollback_but_not_uninstall() {
    let root = temp_root("revoked-invalid");
    let home = root.join("home");
    let service = root.join("krabb.service");
    let record = snapshot::store(&home).join("revoked.json");

    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("config.toml"), "model = 'old'").unwrap();
    snapshot::create(&home, "0.1.0", "0.2.0", &[], false).unwrap();
    fs::write(home.join("config.toml"), "model = 'new'").unwrap();
    krabb_file::save(&record, b"[\"../escape\"]").unwrap();

    snapshot::uninstalled(&home, "echo").unwrap();

    let error = snapshot::rollback(&home, &service, true, true).await.unwrap_err();

    assert!(error.to_string().contains("uninstalled since the update is invalid"), "{error}");
    assert_eq!(fs::read(&record).unwrap(), b"[\"../escape\"]");

    let full = (0..256).map(|index| format!("plugin{index}")).collect::<Vec<_>>();

    krabb_file::save(&record, serde_json::to_vec(&full).unwrap()).unwrap();
    snapshot::uninstalled(&home, "echo").unwrap();
    snapshot::uninstalled(&home, "other").unwrap();

    let error = snapshot::rollback(&home, &service, true, true).await.unwrap_err();
    let recorded: Vec<String> = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();

    assert!(error.to_string().contains("Too many plugins"), "{error}");
    assert_eq!(recorded.len(), 257);
    assert_eq!(fs::read_to_string(home.join("config.toml")).unwrap(), "model = 'new'");

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn recovery_points_wait_for_plugin_changes_and_release_them() {
    let root = temp_root("revoked-lock");
    let home = root.join("home");

    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("config.toml"), "").unwrap();

    let held = crate::lock_at(&home, ".plugins.lock").unwrap();

    let busy = std::thread::spawn({
        let home = home.clone();
        move || snapshot::create(&home, "0.1.0", "0.2.0", &[], false).map(drop)
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    assert!(snapshot::load(&home).unwrap().is_none());

    drop(held);

    busy.join().unwrap().unwrap();
    snapshot::rollback(&home, &root.join("krabb.service"), true, true).await.unwrap();

    assert!(crate::lock_at(&home, ".plugins.lock").is_ok());

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn detached_plugins_carry_across_the_restart_and_missing_ones_are_reported() {
    let root = temp_root("carry");

    fs::create_dir_all(root.join("updates")).unwrap();

    finish::carry(&root, &[]).unwrap();

    assert!(root.join(DETACHED).exists());
    assert!(carried(&root).is_empty());
    assert!(!root.join(DETACHED).exists());

    finish::carry(&root, &["telegram".into(), "../escape".into()]).unwrap();

    assert_eq!(carried(&root), ["telegram".to_owned()].into_iter().collect());
    assert!(!root.join(DETACHED).exists());

    finish::carry(&root, &["telegram".into()]).unwrap();
    finish::carry(&root, &[]).unwrap();

    assert!(carried(&root).is_empty());

    let attached = ["echo".to_owned(), "tui".to_owned()];

    assert_eq!(finish::missing(&attached, &["tui".into()]), ["echo"]);
    assert!(finish::missing(&attached, &attached).is_empty());

    finish::carry(&root, &["telegram".into()]).unwrap();
    fs::write(root.join("config.toml"), "").unwrap();

    let point = snapshot::create(&root, "0.1.0", "0.2.0", &[], false).unwrap();

    snapshot::restore(&root, &point, &root.join("none.service"), true).await.unwrap();

    assert!(!root.join(DETACHED).exists());

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn finishing_requires_the_staged_version() {
    let root = temp_root("finish");
    let dir = root.join("stage");

    fs::create_dir_all(&dir).unwrap();

    krabb_file::save(
        dir.join(stage::PLAN),
        serde_json::to_vec(&serde_json::json!({"version": "9.9.9", "plugins": []})).unwrap(),
    )
    .unwrap();

    let error = finish::run(&root, &dir, false, true).await.unwrap_err();

    assert!(error.to_string().contains("staged update is for 9.9.9"));
    assert!(finish::run(&root, &root.join("missing"), false, true).await.is_err());

    krabb_file::save(
        dir.join(stage::PLAN),
        serde_json::to_vec(&serde_json::json!({"version": VERSION, "plugins": []})).unwrap(),
    )
    .unwrap();

    finish::run(&root, &dir, false, true).await.unwrap();

    assert_eq!(krabb_migrations::version(&root).unwrap(), schema::plan(&root).unwrap().to);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn update_options_parse_versions_and_rollback() {
    let parse = |args: &[&str]| crate::Cli::try_parse_from(args).map(|cli| cli.command);

    assert!(matches!(
        parse(&["krabb", "update", "0.2.0", "-y"]).unwrap(),
        crate::Command::Update(Options { version: Some(version), yes: true, .. }) if version == "0.2.0"
    ));

    assert!(matches!(
        parse(&["krabb", "update", "--rollback"]).unwrap(),
        crate::Command::Update(Options { rollback: true, version: None, .. })
    ));

    assert!(parse(&["krabb", "update", "0.2.0", "--rollback"]).is_err());
    assert!(parse(&["krabb", "update", "--start"]).is_err());
}

#[test]
fn interrupted_downloads_and_incompatible_plugins_never_reach_the_installation() {
    let root = temp_root("reject");
    let home = root.join("home");
    let release = release(&root, "0.2.0");
    let base = format!("file://{}", release.display());

    fs::create_dir_all(&home).unwrap();

    let mut lock = Lock::default();

    lock.plugins.insert("echo".into(), entry("0.1.0", "index:echo", false));

    let location = publish(&root, ECHO, "0.2.0");
    let load = || index::load_from(&home, &location);
    let blobs = root.join("registry/blobs/sha256");
    let blob = fs::read_dir(&blobs).unwrap().next().unwrap().unwrap().path();
    let complete = fs::read(&blob).unwrap();

    fs::write(&blob, &complete[..complete.len() / 2]).unwrap();

    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();
    let error = stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &load).unwrap_err();

    assert!(error.to_string().contains("failed verification"), "{error}");

    drop(staged);

    assert!(!home.join("updates/v0.2.0").exists());

    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();

    let error = stage::plugins(
        &mut staged,
        &lock,
        &inspection(Some(Protocol { major: 2, minor: 0 })),
        &load,
    )
    .unwrap_err();

    assert!(error.to_string().contains("cannot load the installed echo plugin"), "{error}");
    assert!(error.to_string().ends_with("The installation was not changed."));

    let mut linked = Lock::default();

    linked.plugins.insert("tui".into(), entry("0.1.0", "/src/tui", true));

    let error = stage::plugins(&mut staged, &linked, &inspection(Some(NEXT)), &load).unwrap_err();

    assert!(error.to_string().contains("Plugin tui uses protocol 0.1, which Krabb 0.2.0"));

    drop(staged);
    fs::remove_dir_all(root.join("registry")).unwrap();

    let location = publish(&root, &ECHO.replace("major = 1", "major = 9"), "0.2.0");
    let load = || index::load_from(&home, &location);
    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();

    stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &load).unwrap();

    let error = stage::compatible(&staged, &inspection(Some(NEXT))).unwrap_err();

    assert!(
        error.to_string().contains("Plugin echo 0.2.0 uses protocol 9.0, which Krabb 0.2.0"),
        "{error}"
    );

    assert!(stage::compatible(&staged, &inspection(None)).is_ok());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn staging_keeps_pre_release_pins_on_pre_releases() {
    let root = temp_root("stage-unstable");
    let home = root.join("home");
    let base = format!("file://{}", release(&root, "0.2.0").display());
    let location = publish(&root, ECHO, "0.2.0-rc.1");
    let load = || index::load_from(&home, &location);
    let mut lock = Lock::default();

    fs::create_dir_all(&home).unwrap();
    lock.plugins.insert("echo".into(), entry("0.1.0", "index:echo", false));

    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();
    let error = stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &load).unwrap_err();

    assert!(error.to_string().contains("No stable version of echo"), "{error}");

    drop(staged);
    lock.plugins.insert("echo".into(), entry("0.1.0-rc.1", "index:echo@0.1.0-rc.1", false));

    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();

    stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &load).unwrap();

    let plan = stage::plan(&staged.dir).unwrap();

    assert!(plan.plugins[0].file.contains("-v0.2.0-rc.1-"));
    assert_eq!(plan.plugins[0].pin.as_deref(), Some("0.2.0-rc.1"));

    let schema = stage::Schema { from: 1, to: 1, steps: Vec::new() };
    let service = root.join("none.service");
    let text = preview("0.2.0", &lock, &staged, &schema, &service, false).text;

    assert!(text.contains("echo: 0.1.0-rc.1 → 0.2.0 (pinned to pre-release 0.2.0-rc.1)"), "{text}");

    drop(staged);
    fs::remove_dir_all(root.join("registry")).unwrap();

    let location = publish(&root, ECHO, "0.2.0");
    let load = || index::load_from(&home, &location);
    let mut staged = stage::prepare(&home, "0.2.0", Some(&base)).unwrap();

    stage::plugins(&mut staged, &lock, &inspection(Some(NEXT)), &load).unwrap();

    assert_eq!(stage::plan(&staged.dir).unwrap().plugins[0].pin, None);

    let text = preview("0.2.0", &lock, &staged, &schema, &service, false).text;

    assert!(
        text.contains("(pre-release pin to 0.1.0-rc.1 replaced by the stable release)"),
        "{text}"
    );

    drop(staged);

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn failed_activation_restores_the_previous_installation() {
    let root = temp_root("activation");
    let home = root.join("home");
    let bin = root.join("bin");
    let binaries = [bin.join("krabb"), bin.join("krabb-daemon")];

    for (path, content) in [
        (home.join("config.toml"), "model = 'old'"),
        (home.join("plugins.lock"), "{}"),
        (binaries[0].clone(), "old krabb"),
        (binaries[1].clone(), "old daemon"),
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    let point = snapshot::create(&home, "0.1.0", "0.2.0", &binaries, false).unwrap();

    for (path, content) in [
        (home.join("config.toml"), "model = 'half migrated'"),
        (home.join(krabb_migrations::JOURNAL), "{}"),
        (binaries[0].clone(), "new krabb"),
        (binaries[1].clone(), "new daemon"),
    ] {
        fs::write(path, content).unwrap();
    }

    snapshot::restore(&home, &point, &root.join("none.service"), true).await.unwrap();

    assert_eq!(fs::read_to_string(home.join("config.toml")).unwrap(), "model = 'old'");
    assert_eq!(fs::read_to_string(&binaries[0]).unwrap(), "old krabb");
    assert_eq!(fs::read_to_string(&binaries[1]).unwrap(), "old daemon");
    assert!(!home.join(krabb_migrations::JOURNAL).exists());
    assert!(snapshot::load(&home).unwrap().is_none());

    let _ = fs::remove_dir_all(root);
}
