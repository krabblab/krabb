use std::{fs, io, path::PathBuf};

use super::{
    BACKUP, Error, JOURNAL, MARKER, Plan, Registry, Step, migrate, plan, restore, version,
};

fn home(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("krabb-migrations-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);

    fs::create_dir_all(&root).unwrap();

    root
}

fn mark(root: &std::path::Path) -> io::Result<()> {
    fs::write(root.join("data/state.txt"), "migrated")
}

fn move_config(root: &std::path::Path) -> io::Result<()> {
    fs::rename(root.join("old.toml"), root.join("config.toml"))
}

fn fail(root: &std::path::Path) -> io::Result<()> {
    fs::write(root.join("data/state.txt"), "partial")?;
    fs::write(root.join("created.txt"), "partial")?;

    Err(io::Error::other("disk full"))
}

static STEPS: &[Step] = &[
    Step { name: "baseline", paths: &[], apply: |_| Ok(()) },
    Step { name: "mark state", paths: &["data"], apply: mark },
    Step { name: "rename config", paths: &["old.toml", "config.toml"], apply: move_config },
];

static FAILING: &[Step] = &[
    Step { name: "baseline", paths: &[], apply: |_| Ok(()) },
    Step { name: "break", paths: &["data", "created.txt"], apply: fail },
];

#[test]
fn unversioned_homes_migrate_through_every_skipped_step() {
    let root = home("skip");
    let registry = Registry::new(STEPS);

    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/state.txt"), "original").unwrap();
    fs::write(root.join("old.toml"), "model = 'x'").unwrap();

    assert_eq!(version(&root).unwrap(), 0);

    assert_eq!(
        plan(&root, &registry).unwrap(),
        Plan {
            from: 0,
            to: 3,
            steps: vec!["baseline", "mark state", "rename config"],
            interrupted: false
        }
    );

    let outcome = migrate(&root, &registry).unwrap();

    assert_eq!((outcome.from, outcome.to), (0, 3));
    assert_eq!(version(&root).unwrap(), 3);
    assert_eq!(fs::read_to_string(root.join("data/state.txt")).unwrap(), "migrated");
    assert_eq!(fs::read_to_string(root.join("config.toml")).unwrap(), "model = 'x'");
    assert!(!root.join("old.toml").exists());
    assert!(!root.join(JOURNAL).exists());
    assert!(!root.join(BACKUP).exists());

    let again = migrate(&root, &registry).unwrap();

    assert_eq!((again.from, again.to, again.steps.len()), (3, 3, 0));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn intermediate_versions_run_only_remaining_steps() {
    let root = home("partial");

    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join(MARKER), r#"{"schema":2}"#).unwrap();
    krabb_file::private(root.join(MARKER)).unwrap();
    fs::write(root.join("old.toml"), "a").unwrap();

    let outcome = migrate(&root, &Registry::new(STEPS)).unwrap();

    assert_eq!(outcome.steps, ["rename config"]);
    assert!(!root.join("data/state.txt").exists());
    assert!(root.join("config.toml").is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_steps_restore_the_recovery_point() {
    let root = home("failed");

    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/state.txt"), "original").unwrap();

    let error = migrate(&root, &Registry::new(FAILING)).unwrap_err();

    assert!(matches!(error, Error::Step { name: "break", .. }));
    assert!(error.to_string().contains("rolled back: disk full"));
    assert_eq!(fs::read_to_string(root.join("data/state.txt")).unwrap(), "original");
    assert!(!root.join("created.txt").exists());
    assert_eq!(version(&root).unwrap(), 0);
    assert!(!root.join(JOURNAL).exists());
    assert!(!root.join(BACKUP).exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn interrupted_migrations_are_restored_before_retrying() {
    let root = home("interrupted");
    let registry = Registry::new(STEPS);

    fs::create_dir_all(root.join(BACKUP).join("data")).unwrap();
    fs::write(root.join(BACKUP).join("data/state.txt"), "original").unwrap();
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/state.txt"), "half written").unwrap();
    fs::write(root.join("config.toml"), "half written").unwrap();
    fs::write(root.join(MARKER), r#"{"schema":3}"#).unwrap();
    krabb_file::private(root.join(MARKER)).unwrap();

    fs::write(
        root.join(JOURNAL),
        r#"{"from":0,"to":3,"paths":[
            {"path":"schema.json","present":false},
            {"path":"data","present":true},
            {"path":"config.toml","present":false}]}"#,
    )
    .unwrap();

    krabb_file::private(root.join(JOURNAL)).unwrap();

    assert!(plan(&root, &registry).unwrap().interrupted);
    assert_eq!(plan(&root, &registry).unwrap().from, 0);

    assert!(restore(&root).unwrap());
    assert_eq!(fs::read_to_string(root.join("data/state.txt")).unwrap(), "original");
    assert!(!root.join("config.toml").exists());
    assert_eq!(version(&root).unwrap(), 0);
    assert!(!restore(&root).unwrap());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn newer_and_invalid_homes_are_rejected_before_changes() {
    let root = home("newer");
    let registry = Registry::new(STEPS);

    fs::write(root.join(MARKER), r#"{"schema":9}"#).unwrap();
    krabb_file::private(root.join(MARKER)).unwrap();

    let error = migrate(&root, &registry).unwrap_err();

    assert!(matches!(error, Error::Newer { found: 9, supported: 3 }));
    assert!(error.to_string().contains("install a newer Krabb"));
    assert_eq!(io::Error::from(error).kind(), io::ErrorKind::InvalidData);

    fs::write(root.join(MARKER), r#"{"schema":"one"}"#).unwrap();

    assert!(matches!(version(&root), Err(Error::Invalid(_))));

    fs::remove_file(root.join(MARKER)).unwrap();
    fs::write(root.join(JOURNAL), r#"{"from":0,"to":1,"paths":[{"path":"../x","present":true}]}"#)
        .unwrap();

    krabb_file::private(root.join(JOURNAL)).unwrap();

    assert!(matches!(plan(&root, &registry), Err(Error::Invalid(_))));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn step_paths_must_stay_inside_the_home() {
    static ESCAPE: &[Step] = &[Step { name: "escape", paths: &["../outside"], apply: |_| Ok(()) }];
    static RESERVED: &[Step] = &[Step { name: "journal", paths: &[JOURNAL], apply: |_| Ok(()) }];

    let root = home("escape");

    assert!(matches!(migrate(&root, &Registry::new(ESCAPE)), Err(Error::Invalid(_))));
    assert!(matches!(migrate(&root, &Registry::new(RESERVED)), Err(Error::Invalid(_))));
    assert_eq!(version(&root).unwrap(), 0);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn concurrent_migrations_are_refused() {
    let root = home("busy");
    let _held = super::lock(&root).unwrap();

    assert!(matches!(migrate(&root, &Registry::new(STEPS)), Err(Error::Busy)));
    assert_eq!(io::Error::from(Error::Busy).kind(), io::ErrorKind::WouldBlock);

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_restored_as_links() {
    let root = home("link");
    let store = root.join("store");

    fs::create_dir_all(root.join("plugins/tui")).unwrap();
    std::os::unix::fs::symlink("/opt/build/tui", root.join("plugins/tui/bin")).unwrap();

    let entries = super::recovery::save(&root, &store, &["plugins".into(), "gone".into()]).unwrap();

    assert_eq!(entries[1], super::recovery::Entry { path: "gone".into(), present: false });

    fs::remove_dir_all(root.join("plugins")).unwrap();
    fs::write(root.join("gone"), "new").unwrap();
    super::recovery::restore(&root, &store, &entries).unwrap();

    assert_eq!(
        fs::read_link(root.join("plugins/tui/bin")).unwrap(),
        PathBuf::from("/opt/build/tui")
    );

    assert!(!root.join("gone").exists());

    fs::remove_dir_all(&store).unwrap();

    let missing = super::recovery::restore(&root, &store, &entries).unwrap_err();

    assert!(missing.to_string().contains("recovery point is missing plugins"));

    let _ = fs::remove_dir_all(root);
}

/// Prepares `name` at schema `origin` from an unversioned home with the original content.
fn origin_home(name: &str, origin: usize) -> PathBuf {
    let root = home(name);

    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/state.txt"), "original").unwrap();
    fs::write(root.join("old.toml"), "model = 'x'").unwrap();

    if origin > 0 {
        migrate(&root, &Registry::new(&STEPS[..origin])).unwrap();
    }

    assert_eq!(version(&root).unwrap(), origin as u32);

    root
}

/// Runs `migrate`'s phases in order and stops after `cut` of them, as a crash would.
fn interrupt(root: &std::path::Path, origin: usize, cut: usize) {
    let steps = &STEPS[origin..];
    let mut paths = vec![MARKER.to_owned()];

    for step in steps {
        paths.extend(step.paths.iter().map(|path| (*path).to_owned()));
    }

    paths.dedup();

    let mut phase = 0;

    let mut next = |action: &mut dyn FnMut()| {
        if phase < cut {
            action();
        }

        phase += 1;
    };

    let mut entries = Vec::new();

    next(&mut || {
        entries = super::recovery::save(root, &root.join(BACKUP), &paths).unwrap();
    });

    if cut == 0 {
        fs::create_dir_all(root.join(BACKUP).join("partial")).unwrap();
    }

    next(&mut || {
        let journal = super::journal::Journal {
            from: origin as u32,
            to: STEPS.len() as u32,
            paths: entries.clone(),
        };

        super::journal::write(root, &journal).unwrap();
    });

    for step in steps {
        next(&mut || (step.apply)(root).unwrap());
    }

    next(&mut || {
        let marker = format!(r#"{{"schema":{}}}"#, STEPS.len());

        krabb_file::save(root.join(MARKER), marker.into_bytes()).unwrap();
    });

    next(&mut || super::journal::remove(root).unwrap());
    next(&mut || super::recovery::remove(&root.join(BACKUP)).unwrap());
}

#[test]
fn every_origin_recovers_from_interruption_at_every_phase() {
    let registry = Registry::new(STEPS);

    for origin in 0..STEPS.len() {
        let phases = 4 + STEPS.len() - origin;

        for cut in 0..=phases {
            let root = origin_home(&format!("phase-{origin}-{cut}"), origin);

            interrupt(&root, origin, cut);

            let outcome = migrate(&root, &registry).unwrap();
            let case = format!("origin {origin}, cut after {cut} phase(s)");

            assert_eq!(outcome.to, STEPS.len() as u32, "{case}");
            assert_eq!(version(&root).unwrap(), STEPS.len() as u32, "{case}");

            assert_eq!(
                fs::read_to_string(root.join("data/state.txt")).unwrap(),
                "migrated",
                "{case}"
            );

            assert_eq!(
                fs::read_to_string(root.join("config.toml")).unwrap(),
                "model = 'x'",
                "{case}"
            );

            assert!(!root.join("old.toml").exists(), "{case}");
            assert!(!root.join(JOURNAL).exists() && !root.join(BACKUP).exists(), "{case}");

            let _ = fs::remove_dir_all(root);
        }
    }
}
