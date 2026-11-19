//! Host-owned registry of home schema migrations.

use std::{io, path::Path};

use krabb_migrations::{Registry, Step};
use tracing::{info, warn};

static STEPS: &[Step] = &[Step { name: "Record the home schema", paths: &[], apply: baseline }];

static REGISTRY: Registry = Registry::new(STEPS);

fn baseline(_: &Path) -> io::Result<()> {
    Ok(())
}

/// Restores an interrupted migration and applies pending steps; the caller owns the home.
pub(super) fn migrate(root: &Path) -> io::Result<()> {
    let plan = krabb_migrations::migrate(root, &REGISTRY)?;

    if plan.interrupted {
        warn!("Restored the Krabb home after an interrupted migration.");
    }

    if !plan.steps.is_empty() {
        info!(from = plan.from, to = plan.to, "Migrated the Krabb home schema.");
    }

    Ok(())
}

/// Pending migrations for this home, computed without changing it.
pub(super) fn plan(root: &Path) -> io::Result<krabb_migrations::Plan> {
    krabb_migrations::plan(root, &REGISTRY).map_err(Into::into)
}

/// Rejects a home with a newer or unreadable schema without changing it.
pub(super) fn check(root: &Path) -> io::Result<()> {
    krabb_migrations::plan(root, &REGISTRY).map(|_| ()).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::{fs, io};

    #[test]
    fn homes_are_stamped_and_newer_schemas_are_refused() {
        let root = std::env::temp_dir().join(format!("krabb-schema-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        fs::create_dir_all(&root).unwrap();
        super::check(&root).unwrap();
        super::migrate(&root).unwrap();

        assert_eq!(krabb_migrations::version(&root).unwrap(), super::REGISTRY.latest());

        krabb_file::save(root.join(krabb_migrations::MARKER), br#"{"schema":99}"#).unwrap();

        let error = super::check(&root).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("schema 99"));
        assert!(super::migrate(&root).is_err());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exclusive_owners_restore_interrupted_migrations_before_opening_state() {
        let root = std::env::temp_dir().join(format!("krabb-schema-owner-{}", std::process::id()));
        let backup = root.join(krabb_migrations::BACKUP);
        let _ = fs::remove_dir_all(&root);

        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("config.toml"), "original").unwrap();
        fs::write(root.join("config.toml"), "half written").unwrap();

        krabb_file::save(
            root.join(krabb_migrations::JOURNAL),
            br#"{"from":0,"to":1,"paths":[{"path":"config.toml","present":true}]}"#,
        )
        .unwrap();

        let lock = crate::daemon_lock(&root).unwrap();

        assert_eq!(fs::read_to_string(root.join("config.toml")).unwrap(), "original");
        assert!(!root.join(krabb_migrations::JOURNAL).exists());
        assert_eq!(krabb_migrations::version(&root).unwrap(), super::REGISTRY.latest());

        drop(lock);

        let _ = fs::remove_dir_all(root);
    }
}
