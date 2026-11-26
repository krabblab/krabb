use std::fs;

use super::*;

const DIGEST: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";
const LINUX: &str = "x86_64-unknown-linux-gnu";
const HOST: Host<'static> = Host { protocol: Protocol::CURRENT, version: "0.2.0" };

fn release(version: &str, protocol: (u16, u16), yanked: Option<&str>) -> Value {
    serde_json::json!({
        "version": version,
        "protocol": {"major": protocol.0, "minor": protocol.1},
        "released": "2026-11-20",
        "permissions": ["network"],
        "secrets": ["KRABB_ECHO_KEY"],
        "yanked": yanked,
        "artifacts": {LINUX: {"sha256": DIGEST, "size": 3}},
    })
}

fn index(releases: Vec<Value>) -> Index {
    let value = serde_json::json!({
        "schema": 1,
        "registry": "ghcr.io/krabblab/krabb/plugins",
        "plugins": {"echo": releases},
    });

    let index = serde_json::from_value::<Index>(value).unwrap();

    validate(&index, false).unwrap();
    index
}

fn sample() -> Index {
    index(vec![
        release("0.1.0", (0, 1), None),
        release("0.3.0", (0, 1), Some("crashes on start")),
        release("0.2.0", (0, 1), None),
        release("0.4.0-rc.1", (0, 1), None),
        release("1.0.0", (1, 0), None),
    ])
}

fn pick(index: &Index, version: Option<&str>) -> Result<String, String> {
    choose(index, version, false)
}

fn choose(index: &Index, version: Option<&str>, unstable: bool) -> Result<String, String> {
    select_for(index, "echo", version, unstable, HOST, "linux", "x86_64")
        .map(|choice| choice.release.version.clone())
        .map_err(|error| error.to_string())
}

#[test]
fn sources_name_a_plugin_and_an_optional_pin() {
    assert_eq!(source("tui", None), "index:tui");

    assert_eq!(source("tui", Some("0.1.0")), "index:tui@0.1.0");
    assert_eq!(parse("index:tui"), Some(("tui", None)));
    assert_eq!(parse("index:tui@0.1.0"), Some(("tui", Some("0.1.0"))));
    assert_eq!(parse("/src/tui"), None);
    assert_eq!(parse("https://example.com/tui.tar.gz#sha256=00"), None);
}

#[test]
fn selection_prefers_the_newest_supported_stable_version() {
    let index = sample();

    assert_eq!(pick(&index, None).unwrap(), "0.2.0");
    assert_eq!(pick(&index, Some("0.1.0")).unwrap(), "0.1.0");
    assert_eq!(pick(&index, Some("0.3.0")).unwrap(), "0.3.0");
    assert_eq!(pick(&index, Some("0.4.0-rc.1")).unwrap(), "0.4.0-rc.1");
    assert!(pick(&index, Some("1.0.0")).unwrap_err().contains("needs plugin protocol 1.0"));
    assert!(pick(&index, Some("9.9.9")).unwrap_err().contains("has no version 9.9.9"));

    let error = select_for(&index, "slack", None, false, HOST, "linux", "x86_64");

    assert!(error.err().unwrap().to_string().contains("not in the official plugin index"));

    let error = select_for(&index, "echo", None, false, HOST, "macos", "aarch64");

    assert!(error.err().unwrap().to_string().contains("No release of plugin echo has a build"));

    let error = select_for(&index, "echo", Some("0.1.0"), false, HOST, "macos", "aarch64");

    assert!(error.err().unwrap().to_string().contains("has no build for aarch64-apple-darwin"));

    let error = select_for(&index, "echo", None, false, HOST, "plan9", "x86_64");

    assert!(error.err().unwrap().to_string().contains("--source"));

    let next = Host { protocol: Protocol { major: 1, minor: 2 }, ..HOST };
    let next = select_for(&index, "echo", None, false, next, "linux", "x86_64");

    assert_eq!(next.unwrap().release.version, "1.0.0");
}

