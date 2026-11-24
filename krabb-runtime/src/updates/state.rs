//! The state contract a plugin version declares: the schema its saved state uses and the older
//! schemas it migrates itself. The host never migrates plugin state; it previews each change and
//! refuses one the new version cannot read before anything is replaced.

use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use crate::{Entry, Manifest, PluginError};

/// How many older schemas one version may migrate from, keeping manifests small.
const ORIGINS: usize = 64;

/// Schemas of uninstalled plugins whose saved data stays in the home, so a reinstall is checked.
const KEPT: &str = "data/plugin-states.json";

const KEPT_LIMIT: u64 = 256 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct State {
    /// The schema of the state this version reads and writes, starting at 1.
    pub schema: u32,
    /// Older schemas this version migrates to `schema` itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<u32>,
}

impl State {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let mut origins = self.from.clone();

        origins.sort_unstable();
        origins.dedup();

        if self.schema == 0
            || origins.len() != self.from.len()
            || self.from.len() > ORIGINS
            || self.from.iter().any(|origin| *origin == 0 || *origin >= self.schema)
        {
            return Err(format!(
                "Plugin state must name a schema from 1 and up to {ORIGINS} distinct older \
                 schemas it migrates from."
            ));
        }

        Ok(())
    }
}

/// A change of saved state that replacing a plugin brings, which the plugin applies itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Migration {
    /// The schema recorded for the installed version, or `None` when it declared none.
    pub from: Option<u32>,
    pub to: u32,
}

impl Migration {
    /// How previews name this change.
    pub(crate) fn describe(&self) -> String {
        match self.from {
            Some(from) => format!("state: schema {from} → {} when it starts", self.to),
            None => format!("state: starts recording schema {}", self.to),
        }
    }
}

/// What installing MANIFEST over the recorded state of its plugin does: nothing, a migration
/// the new version declares, or an error when it cannot read that state. The recorded state is
/// the installed ENTRY's, or the one kept when the plugin was uninstalled.
pub(crate) fn check(
    root: &Path,
    entry: Option<&Entry>,
    manifest: &Manifest,
) -> Result<Option<Migration>, String> {
    let installed = match entry {
        Some(entry) => entry.state.clone(),
        None => kept(root).map_err(|error| error.to_string())?.remove(&manifest.id),
    };

    transition(&manifest.id, &manifest.version, installed.as_ref(), manifest.state.as_ref())
}

/// Keeps the recorded schema of an uninstalled plugin, whose saved data outlives it.
pub(crate) fn keep(root: &Path, id: &str, state: Option<&State>) -> Result<(), PluginError> {
    let mut states = kept(root)?;

    match state {
        Some(state) => states.insert(id.to_owned(), state.clone()),
        None => states.remove(id),
    };

    save(root, &states)
}

/// Drops the kept schema once the plugin is installed again and its lock entry records it.
pub(crate) fn release(root: &Path, id: &str) -> Result<(), PluginError> {
    let mut states = kept(root)?;

    if states.remove(id).is_none() {
        return Ok(());
    }

    save(root, &states)
}

/// The kept schemas; a record that cannot be read refuses instead of forgetting them.
fn kept(root: &Path) -> Result<BTreeMap<String, State>, PluginError> {
    let unreadable = || -> PluginError {
        format!(
            "The saved plugin state records in {} cannot be read. The installation was not \
             changed.",
            root.join(KEPT).display()
        )
        .into()
    };

    let Some(bytes) = krabb_file::load(root.join(KEPT), KEPT_LIMIT).map_err(|_| unreadable())?
    else {
        return Ok(BTreeMap::new());
    };

    serde_json::from_slice(&bytes).map_err(|_| unreadable())
}

