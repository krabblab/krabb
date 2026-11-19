use std::fs;

use super::*;

fn temp_root(name: &str) -> PathBuf {
    let root = crate::tests::test_root(&format!("upgrade-{name}"));

    fs::create_dir_all(&root).unwrap();

    root
}

fn sha(path: &Path) -> String {
    crate::digest::sha256(fs::read(path).unwrap())
}

fn tarball(release: &Path, package: &str, files: &[(&str, &str)]) -> String {
    let work = release.join(".work").join(package);

    for (path, content) in files {
        let file = work.join(path);

        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, content).unwrap();
    }

    let archive = release.join(format!("{package}.tar.gz"));

    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(release.join(".work"))
        .arg(package)
        .status()
        .unwrap();

    assert!(status.success());

    format!("{}  {}\n", sha(&archive), archive.file_name().unwrap().to_string_lossy())
}

fn release(root: &Path, version: &str) -> PathBuf {
    let release = root.join("release");
    let exe = std::env::consts::EXE_SUFFIX;
    let core = releases::archive("core", version).unwrap();
    let echo = releases::archive("echo", version).unwrap();
    let core = core.trim_end_matches(".tar.gz").trim_end_matches(".zip");
    let echo = echo.trim_end_matches(".tar.gz").trim_end_matches(".zip");

    fs::create_dir_all(&release).unwrap();

    let mut sums = tarball(
        &release,
        core,
        &[
            (&format!("bin/krabb{exe}"), "new krabb"),
            (&format!("bin/krabb-daemon{exe}"), "new daemon"),
        ],
    );

    sums.push_str(&tarball(
        &release,
        echo,
        &[
            (
                "krabb-plugin.toml",
                "id = 'echo'\nversion = '0.2.0'\ncapabilities = ['tool']\n\
             permissions = ['network']\nsecrets = ['KRABB_ECHO_KEY']\n\n\
             [protocol]\nmajor = 0\nminor = 1\n",
            ),
            (&format!("bin/krabb-plugin-echo{exe}"), "echo"),
        ],
    ));

    fs::write(release.join("SHA256SUMS"), sums).unwrap();

    release
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
        Some("permissions: adds memory; removes filesystem")
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
fn staging_verifies_and_reads_the_release_set() {
    let root = temp_root("stage");
    let home = root.join("home");
    let release = release(&root, "0.2.0");
    let base = format!("file://{}", release.display());

    fs::create_dir_all(&home).unwrap();

    let staged = stage::prepare(&home, "0.2.0", &["echo".into()], Some(&base)).unwrap();

    let dir = staged.dir.clone();

    assert_eq!(fs::read_to_string(&staged.krabb).unwrap(), "new krabb");
    assert_eq!(fs::read_to_string(&staged.daemon).unwrap(), "new daemon");
    assert_eq!(staged.manifests["echo"].permissions, ["network"]);

    let plan = stage::plan(&dir).unwrap();

    assert_eq!(plan.version, "0.2.0");
    assert_eq!(plan.plugins[0].id, "echo");
    assert!(releases::official_at("echo", &plan.plugins[0].source, Some(&base)));

    let mut lock = Lock::default();

    lock.plugins.insert("echo".into(), entry("0.1.0", &plan.plugins[0].source, false));
    lock.plugins.insert("tui".into(), entry("0.1.0", "/src/tui", true));

    let kept = lock.plugins.iter().filter(|(id, _)| *id == "tui").collect::<Vec<_>>();
    let schema = stage::Schema { from: 1, to: 2, steps: vec!["move".into()] };

    let preview =
        preview("0.2.0", &lock, &kept, &staged, &schema, &root.join("none.service"), false);

    for row in [
        "Update Krabb 0.1.0 to 0.2.0",
        "echo: 0.1.0 → 0.2.0",
        "echo permissions: adds network; removes filesystem",
        "echo secrets: adds KRABB_ECHO_KEY",
        "tui: stays at 0.1.0 (linked development build)",
        "home schema: 1 → 2 (1 migration)",
        "service: not installed",
        "`krabb update --rollback` restores 0.1.0",
    ] {
        assert!(preview.text.contains(row), "{row}\n{}", preview.text);
    }

    assert_eq!(preview.value["kept"], serde_json::json!(["tui"]));

    drop(staged);

    assert!(!dir.exists());

    fs::write(
        release.join("SHA256SUMS"),
        fs::read_to_string(release.join("SHA256SUMS")).unwrap().replacen(
            &sha(&release.join(releases::archive("core", "0.2.0").unwrap())),
            &"0".repeat(64),
            1,
        ),
    )
    .unwrap();

    let error = stage::prepare(&home, "0.2.0", &[], Some(&base)).err().unwrap();

    assert!(error.to_string().contains("failed verification"), "{error}");
    assert!(stage::prepare(&home, "0.3.0", &[], Some(&base)).is_err());

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
async fn finishing_requires_the_staged_version() {
    let root = temp_root("finish");
    let dir = root.join("stage");

    fs::create_dir_all(&dir).unwrap();

    krabb_file::save(
        dir.join(stage::PLAN),
        serde_json::to_vec(&stage::Plan { version: "9.9.9".into(), plugins: Vec::new() }).unwrap(),
    )
    .unwrap();

    let error = finish::run(&root, &dir, false, true).await.unwrap_err();

    assert!(error.to_string().contains("staged update is for 9.9.9"));
    assert!(finish::run(&root, &root.join("missing"), false, true).await.is_err());

    krabb_file::save(
        dir.join(stage::PLAN),
        serde_json::to_vec(&stage::Plan { version: VERSION.into(), plugins: Vec::new() }).unwrap(),
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
    let echo = release.join(releases::archive("echo", "0.2.0").unwrap());
    let complete = fs::read(&echo).unwrap();

    fs::create_dir_all(&home).unwrap();
    fs::write(&echo, &complete[..complete.len() / 2]).unwrap();

    let error = stage::prepare(&home, "0.2.0", &["echo".into()], Some(&base)).err().unwrap();

    assert!(error.to_string().contains("failed verification"), "{error}");
    assert!(!home.join("updates/v0.2.0").exists());

    let archive = releases::archive("echo", "0.2.0").unwrap();
    let package = archive.trim_end_matches(".tar.gz").trim_end_matches(".zip");

    let manifest = "id = 'echo'\nversion = '0.2.0'\ncapabilities = ['tool']\n\n\
                    [protocol]\nmajor = 9\nminor = 0\n";

    let exe = std::env::consts::EXE_SUFFIX;

    let line = tarball(
        &release,
        package,
        &[("krabb-plugin.toml", manifest), (&format!("bin/krabb-plugin-echo{exe}"), "echo")],
    );

    let name = echo.file_name().unwrap().to_string_lossy().into_owned();

    let mut sums = fs::read_to_string(release.join("SHA256SUMS"))
        .unwrap()
        .lines()
        .filter(|row| !row.ends_with(&name))
        .map(|row| format!("{row}\n"))
        .collect::<String>();

    sums.push_str(&line);
    fs::write(release.join("SHA256SUMS"), sums).unwrap();

    let staged = stage::prepare(&home, "0.2.0", &["echo".into()], Some(&base)).unwrap();

    let inspection = |protocol| stage::Inspection {
        version: "0.2.0".into(),
        schema: stage::Schema { from: 1, to: 1, steps: Vec::new() },
        protocol,
    };

    let error = stage::compatible(&staged, &inspection(Some(Protocol::CURRENT))).unwrap_err();

    assert!(error.to_string().contains("echo plugin in Krabb 0.2.0 uses protocol 9.0"), "{error}");
    assert!(error.to_string().ends_with("The installation was not changed."));
    assert!(stage::compatible(&staged, &inspection(None)).is_ok());

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