#[test]
fn versions_that_need_a_newer_krabb_are_skipped_and_explained() {
    let mut newer = release("0.3.0", (0, 1), None);

    newer["krabb"] = "0.3.0-rc.1".into();

    let index = index(vec![release("0.1.0", (0, 1), None), newer.clone()]);

    assert_eq!(pick(&index, None).unwrap(), "0.1.0");

    assert_eq!(
        pick(&index, Some("0.3.0")).unwrap_err(),
        "Plugin echo 0.3.0 needs Krabb 0.3.0-rc.1 or newer.\nUpdate Krabb with `krabb update`."
    );

    let updated = Host { version: "0.3.0", ..HOST };
    let choice = select_for(&index, "echo", None, false, updated, "linux", "x86_64").unwrap();

    assert_eq!(choice.release.version, "0.3.0");

    let only = self::index(vec![newer]);
    let error = select_for(&only, "echo", None, false, HOST, "linux", "x86_64").err().unwrap();
    let unavailable = error.downcast_ref::<Unavailable>().unwrap();

    assert_eq!(unavailable.to_string(), "Plugin echo needs a newer version of Krabb.");
    assert_eq!(unavailable.next(), "Update Krabb with `krabb update`.");

    let mut future = release("0.4.0", (0, 1), None);

    future["krabb"] = "99.0.0".into();

    let mixed = self::index(vec![future, release("0.5.0-rc.1", (0, 1), None)]);
    let error = select_for(&mixed, "echo", None, false, HOST, "linux", "x86_64").err().unwrap();

    assert!(error.downcast_ref::<Unavailable>().unwrap().next().contains("--unstable"));

    let rows = history(&mixed, "echo", None, false).unwrap();
    let notes = notes(rows.iter().find(|row| row.version == "0.4.0").unwrap());

    assert!(notes.contains(&"needs Krabb 99.0.0 or newer".to_owned()), "{notes:?}");
    assert!(rows.iter().all(|row| row.compatible));
}

#[test]
fn an_index_naming_an_invalid_krabb_version_is_refused() {
    let mut broken = release("0.1.0", (0, 1), None);

    broken["krabb"] = "soon".into();

    let value = serde_json::json!({
        "schema": 1,
        "registry": "ghcr.io/krabblab/krabb/plugins",
        "plugins": {"echo": [broken]},
    });

    let index = serde_json::from_value::<Index>(value).unwrap();

    assert_eq!(validate(&index, false).unwrap_err(), "echo 0.1.0 names an invalid Krabb version");
}

#[test]
fn unstable_selection_includes_pre_releases_but_not_withdrawn_versions() {
    let unstable = |index: &Index| choose(index, None, true);

    assert_eq!(unstable(&sample()).unwrap(), "0.4.0-rc.1");

    let candidates = index(vec![
        release("0.1.0-rc.1", (0, 1), None),
        release("0.1.0-rc.2", (0, 1), Some("broken")),
    ]);

    assert_eq!(unstable(&candidates).unwrap(), "0.1.0-rc.1");

    assert_eq!(
        pick(&candidates, None).unwrap_err(),
        "No stable release of plugin echo works with this version of Krabb."
    );

    let error =
        select_for(&candidates, "echo", None, false, HOST, "linux", "x86_64").err().unwrap();

    assert_eq!(
        error.downcast_ref::<Unavailable>().unwrap().next(),
        "Install the newest pre-release with `krabb plugin install echo --unstable`."
    );

    let withdrawn = index(vec![release("0.1.0-rc.1", (0, 1), Some("broken"))]);

    assert_eq!(
        unstable(&withdrawn).unwrap_err(),
        "No release of plugin echo works with this version of Krabb."
    );

    let error = select_for(&withdrawn, "echo", None, true, HOST, "linux", "x86_64").err().unwrap();

    assert_eq!(
        error.downcast_ref::<Unavailable>().unwrap().next(),
        "See every release with `krabb plugin versions echo`."
    );
}