fn save(root: &Path, states: &BTreeMap<String, State>) -> Result<(), PluginError> {
    if states.is_empty() {
        return match std::fs::remove_file(root.join(KEPT)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    }

    krabb_file::save(root.join(KEPT), serde_json::to_vec_pretty(states)?)?;

    Ok(())
}

/// The change from INSTALLED to NEXT for plugin ID at VERSION, or why it is refused.
pub(crate) fn transition(
    id: &str,
    version: &str,
    installed: Option<&State>,
    next: Option<&State>,
) -> Result<Option<Migration>, String> {
    let refused = |reason: String| {
        Err(format!("Plugin {id} {version} {reason}. The installation was not changed."))
    };

    match (installed, next) {
        (None, None) => Ok(None),
        (None, Some(next)) => Ok(Some(Migration { from: None, to: next.schema })),

        (Some(installed), None) => refused(format!(
            "declares no state schema, but the installed state uses schema {}",
            installed.schema
        )),

        (Some(installed), Some(next)) if installed.schema == next.schema => Ok(None),

        (Some(installed), Some(next)) if installed.schema > next.schema => refused(format!(
            "reads state schema {}, older than the installed schema {}",
            next.schema, installed.schema
        )),

        (Some(installed), Some(next)) if next.from.contains(&installed.schema) => {
            Ok(Some(Migration { from: Some(installed.schema), to: next.schema }))
        }

        (Some(installed), Some(next)) => refused(format!(
            "cannot migrate state from schema {} to {}",
            installed.schema, next.schema
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(schema: u32, from: &[u32]) -> State {
        State { schema, from: from.to_vec() }
    }

    #[test]
    fn manifests_name_a_schema_and_only_older_distinct_origins() {
        assert!(state(1, &[]).validate().is_ok());
        assert!(state(3, &[1, 2]).validate().is_ok());

        for invalid in [state(0, &[]), state(2, &[2]), state(3, &[1, 1]), state(2, &[0])] {
            assert!(invalid.validate().is_err(), "{invalid:?}");
        }

        let many = (1..=65).collect::<Vec<_>>();

        assert!(state(66, &many).validate().is_err());

        let parsed: State = toml::from_str("schema = 2\nfrom = [1]").unwrap();

        assert_eq!(parsed, state(2, &[1]));

        let later: State =
            serde_json::from_str(r#"{"schema": 2, "from": [1], "since": 3}"#).unwrap();

        assert_eq!(later, state(2, &[1]), "fields added later are ignored");
    }

    #[test]
    fn replacements_migrate_declared_origins_and_refuse_the_rest() {
        let check = |installed: Option<State>, next: Option<State>| {
            transition("memory", "0.2.0", installed.as_ref(), next.as_ref())
        };

        assert_eq!(check(None, None), Ok(None));
        assert_eq!(check(Some(state(2, &[1])), Some(state(2, &[1]))), Ok(None));
        assert_eq!(check(None, Some(state(1, &[]))), Ok(Some(Migration { from: None, to: 1 })));

        assert_eq!(
            check(Some(state(1, &[])), Some(state(3, &[1, 2]))),
            Ok(Some(Migration { from: Some(1), to: 3 }))
        );

        for (installed, next, reason) in [
            (state(3, &[]), Some(state(2, &[1])), "older than the installed schema 3"),
            (state(1, &[]), Some(state(3, &[2])), "cannot migrate state from schema 1 to 3"),
            (state(1, &[]), None, "declares no state schema"),
        ] {
            let error = check(Some(installed), next).unwrap_err();

            assert!(error.contains(reason), "{error}");
            assert!(error.starts_with("Plugin memory 0.2.0 "), "{error}");
            assert!(error.ends_with("The installation was not changed."), "{error}");
        }

        assert_eq!(
            Migration { from: Some(1), to: 2 }.describe(),
            "state: schema 1 → 2 when it starts"
        );

        assert_eq!(Migration { from: None, to: 1 }.describe(), "state: starts recording schema 1");
    }

    #[cfg(unix)]
    fn source(root: &std::path::Path, version: &str, state: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let source = root.join(format!("source-{version}"));
        let binary = source.join("bin").join(crate::plugin_name("keeper"));

        std::fs::create_dir_all(source.join("bin")).unwrap();

        std::fs::write(
            source.join("krabb-plugin.toml"),
            format!(
                "id='keeper'\nversion='{version}'\nprotocol={{major=0,minor=1}}\n\
                 capabilities=['memory']\n{state}"
            ),
        )
        .unwrap();

        let hello = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocol": {"major": 0, "minor": 1},
                "id": "keeper",
                "version": version,
                "capabilities": ["memory"],
            },
        });

        let stopped = serde_json::json!({"jsonrpc": "2.0", "id": 9999, "result": {"ok": true}});

        let script = [
            "#!/bin/sh".to_owned(),
            "while IFS= read -r line; do".into(),
            "case \"$line\" in".into(),
            format!("*hello*) printf '%s\\n' '{hello}' ;;"),
            format!("*shutdown*) printf '%s\\n' '{stopped}'; exit 0 ;;"),
            "esac".into(),
            "done".into(),
        ];

        std::fs::write(&binary, script.join("\n") + "\n").unwrap();

        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

        source
    }

    #[cfg(unix)]
    fn install(root: &std::path::Path, source: &std::path::Path) -> Result<(), String> {
        crate::link_at(
            crate::Source {
                id: "keeper".into(),
                source: Some(source.display().to_string()),
                revision: None,
                version: None,
                yes: true,
            },
            false,
            root,
        )
        .map(drop)
        .map_err(|error| error.to_string())
    }

    #[cfg(unix)]
    #[test]
    fn installs_record_the_state_schema_and_refuse_unreadable_replacements() {
        let root = crate::tests::test_root("plugin-state");

        crate::init_at(&root).unwrap();

        let recorded = |root: &std::path::Path| {
            crate::load_lock_at(root).unwrap().plugins["keeper"].state.clone()
        };

        install(&root, &source(&root, "0.1.0", "[state]\nschema = 2\nfrom = [1]\n")).unwrap();

        assert_eq!(recorded(&root), Some(state(2, &[1])));

        for (version, declared, reason) in [
            ("0.0.9", "[state]\nschema = 1\n", "older than the installed schema 2"),
            ("0.2.0", "[state]\nschema = 3\n", "cannot migrate state from schema 2 to 3"),
            ("0.2.1", "", "declares no state schema"),
        ] {
            let error = install(&root, &source(&root, version, declared)).unwrap_err();

            assert!(error.contains(reason), "{error}");
            assert_eq!(crate::load_lock_at(&root).unwrap().plugins["keeper"].version, "0.1.0");
        }

        install(&root, &source(&root, "0.3.0", "[state]\nschema = 3\nfrom = [1, 2]\n")).unwrap();

        assert_eq!(recorded(&root), Some(state(3, &[1, 2])));

        let kept = root.join(KEPT);
        let removal = crate::PluginRemove { id: "keeper".into(), yes: true };

        crate::remove_at(removal, &root).unwrap();

        assert!(kept.is_file());

        let error = install(&root, &source(&root, "0.0.8", "[state]\nschema = 1\n")).unwrap_err();

        assert!(error.contains("older than the installed schema 3"), "{error}");

        let current = source(&root, "0.3.1", "[state]\nschema = 3\nfrom = [1, 2]\n");

        install(&root, &current).unwrap();

        assert!(!kept.exists());

        let older = source(&root, "0.4.0", "[state]\nschema = 2\nfrom = [1]\n");

        for file in ["krabb-plugin.toml", &format!("bin/{}", crate::plugin_name("keeper"))] {
            std::fs::copy(older.join(file), current.join(file)).unwrap();
        }

        let error = crate::update_at(&root, true, false).unwrap_err().to_string();

        assert!(
            error.contains("reads state schema 2, older than the installed schema 3"),
            "{error}"
        );

        assert_eq!(crate::load_lock_at(&root).unwrap().plugins["keeper"].version, "0.3.1");
        assert_eq!(recorded(&root), Some(state(3, &[1, 2])));

        crate::remove_at(crate::PluginRemove { id: "keeper".into(), yes: true }, &root).unwrap();
        std::fs::write(&kept, "not json").unwrap();

        let error = install(&root, &current).unwrap_err();

        assert!(error.contains("cannot be read"), "{error}");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn older_locks_and_manifests_without_state_still_load() {
        let entry: crate::Entry = serde_json::from_value(serde_json::json!({
            "source": "index:memory",
            "version": "0.1.0",
            "protocol": {"major": 0, "minor": 1},
            "capabilities": ["memory"],
        }))
        .unwrap();

        assert_eq!(entry.state, None);
        assert!(serde_json::to_value(&entry).unwrap().get("state").is_none());

        let manifest: crate::Manifest = toml::from_str(
            "id='memory'\nversion='0.1.0'\nprotocol={major=0,minor=1}\ncapabilities=['memory']\n",
        )
        .unwrap();

        assert_eq!(manifest.state, None);
        assert!(manifest.validate().is_ok());

        let declared: crate::Manifest = toml::from_str(
            "id='memory'\nversion='0.2.0'\nprotocol={major=0,minor=1}\ncapabilities=['memory']\n\
             [state]\nschema=2\nfrom=[1]\n",
        )
        .unwrap();

        assert_eq!(declared.state, Some(state(2, &[1])));
        assert!(declared.validate().is_ok());
    }
}