#[test]
fn indexes_reject_malformed_or_unsafe_entries() {
    let base = serde_json::json!({
        "schema": 1,
        "registry": "ghcr.io/krabblab/krabb/plugins",
        "plugins": {"echo": [release("0.1.0", (0, 1), None)]},
    });

    let check = |change: &dyn Fn(&mut Value), local: bool| {
        let mut value = base.clone();

        change(&mut value);

        validate(&serde_json::from_value(value).unwrap(), local)
    };

    assert!(check(&|_| {}, false).is_ok());
    assert!(check(&|value| value["schema"] = 2.into(), false).unwrap_err().contains("schema 2"));

    for registry in ["ghcr.io", "ghcr.io/../x", "GHCR.io/x", "ghcr.io//x", "file:///tmp/r", ""] {
        let error = check(&|value| value["registry"] = registry.into(), false);

        assert!(error.unwrap_err().contains("not an allowed location"), "{registry}");
    }

    assert!(check(&|value| value["registry"] = "file:///tmp/r".into(), true).is_ok());
    assert!(check(&|value| value["registry"] = "localhost:5000/krabb".into(), false).is_ok());

    let duplicate = |value: &mut Value| {
        value["plugins"]["echo"] =
            serde_json::json!([release("0.1.0", (0, 1), None), release("0.1.0", (0, 1), None)]);
    };

    assert!(check(&duplicate, false).unwrap_err().contains("more than once"));

    let id = |value: &mut Value| {
        value["plugins"] = serde_json::json!({"Bad_ID": [release("0.1.0", (0, 1), None)]});
    };

    assert!(check(&id, false).unwrap_err().contains("plugin ID Bad_ID"));

    for (field, bad) in
        [("sha256", Value::from(DIGEST.to_ascii_uppercase())), ("sha256", Value::from("ab12"))]
    {
        let artifact = |value: &mut Value| {
            value["plugins"]["echo"][0]["artifacts"][LINUX][field] = bad.clone()
        };

        assert!(check(&artifact, false).unwrap_err().contains("invalid"), "{field}");
    }

    let mut later = base.clone();

    later["plugins"]["echo"][0]["artifacts"]["riscv64-linux"] =
        serde_json::json!({"sha256": DIGEST, "size": 1});

    later["plugins"]["echo"][0]["artifacts"]["x86_64-apple-darwin"] =
        serde_json::json!({"sha256": DIGEST, "size": ARCHIVE_LIMIT + 1});

    let parsed = parse_index(later.to_string().as_bytes(), false).unwrap();

    assert_eq!(parsed.plugins["echo"][0].artifacts.keys().collect::<Vec<_>>(), [LINUX]);

    assert!(
        parse_index(b"{", false)
            .unwrap_err()
            .to_string()
            .starts_with("The plugin index is invalid")
    );
}

#[test]
fn local_indexes_pull_verified_archives_from_local_registries() {
    let root = crate::tests::test_root("registry-local");
    let registry = root.join("registry");
    let content = b"archive";
    let digest = crate::digest::sha256(content);
    let blobs = registry.join("blobs/sha256");

    fs::create_dir_all(&blobs).unwrap();
    fs::write(blobs.join(&digest), content).unwrap();

    let mut value = serde_json::json!({
        "schema": 1,
        "registry": format!("file://{}", registry.display()),
        "plugins": {"echo": [release("0.1.0", (0, 1), None)]},
    });

    value["plugins"]["echo"][0]["artifacts"] = serde_json::json!({
        LINUX: {"sha256": digest, "size": content.len()},
        "aarch64-apple-darwin": {"sha256": digest, "size": content.len()},
        "x86_64-apple-darwin": {"sha256": digest, "size": content.len()},
        "aarch64-unknown-linux-gnu": {"sha256": digest, "size": content.len()},
        "x86_64-pc-windows-msvc": {"sha256": digest, "size": content.len()},
        "aarch64-pc-windows-msvc": {"sha256": digest, "size": content.len()},
    });

    let path = root.join("index.json");

    fs::write(&path, value.to_string()).unwrap();

    let index = load_from(&root, &format!("file://{}", path.display())).unwrap();
    let choice = select(&index, "echo", None, Host::CURRENT).unwrap();
    let archive = pull(&index, "echo", &choice, &root).unwrap();

    assert_eq!(fs::read(&archive).unwrap(), content);
    assert!(archive.file_name().unwrap().to_string_lossy().starts_with("krabb-echo-v0.1.0-"));

    fs::write(blobs.join(&digest), b"tampered").unwrap();

    let error = pull(&index, "echo", &choice, &root).unwrap_err().to_string();

    assert!(error.contains("Plugin echo 0.1.0 failed verification"), "{error}");

    let source = unpacked(&index, "echo", None).err().unwrap().to_string();

    assert!(source.contains("failed verification"), "{source}");

    let missing = load_from(&root, &format!("file://{}", root.join("none.json").display()));

    assert!(missing.unwrap_err().to_string().starts_with("The plugin index is unreadable"));
    assert!(load_from(&root, "http://example.com/index.json").is_err());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn unreachable_indexes_fall_back_to_the_cached_copy() {
    let root = crate::tests::test_root("registry-cache");
    let location = "https://127.0.0.1:9/index.json";

    fs::create_dir_all(&root).unwrap();

    let error = load_from(&root, location).unwrap_err().to_string();

    assert!(error.starts_with("The plugin index is unavailable"), "{error}");
    assert!(error.ends_with("install with --source."), "{error}");

    let cached = serde_json::json!({
        "schema": 1,
        "registry": "ghcr.io/krabblab/krabb/plugins",
        "plugins": {"echo": [release("0.1.0", (0, 1), None)]},
    });

    let offline = "https://127.0.0.1:9/offline.json";

    krabb_file::save(cache(&root, location), cached.to_string()).unwrap();
    krabb_file::save(cache(&root, offline), cached.to_string()).unwrap();

    let index = load_from(&root, offline).unwrap();

    assert_eq!(index.plugins["echo"][0].version, "0.1.0");
    assert!(load_from(&root, "https://127.0.0.1:9/other.json").is_err());
    assert_eq!(cache(&root, INDEX), root.join("cache/plugin-index.json"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn history_marks_latest_installed_and_unsupported_versions() {
    let index = sample();
    let rows = history(&index, "echo", Some("0.1.0"), false).unwrap();

    let summary = rows
        .iter()
        .map(|row| format!("{} {}", row.version, notes(row).join("; ")))
        .collect::<Vec<_>>();

    let target = releases::target(std::env::consts::OS, std::env::consts::ARCH).map(|(t, _)| t);
    let suffix = if target == Some(LINUX) { "" } else { "; no build for this system" };

    assert_eq!(summary[0], format!("1.0.0 needs protocol 1.0{suffix}"));
    assert_eq!(summary[1], format!("0.4.0-rc.1 pre-release{suffix}"));
    assert_eq!(summary[2], format!("0.3.0 withdrawn: crashes on start{suffix}"));
    assert_eq!(summary[4], format!("0.1.0 installed{suffix}"));

    if suffix.is_empty() {
        assert_eq!(summary[3], "0.2.0 latest");
        assert!(rows[3].latest);
    }

    assert!(rows[4].installed);
    assert!(!rows[0].compatible);
    assert!(history(&sample(), "slack", None, false).is_err());

    let rows = history(&index, "echo", Some("0.1.0"), true).unwrap();

    if suffix.is_empty() {
        assert!(rows[1].latest);
        assert!(!rows[3].latest);
    }
}

#[test]
fn pins_explain_their_risk_and_how_to_follow_updates() {
    let index = sample();
    let target = releases::target(std::env::consts::OS, std::env::consts::ARCH).map(|(t, _)| t);

    let old = pin(&index, "echo", "0.1.0", false).unwrap();

    if target == Some(LINUX) {
        assert!(old.contains("the latest supported version is 0.2.0"), "{old}");

        assert!(
            pin(&index, "echo", "0.2.0", false).unwrap().starts_with("echo is pinned to 0.2.0.")
        );
    }

    assert!(old.ends_with("Run `krabb plugin install echo@latest --yes` to follow updates again."));

    assert_eq!(
        pin(&index, "echo", "0.4.0-rc.1", false).unwrap(),
        "echo is pinned to pre-release 0.4.0-rc.1."
    );

    let released = self::index(vec![
        release("0.1.0-rc.1", (0, 1), None),
        release("0.1.0-rc.2", (0, 1), None),
        release("0.1.0", (0, 1), None),
    ]);

    let candidates =
        self::index(vec![release("0.1.0-rc.1", (0, 1), None), release("0.1.0-rc.2", (0, 1), None)]);

    if target == Some(LINUX) {
        let notice = pin(&released, "echo", "0.1.0-rc.1", false).unwrap();

        let newer = "echo is pinned to pre-release 0.1.0-rc.1, and 0.1.0 is newer.\n";

        assert!(notice.starts_with(newer), "{notice}");
        assert!(notice.ends_with("to follow stable releases."), "{notice}");

        let notice = pin(&candidates, "echo", "0.1.0-rc.1", false).unwrap();

        assert!(notice.ends_with("--unstable --yes` to move to a newer pre-release."), "{notice}");

        let notice = pin(&candidates, "echo", "0.1.0-rc.1", true).unwrap();
        let newer = "echo is pinned to pre-release 0.1.0-rc.1, and 0.1.0-rc.2 is newer.\n";

        assert!(notice.starts_with(newer), "{notice}");
        assert!(notice.ends_with("to follow updates again."), "{notice}");
    }

    let withdrawn = pin(&index, "echo", "0.3.0", false).unwrap();

    assert!(withdrawn.starts_with("echo 0.3.0 was withdrawn: Crashes on start."), "{withdrawn}");
    assert!(pin(&index, "echo", "9.9.9", false).is_none());
    assert!(pin(&index, "slack", "0.1.0", false).is_none());
}

#[test]
fn official_sources_unpack_the_selected_plugin() {
    let root = crate::tests::test_root("registry-unpack");
    let work = root.join("work");

    let (target, extension) =
        releases::target(std::env::consts::OS, std::env::consts::ARCH).unwrap();

    let package = work.join("krabb-echo-v0.1.0");
    let archive = root.join(format!("echo.{extension}"));

    fs::create_dir_all(package.join("bin")).unwrap();
    fs::write(package.join("krabb-plugin.toml"), "id = 'echo'\n").unwrap();

    let format: &[&str] = if extension == "zip" { &["--format=zip", "-cf"] } else { &["-czf"] };

    let status = std::process::Command::new("tar")
        .args(format)
        .arg(&archive)
        .arg("-C")
        .arg(&work)
        .arg("krabb-echo-v0.1.0")
        .status()
        .unwrap();

    assert!(status.success());

    let bytes = fs::read(&archive).unwrap();
    let digest = crate::digest::sha256(&bytes);
    let blobs = root.join("registry/blobs/sha256");

    fs::create_dir_all(&blobs).unwrap();
    fs::write(blobs.join(&digest), &bytes).unwrap();

    let mut value = serde_json::json!({
        "schema": 1,
        "registry": format!("file://{}", root.join("registry").display()),
        "plugins": {"echo": [release("0.1.0", (0, 1), None)]},
    });

    value["plugins"]["echo"][0]["artifacts"] =
        serde_json::json!({target: {"sha256": digest, "size": bytes.len()}});

    let index = serde_json::from_value::<Index>(value).unwrap();
    let source = unpacked(&index, "echo", Some("0.1.0")).unwrap();
    let temp = source.temp.clone().unwrap();

    assert!(source.path.join("krabb-plugin.toml").is_file());

    drop(source);

    assert!(!temp.exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn updates_never_select_an_older_version() {
    let entry = serde_json::from_value::<Entry>(serde_json::json!({
        "source": "index:echo",
        "version": "0.3.0",
        "protocol": {"major": 0, "minor": 1},
        "capabilities": [],
    }))
    .unwrap();

    assert!(newer("0.3.1", &entry));
    assert!(!newer("0.3.0", &entry));
    assert!(!newer("0.2.0", &entry));
    assert!(!newer("0.3.0-rc.1", &entry));
}

#[test]
fn registry_references_and_index_manifests_are_parsed_strictly() {
    assert_eq!(reference(INDEX), Some(("ghcr.io", "krabblab/krabb/plugins", "index")));
    assert_eq!(reference("localhost:5000/plugins:v1"), Some(("localhost:5000", "plugins", "v1")));

    for invalid in ["ghcr.io/plugins", "ghcr.io:index", "GHCR.io/x:index", "ghcr.io/x:-tag"] {
        assert_eq!(reference(invalid), None, "{invalid}");
    }

    let digest = "a".repeat(64);

    let manifest = |media: &str, digest: &str| {
        serde_json::json!({"layers": [{"mediaType": media, "digest": format!("sha256:{digest}")}]})
            .to_string()
    };

    assert_eq!(layer(manifest(INDEX_MEDIA, &digest).as_bytes()).unwrap(), digest);
    assert!(layer(manifest("application/json", &digest).as_bytes()).is_err());
    assert!(layer(manifest(INDEX_MEDIA, "short").as_bytes()).is_err());
    assert!(layer(b"[]").is_err());
    assert!(layer(b"not json").is_err());

    let root = crate::tests::test_root("registry-reference");
    let error = load_from(&root, "ghcr.io/plugins").unwrap_err().to_string();

    assert!(error.starts_with("KRABB_PLUGIN_INDEX must be an OCI reference"), "{error}");

    let _ = fs::remove_dir_all(root);
}
